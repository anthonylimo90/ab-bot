//! Runtime reconciliation helpers for workspace-driven service enablement.

use sqlx::PgPool;

/// Returns true when at least one workspace has arb auto-execution enabled.
pub async fn any_workspace_arb_enabled(pool: &PgPool) -> Result<bool, sqlx::Error> {
    let enabled: Option<(bool,)> = sqlx::query_as(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM workspaces
            WHERE COALESCE(arb_auto_execute, FALSE) = TRUE
        )
        "#,
    )
    .fetch_optional(pool)
    .await?;

    Ok(enabled.map(|(v,)| v).unwrap_or(false))
}

/// Returns true when at least one workspace has exit handler enabled.
pub async fn any_workspace_exit_handler_enabled(pool: &PgPool) -> Result<bool, sqlx::Error> {
    let enabled: Option<(bool,)> = sqlx::query_as(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM workspaces
            WHERE COALESCE(exit_handler_enabled, FALSE) = TRUE
        )
        "#,
    )
    .fetch_optional(pool)
    .await?;

    Ok(enabled.map(|(v,)| v).unwrap_or(false))
}

/// Returns true when at least one workspace has live-trading enabled.
pub async fn any_workspace_live_enabled(pool: &PgPool) -> Result<bool, sqlx::Error> {
    let enabled: Option<(bool,)> = sqlx::query_as(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM workspaces
            WHERE COALESCE(live_trading_enabled, FALSE) = TRUE
        )
        "#,
    )
    .fetch_optional(pool)
    .await?;

    Ok(enabled.map(|(v,)| v).unwrap_or(false))
}
