//! Keeps the trading wallet's cash tradable. Since the CLOB V2 cutover the
//! exchange settles in pUSD, so USDC.e that arrives in the trading wallet is
//! wrapped into pUSD by the Collateral Onramp (1:1, and redeemable through
//! the Offramp, which is how withdrawals pay it out).
//!
//! A wrap spends one of the wallet's nonces. It is sent only under the
//! wallet's spend lock, while no withdrawal from it is unresolved and nothing
//! else it sent waits to be mined, and the lock is held until the wrap is
//! mined (its gas raised meanwhile when the network asks more): no withdrawal
//! is signed behind a wrap that might yet be dropped, which would leave it
//! waiting on a nonce nothing fills.

use std::sync::Arc;
use std::time::Duration;

use auth::TradingWallet;
use polymarket_core::api::polygon::BroadcastOutcome;
use polymarket_core::api::PolygonClient;
use sqlx::PgPool;
use tracing::{info, warn};

use crate::withdrawals::{chain_error_message, SpendLock, UNRESOLVED_STATUSES};

/// How often the trading wallet is checked for USDC.e to wrap.
const SWEEP_INTERVAL: Duration = Duration::from_secs(300);
/// Less USDC.e than this (one dollar) is left as it is.
const MIN_WRAP_UNITS: u128 = 1_000_000;
/// How often a sent wrap is checked for a receipt, and how many times.
const RECEIPT_POLL: Duration = Duration::from_millis(if cfg!(test) { 5 } else { 2000 });
const RECEIPT_POLLS: u32 = 150;
/// Every this many polls, an unmined wrap paying less than the network asks
/// is signed again at its nonce for more.
const BUMP_EVERY_POLLS: u32 = 30;

/// What one look at the wallet did.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WrapOutcome {
    /// Another spend from the wallet is in progress or unresolved; nothing sent.
    Busy,
    /// Too little USDC.e to wrap.
    Nothing,
    /// This many USDC.e base units were wrapped into pUSD.
    Wrapped(u128),
    /// The wrap was sent but not mined in time; the chain settles it.
    Unmined,
}

/// Wrap the wallet's USDC.e into pUSD, if it holds any and nothing else
/// spends from it now.
pub(crate) async fn wrap_idle_usdc(
    pool: &PgPool,
    client: &PolygonClient,
    wallet: &TradingWallet,
) -> anyhow::Result<WrapOutcome> {
    let address = wallet.address_string();
    let Some(mut lock) = SpendLock::try_acquire(pool, &address).await? else {
        return Ok(WrapOutcome::Busy);
    };
    let outcome = wrap_under_lock(&mut lock, client, wallet).await;
    lock.release().await;
    outcome
}

async fn wrap_under_lock(
    lock: &mut SpendLock,
    client: &PolygonClient,
    wallet: &TradingWallet,
) -> anyhow::Result<WrapOutcome> {
    let address = wallet.address_string();
    let unresolved: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM wallet_withdrawals \
         WHERE LOWER(wallet_address) = LOWER($1) AND status = ANY($2))",
    )
    .bind(&address)
    .bind(&UNRESOLVED_STATUSES[..])
    .fetch_one(lock.conn())
    .await?;
    if unresolved {
        return Ok(WrapOutcome::Busy);
    }
    let chain = |error: polymarket_core::Error| anyhow::anyhow!(chain_error_message(&error));
    let (mined, pending) = client.nonces(&wallet.address()).await.map_err(chain)?;
    if mined != pending {
        return Ok(WrapOutcome::Busy);
    }
    let units = client
        .get_usdc_balance_units(&address)
        .await
        .map_err(chain)?;
    if units < MIN_WRAP_UNITS {
        return Ok(WrapOutcome::Nothing);
    }

    let mut latest = client
        .prepare_wrap(wallet.signer(), units, mined)
        .await
        .map_err(chain)?;
    if let Ok(BroadcastOutcome::Rejected(reason)) =
        client.broadcast(&latest.raw, &latest.tx_hash).await
    {
        anyhow::bail!("the node refused the wrap: {reason}");
    }
    info!(wallet = %address, units, tx_hash = %latest.tx_hash, "Wrapping USDC.e into pUSD");
    // Every hash sent at the nonce: whichever is mined settles the wrap.
    let mut hashes = vec![latest.tx_hash.clone()];
    for poll in 1..=RECEIPT_POLLS {
        tokio::time::sleep(RECEIPT_POLL).await;
        for hash in &hashes {
            match client.transaction_receipt(hash).await {
                Ok(Some(receipt)) if receipt.success => return Ok(WrapOutcome::Wrapped(units)),
                Ok(Some(_)) => anyhow::bail!("the wrap {hash} reverted"),
                Ok(None) => {}
                Err(error) => {
                    warn!(error = %chain_error_message(&error), "Could not read a wrap's receipt")
                }
            }
        }
        if poll % BUMP_EVERY_POLLS == 0 {
            match client.gas_price_wei().await {
                Ok(network) if network > latest.gas_price => {
                    let min_gas_price = latest.gas_price + latest.gas_price / 10 + 1;
                    match client
                        .prepare_replacement(wallet.signer(), &latest.raw, mined, min_gas_price)
                        .await
                    {
                        Ok(replacement) => {
                            let _ = client
                                .broadcast(&replacement.raw, &replacement.tx_hash)
                                .await;
                            info!(wallet = %address, gas_price = replacement.gas_price, "Raised the gas price of a waiting wrap");
                            hashes.push(replacement.tx_hash.clone());
                            latest = replacement;
                        }
                        Err(error) => {
                            warn!(error = %chain_error_message(&error), "Could not sign a wrap's replacement")
                        }
                    }
                }
                Ok(_) => {}
                Err(error) => {
                    warn!(error = %chain_error_message(&error), "Could not read the gas price")
                }
            }
        }
    }
    warn!(wallet = %address, ?hashes, "A wrap was not mined in time; withdrawals may wait behind it");
    Ok(WrapOutcome::Unmined)
}

/// Wrap the trading wallet's USDC.e now and then every [`SWEEP_INTERVAL`],
/// telling the exchange to re-read the wallet's collateral after a wrap.
pub fn spawn_collateral_sweeper(
    pool: PgPool,
    client: PolygonClient,
    wallet: TradingWallet,
    executor: Arc<trading_engine::OrderExecutor>,
) {
    tokio::spawn(async move {
        loop {
            match wrap_idle_usdc(&pool, &client, &wallet).await {
                Ok(WrapOutcome::Wrapped(units)) => {
                    info!(units, "Wrapped the trading wallet's USDC.e into pUSD");
                    if let Err(error) = executor.refresh_clob_allowance_cache().await {
                        warn!(%error, "Could not refresh the exchange's view of the wallet");
                    }
                }
                Ok(_) => {}
                Err(error) => warn!(%error, "Could not wrap the trading wallet's USDC.e"),
            }
            tokio::time::sleep(SWEEP_INTERVAL).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::withdrawals::tests::{insert, mock_node, setup, Chain, Row};
    use polymarket_core::api::polygon::legacy_call;
    use std::sync::Mutex;

    fn wallet() -> TradingWallet {
        TradingWallet::from_private_key(&format!("{:064x}", 1)).unwrap()
    }

    #[sqlx::test(migrations = false)]
    async fn idle_usdc_is_wrapped_for_the_wallet_only_when_nothing_else_spends(pool: PgPool) {
        let (user, workspace) = setup(&pool).await;
        let chain = Arc::new(Mutex::new(Chain {
            usdc_units: 25_000_000,
            pol_wei: 1_000_000_000_000_000_000,
            latest_nonce: 6,
            pending_nonce: 7,
            mine_on_send: true,
            ..Default::default()
        }));
        let client = mock_node(chain.clone()).await;
        let wallet = wallet();

        // Something the wallet sent still waits to be mined.
        assert_eq!(
            wrap_idle_usdc(&pool, &client, &wallet).await.unwrap(),
            WrapOutcome::Busy
        );
        chain.lock().unwrap().pending_nonce = 6;

        // A withdrawal from it is unresolved.
        let id = insert(
            &pool,
            user,
            workspace,
            Row {
                wallet: &wallet.address_string().to_lowercase(),
                amount: 1,
                status: "submitted",
                tx_hash: Some("0xw"),
                nonce: Some(5),
                journaled: true,
                age_minutes: 0,
            },
        )
        .await;
        assert_eq!(
            wrap_idle_usdc(&pool, &client, &wallet).await.unwrap(),
            WrapOutcome::Busy
        );
        sqlx::query("UPDATE wallet_withdrawals SET status = 'confirmed' WHERE id = $1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();

        // A withdrawal is being decided: the spend lock is held.
        let held = SpendLock::acquire(&pool, &wallet.address_string())
            .await
            .unwrap();
        assert_eq!(
            wrap_idle_usdc(&pool, &client, &wallet).await.unwrap(),
            WrapOutcome::Busy
        );
        held.release().await;

        assert_eq!(
            wrap_idle_usdc(&pool, &client, &wallet).await.unwrap(),
            WrapOutcome::Wrapped(25_000_000)
        );
        let raw = chain.lock().unwrap().broadcasts[0].clone();
        let (to, _, input) = legacy_call(&raw).unwrap();
        assert_eq!(
            to,
            polymarket_core::signing::COLLATERAL_ONRAMP_ADDRESS.to_ascii_lowercase()
        );
        // wrap(USDC.e, the wallet itself, its whole balance)
        let input = hex::encode(input);
        assert!(input.starts_with(&hex::encode(
            &alloy_primitives::keccak256("wrap(address,address,uint256)".as_bytes())[..4]
        )));
        assert!(input.contains(
            &wallet
                .address_string()
                .trim_start_matches("0x")
                .to_ascii_lowercase()
        ));
        assert_eq!(
            polymarket_core::api::polygon::legacy_nonce_and_gas_price(&raw).map(|(n, _)| n),
            Some(6)
        );

        // Too little to bother with.
        chain.lock().unwrap().usdc_units = 999_999;
        assert_eq!(
            wrap_idle_usdc(&pool, &client, &wallet).await.unwrap(),
            WrapOutcome::Nothing
        );
    }

    #[sqlx::test(migrations = false)]
    async fn a_waiting_wrap_is_signed_again_at_its_nonce_and_a_doomed_one_never_sent(pool: PgPool) {
        setup(&pool).await;
        let chain = Arc::new(Mutex::new(Chain {
            usdc_units: 5_000_000,
            pol_wei: 1_000_000_000_000_000_000,
            gas_estimate: None,
            ..Default::default()
        }));
        let client = mock_node(chain.clone()).await;
        let wallet = wallet();
        // The node expects the wrap to revert (no allowance, a paused Onramp).
        assert!(wrap_idle_usdc(&pool, &client, &wallet).await.is_err());
        assert!(chain.lock().unwrap().broadcasts.is_empty());

        // Never mined, while the network asks more than it pays.
        {
            let mut chain = chain.lock().unwrap();
            chain.gas_estimate = Some(90_000);
            chain.gas_price = 100_000_000_000;
        }
        let client_for_bump = client.clone();
        let chain_for_bump = chain.clone();
        let wrapping =
            tokio::spawn(async move { wrap_idle_usdc(&pool, &client_for_bump, &wallet).await });
        // Let the first send happen, then raise the network's price.
        while chain_for_bump.lock().unwrap().broadcasts.is_empty() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        chain_for_bump.lock().unwrap().gas_price = 200_000_000_000;
        assert_eq!(wrapping.await.unwrap().unwrap(), WrapOutcome::Unmined);
        let broadcasts = chain.lock().unwrap().broadcasts.clone();
        assert!(broadcasts.len() > 1);
        for raw in &broadcasts {
            assert_eq!(legacy_call(raw), legacy_call(&broadcasts[0]));
            assert_eq!(
                polymarket_core::api::polygon::legacy_nonce_and_gas_price(raw).map(|(n, _)| n),
                Some(0)
            );
        }
    }
}
