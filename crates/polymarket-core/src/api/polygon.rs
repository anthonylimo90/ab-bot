//! Polygon RPC client for on-chain data.

use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// USDC.e contract on Polygon (PoS bridged, 6 decimals) — used by Polymarket.
const POLYGON_USDC_ADDRESS: &str = "0x2791Bca1f2de4661ED88A30C99A7a9449Aa84174";

/// Native USDC contract on Polygon (CCTP, 6 decimals) — NOT used by Polymarket.
const POLYGON_NATIVE_USDC_ADDRESS: &str = "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359";

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
