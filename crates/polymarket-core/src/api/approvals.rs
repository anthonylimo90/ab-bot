//! On-chain ERC-20 / ERC-1155 approval transactions for Polymarket.
//!
//! Sends the approvals required before the CLOB (V2) can move funds on
//! behalf of the maker wallet, and before its USDC.e can be wrapped into
//! pUSD and pUSD paid out as USDC.e:
//!
//! 1. pUSD    → approve(CTF Exchange,           MAX)
//! 2. CTF     → setApprovalForAll(CTF Exchange, true)
//! 3. pUSD    → approve(Neg Risk CTF Exchange,  MAX)
//! 4. CTF     → setApprovalForAll(Neg Risk CTF Exchange, true)
//! 5. pUSD    → approve(Neg Risk Adapter,       MAX)
//! 6. CTF     → setApprovalForAll(Neg Risk Adapter, true)
//! 7. USDC.e  → approve(Collateral Onramp,      MAX)
//! 8. pUSD    → approve(Collateral Offramp,     MAX)
//!
//! The ramps only ever pull from their caller, so their approvals let no one
//! else move the wallet's funds.

use crate::signing::{
    COLLATERAL_ADDRESS, COLLATERAL_OFFRAMP_ADDRESS, COLLATERAL_ONRAMP_ADDRESS, CTF_ADDRESS,
    CTF_EXCHANGE_ADDRESS, NEG_RISK_ADAPTER_ADDRESS, NEG_RISK_CTF_EXCHANGE_ADDRESS, USDC_ADDRESS,
};
use alloy_consensus::TxLegacy;
use alloy_primitives::{Address, Bytes, TxKind, U256};
use alloy_signer_local::PrivateKeySigner;
use anyhow::{Context, Result};
use tracing::{info, warn};

/// How often, and how many times, an approval's on-chain state is re-read
/// before it is reported as not in effect.
const APPROVAL_CONFIRM_INTERVAL: std::time::Duration =
    std::time::Duration::from_millis(if cfg!(test) { 10 } else { 2000 });
const APPROVAL_CONFIRM_POLLS: u32 = 60;
/// How often an approval is checked for a receipt.
const RECEIPT_POLL: std::time::Duration =
    std::time::Duration::from_millis(if cfg!(test) { 5 } else { 2000 });
/// An approval without a receipt after this many polls is signed again at
/// its nonce for more gas, up to [`SEND_ROUNDS`] sends in all.
const RESEND_AFTER_POLLS: u32 = 15;
const SEND_ROUNDS: u32 = 4;
/// The most an approval's gas price is raised to, as a multiple of what the
/// network asks.
const MAX_GAS_OVER_NETWORK: u128 = 3;
/// Minimum POL balance needed for approval transactions (~8 txns × 60k gas × ~40 gwei).
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

/// RPC helper to get the next nonce for an address, counting transactions
/// still pending so an approval never replaces one (e.g. a withdrawal).
async fn get_nonce(http: &reqwest::Client, rpc_url: &str, address: &Address) -> Result<u64> {
    let addr = format!("{:?}", address);
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "eth_getTransactionCount",
        "params": [addr, "pending"]
    });

    let resp: serde_json::Value = http.post(rpc_url).json(&body).send().await?.json().await?;
    let hex = rpc_result_hex(&resp, "eth_getTransactionCount")?;
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
    let hex = rpc_result_hex(&resp, "eth_gasPrice")?;
    Ok(u128::from_str_radix(hex.trim_start_matches("0x"), 16)?)
}

/// What a new approval offers: a fifth more than the network asks now, for
/// faster inclusion.
async fn offered_gas_price(http: &reqwest::Client, rpc_url: &str) -> Result<u128> {
    let network = get_gas_price(http, rpc_url).await?;
    Ok(network + network / 5)
}

/// Extract hex string from JSON-RPC result, surfacing provider errors explicitly.
fn rpc_result_hex<'a>(resp: &'a serde_json::Value, method: &str) -> Result<&'a str> {
    if let Some(err) = resp.get("error") {
        let code = err.get("code").and_then(|v| v.as_i64()).unwrap_or_default();
        let message = err
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown RPC error");
        anyhow::bail!("{method} RPC error ({code}): {message}");
    }

    resp.get("result")
        .and_then(|v| v.as_str())
        .with_context(|| format!("Missing result from {method}"))
}

/// Send a signed legacy transaction: its hash when the node accepted it or
/// already holds it, `None` when its nonce is already used (the state it was
/// meant to set then decides).
async fn send_raw_tx(
    http: &reqwest::Client,
    rpc_url: &str,
    signer: &PrivateKeySigner,
    tx: TxLegacy,
) -> Result<Option<String>> {
    use alloy_consensus::transaction::RlpEcdsaTx;
    use alloy_network::TxSignerSync;
    use alloy_primitives::bytes::BytesMut;

    let mut tx = tx;
    let sig = signer
        .sign_transaction_sync(&mut tx)
        .context("Failed to sign transaction")?;

    let mut encoded = BytesMut::new();
    tx.rlp_encode_signed(&sig, &mut encoded);

    let tx_hash = format!("{:#x}", alloy_primitives::keccak256(&encoded));
    let raw_hex = format!("0x{}", hex::encode(&encoded));
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "eth_sendRawTransaction",
        "params": [raw_hex]
    });

    let resp: serde_json::Value = http.post(rpc_url).json(&body).send().await?.json().await?;

    if let Some(err) = resp.get("error").filter(|err| !err.is_null()) {
        let msg = err["message"].as_str().unwrap_or("unknown");
        // The node holds this very transaction already.
        if msg.contains("already known") {
            return Ok(Some(tx_hash));
        }
        // Mined already, or another transaction took the nonce.
        if msg.contains("nonce too low") {
            warn!(
                error = msg,
                "Approval send outcome unclear; checking the chain instead"
            );
            return Ok(None);
        }
        anyhow::bail!("eth_sendRawTransaction error: {}", msg);
    }
    resp["result"]
        .as_str()
        .context("Missing tx hash in response")?;

    Ok(Some(tx_hash))
}

/// Send an approval and wait for it to take effect. While it waits without
/// a receipt it is signed again at its nonce for a tenth more gas (at least
/// what the network asks now, never over [`MAX_GAS_OVER_NETWORK`] times it),
/// so an underpriced approval does not hold the wallet's later transactions
/// back, but only while the nonce is still its own: the node holds one of
/// its transactions, or counts nothing pending at the nonce. Whichever send
/// is mined, the on-chain state alone proves it.
async fn send_approval<F, Fut>(
    http: &reqwest::Client,
    rpc_url: &str,
    signer: &PrivateKeySigner,
    mut tx: TxLegacy,
    is_set: F,
) -> Result<()>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<bool>>,
{
    let Some(hash) = send_raw_tx(http, rpc_url, signer, tx.clone()).await? else {
        return await_state(is_set).await;
    };
    info!(tx_hash = %hash, nonce = tx.nonce, "Approval sent");
    let mut hashes = vec![hash];
    for round in 1..=SEND_ROUNDS {
        match any_receipt(http, rpc_url, &hashes).await {
            Some((hash, true)) => {
                info!(tx_hash = %hash, "Approval mined");
                break;
            }
            Some((hash, false)) => anyhow::bail!("Approval transaction {hash} reverted"),
            None if round == SEND_ROUNDS => {
                warn!(hashes = ?hashes, "Approval not mined yet; checking the chain");
            }
            None => {
                if !nonce_still_ours(http, rpc_url, signer, &tx, &hashes).await {
                    continue;
                }
                let network = match get_gas_price(http, rpc_url).await {
                    Ok(price) => price,
                    Err(error) => {
                        warn!(%error, "Could not read the gas price to raise an approval's");
                        continue;
                    }
                };
                let price = (tx.gas_price + tx.gas_price / 10 + 1).max(network + network / 5);
                if price > network.saturating_mul(MAX_GAS_OVER_NETWORK) {
                    continue;
                }
                tx.gas_price = price;
                match send_raw_tx(http, rpc_url, signer, tx.clone()).await {
                    Ok(Some(hash)) => {
                        info!(tx_hash = %hash, gas_price = price, "Raised an approval's gas price");
                        hashes.push(hash);
                    }
                    // Its nonce is used: a receipt or the state decides.
                    Ok(None) => {}
                    Err(error) => {
                        warn!(%error, "Could not send an approval again for more gas")
                    }
                }
            }
        }
    }
    await_state(is_set).await
}

/// Whether the approval's nonce is still its own to sign at again: the node
/// holds one of its `hashes`, or counts nothing pending at the nonce.
/// Otherwise another transaction may hold it, and a replacement would push
/// that one out.
async fn nonce_still_ours(
    http: &reqwest::Client,
    rpc_url: &str,
    signer: &PrivateKeySigner,
    tx: &TxLegacy,
    hashes: &[String],
) -> bool {
    for hash in hashes {
        if matches!(transaction_known(http, rpc_url, hash).await, Ok(true)) {
            return true;
        }
    }
    matches!(get_nonce(http, rpc_url, &signer.address()).await, Ok(pending) if pending <= tx.nonce)
}

/// Whether the node knows a transaction (pending or mined).
async fn transaction_known(http: &reqwest::Client, rpc_url: &str, hash: &str) -> Result<bool> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "eth_getTransactionByHash",
        "params": [hash]
    });
    let resp: serde_json::Value = http.post(rpc_url).json(&body).send().await?.json().await?;
    if let Some(err) = resp.get("error").filter(|err| !err.is_null()) {
        anyhow::bail!("eth_getTransactionByHash error: {err}");
    }
    Ok(resp.get("result").is_some_and(|result| !result.is_null()))
}

/// The first receipt found for one of `hashes` (and whether it succeeded),
/// polling for up to [`RESEND_AFTER_POLLS`].
async fn any_receipt(
    http: &reqwest::Client,
    rpc_url: &str,
    hashes: &[String],
) -> Option<(String, bool)> {
    for _ in 0..RESEND_AFTER_POLLS {
        tokio::time::sleep(RECEIPT_POLL).await;
        for hash in hashes {
            let body = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "eth_getTransactionReceipt",
                "params": [hash]
            });
            let response: Result<serde_json::Value> =
                async { Ok(http.post(rpc_url).json(&body).send().await?.json().await?) }.await;
            match response {
                Ok(response) => {
                    if let Some(receipt) = response.get("result").filter(|r| !r.is_null()) {
                        return Some((hash.clone(), receipt["status"].as_str() == Some("0x1")));
                    }
                }
                Err(error) => {
                    warn!(%error, tx_hash = %hash, "Could not read an approval's receipt")
                }
            }
        }
    }
    None
}

/// Wait for an approval's on-chain state, which alone proves it took effect.
async fn await_state<F, Fut>(is_set: F) -> Result<()>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<bool>>,
{
    for attempt in 0..=APPROVAL_CONFIRM_POLLS {
        if is_set().await? {
            return Ok(());
        }
        if attempt < APPROVAL_CONFIRM_POLLS {
            tokio::time::sleep(APPROVAL_CONFIRM_INTERVAL).await;
        }
    }
    anyhow::bail!("Approval was not in effect on chain after its transaction; retry on restart")
}

/// The allowance `owner` gives `spender` of `token`.
async fn check_erc20_allowance(
    http: &reqwest::Client,
    rpc_url: &str,
    token: &Address,
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
            { "to": format!("{token:#x}"), "data": format!("0x{}", hex::encode(&data)) },
            "latest"
        ]
    });

    let resp: serde_json::Value = http.post(rpc_url).json(&body).send().await?.json().await?;
    let hex_val = rpc_result_hex(&resp, "eth_call allowance")?;
    let val = U256::from_str_radix(hex_val.trim_start_matches("0x"), 16)
        .context("Failed to parse allowance hex result")?;
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
    let hex_val = rpc_result_hex(&resp, "eth_call isApprovedForAll")?;
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
    let hex = rpc_result_hex(&resp, "eth_getBalance")?;
    u128::from_str_radix(hex.trim_start_matches("0x"), 16)
        .context("Failed to parse eth_getBalance result")
}

/// The ERC-20 approvals the wallet gives: (token name, token, spender name, spender).
fn erc20_approvals() -> [(&'static str, Address, &'static str, Address); 5] {
    let pusd: Address = COLLATERAL_ADDRESS.parse().expect("invalid pUSD address");
    let usdc: Address = USDC_ADDRESS.parse().expect("invalid USDC address");
    [
        ("pUSD", pusd, "CTF Exchange", exchange_spenders()[0].1),
        (
            "pUSD",
            pusd,
            "Neg Risk CTF Exchange",
            exchange_spenders()[1].1,
        ),
        ("pUSD", pusd, "Neg Risk Adapter", exchange_spenders()[2].1),
        (
            "USDC.e",
            usdc,
            "Collateral Onramp",
            COLLATERAL_ONRAMP_ADDRESS
                .parse()
                .expect("invalid Onramp address"),
        ),
        (
            "pUSD",
            pusd,
            "Collateral Offramp",
            COLLATERAL_OFFRAMP_ADDRESS
                .parse()
                .expect("invalid Offramp address"),
        ),
    ]
}

/// The contracts that settle trades, and so move the wallet's outcome tokens.
fn exchange_spenders() -> [(&'static str, Address); 3] {
    [
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
    ]
}

/// Ensure every Polymarket approval is set for the given wallet.
///
/// Checks existing approvals first and only sends transactions for missing ones.
/// Returns the number of new approvals sent.
pub async fn ensure_polymarket_approvals(signer: &PrivateKeySigner, rpc_url: &str) -> Result<u32> {
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .context("Failed to build HTTP client")?;
    let address = signer.address();
    let ctf: Address = CTF_ADDRESS.parse().expect("invalid CTF address");

    // Check POL balance for gas before attempting any transactions
    let pol_balance = get_native_balance(&http, rpc_url, &address)
        .await
        .context("Failed fetching wallet POL balance for approval checks")?;
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

    let mut approvals_sent = 0u32;
    // Consider "sufficient" if allowance > 1M USDC (to avoid re-approving on every restart)
    let threshold = U256::from(1_000_000_000_000u64); // 1M in 6-decimal units

    for (token_name, token, spender_name, spender) in erc20_approvals() {
        let allowance = check_erc20_allowance(&http, rpc_url, &token, &address, &spender).await?;
        if allowance >= threshold {
            info!(
                token = token_name,
                spender = spender_name,
                "Approval already set"
            );
            continue;
        }
        info!(
            token = token_name,
            spender = spender_name,
            "Setting approval"
        );
        // A fresh pending nonce each time: an unclear send may or may not
        // have used the previous one.
        let tx = TxLegacy {
            chain_id: Some(CHAIN_ID),
            nonce: get_nonce(&http, rpc_url, &address).await?,
            gas_price: offered_gas_price(&http, rpc_url).await?,
            gas_limit: 100_000, // a proxied token's approve uses ~67k gas
            to: TxKind::Call(token),
            value: U256::ZERO,
            input: encode_approve(spender),
        };
        send_approval(&http, rpc_url, signer, tx, || async {
            Ok(
                check_erc20_allowance(&http, rpc_url, &token, &address, &spender).await?
                    >= threshold,
            )
        })
        .await?;
        approvals_sent += 1;
    }

    for (name, spender) in exchange_spenders() {
        let approved = check_erc1155_approval(&http, rpc_url, &address, &spender).await?;
        if approved {
            info!(spender = %name, "CTF approval already set");
            continue;
        }
        info!(spender = %name, "Setting CTF setApprovalForAll");
        let tx = TxLegacy {
            chain_id: Some(CHAIN_ID),
            nonce: get_nonce(&http, rpc_url, &address).await?,
            gas_price: offered_gas_price(&http, rpc_url).await?,
            gas_limit: 100_000,
            to: TxKind::Call(ctf),
            value: U256::ZERO,
            input: encode_set_approval_for_all(spender),
        };
        send_approval(&http, rpc_url, signer, tx, || {
            check_erc1155_approval(&http, rpc_url, &address, &spender)
        })
        .await?;
        approvals_sent += 1;
    }

    info!(
        approvals_sent = approvals_sent,
        wallet = %address,
        "Polymarket approval check complete"
    );

    Ok(approvals_sent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn the_v2_exchanges_and_the_collateral_ramps_are_approved() {
        let erc20: Vec<(String, String)> = erc20_approvals()
            .iter()
            .map(|(_, token, _, spender)| (format!("{token:#x}"), format!("{spender:#x}")))
            .collect();
        let lower = |address: &str| address.to_ascii_lowercase();
        let pusd = lower(COLLATERAL_ADDRESS);
        assert_eq!(
            erc20,
            [
                (pusd.clone(), lower(CTF_EXCHANGE_ADDRESS)),
                (pusd.clone(), lower(NEG_RISK_CTF_EXCHANGE_ADDRESS)),
                (pusd.clone(), lower(NEG_RISK_ADAPTER_ADDRESS)),
                (lower(USDC_ADDRESS), lower(COLLATERAL_ONRAMP_ADDRESS)),
                (pusd, lower(COLLATERAL_OFFRAMP_ADDRESS)),
            ]
        );
        let operators: Vec<String> = exchange_spenders()
            .iter()
            .map(|(_, spender)| format!("{spender:#x}"))
            .collect();
        assert_eq!(
            operators,
            [
                lower(CTF_EXCHANGE_ADDRESS),
                lower(NEG_RISK_CTF_EXCHANGE_ADDRESS),
                lower(NEG_RISK_ADAPTER_ADDRESS)
            ]
        );
    }

    #[tokio::test]
    async fn an_unclear_send_counts_only_once_the_approval_is_in_effect() {
        // Takes effect on the third look.
        let looks = AtomicU32::new(0);
        await_state(|| async { Ok(looks.fetch_add(1, Ordering::SeqCst) >= 2) })
            .await
            .unwrap();
        assert_eq!(looks.load(Ordering::SeqCst), 3);

        // Never takes effect: an error, not a success.
        assert!(await_state(|| async { Ok(false) }).await.is_err());
    }

    /// A JSON-RPC node answering each request with `answer`, one connection
    /// per request.
    async fn mock_node<A>(answer: A) -> String
    where
        A: Fn(&serde_json::Value) -> serde_json::Value + Send + Sync + 'static,
    {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let answer = std::sync::Arc::new(answer);
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let answer = answer.clone();
                tokio::spawn(async move {
                    let mut bytes = Vec::new();
                    let body = loop {
                        let mut buffer = [0; 4096];
                        let count = socket.read(&mut buffer).await.unwrap();
                        if count == 0 {
                            return;
                        }
                        bytes.extend_from_slice(&buffer[..count]);
                        let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") else {
                            continue;
                        };
                        let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                        let length: usize = headers
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length: "))
                            .unwrap()
                            .trim()
                            .parse()
                            .unwrap();
                        if bytes.len() >= end + 4 + length {
                            break bytes[end + 4..end + 4 + length].to_vec();
                        }
                    };
                    let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
                    let reply = serde_json::json!({"jsonrpc": "2.0", "id": 1})
                        .as_object()
                        .unwrap()
                        .clone()
                        .into_iter()
                        .chain(answer(&request).as_object().unwrap().clone())
                        .collect::<serde_json::Map<_, _>>();
                    let reply = serde_json::Value::Object(reply).to_string();
                    socket
                        .write_all(
                            format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                                 Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                                reply.len(),
                                reply
                            )
                            .as_bytes(),
                        )
                        .await
                        .unwrap();
                });
            }
        });
        format!("http://{address}")
    }

    fn approval_tx(gas_price: u128) -> TxLegacy {
        TxLegacy {
            chain_id: Some(CHAIN_ID),
            nonce: 4,
            gas_price,
            gas_limit: 100_000,
            to: TxKind::Call(COLLATERAL_ADDRESS.parse().unwrap()),
            value: U256::ZERO,
            input: encode_approve(COLLATERAL_ONRAMP_ADDRESS.parse().unwrap()),
        }
    }

    #[tokio::test]
    async fn an_approval_left_unmined_is_signed_again_at_its_nonce_for_more_gas() {
        const NETWORK: u128 = 30_000_000_000;
        let sent = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let node = {
            let sent = sent.clone();
            mock_node(move |request| {
                let mut sent = sent.lock().unwrap();
                match request["method"].as_str().unwrap() {
                    "eth_gasPrice" => serde_json::json!({"result": format!("{NETWORK:#x}")}),
                    "eth_sendRawTransaction" => {
                        let raw = request["params"][0].as_str().unwrap().to_string();
                        let hash = format!(
                            "{:#x}",
                            alloy_primitives::keccak256(
                                hex::decode(raw.trim_start_matches("0x")).unwrap()
                            )
                        );
                        sent.push(raw);
                        serde_json::json!({"result": hash})
                    }
                    // The node holds every send; another transaction is
                    // pending behind them.
                    "eth_getTransactionByHash" => {
                        let held = sent.iter().any(|raw| {
                            format!(
                                "{:#x}",
                                alloy_primitives::keccak256(
                                    hex::decode(raw.trim_start_matches("0x")).unwrap()
                                )
                            ) == request["params"][0].as_str().unwrap()
                        });
                        serde_json::json!({"result": held.then(|| serde_json::json!({}))})
                    }
                    "eth_getTransactionCount" => serde_json::json!({"result": "0x6"}),
                    // Only the third send is mined.
                    "eth_getTransactionReceipt" => {
                        let mined = sent.get(2).map(|raw| {
                            format!(
                                "{:#x}",
                                alloy_primitives::keccak256(
                                    hex::decode(raw.trim_start_matches("0x")).unwrap()
                                )
                            )
                        });
                        if mined.as_deref() == request["params"][0].as_str() {
                            serde_json::json!({"result": {"status": "0x1"}})
                        } else {
                            serde_json::json!({"result": null})
                        }
                    }
                    other => panic!("unexpected RPC method {other}"),
                }
            })
            .await
        };
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let signer: PrivateKeySigner = format!("{:064x}", 1).parse().unwrap();
        let in_effect = || async { Ok(sent.lock().unwrap().len() >= 3) };
        send_approval(&http, &node, &signer, approval_tx(NETWORK), in_effect)
            .await
            .unwrap();
        let prices: Vec<u128> = sent
            .lock()
            .unwrap()
            .iter()
            .map(|raw| {
                let (nonce, price) = crate::api::polygon::legacy_nonce_and_gas_price(raw).unwrap();
                assert_eq!(nonce, 4);
                price
            })
            .collect();
        assert_eq!(prices.len(), 3, "{prices:?}");
        // At least a fifth over the network, then a tenth over the last.
        assert_eq!(prices[1], NETWORK + NETWORK / 5);
        assert_eq!(prices[2], prices[1] + prices[1] / 10 + 1);
        assert!(prices.iter().all(|price| *price <= 3 * NETWORK));
    }

    #[tokio::test]
    async fn an_approval_is_never_signed_again_over_a_transaction_holding_its_nonce() {
        use std::sync::atomic::AtomicU64;
        let sent = std::sync::Arc::new(AtomicU32::new(0));
        let pending = std::sync::Arc::new(AtomicU64::new(5));
        let node = {
            let (sent, pending) = (sent.clone(), pending.clone());
            mock_node(move |request| match request["method"].as_str().unwrap() {
                "eth_gasPrice" => serde_json::json!({"result": "0x6fc23ac00"}),
                "eth_sendRawTransaction" => {
                    sent.fetch_add(1, Ordering::SeqCst);
                    serde_json::json!({"result": "0x01"})
                }
                // The node dropped the approval and holds nothing of ours.
                "eth_getTransactionByHash" | "eth_getTransactionReceipt" => {
                    serde_json::json!({"result": null})
                }
                "eth_getTransactionCount" => serde_json::json!({
                    "result": format!("{:#x}", pending.load(Ordering::SeqCst))
                }),
                other => panic!("unexpected RPC method {other}"),
            })
            .await
        };
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let signer: PrivateKeySigner = format!("{:064x}", 1).parse().unwrap();
        // Another transaction is pending at its nonce (4): sent once only.
        send_approval(&http, &node, &signer, approval_tx(1), || async { Ok(true) })
            .await
            .unwrap();
        assert_eq!(sent.load(Ordering::SeqCst), 1);
        // Nothing pending at its nonce: signed again for more gas.
        sent.store(0, Ordering::SeqCst);
        pending.store(4, Ordering::SeqCst);
        send_approval(&http, &node, &signer, approval_tx(1), || async { Ok(true) })
            .await
            .unwrap();
        assert!(sent.load(Ordering::SeqCst) > 1);
    }

    #[tokio::test]
    async fn a_reverted_approval_is_an_error() {
        let node = mock_node(|request| match request["method"].as_str().unwrap() {
            "eth_sendRawTransaction" => serde_json::json!({"result": "0x01"}),
            "eth_getTransactionReceipt" => serde_json::json!({"result": {"status": "0x0"}}),
            other => panic!("unexpected RPC method {other}"),
        })
        .await;
        let http = reqwest::Client::builder().no_proxy().build().unwrap();
        let signer: PrivateKeySigner = format!("{:064x}", 1).parse().unwrap();
        let error = send_approval(&http, &node, &signer, approval_tx(1), || async { Ok(true) })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("reverted"), "{error}");
    }
}
