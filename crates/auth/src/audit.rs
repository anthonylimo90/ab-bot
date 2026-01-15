//! Audit logging for security and compliance.

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Types of auditable actions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    // Authentication
    Login,
    Logout,
    LoginFailed,
    TokenRefresh,
    ApiKeyCreated,
    ApiKeyRevoked,

    // Trading
    CreatePosition,
    ClosePosition,
    ManualExit,
    EmergencyExitAll,

    // Copy Trading
    AddTrackedWallet,
    RemoveTrackedWallet,
    UpdateTrackedWallet,
    CopyTradeExecuted,

    // Risk Management
    CreateStopLoss,
    RemoveStopLoss,
    StopLossTriggered,
    CircuitBreakerTripped,
    CircuitBreakerReset,

    // Configuration
    ConfigChange,
    PositionLimitsChange,

    // Data Access
    ExportData,
    ViewSensitiveData,

    // Other
    Custom(String),
}

/// An audit event record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub id: i64,
    /// When the event occurred.
    pub timestamp: DateTime<Utc>,
    /// User who performed the action (if authenticated).
    pub user_id: Option<String>,
    /// Type of action.
    pub action: AuditAction,
    /// Resource that was affected.
    pub resource: String,
    /// Additional details about the action.
    pub details: serde_json::Value,
    /// IP address of the client.
    pub ip_address: Option<IpAddr>,
    /// User agent string.
    pub user_agent: Option<String>,
    /// Was the action successful?
    pub success: bool,
    /// Error message if unsuccessful.
    pub error: Option<String>,
}

impl AuditEvent {
    /// Create a new audit event builder.
    pub fn builder(action: AuditAction, resource: impl Into<String>) -> AuditEventBuilder {
        AuditEventBuilder {
            action,
            resource: resource.into(),
            user_id: None,
            details: serde_json::Value::Null,
            ip_address: None,
            user_agent: None,
            success: true,
            error: None,
        }
    }
}

/// Builder for audit events.
pub struct AuditEventBuilder {
    action: AuditAction,
    resource: String,
    user_id: Option<String>,
    details: serde_json::Value,
    ip_address: Option<IpAddr>,
    user_agent: Option<String>,
    success: bool,
    error: Option<String>,
}

impl AuditEventBuilder {
    pub fn user(mut self, user_id: impl Into<String>) -> Self {
        self.user_id = Some(user_id.into());
        self
    }

    pub fn details(mut self, details: serde_json::Value) -> Self {
        self.details = details;
        self
    }

    pub fn ip(mut self, ip: IpAddr) -> Self {
        self.ip_address = Some(ip);
        self
    }

    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.user_agent = Some(ua.into());
        self
    }

    pub fn failure(mut self, error: impl Into<String>) -> Self {
        self.success = false;
        self.error = Some(error.into());
        self
    }

    pub fn build(self) -> AuditEvent {
        AuditEvent {
            id: 0, // Set by database
            timestamp: Utc::now(),
            user_id: self.user_id,
            action: self.action,
            resource: self.resource,
            details: self.details,
            ip_address: self.ip_address,
            user_agent: self.user_agent,
            success: self.success,
            error: self.error,
        }
    }
}

/// Storage backend for audit logs.
#[async_trait::async_trait]
pub trait AuditStorage: Send + Sync {
    /// Store an audit event.
    async fn store(&self, event: &AuditEvent) -> Result<i64>;

    /// Query audit events.
    async fn query(&self, filter: &AuditFilter) -> Result<Vec<AuditEvent>>;

    /// Count events matching filter.
    async fn count(&self, filter: &AuditFilter) -> Result<u64>;
}

/// Filter for querying audit events.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuditFilter {
    pub user_id: Option<String>,
    pub action: Option<AuditAction>,
    pub resource_prefix: Option<String>,
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    pub success_only: Option<bool>,
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

impl AuditFilter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn user(mut self, user_id: impl Into<String>) -> Self {
        self.user_id = Some(user_id.into());
        self
    }

    pub fn action(mut self, action: AuditAction) -> Self {
        self.action = Some(action);
        self
    }

    pub fn resource(mut self, prefix: impl Into<String>) -> Self {
        self.resource_prefix = Some(prefix.into());
        self
    }

    pub fn time_range(mut self, from: DateTime<Utc>, to: DateTime<Utc>) -> Self {
        self.from = Some(from);
        self.to = Some(to);
        self
    }

    pub fn limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit);
        self
    }

    pub fn offset(mut self, offset: u32) -> Self {
        self.offset = Some(offset);
        self
    }
}

/// In-memory audit storage for testing.
pub struct MemoryAuditStorage {
    events: Arc<tokio::sync::RwLock<Vec<AuditEvent>>>,
    next_id: Arc<std::sync::atomic::AtomicI64>,
}

impl MemoryAuditStorage {
    pub fn new() -> Self {
        Self {
            events: Arc::new(tokio::sync::RwLock::new(Vec::new())),
            next_id: Arc::new(std::sync::atomic::AtomicI64::new(1)),
        }
    }
}

impl Default for MemoryAuditStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl AuditStorage for MemoryAuditStorage {
    async fn store(&self, event: &AuditEvent) -> Result<i64> {
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut stored = event.clone();
        stored.id = id;

        let mut events = self.events.write().await;
        events.push(stored);

        Ok(id)
    }

    async fn query(&self, filter: &AuditFilter) -> Result<Vec<AuditEvent>> {
        let events = self.events.read().await;

        let filtered: Vec<_> = events
            .iter()
            .filter(|e| {
                if let Some(ref user) = filter.user_id {
                    if e.user_id.as_ref() != Some(user) {
                        return false;
                    }
                }
                if let Some(ref action) = filter.action {
                    if &e.action != action {
                        return false;
                    }
                }
                if let Some(ref prefix) = filter.resource_prefix {
                    if !e.resource.starts_with(prefix) {
                        return false;
                    }
                }
                if let Some(from) = filter.from {
                    if e.timestamp < from {
                        return false;
                    }
                }
                if let Some(to) = filter.to {
                    if e.timestamp > to {
                        return false;
                    }
                }
                if let Some(success_only) = filter.success_only {
                    if e.success != success_only {
                        return false;
                    }
                }
                true
            })
            .cloned()
            .collect();

        let offset = filter.offset.unwrap_or(0) as usize;
        let limit = filter.limit.unwrap_or(100) as usize;

        Ok(filtered.into_iter().skip(offset).take(limit).collect())
    }

    async fn count(&self, filter: &AuditFilter) -> Result<u64> {
        let events = self.query(filter).await?;
        Ok(events.len() as u64)
    }
}

/// Audit logger service.
pub struct AuditLogger {
    storage: Arc<dyn AuditStorage>,
    /// Async channel for non-blocking logging.
    tx: mpsc::Sender<AuditEvent>,
}

impl AuditLogger {
    /// Create a new audit logger.
    pub fn new(storage: Arc<dyn AuditStorage>) -> Self {
        let (tx, mut rx) = mpsc::channel::<AuditEvent>(10000);

        let storage_clone = storage.clone();
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                if let Err(e) = storage_clone.store(&event).await {
                    tracing::error!(error = %e, "Failed to store audit event");
                }
            }
        });

        Self { storage, tx }
    }

    /// Log an audit event (non-blocking).
    pub fn log(&self, event: AuditEvent) {
        if self.tx.try_send(event).is_err() {
            tracing::warn!("Audit log channel full, event dropped");
        }
    }

    /// Log an audit event (blocking, ensures delivery).
    pub async fn log_sync(&self, event: AuditEvent) -> Result<i64> {
        self.storage.store(&event).await
    }

    /// Query audit events.
    pub async fn query(&self, filter: &AuditFilter) -> Result<Vec<AuditEvent>> {
        self.storage.query(filter).await
    }

    /// Count events matching filter.
    pub async fn count(&self, filter: &AuditFilter) -> Result<u64> {
        self.storage.count(filter).await
    }

    // Convenience methods for common audit events

    /// Log a login event.
    pub fn log_login(&self, user_id: &str, ip: Option<IpAddr>, success: bool) {
        let mut builder = AuditEvent::builder(
            if success {
                AuditAction::Login
            } else {
                AuditAction::LoginFailed
            },
            format!("user/{}", user_id),
        )
        .user(user_id);

        if let Some(ip) = ip {
            builder = builder.ip(ip);
        }

        if !success {
            builder = builder.failure("Invalid credentials");
        }

        self.log(builder.build());
    }

    /// Log a trade action.
    pub fn log_trade(
        &self,
        user_id: &str,
        action: AuditAction,
        position_id: &str,
        details: serde_json::Value,
    ) {
        let event = AuditEvent::builder(action, format!("position/{}", position_id))
            .user(user_id)
            .details(details)
            .build();

        self.log(event);
    }

    /// Log a configuration change.
    pub fn log_config_change(
        &self,
        user_id: &str,
        config_key: &str,
        old_value: serde_json::Value,
        new_value: serde_json::Value,
    ) {
        let event =
            AuditEvent::builder(AuditAction::ConfigChange, format!("config/{}", config_key))
                .user(user_id)
                .details(serde_json::json!({
                    "old": old_value,
                    "new": new_value,
                }))
                .build();

        self.log(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_audit_event_builder() {
        let event = AuditEvent::builder(AuditAction::Login, "user/123")
            .user("user123")
            .details(serde_json::json!({"method": "password"}))
            .build();

        assert_eq!(event.action, AuditAction::Login);
        assert_eq!(event.resource, "user/123");
        assert_eq!(event.user_id, Some("user123".to_string()));
        assert!(event.success);
    }

    #[tokio::test]
    async fn test_memory_storage() {
        let storage = MemoryAuditStorage::new();

        let event = AuditEvent::builder(AuditAction::CreatePosition, "position/abc")
            .user("trader1")
            .build();

        let id = storage.store(&event).await.unwrap();
        assert!(id > 0);

        let events = storage.query(&AuditFilter::new()).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, id);
    }

    #[tokio::test]
    async fn test_filter_by_user() {
        let storage = MemoryAuditStorage::new();

        storage
            .store(
                &AuditEvent::builder(AuditAction::Login, "user/1")
                    .user("user1")
                    .build(),
            )
            .await
            .unwrap();
        storage
            .store(
                &AuditEvent::builder(AuditAction::Login, "user/2")
                    .user("user2")
                    .build(),
            )
            .await
            .unwrap();

        let filter = AuditFilter::new().user("user1");
        let events = storage.query(&filter).await.unwrap();

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].user_id, Some("user1".to_string()));
    }

    #[tokio::test]
    async fn test_filter_by_action() {
        let storage = MemoryAuditStorage::new();

        storage
            .store(&AuditEvent::builder(AuditAction::Login, "user/1").build())
            .await
            .unwrap();
        storage
            .store(&AuditEvent::builder(AuditAction::CreatePosition, "pos/1").build())
            .await
            .unwrap();

        let filter = AuditFilter::new().action(AuditAction::Login);
        let events = storage.query(&filter).await.unwrap();

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].action, AuditAction::Login);
    }

    #[tokio::test]
    async fn test_audit_logger() {
        let storage = Arc::new(MemoryAuditStorage::new());
        let logger = AuditLogger::new(storage.clone());

        logger.log_login("user1", None, true);

        // Give async task time to process
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let events = storage.query(&AuditFilter::new()).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].action, AuditAction::Login);
    }
}
