//! PostgreSQL storage backend for audit logs.

use anyhow::Result;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use std::net::IpAddr;

use crate::audit::{AuditAction, AuditEvent, AuditFilter, AuditStorage};

/// PostgreSQL-backed audit storage.
pub struct PostgresAuditStorage {
    pool: PgPool,
}

impl PostgresAuditStorage {
    /// Create a new PostgreSQL audit storage.
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// Database row for audit events.
#[derive(Debug, sqlx::FromRow)]
struct AuditRow {
    id: i64,
    timestamp: DateTime<Utc>,
    user_id: Option<String>,
    action: String,
    resource: String,
    details: Option<serde_json::Value>,
    ip_address: Option<String>,
    user_agent: Option<String>,
    success: bool,
    error: Option<String>,
}

impl AuditRow {
    fn into_event(self) -> AuditEvent {
        let action = parse_action(&self.action);
        let ip_address = self.ip_address.and_then(|s| s.parse::<IpAddr>().ok());

        AuditEvent {
            id: self.id,
            timestamp: self.timestamp,
            user_id: self.user_id,
            action,
            resource: self.resource,
            details: self.details.unwrap_or(serde_json::Value::Null),
            ip_address,
            user_agent: self.user_agent,
            success: self.success,
            error: self.error,
        }
    }
}

/// Parse action string to AuditAction enum.
fn parse_action(action: &str) -> AuditAction {
    match action {
        "login" => AuditAction::Login,
        "logout" => AuditAction::Logout,
        "login_failed" => AuditAction::LoginFailed,
        "token_refresh" => AuditAction::TokenRefresh,
        "api_key_created" => AuditAction::ApiKeyCreated,
        "api_key_revoked" => AuditAction::ApiKeyRevoked,
        "user_created" => AuditAction::UserCreated,
        "user_updated" => AuditAction::UserUpdated,
        "user_deleted" => AuditAction::UserDeleted,
        "user_viewed" => AuditAction::UserViewed,
        "create_position" => AuditAction::CreatePosition,
        "close_position" => AuditAction::ClosePosition,
        "manual_exit" => AuditAction::ManualExit,
        "emergency_exit_all" => AuditAction::EmergencyExitAll,
        "add_tracked_wallet" => AuditAction::AddTrackedWallet,
        "remove_tracked_wallet" => AuditAction::RemoveTrackedWallet,
        "update_tracked_wallet" => AuditAction::UpdateTrackedWallet,
        "copy_trade_executed" => AuditAction::CopyTradeExecuted,
        "create_stop_loss" => AuditAction::CreateStopLoss,
        "remove_stop_loss" => AuditAction::RemoveStopLoss,
        "stop_loss_triggered" => AuditAction::StopLossTriggered,
        "circuit_breaker_tripped" => AuditAction::CircuitBreakerTripped,
        "circuit_breaker_reset" => AuditAction::CircuitBreakerReset,
        "config_change" => AuditAction::ConfigChange,
        "position_limits_change" => AuditAction::PositionLimitsChange,
        "export_data" => AuditAction::ExportData,
        "view_sensitive_data" => AuditAction::ViewSensitiveData,
        other => AuditAction::Custom(other.to_string()),
    }
}

/// Serialize action enum to database string.
fn action_to_string(action: &AuditAction) -> String {
    match action {
        AuditAction::Login => "login".to_string(),
        AuditAction::Logout => "logout".to_string(),
        AuditAction::LoginFailed => "login_failed".to_string(),
        AuditAction::TokenRefresh => "token_refresh".to_string(),
        AuditAction::ApiKeyCreated => "api_key_created".to_string(),
        AuditAction::ApiKeyRevoked => "api_key_revoked".to_string(),
        AuditAction::UserCreated => "user_created".to_string(),
        AuditAction::UserUpdated => "user_updated".to_string(),
        AuditAction::UserDeleted => "user_deleted".to_string(),
        AuditAction::UserViewed => "user_viewed".to_string(),
        AuditAction::CreatePosition => "create_position".to_string(),
        AuditAction::ClosePosition => "close_position".to_string(),
        AuditAction::ManualExit => "manual_exit".to_string(),
        AuditAction::EmergencyExitAll => "emergency_exit_all".to_string(),
        AuditAction::AddTrackedWallet => "add_tracked_wallet".to_string(),
        AuditAction::RemoveTrackedWallet => "remove_tracked_wallet".to_string(),
        AuditAction::UpdateTrackedWallet => "update_tracked_wallet".to_string(),
        AuditAction::CopyTradeExecuted => "copy_trade_executed".to_string(),
        AuditAction::CreateStopLoss => "create_stop_loss".to_string(),
        AuditAction::RemoveStopLoss => "remove_stop_loss".to_string(),
        AuditAction::StopLossTriggered => "stop_loss_triggered".to_string(),
        AuditAction::CircuitBreakerTripped => "circuit_breaker_tripped".to_string(),
        AuditAction::CircuitBreakerReset => "circuit_breaker_reset".to_string(),
        AuditAction::ConfigChange => "config_change".to_string(),
        AuditAction::PositionLimitsChange => "position_limits_change".to_string(),
        AuditAction::ExportData => "export_data".to_string(),
        AuditAction::ViewSensitiveData => "view_sensitive_data".to_string(),
        AuditAction::Custom(s) => s.clone(),
    }
}

#[async_trait::async_trait]
impl AuditStorage for PostgresAuditStorage {
    async fn store(&self, event: &AuditEvent) -> Result<i64> {
        let action = action_to_string(&event.action);
        let ip_str = event.ip_address.map(|ip| ip.to_string());

        let row: (i64,) = sqlx::query_as(
            r#"
            INSERT INTO audit_log (timestamp, user_id, action, resource, details, ip_address, user_agent, success, error)
            VALUES ($1, $2, $3, $4, $5, $6::inet, $7, $8, $9)
            RETURNING id
            "#,
        )
        .bind(event.timestamp)
        .bind(&event.user_id)
        .bind(&action)
        .bind(&event.resource)
        .bind(&event.details)
        .bind(&ip_str)
        .bind(&event.user_agent)
        .bind(event.success)
        .bind(&event.error)
        .fetch_one(&self.pool)
        .await?;

        Ok(row.0)
    }

    async fn query(&self, filter: &AuditFilter) -> Result<Vec<AuditEvent>> {
        // Build dynamic query with filters
        let mut query = String::from(
            r#"
            SELECT id, timestamp, user_id, action, resource, details,
                   ip_address::text, user_agent, success, error
            FROM audit_log
            WHERE 1=1
            "#,
        );

        let mut param_count = 0;

        if filter.user_id.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND user_id = ${}", param_count));
        }

        if filter.action.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND action = ${}", param_count));
        }

        if filter.resource_prefix.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND resource LIKE ${}", param_count));
        }

        if filter.from.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND timestamp >= ${}", param_count));
        }

        if filter.to.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND timestamp <= ${}", param_count));
        }

        if filter.success_only.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND success = ${}", param_count));
        }

        query.push_str(" ORDER BY timestamp DESC");

        if filter.limit.is_some() {
            param_count += 1;
            query.push_str(&format!(" LIMIT ${}", param_count));
        }

        if filter.offset.is_some() {
            param_count += 1;
            query.push_str(&format!(" OFFSET ${}", param_count));
        }

        // Build query with dynamic bindings
        let mut query_builder = sqlx::query_as::<_, AuditRow>(&query);

        if let Some(ref user_id) = filter.user_id {
            query_builder = query_builder.bind(user_id);
        }

        if let Some(ref action) = filter.action {
            query_builder = query_builder.bind(action_to_string(action));
        }

        if let Some(ref prefix) = filter.resource_prefix {
            query_builder = query_builder.bind(format!("{}%", prefix));
        }

        if let Some(from) = filter.from {
            query_builder = query_builder.bind(from);
        }

        if let Some(to) = filter.to {
            query_builder = query_builder.bind(to);
        }

        if let Some(success_only) = filter.success_only {
            query_builder = query_builder.bind(success_only);
        }

        if let Some(limit) = filter.limit {
            query_builder = query_builder.bind(limit as i64);
        }

        if let Some(offset) = filter.offset {
            query_builder = query_builder.bind(offset as i64);
        }

        let rows = query_builder.fetch_all(&self.pool).await?;

        Ok(rows.into_iter().map(|r| r.into_event()).collect())
    }

    async fn count(&self, filter: &AuditFilter) -> Result<u64> {
        // Build dynamic count query with filters
        let mut query = String::from("SELECT COUNT(*) FROM audit_log WHERE 1=1");

        let mut param_count = 0;

        if filter.user_id.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND user_id = ${}", param_count));
        }

        if filter.action.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND action = ${}", param_count));
        }

        if filter.resource_prefix.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND resource LIKE ${}", param_count));
        }

        if filter.from.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND timestamp >= ${}", param_count));
        }

        if filter.to.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND timestamp <= ${}", param_count));
        }

        if filter.success_only.is_some() {
            param_count += 1;
            query.push_str(&format!(" AND success = ${}", param_count));
        }

        let _ = param_count; // suppress unused warning

        let mut query_builder = sqlx::query_as::<_, (i64,)>(&query);

        if let Some(ref user_id) = filter.user_id {
            query_builder = query_builder.bind(user_id);
        }

        if let Some(ref action) = filter.action {
            query_builder = query_builder.bind(action_to_string(action));
        }

        if let Some(ref prefix) = filter.resource_prefix {
            query_builder = query_builder.bind(format!("{}%", prefix));
        }

        if let Some(from) = filter.from {
            query_builder = query_builder.bind(from);
        }

        if let Some(to) = filter.to {
            query_builder = query_builder.bind(to);
        }

        if let Some(success_only) = filter.success_only {
            query_builder = query_builder.bind(success_only);
        }

        let (count,) = query_builder.fetch_one(&self.pool).await?;

        Ok(count as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_to_string() {
        assert_eq!(action_to_string(&AuditAction::Login), "login");
        assert_eq!(action_to_string(&AuditAction::UserCreated), "user_created");
        assert_eq!(
            action_to_string(&AuditAction::Custom("custom_action".to_string())),
            "custom_action"
        );
    }

    #[test]
    fn test_parse_action() {
        assert_eq!(parse_action("login"), AuditAction::Login);
        assert_eq!(parse_action("user_created"), AuditAction::UserCreated);
        assert_eq!(
            parse_action("unknown_action"),
            AuditAction::Custom("unknown_action".to_string())
        );
    }
}
