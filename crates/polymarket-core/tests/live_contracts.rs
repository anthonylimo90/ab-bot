//! Read-only checks of the bot's order signing and contract addresses
//! against the deployed Polymarket contracts on Polygon. Each order is
//! signed by the bot's own signer with a well-known test key that holds
//! nothing, and is only checked with `eth_call`: nothing is sent. They
//! reach the network, so they are ignored by default and never run in CI:
//!
//! ```bash
//! cargo test -p polymarket-core --test live_contracts -- --ignored --test-threads=1
//! ```
//!
//! `LIVE_POLYGON_RPC_URL` picks the Polygon node; by default it is the
//! public node the API server falls back to when none is configured.

use alloy_primitives::{Address, Bytes, FixedBytes, U256};
use alloy_signer_local::PrivateKeySigner;
use alloy_sol_types::{sol, SolCall};
use polymarket_core::api::polygon::PUBLIC_POLYGON_RPC_URL;
use polymarket_core::signing::{
    Eip712Domain, OrderSide, OrderSigner, SignedOrder, COLLATERAL_ADDRESS,
    COLLATERAL_OFFRAMP_ADDRESS, COLLATERAL_ONRAMP_ADDRESS, CTF_ADDRESS, CTF_EXCHANGE_ADDRESS,
    NEG_RISK_ADAPTER_ADDRESS, NEG_RISK_CTF_EXCHANGE_ADDRESS, USDC_ADDRESS,
};
use rust_decimal::Decimal;

sol! {
    struct Order {
        uint256 salt;
        address maker;
        address signer;
        uint256 tokenId;
        uint256 makerAmount;
        uint256 takerAmount;
        uint8 side;
        uint8 signatureType;
        uint256 timestamp;
        bytes32 metadata;
        bytes32 builder;
        bytes signature;
    }

    function hashOrder(Order order) external view returns (bytes32);
    function validateOrderSignature(bytes32 orderHash, Order order) external view;
    function validateOrder(Order order) external view;
    function eip712Domain() external view returns (
        bytes1 fields,
        string name,
        string version,
        uint256 chainId,
        address verifyingContract,
        bytes32 salt,
        uint256[] extensions
    );
    function getCollateral() external view returns (address);
    function getCtf() external view returns (address);
    function paused() external view returns (bool);
    function decimals() external view returns (uint8);
}

/// The first well-known development key (Hardhat/Anvil account 0): public,
/// and so never funded on mainnet.
const TEST_KEY: &str = "ac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80";

/// An outcome token id of the shape the exchange lists.
const TOKEN_ID: &str =
    "71321045679252212594626385532706912750332728571942532289631379312455583992563";

fn rpc_url() -> String {
    std::env::var("LIVE_POLYGON_RPC_URL").unwrap_or_else(|_| PUBLIC_POLYGON_RPC_URL.into())
}

fn address(text: &str) -> Address {
    text.parse().expect("address")
}

/// `eth_call` at the latest block: the return data, or the revert as `Err`.
async fn call(to: &str, data: Vec<u8>) -> Result<Vec<u8>, String> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "eth_call",
        "params": [{ "to": to, "data": format!("0x{}", hex::encode(data)) }, "latest"],
    });
    let answer: serde_json::Value = reqwest::Client::new()
        .post(rpc_url())
        .json(&body)
        .send()
        .await
        .expect("rpc reachable")
        .error_for_status()
        .expect("rpc answered")
        .json()
        .await
        .expect("rpc json");
    if let Some(error) = answer.get("error") {
        return Err(error.to_string());
    }
    let result = answer["result"].as_str().expect("result");
    Ok(hex::decode(result.trim_start_matches("0x")).expect("hex result"))
}

async fn code_size(address: &str) -> usize {
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "eth_getCode", "params": [address, "latest"],
    });
    let answer: serde_json::Value = reqwest::Client::new()
        .post(rpc_url())
        .json(&body)
        .send()
        .await
        .expect("rpc reachable")
        .json()
        .await
        .expect("rpc json");
    answer["result"]
        .as_str()
        .expect("code")
        .len()
        .saturating_sub(2)
        / 2
}

/// The order exactly as the bot sends it to the CLOB, read back into the
/// exchange's `Order` struct: the wire format is what gets matched.
fn on_chain_order(wire: &SignedOrder) -> Order {
    let word = |hex32: &str| hex32.parse::<FixedBytes<32>>().expect("bytes32");
    Order {
        salt: U256::from(wire.salt),
        maker: address(&wire.maker),
        signer: address(&wire.signer),
        tokenId: wire.token_id.parse().expect("token id"),
        makerAmount: wire.maker_amount.parse().expect("maker amount"),
        takerAmount: wire.taker_amount.parse().expect("taker amount"),
        side: match wire.side.as_str() {
            "BUY" => 0,
            "SELL" => 1,
            other => panic!("side {other}"),
        },
        signatureType: wire.signature_type,
        timestamp: wire.timestamp.parse().expect("timestamp"),
        metadata: word(&wire.metadata),
        builder: word(&wire.builder),
        signature: wire.signature.parse::<Bytes>().expect("signature"),
    }
}

async fn signed(signer: &OrderSigner, side: OrderSide) -> SignedOrder {
    let order = signer
        .order_builder()
        .token_id_str(TOKEN_ID)
        .side(side)
        .price(Decimal::new(37, 2))
        .size(Decimal::new(125, 1))
        .expires_at(0)
        .build()
        .expect("order");
    signer.sign_order(&order).await.expect("signed")
}

/// The exchange at `exchange` hashes the bot's order as the bot does, and
/// accepts its signature; the same order with a corrupted signature, or
/// signed for the other exchange, is refused.
async fn check_exchange(exchange: &str, signer: &OrderSigner, other: &OrderSigner) {
    for side in [OrderSide::Buy, OrderSide::Sell] {
        let wire = signed(signer, side).await;
        let order = on_chain_order(&wire);
        let bot_hash = wire.order_hash.clone().expect("order hash");

        let answer = call(
            exchange,
            hashOrderCall {
                order: order.clone(),
            }
            .abi_encode(),
        )
        .await
        .expect("hashOrder");
        let chain_hash = hashOrderCall::abi_decode_returns(&answer, true)
            .expect("bytes32")
            ._0;
        assert_eq!(format!("{chain_hash:#x}"), bot_hash, "{side:?} order hash");

        let validate = |order: Order| validateOrderSignatureCall {
            orderHash: chain_hash,
            order,
        };
        call(exchange, validate(order.clone()).abi_encode())
            .await
            .unwrap_or_else(|e| panic!("{side:?} signature refused: {e}"));
        call(
            exchange,
            validateOrderCall {
                order: order.clone(),
            }
            .abi_encode(),
        )
        .await
        .unwrap_or_else(|e| panic!("{side:?} order refused: {e}"));

        let mut corrupted = order.clone();
        let mut bytes = corrupted.signature.to_vec();
        bytes[10] ^= 0xff;
        corrupted.signature = bytes.into();
        assert!(
            call(exchange, validate(corrupted).abi_encode())
                .await
                .is_err(),
            "{side:?} corrupted signature accepted"
        );

        let elsewhere = on_chain_order(&signed(other, side).await);
        let elsewhere_hash = call(
            exchange,
            hashOrderCall {
                order: elsewhere.clone(),
            }
            .abi_encode(),
        )
        .await
        .expect("hashOrder");
        let elsewhere_hash = hashOrderCall::abi_decode_returns(&elsewhere_hash, true)
            .expect("bytes32")
            ._0;
        let wrong = validateOrderSignatureCall {
            orderHash: elsewhere_hash,
            order: elsewhere,
        };
        assert!(
            call(exchange, wrong.abi_encode()).await.is_err(),
            "{side:?} order signed for the other exchange accepted"
        );
    }
}

#[tokio::test]
#[ignore = "reaches a real Polygon node"]
async fn exchanges_accept_the_bots_signed_orders() {
    let key: PrivateKeySigner = TEST_KEY.parse().expect("key");
    let standard = OrderSigner::new(key.clone());
    let neg_risk = OrderSigner::new_neg_risk(key);
    check_exchange(CTF_EXCHANGE_ADDRESS, &standard, &neg_risk).await;
    check_exchange(NEG_RISK_CTF_EXCHANGE_ADDRESS, &neg_risk, &standard).await;
}

#[tokio::test]
#[ignore = "reaches a real Polygon node"]
async fn exchanges_publish_the_bots_domain_and_contracts() {
    for (exchange, domain) in [
        (CTF_EXCHANGE_ADDRESS, Eip712Domain::ctf_exchange()),
        (
            NEG_RISK_CTF_EXCHANGE_ADDRESS,
            Eip712Domain::neg_risk_ctf_exchange(),
        ),
    ] {
        let answer = call(exchange, eip712DomainCall {}.abi_encode())
            .await
            .expect("eip712Domain");
        let published = eip712DomainCall::abi_decode_returns(&answer, true).expect("domain");
        assert_eq!(published.name, domain.name);
        assert_eq!(published.version, domain.version);
        assert_eq!(published.chainId, domain.chain_id);
        assert_eq!(published.verifyingContract, domain.verifying_contract);

        let collateral = call(exchange, getCollateralCall {}.abi_encode())
            .await
            .expect("collateral");
        let collateral = getCollateralCall::abi_decode_returns(&collateral, true)
            .expect("address")
            ._0;
        assert_eq!(
            collateral,
            address(COLLATERAL_ADDRESS),
            "{exchange} collateral"
        );
        let ctf = call(exchange, getCtfCall {}.abi_encode())
            .await
            .expect("ctf");
        let ctf = getCtfCall::abi_decode_returns(&ctf, true)
            .expect("address")
            ._0;
        assert_eq!(ctf, address(CTF_ADDRESS), "{exchange} conditional tokens");
        let paused = call(exchange, pausedCall {}.abi_encode())
            .await
            .expect("paused");
        assert!(
            !pausedCall::abi_decode_returns(&paused, true)
                .expect("bool")
                ._0
        );
    }

    for token in [USDC_ADDRESS, COLLATERAL_ADDRESS] {
        let answer = call(token, decimalsCall {}.abi_encode())
            .await
            .expect("decimals");
        let decimals = decimalsCall::abi_decode_returns(&answer, true)
            .expect("uint8")
            ._0;
        assert_eq!(decimals, 6, "{token} decimals");
    }

    for contract in [
        CTF_EXCHANGE_ADDRESS,
        NEG_RISK_CTF_EXCHANGE_ADDRESS,
        NEG_RISK_ADAPTER_ADDRESS,
        COLLATERAL_ADDRESS,
        COLLATERAL_ONRAMP_ADDRESS,
        COLLATERAL_OFFRAMP_ADDRESS,
        USDC_ADDRESS,
        CTF_ADDRESS,
    ] {
        assert!(code_size(contract).await > 0, "no contract at {contract}");
    }
}
