use sqlx::{FromRow, PgPool};
use uuid::Uuid;

#[derive(Debug, Clone, FromRow)]
pub struct CanonicalWorkspaceMembership {
    pub id: Uuid,
    pub role: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct CanonicalWorkspaceFlags {
    pub id: Uuid,
    pub arb_auto_execute: bool,
    pub live_trading_enabled: bool,
    pub exit_handler_enabled: bool,
}

pub async fn resolve_canonical_workspace_id(pool: &PgPool) -> Result<Option<Uuid>, sqlx::Error> {
    let row: Option<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT w.id
        FROM workspaces w
        ORDER BY
            CASE
                WHEN COALESCE(w.live_trading_enabled, FALSE)
                  OR COALESCE(w.exit_handler_enabled, FALSE)
                  OR COALESCE(w.arb_auto_execute, FALSE)
                THEN 0
                ELSE 1
            END,
            CASE WHEN w.trading_wallet_address IS NOT NULL THEN 0 ELSE 1 END,
            w.created_at ASC,
            w.id ASC
        LIMIT 1
        "#,
    )
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|(id,)| id))
}

pub async fn resolve_canonical_workspace_membership(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<Option<CanonicalWorkspaceMembership>, sqlx::Error> {
    let Some(workspace_id) = resolve_canonical_workspace_id(pool).await? else {
        return Ok(None);
    };

    sqlx::query_as::<_, CanonicalWorkspaceMembership>(
        r#"
        SELECT wm.workspace_id AS id, wm.role
        FROM workspace_members wm
        WHERE wm.workspace_id = $1
          AND wm.user_id = $2
        "#,
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
}

pub async fn load_canonical_workspace_flags(
    pool: &PgPool,
) -> Result<Option<CanonicalWorkspaceFlags>, sqlx::Error> {
    let Some(workspace_id) = resolve_canonical_workspace_id(pool).await? else {
        return Ok(None);
    };

    sqlx::query_as::<_, CanonicalWorkspaceFlags>(
        r#"
        SELECT
            w.id,
            COALESCE(w.arb_auto_execute, FALSE) AS arb_auto_execute,
            COALESCE(w.live_trading_enabled, FALSE) AS live_trading_enabled,
            COALESCE(w.exit_handler_enabled, FALSE) AS exit_handler_enabled
        FROM workspaces w
        WHERE w.id = $1
        "#,
    )
    .bind(workspace_id)
    .fetch_optional(pool)
    .await
}
