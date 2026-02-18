use sqlx::PgPool;
use tracing::warn;

/// Returns true when wallet_features.strategy_type exists in the active schema.
pub async fn wallet_features_has_strategy_type(pool: &PgPool) -> bool {
    sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM pg_attribute
            WHERE attrelid = 'wallet_features'::regclass
              AND attname = 'strategy_type'
              AND NOT attisdropped
        )
        "#,
    )
    .fetch_one(pool)
    .await
    .unwrap_or_else(|error| {
        warn!(
            error = %error,
            "Failed to inspect wallet_features.strategy_type; assuming missing"
        );
        false
    })
}
