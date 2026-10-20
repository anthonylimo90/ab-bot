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
