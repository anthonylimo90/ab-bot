use std::sync::Arc;

use auth::Claims;
use axum::extract::{FromRef, FromRequestParts};
use axum::http::request::Parts;
use polymarket_core::types::WorkspaceRole;
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};
use crate::state::AppState;

#[cfg(test)]
#[path = "workspace_scope/access_tests.rs"]
mod access_tests;
#[cfg(test)]
#[path = "workspace_scope/test_fixture.rs"]
pub(crate) mod test_fixture;

/// Pool-only state used by the workspace authorization extractors.
#[derive(Clone)]
pub struct WorkspaceAccessState(PgPool);

impl FromRef<Arc<AppState>> for WorkspaceAccessState {
    fn from_ref(state: &Arc<AppState>) -> Self {
        Self(state.pool.clone())
    }
}

/// Access to the canonical workspace's shared position and order book.
/// The private field ensures this guard can only be obtained by authorization.
pub struct WorkspaceReadAccess(());

/// Permission to trade in the canonical workspace, in addition to platform RBAC.
pub struct WorkspaceTradeAccess(());

async fn authorize_workspace_access(parts: &Parts, pool: &PgPool, trading: bool) -> ApiResult<()> {
    // Claims are supplied by require_auth after JWT validation.
    let claims = parts
        .extensions
        .get::<Claims>()
        .ok_or_else(|| ApiError::Unauthorized("Authentication required".into()))?;
    authorize_workspace_claims(pool, claims, trading).await
}

pub(crate) async fn authorize_workspace_claims(
    pool: &PgPool,
    claims: &Claims,
    trading: bool,
) -> ApiResult<()> {
    let user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("Invalid user ID".into()))?;
    let membership = resolve_canonical_workspace_membership(pool, user_id).await?;
    require_workspace_role(
        claims,
        membership.as_ref().map(|m| m.role.as_str()),
        trading,
    )
}

fn require_workspace_role(claims: &Claims, role: Option<&str>, trading: bool) -> ApiResult<()> {
    let role = role
        .ok_or_else(|| ApiError::Forbidden("Not a member of the canonical workspace".into()))?
        .parse::<WorkspaceRole>()
        .map_err(|_| ApiError::Forbidden("Unrecognized workspace role".into()))?;
    if trading
        && (!claims.role.can_trade()
            || !matches!(
                role,
                WorkspaceRole::Owner | WorkspaceRole::Admin | WorkspaceRole::Member
            ))
    {
        return Err(ApiError::Forbidden(
            "Trading permission required in the canonical workspace".into(),
        ));
    }
    Ok(())
}

/// One database snapshot per event, with no stale authorization cache.
pub(crate) async fn authorize_workspace_session(
    pool: &PgPool,
    jwt: &auth::jwt::JwtAuth,
    token: &str,
) -> ApiResult<()> {
    let claims = crate::middleware::validate_session_token(jwt, token)?;
    let user = Uuid::parse_str(&claims.sub)
        .map_err(|_| ApiError::Unauthorized("Invalid subject".into()))?;
    let query = format!(
        "SELECT u.auth_version, u.role::integer,
         EXISTS(SELECT 1 FROM revoked_sessions WHERE jti=$2), wm.role
         FROM users u LEFT JOIN workspace_members wm
           ON wm.user_id=u.id AND wm.workspace_id=({CANONICAL_WORKSPACE_SQL})
         WHERE u.id=$1"
    );
    let current: Option<(i64, i32, bool, Option<String>)> = sqlx::query_as(&query)
        .bind(user)
        .bind(&claims.jti)
        .fetch_optional(pool)
        .await?;
    crate::middleware::require_current_session(&claims, current.as_ref().map(|r| (r.0, r.1, r.2)))?;
    require_workspace_role(
        &claims,
        current.as_ref().and_then(|r| r.3.as_deref()),
        false,
    )
}

#[axum::async_trait]
impl<S> FromRequestParts<S> for WorkspaceReadAccess
where
    S: Send + Sync,
    WorkspaceAccessState: FromRef<S>,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> ApiResult<Self> {
        authorize_workspace_access(parts, &WorkspaceAccessState::from_ref(state).0, false).await?;
        Ok(Self(()))
    }
}

#[axum::async_trait]
impl<S> FromRequestParts<S> for WorkspaceTradeAccess
where
    S: Send + Sync,
    WorkspaceAccessState: FromRef<S>,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> ApiResult<Self> {
        authorize_workspace_access(parts, &WorkspaceAccessState::from_ref(state).0, true).await?;
        Ok(Self(()))
    }
}

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

const CANONICAL_WORKSPACE_SQL: &str = r#"
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
        "#;

pub async fn resolve_canonical_workspace_id(pool: &PgPool) -> Result<Option<Uuid>, sqlx::Error> {
    let row: Option<(Uuid,)> = sqlx::query_as(CANONICAL_WORKSPACE_SQL)
        .fetch_optional(pool)
        .await?;

    Ok(row.map(|(id,)| id))
}

pub async fn resolve_canonical_workspace_membership(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<Option<CanonicalWorkspaceMembership>, sqlx::Error> {
    let query = format!(
        r#"
        SELECT wm.workspace_id AS id, wm.role
        FROM workspace_members wm
        WHERE wm.workspace_id = ({CANONICAL_WORKSPACE_SQL})
          AND wm.user_id = $1
        "#,
    );
    sqlx::query_as::<_, CanonicalWorkspaceMembership>(&query)
        .bind(user_id)
        .fetch_optional(pool)
        .await
}

pub async fn load_canonical_workspace_flags(
    pool: &PgPool,
) -> Result<Option<CanonicalWorkspaceFlags>, sqlx::Error> {
    let query = format!(
        r#"
        SELECT
            w.id,
            COALESCE(w.arb_auto_execute, FALSE) AS arb_auto_execute,
            COALESCE(w.live_trading_enabled, FALSE) AS live_trading_enabled,
            COALESCE(w.exit_handler_enabled, FALSE) AS exit_handler_enabled
        FROM workspaces w
        WHERE w.id = ({CANONICAL_WORKSPACE_SQL})
        "#,
    );
    sqlx::query_as::<_, CanonicalWorkspaceFlags>(&query)
        .fetch_optional(pool)
        .await
}
