//! Health check handlers.

use axum::extract::State;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use utoipa::ToSchema;

use crate::error::ApiResult;
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
pub async fn readiness(State(state): State<Arc<AppState>>) -> ApiResult<Json<HealthResponse>> {
    // Check database connection
    let db_status = match sqlx::query("SELECT 1").fetch_one(&state.pool).await {
        Ok(_) => "connected".to_string(),
        Err(e) => format!("error: {}", e),
    };

    let status = if db_status == "connected" {
        "ready"
    } else {
        "degraded"
    };

    Ok(Json(HealthResponse {
        status: status.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        timestamp: Utc::now(),
        database: Some(db_status),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_health_check() {
        let response = health_check().await;
        assert_eq!(response.status, "healthy");
    }
}
