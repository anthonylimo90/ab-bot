use anyhow::{anyhow, Context, Result};
use api_server::dynamic_tuner::{channels, DynamicConfigUpdate};
use chrono::Utc;
use redis::AsyncCommands;
use rust_decimal::Decimal;
use serde_json::json;
use sqlx::Row;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();

    let mut args = std::env::args().skip(1);
    let key = args
        .next()
        .ok_or_else(|| anyhow!("usage: cargo run -p api-server --example set_dynamic_config -- <KEY> <CURRENT_VALUE> [DEFAULT_VALUE] [REASON]"))?;
    let current_value: Decimal = args
        .next()
        .ok_or_else(|| anyhow!("missing CURRENT_VALUE"))?
        .parse()
        .context("invalid CURRENT_VALUE")?;
    let default_value = args
        .next()
        .map(|value| value.parse::<Decimal>().context("invalid DEFAULT_VALUE"))
        .transpose()?;
    let reason = args
        .next()
        .unwrap_or_else(|| format!("manual correction: {key}={current_value}"));

    let database_url =
        std::env::var("DATABASE_URL").context("DATABASE_URL must be set in the environment")?;
    let redis_url = std::env::var("DYNAMIC_TUNER_REDIS_URL")
        .or_else(|_| std::env::var("REDIS_URL"))
        .context("REDIS_URL or DYNAMIC_TUNER_REDIS_URL must be set in the environment")?;

    let pool = sqlx::PgPool::connect(&database_url)
        .await
        .context("failed connecting to postgres")?;
    let existing = sqlx::query(
        r#"
        SELECT current_value, default_value, min_value, max_value, max_step_pct
        FROM dynamic_config
        WHERE key = $1
        "#,
    )
    .bind(&key)
    .fetch_optional(&pool)
    .await
    .context("failed loading dynamic_config row")?
    .ok_or_else(|| anyhow!("dynamic_config row for {key} does not exist"))?;

    let old_value: Decimal = existing.get("current_value");
    let old_default: Decimal = existing.get("default_value");
    let min_value: Decimal = existing.get("min_value");
    let max_value: Decimal = existing.get("max_value");
    let max_step_pct: Decimal = existing.get("max_step_pct");

    let clamped_current = current_value.max(min_value).min(max_value);
    let clamped_default = default_value
        .unwrap_or(old_default)
        .max(min_value)
        .min(max_value);

    sqlx::query(
        r#"
        UPDATE dynamic_config
        SET current_value = $2,
            default_value = $3,
            min_value = $4,
            max_value = $5,
            max_step_pct = $6,
            last_good_value = $2,
            pending_eval = FALSE,
            pending_baseline = NULL,
            last_applied_at = NULL,
            updated_by = 'workspace_manual',
            last_reason = $7
        WHERE key = $1
        "#,
    )
    .bind(&key)
    .bind(clamped_current)
    .bind(clamped_default)
    .bind(min_value)
    .bind(max_value)
    .bind(max_step_pct)
    .bind(&reason)
    .execute(&pool)
    .await
    .context("failed updating dynamic_config row")?;

    sqlx::query(
        r#"
        INSERT INTO dynamic_config_history
            (config_key, old_value, new_value, action, reason)
        VALUES ($1, $2, $3, 'manual_update', $4)
        "#,
    )
    .bind(&key)
    .bind(old_value)
    .bind(clamped_current)
    .bind(&reason)
    .execute(&pool)
    .await
    .context("failed inserting dynamic_config_history row")?;

    let redis_client = redis::Client::open(redis_url).context("failed opening redis client")?;
    let mut redis = redis::aio::ConnectionManager::new(redis_client)
        .await
        .context("failed connecting to redis")?;
    let payload = serde_json::to_string(&DynamicConfigUpdate {
        key: key.clone(),
        value: clamped_current,
        reason: reason.clone(),
        source: "workspace_manual".to_string(),
        timestamp: Utc::now(),
        metrics: json!({
            "manual": true,
            "old_value": old_value.to_string(),
            "old_default_value": old_default.to_string(),
            "new_default_value": clamped_default.to_string(),
        }),
    })?;
    let _: () = redis
        .publish(channels::CONFIG_UPDATES, payload)
        .await
        .context("failed publishing runtime update")?;

    println!(
        "updated key={key} current_value={} default_value={} old_value={} old_default_value={}",
        clamped_current, clamped_default, old_value, old_default
    );

    Ok(())
}
