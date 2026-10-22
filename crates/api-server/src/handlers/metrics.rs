//! Prometheus metrics for operating the trading system: the circuit breaker,
//! worker liveness, positions, and orders and withdrawals awaiting
//! resolution.
//!
//! Disabled (404) unless `METRICS_TOKEN` is set to at least 16 characters;
//! requests must then carry it as a bearer token, since the figures include
//! equity and P&L.

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use rust_decimal::Decimal;
use std::fmt::Write;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::state::AppState;

/// The shortest token accepted, so a guessable one never enables the endpoint.
const MIN_TOKEN_LEN: usize = 16;

/// `GET /metrics`, in the Prometheus text format.
pub async fn metrics(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let token = std::env::var("METRICS_TOKEN").ok();
    metrics_for(&state, &headers, token.as_deref()).await
}

async fn metrics_for(state: &AppState, headers: &HeaderMap, token: Option<&str>) -> Response {
    let Some(token) = token.filter(|token| token.len() >= MIN_TOKEN_LEN) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let presented = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    if !presented.is_some_and(|presented| constant_time_eq(presented, token)) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    (
        [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
        render(state).await,
    )
        .into_response()
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// Positions not yet closed, by state and execution mode.
const POSITION_STATES: &str = "SELECT CASE state WHEN 0 THEN 'pending' WHEN 1 THEN 'open' \
     WHEN 2 THEN 'exit_ready' WHEN 3 THEN 'closing' WHEN 6 THEN 'exit_failed' \
     WHEN 7 THEN 'stalled' END, execution_mode, COUNT(*) \
     FROM positions WHERE state IN (0, 1, 2, 3, 6, 7) GROUP BY 1, 2 ORDER BY 1, 2";

/// Orders whose outcome is unknown, and withdrawals not yet settled.
const UNRESOLVED: &str = "SELECT \
     (SELECT COUNT(*) FROM execution_intents WHERE status IN ('submitting', 'unknown')), \
     (SELECT COUNT(*) FROM wallet_withdrawals \
        WHERE status IN ('pending', 'submitting', 'submitted')), \
     (SELECT COALESCE(SUM(amount), 0) FROM wallet_withdrawals \
        WHERE status IN ('pending', 'submitting', 'submitted'))";

async fn render(state: &AppState) -> String {
    let mut out = String::new();
    let mut gauge = |name: &str, help: &str, samples: &[(String, String)]| {
        let _ = writeln!(out, "# HELP {name} {help}\n# TYPE {name} gauge");
        for (labels, value) in samples {
            let _ = writeln!(out, "{name}{labels} {value}");
        }
    };
    let one = |value: String| vec![(String::new(), value)];

    let breaker = state.circuit_breaker.state().await;
    gauge(
        "abbot_circuit_breaker_tripped",
        "1 while the circuit breaker halts trading.",
        &one(u8::from(breaker.tripped).to_string()),
    );
    gauge(
        "abbot_circuit_breaker_consecutive_losses",
        "Losing trades in a row.",
        &one(breaker.consecutive_losses.to_string()),
    );
    gauge(
        "abbot_circuit_breaker_trips_today",
        "Circuit breaker trips today.",
        &one(breaker.trips_today.to_string()),
    );
    gauge(
        "abbot_daily_pnl_usd",
        "Realized P&L today, as the circuit breaker counts it.",
        &one(decimal(breaker.daily_pnl)),
    );
    gauge(
        "abbot_equity_usd",
        "Trading equity last fed to the circuit breaker.",
        &one(decimal(breaker.current_value)),
    );
    gauge(
        "abbot_live_trading",
        "1 when the executor places live orders.",
        &one(u8::from(state.order_executor.is_live()).to_string()),
    );

    let now = chrono::Utc::now().timestamp();
    let heartbeats: Vec<(String, String)> = [
        ("arb_executor", &state.arb_executor_heartbeat),
        ("exit_handler", &state.exit_handler_heartbeat),
        ("quant_executor", &state.quant_executor_heartbeat),
    ]
    .into_iter()
    .filter_map(|(worker, beat)| {
        let at = beat.load(Ordering::Relaxed);
        (at > 0).then(|| {
            (
                format!("{{worker=\"{worker}\"}}"),
                (now - at).max(0).to_string(),
            )
        })
    })
    .collect();
    gauge(
        "abbot_worker_heartbeat_age_seconds",
        "Seconds since a trading worker last reported progress.",
        &heartbeats,
    );

    let positions: Result<Vec<(Option<String>, String, i64)>, _> =
        sqlx::query_as(POSITION_STATES).fetch_all(&state.pool).await;
    let unresolved: Result<(i64, i64, Decimal), _> =
        sqlx::query_as(UNRESOLVED).fetch_one(&state.pool).await;
    let database_up = positions.is_ok() && unresolved.is_ok();
    gauge(
        "abbot_database_up",
        "1 when the metrics' database reads succeeded.",
        &one(u8::from(database_up).to_string()),
    );
    if let Ok(positions) = positions {
        let samples: Vec<(String, String)> = positions
            .into_iter()
            .filter_map(|(state, mode, count)| {
                state.map(|state| {
                    (
                        format!("{{state=\"{state}\",mode=\"{}\"}}", label(&mode)),
                        count.to_string(),
                    )
                })
            })
            .collect();
        gauge(
            "abbot_positions",
            "Positions not yet closed, by state and execution mode.",
            &samples,
        );
    }
    if let Ok((intents, withdrawals, withdrawal_usd)) = unresolved {
        gauge(
            "abbot_unresolved_order_intents",
            "Live orders whose outcome is unknown (they block new orders).",
            &one(intents.to_string()),
        );
        gauge(
            "abbot_unresolved_withdrawals",
            "Withdrawals not yet confirmed or failed.",
            &one(withdrawals.to_string()),
        );
        gauge(
            "abbot_unresolved_withdrawals_usd",
            "USDC in withdrawals not yet confirmed or failed.",
            &one(decimal(withdrawal_usd)),
        );
    }
    out
}

fn decimal(value: Decimal) -> String {
    value.normalize().to_string()
}

/// A label value safe in the text format.
fn label(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use sqlx::Executor;

    const TOKEN: &str = "0123456789abcdef-test";

    fn bearer(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        headers
    }

    async fn body(response: Response) -> String {
        String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap()
    }

    #[sqlx::test(migrations = false)]
    async fn metrics_need_a_configured_token_and_report_the_trading_state(pool: sqlx::PgPool) {
        pool.execute(
            "CREATE TABLE positions (state SMALLINT, execution_mode TEXT); \
             INSERT INTO positions VALUES (1, 'live'), (1, 'live'), (6, 'paper'), (4, 'live'); \
             CREATE TABLE execution_intents (status TEXT); \
             INSERT INTO execution_intents VALUES ('unknown'), ('acknowledged'); \
             CREATE TABLE wallet_withdrawals (status TEXT, amount NUMERIC); \
             INSERT INTO wallet_withdrawals VALUES ('submitted', 25.5), ('confirmed', 99)",
        )
        .await
        .unwrap();
        let state = AppState::for_tests(pool);

        // Off without a token, or with one too short to be safe.
        let none = metrics_for(&state, &bearer(TOKEN), None).await;
        assert_eq!(none.status(), StatusCode::NOT_FOUND);
        let short = metrics_for(&state, &bearer("short"), Some("short")).await;
        assert_eq!(short.status(), StatusCode::NOT_FOUND);
        // The token is required.
        let wrong = metrics_for(&state, &bearer("0123456789abcdef-nope"), Some(TOKEN)).await;
        assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
        let missing = metrics_for(&state, &HeaderMap::new(), Some(TOKEN)).await;
        assert_eq!(missing.status(), StatusCode::UNAUTHORIZED);

        let text = body(metrics_for(&state, &bearer(TOKEN), Some(TOKEN)).await).await;
        for line in [
            "abbot_circuit_breaker_tripped 0",
            "abbot_database_up 1",
            "abbot_positions{state=\"open\",mode=\"live\"} 2",
            "abbot_positions{state=\"exit_failed\",mode=\"paper\"} 1",
            "abbot_unresolved_order_intents 1",
            "abbot_unresolved_withdrawals 1",
            "abbot_unresolved_withdrawals_usd 25.5",
        ] {
            assert!(text.lines().any(|l| l == line), "{line} missing in\n{text}");
        }
        assert!(!text.contains("state=\"closed\""));
    }
}
