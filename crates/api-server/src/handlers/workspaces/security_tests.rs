//! Authorization regression tests for workspace settings that reach global state:
//! runtime flags (which feed the canonical-workspace election) and `dynamic_config`.
//! They run against the real user/workspace schema with two workspaces.

use super::*;
use crate::workspace_scope::{resolve_canonical_workspace_id, test_fixture as fx};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::Router;
use serde_json::json;
use sqlx::PgPool;

struct Fixture {
    app: Router,
    /// Canonical workspace (older) and a second, non-canonical one.
    canonical: Uuid,
    other: Uuid,
    tokens: std::collections::HashMap<&'static str, String>,
    user_ids: std::collections::HashMap<&'static str, Uuid>,
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
        ("admin_a2", canonical, "admin", UserRole::Trader),
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
    let mut user_ids = std::collections::HashMap::new();
    for (name, id, platform) in ids {
        tokens.insert(name, fx::token(&state, id, platform).await);
        user_ids.insert(name, id);
    }
    let app = Router::new()
        .route(
            "/api/v1/workspaces/:workspace_id",
            put(update_workspace).get(get_workspace),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/members/:member_id",
            put(update_member_role).delete(remove_member),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/dynamic-tuning/opportunity-selection",
            put(update_opportunity_selection_settings),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/dynamic-tuning/arb-executor",
            put(update_arb_executor_config),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/dynamic-tuning/status",
            get(get_dynamic_tuner_status),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/dynamic-tuning/history",
            get(get_dynamic_tuning_history),
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
        user_ids,
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

#[sqlx::test(migrations = false)]
async fn global_tuning_is_visible_only_to_canonical_members(pool: PgPool) {
    let f = fixture(&pool).await;
    for suffix in ["/dynamic-tuning/status", "/dynamic-tuning/history"] {
        for workspace in [f.other, f.canonical] {
            let uri = format!("/api/v1/workspaces/{workspace}{suffix}");
            // Owning another workspace grants no view of global state...
            let (status, _) = fx::call(&f.app, "GET", &uri, Some(&f.tokens["owner_b"]), None).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "owner_b {uri}");
            // ...while canonical members see it whichever id the path names.
            let (status, _) =
                fx::call(&f.app, "GET", &uri, Some(&f.tokens["viewer_a"]), None).await;
            assert_eq!(status, StatusCode::OK, "viewer_a {uri}");
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

    // The update's audit entry is written before it answers.
    let mut details = None;
    for _ in 0..60 {
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT details::text FROM audit_log WHERE action = 'workspace_updated' AND resource = $1",
        )
        .bind(format!("workspace/{}", f.canonical))
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

impl Fixture {
    /// PUT the member's role as `who`; returns the status.
    async fn set_role(&self, who: &str, member: &str, role: &str) -> StatusCode {
        let uri = format!(
            "/api/v1/workspaces/{}/members/{}",
            self.canonical, self.user_ids[member]
        );
        fx::call(
            &self.app,
            "PUT",
            &uri,
            Some(&self.tokens[who]),
            Some(json!({ "role": role })),
        )
        .await
        .0
    }

    async fn role_of(&self, pool: &PgPool, member: &str) -> String {
        sqlx::query_scalar(
            "SELECT role FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
        )
        .bind(self.canonical)
        .bind(self.user_ids[member])
        .fetch_one(pool)
        .await
        .unwrap()
    }
}

#[sqlx::test(migrations = false)]
async fn only_the_owner_can_change_an_admins_role(pool: PgPool) {
    let f = fixture(&pool).await;
    // An admin cannot demote a peer admin (remove_member already forbids removing one)...
    for role in ["member", "viewer"] {
        assert_eq!(
            f.set_role("admin_a", "admin_a2", role).await,
            StatusCode::FORBIDDEN,
            "{role}"
        );
    }
    assert_eq!(f.role_of(&pool, "admin_a2").await, "admin");
    // ...nor can anyone below admin.
    assert_eq!(
        f.set_role("member_a", "admin_a2", "viewer").await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(f.role_of(&pool, "admin_a2").await, "admin");
    // The owner can.
    assert_eq!(
        f.set_role("owner_a", "admin_a2", "member").await,
        StatusCode::OK
    );
    assert_eq!(f.role_of(&pool, "admin_a2").await, "member");
}

#[sqlx::test(migrations = false)]
async fn admins_can_still_manage_non_admin_members(pool: PgPool) {
    let f = fixture(&pool).await;
    assert_eq!(
        f.set_role("admin_a", "member_a", "viewer").await,
        StatusCode::OK
    );
    assert_eq!(f.role_of(&pool, "member_a").await, "viewer");
    assert_eq!(
        f.set_role("admin_a", "viewer_a", "member").await,
        StatusCode::OK
    );
    // Promotion to admin and changing the owner stay as they were.
    assert_eq!(
        f.set_role("admin_a", "member_a", "admin").await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        f.set_role("admin_a", "owner_a", "member").await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        f.set_role("owner_a", "member_a", "admin").await,
        StatusCode::OK
    );
}

/// Store an invite with the statement `create_workspace_invite` runs (that
/// handler also needs an email client) and return its token.
async fn stored_invite(pool: &PgPool, workspace: Uuid, email: &str, invited_by: Uuid) -> String {
    stored_invite_for(pool, workspace, email, invited_by, "member").await
}

async fn stored_invite_for(
    pool: &PgPool,
    workspace: Uuid,
    email: &str,
    invited_by: Uuid,
    role: &str,
) -> String {
    let token = generate_invite_token();
    sqlx::query(
        r#"
        INSERT INTO workspace_invites (id, workspace_id, email, role, token_hash, invited_by, expires_at, created_at)
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(Uuid::new_v4())
    .bind(workspace)
    .bind(email)
    .bind(role)
    .bind(hash_token(&token))
    .bind(invited_by)
    .bind(Utc::now() + Duration::days(7))
    .bind(Utc::now())
    .execute(pool)
    .await
    .unwrap();
    token
}

#[sqlx::test(migrations = false)]
async fn invites_work_on_the_migrated_schema(pool: PgPool) {
    // Migration 044 dropped workspace_invites; 090 restores it. This runs the
    // invite flow against the schema the migrations actually produce.
    let f = fixture(&pool).await;
    let state = AppState::for_tests(pool.clone());
    let owner = f.user_ids["owner_a"];
    let app = Router::new()
        .route(
            "/api/v1/workspaces/:workspace_id/invites",
            get(list_workspace_invites),
        )
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            crate::middleware::require_auth,
        ))
        .route("/api/v1/invites/:token", get(get_invite_info))
        .route(
            "/api/v1/invites/:token/accept",
            axum::routing::post(accept_invite),
        )
        .with_state(state.clone());

    // An existing account accepts while signed in.
    let invitee = fx::user(&pool, UserRole::Trader).await;
    let invitee_email = format!("{invitee}@example.test");
    let token = stored_invite(&pool, f.canonical, &invitee_email, owner).await;
    let (status, listed) = fx::call(
        &app,
        "GET",
        &format!("/api/v1/workspaces/{}/invites", f.canonical),
        Some(&f.tokens["owner_a"]),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed.as_array().map(Vec::len), Some(1), "{listed}");
    let (status, info) =
        fx::call(&app, "GET", &format!("/api/v1/invites/{token}"), None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(info["user_exists"], json!(true), "{info}");
    let invitee_token = fx::token(&state, invitee, UserRole::Trader).await;
    let (status, accepted) = fx::call(
        &app,
        "POST",
        &format!("/api/v1/invites/{token}/accept"),
        Some(&invitee_token),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{accepted}");
    let role: String = sqlx::query_scalar(
        "SELECT role FROM workspace_members WHERE workspace_id = $1 AND user_id = $2",
    )
    .bind(f.canonical)
    .bind(invitee)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(role, "member");
    // A used invite cannot be accepted again.
    let (status, _) = fx::call(
        &app,
        "POST",
        &format!("/api/v1/invites/{token}/accept"),
        Some(&invitee_token),
        Some(json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::GONE);

    // A new account is created by accepting.
    let token = stored_invite(&pool, f.canonical, "new.person@example.test", owner).await;
    let (status, accepted) = fx::call(
        &app,
        "POST",
        &format!("/api/v1/invites/{token}/accept"),
        None,
        Some(json!({ "password": "long enough password" })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{accepted}");
    assert_eq!(accepted["is_new_user"], json!(true), "{accepted}");
}

async fn pending_invites_from(pool: &PgPool, inviter: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM workspace_invites WHERE invited_by = $1 AND accepted_at IS NULL",
    )
    .bind(inviter)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// An invite carries its sender's authority. A removed or demoted admin's
/// pending invites are withdrawn, and one that survives anyway (here, a role
/// changed outside the handlers) cannot be accepted.
#[sqlx::test(migrations = false)]
async fn an_invite_lapses_when_its_sender_can_no_longer_send_it(pool: PgPool) {
    let f = fixture(&pool).await;
    let state = AppState::for_tests(pool.clone());
    let invites = Router::new()
        .route("/api/v1/invites/:token", get(get_invite_info))
        .route(
            "/api/v1/invites/:token/accept",
            axum::routing::post(accept_invite),
        )
        .with_state(state.clone());
    let accept = |token: String, invitee_token: String| {
        let invites = invites.clone();
        async move {
            fx::call(
                &invites,
                "POST",
                &format!("/api/v1/invites/{token}/accept"),
                Some(&invitee_token),
                Some(json!({})),
            )
            .await
        }
    };
    let invitee = fx::user(&pool, UserRole::Trader).await;
    let invitee_token = fx::token(&state, invitee, UserRole::Trader).await;
    let invitee_email = format!("{invitee}@example.test");
    let is_member = |user: Uuid| {
        let pool = pool.clone();
        let workspace = f.canonical;
        async move {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM workspace_members WHERE workspace_id = $1 AND user_id = $2)",
            )
            .bind(workspace)
            .bind(user)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };

    // Demoted outside the handlers: the invite is still stored but lapsed.
    let token = stored_invite(&pool, f.canonical, &invitee_email, f.user_ids["admin_a2"]).await;
    sqlx::query("UPDATE workspace_members SET role = 'member' WHERE user_id = $1")
        .bind(f.user_ids["admin_a2"])
        .execute(&pool)
        .await
        .unwrap();
    let (status, body) = fx::call(
        &invites,
        "GET",
        &format!("/api/v1/invites/{token}"),
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::GONE, "{body}");
    let (status, body) = accept(token, invitee_token.clone()).await;
    assert_eq!(status, StatusCode::GONE, "{body}");
    assert!(!is_member(invitee).await);

    // An admin invite needs the owner: one stored from an admin is refused.
    let token = stored_invite_for(
        &pool,
        f.canonical,
        &invitee_email,
        f.user_ids["admin_a"],
        "admin",
    )
    .await;
    let (status, body) = accept(token, invitee_token.clone()).await;
    assert_eq!(status, StatusCode::GONE, "{body}");
    assert!(!is_member(invitee).await);
    sqlx::query("DELETE FROM workspace_invites")
        .execute(&pool)
        .await
        .unwrap();

    // The owner demotes an admin: the admin's pending invites are withdrawn.
    stored_invite(
        &pool,
        f.canonical,
        "one@example.test",
        f.user_ids["admin_a"],
    )
    .await;
    stored_invite(
        &pool,
        f.canonical,
        "two@example.test",
        f.user_ids["admin_a"],
    )
    .await;
    let owner_invite =
        stored_invite(&pool, f.canonical, &invitee_email, f.user_ids["owner_a"]).await;
    assert_eq!(pending_invites_from(&pool, f.user_ids["admin_a"]).await, 2);
    assert_eq!(
        f.set_role("owner_a", "admin_a", "member").await,
        StatusCode::OK
    );
    assert_eq!(pending_invites_from(&pool, f.user_ids["admin_a"]).await, 0);

    // The owner removes a member who sent invites while an admin: withdrawn too.
    sqlx::query("UPDATE workspace_members SET role = 'admin' WHERE user_id = $1")
        .bind(f.user_ids["admin_a"])
        .execute(&pool)
        .await
        .unwrap();
    stored_invite(
        &pool,
        f.canonical,
        "three@example.test",
        f.user_ids["admin_a"],
    )
    .await;
    let uri = format!(
        "/api/v1/workspaces/{}/members/{}",
        f.canonical, f.user_ids["admin_a"]
    );
    let (status, _) = fx::call(&f.app, "DELETE", &uri, Some(&f.tokens["owner_a"]), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(pending_invites_from(&pool, f.user_ids["admin_a"]).await, 0);

    // The owner's own invite is untouched and still works.
    let (status, body) = accept(owner_invite, invitee_token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(is_member(invitee).await);
}
