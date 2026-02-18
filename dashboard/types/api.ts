// Auth types
// Platform-level roles (distinct from WorkspaceRole which controls per-workspace access)
export type UserRole = "Viewer" | "Trader" | "PlatformAdmin";

export interface User {
  id: string;
  email?: string;
  name?: string;
  wallet_address?: string;
  role: UserRole;
  created_at: string;
}

export interface WalletUser {
  id: string;
  wallet_address: string;
  email?: string;
  name?: string;
  role: string;
  created_at: string;
}

export interface AuthResponse {
  token: string;
  user: User;
}

export interface LoginRequest {
  email: string;
  password: string;
}

// User management types (admin)
export interface UserListItem {
  id: string;
  email: string;
  name?: string;
  role: UserRole;
  created_at: string;
  last_login?: string;
}

export interface CreateUserRequest {
  email: string;
  password: string;
  name?: string;
  role?: UserRole;
}

export interface UpdateUserRequest {
  name?: string;
  role?: UserRole;
  password?: string;
}

// WalletPosition - shared UI position format used by WalletCard and ManualPositions
export interface WalletPosition {
  id: string;
  marketId: string;
  marketQuestion?: string;
  outcome: "yes" | "no";
  quantity: number;
  entryPrice: number;
  currentPrice: number;
  pnl: number;
  pnlPercent: number;
}

// Position types
export type PositionSide = "long" | "short";
export type PositionOutcome = "yes" | "no";
export type PositionStatus = "open" | "closed" | "all";

/** Full position lifecycle state from backend */
export type PositionState =
  | "pending"
  | "open"
  | "exit_ready"
  | "closing"
  | "closed"
  | "entry_failed"
  | "exit_failed"
  | "stalled";

export interface Position {
  id: string;
  market_id: string;
  outcome: PositionOutcome;
  side: PositionSide;
  quantity: number;
  entry_price: number;
  current_price: number;
  unrealized_pnl: number;
  unrealized_pnl_pct: number;
  stop_loss?: number;
  take_profit?: number;
  is_copy_trade: boolean;
  source_wallet?: string;
  realized_pnl?: number;
  opened_at: string;
  updated_at: string;
  /** Full lifecycle state (if returned by API) */
  state?: PositionState;
  /** Actual exit prices (from backend close_via_exit / close_via_resolution) */
  yes_exit_price?: number;
  no_exit_price?: number;
  /** Fee breakdown */
  entry_fees?: number;
  exit_fees?: number;
}

// Market types
export interface Market {
  id: string;
  question: string;
  description?: string;
  category: string;
  end_date: string;
  active: boolean;
  yes_price: number;
  no_price: number;
  volume_24h: number;
  liquidity: number;
  created_at: string;
}

export interface PriceLevel {
  price: number;
  quantity: number;
}

export interface SpreadInfo {
  yes_spread: number;
  no_spread: number;
  arb_spread?: number;
}

export interface Orderbook {
  market_id: string;
  timestamp: string;
  yes_bids: PriceLevel[];
  yes_asks: PriceLevel[];
  no_bids: PriceLevel[];
  no_asks: PriceLevel[];
  spread: SpreadInfo;
}

// Wallet types
export interface TrackedWallet {
  address: string;
  label?: string;
  copy_enabled: boolean;
  allocation_pct: number;
  max_position_size: number;
  success_score: number;
  total_pnl: number;
  win_rate: number;
  total_trades: number;
  added_at: string;
  last_activity?: string;
}

export interface WalletMetrics {
  address: string;
  roi: number;
  sharpe_ratio: number;
  sortino_ratio?: number;
  volatility?: number;
  max_drawdown: number;
  avg_trade_size: number;
  avg_hold_time_hours: number;
  profit_factor: number;
  recent_pnl_30d: number;
  category_win_rates: Record<string, number>;
  calculated_at: string;
}

// For display purposes (combines TrackedWallet with additional UI data)
export interface Wallet extends TrackedWallet {
  metrics?: WalletMetrics;
  equity_curve?: { time: string; value: number }[];
  prediction?: {
    success_probability: number;
    confidence: number;
    category:
      | "HIGH_POTENTIAL"
      | "MODERATE"
      | "LOW_POTENTIAL"
      | "INSUFFICIENT_DATA";
  };
}

// Order types
export type OrderSide = "Buy" | "Sell";
export type OrderType = "Market" | "Limit" | "StopLoss" | "TakeProfit";
export type OrderStatus =
  | "Pending"
  | "Open"
  | "PartiallyFilled"
  | "Filled"
  | "Cancelled"
  | "Rejected"
  | "Expired";

export interface PlaceOrderRequest {
  market_id: string;
  outcome: PositionOutcome;
  side: OrderSide;
  order_type: OrderType;
  quantity: number;
  price?: number;
  stop_price?: number;
  time_in_force?: string;
  client_order_id?: string;
}

export interface Order {
  id: string;
  client_order_id?: string;
  market_id: string;
  outcome: string;
  side: OrderSide;
  order_type: OrderType;
  status: OrderStatus;
  quantity: number;
  filled_quantity: number;
  remaining_quantity: number;
  price?: number;
  avg_fill_price?: number;
  stop_price?: number;
  /** Expected price at signal detection time (for slippage checks) */
  expected_price?: number;
  /** Calculated slippage: |avg_fill_price - expected_price| / expected_price */
  slippage_pct?: number;
  time_in_force: string;
  created_at: string;
  updated_at: string;
  filled_at?: string;
}

// Backtest types
export type StrategyType = "arbitrage" | "momentum" | "mean_reversion" | "grid";

export type SlippageModel =
  | { type: "none" }
  | { type: "fixed"; pct: number }
  | { type: "volume_based"; base_pct: number; volume_factor: number };

export interface StrategyConfig {
  type: StrategyType;
  // Arbitrage
  min_spread?: number;
  max_position?: number;
  // Momentum
  lookback_hours?: number;
  threshold?: number;
  position_size?: number;
  // MeanReversion
  window_hours?: number;
  std_threshold?: number;
  // Grid
  grid_levels?: number;
  grid_spacing_pct?: number;
  order_size?: number;
}

export interface BacktestParams {
  strategy: StrategyConfig;
  start_date: string;
  end_date: string;
  initial_capital: number;
  markets?: string[];
  slippage_model?: SlippageModel;
  fee_pct?: number;
}

export interface EquityPoint {
  timestamp: string;
  value: number;
}

export interface TradeLogEntry {
  market_id: string;
  outcome_id: string;
  trade_type: string;
  entry_time: string;
  exit_time?: string;
  entry_price: number;
  exit_price?: number;
  quantity: number;
  fees: number;
  pnl?: number;
  return_pct?: number;
}

export interface BacktestResult {
  id: string;
  strategy: StrategyConfig;
  start_date: string;
  end_date: string;
  initial_capital: number;
  final_value: number;
  total_return: number;
  total_return_pct: number;
  annualized_return: number;
  sharpe_ratio: number;
  sortino_ratio: number;
  max_drawdown: number;
  max_drawdown_pct: number;
  total_trades: number;
  winning_trades: number;
  losing_trades: number;
  win_rate: number;
  avg_win: number;
  avg_loss: number;
  profit_factor: number;
  total_fees: number;
  created_at: string;
  status: "pending" | "running" | "completed" | "failed";
  error?: string;
  equity_curve?: EquityPoint[];
  // Extended metrics
  expectancy?: number;
  calmar_ratio?: number;
  var_95?: number;
  cvar_95?: number;
  recovery_factor?: number;
  best_trade_return?: number;
  worst_trade_return?: number;
  max_consecutive_wins?: number;
  max_consecutive_losses?: number;
  avg_trade_duration_hours?: number;
  trade_log?: TradeLogEntry[];
}

// Portfolio types (derived from positions)
export interface PortfolioStats {
  total_value: number;
  total_pnl: number;
  total_pnl_percent: number;
  today_pnl: number;
  today_pnl_percent: number;
  unrealized_pnl: number;
  realized_pnl: number;
  total_fees: number;
  win_rate: number;
  total_trades: number;
  winning_trades: number;
  active_positions: number;
}

export interface PortfolioHistory {
  timestamp: string;
  value: number;
}

// Allocation types
export type AllocationStrategy =
  | "EQUAL_WEIGHT"
  | "PERFORMANCE_WEIGHTED"
  | "RISK_ADJUSTED"
  | "CUSTOM";

export interface StrategyAllocation {
  strategy_id: string;
  strategy_type: "WALLET" | "ARBITRAGE";
  wallet_address?: string;
  allocation_percent: number;
  allocation_amount: number;
}

export interface AllocationConfig {
  id: string;
  total_capital: number;
  strategy: AllocationStrategy;
  allocations: StrategyAllocation[];
  active: boolean;
  created_at: string;
  updated_at: string;
}

// Activity types
export type ActivityType =
  | "TRADE_COPIED"
  | "TRADE_COPY_SKIPPED"
  | "TRADE_COPY_FAILED"
  | "POSITION_OPENED"
  | "POSITION_CLOSED"
  | "STOP_LOSS_TRIGGERED"
  | "TAKE_PROFIT_TRIGGERED"
  | "ARBITRAGE_DETECTED"
  | "ARB_POSITION_OPENED"
  | "ARB_POSITION_CLOSED"
  | "ARB_EXECUTION_FAILED"
  | "ARB_EXIT_FAILED"
  | "RECOMMENDATION_NEW"
  | "ALLOCATION_ACTIVATED";

export interface Activity {
  id: string;
  type: ActivityType;
  message: string;
  details?: Record<string, unknown>;
  pnl?: number;
  created_at: string;
}

// Recommendation types
export interface Recommendation {
  id: string;
  type: "COPY_WALLET" | "ARBITRAGE" | "POSITION_EXIT";
  confidence: number;
  wallet?: Wallet;
  expected_return?: number;
  risk_level: "LOW" | "MEDIUM" | "HIGH";
  reason: string;
  created_at: string;
  expires_at?: string;
}

// Trade classification types (Event vs Arb)
export type TradeClassification = "event" | "arbitrage" | "mixed";
export type TradingStyle = "event_trader" | "arb_trader" | "mixed";
export type CopyBehavior = "copy_all" | "events_only" | "arb_threshold";
export type WalletTier = "active" | "bench" | "untracked";

// Copy settings for a wallet
export interface CopySettings {
  copy_behavior: CopyBehavior;
  allocation_pct: number;
  max_position_size: number;
  arb_threshold_pct?: number; // Min spread % for arb replication
}

// Decision Brief for wallet strategy profiling
export interface DecisionBrief {
  trading_style: TradingStyle;
  slippage_tolerance: "tight" | "moderate" | "loose";
  preferred_categories: string[];
  typical_hold_time: string;
  event_trade_ratio: number;
  arb_trade_ratio: number;
  fitness_score: number;
  fitness_reasons: string[];
}

// Extended wallet with roster info
export interface RosterWallet extends Wallet {
  tier: WalletTier;
  copy_settings?: CopySettings;
  decision_brief?: DecisionBrief;
}

// Vault types (connected user wallets for live trading)
export interface ConnectedWallet {
  id: string;
  address: string;
  label?: string;
  is_primary: boolean;
  created_at: string;
}

export interface StoreWalletRequest {
  address: string;
  private_key: string;
  label?: string;
}

// Health check
export interface HealthResponse {
  status: string;
  version: string;
  timestamp: string;
  database?: string;
}

// API Response types
export interface ApiError {
  code: string;
  message: string;
  details?: Record<string, unknown>;
}

export interface PaginatedResponse<T> {
  data: T[];
  total: number;
  limit: number;
  offset: number;
}

// WebSocket types
export type PositionUpdateType =
  | "Opened"
  | "Updated"
  | "Closed"
  | "PriceChanged";
export type SignalType =
  | "Arbitrage"
  | "CopyTrade"
  | "StopLoss"
  | "TakeProfit"
  | "Alert";

export interface OrderbookUpdate {
  market_id: string;
  timestamp: string;
  yes_bid: number;
  yes_ask: number;
  no_bid: number;
  no_ask: number;
  arb_spread?: number;
}

export interface PositionUpdate {
  position_id: string;
  market_id: string;
  update_type: PositionUpdateType;
  quantity: number;
  current_price: number;
  unrealized_pnl: number;
  timestamp: string;
}

export interface SignalUpdate {
  signal_id: string;
  signal_type: SignalType;
  market_id: string;
  outcome_id: string;
  action: string;
  confidence: number;
  timestamp: string;
  metadata: Record<string, unknown>;
}

export type WebSocketMessage =
  | { type: "Orderbook"; data: OrderbookUpdate }
  | { type: "Position"; data: PositionUpdate }
  | { type: "Signal"; data: SignalUpdate }
  | { type: "Subscribed"; channel: string }
  | { type: "Unsubscribed"; channel: string }
  | { type: "Error"; code: string; message: string }
  | { type: "Ping" }
  | { type: "Pong" };

// Discovery/Live trades types
export type PredictionCategory =
  | "HIGH_POTENTIAL"
  | "MODERATE"
  | "LOW_POTENTIAL"
  | "INSUFFICIENT_DATA";

export interface WalletTrade {
  transaction_hash: string;
  wallet_address: string;
  asset_id: string;
  side: string;
  price: number;
  quantity: number;
  value: number;
  timestamp: string;
  title?: string;
  outcome?: string;
}

export interface LiveTrade {
  wallet_address: string;
  wallet_label?: string;
  tx_hash: string;
  timestamp: string;
  market_id: string;
  market_question?: string;
  outcome: string;
  direction: "buy" | "sell";
  price: number;
  quantity: number;
  value: number;
}

export interface DiscoveredWallet {
  address: string;
  rank: number;
  roi_7d: number;
  roi_30d: number;
  roi_90d: number;
  sharpe_ratio: number;
  sortino_ratio?: number;
  volatility?: number;
  total_trades: number;
  win_rate: number;
  max_drawdown: number;
  prediction: PredictionCategory;
  confidence: number;
  is_tracked: boolean;
  trades_24h: number;
  total_pnl: number;
  composite_score?: number;
  strategy_type?: string;
  staleness_days: number;
}

// Market regime types
export type MarketRegimeType =
  | "BullVolatile"
  | "BullCalm"
  | "BearVolatile"
  | "BearCalm"
  | "Ranging"
  | "Uncertain";

export interface MarketRegimeResponse {
  regime: MarketRegimeType;
  label: string;
  icon: string;
  description: string;
}

// Calibration types
export interface CalibrationBucket {
  lower: number;
  upper: number;
  avg_predicted: number;
  observed_rate: number;
  count: number;
  gap: number;
}

export interface CalibrationReport {
  buckets: CalibrationBucket[];
  ece: number;
  total_predictions: number;
  recommended_threshold: number;
}

// Copy performance types
export interface CopyPerformanceResponse {
  address: string;
  reported_win_rate: number;
  copy_win_rate: number | null;
  copy_trade_count: number;
  copy_total_pnl: number;
  divergence_pp: number | null;
  has_significant_divergence: boolean;
}

// Workspace types
export type SetupMode = "manual" | "automatic";
export type WorkspaceRole = "owner" | "admin" | "member" | "viewer";

export interface Workspace {
  id: string;
  name: string;
  description?: string;
  setup_mode: SetupMode;
  total_budget: number;
  reserved_cash_pct: number;
  auto_optimize_enabled: boolean;
  optimization_interval_hours: number;
  min_roi_30d?: number;
  min_sharpe?: number;
  min_win_rate?: number;
  min_trades_30d?: number;
  trading_wallet_address?: string;
  walletconnect_project_id?: string;
  polygon_rpc_url?: string;
  alchemy_api_key?: string;
  arb_auto_execute: boolean;
  copy_trading_enabled: boolean;
  live_trading_enabled: boolean;
  my_role: WorkspaceRole;
  onboarding_completed?: boolean;
  created_by?: string;
  created_at: string;
  updated_at: string;
}

export interface WorkspaceListItem {
  id: string;
  name: string;
  description?: string;
  setup_mode: SetupMode;
  owner_email?: string;
  member_count: number;
  my_role?: WorkspaceRole;
  created_at: string;
}

export interface WorkspaceMember {
  workspace_id: string;
  user_id: string;
  role: WorkspaceRole;
  joined_at: string;
  email?: string;
  name?: string;
}

export interface WorkspaceInvite {
  id: string;
  workspace_id: string;
  email: string;
  role: WorkspaceRole;
  invited_by: string;
  expires_at: string;
  accepted_at?: string;
  created_at: string;
  workspace_name?: string;
  inviter_email?: string;
}

export interface WorkspaceAllocation {
  id: string;
  workspace_id: string;
  wallet_address: string;
  allocation_pct: number;
  max_position_size?: number;
  tier: "active" | "bench";
  auto_assigned: boolean;
  auto_assigned_reason?: string;
  backtest_roi?: number;
  backtest_sharpe?: number;
  backtest_win_rate?: number;
  copy_behavior: CopyBehavior;
  arb_threshold_pct?: number;
  added_by?: string;
  added_at: string;
  updated_at: string;
  // Pin status (prevents auto-demotion)
  pinned?: boolean;
  pinned_at?: string;
  pinned_by?: string;
  // Probation status (new wallets)
  probation_until?: string;
  probation_allocation_pct?: number;
  // Loss tracking
  consecutive_losses?: number;
  last_loss_at?: string;
  // Confidence score
  confidence_score?: number;
  // Grace period
  grace_period_started_at?: string;
  grace_period_reason?: string;
  // Wallet metadata
  wallet_label?: string;
  wallet_success_score?: number;
}

// Wallet ban (prevents auto-promotion)
export interface WalletBan {
  id: string;
  wallet_address: string;
  reason?: string;
  banned_at: string;
  expires_at?: string;
}

export interface RotationHistoryEntry {
  id: string;
  action: string;
  wallet_in?: string;
  wallet_out?: string;
  reason: string;
  evidence: Record<string, unknown>;
  triggered_by?: string;
  is_automatic: boolean;
  notification_sent: boolean;
  acknowledged: boolean;
  acknowledged_at?: string;
  acknowledged_by?: string;
  created_at: string;
}

export interface OnboardingStatus {
  workspace_id?: string;
  workspace_name?: string;
  setup_mode?: SetupMode;
  onboarding_completed: boolean;
  onboarding_step: number;
  total_budget?: number;
  active_wallet_count: number;
  bench_wallet_count: number;
}

export interface UserSettings {
  user_id: string;
  onboarding_completed: boolean;
  onboarding_step: number;
  default_workspace_id?: string;
  preferences: Record<string, unknown>;
  created_at: string;
  updated_at: string;
}

export interface CreateWorkspaceRequest {
  name: string;
  description?: string;
  owner_email: string;
  setup_mode?: SetupMode;
}

export interface UpdateWorkspaceRequest {
  name?: string;
  description?: string;
  setup_mode?: SetupMode;
  total_budget?: number;
  reserved_cash_pct?: number;
  auto_optimize_enabled?: boolean;
  optimization_interval_hours?: number;
  min_roi_30d?: number;
  min_sharpe?: number;
  min_win_rate?: number;
  min_trades_30d?: number;
  walletconnect_project_id?: string;
  polygon_rpc_url?: string;
  alchemy_api_key?: string;
  arb_auto_execute?: boolean;
  copy_trading_enabled?: boolean;
  live_trading_enabled?: boolean;
}

export interface CreateInviteRequest {
  email: string;
  role: WorkspaceRole;
}

export interface InviteInfo {
  workspace_name: string;
  inviter_email: string;
  email: string;
  role: WorkspaceRole;
  expires_at: string;
  user_exists: boolean;
}

export interface AcceptInviteRequest {
  email?: string;
  password?: string;
  name?: string;
}

export interface AcceptInviteResponse {
  workspace_id: string;
  workspace_name: string;
  role: string;
  is_new_user: boolean;
  // Only present for new users
  token?: string;
  user?: User;
}

export interface AddAllocationRequest {
  allocation_pct?: number;
  max_position_size?: number;
  tier?: "active" | "bench";
  copy_behavior?: CopyBehavior;
  arb_threshold_pct?: number;
}

export interface UpdateAllocationRequest {
  allocation_pct?: number;
  max_position_size?: number;
  copy_behavior?: CopyBehavior;
  arb_threshold_pct?: number;
}

export interface SetBudgetRequest {
  total_budget: number;
  reserved_cash_pct?: number;
}

export interface AutoSetupConfig {
  min_roi_30d?: number;
  min_sharpe?: number;
  min_win_rate?: number;
  min_trades_30d?: number;
}

export interface AutoSelectedWallet {
  address: string;
  allocation_pct: number;
  roi_30d?: number;
  sharpe_ratio?: number;
  win_rate?: number;
  reason: string;
}

export interface AutoSetupResponse {
  success: boolean;
  message: string;
  selected_wallets: AutoSelectedWallet[];
}

// Optimizer Status types
export interface OptimizerCriteria {
  min_roi_30d: number | null;
  min_sharpe: number | null;
  min_win_rate: number | null;
  min_trades_30d: number | null;
}

export interface OptimizerPortfolioMetrics {
  total_roi_30d: number;
  avg_sharpe: number;
  avg_win_rate: number;
  total_value: number;
}

export interface OptimizerStatus {
  enabled: boolean;
  last_run_at: string | null;
  next_run_at: string | null;
  interval_hours: number;
  criteria: OptimizerCriteria;
  active_wallet_count: number;
  bench_wallet_count: number;
  portfolio_metrics: OptimizerPortfolioMetrics;
}

export interface OptimizationResult {
  candidates_found: number;
  wallets_promoted: number;
  thresholds: {
    min_roi_30d?: number;
    min_sharpe?: number;
    min_win_rate?: number;
    min_trades_30d?: number;
  };
  message?: string;
}

// Order Signing Types (for MetaMask trade signing)
export interface Eip712Domain {
  name: string;
  version: string;
  chainId: number;
  verifyingContract: string;
}

export interface Eip712Order {
  salt: string;
  maker: string;
  signer: string;
  taker: string;
  tokenId: string;
  makerAmount: string;
  takerAmount: string;
  expiration: string;
  nonce: string;
  feeRateBps: string;
  side: number;
  signatureType: number;
}

export interface Eip712TypedData {
  types: {
    EIP712Domain: Array<{ name: string; type: string }>;
    Order: Array<{ name: string; type: string }>;
  };
  primaryType: string;
  domain: Eip712Domain;
  message: Eip712Order;
}

export interface OrderSummary {
  side: string;
  outcome: string;
  price: string;
  size: string;
  total_cost: string;
  potential_payout: string;
}

export interface PrepareOrderRequest {
  token_id: string;
  side: "BUY" | "SELL";
  price: number;
  size: number;
  maker_address: string;
  neg_risk?: boolean;
  expires_in_secs?: number;
}

export interface PrepareOrderResponse {
  pending_order_id: string;
  typed_data: Eip712TypedData;
  expires_at: string;
  summary: OrderSummary;
}

export interface SubmitOrderRequest {
  pending_order_id: string;
  signature: string;
}

export interface SubmitOrderResponse {
  success: boolean;
  order_id?: string;
  message: string;
  tx_hash?: string;
}

// Service status types
export interface ServiceStatusItem {
  running: boolean;
  reason?: string;
}

export interface ServiceStatus {
  harvester: ServiceStatusItem;
  metrics_calculator: ServiceStatusItem;
  copy_trading: ServiceStatusItem;
  arb_executor: ServiceStatusItem;
  live_trading: ServiceStatusItem;
}

// Risk monitoring types
export type TripReason =
  | "daily_loss_limit"
  | "max_drawdown"
  | "consecutive_losses"
  | "manual"
  | "connectivity"
  | "market_conditions";

export interface CircuitBreakerConfig {
  max_daily_loss: number;
  max_drawdown_pct: number;
  max_consecutive_losses: number;
  cooldown_minutes: number;
  enabled: boolean;
}

export interface RecoveryState {
  current_stage: number;
  total_stages: number;
  capacity_pct: number;
  started_at: string;
  next_stage_at: string | null;
  trades_this_stage: number;
  recovery_pnl: number;
}

export interface CircuitBreakerStatus {
  tripped: boolean;
  trip_reason: TripReason | null;
  tripped_at: string | null;
  resume_at: string | null;
  daily_pnl: number;
  peak_value: number;
  current_value: number;
  consecutive_losses: number;
  trips_today: number;
  recovery_state: RecoveryState | null;
  config: CircuitBreakerConfig;
}

export interface RecentStopExecution {
  id: string;
  position_id: string;
  market_id: string;
  stop_type: string;
  executed_at: string;
}

export interface StopLossStats {
  total_rules: number;
  active_rules: number;
  executed_rules: number;
  fixed_stops: number;
  percentage_stops: number;
  trailing_stops: number;
  time_based_stops: number;
  recent_executions: RecentStopExecution[];
}

export interface RiskStatus {
  circuit_breaker: CircuitBreakerStatus;
  stop_loss: StopLossStats;
}
