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

/// Whether the environment switches `name` on (`name=true`), as startup reads it.
pub(crate) fn env_enabled(name: &str) -> bool {
    polymarket_core::config::env_switch(name)
}

/// Reconcile runtime service toggles from the canonical workspace's flags.
/// As at startup, a service the environment switches on stays on: the
/// workspace flag can add a service, not remove one the operator configured.
/// Runs on every workspace save and every few seconds on the trading
/// leader, so only a change is logged.
pub async fn reconcile_runtime_service_toggles(state: &AppState) {
    let flags = match load_canonical_workspace_flags(&state.pool).await {
        Ok(flags) => flags,
        Err(error) => {
            tracing::warn!(%error, "Failed to reconcile runtime service toggles");
            return;
        }
    };
    let arb = flags.as_ref().is_some_and(|f| f.arb_auto_execute) || env_enabled("ARB_AUTO_EXECUTE");
    let exit_handler = flags.as_ref().is_some_and(|f| f.exit_handler_enabled)
        || env_enabled("EXIT_HANDLER_ENABLED");
    let live =
        flags.as_ref().is_some_and(|f| f.live_trading_enabled) || env_enabled("LIVE_TRADING");

    if let Some(ref arb_config) = state.arb_executor_config {
        let mut config = arb_config.write().await;
        if config.enabled != arb {
            config.enabled = arb;
            tracing::info!(
                arb_auto_execute = arb,
                "Reconciled arb executor runtime state"
            );
        }
    }
    if let Some(ref eh_config) = state.exit_handler_config {
        let mut config = eh_config.write().await;
        if config.enabled != exit_handler {
            config.enabled = exit_handler;
            tracing::info!(
                exit_handler_enabled = exit_handler,
                "Reconciled exit handler runtime state"
            );
        }
    }
    if state.order_executor.is_live() != live {
        state.order_executor.set_live_mode(live);
        tracing::info!(
            live_trading_enabled = live,
            "Reconciled live trading runtime state"
        );
    }
}
