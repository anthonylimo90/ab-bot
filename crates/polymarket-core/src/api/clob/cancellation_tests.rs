use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

async fn capture_request(body: &'static str) -> (ClobClient, tokio::task::JoinHandle<String>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        loop {
            let mut buffer = [0; 4096];
            let count = socket.read(&mut buffer).await.unwrap();
            assert!(count > 0);
            bytes.extend_from_slice(&buffer[..count]);
            if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..end]);
                let length: usize = headers
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .map(str::to_owned)
                    })
                    .unwrap()
                    .parse()
                    .unwrap();
                if bytes.len() >= end + 4 + length {
                    break;
                }
            }
        }
        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).as_bytes()).await.unwrap();
        String::from_utf8(bytes).unwrap()
    });
    let mut base = ClobClient::new(Some(format!("http://{address}")), None);
    base.http_client = reqwest::Client::builder().no_proxy().build().unwrap();
    (base, server)
}

#[tokio::test]
async fn cancellation_signs_the_exact_delete_body_and_requires_acknowledgement() {
    for (body, expected) in [
        (r#"{"canceled":["exchange-id"],"not_canceled":{}}"#, true),
        (
            r#"{"canceled":[],"not_canceled":{"exchange-id":"already matched"}}"#,
            false,
        ),
        (r#"{}"#, false),
    ] {
        let (base, server) = capture_request(body).await;
        let credentials = ApiCredentials::new(
            "test-key".into(),
            "c2VjcmV0".into(),
            "test-passphrase".into(),
        );
        let signer = OrderSigner::new(format!("{:064x}", 1).parse().unwrap());
        let client = AuthenticatedClobClient::with_credentials(base, signer, credentials.clone());
        assert_eq!(client.cancel_order("exchange-id").await.is_ok(), expected);
        let request = server.await.unwrap();
        let (headers, sent_body) = request.split_once("\r\n\r\n").unwrap();
        assert!(headers.starts_with("DELETE /order HTTP/1.1"));
        assert_eq!(sent_body, r#"{"orderID":"exchange-id"}"#);
        let header = |name: &str| {
            headers
                .lines()
                .find_map(|line| {
                    let (key, value) = line.split_once(": ")?;
                    key.eq_ignore_ascii_case(name).then_some(value)
                })
                .unwrap()
        };
        assert_eq!(header("poly_api_key"), "test-key");
        assert_eq!(
            header("poly_signature"),
            sign_l2_request(
                &credentials,
                "DELETE",
                "/order",
                header("poly_timestamp"),
                Some(sent_body)
            )
            .unwrap()
        );
    }
}

#[tokio::test]
async fn signed_wallet_submission_uses_wallet_credentials_and_exact_l2_body() {
    let (base, server) = capture_request(r#"{"orderID":"exchange-id","status":"live"}"#).await;
    let credentials = ApiCredentials::new(
        "wallet-api-key".into(),
        "c2VjcmV0".into(),
        "wallet-passphrase".into(),
    );
    let signer = OrderSigner::new(format!("{:064x}", 2).parse().unwrap());
    let data = signer
        .order_builder()
        .token_id_str("1")
        .side(crate::signing::OrderSide::Buy)
        .price(Decimal::new(4, 1))
        .size(Decimal::TEN)
        .expires_in(3600)
        .build()
        .unwrap();
    let signed = signer.sign_order(&data).await.unwrap();
    assert!(matches!(
        base.post_signed_order(
            "0x0000000000000000000000000000000000000001",
            &credentials,
            signed.clone(),
            OrderType::Gtd
        )
        .await,
        Err(Error::Auth { .. })
    ));
    let response = base
        .post_signed_order(&signed.maker, &credentials, signed.clone(), OrderType::Gtd)
        .await
        .unwrap();
    assert_eq!(response.order_id, "exchange-id");
    let request = server.await.unwrap();
    let (headers, body) = request.split_once("\r\n\r\n").unwrap();
    assert!(headers.starts_with("POST /order HTTP/1.1"));
    let header = |name: &str| {
        headers
            .lines()
            .find_map(|line| {
                let (key, value) = line.split_once(": ")?;
                key.eq_ignore_ascii_case(name).then_some(value)
            })
            .unwrap()
    };
    assert_eq!(header("poly_address"), signed.maker);
    assert_eq!(header("poly_api_key"), "wallet-api-key");
    assert_eq!(
        header("poly_signature"),
        sign_l2_request(
            &credentials,
            "POST",
            "/order",
            header("poly_timestamp"),
            Some(body)
        )
        .unwrap()
    );
    let body: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(body["owner"], "wallet-api-key");
    assert_eq!(body["orderType"], "GTD");
    assert!(body["order"].get("order_hash").is_none());
}

/// A node answering every request with `status` and `body`.
async fn answer_all(status: &'static str, body: &'static str) -> ClobClient {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0; 4096];
            while !bytes.windows(4).any(|w| w == b"\r\n\r\n") {
                let count = socket.read(&mut buffer).await.unwrap();
                if count == 0 {
                    break;
                }
                bytes.extend_from_slice(&buffer[..count]);
            }
            let _ = socket.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await;
        }
    });
    let mut base = ClobClient::new(Some(format!("http://{address}")), None);
    base.http_client = reqwest::Client::builder().no_proxy().build().unwrap();
    base
}

#[tokio::test]
async fn an_order_is_never_signed_for_a_guessed_exchange() {
    let signer = || OrderSigner::new(format!("{:064x}", 1).parse().unwrap());
    // The lookup fails: no order, rather than one for the wrong exchange.
    let client = AuthenticatedClobClient::new(answer_all("404 Not Found", "{}").await, signer());
    assert!(client
        .create_order(
            "77",
            crate::signing::OrderSide::Buy,
            Decimal::new(5, 1),
            Decimal::TEN,
            OrderType::Gtc,
        )
        .await
        .is_err());
    assert!(client.neg_risk_cache.lock().unwrap().is_empty());

    // A standard market: signed for the CTF Exchange, and remembered.
    let client = AuthenticatedClobClient::new(
        answer_all("200 OK", r#"{"neg_risk": false}"#).await,
        signer(),
    );
    assert!(!client.is_neg_risk("77").await.unwrap());
    assert_eq!(
        client.neg_risk_cache.lock().unwrap().get("77"),
        Some(&false)
    );
}

/// Serves `/data/trades?id=...` from `trades` (by id) until dropped.
async fn trade_server(trades: Vec<serde_json::Value>) -> ClobClient {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut bytes = Vec::new();
            while !bytes.windows(4).any(|w| w == b"\r\n\r\n") {
                let mut buffer = [0; 4096];
                let count = socket.read(&mut buffer).await.unwrap();
                if count == 0 {
                    break;
                }
                bytes.extend_from_slice(&buffer[..count]);
            }
            let request = String::from_utf8_lossy(&bytes);
            let id = request
                .split_once("id=")
                .and_then(|(_, rest)| rest.split([' ', '&']).next())
                .unwrap_or_default()
                .to_string();
            let rows: Vec<_> = trades
                .iter()
                .filter(|trade| trade["id"].as_str() == Some(id.as_str()))
                .cloned()
                .collect();
            let body = serde_json::json!({ "data": rows }).to_string();
            let _ = socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    )
                    .as_bytes(),
                )
                .await;
        }
    });
    let mut base = ClobClient::new(Some(format!("http://{address}")), None);
    base.http_client = reqwest::Client::builder().no_proxy().build().unwrap();
    base
}

#[tokio::test]
async fn a_failed_trade_fills_nothing_and_an_unsettled_one_is_not_yet_evidence() {
    let trade = |id: &str, status: &str, size: &str, price: &str| {
        serde_json::json!({
            "id": id, "status": status, "taker_order_id": "order",
            "size": size, "price": price, "maker_orders": [],
        })
    };
    let order = |matched: &str, trades: &[&str]| ExchangeOrder {
        associate_trades: trades.iter().map(|id| id.to_string()).collect(),
        id: "order".into(),
        status: "MATCHED".into(),
        original_size: "10".into(),
        size_matched: matched.into(),
    };
    let client = |base| {
        AuthenticatedClobClient::with_credentials(
            base,
            OrderSigner::new(format!("{:064x}", 3).parse().unwrap()),
            ApiCredentials::new("key".into(), "c2VjcmV0".into(), "pass".into()),
        )
    };
    let base = trade_server(vec![
        trade("ok", "CONFIRMED", "6", "0.4"),
        trade("failed", "FAILED", "4", "0.5"),
        trade("retrying", "RETRYING", "4", "0.5"),
    ])
    .await;
    let client = client(base);
    // Six confirmed, four failed: six filled at the confirmed price.
    assert_eq!(
        client
            .confirmed_order_fill(&order("10", &["ok", "failed"]))
            .await
            .unwrap(),
        (Decimal::from(6), Decimal::new(4, 1))
    );
    // Every trade failed: nothing filled.
    assert_eq!(
        client
            .confirmed_order_fill(&order("4", &["failed"]))
            .await
            .unwrap(),
        (Decimal::ZERO, Decimal::ZERO)
    );
    // A trade still settling is no evidence yet, nor are totals that differ.
    assert!(client
        .confirmed_order_fill(&order("10", &["ok", "retrying"]))
        .await
        .is_err());
    assert!(client
        .confirmed_order_fill(&order("10", &["ok"]))
        .await
        .is_err());
}
