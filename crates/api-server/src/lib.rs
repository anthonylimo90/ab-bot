//! API Server
//!
//! REST and WebSocket API for the Polymarket trading platform.
//!
//! # Features
//!
//! - **REST API**: CRUD operations for positions, wallets, strategies
//! - **WebSocket**: Real-time orderbook updates, position changes, signals
//! - **OpenAPI**: Auto-generated Swagger documentation
//! - **Authentication**: JWT and API key support
//!
//! # Example
//!
//! ```ignore
//! use api_server::{ApiServer, ServerConfig};
//!
//! let config = ServerConfig::default();
//! let server = ApiServer::new(config, pool).await?;
//! server.run().await?;
//! ```

pub mod accounting_ledger;
pub mod arb_executor;
mod background;
pub mod backtest_automation;
pub mod collateral;
pub mod crypto;
pub mod dynamic_tuner;
pub mod email;
pub mod error;
pub mod exit_handler;
pub mod fee_terms;
pub mod flow_feature_calculator;
pub mod gamma_syncer;
pub mod handlers;
mod intent_reconciler;
pub mod learning;
pub mod learning_evaluator;
pub mod learning_models;
pub mod learning_rollouts;
pub mod market_catalog;
pub mod metrics_calculator;
pub mod middleware;
#[cfg(test)]
mod migration_tests;
pub mod position_reconciler;
pub mod position_service;
pub mod quant_signal_executor;
pub mod redis_forwarder;
pub mod routes;
pub mod runtime_sync;
#[cfg(test)]
mod security_http_tests;
pub mod signals;
pub mod state;
pub mod strategy_health_calculator;
pub mod strategy_modes;
pub mod strategy_pnl_calculator;
pub mod trade_events;
pub mod trading_leader;
pub mod wallet_harvester;
pub mod wallet_inventory;
pub mod websocket;
pub mod withdrawals;
pub mod workspace_scope;

pub use accounting_ledger::{spawn_account_snapshot_calculator, AccountSnapshotConfig};
pub use arb_executor::{spawn_arb_auto_executor, ArbExecutorConfig};
pub use backtest_automation::{spawn_backtest_automation, BacktestAutomationConfig};
pub use dynamic_tuner::{spawn_dynamic_config_subscriber, DynamicTuner};
pub use error::ApiError;
use exit_handler::spawn_exit_handler;
pub use exit_handler::ExitHandlerConfig;
pub use flow_feature_calculator::{spawn_flow_feature_calculator, FlowFeatureConfig};
pub use gamma_syncer::{spawn_gamma_syncer, GammaSyncerConfig};
pub use learning_evaluator::{spawn_learning_evaluator, LearningEvaluatorConfig};
pub use learning_rollouts::{
    spawn_learning_rollout_observer, LearningRolloutController, LearningRolloutObserverConfig,
};
pub use metrics_calculator::{MetricsCalculator, MetricsCalculatorConfig};
pub use position_reconciler::{spawn_position_reconciler, PositionReconcilerConfig};
pub use quant_signal_executor::{spawn_quant_signal_executor, QuantSignalExecutorConfig};
pub use redis_forwarder::{spawn_redis_forwarder, RedisForwarderConfig};
pub use routes::create_router;
pub use signals::{
    spawn_cross_market_signal_generator, spawn_flow_signal_generator,
    spawn_mean_reversion_signal_generator, spawn_resolution_signal_generator,
};
pub use state::AppState;
pub use strategy_health_calculator::{spawn_strategy_health_calculator, StrategyHealthConfig};
pub use strategy_pnl_calculator::{spawn_strategy_pnl_calculator, StrategyPnlConfig};
pub use wallet_harvester::{spawn_wallet_harvester, WalletHarvesterConfig};
pub use wallet_inventory::{spawn_wallet_inventory_reconciler, WalletInventoryConfig};

use axum::extract::DefaultBodyLimit;
use axum::http::Request;
use sqlx::PgPool;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock, Semaphore};
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::{DefaultOnResponse, TraceLayer};
use tracing::{info, Level};

/// Server configuration.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Host to bind to.
    pub host: String,
    /// Port to listen on.
    pub port: u16,
    /// Enable CORS for all origins (development only).
    pub cors_permissive: bool,
    /// Exact origins allowed by CORS (`CORS_ALLOWED_ORIGINS`, comma-separated).
    /// When non-empty it takes precedence over `cors_permissive`.
    pub cors_allowed_origins: Vec<String>,
    /// JWT secret for authentication.
    pub jwt_secret: String,
    /// WebSocket channel capacity.
    pub ws_channel_capacity: usize,
    /// Settings that cannot be read ([`polymarket_core::config::unreadable_settings`]).
    /// The trading leader trips the circuit breaker for them: new entries
    /// halt, exits run.
    pub unreadable_settings: Vec<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 3000,
            cors_permissive: true,
            cors_allowed_origins: Vec::new(),
            jwt_secret: "development-secret-change-in-production".to_string(),
            ws_channel_capacity: 1000,
            unreadable_settings: Vec::new(),
        }
    }
}

/// Split a comma-separated origin list, trimming blanks and trailing slashes
/// (an Origin header never has a path).
/// The route a request matched, as its template (`/invites/{token}`), for
/// logs: the request's own path and query can carry secrets.
fn route_template<B>(request: &Request<B>) -> String {
    request
        .extensions()
        .get::<axum::extract::MatchedPath>()
        .map_or_else(|| "unmatched".to_string(), |path| path.as_str().to_string())
}

fn parse_origin_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(|origin| origin.trim().trim_end_matches('/'))
        .filter(|origin| !origin.is_empty())
        .map(str::to_string)
        .collect()
}

/// Whether `ENVIRONMENT` names a local development setup. Anything else,
/// including unset, is treated as a real deployment.
pub fn is_development_environment(environment: &str) -> bool {
    matches!(
        environment.trim().to_ascii_lowercase().as_str(),
        "development" | "dev" | "local" | "test"
    )
}

/// Build the CORS layer.
///
/// - `cors_allowed_origins` set: only those exact origins, the methods and
///   headers the dashboard uses, and a 10 minute preflight cache.
/// - otherwise `cors_permissive`: any origin (development).
/// - otherwise: any origin as well, for backward compatibility, which makes
///   `CORS_PERMISSIVE=false` a no-op; a startup warning says so.
///
/// Every origin outside development logs an error at startup.
fn build_cors_layer(config: &ServerConfig) -> CorsLayer {
    use axum::http::{header, HeaderValue, Method};

    if !config.cors_allowed_origins.is_empty() {
        let origins: Vec<HeaderValue> = config
            .cors_allowed_origins
            .iter()
            .filter_map(|origin| {
                if origin == "*" {
                    tracing::warn!("CORS_ALLOWED_ORIGINS entry \"*\" ignored: list exact origins");
                    return None;
                }
                match HeaderValue::from_str(origin) {
                    Ok(value) => Some(value),
                    Err(_) => {
                        tracing::warn!(origin = %origin, "Ignoring invalid CORS_ALLOWED_ORIGINS entry");
                        None
                    }
                }
            })
            .collect();
        if origins.is_empty() {
            tracing::error!(
                "CORS_ALLOWED_ORIGINS contains no usable origin; cross-origin requests will be refused"
            );
        } else {
            info!(origins = ?config.cors_allowed_origins, "CORS restricted to configured origins");
        }
        return CorsLayer::new()
            .allow_origin(origins)
            .allow_methods([
                Method::GET,
                Method::POST,
                Method::PUT,
                Method::PATCH,
                Method::DELETE,
                Method::OPTIONS,
            ])
            .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE])
            .max_age(std::time::Duration::from_secs(600));
    }

    // Every origin from here on. Browsers do not attach the bearer token on
    // their own, so this is no CSRF hole, but a deployment should name its
    // dashboard.
    if !is_development_environment(&std::env::var("ENVIRONMENT").unwrap_or_default()) {
        tracing::error!(
            "CORS allows every origin outside development: set CORS_ALLOWED_ORIGINS to the \
             dashboard's origin(s)"
        );
    }

    if config.cors_permissive {
        return CorsLayer::permissive();
    }

    tracing::warn!(
        "CORS_PERMISSIVE=false but CORS_ALLOWED_ORIGINS is not set: all origins are still \
         allowed. Set CORS_ALLOWED_ORIGINS to a comma-separated list of exact origins to restrict access."
    );
    CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any)
}

impl ServerConfig {
    /// Create from environment variables.
    pub fn from_env() -> Self {
        Self {
            host: std::env::var("API_HOST").unwrap_or_else(|_| "0.0.0.0".to_string()),
            // Check PORT first (Railway), then API_PORT, then default to 3000
            port: polymarket_core::config::env_count_opt("PORT")
                .or_else(|| polymarket_core::config::env_count_opt("API_PORT"))
                .unwrap_or(3000),
            cors_permissive: polymarket_core::config::env_flag("CORS_PERMISSIVE", true),
            cors_allowed_origins: std::env::var("CORS_ALLOWED_ORIGINS")
                .map(|raw| parse_origin_list(&raw))
                .unwrap_or_default(),
            jwt_secret: std::env::var("JWT_SECRET")
                .unwrap_or_else(|_| "development-secret-change-in-production".to_string()),
            ws_channel_capacity: polymarket_core::config::env_count("WS_CHANNEL_CAPACITY", 1000),
            unreadable_settings: polymarket_core::config::unreadable_settings(),
        }
    }

    /// Get the socket address.
    pub fn socket_addr(&self) -> SocketAddr {
        format!("{}:{}", self.host, self.port)
            .parse()
            .expect("Invalid socket address")
    }
}

/// The API server.
pub struct ApiServer {
    config: ServerConfig,
    state: AppState,
}

impl ApiServer {
    /// Create a new API server.
    pub async fn new(config: ServerConfig, pool: PgPool) -> anyhow::Result<Self> {
        // Create broadcast channels for WebSocket and automation
        let (orderbook_tx, _) = broadcast::channel(config.ws_channel_capacity);
        let (position_tx, _) = broadcast::channel(config.ws_channel_capacity);
        let (signal_tx, _) = broadcast::channel(config.ws_channel_capacity);
        let (trade_event_tx, _) = broadcast::channel(config.ws_channel_capacity);
        let (arb_entry_tx, _) = broadcast::channel(config.ws_channel_capacity);
        let (quant_signal_tx, _) = broadcast::channel(config.ws_channel_capacity);

        // Create app state
        let state = AppState::new(
            pool,
            config.jwt_secret.clone(),
            orderbook_tx,
            position_tx,
            signal_tx,
            trade_event_tx,
            arb_entry_tx,
            quant_signal_tx,
        )
        .await?;

        Ok(Self { config, state })
    }

    /// Run the server.
    pub async fn run(self) -> anyhow::Result<()> {
        // Listen for stop signals before anything trades: one arriving while
        // the executors start must still lead to a drain.
        let stop = polymarket_core::shutdown::StopSignals::install()?;
        self.run_until(stop).await
    }

    /// Run the server until `stop` receives a stop signal, then drain. A
    /// caller that listened while starting hands its listener over, so a
    /// signal it already received is not lost.
    pub async fn run_until(
        mut self,
        mut stop: polymarket_core::shutdown::StopSignals,
    ) -> anyhow::Result<()> {
        // ── Pre-create arb executor config Arc (must happen before Arc-wrapping state) ──
        let mut arb_config_pre = ArbExecutorConfig::from_env();
        if !arb_config_pre.enabled {
            match crate::runtime_sync::canonical_workspace_arb_enabled(&self.state.pool).await {
                Ok(true) => {
                    arb_config_pre.enabled = true;
                    info!(
                        "Enabling arb auto-executor because the canonical workspace has arb_auto_execute=true"
                    );
                }
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "Failed to read workspace arb_auto_execute flags; falling back to env-only arb config"
                    );
                }
            }
        }
        let arb_config_arc = Arc::new(RwLock::new(arb_config_pre));
        self.state.arb_executor_config = Some(arb_config_arc.clone());

        // Pre-create exit handler config Arc (must happen before Arc-wrapping state)
        let mut exit_config_pre = ExitHandlerConfig::from_env();
        if !exit_config_pre.enabled {
            match crate::runtime_sync::canonical_workspace_exit_handler_enabled(&self.state.pool)
                .await
            {
                Ok(true) => {
                    exit_config_pre.enabled = true;
                    info!(
                        "Enabling exit handler because the canonical workspace has exit_handler_enabled=true"
                    );
                }
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "Failed to read workspace exit_handler_enabled flags; falling back to env-only exit config"
                    );
                }
            }
        }
        let exit_config_arc = Arc::new(RwLock::new(exit_config_pre));
        self.state.exit_handler_config = Some(exit_config_arc.clone());

        let quant_config = Arc::new(RwLock::new(QuantSignalExecutorConfig::from_env()));
        self.state.quant_executor_config = Some(quant_config.clone());
        // One refresh owner, at the faster legacy consumer's configured cadence.
        let refresh_secs = arb_config_arc
            .read()
            .await
            .cache_refresh_secs
            .min(quant_config.read().await.cache_refresh_secs)
            .max(1);
        arb_config_arc.write().await.cache_refresh_secs = refresh_secs;

        // ── Wrap state in Arc and build router ──
        let state = Arc::new(self.state);

        let (router, limiter_janitor) = routes::create_router_with_janitor(state.clone());
        // Prune per-client rate-limit state, which otherwise grows without bound.
        limiter_janitor.spawn();
        let router = router
            .layer(
                TraceLayer::new_for_http()
                    // The route template, never the request's own path or
                    // query: invite tokens and other secrets travel in them.
                    .make_span_with(|request: &Request<_>| {
                        tracing::debug_span!(
                            "request",
                            method = %request.method(),
                            route = route_template(request),
                        )
                    })
                    .on_request(|request: &Request<_>, _span: &tracing::Span| {
                        tracing::debug!(
                            method = %request.method(),
                            route = route_template(request),
                            "Incoming request"
                        );
                    })
                    .on_response(DefaultOnResponse::new().level(Level::DEBUG))
                    .on_failure(
                        |error: tower_http::classify::ServerErrorsFailureClass,
                         latency: std::time::Duration,
                         _span: &tracing::Span| {
                            tracing::error!(
                                error = %error,
                                latency_ms = latency.as_millis(),
                                "Request failed"
                            );
                        },
                    ),
            )
            .layer(DefaultBodyLimit::max(2 * 1024 * 1024)) // 2 MB
            .layer(build_cors_layer(&self.config));

        // ── Spawn background tasks ──

        // Shared semaphore to coordinate heavy background DB tasks.
        // 3 permits ensures at most 3 heavy batch operations run simultaneously,
        // leaving ~17 connections available for HTTP handlers + lighter tasks.
        let db_semaphore = Arc::new(Semaphore::new(3));
        info!("DB backpressure semaphore created (3 permits)");

        // Spawn Redis forwarder for signal bridging
        let redis_config = RedisForwarderConfig::from_env();
        spawn_redis_forwarder(
            redis_config,
            state.signal_tx.clone(),
            state.orderbook_tx.clone(),
            state.arb_entry_tx.clone(),
        );

        // Stored overrides apply before any executor trades, whether or not
        // Redis (which carries later updates) is reachable.
        crate::dynamic_tuner::clear_untouched_seeds_when_disabled(&state.pool).await;
        crate::dynamic_tuner::apply_dynamic_config_snapshot(
            &state.pool,
            Some(&arb_config_arc),
            Some(&quant_config),
        )
        .await;

        // The exit handler's config, shared with the entry executors: a live
        // entry waits while the handler is off or not running.
        let exit_handler_config = state
            .exit_handler_config
            .clone()
            .unwrap_or_else(|| Arc::new(RwLock::new(ExitHandlerConfig::from_env())));
        let exit_readiness = crate::exit_handler::ExitReadiness::new(
            exit_handler_config.clone(),
            state.exit_handler_heartbeat.clone(),
        )
        .watching_exits(state.pool.clone());

        // Unreadable settings hold entries in every process, leading or not.
        hold_for_unreadable_settings(&state, &self.config.unreadable_settings);

        // Every process takes up what operators change through any of them:
        // the leader for its trading, another for the checks its own
        // requests make (manual orders, breaker status).
        spawn_operator_sync(state.clone());
        // And loads the trading signer while live trading is on without one.
        crate::state::spawn_signer_keeper(state.clone());

        // ── Trading: run by the trading leader only ──
        // Another API process (the next version, while a deploy overlaps the
        // two) serves requests and waits; it starts trading once this one
        // lets go of the lock, on the state this one last stored.
        let leadership_lost = Arc::new(tokio::sync::Notify::new());
        {
            let state = state.clone();
            let lost = leadership_lost.clone();
            let arb_config_arc = arb_config_arc.clone();
            let quant_config = quant_config.clone();
            let unreadable_settings = self.config.unreadable_settings.clone();
            tokio::spawn(async move {
                let executor = state.order_executor.clone();
                let Some(leadership) =
                    trading_leader::acquire(&state.pool, || executor.is_draining()).await
                else {
                    return;
                };
                if state.order_executor.is_draining() {
                    // Shutting down: let the next process lead.
                    return;
                }
                if let Err(error) = take_over_trading(&state).await {
                    tracing::error!(%error, "Could not take over trading; stopping");
                    // The lock goes with `leadership`: stop acting as leader
                    // first, as on a lost lock.
                    state.circuit_breaker.step_down();
                    state.order_executor.begin_draining();
                    lost.notify_one();
                    return;
                }
                halt_for_unreadable_settings(&state, &unreadable_settings).await;
                // Leadership lost: stop saving the breaker's state and
                // starting trades at once, even mid-shutdown (where nothing
                // waits on `lost`), before another process takes over.
                let breaker = state.circuit_breaker.clone();
                let leading = state.circuit_breaker.clone();
                let executor = state.order_executor.clone();
                spawn_trading_tasks(
                    state,
                    arb_config_arc,
                    exit_handler_config,
                    quant_config,
                    exit_readiness,
                );
                leadership.watch(
                    lost,
                    move || {
                        breaker.step_down();
                        executor.begin_draining();
                    },
                    // A save refused for another leader's newer claim steps
                    // the breaker down while the lock may still look held.
                    move || leading.is_leader(),
                );
            });
        }

        // Subscribe local runtime to dynamic updates (arb executor config knobs)
        let redis_url = std::env::var("DYNAMIC_CONFIG_REDIS_URL")
            .or_else(|_| std::env::var("REDIS_URL"))
            .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
        spawn_dynamic_config_subscriber(
            redis_url,
            state.pool.clone(),
            state.arb_executor_config.clone(),
            Some(quant_config.clone()),
        );

        // Spawn wallet harvester (discovers wallets from CLOB trades)
        let harvester_config = WalletHarvesterConfig::from_env();
        spawn_wallet_harvester(
            harvester_config,
            state.clob_client.clone(),
            state.pool.clone(),
            db_semaphore.clone(),
        );

        // Spawn Gamma API market metadata syncer (hourly, populates market_metadata)
        let gamma_config = GammaSyncerConfig::from_env();
        spawn_gamma_syncer(gamma_config, state.pool.clone(), db_semaphore.clone());

        // Spawn flow feature calculator (5 min, aggregates wallet_trades → market_flow_features)
        let flow_config = FlowFeatureConfig::from_env();
        spawn_flow_feature_calculator(flow_config, state.pool.clone(), db_semaphore.clone());

        // Spawn strategy P&L calculator (6h, computes per-strategy performance snapshots)
        let pnl_config = StrategyPnlConfig::from_env();
        spawn_strategy_pnl_calculator(
            pnl_config,
            state.pool.clone(),
            db_semaphore.clone(),
            state.order_executor.clone(),
        );

        // Spawn strategy health calculator (6h, summarizes edge capture + failures)
        let health_config = StrategyHealthConfig::from_env();
        spawn_strategy_health_calculator(
            health_config,
            state.pool.clone(),
            state.order_executor.clone(),
        );

        // Spawn learning evaluator (replays shadow predictions against realized outcomes)
        let learning_evaluator_config = LearningEvaluatorConfig::from_env();
        spawn_learning_evaluator(learning_evaluator_config, state.pool.clone());

        // Spawn rollout observer (persists guardrail observations and auto-rollbacks)
        let learning_rollout_observer_config = LearningRolloutObserverConfig::from_env();
        spawn_learning_rollout_observer(learning_rollout_observer_config, state.pool.clone());

        // Reconcile open-position marks and expose legacy one-legged rows to recovery.
        let position_reconciler_config = PositionReconcilerConfig::from_env();
        spawn_position_reconciler(position_reconciler_config, state.pool.clone());

        // Spawn automated backtest scheduler (polls due schedules and reuses the backtest runner)
        let backtest_automation_config = BacktestAutomationConfig::from_env();
        spawn_backtest_automation(backtest_automation_config, state.pool.clone());

        // Spawn metrics calculator (populates wallet_success_metrics + market regime)
        let metrics_config = MetricsCalculatorConfig::from_env();
        if metrics_config.enabled {
            let calculator = Arc::new(
                MetricsCalculator::with_regime(
                    state.pool.clone(),
                    metrics_config.clone(),
                    state.current_regime.clone(),
                )
                .with_db_semaphore(db_semaphore.clone()),
            );
            tokio::spawn(calculator.run());
            info!(
                interval_secs = metrics_config.interval_secs,
                batch_size = metrics_config.batch_size,
                "Metrics calculator background job spawned (with regime detection)"
            );
        }

        let addr = self.config.socket_addr();
        info!(address = %addr, "Starting API server");

        let listener = tokio::net::TcpListener::bind(addr).await?;
        let stop_accepting = Arc::new(tokio::sync::Notify::new());
        // ConnectInfo gives the rate limiter the TCP peer address to fall back on.
        let server = {
            let stop_accepting = stop_accepting.clone();
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async move { stop_accepting.notified().await })
        };
        let server = std::future::IntoFuture::into_future(server);
        tokio::pin!(server);

        let mut server_error = None;
        let signalled = tokio::select! {
            result = &mut server => {
                if let Err(error) = result {
                    tracing::error!(%error, "API server failed; draining trading operations");
                    server_error = Some(error);
                }
                false
            }
            () = stop.recv() => true,
            () = leadership_lost.notified() => {
                // Another process may lead within seconds. Stop at once
                // rather than drain beside it: what was in flight is
                // journaled, and the next leader's reconcilers book it
                // (into the circuit breaker too), which this process could
                // no longer store. A restart then waits its turn.
                tracing::error!("Trading leadership lost; stopping now");
                state.circuit_breaker.step_down();
                state.order_executor.begin_draining();
                return Err(anyhow::anyhow!("trading leadership lost"));
            }
        };

        // No new trading operation starts from here; started ones (an arb
        // between its legs, an exit, a manual order) finish while open
        // requests do.
        info!("Shutting down: draining requests and trading operations");
        state.order_executor.begin_draining();
        stop_accepting.notify_one();
        let drained = async {
            let requests = async {
                if signalled && tokio::time::timeout(HTTP_DRAIN, &mut server).await.is_err() {
                    tracing::warn!("Requests still open after the drain period; closing them");
                }
            };
            let ((), finished) =
                tokio::join!(requests, state.order_executor.drain(OPERATION_DRAIN));
            finished
        };
        let drained = async {
            let finished = drained.await;
            // Leaving: store what the breaker still owes (a failed save,
            // closes not yet marked), or the next leader counts them again.
            state.circuit_breaker.flush().await;
            finished
        };
        tokio::select! {
            finished = drained => {
                if finished {
                    info!("Trading operations finished; shutting down");
                } else {
                    tracing::warn!(
                        "Trading operations still running at shutdown; their journaled orders \
                         are reconciled on the next start"
                    );
                }
            }
            () = stop.recv() => {
                tracing::warn!(
                    "Stop requested again; exiting without waiting. Journaled orders are \
                     reconciled on the next start"
                );
            }
            // Leadership lost while draining: another process may lead now.
            // Exit at once rather than let this one's background tasks book
            // closes the next leader cannot see in time.
            () = leadership_lost.notified() => {
                tracing::error!("Trading leadership lost while shutting down; stopping now");
                return Err(anyhow::anyhow!("trading leadership lost"));
            }
        }
        match server_error {
            Some(error) => Err(error.into()),
            None => Ok(()),
        }
    }
}

/// The tasks that trade or keep trading state (the circuit breaker's feed
/// included): run by the trading leader only, see [`trading_leader`].
fn spawn_trading_tasks(
    state: Arc<AppState>,
    arb_config_arc: Arc<RwLock<ArbExecutorConfig>>,
    exit_handler_config: Arc<RwLock<ExitHandlerConfig>>,
    quant_config: Arc<RwLock<QuantSignalExecutorConfig>>,
    exit_readiness: crate::exit_handler::ExitReadiness,
) {
    // Spawn arb auto-executor unconditionally (per-signal guard checks enabled).
    // It and the exit handler share the one market catalog.
    spawn_arb_auto_executor(
        arb_config_arc.clone(),
        state.subscribe_arb_entry(),
        state.signal_tx.clone(),
        state.trade_event_tx.clone(),
        state.order_executor.clone(),
        state.circuit_breaker.clone(),
        state.market_catalog.clone(),
        state.pool.clone(),
        state.arb_executor_heartbeat.clone(),
        state.arb_executor_status.clone(),
        exit_readiness.clone(),
    );

    // Spawn exit handler unconditionally (per-tick guard checks enabled)
    spawn_exit_handler(
        exit_handler_config,
        state.order_executor.clone(),
        state.circuit_breaker.clone(),
        state.clob_client.clone(),
        state.signal_tx.clone(),
        state.trade_event_tx.clone(),
        state.pool.clone(),
        state.exit_handler_heartbeat.clone(),
        state.market_catalog.clone(),
    );

    // Spawn dynamic tuner (adaptive runtime configuration) after subscriber
    // so startup sync publications are not missed by local runtime.
    let tuner = Arc::new(
        DynamicTuner::new(
            state.pool.clone(),
            state.current_regime.clone(),
            state.circuit_breaker.clone(),
        )
        .with_order_executor(state.order_executor.clone()),
    );
    tokio::spawn(tuner.start());

    let wallet_inventory_config = WalletInventoryConfig::from_env();
    spawn_wallet_inventory_reconciler(wallet_inventory_config, state.clone());

    // ── Quant signal system: executor + generators ──
    // Executor receives QuantSignal from broadcast channel and evaluates/executes.
    spawn_quant_signal_executor(
        quant_config,
        state.quant_signal_tx.subscribe(),
        state.signal_tx.clone(),
        state.trade_event_tx.clone(),
        state.order_executor.clone(),
        state.circuit_breaker.clone(),
        state.market_catalog.clone(),
        state.pool.clone(),
        state.quant_executor_heartbeat.clone(),
        exit_readiness,
    );

    // Signal generators — each polls feature tables and emits QuantSignal
    use signals::{
        cross_market_signal::CrossMarketSignalConfig, flow_signal::FlowSignalConfig,
        mean_reversion_signal::MeanReversionSignalConfig,
        resolution_signal::ResolutionSignalConfig,
    };

    let flow_signal_config = FlowSignalConfig::from_env();
    spawn_flow_signal_generator(
        flow_signal_config,
        state.pool.clone(),
        state.quant_signal_tx.clone(),
    );

    let resolution_config = ResolutionSignalConfig::from_env();
    spawn_resolution_signal_generator(
        resolution_config,
        state.pool.clone(),
        state.quant_signal_tx.clone(),
    );

    let mean_rev_config = MeanReversionSignalConfig::from_env();
    spawn_mean_reversion_signal_generator(
        mean_rev_config,
        state.pool.clone(),
        state.quant_signal_tx.clone(),
    );

    let cross_market_config = CrossMarketSignalConfig::from_env();
    spawn_cross_market_signal_generator(
        cross_market_config,
        state.pool.clone(),
        state.quant_signal_tx.clone(),
    );

    intent_reconciler::spawn(state.clone());
    // Resolve every journaled on-chain withdrawal from the chain.
    crate::withdrawals::spawn_withdrawal_reconciler(state.clone());
    // Record live manual sales' results with the circuit breaker.
    spawn_manual_result_recorder(state.clone());

    // Spawn canonical account snapshot calculator (wallet cash + marked exposure)
    let account_snapshot_config = AccountSnapshotConfig::from_env();
    spawn_account_snapshot_calculator(account_snapshot_config, state.clone());
}

/// Become the trading leader: reload what another process may have changed
/// since this one started, the circuit breaker's stored state (the previous
/// leader's last save included), its saved limits and the service toggles.
async fn take_over_trading(state: &AppState) -> anyhow::Result<()> {
    // Closes a stopped process booked but never recorded count before any
    // new trade does: entries are held until they are (a manual buy in any
    // process included: the hold is stored as soon as the lead is claimed).
    // A failure does not stop the takeover (exits must run): the closes are
    // retried in the background, entries held, until they are counted.
    state.circuit_breaker.hold_entries(
        MISSED_CLOSES_HOLD,
        "closes a previous trading process left uncounted by the circuit breaker are not \
         counted yet",
    );
    let mut attempt = 0;
    loop {
        attempt += 1;
        match state.circuit_breaker.lead().await {
            Ok(_) => break,
            Err(error) if attempt < 5 => {
                tracing::warn!(attempt, %error, "Could not load the circuit breaker's state; retrying");
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
            Err(error) => return Err(error.context("loading the circuit breaker's state")),
        }
    }
    // Store the hold for the other processes now (a failure leaves it owed
    // to the next sync).
    state.circuit_breaker.flush().await;
    let limits = crate::state::circuit_breaker_config(&state.pool).await?;
    state.circuit_breaker.update_config(limits).await;
    let mut attempt = 0;
    loop {
        attempt += 1;
        // Nothing here trades yet, so every unmarked close is another
        // process's: no cutoff (one would depend on the clocks agreeing).
        match crate::exit_handler::record_missed_closes(&state.pool, &state.circuit_breaker, None)
            .await
        {
            Ok(()) => {
                state.circuit_breaker.release_entries(MISSED_CLOSES_HOLD);
                break;
            }
            Err(error) if attempt < 3 => {
                tracing::warn!(attempt, %error, "Could not record closes the circuit breaker missed; retrying");
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
            Err(error) => {
                tracing::error!(
                    %error,
                    "Could not record closes the circuit breaker missed; new entries wait until they are \
                     (retried every 30 seconds)"
                );
                // From here this process trades and records its own closes:
                // the retry looks only at closes made before now, as late as
                // the cutoff can be.
                spawn_missed_close_retry(
                    state.pool.clone(),
                    state.circuit_breaker.clone(),
                    chrono::Utc::now(),
                );
                break;
            }
        }
    }
    crate::dynamic_tuner::apply_dynamic_config_snapshot(
        &state.pool,
        state.arb_executor_config.as_ref(),
        state.quant_executor_config.as_ref(),
    )
    .await;
    crate::runtime_sync::reconcile_runtime_service_toggles(state).await;
    Ok(())
}

/// The entry hold a failed replay of missed closes sets.
const MISSED_CLOSES_HOLD: &str = "missed_closes";
/// The entry hold unreadable settings set, in every process.
const UNREADABLE_SETTINGS_HOLD: &str = "unreadable_settings";

/// Every 15 seconds while this process leads, records with the circuit
/// breaker the results of live manual sales filled since (in any process).
fn spawn_manual_result_recorder(state: Arc<AppState>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(15)).await;
            if !state.circuit_breaker.is_leader() {
                return;
            }
            if let Err(error) =
                crate::exit_handler::record_manual_results(&state.pool, &state.circuit_breaker)
                    .await
            {
                tracing::warn!(%error, "Could not record manual sales with the circuit breaker; retrying");
            }
        }
    });
}

/// Retries recording the closes the circuit breaker missed until it works,
/// then lifts the hold on new entries. Only closes made before `closed_before`
/// (when this process began trading) are looked at: it records its own. A
/// process that stops leading lets go (the next leader replays them).
fn spawn_missed_close_retry(
    pool: sqlx::PgPool,
    breaker: Arc<risk_manager::CircuitBreaker>,
    closed_before: chrono::DateTime<chrono::Utc>,
) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            if !breaker.is_leader() {
                breaker.release_entries(MISSED_CLOSES_HOLD);
                return;
            }
            match crate::exit_handler::record_missed_closes(&pool, &breaker, Some(closed_before))
                .await
            {
                Ok(()) => {
                    info!("Recorded the closes the circuit breaker missed; new entries may resume");
                    breaker.release_entries(MISSED_CLOSES_HOLD);
                    return;
                }
                Err(error) => {
                    tracing::warn!(%error, "Still cannot record the closes the circuit breaker missed; new entries stay held");
                }
            }
        }
    });
}

/// Holds new entries in this process while settings cannot be read, so they
/// never run on a default the operator did not choose. Set before the process
/// leads or serves orders (a follower's manual buys check it too); a breaker
/// reset does not lift it, a restart with the settings corrected does.
fn hold_for_unreadable_settings(state: &AppState, unreadable: &[String]) {
    if !unreadable.is_empty() {
        state.circuit_breaker.hold_entries(
            UNREADABLE_SETTINGS_HOLD,
            format!("settings that cannot be read: {}", unreadable.join("; ")),
        );
    }
}

/// Trips the circuit breaker (latched, as an operator trip) when settings
/// cannot be read, so the halt shows and is stored: new entries halt and
/// exits keep running, until the settings are corrected, the server
/// restarted and the breaker reset ([`hold_for_unreadable_settings`] keeps
/// entries held in this process whatever a reset does). Also flags live
/// trading without the equity feed the breaker's drawdown limits read.
async fn halt_for_unreadable_settings(state: &AppState, unreadable: &[String]) {
    if !unreadable.is_empty() {
        let reason = format!("settings that cannot be read: {}", unreadable.join("; "));
        // A failed save still trips this process; its next save stores it.
        if let Err(error) = state.circuit_breaker.manual_trip(Some(reason)).await {
            tracing::error!(%error, "Could not store the trip for unreadable settings");
        }
    }
    if state.order_executor.is_live()
        && !polymarket_core::config::env_flag("ACCOUNT_SNAPSHOT_ENABLED", true)
    {
        tracing::error!(
            "Live trading is on with ACCOUNT_SNAPSHOT_ENABLED off: the circuit breaker gets no \
             equity readings, so its drawdown limits and its trip for an account that cannot be \
             valued never act"
        );
    }
}

/// Take up every few seconds what operators change through any API process:
/// circuit breaker trips, resets and limits, the workspace's service
/// toggles and the stored dynamic config. The trading leader's breaker also
/// ends a cooldown and advances recovery here, on time, whether or not
/// anything asks to trade.
fn spawn_operator_sync(state: Arc<AppState>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(5));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            background::pass("operator_sync", sync_operator_settings(&state)).await;
        }
    });
}

async fn sync_operator_settings(state: &AppState) {
    // The limits first: the breaker's clock judges a cooldown's end by them,
    // and does not run on limits that could not be read.
    match crate::state::circuit_breaker_config(&state.pool).await {
        Ok(limits) => {
            if limits != state.circuit_breaker.config().await {
                info!("Applying circuit breaker limits saved through another process");
                state.circuit_breaker.update_config(limits).await;
            }
            state.circuit_breaker.tick().await;
        }
        Err(error) => {
            tracing::warn!(%error, "Could not re-read the circuit breaker limits");
            state.circuit_breaker.sync().await;
        }
    }
    crate::runtime_sync::reconcile_runtime_service_toggles(state).await;
    crate::dynamic_tuner::apply_dynamic_config_snapshot(
        &state.pool,
        state.arb_executor_config.as_ref(),
        state.quant_executor_config.as_ref(),
    )
    .await;
}

/// How long open HTTP requests may finish after a shutdown request.
const HTTP_DRAIN: std::time::Duration = std::time::Duration::from_secs(15);
/// How long started trading operations may finish, alongside open requests:
/// enough for an order's whole preparation deadline (30s by default) and its
/// send, so an arb between its legs gets its second leg out.
const OPERATION_DRAIN: std::time::Duration = std::time::Duration::from_secs(40);

#[cfg(test)]
mod cors_tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{header, Method, Request, StatusCode};
    use tower::ServiceExt;

    fn config(permissive: bool, origins: &[&str]) -> ServerConfig {
        ServerConfig {
            cors_permissive: permissive,
            cors_allowed_origins: origins.iter().map(|o| o.to_string()).collect(),
            ..ServerConfig::default()
        }
    }

    async fn send(
        config: &ServerConfig,
        method: Method,
        origin: &str,
        preflight_headers: bool,
    ) -> axum::http::Response<Body> {
        let app = axum::Router::new()
            .route("/x", axum::routing::any(|| async { "ok" }))
            .layer(build_cors_layer(config));
        let mut request = Request::builder()
            .method(method)
            .uri("/x")
            .header(header::ORIGIN, origin);
        if preflight_headers {
            request = request
                .header(header::ACCESS_CONTROL_REQUEST_METHOD, "DELETE")
                .header(
                    header::ACCESS_CONTROL_REQUEST_HEADERS,
                    "authorization,content-type",
                );
        }
        app.oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    fn allow_origin(response: &axum::http::Response<Body>) -> Option<String> {
        response
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .map(|v| v.to_str().unwrap().to_string())
    }

    #[test]
    fn origin_lists_are_trimmed() {
        assert_eq!(
            parse_origin_list(" https://a.example/ , ,https://b.example:8443,"),
            vec!["https://a.example", "https://b.example:8443"]
        );
        assert!(parse_origin_list("  ").is_empty());
    }

    #[tokio::test]
    async fn an_allowlist_only_admits_exact_origins() {
        let config = config(
            false,
            &["https://app.example", "https://admin.example:8443"],
        );
        for allowed in ["https://app.example", "https://admin.example:8443"] {
            let response = send(&config, Method::GET, allowed, false).await;
            assert_eq!(allow_origin(&response).as_deref(), Some(allowed));
        }
        for denied in [
            "https://evil.example",
            "https://app.example.evil.example",
            "http://app.example",
            "https://app.example:8443",
            "null",
        ] {
            let response = send(&config, Method::GET, denied, false).await;
            assert_eq!(allow_origin(&response), None, "{denied}");
        }
    }

    #[tokio::test]
    async fn the_allowlist_is_used_even_when_cors_is_permissive() {
        let config = config(true, &["https://app.example"]);
        let response = send(&config, Method::GET, "https://evil.example", false).await;
        assert_eq!(allow_origin(&response), None);
    }

    #[tokio::test]
    async fn preflight_advertises_only_the_dashboard_methods_and_headers() {
        let config = config(false, &["https://app.example"]);
        let response = send(&config, Method::OPTIONS, "https://app.example", true).await;
        assert_eq!(response.status(), StatusCode::OK);
        let header = |name| response.headers()[name].to_str().unwrap().to_string();
        assert_eq!(
            allow_origin(&response).as_deref(),
            Some("https://app.example")
        );
        let methods = header(header::ACCESS_CONTROL_ALLOW_METHODS);
        for method in ["GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"] {
            assert!(methods.contains(method), "{methods}");
        }
        assert!(!methods.contains('*'));
        let headers = header(header::ACCESS_CONTROL_ALLOW_HEADERS).to_ascii_lowercase();
        assert!(headers.contains("authorization") && headers.contains("content-type"));
        assert!(!headers.contains('*'), "{headers}");
        assert_eq!(header(header::ACCESS_CONTROL_MAX_AGE), "600");
    }

    #[tokio::test]
    async fn unusable_allowlist_entries_fail_closed() {
        for entries in [&["*"][..], &["bad\norigin"][..], &["*", "bad\norigin"][..]] {
            let config = config(true, entries);
            let response = send(&config, Method::GET, "https://app.example", false).await;
            assert_eq!(allow_origin(&response), None, "{entries:?}");
        }
        // A usable entry next to an unusable one still works.
        let config = config(false, &["*", "https://app.example"]);
        let response = send(&config, Method::GET, "https://app.example", false).await;
        assert_eq!(
            allow_origin(&response).as_deref(),
            Some("https://app.example")
        );
    }

    #[tokio::test]
    async fn without_an_allowlist_behaviour_is_unchanged() {
        // CORS_PERMISSIVE=true: any origin.
        let response = send(
            &config(true, &[]),
            Method::GET,
            "https://any.example",
            false,
        )
        .await;
        assert!(allow_origin(&response).is_some());
        // CORS_PERMISSIVE=false and no allowlist: still any origin (the old behaviour,
        // now logged as a startup warning).
        let response = send(
            &config(false, &[]),
            Method::GET,
            "https://any.example",
            false,
        )
        .await;
        assert_eq!(allow_origin(&response).as_deref(), Some("*"));
    }
}
