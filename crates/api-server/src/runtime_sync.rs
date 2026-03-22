//! Runtime reconciliation helpers for workspace-driven service enablement.

use sqlx::PgPool;

use crate::state::AppState;
use crate::workspace_scope::load_canonical_workspace_flags;

/// Returns true when the canonical trading workspace has arb auto-execution enabled.
pub async fn canonical_workspace_arb_enabled(pool: &PgPool) -> Result<bool, sqlx::Error> {
    Ok(load_canonical_workspace_flags(pool)
        .await?
        .map(|flags| flags.arb_auto_execute)
        .unwrap_or(false))
}

/// Returns true when the canonical trading workspace has exit handler enabled.
pub async fn canonical_workspace_exit_handler_enabled(pool: &PgPool) -> Result<bool, sqlx::Error> {
    Ok(load_canonical_workspace_flags(pool)
        .await?
        .map(|flags| flags.exit_handler_enabled)
        .unwrap_or(false))
}

/// Returns true when the canonical trading workspace has live-trading enabled.
pub async fn canonical_workspace_live_enabled(pool: &PgPool) -> Result<bool, sqlx::Error> {
    Ok(load_canonical_workspace_flags(pool)
        .await?
        .map(|flags| flags.live_trading_enabled)
        .unwrap_or(false))
}

/// Reconcile runtime service toggles from workspace flags.
pub async fn reconcile_runtime_service_toggles(state: &AppState) {
    match canonical_workspace_arb_enabled(&state.pool).await {
        Ok(enabled) => {
            if let Some(ref arb_config) = state.arb_executor_config {
                arb_config.write().await.enabled = enabled;
                tracing::info!(
                    arb_auto_execute = enabled,
                    "Reconciled arb executor runtime state"
                );
            }
        }
        Err(error) => {
            tracing::warn!(
                error = %error,
                "Failed to reconcile arb executor runtime state"
            );
        }
    }

    match canonical_workspace_exit_handler_enabled(&state.pool).await {
        Ok(enabled) => {
            if let Some(ref eh_config) = state.exit_handler_config {
                eh_config.write().await.enabled = enabled;
                tracing::info!(
                    exit_handler_enabled = enabled,
                    "Reconciled exit handler runtime state"
                );
            }
        }
        Err(error) => {
            tracing::warn!(
                error = %error,
                "Failed to reconcile exit handler runtime state"
            );
        }
    }

    match canonical_workspace_live_enabled(&state.pool).await {
        Ok(enabled) => {
            state.order_executor.set_live_mode(enabled);
            tracing::info!(
                live_trading_enabled = enabled,
                "Reconciled live trading runtime state"
            );
        }
        Err(error) => {
            tracing::warn!(
                error = %error,
                "Failed to reconcile live trading runtime state"
            );
        }
    }
}
