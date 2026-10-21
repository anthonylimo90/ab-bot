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
        .route(
            "/api/v1/workspaces/:workspace_id",
            put(update_workspace).get(get_workspace),
        )
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

const SECRET_RPC_URL: &str = "https://polygon-mainnet.g.alchemy.com/v2/SECRETKEY123";
const MASKED_RPC_URL: &str = "https://polygon-mainnet.g.alchemy.com/***";

async fn stored_rpc_url(pool: &PgPool, workspace: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT polygon_rpc_url FROM workspaces WHERE id = $1")
        .bind(workspace)
        .fetch_one(pool)
        .await
        .unwrap()
}

impl Fixture {
    async fn request(
        &self,
        who: &str,
        method: &str,
        workspace: Uuid,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let uri = format!("/api/v1/workspaces/{workspace}");
        fx::call(&self.app, method, &uri, Some(&self.tokens[who]), body).await
    }
}

#[test]
fn rpc_urls_are_masked_down_to_scheme_host_and_port() {
    for (url, masked) in [
        (SECRET_RPC_URL, MASKED_RPC_URL),
        (
            "https://rpc.ankr.com/polygon/abcdef0123456789",
            "https://rpc.ankr.com/***",
        ),
        (
            "https://polygon-mainnet.infura.io/v3/KEY?token=SECRET#frag",
            "https://polygon-mainnet.infura.io/***",
        ),
        (
            "https://user:pass@gateway.tenderly.co:8443/KEY",
            "https://gateway.tenderly.co:8443/***",
        ),
        (
            "https://polygon-rpc.com/?apikey=SECRET",
            "https://polygon-rpc.com/***",
        ),
        // Nothing secret to hide.
        ("https://polygon-rpc.com", "https://polygon-rpc.com"),
        ("https://polygon-rpc.com/", "https://polygon-rpc.com"),
        // Not a URL at all: reveal nothing.
        ("not a url SECRETKEY", "***"),
        ("", "***"),
    ] {
        assert_eq!(mask_rpc_url(url), masked, "{url}");
        assert!(!mask_rpc_url(url).contains("SECRET"), "{url}");
        // Masking is idempotent and recognisable.
        assert_eq!(mask_rpc_url(&mask_rpc_url(url)), mask_rpc_url(url));
    }
    assert!(is_masked_rpc_url(MASKED_RPC_URL));
    assert!(!is_masked_rpc_url(SECRET_RPC_URL));
    assert!(!is_masked_rpc_url("https://polygon-rpc.com"));
}

#[sqlx::test(migrations = false)]
async fn members_never_receive_the_rpc_url_path_or_key(pool: PgPool) {
    let f = fixture(&pool).await;
    sqlx::query("UPDATE workspaces SET polygon_rpc_url = $1 WHERE id = $2")
        .bind(SECRET_RPC_URL)
        .bind(f.canonical)
        .execute(&pool)
        .await
        .unwrap();
    for who in ["owner_a", "admin_a", "member_a", "viewer_a"] {
        let (status, body) = f.request(who, "GET", f.canonical, None).await;
        assert_eq!(status, StatusCode::OK, "{who}");
        assert_eq!(body["polygon_rpc_url"], MASKED_RPC_URL, "{who}");
        assert!(!body.to_string().contains("SECRETKEY123"), "{who}");
    }
    // Unset stays unset.
    sqlx::query("UPDATE workspaces SET polygon_rpc_url = NULL")
        .execute(&pool)
        .await
        .unwrap();
    let (_, body) = f.request("owner_a", "GET", f.canonical, None).await;
    assert!(body["polygon_rpc_url"].is_null());
}

#[sqlx::test(migrations = false)]
async fn saving_the_form_with_the_masked_url_keeps_the_stored_url(pool: PgPool) {
    let f = fixture(&pool).await;
    sqlx::query("UPDATE workspaces SET polygon_rpc_url = $1 WHERE id = $2")
        .bind(SECRET_RPC_URL)
        .bind(f.canonical)
        .execute(&pool)
        .await
        .unwrap();

    // The settings page echoes the (masked) URL it was shown while saving other fields.
    let (status, body) = f
        .request(
            "owner_a",
            "PUT",
            f.canonical,
            Some(json!({"name": "renamed", "polygon_rpc_url": MASKED_RPC_URL})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["name"], "renamed");
    assert_eq!(body["polygon_rpc_url"], MASKED_RPC_URL);
    assert_eq!(
        stored_rpc_url(&pool, f.canonical).await.as_deref(),
        Some(SECRET_RPC_URL)
    );

    // A masked value that is not the current one cannot be applied.
    for bogus in [
        "https://rpc.ankr.com/***",
        "https://polygon-mainnet.g.alchemy.com/***?x=1",
    ] {
        let (status, _) = f
            .request(
                "owner_a",
                "PUT",
                f.canonical,
                Some(json!({"polygon_rpc_url": bogus})),
            )
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bogus}");
    }
    assert_eq!(
        stored_rpc_url(&pool, f.canonical).await.as_deref(),
        Some(SECRET_RPC_URL)
    );

    // A genuinely new URL is stored in full but still returned masked.
    let new_url = "https://rpc.ankr.com/polygon/NEWKEY456";
    let (status, body) = f
        .request(
            "owner_a",
            "PUT",
            f.canonical,
            Some(json!({"polygon_rpc_url": new_url})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["polygon_rpc_url"], "https://rpc.ankr.com/***");
    assert_eq!(
        stored_rpc_url(&pool, f.canonical).await.as_deref(),
        Some(new_url)
    );
}

#[sqlx::test(migrations = false)]
async fn audit_entries_never_contain_the_previous_rpc_url_key(pool: PgPool) {
    let f = fixture(&pool).await;
    sqlx::query("UPDATE workspaces SET polygon_rpc_url = $1 WHERE id = $2")
        .bind(SECRET_RPC_URL)
        .bind(f.canonical)
        .execute(&pool)
        .await
        .unwrap();
    let (status, body) = f
        .request(
            "owner_a",
            "PUT",
            f.canonical,
            Some(json!({"polygon_rpc_url": "https://rpc.ankr.com/polygon/NEWKEY456"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Audit events are written by a background task: wait for ours.
    let mut details = None;
    for _ in 0..60 {
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT details::text FROM audit_log WHERE action = 'workspace_updated' AND resource = $1",
        )
        .bind(format!("user/{}", f.canonical))
        .fetch_all(&pool)
        .await
        .unwrap();
        if let Some(row) = rows.into_iter().next() {
            details = Some(row);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let details = details.expect("the workspace update was audited");
    assert!(details.contains(MASKED_RPC_URL), "{details}");
    assert!(!details.contains("SECRETKEY123"), "{details}");
    assert!(!details.contains("NEWKEY456"), "{details}");
    assert!(!details.contains("/v2/"), "{details}");
}
