//! Health check handlers.

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use crate::state::AppState;

/// Health check response.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct HealthResponse {
    /// Service status.
    pub status: String,
    /// Service version.
    pub version: String,
    /// Current timestamp.
    pub timestamp: DateTime<Utc>,
    /// Database connection status.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
}

/// Health check endpoint.
#[utoipa::path(
    get,
    path = "/health",
    tag = "health",
    responses(
        (status = 200, description = "Service is healthy", body = HealthResponse)
    )
)]
pub async fn health_check() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "healthy".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        timestamp: Utc::now(),
        database: None,
    })
}

/// Readiness check endpoint (includes database check).
#[utoipa::path(
    get,
    path = "/ready",
    tag = "health",
    responses(
        (status = 200, description = "Service is ready", body = HealthResponse),
        (status = 503, description = "Service is not ready")
    )
)]
pub async fn readiness(State(state): State<Arc<AppState>>) -> (StatusCode, Json<HealthResponse>) {
    // Check database connection. This endpoint is unauthenticated, so a failure
    // is logged but never described to the caller (driver errors can name
    // hosts, users and databases); orchestrators only need the status code.
    let (code, status, database) = match sqlx::query("SELECT 1").fetch_one(&state.pool).await {
        Ok(_) => (StatusCode::OK, "ready", "connected"),
        Err(e) => {
            tracing::warn!(error = %e, "Readiness check failed: database unavailable");
            (StatusCode::SERVICE_UNAVAILABLE, "not_ready", "unavailable")
        }
    };

    (
        code,
        Json(HealthResponse {
            status: status.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            timestamp: Utc::now(),
            database: Some(database.to_string()),
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_health_check() {
        let response = health_check().await;
        assert_eq!(response.status, "healthy");
    }

    fn app(pool: sqlx::PgPool) -> axum::Router {
        axum::Router::new()
            .route("/ready", axum::routing::get(readiness))
            .with_state(AppState::for_tests(pool))
    }

    #[sqlx::test(migrations = false)]
    async fn ready_reports_200_when_the_database_answers(pool: sqlx::PgPool) {
        let (status, body) =
            crate::workspace_scope::test_fixture::call(&app(pool), "GET", "/ready", None, None)
                .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["status"], "ready");
        assert_eq!(body["database"], "connected");
    }

    #[sqlx::test(migrations = false)]
    async fn ready_returns_503_without_error_detail_when_the_database_is_down(pool: sqlx::PgPool) {
        let app = app(pool.clone());
        pool.close().await;
        let (status, body) =
            crate::workspace_scope::test_fixture::call(&app, "GET", "/ready", None, None).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["status"], "not_ready");
        assert_eq!(body["database"], "unavailable");
        // Nothing from the driver error (pool state, hosts, ...) reaches the caller.
        let text = body.to_string().to_lowercase();
        for leaked in ["error", "pool", "closed", "postgres", "connection"] {
            assert!(!text.contains(leaked), "{leaked} leaked in {text}");
        }
    }
}
