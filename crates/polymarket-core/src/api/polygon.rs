//! Polygon RPC client for on-chain data.

use crate::signing::CTF_ADDRESS;
use crate::{Error, Result};
use alloy_consensus::TxLegacy;
use alloy_primitives::U256;
use alloy_primitives::{Address, Bytes, TxKind};
use alloy_signer_local::PrivateKeySigner;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::str::FromStr;
use std::time::Duration;

/// USDC.e contract on Polygon (PoS bridged, 6 decimals) — used by Polymarket.
const POLYGON_USDC_ADDRESS: &str = "0x2791Bca1f2de4661ED88A30C99A7a9449Aa84174";

/// Native USDC contract on Polygon (CCTP, 6 decimals) — NOT used by Polymarket.
const POLYGON_NATIVE_USDC_ADDRESS: &str = "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359";
const ERC1155_TRANSFER_SINGLE_TOPIC: &str =
    "0xc3d58168c5ae7397731d063d5bbf3d657854427343f4c083240f7aacaa2d0f62";
const ERC1155_TRANSFER_BATCH_TOPIC: &str =
    "0x4a39dc06d4c0dbc64b70af90fd698a233a518aa5d07e595d983b8c0526c8f7fb";
const ERC20_TRANSFER_SELECTOR: [u8; 4] = [0xa9, 0x05, 0x9c, 0xbb];
const POLYGON_CHAIN_ID: u64 = 137;
const DEFAULT_ERC20_TRANSFER_GAS_LIMIT: u64 = 100_000;

/// Polygon RPC client for querying blockchain data.
#[derive(Clone)]
pub struct PolygonClient {
    rpc_url: String,
    http_client: reqwest::Client,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionReceiptSummary {
    pub tx_hash: String,
    pub block_number: Option<u64>,
    pub gas_used: Option<u64>,
}

impl PolygonClient {
    /// Build a shared HTTP client with sensible timeouts for RPC calls.
    fn build_http_client() -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(10))
            .build()
            .expect("Failed to build HTTP client")
    }

    /// Create a new Polygon client with an Alchemy API key.
    pub fn with_alchemy(api_key: &str) -> Self {
        let rpc_url = format!("https://polygon-mainnet.g.alchemy.com/v2/{}", api_key);
        Self {
            rpc_url,
            http_client: Self::build_http_client(),
        }
    }

    /// Create a new Polygon client with a custom RPC URL.
    pub fn new(rpc_url: String) -> Self {
        Self {
            rpc_url,
            http_client: Self::build_http_client(),
        }
    }

    /// Get the current block number.
    pub async fn get_block_number(&self) -> Result<u64> {
        let response: JsonRpcResponse<String> = self
            .rpc_call("eth_blockNumber", serde_json::json!([]))
            .await?;

        let block_hex = response.result.ok_or_else(|| Error::Api {
            message: "No result in response".to_string(),
            status: None,
        })?;

        let block = u64::from_str_radix(block_hex.trim_start_matches("0x"), 16).map_err(|e| {
            Error::Api {
                message: format!("Failed to parse block number: {}", e),
                status: None,
            }
        })?;

        Ok(block)
    }

    /// Get transaction logs for a contract.
    pub async fn get_logs(
        &self,
        contract_address: &str,
        from_block: u64,
        to_block: u64,
        topics: Option<Vec<String>>,
    ) -> Result<Vec<Log>> {
        let params = serde_json::json!([{
            "address": contract_address,
            "fromBlock": format!("0x{:x}", from_block),
            "toBlock": format!("0x{:x}", to_block),
            "topics": topics.unwrap_or_default()
        }]);

        let response: JsonRpcResponse<Vec<Log>> = self.rpc_call("eth_getLogs", params).await?;

        response.result.ok_or_else(|| Error::Api {
            message: response.error.map(|e| e.message).unwrap_or_default(),
            status: None,
        })
    }

    /// Get transaction logs for a contract with an explicit topics JSON filter.
    pub async fn get_logs_with_topics(
        &self,
        contract_address: &str,
        from_block: u64,
        to_block: u64,
        topics: serde_json::Value,
    ) -> Result<Vec<Log>> {
        let params = serde_json::json!([{
            "address": contract_address,
            "fromBlock": format!("0x{:x}", from_block),
            "toBlock": format!("0x{:x}", to_block),
            "topics": topics
        }]);

        let response: JsonRpcResponse<Vec<Log>> = self.rpc_call("eth_getLogs", params).await?;

        response.result.ok_or_else(|| Error::Api {
            message: response.error.map(|e| e.message).unwrap_or_default(),
            status: None,
        })
    }

    /// Discover CTF ERC-1155 token ids that have been transferred to or from a wallet.
    pub async fn get_ctf_transfer_token_ids(
        &self,
        wallet_address: &str,
        from_block: u64,
        to_block: u64,
    ) -> Result<HashSet<String>> {
        if from_block > to_block {
            return Ok(HashSet::new());
        }

        let wallet_topic = topic_for_address(wallet_address);
        let filters = [
            serde_json::json!([
                ERC1155_TRANSFER_SINGLE_TOPIC,
                serde_json::Value::Null,
                wallet_topic,
                serde_json::Value::Null
            ]),
            serde_json::json!([
                ERC1155_TRANSFER_SINGLE_TOPIC,
                serde_json::Value::Null,
                serde_json::Value::Null,
                wallet_topic
            ]),
            serde_json::json!([
                ERC1155_TRANSFER_BATCH_TOPIC,
                serde_json::Value::Null,
                wallet_topic,
                serde_json::Value::Null
            ]),
            serde_json::json!([
                ERC1155_TRANSFER_BATCH_TOPIC,
                serde_json::Value::Null,
                serde_json::Value::Null,
                wallet_topic
            ]),
        ];

        let mut token_ids = HashSet::new();
        for filter in filters {
            let logs = self
                .get_logs_with_topics(CTF_ADDRESS, from_block, to_block, filter)
                .await?;
            for log in logs {
                for token_id in extract_ctf_token_ids(&log)? {
                    token_ids.insert(token_id);
                }
            }
        }

        Ok(token_ids)
    }

    /// Get transactions for a wallet address (via Alchemy enhanced API).
    pub async fn get_asset_transfers(
        &self,
        address: &str,
        from_block: Option<u64>,
        to_block: Option<u64>,
    ) -> Result<Vec<AssetTransfer>> {
        let params = serde_json::json!([{
            "fromAddress": address,
            "fromBlock": from_block.map(|b| format!("0x{:x}", b)).unwrap_or_else(|| "0x0".to_string()),
            "toBlock": to_block.map(|b| format!("0x{:x}", b)).unwrap_or_else(|| "latest".to_string()),
            "category": ["erc20"],
            "withMetadata": true,
            "maxCount": "0x3e8" // 1000
        }]);

        let response: JsonRpcResponse<AssetTransfersResponse> =
            self.rpc_call("alchemy_getAssetTransfers", params).await?;

        Ok(response.result.map(|r| r.transfers).unwrap_or_default())
    }

    /// Get the USDC.e (PoS bridged) balance for a wallet — the token Polymarket uses.
    pub async fn get_usdc_balance(&self, wallet_address: &str) -> Result<f64> {
        self.get_erc20_balance(wallet_address, POLYGON_USDC_ADDRESS)
            .await
    }

    /// Get the raw on-chain USDC.e balance (6 decimals) for a wallet.
    pub async fn get_usdc_balance_units(&self, wallet_address: &str) -> Result<u128> {
        self.get_erc20_balance_units(wallet_address, POLYGON_USDC_ADDRESS)
            .await
    }

    /// Get the native USDC (CCTP) balance for a wallet — NOT used by Polymarket.
    pub async fn get_native_usdc_balance(&self, wallet_address: &str) -> Result<f64> {
        self.get_erc20_balance(wallet_address, POLYGON_NATIVE_USDC_ADDRESS)
            .await
    }

    /// Get a 6-decimal ERC-20 balance for a wallet address (returns human-readable amount).
    async fn get_erc20_balance(&self, wallet_address: &str, token_address: &str) -> Result<f64> {
        let balance = self
            .get_erc20_balance_units(wallet_address, token_address)
            .await?;

        // USDC has 6 decimals
        Ok(balance as f64 / 1_000_000.0)
    }

    /// Submit a signed USDC.e transfer transaction and return the tx hash.
    pub async fn submit_usdc_transfer(
        &self,
        signer: &PrivateKeySigner,
        to_address: &str,
        amount_units: u128,
    ) -> Result<String> {
        if amount_units == 0 {
            return Err(Error::Api {
                message: "Transfer amount must be greater than zero".to_string(),
                status: None,
            });
        }

        let recipient = Address::from_str(to_address).map_err(|e| Error::Api {
            message: format!("Invalid recipient address: {}", e),
            status: None,
        })?;
        let usdc = Address::from_str(POLYGON_USDC_ADDRESS).expect("invalid USDC address");
        let sender = signer.address();

        let pol_balance = self.get_native_balance_wei(&sender).await?;
        let nonce = self.get_nonce(&sender).await?;
        let gas_price = self.get_gas_price_wei().await?;
        let gas_price = gas_price + gas_price / 5;
        let required_gas = gas_price.saturating_mul(DEFAULT_ERC20_TRANSFER_GAS_LIMIT as u128);

        if pol_balance < required_gas {
            return Err(Error::Api {
                message: format!(
                    "Insufficient POL for gas: {:.6} POL available, need at least {:.6} POL",
                    pol_balance as f64 / 1e18,
                    required_gas as f64 / 1e18
                ),
                status: None,
            });
        }

        let tx = TxLegacy {
            chain_id: Some(POLYGON_CHAIN_ID),
            nonce,
            gas_price,
            gas_limit: DEFAULT_ERC20_TRANSFER_GAS_LIMIT,
            to: TxKind::Call(usdc),
            value: U256::ZERO,
            input: encode_erc20_transfer(recipient, U256::from(amount_units)),
        };

        self.send_raw_tx(signer, tx).await
    }

    /// Wait for a Polygon transaction to be mined and return its receipt summary.
    pub async fn wait_for_transaction(
        &self,
        tx_hash: &str,
        max_attempts: usize,
        poll_interval: Duration,
    ) -> Result<TransactionReceiptSummary> {
        for _ in 0..max_attempts {
            tokio::time::sleep(poll_interval).await;

            let body = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "eth_getTransactionReceipt",
                "params": [tx_hash]
            });

            let response: serde_json::Value = self
                .http_client
                .post(&self.rpc_url)
                .json(&body)
                .send()
                .await?
                .json()
                .await?;

            if let Some(err) = response.get("error") {
                let message = err
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown RPC error");
                return Err(Error::Api {
                    message: format!("eth_getTransactionReceipt RPC error: {}", message),
                    status: None,
                });
            }

            let Some(receipt) = response.get("result") else {
                continue;
            };
            if receipt.is_null() {
                continue;
            }

            let status = receipt
                .get("status")
                .and_then(|value| value.as_str())
                .unwrap_or("0x0");
            if status != "0x1" {
                return Err(Error::Api {
                    message: format!("Transaction {} reverted on-chain", tx_hash),
                    status: None,
                });
            }

            return Ok(TransactionReceiptSummary {
                tx_hash: tx_hash.to_string(),
                block_number: receipt
                    .get("blockNumber")
                    .and_then(|value| value.as_str())
                    .and_then(parse_hex_u64),
                gas_used: receipt
                    .get("gasUsed")
                    .and_then(|value| value.as_str())
                    .and_then(parse_hex_u64),
            });
        }

        Err(Error::Api {
            message: format!(
                "Transaction {} not mined after {} polls",
                tx_hash, max_attempts
            ),
            status: None,
        })
    }

    async fn get_erc20_balance_units(
        &self,
        wallet_address: &str,
        token_address: &str,
    ) -> Result<u128> {
        // balanceOf(address) selector = 0x70a08231 + 32-byte left-padded address
        let addr = wallet_address.trim_start_matches("0x");
        let data = format!("0x70a08231{:0>64}", addr);

        let params = serde_json::json!([
            { "to": token_address, "data": data },
            "latest"
        ]);

        let response: JsonRpcResponse<String> = self.rpc_call("eth_call", params).await?;
        let hex_balance = response.result.ok_or_else(|| Error::Api {
            message: "No result from eth_call".to_string(),
            status: None,
        })?;

        let balance =
            u128::from_str_radix(hex_balance.trim_start_matches("0x"), 16).map_err(|e| {
                Error::Api {
                    message: format!("Failed to parse balance: {}", e),
                    status: None,
                }
            })?;

        Ok(balance)
    }

    async fn rpc_call<T: for<'de> Deserialize<'de>>(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<JsonRpcResponse<T>> {
        let request = JsonRpcRequest {
            jsonrpc: "2.0",
            id: 1,
            method,
            params,
        };

        let response = self
            .http_client
            .post(&self.rpc_url)
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(Error::Api {
                message: format!("RPC request failed: {}", response.status()),
                status: Some(response.status().as_u16()),
            });
        }

        Ok(response.json().await?)
    }

    async fn get_nonce(&self, address: &Address) -> Result<u64> {
        let params = serde_json::json!([format!("{:?}", address), "latest"]);
        let response: JsonRpcResponse<String> =
            self.rpc_call("eth_getTransactionCount", params).await?;

        let nonce_hex = response.result.ok_or_else(|| Error::Api {
            message: "No result from eth_getTransactionCount".to_string(),
            status: None,
        })?;

        u64::from_str_radix(nonce_hex.trim_start_matches("0x"), 16).map_err(|e| Error::Api {
            message: format!("Failed to parse nonce: {}", e),
            status: None,
        })
    }

    async fn get_gas_price_wei(&self) -> Result<u128> {
        let response: JsonRpcResponse<String> =
            self.rpc_call("eth_gasPrice", serde_json::json!([])).await?;
        let gas_hex = response.result.ok_or_else(|| Error::Api {
            message: "No result from eth_gasPrice".to_string(),
            status: None,
        })?;

        u128::from_str_radix(gas_hex.trim_start_matches("0x"), 16).map_err(|e| Error::Api {
            message: format!("Failed to parse gas price: {}", e),
            status: None,
        })
    }

    async fn get_native_balance_wei(&self, address: &Address) -> Result<u128> {
        let params = serde_json::json!([format!("{:?}", address), "latest"]);
        let response: JsonRpcResponse<String> = self.rpc_call("eth_getBalance", params).await?;
        let balance_hex = response.result.ok_or_else(|| Error::Api {
            message: "No result from eth_getBalance".to_string(),
            status: None,
        })?;

        u128::from_str_radix(balance_hex.trim_start_matches("0x"), 16).map_err(|e| Error::Api {
            message: format!("Failed to parse native balance: {}", e),
            status: None,
        })
    }

    async fn send_raw_tx(&self, signer: &PrivateKeySigner, tx: TxLegacy) -> Result<String> {
        use alloy_consensus::transaction::RlpEcdsaTx;
        use alloy_network::TxSignerSync;
        use alloy_primitives::bytes::BytesMut;

        let mut tx = tx;
        let sig = signer
            .sign_transaction_sync(&mut tx)
            .map_err(|e| Error::Api {
                message: format!("Failed to sign transaction: {}", e),
                status: None,
            })?;

        let mut encoded = BytesMut::new();
        tx.rlp_encode_signed(&sig, &mut encoded);

        let raw_hex = format!("0x{}", hex::encode(&encoded));
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_sendRawTransaction",
            "params": [raw_hex]
        });

        let response: serde_json::Value = self
            .http_client
            .post(&self.rpc_url)
            .json(&body)
            .send()
            .await?
            .json()
            .await?;

        if let Some(err) = response.get("error") {
            let message = err
                .get("message")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown RPC error");
            return Err(Error::Api {
                message: format!("eth_sendRawTransaction error: {}", message),
                status: None,
            });
        }

        response
            .get("result")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string())
            .ok_or_else(|| Error::Api {
                message: "Missing tx hash from eth_sendRawTransaction".to_string(),
                status: None,
            })
    }
}

fn encode_erc20_transfer(recipient: Address, amount: U256) -> Bytes {
    let mut data = Vec::with_capacity(68);
    data.extend_from_slice(&ERC20_TRANSFER_SELECTOR);
    data.extend_from_slice(&[0u8; 12]);
    data.extend_from_slice(recipient.as_slice());
    data.extend_from_slice(&amount.to_be_bytes::<32>());
    Bytes::from(data)
}

fn parse_hex_u64(value: &str) -> Option<u64> {
    u64::from_str_radix(value.trim_start_matches("0x"), 16).ok()
}

fn topic_for_address(wallet_address: &str) -> String {
    format!(
        "0x{:0>64}",
        wallet_address.trim_start_matches("0x").to_lowercase()
    )
}

fn extract_ctf_token_ids(log: &Log) -> Result<Vec<String>> {
    let Some(topic_0) = log.topics.first() else {
        return Ok(Vec::new());
    };

    let data = log.data.trim_start_matches("0x");
    match topic_0.as_str() {
        ERC1155_TRANSFER_SINGLE_TOPIC => {
            if data.len() < 64 {
                return Ok(Vec::new());
            }

            let token_id = U256::from_str_radix(&data[..64], 16).map_err(|e| Error::Api {
                message: format!("Failed to parse ERC-1155 TransferSingle token id: {e}"),
                status: None,
            })?;
            Ok(vec![token_id.to_string()])
        }
        ERC1155_TRANSFER_BATCH_TOPIC => {
            let ids_offset = parse_u256_word(data, 0)?;
            let ids_offset_words = u256_to_usize(ids_offset)? / 32;
            let ids_len = u256_to_usize(parse_u256_word(data, ids_offset_words)?)?;

            let mut token_ids = Vec::with_capacity(ids_len);
            for index in 0..ids_len {
                let token_id = parse_u256_word(data, ids_offset_words + 1 + index)?;
                token_ids.push(token_id.to_string());
            }
            Ok(token_ids)
        }
        _ => Ok(Vec::new()),
    }
}

fn parse_u256_word(data: &str, word_index: usize) -> Result<U256> {
    let start = word_index.saturating_mul(64);
    let end = start.saturating_add(64);
    if end > data.len() {
        return Err(Error::Api {
            message: format!("ERC-1155 log data too short for word index {}", word_index),
            status: None,
        });
    }

    U256::from_str_radix(&data[start..end], 16).map_err(|e| Error::Api {
        message: format!("Failed to parse ERC-1155 log word {}: {}", word_index, e),
        status: None,
    })
}

fn u256_to_usize(value: U256) -> Result<usize> {
    value.to_string().parse::<usize>().map_err(|e| Error::Api {
        message: format!("Failed to convert U256 value to usize: {}", e),
        status: None,
    })
}

#[derive(Debug, Serialize)]
struct JsonRpcRequest<'a> {
    jsonrpc: &'a str,
    id: u64,
    method: &'a str,
    params: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct JsonRpcResponse<T> {
    result: Option<T>,
    error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct JsonRpcError {
    code: i64,
    message: String,
}

/// Ethereum log entry.
#[derive(Debug, Clone, Deserialize)]
pub struct Log {
    pub address: String,
    pub topics: Vec<String>,
    pub data: String,
    #[serde(rename = "blockNumber")]
    pub block_number: String,
    #[serde(rename = "transactionHash")]
    pub transaction_hash: String,
    #[serde(rename = "logIndex")]
    pub log_index: String,
}

/// Asset transfer from Alchemy enhanced API.
#[derive(Debug, Clone, Deserialize)]
pub struct AssetTransfer {
    pub from: String,
    pub to: String,
    pub value: Option<f64>,
    pub asset: Option<String>,
    pub hash: String,
    #[serde(rename = "blockNum")]
    pub block_num: String,
    pub metadata: Option<TransferMetadata>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TransferMetadata {
    #[serde(rename = "blockTimestamp")]
    pub block_timestamp: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct AssetTransfersResponse {
    transfers: Vec<AssetTransfer>,
    #[serde(rename = "pageKey")]
    page_key: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{
        extract_ctf_token_ids, Log, ERC1155_TRANSFER_BATCH_TOPIC, ERC1155_TRANSFER_SINGLE_TOPIC,
    };

    fn hex_word(value: u64) -> String {
        format!("{value:064x}")
    }

    #[test]
    fn extract_ctf_token_ids_parses_transfer_single() {
        let log = Log {
            address: "0x0".to_string(),
            topics: vec![ERC1155_TRANSFER_SINGLE_TOPIC.to_string()],
            data: format!("0x{}{}", hex_word(42), hex_word(1)),
            block_number: "0x1".to_string(),
            transaction_hash: "0xabc".to_string(),
            log_index: "0x0".to_string(),
        };

        let token_ids = extract_ctf_token_ids(&log).expect("single transfer should parse");
        assert_eq!(token_ids, vec!["42".to_string()]);
    }

    #[test]
    fn extract_ctf_token_ids_parses_transfer_batch() {
        let data = format!(
            "0x{}{}{}{}{}{}{}{}",
            hex_word(64),  // ids offset
            hex_word(160), // values offset
            hex_word(2),   // ids length
            hex_word(7),   // id[0]
            hex_word(9),   // id[1]
            hex_word(2),   // values length
            hex_word(1),   // value[0]
            hex_word(1),   // value[1]
        );
        let log = Log {
            address: "0x0".to_string(),
            topics: vec![ERC1155_TRANSFER_BATCH_TOPIC.to_string()],
            data,
            block_number: "0x1".to_string(),
            transaction_hash: "0xabc".to_string(),
            log_index: "0x0".to_string(),
        };

        let token_ids = extract_ctf_token_ids(&log).expect("batch transfer should parse");
        assert_eq!(token_ids, vec!["7".to_string(), "9".to_string()]);
    }
}
