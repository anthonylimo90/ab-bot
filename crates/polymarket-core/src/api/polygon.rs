//! Polygon RPC client for on-chain data.

use crate::signing::CTF_ADDRESS;
use crate::{Error, Result};
use alloy_primitives::U256;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::Duration;

/// USDC.e contract on Polygon (PoS bridged, 6 decimals) — used by Polymarket.
const POLYGON_USDC_ADDRESS: &str = "0x2791Bca1f2de4661ED88A30C99A7a9449Aa84174";

/// Native USDC contract on Polygon (CCTP, 6 decimals) — NOT used by Polymarket.
const POLYGON_NATIVE_USDC_ADDRESS: &str = "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359";
const ERC1155_TRANSFER_SINGLE_TOPIC: &str =
    "0xc3d58168c5ae7397731d063d5bbf3d657854427343f4c083240f7aacaa2d0f62";
const ERC1155_TRANSFER_BATCH_TOPIC: &str =
    "0x4a39dc06d4c0dbc64b70af90fd698a233a518aa5d07e595d983b8c0526c8f7fb";

/// Polygon RPC client for querying blockchain data.
#[derive(Clone)]
pub struct PolygonClient {
    rpc_url: String,
    http_client: reqwest::Client,
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

    /// Get the native USDC (CCTP) balance for a wallet — NOT used by Polymarket.
    pub async fn get_native_usdc_balance(&self, wallet_address: &str) -> Result<f64> {
        self.get_erc20_balance(wallet_address, POLYGON_NATIVE_USDC_ADDRESS)
            .await
    }

    /// Get a 6-decimal ERC-20 balance for a wallet address (returns human-readable amount).
    async fn get_erc20_balance(&self, wallet_address: &str, token_address: &str) -> Result<f64> {
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

        // USDC has 6 decimals
        Ok(balance as f64 / 1_000_000.0)
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
