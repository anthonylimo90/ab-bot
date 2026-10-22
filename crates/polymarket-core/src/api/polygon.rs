//! Polygon RPC client for on-chain data.

use crate::signing::{
    COLLATERAL_ADDRESS, COLLATERAL_OFFRAMP_ADDRESS, COLLATERAL_ONRAMP_ADDRESS, CTF_ADDRESS,
    USDC_ADDRESS as POLYGON_USDC_ADDRESS,
};
use crate::{Error, Result};
use alloy_consensus::TxLegacy;
use alloy_primitives::U256;
use alloy_primitives::{Address, Bytes, TxKind};
use alloy_signer_local::PrivateKeySigner;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::str::FromStr;
use std::time::Duration;

const ERC1155_TRANSFER_SINGLE_TOPIC: &str =
    "0xc3d58168c5ae7397731d063d5bbf3d657854427343f4c083240f7aacaa2d0f62";
const ERC1155_TRANSFER_BATCH_TOPIC: &str =
    "0x4a39dc06d4c0dbc64b70af90fd698a233a518aa5d07e595d983b8c0526c8f7fb";
const ERC20_TRANSFER_SELECTOR: [u8; 4] = [0xa9, 0x05, 0x9c, 0xbb];
const POLYGON_CHAIN_ID: u64 = 137;
const DEFAULT_ERC20_TRANSFER_GAS_LIMIT: u64 = 100_000;
/// The least gas a collateral wrap or unwrap is given, whatever the node
/// estimates.
const MIN_RAMP_GAS_LIMIT: u64 = 150_000;
/// What the error of a transaction the node expects to revert starts with.
pub const CALL_WOULD_FAIL: &str = "The node expects the call to fail";

/// How far behind the head balances are read, so every node behind a load
/// balancer already has the block.
pub const BALANCE_BLOCK_LAG: u64 = 2;

/// The cash a wallet holds: USDC.e, and pUSD (the exchange's collateral,
/// redeemable 1:1 for USDC.e). Both are read at one block, so a wrap or
/// unwrap mined between two reads can never be counted twice or not at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CashBalances {
    /// USDC.e, in 6-decimal base units.
    pub usdc_units: u128,
    /// pUSD, in 6-decimal base units.
    pub pusd_units: u128,
    /// The block both were read at.
    pub block: u64,
}

/// The token contracts and Polymarket contracts on Polygon (and the zero
/// address): funds sent to one of them cannot be recovered.
pub fn is_protocol_contract(address: &str) -> bool {
    use crate::signing::{
        CTF_EXCHANGE_ADDRESS, NEG_RISK_ADAPTER_ADDRESS, NEG_RISK_CTF_EXCHANGE_ADDRESS,
    };
    [
        "0x0000000000000000000000000000000000000000",
        POLYGON_USDC_ADDRESS,
        COLLATERAL_ADDRESS,
        COLLATERAL_ONRAMP_ADDRESS,
        COLLATERAL_OFFRAMP_ADDRESS,
        CTF_ADDRESS,
        CTF_EXCHANGE_ADDRESS,
        NEG_RISK_CTF_EXCHANGE_ADDRESS,
        NEG_RISK_ADAPTER_ADDRESS,
        // The V1 exchanges.
        "0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E",
        "0xC5d563A36AE78145C45a50134d48A1215220f80a",
        // Native USDC.
        "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359",
    ]
    .iter()
    .any(|contract| contract.eq_ignore_ascii_case(address.trim()))
}

impl CashBalances {
    pub fn total_units(&self) -> u128 {
        self.usdc_units.saturating_add(self.pusd_units)
    }

    /// The total in USDC.
    pub fn total(&self) -> Decimal {
        Decimal::from_i128_with_scale(i128::try_from(self.total_units()).unwrap_or(i128::MAX), 6)
    }
}

/// Where a withdrawal's USDC.e comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WithdrawalSource {
    /// The wallet's USDC.e, sent with an ERC-20 transfer.
    Usdc,
    /// The wallet's pUSD, which the Collateral Offramp unwraps and pays to
    /// the recipient as USDC.e.
    Pusd,
}

/// Polygon RPC client for querying blockchain data.
#[derive(Clone)]
pub struct PolygonClient {
    rpc_url: String,
    http_client: reqwest::Client,
}

/// A signed transaction whose hash is known before it is broadcast, so it can
/// be journaled first and found on chain whatever happens to the broadcast.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedTransaction {
    /// `0x`-prefixed RLP encoding, ready for `eth_sendRawTransaction`.
    pub raw: String,
    /// `0x`-prefixed keccak-256 of `raw`.
    pub tx_hash: String,
    pub nonce: u64,
    /// Gas price in wei.
    pub gas_price: u128,
}

/// What the node said about a broadcast.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BroadcastOutcome {
    /// The node holds the transaction (newly accepted or already known).
    Accepted,
    /// The node refused it for a reason that can never apply to a transaction
    /// it holds (e.g. insufficient funds). Carries the node's reason.
    Rejected(String),
    /// The node answered with an error that does not settle whether it holds
    /// the transaction (e.g. "nonce too low" after its own retry). Only the
    /// chain can tell.
    Uncertain(String),
}

/// Classify a JSON-RPC error answering `eth_sendRawTransaction`.
fn classify_send_error(message: &str) -> BroadcastOutcome {
    let lower = message.to_ascii_lowercase();
    const HELD: [&str; 5] = [
        "already known",
        "known transaction",
        "already imported",
        "alreadyknown",
        "already in mempool",
    ];
    // Validation failures a node reports only for a transaction it refused.
    const REFUSED: [&str; 6] = [
        "insufficient funds",
        "intrinsic gas too low",
        "exceeds block gas limit",
        "invalid sender",
        "invalid chain id",
        "oversized data",
    ];
    if HELD.iter().any(|pattern| lower.contains(pattern)) {
        BroadcastOutcome::Accepted
    } else if REFUSED.iter().any(|pattern| lower.contains(pattern)) {
        BroadcastOutcome::Rejected(message.to_string())
    } else {
        BroadcastOutcome::Uncertain(message.to_string())
    }
}

/// The message of a JSON-RPC `error` member (an object or a bare string).
fn rpc_error_message(error: &serde_json::Value) -> String {
    match error {
        serde_json::Value::String(message) => message.clone(),
        other => other
            .get("message")
            .and_then(|value| value.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| other.to_string()),
    }
}

/// A mined transaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MinedTransaction {
    /// `false` when the transaction reverted (it was mined but did nothing).
    pub success: bool,
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
        contract_addresses: &[&str],
        from_block: u64,
        to_block: u64,
        topics: serde_json::Value,
    ) -> Result<Vec<Log>> {
        let params = serde_json::json!([{
            "address": contract_addresses,
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
                .get_logs_with_topics(&[CTF_ADDRESS], from_block, to_block, filter)
                .await?;
            for log in logs {
                for token_id in extract_ctf_token_ids(&log)? {
                    token_ids.insert(token_id);
                }
            }
        }

        Ok(token_ids)
    }

    /// Get ERC-20 transfers involving a wallet address (via Alchemy enhanced API).
    ///
    /// `alchemy_getAssetTransfers` filters on exactly one side per call, so the
    /// wallet's outgoing (`fromAddress`) and incoming (`toAddress`) transfers are
    /// fetched with two calls and merged. A transfer that matches both filters
    /// (e.g. a self-transfer) is returned once, identified by Alchemy's
    /// `uniqueId` (`<tx hash>:log:<log index>`), falling back to a
    /// hash/from/to/value/asset key. The result is ordered by block number.
    ///
    /// Each direction returns at most 1000 transfers (oldest first); paging via
    /// `pageKey` is not implemented. If either call fails the whole call fails,
    /// so callers never see a one-directional view of the wallet.
    pub async fn get_asset_transfers(
        &self,
        address: &str,
        from_block: Option<u64>,
        to_block: Option<u64>,
    ) -> Result<Vec<AssetTransfer>> {
        let mut per_direction = Vec::with_capacity(2);
        for direction in [TransferDirection::From, TransferDirection::To] {
            let params = asset_transfers_params(address, direction, from_block, to_block);
            let response: JsonRpcResponse<AssetTransfersResponse> =
                self.rpc_call("alchemy_getAssetTransfers", params).await?;
            let result = response.result.ok_or_else(|| Error::Api {
                message: format!(
                    "alchemy_getAssetTransfers ({}) returned no result: {}",
                    direction.filter_key(),
                    response.error.map(|e| e.message).unwrap_or_default()
                ),
                status: None,
            })?;
            per_direction.push(result.transfers);
        }

        Ok(merge_asset_transfers(per_direction))
    }

    /// The wallet's USDC.e and pUSD, read at one recent block (named by its
    /// hash, so a reorganization between the reads cannot mix two blocks).
    pub async fn get_cash_balances(&self, wallet_address: &str) -> Result<CashBalances> {
        let block = self
            .get_block_number()
            .await?
            .saturating_sub(BALANCE_BLOCK_LAG);
        let header = self
            .rpc_value(
                "eth_getBlockByNumber",
                serde_json::json!([format!("{block:#x}"), false]),
            )
            .await?;
        let hash = header
            .get("hash")
            .and_then(|hash| hash.as_str())
            .ok_or_else(|| Error::Api {
                message: format!("No hash for block {block}"),
                status: None,
            })?;
        let at = serde_json::json!({ "blockHash": hash });
        let usdc_units = self
            .get_erc20_balance_units_at(wallet_address, POLYGON_USDC_ADDRESS, at.clone())
            .await?;
        let pusd_units = self
            .get_erc20_balance_units_at(wallet_address, COLLATERAL_ADDRESS, at)
            .await?;
        Ok(CashBalances {
            usdc_units,
            pusd_units,
            block,
        })
    }

    /// The wallet's USDC.e balance now (6-decimal base units).
    pub async fn get_usdc_balance_units(&self, wallet_address: &str) -> Result<u128> {
        self.get_erc20_balance_units_at(wallet_address, POLYGON_USDC_ADDRESS, "latest".into())
            .await
    }

    /// Sign a withdrawal of `amount_units` USDC.e to `to_address` from
    /// `source`, at the sender's next pending nonce, paying at least
    /// `min_gas_price` wei for gas, after checking it holds enough POL for
    /// gas. A withdrawal the node expects to fail (the token already moved, a
    /// paused Offramp) is refused here. Nothing is sent.
    pub async fn prepare_withdrawal(
        &self,
        signer: &PrivateKeySigner,
        to_address: &str,
        amount_units: u128,
        source: WithdrawalSource,
        min_gas_price: u128,
    ) -> Result<SignedTransaction> {
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
        let amount = U256::from(amount_units);
        let (to, input, min_gas_limit) = match source {
            WithdrawalSource::Usdc => (
                usdc,
                encode_erc20_transfer(recipient, amount),
                DEFAULT_ERC20_TRANSFER_GAS_LIMIT,
            ),
            WithdrawalSource::Pusd => (
                Address::from_str(COLLATERAL_OFFRAMP_ADDRESS).expect("invalid Offramp address"),
                encode_ramp_call("unwrap", usdc, recipient, amount),
                MIN_RAMP_GAS_LIMIT,
            ),
        };
        let gas_limit = self
            .gas_limit(signer.address(), to, &input, min_gas_limit)
            .await?;
        self.sign_call(signer, to, input, gas_limit, None, min_gas_price)
            .await
    }

    /// Sign a wrap of `amount_units` of the wallet's USDC.e into pUSD for
    /// itself, at exactly `nonce`. A wrap the node expects to fail (no
    /// allowance, a paused Onramp) is refused here. Nothing is sent.
    pub async fn prepare_wrap(
        &self,
        signer: &PrivateKeySigner,
        amount_units: u128,
        nonce: u64,
    ) -> Result<SignedTransaction> {
        let usdc = Address::from_str(POLYGON_USDC_ADDRESS).expect("invalid USDC address");
        let onramp = Address::from_str(COLLATERAL_ONRAMP_ADDRESS).expect("invalid Onramp address");
        let input = encode_ramp_call("wrap", usdc, signer.address(), U256::from(amount_units));
        let gas_limit = self
            .gas_limit(signer.address(), onramp, &input, MIN_RAMP_GAS_LIMIT)
            .await?;
        self.sign_call(signer, onramp, input, gas_limit, Some(nonce), 0)
            .await
    }

    /// Sign `original` again at exactly `nonce`, making the same call with
    /// the same gas limit, paying at least `min_gas_price` wei for gas: a
    /// replacement for it. Two transactions at one nonce can never both be
    /// mined. Nothing is sent.
    pub async fn prepare_replacement(
        &self,
        signer: &PrivateKeySigner,
        original: &str,
        nonce: u64,
        min_gas_price: u128,
    ) -> Result<SignedTransaction> {
        let tx = decode_legacy(original)
            .filter(|tx| tx.value.is_zero())
            .ok_or_else(|| Error::Api {
                message: "The transaction to replace is unreadable".to_string(),
                status: None,
            })?;
        let TxKind::Call(to) = tx.to else {
            return Err(Error::Api {
                message: "The transaction to replace calls no contract".to_string(),
                status: None,
            });
        };
        self.sign_call(
            signer,
            to,
            tx.input,
            tx.gas_limit,
            Some(nonce),
            min_gas_price,
        )
        .await
    }

    /// The network's current gas price, in wei.
    pub async fn gas_price_wei(&self) -> Result<u128> {
        self.get_gas_price_wei().await
    }

    /// The sender's transaction count mined at the head, and with those still
    /// pending: equal when nothing it sent waits to be mined.
    pub async fn nonces(&self, address: &Address) -> Result<(u64, u64)> {
        Ok((
            self.get_nonce(address, "latest").await?,
            self.get_nonce(address, "pending").await?,
        ))
    }

    /// Gas for a call: half again the node's estimate, and never less than
    /// `min`. Estimating simulates the call, so one the node expects to
    /// revert is an error, told apart from a node that could not estimate.
    async fn gas_limit(&self, from: Address, to: Address, input: &Bytes, min: u64) -> Result<u64> {
        let params = serde_json::json!([{
            "from": format!("{from:#x}"),
            "to": format!("{to:#x}"),
            "data": format!("0x{}", hex::encode(input)),
        }]);
        let response = self.rpc_response("eth_estimateGas", params).await?;
        if let Some(error) = response.get("error").filter(|error| !error.is_null()) {
            let message = rpc_error_message(error);
            let reverted = error.get("code").and_then(|code| code.as_i64()) == Some(3)
                || message.to_ascii_lowercase().contains("revert");
            return Err(Error::Api {
                message: if reverted {
                    format!("{CALL_WOULD_FAIL}: {message}")
                } else {
                    format!("Could not estimate gas: {message}")
                },
                status: None,
            });
        }
        let estimate = response
            .get("result")
            .and_then(|value| value.as_str())
            .and_then(parse_hex_u64)
            .ok_or_else(|| Error::Api {
                message: "No gas estimate from eth_estimateGas".to_string(),
                status: None,
            })?;
        Ok(estimate.saturating_add(estimate / 2).max(min))
    }

    /// Sign a call of `to` with `input`, at `nonce` (else the sender's next
    /// pending nonce), paying the network's gas price plus a fifth and at
    /// least `min_gas_price`, after checking the sender holds the POL its gas
    /// may cost.
    async fn sign_call(
        &self,
        signer: &PrivateKeySigner,
        to: Address,
        input: Bytes,
        gas_limit: u64,
        nonce: Option<u64>,
        min_gas_price: u128,
    ) -> Result<SignedTransaction> {
        let sender = signer.address();
        let pol_balance = self.get_native_balance_wei(&sender).await?;
        // "pending" counts transactions still in the mempool, so a new
        // transfer never reuses (and replaces) the nonce of one not yet mined.
        let nonce = match nonce {
            Some(nonce) => nonce,
            None => self.get_nonce(&sender, "pending").await?,
        };
        let gas_price = self.get_gas_price_wei().await?;
        let gas_price = (gas_price + gas_price / 5).max(min_gas_price);
        let required_gas = gas_price.saturating_mul(gas_limit as u128);

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
            gas_limit,
            to: TxKind::Call(to),
            value: U256::ZERO,
            input,
        };

        sign_legacy_transaction(signer, tx)
    }

    /// Send the signed transaction `raw`, whose hash is `tx_hash`. An `Err`
    /// (transport failure, HTTP error, unreadable answer) leaves open whether
    /// the node received it.
    pub async fn broadcast(&self, raw: &str, tx_hash: &str) -> Result<BroadcastOutcome> {
        let response = self
            .rpc_response("eth_sendRawTransaction", serde_json::json!([raw]))
            .await?;
        if let Some(error) = response.get("error").filter(|error| !error.is_null()) {
            return Ok(classify_send_error(&rpc_error_message(error)));
        }
        match response.get("result").and_then(|value| value.as_str()) {
            Some(hash) if hash.eq_ignore_ascii_case(tx_hash) => Ok(BroadcastOutcome::Accepted),
            Some(hash) => Err(Error::Api {
                message: format!("Node reported hash {hash} for transaction {tx_hash}"),
                status: None,
            }),
            None => Err(Error::Api {
                message: "Missing tx hash from eth_sendRawTransaction".to_string(),
                status: None,
            }),
        }
    }

    /// A block deep enough not to be reorganized: the chain's `finalized`
    /// block, or 128 blocks below the head when the node does not report one.
    /// Never past the block balances are read at ([`BALANCE_BLOCK_LAG`] below
    /// the head), so a transfer counted final is in every balance read after.
    pub async fn safe_block_number(&self) -> Result<u64> {
        let head = self.get_block_number().await?;
        let finalized = self
            .rpc_value(
                "eth_getBlockByNumber",
                serde_json::json!(["finalized", false]),
            )
            .await
            .ok()
            .and_then(|block| {
                block
                    .get("number")
                    .and_then(|value| value.as_str())
                    .and_then(parse_hex_u64)
            });
        Ok(match finalized {
            Some(block) => block.min(head.saturating_sub(BALANCE_BLOCK_LAG)),
            None => head.saturating_sub(128),
        })
    }

    /// Number of transactions sent by `address` and mined by block `block`.
    pub async fn transaction_count_at(&self, address: &str, block: u64) -> Result<u64> {
        let address = Address::from_str(address).map_err(|e| Error::Api {
            message: format!("Invalid address: {}", e),
            status: None,
        })?;
        self.get_nonce(&address, &format!("0x{block:x}")).await
    }

    /// The receipt of a transaction, or `None` while it is not mined.
    pub async fn transaction_receipt(&self, tx_hash: &str) -> Result<Option<MinedTransaction>> {
        let receipt = self
            .rpc_value("eth_getTransactionReceipt", serde_json::json!([tx_hash]))
            .await?;
        if receipt.is_null() {
            return Ok(None);
        }
        // Only an explicit 1 or 0 decides success; anything else is an
        // unreadable receipt, never a revert (which would settle a transfer
        // as unsent).
        let success = match receipt
            .get("status")
            .and_then(|value| value.as_str())
            .and_then(parse_hex_u64)
        {
            Some(1) => true,
            Some(0) => false,
            _ => {
                return Err(Error::Api {
                    message: format!("Unreadable status in the receipt of {tx_hash}"),
                    status: None,
                })
            }
        };
        Ok(Some(MinedTransaction {
            success,
            block_number: receipt
                .get("blockNumber")
                .and_then(|value| value.as_str())
                .and_then(parse_hex_u64),
            gas_used: receipt
                .get("gasUsed")
                .and_then(|value| value.as_str())
                .and_then(parse_hex_u64),
        }))
    }

    /// Whether the node knows the transaction at all (pending or mined).
    pub async fn transaction_known(&self, tx_hash: &str) -> Result<bool> {
        let tx = self
            .rpc_value("eth_getTransactionByHash", serde_json::json!([tx_hash]))
            .await?;
        Ok(!tx.is_null())
    }

    /// Number of transactions from `address` that are mined (`"latest"`) or
    /// also pending (`"pending"`): the next nonce in each view.
    pub async fn transaction_count(&self, address: &str, block_tag: &str) -> Result<u64> {
        let address = Address::from_str(address).map_err(|e| Error::Api {
            message: format!("Invalid address: {}", e),
            status: None,
        })?;
        self.get_nonce(&address, block_tag).await
    }

    /// Call `method` and return its `result`, which may be JSON null. Any
    /// HTTP error, `error` member or missing `result` is an `Err`: never read
    /// as "not found".
    async fn rpc_value(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let response = self.rpc_response(method, params).await?;
        if let Some(error) = response.get("error").filter(|error| !error.is_null()) {
            return Err(Error::Api {
                message: format!("{method} RPC error: {}", rpc_error_message(error)),
                status: None,
            });
        }
        response.get("result").cloned().ok_or_else(|| Error::Api {
            message: format!("{method} answered without a result"),
            status: None,
        })
    }

    /// POST a JSON-RPC call and return the decoded body of a 2xx answer.
    async fn rpc_response(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params
        });
        let response = self
            .http_client
            .post(&self.rpc_url)
            .json(&body)
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            return Err(Error::Api {
                message: format!("{method} failed with HTTP {status}"),
                status: Some(status.as_u16()),
            });
        }
        Ok(response.json().await?)
    }

    async fn get_erc20_balance_units_at(
        &self,
        wallet_address: &str,
        token_address: &str,
        block: serde_json::Value,
    ) -> Result<u128> {
        // balanceOf(address) selector = 0x70a08231 + 32-byte left-padded address
        let addr = wallet_address.trim_start_matches("0x");
        let data = format!("0x70a08231{:0>64}", addr);

        let params = serde_json::json!([
            { "to": token_address, "data": data },
            block
        ]);

        let response: JsonRpcResponse<String> = self.rpc_call("eth_call", params).await?;
        let hex_balance = response.result.ok_or_else(|| Error::Api {
            message: format!(
                "No result from eth_call: {}",
                response.error.map(|e| e.message).unwrap_or_default()
            ),
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

    async fn get_nonce(&self, address: &Address, block_tag: &str) -> Result<u64> {
        let params = serde_json::json!([format!("{:?}", address), block_tag]);
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
}

/// A signed legacy transaction, from its `0x`-prefixed raw encoding; `None`
/// when it is not one.
fn decode_legacy(raw: &str) -> Option<TxLegacy> {
    use alloy_consensus::transaction::RlpEcdsaTx;
    let bytes = hex::decode(raw.strip_prefix("0x").unwrap_or(raw)).ok()?;
    let mut buf = bytes.as_slice();
    let (tx, _) = TxLegacy::rlp_decode_with_signature(&mut buf).ok()?;
    buf.is_empty().then_some(tx)
}

/// The nonce and gas price (wei) of a signed legacy transaction, from its
/// `0x`-prefixed raw encoding; `None` when it is not one.
pub fn legacy_nonce_and_gas_price(raw: &str) -> Option<(u64, u128)> {
    decode_legacy(raw).map(|tx| (tx.nonce, tx.gas_price))
}

/// What a signed legacy transaction does, apart from its nonce and gas: the
/// contract it calls (lower-case hex), the value it sends and its call data.
pub fn legacy_call(raw: &str) -> Option<(String, U256, Bytes)> {
    let tx = decode_legacy(raw)?;
    match tx.to {
        TxKind::Call(address) => Some((format!("{address:#x}"), tx.value, tx.input)),
        TxKind::Create => None,
    }
}

/// Sign a legacy transaction and compute its hash locally.
fn sign_legacy_transaction(signer: &PrivateKeySigner, tx: TxLegacy) -> Result<SignedTransaction> {
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
    Ok(SignedTransaction {
        raw: format!("0x{}", hex::encode(&encoded)),
        tx_hash: format!("{:#x}", alloy_primitives::keccak256(&encoded)),
        nonce: tx.nonce,
        gas_price: tx.gas_price,
    })
}

/// Call data of a Collateral Onramp `wrap` or Offramp `unwrap`:
/// `name(address asset, address to, uint256 amount)`.
fn encode_ramp_call(name: &str, asset: Address, to: Address, amount: U256) -> Bytes {
    let signature = format!("{name}(address,address,uint256)");
    let mut data = Vec::with_capacity(100);
    data.extend_from_slice(&alloy_primitives::keccak256(signature.as_bytes())[..4]);
    data.extend_from_slice(&[0u8; 12]);
    data.extend_from_slice(asset.as_slice());
    data.extend_from_slice(&[0u8; 12]);
    data.extend_from_slice(to.as_slice());
    data.extend_from_slice(&amount.to_be_bytes::<32>());
    Bytes::from(data)
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

/// Which side of a transfer the queried wallet is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransferDirection {
    /// Transfers sent by the wallet.
    From,
    /// Transfers received by the wallet.
    To,
}

impl TransferDirection {
    fn filter_key(self) -> &'static str {
        match self {
            TransferDirection::From => "fromAddress",
            TransferDirection::To => "toAddress",
        }
    }
}

/// Build the `alchemy_getAssetTransfers` params for one direction.
fn asset_transfers_params(
    address: &str,
    direction: TransferDirection,
    from_block: Option<u64>,
    to_block: Option<u64>,
) -> serde_json::Value {
    let mut filter = serde_json::json!({
        "fromBlock": from_block.map(|b| format!("0x{:x}", b)).unwrap_or_else(|| "0x0".to_string()),
        "toBlock": to_block.map(|b| format!("0x{:x}", b)).unwrap_or_else(|| "latest".to_string()),
        "category": ["erc20"],
        "withMetadata": true,
        "maxCount": "0x3e8", // 1000
        // Newest first: one page must describe the wallet's current behaviour,
        // not the first 1000 transfers it ever made (Alchemy defaults to oldest).
        "order": "desc"
    });
    filter[direction.filter_key()] = serde_json::Value::String(address.to_string());
    serde_json::json!([filter])
}

/// Identity of a transfer for de-duplication across the two direction queries.
fn transfer_identity(transfer: &AssetTransfer) -> String {
    match &transfer.unique_id {
        Some(id) if !id.is_empty() => id.to_lowercase(),
        _ => format!(
            "{}|{}|{}|{:?}|{:?}",
            transfer.hash.to_lowercase(),
            transfer.from.to_lowercase(),
            transfer.to.to_lowercase(),
            transfer.value.map(f64::to_bits),
            transfer.asset
        ),
    }
}

/// Merge per-direction transfer lists: drop duplicates (first occurrence wins)
/// and order by block number. The sort is stable, so transfers within a block
/// keep their relative order; blocks that cannot be parsed sort last.
fn merge_asset_transfers(lists: Vec<Vec<AssetTransfer>>) -> Vec<AssetTransfer> {
    let mut seen = HashSet::new();
    let mut merged: Vec<AssetTransfer> = lists
        .into_iter()
        .flatten()
        .filter(|transfer| seen.insert(transfer_identity(transfer)))
        .collect();
    merged.sort_by_key(|t| parse_hex_u64(&t.block_num).unwrap_or(u64::MAX));
    merged
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
struct JsonRpcError {
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
    /// Alchemy's per-transfer id (`<tx hash>:log:<log index>`); distinguishes
    /// several transfers within one transaction.
    #[serde(rename = "uniqueId", default)]
    pub unique_id: Option<String>,
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
struct AssetTransfersResponse {
    transfers: Vec<AssetTransfer>,
}

#[cfg(test)]
mod tests {
    use super::{
        asset_transfers_params, extract_ctf_token_ids, merge_asset_transfers, AssetTransfer,
        AssetTransfersResponse, Log, TransferDirection, ERC1155_TRANSFER_BATCH_TOPIC,
        ERC1155_TRANSFER_SINGLE_TOPIC,
    };

    #[test]
    fn ramp_calls_encode_as_the_collateral_ramps_declare_them() {
        use super::*;
        alloy_sol_types::sol! {
            function wrap(address _asset, address _to, uint256 _amount) external;
            function unwrap(address _asset, address _to, uint256 _amount) external;
        }
        use alloy_sol_types::SolCall;
        let asset = Address::from_str(POLYGON_USDC_ADDRESS).unwrap();
        let to = Address::repeat_byte(0x42);
        let amount = U256::from(12_345_678u64);
        assert_eq!(
            encode_ramp_call("wrap", asset, to, amount).to_vec(),
            wrapCall {
                _asset: asset,
                _to: to,
                _amount: amount
            }
            .abi_encode()
        );
        assert_eq!(
            encode_ramp_call("unwrap", asset, to, amount).to_vec(),
            unwrapCall {
                _asset: asset,
                _to: to,
                _amount: amount
            }
            .abi_encode()
        );
    }

    #[test]
    fn cash_is_both_tokens_in_usdc() {
        let cash = super::CashBalances {
            usdc_units: 1_500_000,
            pusd_units: 2_250_001,
            block: 9,
        };
        assert_eq!(cash.total_units(), 3_750_001);
        assert_eq!(cash.total(), rust_decimal::Decimal::new(3_750_001, 6));
    }

    #[test]
    fn a_signed_transfer_reads_back_its_nonce_and_gas_price() {
        use super::*;
        let signer: PrivateKeySigner = format!("{:064x}", 1).parse().unwrap();
        let tx = TxLegacy {
            chain_id: Some(POLYGON_CHAIN_ID),
            nonce: 7,
            gas_price: 36_000_000_001,
            gas_limit: DEFAULT_ERC20_TRANSFER_GAS_LIMIT,
            to: TxKind::Call(Address::from_str(POLYGON_USDC_ADDRESS).unwrap()),
            value: U256::ZERO,
            input: encode_erc20_transfer(Address::ZERO, U256::from(5_000_000u64)),
        };
        let signed = sign_legacy_transaction(&signer, tx).unwrap();
        assert_eq!(
            legacy_nonce_and_gas_price(&signed.raw),
            Some((7, 36_000_000_001))
        );
        let (to, value, input) = legacy_call(&signed.raw).unwrap();
        assert_eq!(to, POLYGON_USDC_ADDRESS.to_lowercase());
        assert_eq!(value, U256::ZERO);
        assert_eq!(
            input,
            encode_erc20_transfer(Address::ZERO, U256::from(5_000_000u64))
        );
        assert_eq!(legacy_nonce_and_gas_price("0xraw01"), None);
        assert_eq!(legacy_call("0xraw01"), None);
        assert_eq!(
            legacy_nonce_and_gas_price(&format!("{}00", signed.raw)),
            None
        );
        assert_eq!(legacy_nonce_and_gas_price(""), None);
    }

    fn transfer(
        unique_id: Option<&str>,
        hash: &str,
        from: &str,
        to: &str,
        block: &str,
    ) -> AssetTransfer {
        AssetTransfer {
            from: from.to_string(),
            to: to.to_string(),
            value: Some(10.0),
            asset: Some("USDC".to_string()),
            hash: hash.to_string(),
            unique_id: unique_id.map(str::to_string),
            block_num: block.to_string(),
            metadata: None,
        }
    }

    #[test]
    fn asset_transfer_params_filter_on_the_requested_side_only() {
        let from = asset_transfers_params("0xabc", TransferDirection::From, None, None);
        let filter = &from[0];
        assert_eq!(filter["fromAddress"], "0xabc");
        assert!(filter.get("toAddress").is_none());
        assert_eq!(filter["fromBlock"], "0x0");
        assert_eq!(filter["toBlock"], "latest");
        assert_eq!(filter["maxCount"], "0x3e8");
        assert_eq!(filter["order"], "desc");
        assert_eq!(filter["category"][0], "erc20");

        let to = asset_transfers_params("0xabc", TransferDirection::To, Some(255), Some(4096));
        let filter = &to[0];
        assert_eq!(filter["toAddress"], "0xabc");
        assert!(filter.get("fromAddress").is_none());
        assert_eq!(filter["fromBlock"], "0xff");
        assert_eq!(filter["toBlock"], "0x1000");
    }

    #[test]
    fn merge_asset_transfers_dedupes_by_unique_id_and_orders_by_block() {
        let outgoing = vec![
            transfer(Some("0xaa:log:0x1"), "0xaa", "0xw", "0xex", "0x10"),
            // Self-transfer: appears in both direction queries.
            transfer(Some("0xcc:log:0x0"), "0xcc", "0xw", "0xw", "0x30"),
        ];
        let incoming = vec![
            transfer(Some("0xbb:log:0x2"), "0xbb", "0xex", "0xw", "0x20"),
            transfer(Some("0xCC:log:0x0"), "0xcc", "0xw", "0xw", "0x30"),
            // Same transaction, different log: must NOT be treated as a duplicate.
            transfer(Some("0xaa:log:0x3"), "0xaa", "0xex2", "0xw", "0x10"),
        ];

        let merged = merge_asset_transfers(vec![outgoing, incoming]);
        let ids: Vec<_> = merged
            .iter()
            .map(|t| t.unique_id.clone().unwrap())
            .collect();
        assert_eq!(
            ids,
            vec![
                "0xaa:log:0x1",
                "0xaa:log:0x3",
                "0xbb:log:0x2",
                "0xcc:log:0x0"
            ]
        );
    }

    #[test]
    fn merge_asset_transfers_falls_back_to_content_key_without_unique_id() {
        let a = transfer(None, "0xaa", "0xw", "0xex", "0x10");
        let same = transfer(None, "0xAA", "0xW", "0xEX", "0x10");
        let mut different_value = transfer(None, "0xaa", "0xw", "0xex", "0x10");
        different_value.value = Some(11.0);

        let merged = merge_asset_transfers(vec![vec![a], vec![same, different_value]]);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn merge_asset_transfers_sorts_unparseable_blocks_last() {
        let merged = merge_asset_transfers(vec![vec![
            transfer(Some("x"), "0x1", "0xw", "0xe", "garbage"),
            transfer(Some("y"), "0x2", "0xw", "0xe", "0x2"),
        ]]);
        assert_eq!(merged[0].block_num, "0x2");
        assert_eq!(merged[1].block_num, "garbage");
    }

    #[test]
    fn alchemy_response_with_unique_id_deserializes() {
        let json = r#"{
            "transfers": [{
                "blockNum": "0x1b4",
                "uniqueId": "0xabc:log:0x7",
                "hash": "0xabc",
                "from": "0x1",
                "to": "0x2",
                "value": 12.5,
                "asset": "USDC",
                "category": "erc20",
                "metadata": {"blockTimestamp": "2025-01-01T00:00:00.000Z"}
            }, {
                "blockNum": "0x1b5",
                "hash": "0xdef",
                "from": "0x2",
                "to": "0x1",
                "value": null,
                "asset": null
            }]
        }"#;
        let parsed: AssetTransfersResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.transfers.len(), 2);
        assert_eq!(
            parsed.transfers[0].unique_id.as_deref(),
            Some("0xabc:log:0x7")
        );
        assert_eq!(parsed.transfers[1].unique_id, None);
    }

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

    #[test]
    fn signed_transfer_hash_is_the_hash_the_chain_will_use() {
        use alloy_consensus::{SignableTransaction, TxLegacy};
        use alloy_network::TxSignerSync;
        use alloy_primitives::{Address, TxKind, U256};
        use std::str::FromStr;

        let signer: alloy_signer_local::PrivateKeySigner =
            "0x4c0883a69102937d6231471b5dbb6204fe5129617082792ae468d01a3f362318"
                .parse()
                .unwrap();
        let tx = TxLegacy {
            chain_id: Some(super::POLYGON_CHAIN_ID),
            nonce: 7,
            gas_price: 30_000_000_000,
            gas_limit: super::DEFAULT_ERC20_TRANSFER_GAS_LIMIT,
            to: TxKind::Call(Address::from_str(super::POLYGON_USDC_ADDRESS).unwrap()),
            value: U256::ZERO,
            input: super::encode_erc20_transfer(Address::ZERO, U256::from(1_000_000u64)),
        };
        let signed = super::sign_legacy_transaction(&signer, tx.clone()).unwrap();

        let mut reference = tx;
        let signature = signer.sign_transaction_sync(&mut reference).unwrap();
        let expected = reference.into_signed(signature);
        assert_eq!(signed.tx_hash, format!("{:#x}", expected.hash()));
        assert_eq!(signed.nonce, 7);
        assert!(signed.raw.starts_with("0x"));
    }

    #[test]
    fn only_deterministic_refusals_count_as_not_sent() {
        use super::{classify_send_error, BroadcastOutcome};
        for held in [
            "already known",
            "known transaction: 0xabc",
            "Transaction already imported",
            "AlreadyKnown",
        ] {
            assert_eq!(
                classify_send_error(held),
                BroadcastOutcome::Accepted,
                "{held}"
            );
        }
        for refused in [
            "insufficient funds for gas * price + value",
            "intrinsic gas too low",
            "invalid sender",
        ] {
            assert!(
                matches!(classify_send_error(refused), BroadcastOutcome::Rejected(_)),
                "{refused}"
            );
        }
        // A retrying provider can answer these for a transaction it holds.
        for unclear in [
            "nonce too low",
            "replacement transaction underpriced",
            "transaction underpriced",
            "Your app has exceeded its compute units per second capacity",
            "internal error",
            "unknown RPC error",
        ] {
            assert!(
                matches!(classify_send_error(unclear), BroadcastOutcome::Uncertain(_)),
                "{unclear}"
            );
        }
    }
}
