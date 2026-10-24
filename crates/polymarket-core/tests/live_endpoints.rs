//! Read-only checks of the real Polymarket and Polygon endpoints through the
//! bot's own clients: they prove the responses still parse into what the bot
//! relies on. They reach the network, so they are ignored by default and
//! never run in CI. Run them on demand:
//!
//! ```bash
//! cargo test -p polymarket-core --test live_endpoints -- --ignored --test-threads=1
//! ```
//!
//! `LIVE_POLYGON_RPC_URL` picks the Polygon node; by default it is the
//! public node the API server falls back to when none is configured.
//! Nothing here signs, sends or writes anything.

use polymarket_core::api::polygon::PUBLIC_POLYGON_RPC_URL;
use polymarket_core::api::{ClobClient, PolygonClient};
use polymarket_core::types::{Market, OrderBook};
use rust_decimal::Decimal;

/// A market settled long ago: "Will Ron DeSantis win the U.S. 2024
/// Republican presidential nomination?", which NO won.
const SETTLED_MARKET: &str = "0x12a0cb60174abc437bf1178367c72d11f069e1a3add20b148fb0ab4279b772b2";

/// The Conditional Tokens contract, which holds the collateral behind every
/// outstanding outcome token.
const CONDITIONAL_TOKENS: &str = "0x4D97DCd97eC945f40cF65F87097ACe5EA0476045";

fn clob() -> ClobClient {
    ClobClient::new(None, None)
}

fn polygon() -> PolygonClient {
    PolygonClient::new(
        std::env::var("LIVE_POLYGON_RPC_URL").unwrap_or_else(|_| PUBLIC_POLYGON_RPC_URL.into()),
    )
}

/// A binary market that takes orders now and has both sides quoted.
async fn open_market(clob: &ClobClient) -> (Market, OrderBook) {
    let markets = clob.get_markets().await.expect("active markets");
    assert!(!markets.is_empty(), "no active markets");
    let candidates = markets
        .into_iter()
        .filter(|m| !m.resolved && m.accepting_orders == Some(true) && m.outcomes.len() == 2);
    for market in candidates.take(40) {
        let token = &market.outcomes[0].token_id;
        if let Ok(book) = clob.get_order_book(token).await {
            if !book.bids.is_empty() && !book.asks.is_empty() {
                return (market, book);
            }
        }
    }
    panic!("no open market with a two-sided book among the first 40");
}

fn assert_valid_book(book: &OrderBook) {
    for level in book.bids.iter().chain(&book.asks) {
        assert!(
            level.price > Decimal::ZERO && level.price < Decimal::ONE,
            "{level:?}"
        );
        assert!(level.size > Decimal::ZERO, "{level:?}");
    }
    assert!(
        book.bids.windows(2).all(|w| w[0].price > w[1].price),
        "bids not best (highest) first: {:?}",
        book.bids
    );
    assert!(
        book.asks.windows(2).all(|w| w[0].price < w[1].price),
        "asks not best (lowest) first: {:?}",
        book.asks
    );
    if let (Some(bid), Some(ask)) = (book.bids.first(), book.asks.first()) {
        assert!(bid.price < ask.price, "crossed book: {bid:?} >= {ask:?}");
    }
}

#[tokio::test]
#[ignore = "reaches the real Polymarket API"]
async fn open_market_book_and_trading_terms_parse() {
    let clob = clob();
    let (market, book) = open_market(&clob).await;
    println!("market {} {:?}", market.id, market.question);

    let tokens: Vec<&str> = market
        .outcomes
        .iter()
        .map(|o| o.token_id.as_str())
        .collect();
    assert_ne!(tokens[0], tokens[1]);
    for token in &tokens {
        assert!(
            !token.is_empty() && token.bytes().all(|b| b.is_ascii_digit()),
            "{token}"
        );
    }
    assert_eq!(book.outcome_id, tokens[0]);
    assert_eq!(book.market_id, market.id);
    assert_valid_book(&book);

    let other = clob.get_order_book(tokens[1]).await.expect("second book");
    assert_eq!(other.outcome_id, tokens[1]);
    assert_valid_book(&other);

    // The same market fetched alone matches the listing.
    let single = clob
        .get_market_by_id(&market.id)
        .await
        .expect("market by id");
    assert_eq!(single.id, market.id);
    let single_tokens: Vec<&str> = single
        .outcomes
        .iter()
        .map(|o| o.token_id.as_str())
        .collect();
    assert_eq!(single_tokens, tokens);
    assert!(!single.resolved);

    let tick = clob.get_tick_size(tokens[0]).await.expect("tick size");
    assert!(tick > Decimal::ZERO && tick < Decimal::ONE, "{tick}");
    for level in book.bids.iter().chain(&book.asks) {
        assert!(
            (level.price % tick).is_zero(),
            "{} is off the {tick} grid",
            level.price
        );
    }

    let neg_risk = clob.get_neg_risk(tokens[0]).await.expect("neg risk");
    assert_eq!(
        neg_risk,
        clob.get_neg_risk(tokens[1]).await.expect("neg risk")
    );

    let fees = clob
        .get_fee_schedule(&market.id)
        .await
        .expect("fee schedule");
    assert!(
        fees.rate >= Decimal::ZERO && fees.rate < Decimal::ONE,
        "{fees:?}"
    );
    assert!(fees.exponent > Decimal::ZERO, "{fees:?}");
    println!("tick {tick}, neg_risk {neg_risk}, fees {fees:?}");
}

#[tokio::test]
#[ignore = "reaches the real Polymarket API"]
async fn settled_market_names_its_winner() {
    let market = clob()
        .get_market_by_id(SETTLED_MARKET)
        .await
        .expect("market");
    assert!(market.resolved);
    let winner = |name: &str| {
        market
            .outcomes
            .iter()
            .find(|o| o.name.eq_ignore_ascii_case(name))
            .unwrap_or_else(|| panic!("no {name} outcome: {:?}", market.outcomes))
            .winner
    };
    assert_eq!(winner("yes"), Some(false));
    assert_eq!(winner("no"), Some(true));
}

#[tokio::test]
#[ignore = "reaches the real Polymarket Data API"]
async fn recent_trades_parse() {
    let trades = clob().get_recent_trades(50).await.expect("recent trades");
    assert!(!trades.is_empty());
    let now = chrono::Utc::now().timestamp();
    for trade in &trades {
        assert!(trade.side == "BUY" || trade.side == "SELL", "{trade:?}");
        assert!(trade.price > 0.0 && trade.price < 1.0, "{trade:?}");
        assert!(trade.size > 0.0, "{trade:?}");
        assert!(trade.wallet_address.starts_with("0x"), "{trade:?}");
        assert!(
            trade.timestamp > now - 86_400 && trade.timestamp <= now + 60,
            "{trade:?}"
        );
    }
}

#[tokio::test]
#[ignore = "reaches a real Polygon node"]
async fn chain_reads_parse() {
    let polygon = polygon();
    let head = polygon.get_block_number().await.expect("head");
    let safe = polygon.safe_block_number().await.expect("safe block");
    assert!(
        safe <= head && head - safe < 10_000,
        "safe {safe}, head {head}"
    );

    let balances = polygon
        .get_cash_balances(CONDITIONAL_TOKENS)
        .await
        .expect("cash balances");
    assert!(balances.usdc_units > 0, "{balances:?}");
    assert!(balances.block <= head);
    let now_units = polygon
        .get_usdc_balance_units(CONDITIONAL_TOKENS)
        .await
        .expect("usdc balance");
    assert!(now_units > 0);

    // A contract has sent no transactions of its own.
    assert_eq!(
        polygon
            .transaction_count_at(CONDITIONAL_TOKENS, safe)
            .await
            .expect("nonce"),
        polygon
            .transaction_count(CONDITIONAL_TOKENS, "latest")
            .await
            .expect("nonce"),
    );

    let gas = polygon.gas_price_wei().await.expect("gas price");
    assert!(gas > 0);

    let missing = format!("0x{}", "ab".repeat(32));
    assert!(polygon
        .transaction_receipt(&missing)
        .await
        .expect("receipt")
        .is_none());
    println!("head {head}, safe {safe}, gas {gas} wei, balances {balances:?}");
}

/// The exchange's answer to a key-derivation request for a fresh wallet,
/// signed by the bot (`valid`) or with its signature corrupted. GET only:
/// derivation finds an existing key and never creates one.
async fn derive_answer(valid: bool) -> (u16, String) {
    use alloy_signer_local::PrivateKeySigner;
    use polymarket_core::signing::OrderSigner;
    let key = PrivateKeySigner::random();
    let address = format!("{:#x}", key.address());
    let signer = OrderSigner::new(key);
    let timestamp = chrono::Utc::now().timestamp() as u64;
    let mut signature = signer
        .sign_clob_auth_message(timestamp, 0)
        .await
        .expect("signature");
    if !valid {
        // Flip a byte of r.
        let flipped = if &signature[10..12] == "00" {
            "11"
        } else {
            "00"
        };
        signature.replace_range(10..12, flipped);
    }
    let response = reqwest::Client::new()
        .get("https://clob.polymarket.com/auth/derive-api-key")
        .header("POLY_ADDRESS", address)
        .header("POLY_SIGNATURE", signature)
        .header("POLY_TIMESTAMP", timestamp.to_string())
        .header("POLY_NONCE", "0")
        .send()
        .await
        .expect("exchange reachable");
    let status = response.status().as_u16();
    (status, response.text().await.unwrap_or_default())
}

#[tokio::test]
#[ignore = "reaches the real Polymarket API"]
async fn exchange_accepts_the_bots_l1_auth_signature() {
    // Signed by the bot: authenticated, and refused only because the new
    // wallet has no key to derive.
    let (status, body) = derive_answer(true).await;
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("Could not derive api key"), "{body}");
    // The same request with a corrupted signature fails authentication.
    let (status, body) = derive_answer(false).await;
    assert_eq!(status, 401, "{body}");
    assert!(body.contains("Invalid L1 Request headers"), "{body}");
}
