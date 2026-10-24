//! The bot's on-chain money paths, run for real against a local fork of
//! Polygon mainnet: approvals, the collateral sweeper's wrap, USDC.e and
//! pUSD withdrawals, and a fee-bump replacement. Every transaction is
//! signed by the bot's own code and broadcast to the fork, where it runs
//! against the deployed contracts; nothing reaches the real chain.
//!
//! Start a fork node, then point the tests at it:
//!
//! ```bash
//! # see scripts/polygon-fork: an archive Polygon node, through the relay
//! cd scripts/polygon-fork && npm install && npm run fork
//! FORK_RPC_URL=http://127.0.0.1:8545 \
//!   cargo test -p polymarket-core --test fork_money_paths -- --ignored --test-threads=1
//! ```
//!
//! The node must accept Hardhat's `hardhat_*` and `evm_*` test methods.

use alloy_primitives::Address;
use alloy_signer_local::PrivateKeySigner;
use polymarket_core::api::approvals::ensure_polymarket_approvals;
use polymarket_core::api::polygon::{
    legacy_nonce_and_gas_price, BroadcastOutcome, PolygonClient, WithdrawalSource, CALL_WOULD_FAIL,
};
use polymarket_core::signing::{CTF_ADDRESS, USDC_ADDRESS};

/// Hardhat/Anvil development account 1: funded with POL on the fork, public
/// and so never funded on mainnet.
const TEST_KEY: &str = "59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d";

const USDC: u128 = 1_000_000;

fn fork_url() -> String {
    std::env::var("FORK_RPC_URL").expect("FORK_RPC_URL names the fork node")
}

async fn rpc(method: &str, params: serde_json::Value) -> serde_json::Value {
    let answer: serde_json::Value = reqwest::Client::new()
        .post(fork_url())
        .json(&serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }))
        .send()
        .await
        .expect("fork reachable")
        .json()
        .await
        .expect("fork json");
    if let Some(error) = answer.get("error") {
        panic!("{method}: {error}");
    }
    answer["result"].clone()
}

/// One block at a time: Hardhat keeps no state for the blocks inside a
/// range mined at once, and balances are read a few blocks back.
async fn mine(blocks: u64) {
    for _ in 0..blocks {
        rpc("evm_mine", serde_json::json!([])).await;
    }
}

/// Move `units` of USDC.e to `to` from the Conditional Tokens contract,
/// which holds the collateral behind every outcome token.
async fn fund_usdc(to: Address, units: u128) {
    rpc(
        "hardhat_impersonateAccount",
        serde_json::json!([CTF_ADDRESS]),
    )
    .await;
    rpc(
        "hardhat_setBalance",
        serde_json::json!([CTF_ADDRESS, "0x3635c9adc5dea00000"]),
    )
    .await;
    let data = format!(
        "0xa9059cbb{:0>64}{:064x}",
        hex::encode(to.as_slice()),
        units
    );
    let hash = rpc(
        "eth_sendTransaction",
        serde_json::json!([{ "from": CTF_ADDRESS, "to": USDC_ADDRESS, "data": data }]),
    )
    .await;
    rpc(
        "hardhat_stopImpersonatingAccount",
        serde_json::json!([CTF_ADDRESS]),
    )
    .await;
    let receipt = rpc("eth_getTransactionReceipt", serde_json::json!([hash])).await;
    assert_eq!(receipt["status"], "0x1", "funding transfer failed");
}

/// Broadcast `raw` and wait for it to be mined successfully.
async fn send_and_confirm(client: &PolygonClient, raw: &str, hash: &str) {
    match client.broadcast(raw, hash).await.expect("broadcast") {
        BroadcastOutcome::Accepted => {}
        other => panic!("broadcast of {hash}: {other:?}"),
    }
    mine(1).await;
    let mined = client
        .transaction_receipt(hash)
        .await
        .expect("receipt")
        .expect("mined");
    assert!(mined.success, "{hash} reverted");
}

#[tokio::test]
#[ignore = "needs a local Polygon fork node (FORK_RPC_URL)"]
async fn approvals_wrap_withdrawals_and_fee_bump_settle_on_a_polygon_fork() {
    let client = PolygonClient::new(fork_url());
    let signer: PrivateKeySigner = TEST_KEY.parse().unwrap();
    let wallet = signer.address();
    let wallet_hex = format!("{wallet:#x}");
    // A fresh destination, empty before the test.
    let destination = Address::from_slice(
        &alloy_primitives::keccak256(format!("{:?}", std::time::SystemTime::now()))[12..],
    );
    let destination_hex = format!("{destination:#x}");

    fund_usdc(wallet, 100 * USDC).await;
    mine(4).await;
    let start = client
        .get_cash_balances(&wallet_hex)
        .await
        .expect("balances");
    assert!(start.usdc_units >= 100 * USDC, "{start:?}");

    // The bot's start-up approvals: all eight, then none on a second run.
    let sent = ensure_polymarket_approvals(&signer, &fork_url())
        .await
        .expect("approvals");
    assert_eq!(sent, 8, "approvals sent on a new wallet");
    let again = ensure_polymarket_approvals(&signer, &fork_url())
        .await
        .expect("approvals again");
    assert_eq!(again, 0, "approvals repeated");

    // The collateral sweeper's wrap of USDC.e into pUSD.
    let (_, pending) = client.nonces(&wallet).await.expect("nonces");
    let wrap = client
        .prepare_wrap(&signer, 50 * USDC, pending)
        .await
        .expect("wrap");
    send_and_confirm(&client, &wrap.raw, &wrap.tx_hash).await;
    mine(4).await;
    let wrapped = client
        .get_cash_balances(&wallet_hex)
        .await
        .expect("balances");
    assert_eq!(wrapped.usdc_units, start.usdc_units - 50 * USDC);
    assert_eq!(wrapped.pusd_units, start.pusd_units + 50 * USDC);

    // A USDC.e withdrawal.
    let usdc_out = client
        .prepare_withdrawal(
            &signer,
            &destination_hex,
            10 * USDC,
            WithdrawalSource::Usdc,
            0,
        )
        .await
        .expect("USDC.e withdrawal");
    send_and_confirm(&client, &usdc_out.raw, &usdc_out.tx_hash).await;
    assert_eq!(
        client
            .get_usdc_balance_units(&destination_hex)
            .await
            .unwrap(),
        10 * USDC
    );

    // Sending a mined transaction again is never read as refused: that
    // would mark a withdrawal that paid out as failed.
    let resent = client
        .broadcast(&usdc_out.raw, &usdc_out.tx_hash)
        .await
        .expect("resend");
    assert!(
        !matches!(resent, BroadcastOutcome::Rejected(_)),
        "a mined withdrawal read as refused: {resent:?}"
    );

    // A pUSD withdrawal: the Offramp pays the destination USDC.e.
    let pusd_out = client
        .prepare_withdrawal(
            &signer,
            &destination_hex,
            20 * USDC,
            WithdrawalSource::Pusd,
            0,
        )
        .await
        .expect("pUSD withdrawal");
    send_and_confirm(&client, &pusd_out.raw, &pusd_out.tx_hash).await;
    mine(4).await;
    assert_eq!(
        client
            .get_usdc_balance_units(&destination_hex)
            .await
            .unwrap(),
        30 * USDC
    );
    let after = client
        .get_cash_balances(&wallet_hex)
        .await
        .expect("balances");
    assert_eq!(after.pusd_units, wrapped.pusd_units - 20 * USDC);
    assert_eq!(after.usdc_units, wrapped.usdc_units - 10 * USDC);

    // More pUSD than the wallet holds is refused before anything is signed.
    let refused = client
        .prepare_withdrawal(
            &signer,
            &destination_hex,
            after.pusd_units + 1,
            WithdrawalSource::Pusd,
            0,
        )
        .await
        .expect_err("an unfunded withdrawal was signed");
    assert!(refused.to_string().contains(CALL_WOULD_FAIL), "{refused}");

    // A fee bump: a stuck withdrawal replaced at the same nonce pays out
    // once, through the replacement.
    rpc("evm_setAutomine", serde_json::json!([false])).await;
    let stuck = client
        .prepare_withdrawal(&signer, &destination_hex, USDC, WithdrawalSource::Usdc, 0)
        .await
        .expect("withdrawal");
    assert!(matches!(
        client
            .broadcast(&stuck.raw, &stuck.tx_hash)
            .await
            .expect("broadcast"),
        BroadcastOutcome::Accepted
    ));
    let (_, stuck_price) = legacy_nonce_and_gas_price(&stuck.raw).expect("legacy tx");
    let bump = client
        .prepare_replacement(&signer, &stuck.raw, stuck.nonce, stuck_price * 2)
        .await
        .expect("replacement");
    assert_eq!(bump.nonce, stuck.nonce);
    assert_ne!(bump.tx_hash, stuck.tx_hash);
    let bumped = client
        .broadcast(&bump.raw, &bump.tx_hash)
        .await
        .expect("broadcast");
    rpc("evm_setAutomine", serde_json::json!([true])).await;
    assert!(matches!(bumped, BroadcastOutcome::Accepted), "{bumped:?}");
    mine(1).await;
    assert!(client
        .transaction_receipt(&stuck.tx_hash)
        .await
        .expect("receipt")
        .is_none());
    assert!(
        client
            .transaction_receipt(&bump.tx_hash)
            .await
            .expect("receipt")
            .expect("mined")
            .success
    );
    assert_eq!(
        client
            .get_usdc_balance_units(&destination_hex)
            .await
            .unwrap(),
        31 * USDC
    );
    let (mined_nonce, _) = client.nonces(&wallet).await.expect("nonces");
    assert_eq!(mined_nonce, stuck.nonce + 1);
}
