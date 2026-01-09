// Position types
export type PositionSide = 'long' | 'short';
export type PositionOutcome = 'yes' | 'no';
export type PositionStatus = 'open' | 'closed' | 'all';

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
  opened_at: string;
  updated_at: string;
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
    category: 'HIGH_POTENTIAL' | 'MODERATE' | 'LOW_POTENTIAL' | 'INSUFFICIENT_DATA';
  };
}

// Order types
export type OrderSide = 'Buy' | 'Sell';
export type OrderType = 'Market' | 'Limit' | 'StopLoss' | 'TakeProfit';
export type OrderStatus = 'Pending' | 'Open' | 'PartiallyFilled' | 'Filled' | 'Cancelled' | 'Rejected' | 'Expired';

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
  time_in_force: string;
  created_at: string;
  updated_at: string;
  filled_at?: string;
}

// Backtest types
export type StrategyType = 'Arbitrage' | 'Momentum' | 'MeanReversion' | 'CopyTrading';
export type SlippageModel = 'None' | 'Fixed' | 'VolumeBased';

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
  // CopyTrading
  wallets?: string[];
  allocation_pct?: number;
}

export interface BacktestParams {
  strategy: StrategyConfig;
  start_date: string;
  end_date: string;
  initial_capital: number;
  markets?: string[];
  slippage_model?: SlippageModel;
  slippage_pct?: number;
  fee_pct?: number;
}

export interface EquityPoint {
  timestamp: string;
  value: number;
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
  status: 'pending' | 'running' | 'completed' | 'failed';
  error?: string;
  equity_curve?: EquityPoint[];
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
export type AllocationStrategy = 'EQUAL_WEIGHT' | 'PERFORMANCE_WEIGHTED' | 'RISK_ADJUSTED' | 'CUSTOM';

export interface StrategyAllocation {
  strategy_id: string;
  strategy_type: 'WALLET' | 'ARBITRAGE';
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
  | 'TRADE_COPIED'
  | 'POSITION_OPENED'
  | 'POSITION_CLOSED'
  | 'STOP_LOSS_TRIGGERED'
  | 'TAKE_PROFIT_TRIGGERED'
  | 'ARBITRAGE_DETECTED'
  | 'RECOMMENDATION_NEW'
  | 'ALLOCATION_ACTIVATED';

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
  type: 'COPY_WALLET' | 'ARBITRAGE' | 'POSITION_EXIT';
  confidence: number;
  wallet?: Wallet;
  expected_return?: number;
  risk_level: 'LOW' | 'MEDIUM' | 'HIGH';
  reason: string;
  created_at: string;
  expires_at?: string;
}

// Trade classification types (Event vs Arb)
export type TradeClassification = 'event' | 'arbitrage' | 'mixed';
export type TradingStyle = 'event_trader' | 'arb_trader' | 'mixed';
export type CopyBehavior = 'copy_all' | 'events_only' | 'arb_threshold';
export type WalletTier = 'active' | 'bench' | 'untracked';

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
  slippage_tolerance: 'tight' | 'moderate' | 'loose';
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

// Demo mode types
export interface DemoBalance {
  balance: number;
  initial_balance: number;
  pnl: number;
  pnl_percent: number;
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
export type PositionUpdateType = 'Opened' | 'Updated' | 'Closed' | 'PriceChanged';
export type SignalType = 'Arbitrage' | 'CopyTrade' | 'StopLoss' | 'TakeProfit' | 'Alert';

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
  | { type: 'Orderbook'; data: OrderbookUpdate }
  | { type: 'Position'; data: PositionUpdate }
  | { type: 'Signal'; data: SignalUpdate }
  | { type: 'Subscribed'; channel: string }
  | { type: 'Unsubscribed'; channel: string }
  | { type: 'Error'; code: string; message: string }
  | { type: 'Ping' }
  | { type: 'Pong' };
