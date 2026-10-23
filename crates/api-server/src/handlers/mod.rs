//! API request handlers.

pub mod accounting;
pub mod activity;
pub mod admin_workspaces;
pub mod auth;
pub mod backtest;

pub mod discover;
pub mod health;
pub mod markets;
pub mod metrics;
pub mod order_signing;
pub mod positions;
pub mod recovery;
pub mod risk;
pub mod signals;
pub mod strategy_health;
pub mod trade_flow;
pub mod trading;
pub mod users;
pub mod vault;
pub mod wallet_auth;
pub mod wallets;
pub mod workspaces;

/// Record a change to money or risk controls before the request is answered,
/// so the record survives a crash or a full queue. A failed write is logged;
/// the change it describes has already been made.
pub(crate) async fn audit_change(
    state: &crate::state::AppState,
    user_id: uuid::Uuid,
    action: ::auth::AuditAction,
    resource: impl Into<String>,
    details: serde_json::Value,
) {
    let event = ::auth::AuditEvent::builder(action, resource)
        .user(user_id.to_string())
        .details(details)
        .build();
    if let Err(error) = state.audit_logger.log_sync(event).await {
        tracing::error!(%error, "Could not record an audit event");
    }
}
