//! On-chain ERC-20 / ERC-1155 approval transactions for Polymarket.
//!
//! Sends the 6 approval transactions required before the CLOB can move
//! funds on behalf of the maker wallet:
//!
//! 1. USDC.e  → approve(CTF Exchange,          MAX)
//! 2. CTF     → setApprovalForAll(CTF Exchange, true)
//! 3. USDC.e  → approve(Neg Risk CTF Exchange,  MAX)
//! 4. CTF     → setApprovalForAll(Neg Risk CTF Exchange, true)
//! 5. USDC.e  → approve(Neg Risk Adapter,       MAX)
//! 6. CTF     → setApprovalForAll(Neg Risk Adapter, true)

use crate::signing::{
    CTF_ADDRESS, CTF_EXCHANGE_ADDRESS, NEG_RISK_ADAPTER_ADDRESS, NEG_RISK_CTF_EXCHANGE_ADDRESS,
    USDC_ADDRESS,
};
use alloy_consensus::TxLegacy;
use alloy_primitives::{Address, Bytes, TxKind, U256};
use alloy_signer_local::PrivateKeySigner;
use anyhow::{Context, Result};
use tracing::{info, warn};

/// Minimum POL balance needed for approval transactions (~6 txns × 60k gas × ~50 gwei).
const MIN_GAS_WEI: u128 = 20_000_000_000_000_000; // 0.02 POL

/// Polygon chain ID.
const CHAIN_ID: u64 = 137;

/// ERC-20 `approve(address,uint256)` selector.
const APPROVE_SELECTOR: [u8; 4] = [0x09, 0x5e, 0xa7, 0xb3];

/// ERC-1155 `setApprovalForAll(address,bool)` selector.
const SET_APPROVAL_SELECTOR: [u8; 4] = [0xa2, 0x2c, 0xb4, 0x65];

/// Max uint256 for unlimited approval.
const MAX_UINT256: U256 = U256::MAX;

/// Build calldata for `approve(spender, MAX_UINT256)`.
fn encode_approve(spender: Address) -> Bytes {
    let mut data = Vec::with_capacity(68);
    data.extend_from_slice(&APPROVE_SELECTOR);
    // address left-padded to 32 bytes
    data.extend_from_slice(&[0u8; 12]);
    data.extend_from_slice(spender.as_slice());
    // uint256 value
    data.extend_from_slice(&MAX_UINT256.to_be_bytes::<32>());
    Bytes::from(data)
}

/// Build calldata for `setApprovalForAll(operator, true)`.
fn encode_set_approval_for_all(operator: Address) -> Bytes {
    let mut data = Vec::with_capacity(68);
    data.extend_from_slice(&SET_APPROVAL_SELECTOR);
    // address left-padded to 32 bytes
    data.extend_from_slice(&[0u8; 12]);
    data.extend_from_slice(operator.as_slice());
    // bool true = 1, left-padded to 32 bytes
    data.extend_from_slice(&[0u8; 31]);
    data.push(1);
    Bytes::from(data)
}

/// RPC helper to get the current nonce for an address.
async fn get_nonce(http: &reqwest::Client, rpc_url: &str, address: &Address) -> Result<u64> {
    let addr = format!("{:?}", address);
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "eth_getTransactionCount",
        "params": [addr, "latest"]
    });

    let resp: serde_json::Value = http.post(rpc_url).json(&body).send().await?.json().await?;
    let hex = resp["result"].as_str().context("Missing nonce result")?;
    Ok(u64::from_str_radix(hex.trim_start_matches("0x"), 16)?)
}

/// RPC helper to get the current gas price.
async fn get_gas_price(http: &reqwest::Client, rpc_url: &str) -> Result<u128> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "eth_gasPrice",
        "params": []
    });

    let resp: serde_json::Value = http.post(rpc_url).json(&body).send().await?.json().await?;
    let hex = resp["result"].as_str().context("Missing gasPrice result")?;
    Ok(u128::from_str_radix(hex.trim_start_matches("0x"), 16)?)
}

/// Send a signed legacy transaction and return the tx hash.
async fn send_raw_tx(
    http: &reqwest::Client,
    rpc_url: &str,
    signer: &PrivateKeySigner,
    tx: TxLegacy,
) -> Result<String> {
    use alloy_consensus::transaction::RlpEcdsaTx;
    use alloy_network::TxSignerSync;
    use alloy_primitives::bytes::BytesMut;

    let mut tx = tx;
    let sig = signer
        .sign_transaction_sync(&mut tx)
        .context("Failed to sign transaction")?;

    let mut encoded = BytesMut::new();
    tx.rlp_encode_signed(&sig, &mut encoded);

    let raw_hex = format!("0x{}", hex::encode(&encoded));
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "eth_sendRawTransaction",
        "params": [raw_hex]
    });

    let resp: serde_json::Value = http.post(rpc_url).json(&body).send().await?.json().await?;

    if let Some(err) = resp.get("error") {
        let msg = err["message"].as_str().unwrap_or("unknown");
        // "already known" means the tx was already sent/mined — not a real error
        if msg.contains("already known") || msg.contains("nonce too low") {
            warn!(error = msg, "Transaction likely already mined, continuing");
            return Ok("already_mined".to_string());
        }
        anyhow::bail!("eth_sendRawTransaction error: {}", msg);
    }

    let tx_hash = resp["result"]
        .as_str()
        .context("Missing tx hash in response")?
        .to_string();

    Ok(tx_hash)
}

/// Wait for a transaction to be mined (simple polling).
async fn wait_for_receipt(http: &reqwest::Client, rpc_url: &str, tx_hash: &str) -> Result<()> {
    if tx_hash == "already_mined" {
        return Ok(());
    }

    for _ in 0..60 {
        // poll every 2 seconds for up to 2 minutes
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_getTransactionReceipt",
            "params": [tx_hash]
        });

        let resp: serde_json::Value = http.post(rpc_url).json(&body).send().await?.json().await?;

        if let Some(receipt) = resp.get("result") {
            if !receipt.is_null() {
                let status = receipt["status"].as_str().unwrap_or("0x0");
                if status == "0x1" {
                    return Ok(());
                } else {
                    anyhow::bail!("Transaction {} reverted", tx_hash);
                }
            }
        }
    }

    anyhow::bail!("Transaction {} not mined after 120s", tx_hash)
}

/// Check if a USDC.e allowance is already set for a spender.
async fn check_erc20_allowance(
    http: &reqwest::Client,
    rpc_url: &str,
    owner: &Address,
    spender: &Address,
) -> Result<U256> {
    // allowance(address,address) selector = 0xdd62ed3e
    let mut data = vec![0xdd, 0x62, 0xed, 0x3e];
    data.extend_from_slice(&[0u8; 12]);
    data.extend_from_slice(owner.as_slice());
    data.extend_from_slice(&[0u8; 12]);
    data.extend_from_slice(spender.as_slice());

    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "eth_call",
        "params": [
            { "to": USDC_ADDRESS, "data": format!("0x{}", hex::encode(&data)) },
            "latest"
        ]
    });

    let resp: serde_json::Value = http.post(rpc_url).json(&body).send().await?.json().await?;
    let hex_val = resp["result"].as_str().unwrap_or("0x0");
    let val = U256::from_str_radix(hex_val.trim_start_matches("0x"), 16).unwrap_or_default();
    Ok(val)
}

/// Check if CTF isApprovedForAll for an operator.
async fn check_erc1155_approval(
    http: &reqwest::Client,
    rpc_url: &str,
    owner: &Address,
    operator: &Address,
) -> Result<bool> {
    // isApprovedForAll(address,address) selector = 0xe985e9c5
    let mut data = vec![0xe9, 0x85, 0xe9, 0xc5];
    data.extend_from_slice(&[0u8; 12]);
    data.extend_from_slice(owner.as_slice());
    data.extend_from_slice(&[0u8; 12]);
    data.extend_from_slice(operator.as_slice());

    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "eth_call",
        "params": [
            { "to": CTF_ADDRESS, "data": format!("0x{}", hex::encode(&data)) },
            "latest"
        ]
    });

    let resp: serde_json::Value = http.post(rpc_url).json(&body).send().await?.json().await?;
    let hex_val = resp["result"].as_str().unwrap_or("0x0");
    // Non-zero means approved
    let approved = !hex_val
        .trim_start_matches("0x")
        .trim_start_matches('0')
        .is_empty();
    Ok(approved)
}

/// Check native POL/MATIC balance for gas.
async fn get_native_balance(
    http: &reqwest::Client,
    rpc_url: &str,
    address: &Address,
) -> Result<u128> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "eth_getBalance",
        "params": [format!("{:?}", address), "latest"]
    });

    let resp: serde_json::Value = http.post(rpc_url).json(&body).send().await?.json().await?;
    let hex = resp["result"].as_str().unwrap_or("0x0");
    Ok(u128::from_str_radix(hex.trim_start_matches("0x"), 16).unwrap_or(0))
}

/// Ensure all 6 Polymarket approvals are set for the given wallet.
///
/// Checks existing approvals first and only sends transactions for missing ones.
/// Returns the number of new approvals sent.
pub async fn ensure_polymarket_approvals(signer: &PrivateKeySigner, rpc_url: &str) -> Result<u32> {
    let http = reqwest::Client::new();
    let address = signer.address();

    let spenders: [(&str, Address); 3] = [
        (
            "CTF Exchange",
            CTF_EXCHANGE_ADDRESS
                .parse()
                .expect("invalid CTF Exchange address"),
        ),
        (
            "Neg Risk CTF Exchange",
            NEG_RISK_CTF_EXCHANGE_ADDRESS
                .parse()
                .expect("invalid Neg Risk CTF Exchange address"),
        ),
        (
            "Neg Risk Adapter",
            NEG_RISK_ADAPTER_ADDRESS
                .parse()
                .expect("invalid Neg Risk Adapter address"),
        ),
    ];

    let usdc: Address = USDC_ADDRESS.parse().expect("invalid USDC address");
    let ctf: Address = CTF_ADDRESS.parse().expect("invalid CTF address");

    // Check POL balance for gas before attempting any transactions
    let pol_balance = get_native_balance(&http, rpc_url, &address)
        .await
        .unwrap_or(0);
    info!(
        wallet = %address,
        pol_balance_wei = pol_balance,
        pol_balance_human = format!("{:.6}", pol_balance as f64 / 1e18),
        "Checking POL balance for approval gas"
    );
    if pol_balance < MIN_GAS_WEI {
        anyhow::bail!(
            "Wallet {:?} needs POL for gas to set approvals ({:.6} POL available, need >= {:.4} POL). \
             Send at least 0.1 POL to this address on Polygon.",
            address,
            pol_balance as f64 / 1e18,
            MIN_GAS_WEI as f64 / 1e18
        );
    }

    let mut nonce = get_nonce(&http, rpc_url, &address).await?;
    let gas_price = get_gas_price(&http, rpc_url).await?;
    // Use 1.2x gas price for faster inclusion
    let gas_price = gas_price + gas_price / 5;
    let mut approvals_sent = 0u32;

    for (name, spender) in &spenders {
        // 1. Check & set USDC.e approval
        let allowance = check_erc20_allowance(&http, rpc_url, &address, spender).await?;
        // Consider "sufficient" if allowance > 1M USDC (to avoid re-approving on every restart)
        let threshold = U256::from(1_000_000_000_000u64); // 1M USDC in 6-decimal
        if allowance < threshold {
            info!(spender = %name, "Setting USDC.e approval");
            let tx = TxLegacy {
                chain_id: Some(CHAIN_ID),
                nonce,
                gas_price,
                gas_limit: 100_000, // USDC.e proxy uses ~67k gas
                to: TxKind::Call(usdc),
                value: U256::ZERO,
                input: encode_approve(*spender),
            };
            let hash = send_raw_tx(&http, rpc_url, signer, tx).await?;
            info!(tx_hash = %hash, spender = %name, "USDC.e approve tx sent");
            wait_for_receipt(&http, rpc_url, &hash).await?;
            nonce += 1;
            approvals_sent += 1;
        } else {
            info!(spender = %name, "USDC.e approval already set");
        }

        // 2. Check & set CTF setApprovalForAll
        let approved = check_erc1155_approval(&http, rpc_url, &address, spender).await?;
        if !approved {
            info!(spender = %name, "Setting CTF setApprovalForAll");
            let tx = TxLegacy {
                chain_id: Some(CHAIN_ID),
                nonce,
                gas_price,
                gas_limit: 100_000,
                to: TxKind::Call(ctf),
                value: U256::ZERO,
                input: encode_set_approval_for_all(*spender),
            };
            let hash = send_raw_tx(&http, rpc_url, signer, tx).await?;
            info!(tx_hash = %hash, spender = %name, "CTF setApprovalForAll tx sent");
            wait_for_receipt(&http, rpc_url, &hash).await?;
            nonce += 1;
            approvals_sent += 1;
        } else {
            info!(spender = %name, "CTF approval already set");
        }
    }

    info!(
        approvals_sent = approvals_sent,
        wallet = %address,
        "Polymarket approval check complete"
    );

    Ok(approvals_sent)
}
