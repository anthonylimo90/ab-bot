//! Keeps the trading wallet's cash tradable. Since the CLOB V2 cutover the
//! exchange settles in pUSD, so USDC.e that arrives in the trading wallet is
//! wrapped into pUSD by the Collateral Onramp (1:1, and redeemable through
//! the Offramp, which is how withdrawals pay it out).
//!
//! A wrap spends one of the wallet's nonces. It is sent only under the
//! wallet's spend lock, while no withdrawal from it is unresolved and nothing
//! else it sent waits to be mined, and the lock is kept until the wrap is
//! mined and balance reads include it: a withdrawal signed behind a waiting
//! wrap would plan on USDC.e the wrap is about to take, or wait on a nonce
//! nothing fills should the wrap be dropped. Meanwhile every look sends the
//! wrap again if the node dropped it and raises its gas when the network asks
//! more or the node refuses it, until its nonce is used. After
//! [`MAX_HOLD`] the lock is let go (and said so) rather than block
//! withdrawals without end; a restart lets go of it too.

use std::sync::Arc;
use std::time::{Duration, Instant};

use auth::TradingWallet;
use polymarket_core::api::polygon::{BroadcastOutcome, SignedTransaction, BALANCE_BLOCK_LAG};
use polymarket_core::api::PolygonClient;
use sqlx::PgPool;
use tracing::{info, warn};

use crate::withdrawals::{chain_error_message, SpendLock, UNRESOLVED_STATUSES};

/// How often the trading wallet is checked for USDC.e to wrap.
const SWEEP_INTERVAL: Duration = Duration::from_secs(300);
/// How often a wrap still unmined is looked after.
const FOLLOW_UP_INTERVAL: Duration = Duration::from_secs(30);
/// Less USDC.e than this (one dollar) is left as it is.
const MIN_WRAP_UNITS: u128 = 1_000_000;
/// How often a wrap is checked for a receipt while a look waits for it.
const RECEIPT_POLL: Duration = Duration::from_millis(if cfg!(test) { 5 } else { 2000 });
/// How many polls a new wrap is waited for, and a waiting one on a later look.
const NEW_WRAP_POLLS: u32 = 45;
const FOLLOW_UP_POLLS: u32 = 10;
/// Every this many polls a waiting wrap's nonce is checked, the wrap sent
/// again if the node no longer knows it, and its gas raised if underpriced.
const CHECK_EVERY_POLLS: u32 = 5;
/// How long a wrap's nonce must have been found used, without one of its
/// receipts, before it counts as taken by another transaction (a receipt can
/// lag the nonce on another node).
const NONCE_USED_CONFIRM: Duration = if cfg!(test) {
    Duration::from_millis(10)
} else {
    Duration::from_secs(120)
};
/// The most a waiting wrap's gas price is raised to, as a multiple of what
/// the network asks.
const MAX_GAS_OVER_NETWORK: u128 = 3;
/// How long the spend lock is kept for a wrap that is not mined.
const MAX_HOLD: Duration = Duration::from_secs(1800);
/// How long the sweeper may be kept from wrapping before it says so.
const STARVED_AFTER: Duration = Duration::from_secs(3600);

/// What one look at the wallet did.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WrapOutcome {
    /// Another spend from the wallet is in progress or unresolved; nothing sent.
    Busy,
    /// Too little USDC.e to wrap.
    Nothing,
    /// This many USDC.e base units were wrapped into pUSD.
    Wrapped(u128),
    /// The wrap was sent but is not mined yet; later looks follow it up.
    Unmined,
    /// Another transaction used the wrap's nonce; the USDC.e was not wrapped.
    Superseded,
}

/// A wrap sent and not yet settled.
#[derive(Debug, Clone)]
struct PendingWrap {
    nonce: u64,
    units: u128,
    sent_at: Instant,
    /// The transaction last signed at the nonce.
    latest: SignedTransaction,
    /// Every hash signed at the nonce: whichever is mined settles the wrap.
    hashes: Vec<String>,
    /// Since when checks have found the nonce used and none of its receipts.
    nonce_used_since: Option<Instant>,
}

/// Wraps the wallet's idle USDC.e, remembering a wrap until it settles.
#[derive(Default)]
pub(crate) struct Sweeper {
    pending: Option<PendingWrap>,
    /// The wallet's spend lock, kept while a wrap waits to be mined.
    lock: Option<SpendLock>,
    /// Since when every look has found the wallet busy.
    busy_since: Option<Instant>,
}

impl Sweeper {
    /// Follow up a waiting wrap, or wrap the wallet's USDC.e if it holds any
    /// and nothing else spends from it now.
    pub(crate) async fn look(
        &mut self,
        pool: &PgPool,
        client: &PolygonClient,
        wallet: &TradingWallet,
    ) -> anyhow::Result<WrapOutcome> {
        let address = wallet.address_string();
        // A held lock whose session no longer answers holds nothing.
        if let Some(lock) = self.lock.as_mut() {
            if !lock.is_alive().await {
                warn!("The spend lock's database session stopped answering; taking it again");
                if let Some(lock) = self.lock.take() {
                    lock.abandon();
                }
            }
        }
        let outcome = if let Some(wrap) = &self.pending {
            // Keep (or take back) the lock while it waits, within the bound.
            if self.lock.is_none() && wrap.sent_at.elapsed() < MAX_HOLD {
                self.lock = SpendLock::try_acquire(pool, &address).await?;
            }
            self.follow_up(client, wallet, FOLLOW_UP_POLLS).await
        } else {
            let Some(mut lock) = SpendLock::try_acquire(pool, &address).await? else {
                return Ok(self.busy(client, &address).await);
            };
            let outcome = self.wrap(&mut lock, client, wallet).await;
            self.lock = Some(lock);
            outcome
        };
        match &self.pending {
            None => self.release().await,
            Some(wrap) if wrap.sent_at.elapsed() >= MAX_HOLD && self.lock.is_some() => {
                warn!(
                    nonce = wrap.nonce,
                    "A wrap has waited half an hour; withdrawals are no longer held back for it"
                );
                self.release().await;
            }
            Some(_) => {}
        }
        if matches!(outcome, Ok(WrapOutcome::Busy)) {
            return Ok(self.busy(client, &address).await);
        }
        self.busy_since = None;
        outcome
    }

    /// Whether a wrap is waiting to be mined.
    pub(crate) fn waiting(&self) -> bool {
        self.pending.is_some()
    }

    async fn release(&mut self) {
        if let Some(lock) = self.lock.take() {
            // A session that does not answer is closed when the release is
            // given up, which frees its locks as well.
            let _ = tokio::time::timeout(Duration::from_secs(10), lock.release()).await;
        }
    }

    /// A look that could not wrap; after long enough, said so if USDC.e
    /// is waiting for it.
    async fn busy(&mut self, client: &PolygonClient, address: &str) -> WrapOutcome {
        let since = *self.busy_since.get_or_insert_with(Instant::now);
        if since.elapsed() >= STARVED_AFTER {
            if matches!(
                client.get_usdc_balance_units(address).await,
                Ok(units) if units >= MIN_WRAP_UNITS
            ) {
                warn!(
                    minutes = since.elapsed().as_secs() / 60,
                    "The trading wallet's USDC.e has waited long to be wrapped: a withdrawal \
                     from it is unresolved or a transaction it sent is still pending"
                );
            }
            self.busy_since = Some(Instant::now());
        }
        WrapOutcome::Busy
    }

    async fn wrap(
        &mut self,
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

        let signed = client
            .prepare_wrap(wallet.signer(), units, mined)
            .await
            .map_err(chain)?;
        if let Ok(BroadcastOutcome::Rejected(reason)) =
            client.broadcast(&signed.raw, &signed.tx_hash).await
        {
            anyhow::bail!("the node refused the wrap: {reason}");
        }
        info!(wallet = %address, units, tx_hash = %signed.tx_hash, "Wrapping USDC.e into pUSD");
        self.pending = Some(PendingWrap {
            nonce: mined,
            units,
            sent_at: Instant::now(),
            hashes: vec![signed.tx_hash.clone()],
            latest: signed,
            nonce_used_since: None,
        });
        self.follow_up(client, wallet, NEW_WRAP_POLLS).await
    }

    /// Wait up to `polls` for the pending wrap to settle, sending it again
    /// should the node drop it and raising its gas when underpriced.
    async fn follow_up(
        &mut self,
        client: &PolygonClient,
        wallet: &TradingWallet,
        polls: u32,
    ) -> anyhow::Result<WrapOutcome> {
        let Some(mut wrap) = self.pending.take() else {
            return Ok(WrapOutcome::Nothing);
        };
        for poll in 1..=polls {
            tokio::time::sleep(RECEIPT_POLL).await;
            for hash in &wrap.hashes {
                match client.transaction_receipt(hash).await {
                    Ok(Some(receipt)) => {
                        anyhow::ensure!(receipt.success, "the wrap {hash} reverted");
                        if let Some(block) = receipt.block_number {
                            await_balances_after(client, block).await;
                        }
                        return Ok(WrapOutcome::Wrapped(wrap.units));
                    }
                    Ok(None) => {}
                    Err(error) => {
                        warn!(error = %chain_error_message(&error), "Could not read a wrap's receipt")
                    }
                }
            }
            if poll % CHECK_EVERY_POLLS != 0 {
                continue;
            }
            // Its nonce used, on checks that each read every one of its
            // receipts and found none, for long enough that no lagging node
            // explains it: another transaction took it.
            match client.nonces(&wallet.address()).await {
                Ok((mined, _)) if mined > wrap.nonce => {
                    let mut none_of_ours = true;
                    for hash in &wrap.hashes {
                        none_of_ours &= matches!(client.transaction_receipt(hash).await, Ok(None));
                    }
                    if none_of_ours {
                        let since = *wrap.nonce_used_since.get_or_insert_with(Instant::now);
                        if since.elapsed() >= NONCE_USED_CONFIRM {
                            warn!(
                                nonce = wrap.nonce,
                                "A wrap's nonce was used by another transaction"
                            );
                            return Ok(WrapOutcome::Superseded);
                        }
                    }
                    continue;
                }
                Ok(_) => wrap.nonce_used_since = None,
                Err(error) => {
                    warn!(error = %chain_error_message(&error), "Could not read the wallet's nonce")
                }
            }
            // Dropped by the node: send it again. Refused again: raise its gas.
            let mut refused = false;
            if matches!(
                client.transaction_known(&wrap.latest.tx_hash).await,
                Ok(false)
            ) {
                // The node's refusal, not a transport failure.
                refused = matches!(
                    client
                        .broadcast(&wrap.latest.raw, &wrap.latest.tx_hash)
                        .await,
                    Ok(BroadcastOutcome::Rejected(_) | BroadcastOutcome::Uncertain(_))
                );
            }
            // A replacement must pay a tenth more; it never pays more than
            // [`MAX_GAS_OVER_NETWORK`] times what the network asks.
            let min_gas_price = wrap.latest.gas_price + wrap.latest.gas_price / 10 + 1;
            match client.gas_price_wei().await {
                Ok(network)
                    if (network > wrap.latest.gas_price || refused)
                        && min_gas_price <= network.saturating_mul(MAX_GAS_OVER_NETWORK) =>
                {
                    match client
                        .prepare_replacement(
                            wallet.signer(),
                            &wrap.latest.raw,
                            wrap.nonce,
                            min_gas_price,
                        )
                        .await
                    {
                        Ok(replacement) => {
                            let outcome = client
                                .broadcast(&replacement.raw, &replacement.tx_hash)
                                .await;
                            info!(
                                gas_price = replacement.gas_price,
                                ?outcome,
                                "Raised the gas price of a waiting wrap"
                            );
                            // Kept whatever the node said: should it hold it
                            // after all its receipt settles the wrap, and the
                            // next raise starts from it.
                            wrap.hashes.push(replacement.tx_hash.clone());
                            wrap.latest = replacement;
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
        warn!(nonce = wrap.nonce, hashes = ?wrap.hashes, "A wrap is not mined yet; it is followed up");
        self.pending = Some(wrap);
        Ok(WrapOutcome::Unmined)
    }
}

fn chain(error: polymarket_core::Error) -> anyhow::Error {
    anyhow::anyhow!(chain_error_message(&error))
}

/// Wait (briefly) until the head is far enough past `block` that balances
/// read [`BALANCE_BLOCK_LAG`] blocks behind it include what was mined there,
/// so a withdrawal planned right after a wrap does not draw on USDC.e the wrap
/// already took.
async fn await_balances_after(client: &PolygonClient, block: u64) {
    for _ in 0..NEW_WRAP_POLLS {
        match client.get_block_number().await {
            Ok(head) if head > block + BALANCE_BLOCK_LAG => return,
            _ => tokio::time::sleep(RECEIPT_POLL).await,
        }
    }
}

/// Wrap the trading wallet's USDC.e now and then every [`SWEEP_INTERVAL`]
/// (every [`FOLLOW_UP_INTERVAL`] while a wrap waits to be mined), telling
/// the exchange to re-read the wallet's collateral after a wrap.
pub fn spawn_collateral_sweeper(
    pool: PgPool,
    client: PolygonClient,
    wallet: TradingWallet,
    executor: Arc<trading_engine::OrderExecutor>,
) {
    tokio::spawn(async move {
        let mut sweeper = Sweeper::default();
        loop {
            match sweeper.look(&pool, &client, &wallet).await {
                Ok(WrapOutcome::Wrapped(units)) => {
                    info!(units, "Wrapped the trading wallet's USDC.e into pUSD");
                    if let Err(error) = executor.refresh_clob_allowance_cache().await {
                        warn!(%error, "Could not refresh the exchange's view of the wallet");
                    }
                }
                Ok(_) => {}
                Err(error) => warn!(%error, "Could not wrap the trading wallet's USDC.e"),
            }
            let interval = if sweeper.waiting() {
                FOLLOW_UP_INTERVAL
            } else {
                SWEEP_INTERVAL
            };
            tokio::time::sleep(interval).await;
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
            Sweeper::default()
                .look(&pool, &client, &wallet)
                .await
                .unwrap(),
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
            Sweeper::default()
                .look(&pool, &client, &wallet)
                .await
                .unwrap(),
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
            Sweeper::default()
                .look(&pool, &client, &wallet)
                .await
                .unwrap(),
            WrapOutcome::Busy
        );
        held.release().await;

        assert_eq!(
            Sweeper::default()
                .look(&pool, &client, &wallet)
                .await
                .unwrap(),
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
            Sweeper::default()
                .look(&pool, &client, &wallet)
                .await
                .unwrap(),
            WrapOutcome::Nothing
        );
    }

    #[sqlx::test(migrations = false)]
    async fn a_waiting_wrap_is_signed_again_at_its_nonce_and_a_doomed_one_never_sent(pool: PgPool) {
        let (user, workspace) = setup(&pool).await;
        let chain = Arc::new(Mutex::new(Chain {
            usdc_units: 5_000_000,
            pol_wei: 1_000_000_000_000_000_000,
            gas_estimate: None,
            ..Default::default()
        }));
        let client = mock_node(chain.clone()).await;
        let wallet = wallet();
        let mut sweeper = Sweeper::default();
        // The node expects the wrap to revert (no allowance, a paused Onramp).
        assert!(sweeper.look(&pool, &client, &wallet).await.is_err());
        assert!(chain.lock().unwrap().broadcasts.is_empty());
        assert!(!sweeper.waiting());

        // Never mined, while the network asks more than it pays.
        {
            let mut chain = chain.lock().unwrap();
            chain.gas_estimate = Some(90_000);
            chain.gas_price = 100_000_000_000;
        }
        let chain_for_bump = chain.clone();
        let raise = tokio::spawn(async move {
            while chain_for_bump.lock().unwrap().broadcasts.is_empty() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            chain_for_bump.lock().unwrap().gas_price = 200_000_000_000;
        });
        assert_eq!(
            sweeper.look(&pool, &client, &wallet).await.unwrap(),
            WrapOutcome::Unmined
        );
        raise.await.unwrap();
        assert!(sweeper.waiting());
        // While it waits no withdrawal can be signed behind it.
        assert!(SpendLock::try_acquire(&pool, &wallet.address_string())
            .await
            .unwrap()
            .is_none());
        let broadcasts = chain.lock().unwrap().broadcasts.clone();
        assert_eq!(broadcasts.len(), 2);
        for raw in &broadcasts {
            assert_eq!(legacy_call(raw), legacy_call(&broadcasts[0]));
            assert_eq!(
                polymarket_core::api::polygon::legacy_nonce_and_gas_price(raw).map(|(n, _)| n),
                Some(0)
            );
        }

        // A withdrawal signed behind it all the same (the lock let go after
        // half an hour, or a restart); the node then drops the wrap. The next
        // look sends it again, unresolved withdrawal or not.
        insert(
            &pool,
            user,
            workspace,
            Row {
                wallet: &wallet.address_string().to_lowercase(),
                amount: 1,
                status: "submitted",
                tx_hash: Some("0xw"),
                nonce: Some(1),
                journaled: true,
                age_minutes: 0,
            },
        )
        .await;
        let latest = broadcasts[1].clone();
        let latest_hash = format!(
            "{:#x}",
            alloy_primitives::keccak256(hex::decode(latest.trim_start_matches("0x")).unwrap())
        );
        chain.lock().unwrap().known.clear();
        assert_eq!(
            sweeper.look(&pool, &client, &wallet).await.unwrap(),
            WrapOutcome::Unmined
        );
        assert_eq!(chain.lock().unwrap().broadcasts.last(), Some(&latest));

        // Mined at last: settled, and forgotten.
        chain.lock().unwrap().mine(&latest_hash, true);
        assert_eq!(
            sweeper.look(&pool, &client, &wallet).await.unwrap(),
            WrapOutcome::Wrapped(5_000_000)
        );
        assert!(!sweeper.waiting());
        // Settled: the wallet is free again.
        let free = SpendLock::try_acquire(&pool, &wallet.address_string())
            .await
            .unwrap();
        assert!(free.is_some());
        free.unwrap().release().await;
    }

    #[sqlx::test(migrations = false)]
    async fn a_held_lock_whose_session_died_is_taken_again(pool: PgPool) {
        setup(&pool).await;
        let chain = Arc::new(Mutex::new(Chain {
            usdc_units: 5_000_000,
            pol_wei: 1_000_000_000_000_000_000,
            ..Default::default()
        }));
        let client = mock_node(chain.clone()).await;
        let wallet = wallet();
        let mut sweeper = Sweeper::default();
        assert_eq!(
            sweeper.look(&pool, &client, &wallet).await.unwrap(),
            WrapOutcome::Unmined
        );
        // The session holding the lock is cut off (a proxy, a timeout).
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(sweeper.lock.as_mut().unwrap().conn())
            .await
            .unwrap();
        sqlx::query("SELECT pg_terminate_backend($1)")
            .bind(pid)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            sweeper.look(&pool, &client, &wallet).await.unwrap(),
            WrapOutcome::Unmined
        );
        // Held again, by a live session: withdrawals still wait for the wrap.
        let mut lock = sweeper.lock.take().unwrap();
        assert!(lock.is_alive().await);
        assert!(SpendLock::try_acquire(&pool, &wallet.address_string())
            .await
            .unwrap()
            .is_none());
        lock.release().await;
    }

    #[sqlx::test(migrations = false)]
    async fn a_wrap_the_node_keeps_refusing_is_signed_again_for_more_gas(pool: PgPool) {
        setup(&pool).await;
        let chain = Arc::new(Mutex::new(Chain {
            usdc_units: 5_000_000,
            pol_wei: 1_000_000_000_000_000_000,
            send_error: Some("transaction underpriced".into()),
            ..Default::default()
        }));
        let client = mock_node(chain.clone()).await;
        let mut sweeper = Sweeper::default();
        assert_eq!(
            sweeper.look(&pool, &client, &wallet()).await.unwrap(),
            WrapOutcome::Unmined
        );
        // The network asks no more, yet each refusal is met with a higher
        // price at the same nonce, up to three times what the network asks.
        let prices: Vec<u128> = chain
            .lock()
            .unwrap()
            .broadcasts
            .iter()
            .map(|raw| {
                let (nonce, price) =
                    polymarket_core::api::polygon::legacy_nonce_and_gas_price(raw).unwrap();
                assert_eq!(nonce, 0);
                price
            })
            .collect();
        assert!(
            prices.windows(2).any(|pair| pair[1] > pair[0]),
            "{prices:?}"
        );
        assert!(prices
            .iter()
            .all(|price| *price <= 3 * 30_000_000_000 * 11 / 10 + 1));
    }

    #[sqlx::test(migrations = false)]
    async fn a_wrap_is_given_up_only_when_its_nonce_stays_used(pool: PgPool) {
        setup(&pool).await;
        let chain = Arc::new(Mutex::new(Chain {
            usdc_units: 5_000_000,
            pol_wei: 1_000_000_000_000_000_000,
            ..Default::default()
        }));
        let client = mock_node(chain.clone()).await;
        let mut sweeper = Sweeper::default();
        assert_eq!(
            sweeper.look(&pool, &client, &wallet()).await.unwrap(),
            WrapOutcome::Unmined
        );
        // Its nonce now reads as used, but one look only: not yet given up.
        {
            let mut chain = chain.lock().unwrap();
            chain.latest_nonce = 1;
            chain.pending_nonce = 1;
        }
        let wrap = sweeper.pending.clone().unwrap();
        sweeper.pending = Some(PendingWrap {
            nonce_used_since: None,
            ..wrap
        });
        let outcome = sweeper
            .follow_up(&client, &wallet(), CHECK_EVERY_POLLS)
            .await
            .unwrap();
        assert_eq!(outcome, WrapOutcome::Unmined);
        assert!(sweeper.pending.as_ref().unwrap().nonce_used_since.is_some());
        // Still used a while later: taken by another transaction.
        tokio::time::sleep(NONCE_USED_CONFIRM).await;
        assert_eq!(
            sweeper
                .follow_up(&client, &wallet(), CHECK_EVERY_POLLS)
                .await
                .unwrap(),
            WrapOutcome::Superseded
        );
        sweeper.release().await;
    }

    #[sqlx::test(migrations = false)]
    async fn a_wrap_whose_nonce_another_transaction_took_is_given_up(pool: PgPool) {
        setup(&pool).await;
        let chain = Arc::new(Mutex::new(Chain {
            usdc_units: 5_000_000,
            pol_wei: 1_000_000_000_000_000_000,
            // A node behind: the nonce it reports is already used.
            send_error: Some("nonce too low".into()),
            ..Default::default()
        }));
        let client = mock_node(chain.clone()).await;
        chain.lock().unwrap().latest_nonce = 0;
        let mut sweeper = Sweeper::default();
        let chain_for_nonce = chain.clone();
        let advance = tokio::spawn(async move {
            while chain_for_nonce.lock().unwrap().broadcasts.is_empty() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
            let mut chain = chain_for_nonce.lock().unwrap();
            chain.latest_nonce = 1;
            chain.pending_nonce = 1;
        });
        assert_eq!(
            sweeper.look(&pool, &client, &wallet()).await.unwrap(),
            WrapOutcome::Superseded
        );
        advance.await.unwrap();
        assert!(!sweeper.waiting());
    }
}
