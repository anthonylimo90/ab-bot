//! Shared market catalog for executors and API reads.

use polymarket_core::api::{ClobClient, GammaClient};
use polymarket_core::types::Market;
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, warn};
mod storage;
pub(crate) use storage::catalog_observed_at;
/// Infer category from market question.
pub(crate) fn infer_category(question: &str) -> String {
    let q = question.to_lowercase();

    if q.contains("bitcoin") || q.contains("btc") || q.contains("ethereum") || q.contains("crypto")
    {
        "crypto".to_string()
    } else if q.contains("president")
        || q.contains("election")
        || q.contains("congress")
        || q.contains("senate")
    {
        "politics".to_string()
    } else if q.contains("nfl")
        || q.contains("nba")
        || q.contains("world cup")
        || q.contains("super bowl")
    {
        "sports".to_string()
    } else if q.contains("stock") || q.contains("s&p") || q.contains("nasdaq") || q.contains("fed")
    {
        "finance".to_string()
    } else if q.contains("ai")
        || q.contains("openai")
        || q.contains("google")
        || q.contains("apple")
    {
        "tech".to_string()
    } else {
        "other".to_string()
    }
}

pub(crate) use polymarket_core::types::yes_no_indices;

/// YES and NO prices of a binary market under [`yes_no_indices`]; `None`
/// for a price the source did not report (never a made-up 0.50).
pub(crate) fn extract_prices(
    outcomes: &[polymarket_core::types::Outcome],
) -> (Option<Decimal>, Option<Decimal>) {
    match yes_no_indices(outcomes) {
        Some((yes, no)) => (outcomes[yes].price, outcomes[no].price),
        None => (None, None),
    }
}

/// Cached mapping of market_id → (yes_token_id, no_token_id).
pub struct OutcomeTokenCache {
    clob_client: Arc<ClobClient>,
    gamma_client: GammaClient,
    tokens: RwLock<HashMap<String, (String, String)>>,
    /// Shared set of the market (and token) IDs a new position may enter:
    /// not resolved and taking orders at the last refresh or single read.
    active_clob_markets: Arc<RwLock<HashSet<String>>>,
    /// Database pool for persisting token→condition_id mappings.
    pool: Option<PgPool>,
    /// Serialize cache mutations to avoid overlapping upserts deadlocking.
    refresh_lock: Mutex<()>,
    last_refresh: RwLock<Option<Instant>>,
    hydration_lock: Mutex<()>,
    publish_lock: Mutex<()>,
    recent_hydrations: Mutex<HashMap<String, (Market, Instant)>>,
}

impl OutcomeTokenCache {
    pub(crate) fn new(
        clob_client: Arc<ClobClient>,
        active_clob_markets: Arc<RwLock<HashSet<String>>>,
    ) -> Self {
        Self {
            clob_client,
            gamma_client: GammaClient::new(None),
            tokens: RwLock::new(HashMap::new()),
            active_clob_markets,
            pool: None,
            refresh_lock: Mutex::new(()),
            last_refresh: RwLock::new(None),
            hydration_lock: Mutex::new(()),
            publish_lock: Mutex::new(()),
            recent_hydrations: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn with_pool(mut self, pool: PgPool) -> Self {
        self.pool = Some(pool);
        self
    }

    /// Refresh the cache using Gamma as the source of tradable markets.
    pub(crate) async fn refresh_if_stale(
        &self,
        max_age: Duration,
    ) -> anyhow::Result<(usize, usize)> {
        let _guard = self.refresh_lock.lock().await;
        if self
            .last_refresh
            .read()
            .await
            .is_some_and(|time| time.elapsed() < max_age)
        {
            return Ok((
                self.len().await,
                self.active_clob_markets.read().await.len(),
            ));
        }
        let started = Instant::now();
        let gamma_page_size =
            polymarket_core::config::env_count::<u32>("GAMMA_ARB_MARKET_PAGE_SIZE", 200);
        let source = self.gamma_client.get_all_markets(gamma_page_size).await?;
        // Reject an unexpectedly empty upstream snapshot; retain known data.
        anyhow::ensure!(!source.is_empty(), "Gamma returned an empty market catalog");
        let mut tradable_ids = HashSet::new();
        let mut unique = std::collections::BTreeMap::new();
        for raw in source {
            let tradable = raw.is_tradable();
            if let Some(market) = raw.into_market() {
                if tradable {
                    tradable_ids.insert(market.id.clone());
                }
                unique.insert(market.id.clone(), market);
            }
        }
        let _publish = self.publish_lock.lock().await;
        // Targeted reads may complete while the slow full catalog is in flight.
        // Keep those newer observations rather than overwriting them with old pages.
        {
            let mut recent = self.recent_hydrations.lock().await;
            recent.retain(|_, (_, observed)| *observed >= started);
            for (id, (market, _)) in recent.iter() {
                if market.is_tradable() {
                    tradable_ids.insert(id.clone());
                } else {
                    tradable_ids.remove(id);
                }
                unique.insert(id.clone(), market.clone());
            }
        }
        let markets: Vec<Market> = unique.into_values().collect();
        anyhow::ensure!(
            !markets.is_empty(),
            "Gamma catalog contained no usable binary markets"
        );
        if let Some(pool) = &self.pool {
            storage::persist_markets(pool, &markets, true).await?;
        }
        let mut map = HashMap::new();
        let mut active_set = HashSet::new();
        // Collect token_id → condition_id mappings for DB cache
        let mut token_condition_pairs: Vec<(String, String)> = Vec::new();

        for market in &markets {
            if tradable_ids.contains(&market.id) {
                active_set.insert(market.id.clone());
                // Also insert outcome token IDs so trades whose market_id
                // fell back to asset_id (when condition_id is absent) still
                // match the active set.
                for outcome in &market.outcomes {
                    active_set.insert(outcome.token_id.clone());
                    // Map token_id → condition_id (market.id) for the DB cache
                    token_condition_pairs.push((outcome.token_id.clone(), market.id.clone()));
                }
            }
            // Tokens are kept for every listed binary market, tradable or
            // not: exits and resolution need them. Entries check the active
            // set above.
            if let Some((yes, no)) = yes_no_indices(&market.outcomes) {
                map.insert(
                    market.id.clone(),
                    (
                        market.outcomes[yes].token_id.clone(),
                        market.outcomes[no].token_id.clone(),
                    ),
                );
            }
        }

        let count = map.len();
        let active_set_size = active_set.len();
        // Hold both guards until publication is complete.
        let mut tokens = self.tokens.write().await;
        let mut active = self.active_clob_markets.write().await;
        *tokens = map;
        *active = active_set;
        *self.last_refresh.write().await = Some(Instant::now());
        drop(active);
        drop(tokens);

        // Deduplicate by token_id (same token can appear if cursor pages overlap)
        let deduped: std::collections::HashMap<&str, &str> = token_condition_pairs
            .iter()
            .map(|(t, c)| (t.as_str(), c.as_str()))
            .collect();
        let mut token_condition_pairs: Vec<(String, String)> = deduped
            .into_iter()
            .map(|(t, c)| (t.to_owned(), c.to_owned()))
            .collect();
        token_condition_pairs.sort_by(|a, b| a.0.cmp(&b.0));

        // Batch UPSERT token→condition_id mappings to DB for resolution checks
        if let Some(ref pool) = self.pool {
            if let Err(e) = Self::upsert_token_condition_cache(pool, &token_condition_pairs).await {
                warn!(error = %e, "Failed to update token_condition_cache");
            } else {
                debug!(
                    pairs = token_condition_pairs.len(),
                    "Updated token_condition_cache"
                );
            }
        }

        Ok((count, active_set_size))
    }

    /// Hydrate a single market into the cache without forcing a full-market refresh.
    pub(crate) async fn hydrate_market(
        &self,
        market_id: &str,
    ) -> anyhow::Result<Option<(String, String)>> {
        if let Some(ids) = self.get(market_id).await {
            return Ok(Some(ids));
        }

        let _guard = self.hydration_lock.lock().await;
        if let Some(ids) = self.get(market_id).await {
            return Ok(Some(ids));
        }
        self.fetch_market(market_id).await
    }

    /// Refresh one stale token mapping without downloading the market universe.
    pub(crate) async fn refresh_market(
        &self,
        market_id: &str,
    ) -> anyhow::Result<Option<(String, String)>> {
        let _guard = self.hydration_lock.lock().await;
        self.fetch_market(market_id).await
    }

    async fn fetch_market(&self, market_id: &str) -> anyhow::Result<Option<(String, String)>> {
        let market = match self.clob_client.get_market_by_id(market_id).await {
            Ok(market) => market,
            Err(e) => {
                warn!(market_id = %market_id, error = %e, "Failed to fetch single market for token cache hydration");
                return Ok(None);
            }
        };

        let _publish = self.publish_lock.lock().await;
        if let Some(pool) = &self.pool {
            storage::persist_markets(pool, std::slice::from_ref(&market), false).await?;
        }
        let result = self.apply_market_update(&market).await?;
        self.recent_hydrations
            .lock()
            .await
            .insert(market.id.clone(), (market, Instant::now()));
        Ok(result)
    }

    /// Whether a new position may enter the market: tradable at the last
    /// catalog refresh or single-market read. A market read since as resolved
    /// or not accepting orders is not, though its tokens stay cached.
    pub(crate) async fn is_tradable(&self, market_id: &str) -> bool {
        self.active_clob_markets.read().await.contains(market_id)
    }

    /// Number of cached market → token mappings.
    pub(crate) async fn len(&self) -> usize {
        self.tokens.read().await.len()
    }

    /// Insert an alias mapping (e.g., asset_id → same token pair).
    /// Used by exit_handler when a position's market_id is an asset_id.
    pub(crate) async fn alias(&self, alias: &str, ids: &(String, String)) {
        self.tokens
            .write()
            .await
            .insert(alias.to_string(), ids.clone());
    }

    async fn apply_market_update(
        &self,
        market: &Market,
    ) -> anyhow::Result<Option<(String, String)>> {
        let mut tokens = self.tokens.write().await;
        let mut active_markets = self.active_clob_markets.write().await;

        // The active set holds the markets a new position may enter; the
        // tokens below are kept either way (exits and resolution need them).
        if market.is_tradable() {
            active_markets.insert(market.id.clone());
            for outcome in &market.outcomes {
                active_markets.insert(outcome.token_id.clone());
            }
        } else {
            active_markets.remove(&market.id);
            for outcome in &market.outcomes {
                active_markets.remove(&outcome.token_id);
            }
        }

        let token_pair = if let Some((yes, no)) = yes_no_indices(&market.outcomes) {
            let (yes_id, no_id) = (
                market.outcomes[yes].token_id.clone(),
                market.outcomes[no].token_id.clone(),
            );
            tokens.insert(market.id.clone(), (yes_id.clone(), no_id.clone()));
            Some((yes_id, no_id))
        } else {
            tokens.remove(&market.id);
            None
        };
        drop(tokens);
        drop(active_markets);

        if let Some(ref pool) = self.pool {
            let mut token_condition_pairs: Vec<(String, String)> = market
                .outcomes
                .iter()
                .map(|outcome| (outcome.token_id.clone(), market.id.clone()))
                .collect();
            token_condition_pairs.sort_by(|a, b| a.0.cmp(&b.0));
            if let Err(e) = Self::upsert_token_condition_cache(pool, &token_condition_pairs).await {
                warn!(market_id = %market.id, error = %e, "Failed to update token_condition_cache during single-market hydration");
            } else {
                debug!(market_id = %market.id, pairs = token_condition_pairs.len(), "Hydrated token_condition_cache for single market");
            }
        }

        Ok(token_pair)
    }

    /// Batch UPSERT token→condition_id mappings into the database cache.
    async fn upsert_token_condition_cache(
        pool: &PgPool,
        pairs: &[(String, String)],
    ) -> anyhow::Result<()> {
        if pairs.is_empty() {
            return Ok(());
        }

        // Batch in smaller deterministic chunks to reduce row-lock contention.
        const BATCH_SIZE: usize = 250;
        for chunk in pairs.chunks(BATCH_SIZE) {
            let mut qb: sqlx::QueryBuilder<sqlx::Postgres> = sqlx::QueryBuilder::new(
                "INSERT INTO token_condition_cache (token_id, condition_id, updated_at) ",
            );
            qb.push_values(chunk, |mut b, (token_id, condition_id)| {
                b.push_bind(token_id)
                    .push_bind(condition_id)
                    .push_bind(chrono::Utc::now());
            });
            qb.push(
                " ON CONFLICT (token_id) DO UPDATE SET condition_id = EXCLUDED.condition_id, updated_at = EXCLUDED.updated_at WHERE token_condition_cache.condition_id IS DISTINCT FROM EXCLUDED.condition_id",
            );
            qb.build().execute(pool).await?;
        }

        Ok(())
    }

    /// Look up token IDs for a given market.
    pub(crate) async fn get(&self, market_id: &str) -> Option<(String, String)> {
        self.tokens.read().await.get(market_id).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::get, Json, Router};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn targeted_hydration_runs_during_full_download_and_survives_publication() {
        let entered = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        let signal = entered.clone();
        let wait = resume.clone();
        let app = Router::new()
            .route(
                "/markets",
                get(move || {
                    let signal = signal.clone();
                    let wait = wait.clone();
                    async move {
                        signal.notify_one();
                        wait.notified().await;
                        Json(
                            serde_json::json!([{"conditionId":"initial","question":"Initial?",
                        "active":true,"acceptingOrders":true,"enableOrderBook":true,
                        "outcomes":"[\"Yes\",\"No\"]","clobTokenIds":"[\"1\",\"2\"]"}]),
                        )
                    }
                }),
            )
            .route(
                "/markets/late",
                get(|| async {
                    Json(
                        serde_json::json!({"condition_id":"late","question":"Late?","active":true,
                    "accepting_orders":true,"enable_order_book":true,
                    "tokens":[{"token_id":"3","outcome":"Yes"},{"token_id":"4","outcome":"No"}]}),
                    )
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut catalog = OutcomeTokenCache::new(
            Arc::new(ClobClient::new(Some(url.clone()), None)),
            Arc::new(RwLock::new(HashSet::new())),
        );
        catalog.gamma_client = GammaClient::new(Some(url));
        let catalog = Arc::new(catalog);
        let worker = catalog.clone();
        let full =
            tokio::spawn(async move { worker.refresh_if_stale(Duration::from_secs(300)).await });
        entered.notified().await;
        let ids = tokio::time::timeout(Duration::from_secs(2), catalog.hydrate_market("late"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ids, Some(("3".into(), "4".into())));
        resume.notify_one();
        full.await.unwrap().unwrap();
        assert_eq!(catalog.get("late").await, ids);
        assert!(catalog.get("initial").await.is_some());
        server.abort();
    }

    #[tokio::test]
    async fn concurrent_consumers_share_refresh_and_hydration() {
        let requests = Arc::new(AtomicUsize::new(0));
        let count = requests.clone();
        let app = Router::new().route(
            "/markets",
            get(move || {
                let count = count.clone();
                async move {
                    count.fetch_add(1, Ordering::SeqCst);
                    Json(serde_json::json!([{
                        "conditionId":"shared", "question":"Shared market?",
                        "active":true,"closed":false,"acceptingOrders":true,
                        "enableOrderBook":true,"outcomes":"[\"Yes\",\"No\"]",
                        "clobTokenIds":"[\"yes-token\",\"no-token\"]"
                    }]))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let active = Arc::new(RwLock::new(HashSet::new()));
        let mut catalog =
            OutcomeTokenCache::new(Arc::new(ClobClient::new(None, None)), active.clone());
        catalog.gamma_client = GammaClient::new(Some(url));
        let age = Duration::from_secs(300);
        let (a, b) = tokio::join!(catalog.refresh_if_stale(age), catalog.refresh_if_stale(age));
        assert_eq!(a.unwrap(), (1, 3));
        assert_eq!(b.unwrap(), (1, 3));
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        assert_eq!(
            catalog.hydrate_market("shared").await.unwrap(),
            Some(("yes-token".into(), "no-token".into()))
        );
        assert!(active.read().await.contains("yes-token"));
        // A failed refresh preserves the last complete snapshot and permits retry.
        server.abort();
        catalog.gamma_client = GammaClient::new(Some("http://127.0.0.1:0".into()));
        assert!(catalog.refresh_if_stale(Duration::ZERO).await.is_err());
        assert!(catalog.get("shared").await.is_some());
    }

    #[tokio::test]
    async fn a_full_refresh_keeps_a_halted_markets_tokens_out_of_the_active_set() {
        let app = Router::new().route(
            "/markets",
            get(|| async {
                Json(serde_json::json!([
                    {"conditionId":"open","question":"Open?","active":true,
                     "acceptingOrders":true,"enableOrderBook":true,
                     "outcomes":"[\"Yes\",\"No\"]","clobTokenIds":"[\"1\",\"2\"]"},
                    {"conditionId":"halted","question":"Halted?","active":true,
                     "acceptingOrders":false,"enableOrderBook":true,
                     "outcomes":"[\"Yes\",\"No\"]","clobTokenIds":"[\"3\",\"4\"]"}
                ]))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut catalog = OutcomeTokenCache::new(
            Arc::new(ClobClient::new(None, None)),
            Arc::new(RwLock::new(HashSet::new())),
        );
        catalog.gamma_client = GammaClient::new(Some(url));
        catalog
            .refresh_if_stale(Duration::from_secs(300))
            .await
            .unwrap();
        assert!(catalog.is_tradable("open").await);
        assert!(!catalog.is_tradable("halted").await);
        // Exits find the halted market's tokens without another exchange read.
        assert_eq!(catalog.get("halted").await, Some(("3".into(), "4".into())));
        server.abort();
    }

    #[tokio::test]
    async fn a_halted_market_keeps_its_tokens_but_takes_no_entry() {
        let accepting = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = accepting.clone();
        let app = Router::new().route(
            "/markets/halted",
            get(move || {
                let accepting = flag.load(Ordering::SeqCst);
                async move {
                    Json(serde_json::json!({
                        "condition_id":"halted","question":"Halted?","active":true,
                        "closed":false,"enable_order_book":true,
                        "accepting_orders":accepting,
                        "tokens":[{"token_id":"5","outcome":"Yes"},{"token_id":"6","outcome":"No"}]
                    }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let catalog = OutcomeTokenCache::new(
            Arc::new(ClobClient::new(Some(url), None)),
            Arc::new(RwLock::new(HashSet::new())),
        );

        // Exits still find the tokens; entries see a market that takes no orders.
        assert_eq!(
            catalog.hydrate_market("halted").await.unwrap(),
            Some(("5".into(), "6".into()))
        );
        assert!(!catalog.is_tradable("halted").await);

        // Once the exchange takes orders again, a fresh read admits entries.
        accepting.store(true, Ordering::SeqCst);
        catalog.refresh_market("halted").await.unwrap();
        assert!(catalog.is_tradable("halted").await);

        // And a halt read later withdraws it again.
        accepting.store(false, Ordering::SeqCst);
        catalog.refresh_market("halted").await.unwrap();
        assert!(!catalog.is_tradable("halted").await);
        assert!(catalog.get("halted").await.is_some());
        server.abort();
    }
}
