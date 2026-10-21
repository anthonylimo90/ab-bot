//! Authorization regression tests for workspace settings that reach global state:
//! runtime flags (which feed the canonical-workspace election) and `dynamic_config`.
//! They run against the real user/workspace schema with two workspaces.

use super::*;
use crate::workspace_scope::{resolve_canonical_workspace_id, test_fixture as fx};
use axum::http::StatusCode;
use axum::routing::put;
use axum::Router;
use serde_json::json;
use sqlx::PgPool;

struct Fixture {
    app: Router,
    /// Canonical workspace (older) and a second, non-canonical one.
    canonical: Uuid,
    other: Uuid,
    tokens: std::collections::HashMap<&'static str, String>,
}

impl Fixture {
    async fn send(
        &self,
        who: &str,
        workspace: Uuid,
        suffix: &str,
        body: serde_json::Value,
    ) -> StatusCode {
        let uri = format!("/api/v1/workspaces/{workspace}{suffix}");
        fx::call(&self.app, "PUT", &uri, Some(&self.tokens[who]), Some(body))
            .await
            .0
    }
}

async fn fixture(pool: &PgPool) -> Fixture {
    fx::real_schema(pool).await;
    let state = AppState::for_tests(pool.clone());
    let canonical = fx::workspace(pool, "canonical", 1000).await;
    let other = fx::workspace(pool, "other", 10).await;
    let mut ids = Vec::new();
    for (name, workspace, role, platform) in [
        ("owner_a", canonical, "owner", UserRole::Trader),
        ("admin_a", canonical, "admin", UserRole::Trader),
        ("member_a", canonical, "member", UserRole::Trader),
        ("viewer_a", canonical, "viewer", UserRole::Trader),
        ("owner_b", other, "owner", UserRole::Trader),
        ("admin_b", other, "admin", UserRole::Trader),
    ] {
        ids.push((
            name,
            fx::member(pool, workspace, role, platform).await,
            platform,
        ));
    }
    // A platform admin that belongs to no workspace.
    ids.push((
        "platform_admin",
        fx::user(pool, UserRole::PlatformAdmin).await,
        UserRole::PlatformAdmin,
    ));
    let mut tokens = std::collections::HashMap::new();
    for (name, id, platform) in ids {
        tokens.insert(name, fx::token(&state, id, platform).await);
    }
    let app = Router::new()
        .route("/api/v1/workspaces/:workspace_id", put(update_workspace))
        .route(
            "/api/v1/workspaces/:workspace_id/dynamic-tuning/opportunity-selection",
            put(update_opportunity_selection_settings),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/dynamic-tuning/arb-executor",
            put(update_arb_executor_config),
        )
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::middleware::require_trader,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::middleware::require_auth,
        ))
        .with_state(state);
    Fixture {
        app,
        canonical,
        other,
        tokens,
    }
}

async fn manual_config_rows(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM dynamic_config WHERE updated_by = 'workspace_manual'")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn manual_history_rows(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM dynamic_config_history WHERE action = 'manual_update'")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = false)]
async fn non_canonical_owner_cannot_enable_runtime_flags_or_flip_election(pool: PgPool) {
    let f = fixture(&pool).await;
    assert_eq!(
        resolve_canonical_workspace_id(&pool).await.unwrap(),
        Some(f.canonical)
    );
    for body in [
        json!({"live_trading_enabled": true}),
        json!({"exit_handler_enabled": true}),
        json!({"arb_auto_execute": true}),
        json!({"name": "renamed", "live_trading_enabled": true, "arb_auto_execute": true}),
    ] {
        assert_eq!(
            f.send("owner_b", f.other, "", body.clone()).await,
            StatusCode::FORBIDDEN,
            "{body}"
        );
    }
    assert_eq!(fx::flags(&pool, f.other).await, (false, false, false));
    // The rejected request must not have partially applied (e.g. the rename).
    let name: String = sqlx::query_scalar("SELECT name FROM workspaces WHERE id = $1")
        .bind(f.other)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(name, "other");
    // The election is unchanged: the other workspace did not become canonical.
    assert_eq!(
        resolve_canonical_workspace_id(&pool).await.unwrap(),
        Some(f.canonical)
    );
}

#[sqlx::test(migrations = false)]
async fn canonical_owner_can_enable_flags_and_others_still_cannot(pool: PgPool) {
    let f = fixture(&pool).await;
    assert_eq!(
        f.send(
            "owner_a",
            f.canonical,
            "",
            json!({"live_trading_enabled": true, "exit_handler_enabled": true})
        )
        .await,
        StatusCode::OK
    );
    assert_eq!(fx::flags(&pool, f.canonical).await, (true, true, false));
    assert_eq!(
        resolve_canonical_workspace_id(&pool).await.unwrap(),
        Some(f.canonical)
    );
    // Still rejected for the other workspace now that the canonical one is armed.
    assert_eq!(
        f.send("owner_b", f.other, "", json!({"arb_auto_execute": true}))
            .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(fx::flags(&pool, f.other).await, (false, false, false));
    // update_workspace remains owner-only, as before.
    assert_eq!(
        f.send(
            "admin_a",
            f.canonical,
            "",
            json!({"arb_auto_execute": true})
        )
        .await,
        StatusCode::FORBIDDEN
    );
}

#[sqlx::test(migrations = false)]
async fn disabling_flags_on_a_non_canonical_workspace_stays_allowed(pool: PgPool) {
    let f = fixture(&pool).await;
    sqlx::query("UPDATE workspaces SET live_trading_enabled = TRUE")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        resolve_canonical_workspace_id(&pool).await.unwrap(),
        Some(f.canonical)
    );
    // Turning a stale flag off (and re-sending an already-on flag) is not an enable.
    let status = f
        .send(
            "owner_b",
            f.other,
            "",
            json!({"live_trading_enabled": false, "exit_handler_enabled": false}),
        )
        .await;
    assert_ne!(status, StatusCode::FORBIDDEN);
    assert_eq!(fx::flags(&pool, f.other).await, (false, false, false));
    assert_eq!(fx::flags(&pool, f.canonical).await, (true, false, false));
}

#[sqlx::test(migrations = false)]
async fn global_dynamic_config_needs_canonical_admin_or_platform_admin(pool: PgPool) {
    let f = fixture(&pool).await;
    let endpoints = [
        (
            "/dynamic-tuning/opportunity-selection",
            json!({"aggressiveness": "discovery"}),
        ),
        (
            "/dynamic-tuning/arb-executor",
            json!({"position_size": 10.0}),
        ),
    ];
    for (suffix, body) in &endpoints {
        // Owners/admins of the *other* workspace, plain canonical members and
        // viewers must not write global config, whichever workspace id they use.
        for who in ["owner_b", "admin_b", "member_a", "viewer_a"] {
            for workspace in [f.other, f.canonical] {
                assert_eq!(
                    f.send(who, workspace, suffix, body.clone()).await,
                    StatusCode::FORBIDDEN,
                    "{who} {suffix}"
                );
            }
        }
        assert_eq!(manual_config_rows(&pool).await, 0, "{suffix}");
        assert_eq!(manual_history_rows(&pool).await, 0, "{suffix}");

        // Canonical owner/admin and platform admins pass authorization. The test has no
        // Redis, so the handler fails after persisting, which is what proves the write.
        for who in ["owner_a", "admin_a", "platform_admin"] {
            let status = f.send(who, f.canonical, suffix, body.clone()).await;
            assert_ne!(status, StatusCode::FORBIDDEN, "{who} {suffix}");
            assert!(manual_config_rows(&pool).await > 0, "{who} {suffix}");
            assert!(manual_history_rows(&pool).await > 0, "{who} {suffix}");
            sqlx::query("DELETE FROM dynamic_config WHERE updated_by = 'workspace_manual'")
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("DELETE FROM dynamic_config_history WHERE action = 'manual_update'")
                .execute(&pool)
                .await
                .unwrap();
        }
    }
}
