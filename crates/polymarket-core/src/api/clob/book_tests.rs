use super::*;

/// Shape of a live `GET /book` response: bids ascending and asks descending,
/// i.e. worst level first.
const WORST_FIRST_BOOK: &str = r#"{
    "market": "0xmarket",
    "asset_id": "123",
    "timestamp": "1792618777423",
    "hash": "abc",
    "bids": [
        {"price": "0.001", "size": "2312562.35"},
        {"price": "0.45", "size": "100"},
        {"price": "0.47", "size": "50"}
    ],
    "asks": [
        {"price": "0.999", "size": "11018551.59"},
        {"price": "0.52", "size": "75"},
        {"price": "0.49", "size": "20"}
    ],
    "min_order_size": "5",
    "tick_size": "0.001",
    "neg_risk": true
}"#;

fn prices(levels: &[PriceLevel]) -> Vec<Decimal> {
    levels.iter().map(|l| l.price).collect()
}

fn dec(s: &str) -> Decimal {
    s.parse().unwrap()
}

#[test]
fn rest_book_is_sorted_best_first() {
    let raw: ClobOrderBook = serde_json::from_str(WORST_FIRST_BOOK).unwrap();
    let book: OrderBook = raw.into();

    assert_eq!(
        prices(&book.bids),
        vec![dec("0.47"), dec("0.45"), dec("0.001")]
    );
    assert_eq!(
        prices(&book.asks),
        vec![dec("0.49"), dec("0.52"), dec("0.999")]
    );
    assert_eq!(book.best_bid(), Some(dec("0.47")));
    assert_eq!(book.best_ask(), Some(dec("0.49")));
    assert_eq!(book.bids[0].size, dec("50"));
    assert_eq!(book.asks[0].size, dec("20"));
}

#[test]
fn malformed_levels_are_dropped_instead_of_defaulting_to_zero() {
    let raw: ClobOrderBook = serde_json::from_str(
        r#"{
            "market": "m", "asset_id": "a", "timestamp": "0",
            "bids": [
                {"price": "not-a-number", "size": "10"},
                {"price": "0.40", "size": "0"},
                {"price": "0.30", "size": "5"}
            ],
            "asks": [
                {"price": "0", "size": "10"},
                {"price": "1", "size": "10"},
                {"price": "0.60", "size": "bad"},
                {"price": "0.70", "size": "5"}
            ]
        }"#,
    )
    .unwrap();
    let book: OrderBook = raw.into();

    assert_eq!(prices(&book.bids), vec![dec("0.30")]);
    assert_eq!(prices(&book.asks), vec![dec("0.70")]);
}

#[test]
fn websocket_snapshot_is_sorted_best_first() {
    let mut state = HashMap::new();
    let parsed = parse_ws_message(
        r#"[{"event_type":"book","market":"m","asset_id":"a","timestamp":"1",
            "bids":[{"price":"0.10","size":"1"},{"price":"0.40","size":"2"}],
            "asks":[{"price":"0.90","size":"3"},{"price":"0.55","size":"4"}]}]"#,
        &mut state,
    );

    assert_eq!(parsed.updates.len(), 1);
    let update = &parsed.updates[0];
    assert_eq!(prices(&update.bids), vec![dec("0.40"), dec("0.10")]);
    assert_eq!(prices(&update.asks), vec![dec("0.55"), dec("0.90")]);
    assert_eq!(prices(&state["a"].asks), vec![dec("0.55"), dec("0.90")]);
}

#[test]
fn trades_and_tick_size_changes_leave_the_book_alone() {
    let mut state = HashMap::new();
    parse_ws_message(
        r#"{"event_type":"book","market":"m","asset_id":"a","timestamp":"1",
            "bids":[{"price":"0.40","size":"2"}],"asks":[{"price":"0.60","size":"4"}]}"#,
        &mut state,
    );
    for event in [
        r#"{"event_type":"last_trade_price","market":"m","asset_id":"a","price":"0.60",
            "side":"BUY","size":"1","fee_rate_bps":"0","timestamp":"2"}"#,
        r#"{"event_type":"tick_size_change","market":"m","asset_id":"a",
            "old_tick_size":"0.01","new_tick_size":"0.001","timestamp":"3"}"#,
        r#"[{"event_type":"last_trade_price","market":"m","asset_id":"a","price":"0.60",
            "side":"BUY","size":"1","timestamp":"4"}]"#,
        r#"{"market":"m","asset_id":"a","timestamp":"5"}"#,
    ] {
        let parsed = parse_ws_message(event, &mut state);
        assert!(parsed.updates.is_empty(), "{event}");
        // Known events are not parse misses; a bare object still is.
        assert_eq!(
            parsed.kind.is_parse_miss(),
            !event.contains("event_type"),
            "{event}"
        );
    }
    assert_eq!(prices(&state["a"].bids), vec![dec("0.40")]);
    assert_eq!(prices(&state["a"].asks), vec![dec("0.60")]);
}

#[test]
fn price_change_before_snapshot_is_ignored() {
    let mut state = HashMap::new();
    let parsed = parse_ws_message(
        r#"{"event_type":"price_change","market":"m","timestamp":"1",
            "price_changes":[{"asset_id":"a","price":"0.01","size":"5","side":"SELL"}]}"#,
        &mut state,
    );

    assert!(parsed.updates.is_empty());
    assert!(state.is_empty());
}

#[test]
fn price_changes_keep_the_snapshot_best_first() {
    let mut state = HashMap::new();
    parse_ws_message(
        r#"{"event_type":"book","market":"m","asset_id":"a","timestamp":"1",
            "bids":[{"price":"0.30","size":"1"},{"price":"0.40","size":"2"}],
            "asks":[{"price":"0.70","size":"3"},{"price":"0.60","size":"4"}]}"#,
        &mut state,
    );

    let parsed = parse_ws_message(
        r#"{"event_type":"price_change","market":"m","timestamp":"2",
            "price_changes":[
                {"asset_id":"a","price":"0.55","size":"9","side":"SELL"},
                {"asset_id":"a","price":"0.40","size":"0","side":"BUY"},
                {"asset_id":"a","price":"0.35","size":"6","side":"BUY"}
            ]}"#,
        &mut state,
    );

    let last = parsed.updates.last().unwrap();
    assert_eq!(
        prices(&last.asks),
        vec![dec("0.55"), dec("0.60"), dec("0.70")]
    );
    assert_eq!(prices(&last.bids), vec![dec("0.35"), dec("0.30")]);
}
