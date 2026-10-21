// Decoding of values that cross the Rust -> browser boundary.
//
// The backend serialises `rust_decimal::Decimal` as a JSON *string* (for example
// "0.45") because the workspace enables only the default `serde` feature, whereas
// `f64` fields are JSON numbers (and become `null` when non-finite). The TypeScript
// response types declare these fields as `number`, so every response and WebSocket
// payload that carries Decimal fields has to be coerced here, at the boundary, before
// it reaches the query cache or a component that calls `.toFixed()` on it.
//
// This module is deliberately dependency-free (type imports only) so it can be unit
// tested directly with `node --experimental-strip-types`.

import type {
  Activity,
  BacktestResult,
  CircuitBreakerConfig,
  CircuitBreakerStatus,
  EquityPoint,
  Market,
  Order,
  Orderbook,
  OrderbookUpdate,
  PositionUpdate,
  PositionUpdateType,
  PriceLevel,
  RecoveryState,
  RiskStatus,
  SignalUpdate,
  StrategyConfig,
  TradeFlowEvent,
  TradeLogEntry,
  WebSocketMessage,
  Workspace,
} from "@/types/api";

/**
 * Convert a JSON number or Decimal string to a finite number.
 * Returns `null` for null/undefined, blank strings, non-numeric values and
 * non-finite results (NaN / Infinity), so callers never propagate NaN.
 */
export function toNumber(value: unknown): number | null {
  if (typeof value === "number") return Number.isFinite(value) ? value : null;
  if (typeof value !== "string") return null;
  const trimmed = value.trim();
  if (trimmed === "") return null;
  const parsed = Number(trimmed);
  return Number.isFinite(parsed) ? parsed : null;
}

/** Like {@link toNumber} for fields the API always sends and the UI types as `number`. */
export function toNumberOr(value: unknown, fallback = 0): number {
  return toNumber(value) ?? fallback;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

// ---------------------------------------------------------------------------
// REST response parsers
// ---------------------------------------------------------------------------

export function parseMarket(raw: Market): Market {
  return {
    ...raw,
    yes_price: toNumberOr(raw.yes_price),
    no_price: toNumberOr(raw.no_price),
    volume_24h: toNumberOr(raw.volume_24h),
    liquidity: toNumberOr(raw.liquidity),
  };
}

function parsePriceLevels(levels: PriceLevel[] | null | undefined): PriceLevel[] {
  if (!Array.isArray(levels)) return [];
  const parsed: PriceLevel[] = [];
  for (const level of levels) {
    const price = toNumber(level?.price);
    const quantity = toNumber(level?.quantity);
    // A level we cannot read is dropped rather than shown as a bogus price of 0.
    if (price !== null && quantity !== null) parsed.push({ ...level, price, quantity });
  }
  return parsed;
}

export function parseOrderbook(raw: Orderbook): Orderbook {
  const spread = raw.spread;
  return {
    ...raw,
    yes_bids: parsePriceLevels(raw.yes_bids),
    yes_asks: parsePriceLevels(raw.yes_asks),
    no_bids: parsePriceLevels(raw.no_bids),
    no_asks: parsePriceLevels(raw.no_asks),
    spread: {
      ...spread,
      yes_spread: toNumberOr(spread?.yes_spread),
      no_spread: toNumberOr(spread?.no_spread),
      arb_spread: toNumber(spread?.arb_spread) ?? undefined,
    },
  };
}

export function parseOrder(raw: Order): Order {
  return {
    ...raw,
    quantity: toNumberOr(raw.quantity),
    filled_quantity: toNumberOr(raw.filled_quantity),
    remaining_quantity: toNumberOr(raw.remaining_quantity),
    price: toNumber(raw.price) ?? undefined,
    avg_fill_price: toNumber(raw.avg_fill_price) ?? undefined,
    stop_price: toNumber(raw.stop_price) ?? undefined,
  };
}

const STRATEGY_NUMERIC_KEYS = [
  "min_spread",
  "max_position",
  "lookback_hours",
  "threshold",
  "position_size",
  "window_hours",
  "std_threshold",
  "grid_levels",
  "grid_spacing_pct",
  "order_size",
] as const;

function parseStrategyConfig(raw: StrategyConfig): StrategyConfig {
  const parsed: StrategyConfig = { ...raw };
  for (const key of STRATEGY_NUMERIC_KEYS) {
    if (raw[key] === undefined) continue;
    const value = toNumber(raw[key]);
    if (value === null) delete parsed[key];
    else parsed[key] = value;
  }
  return parsed;
}

function parseEquityPoint(raw: EquityPoint): EquityPoint {
  return {
    ...raw,
    value: toNumber(raw.value),
    drawdown_pct: toNumber(raw.drawdown_pct),
  };
}

function parseTradeLogEntry(raw: TradeLogEntry): TradeLogEntry {
  return {
    ...raw,
    entry_price: toNumber(raw.entry_price),
    exit_price: toNumber(raw.exit_price),
    quantity: toNumber(raw.quantity),
    fees: toNumber(raw.fees),
    pnl: toNumber(raw.pnl),
    return_pct: toNumber(raw.return_pct),
  };
}

/**
 * Backtest metrics are Decimal strings, and may be `null` when the backend cannot
 * represent a non-finite value (for example a Sharpe ratio with zero variance).
 * Null is preserved so the UI can render it as an em dash.
 */
export function parseBacktestResult(raw: BacktestResult): BacktestResult {
  return {
    ...raw,
    strategy: raw.strategy ? parseStrategyConfig(raw.strategy) : raw.strategy,
    initial_capital: toNumber(raw.initial_capital),
    final_value: toNumber(raw.final_value),
    total_return: toNumber(raw.total_return),
    total_return_pct: toNumber(raw.total_return_pct),
    annualized_return: toNumber(raw.annualized_return),
    sharpe_ratio: toNumber(raw.sharpe_ratio),
    sortino_ratio: toNumber(raw.sortino_ratio),
    max_drawdown: toNumber(raw.max_drawdown),
    max_drawdown_pct: toNumber(raw.max_drawdown_pct),
    win_rate: toNumber(raw.win_rate),
    avg_win: toNumber(raw.avg_win),
    avg_loss: toNumber(raw.avg_loss),
    profit_factor: toNumber(raw.profit_factor),
    total_fees: toNumber(raw.total_fees),
    expectancy: toNumber(raw.expectancy),
    calmar_ratio: toNumber(raw.calmar_ratio),
    var_95: toNumber(raw.var_95),
    cvar_95: toNumber(raw.cvar_95),
    recovery_factor: toNumber(raw.recovery_factor),
    best_trade_return: toNumber(raw.best_trade_return),
    worst_trade_return: toNumber(raw.worst_trade_return),
    avg_trade_duration_hours: toNumber(raw.avg_trade_duration_hours),
    equity_curve: raw.equity_curve?.map(parseEquityPoint),
    trade_log: raw.trade_log?.map(parseTradeLogEntry),
  };
}

export function parseCircuitBreakerConfig(raw: CircuitBreakerConfig): CircuitBreakerConfig {
  const parsed: CircuitBreakerConfig = {
    ...raw,
    max_daily_loss: toNumberOr(raw.max_daily_loss),
    max_drawdown_pct: toNumberOr(raw.max_drawdown_pct),
  };
  if (raw.hard_kill_drawdown_pct !== undefined) {
    parsed.hard_kill_drawdown_pct = toNumber(raw.hard_kill_drawdown_pct) ?? undefined;
  }
  return parsed;
}

function parseRecoveryState(raw: RecoveryState): RecoveryState {
  return {
    ...raw,
    capacity_pct: toNumberOr(raw.capacity_pct),
    recovery_pnl: toNumberOr(raw.recovery_pnl),
  };
}

export function parseCircuitBreakerStatus(raw: CircuitBreakerStatus): CircuitBreakerStatus {
  return {
    ...raw,
    daily_pnl: toNumberOr(raw.daily_pnl),
    peak_value: toNumberOr(raw.peak_value),
    current_value: toNumberOr(raw.current_value),
    // The API omits the field entirely when the breaker is not recovering.
    recovery_state: raw.recovery_state ? parseRecoveryState(raw.recovery_state) : null,
    config: parseCircuitBreakerConfig(raw.config),
  };
}

export function parseRiskStatus(raw: RiskStatus): RiskStatus {
  return {
    ...raw,
    circuit_breaker: parseCircuitBreakerStatus(raw.circuit_breaker),
  };
}

export function parseActivity(raw: Activity): Activity {
  return { ...raw, pnl: toNumber(raw.pnl) ?? undefined };
}

export function parseWorkspace(raw: Workspace): Workspace {
  return {
    ...raw,
    total_budget: toNumberOr(raw.total_budget),
    reserved_cash_pct: toNumberOr(raw.reserved_cash_pct),
    min_roi_30d: toNumber(raw.min_roi_30d) ?? undefined,
    min_sharpe: toNumber(raw.min_sharpe) ?? undefined,
    min_win_rate: toNumber(raw.min_win_rate) ?? undefined,
  };
}

// ---------------------------------------------------------------------------
// WebSocket messages
// ---------------------------------------------------------------------------

const POSITION_UPDATE_TYPES: readonly PositionUpdateType[] = [
  "opened",
  "updated",
  "closed",
  "price_changed",
];

export function isPositionUpdateType(value: unknown): value is PositionUpdateType {
  return POSITION_UPDATE_TYPES.includes(value as PositionUpdateType);
}

const TRADE_FLOW_DECIMAL_KEYS = [
  "expected_edge",
  "observed_edge",
  "requested_size_usd",
  "filled_size_usd",
  "fill_price",
  "realized_pnl",
  "unrealized_pnl",
] as const;

function normalizePositionUpdate(data: unknown): PositionUpdate | null {
  if (!isRecord(data) || typeof data.position_id !== "string") return null;
  if (!isPositionUpdateType(data.update_type)) return null;
  const quantity = toNumber(data.quantity);
  const currentPrice = toNumber(data.current_price);
  const unrealizedPnl = toNumber(data.unrealized_pnl);
  // A price or P&L we cannot read must never reach the query cache.
  if (quantity === null || currentPrice === null || unrealizedPnl === null) return null;
  return {
    position_id: data.position_id,
    market_id: typeof data.market_id === "string" ? data.market_id : "",
    update_type: data.update_type,
    quantity,
    current_price: currentPrice,
    unrealized_pnl: unrealizedPnl,
    timestamp: typeof data.timestamp === "string" ? data.timestamp : "",
  };
}

function normalizeOrderbookUpdate(data: unknown): OrderbookUpdate | null {
  if (!isRecord(data) || typeof data.market_id !== "string") return null;
  const yesBid = toNumber(data.yes_bid);
  const yesAsk = toNumber(data.yes_ask);
  const noBid = toNumber(data.no_bid);
  const noAsk = toNumber(data.no_ask);
  if (yesBid === null || yesAsk === null || noBid === null || noAsk === null) return null;
  return {
    market_id: data.market_id,
    timestamp: typeof data.timestamp === "string" ? data.timestamp : "",
    yes_bid: yesBid,
    yes_ask: yesAsk,
    no_bid: noBid,
    no_ask: noAsk,
    arb_spread: toNumber(data.arb_spread) ?? undefined,
  };
}

function normalizeTradeFlowEvent(data: unknown): TradeFlowEvent | null {
  if (!isRecord(data) || typeof data.id !== "string") return null;
  const event = { ...data } as unknown as TradeFlowEvent;
  for (const key of TRADE_FLOW_DECIMAL_KEYS) {
    event[key] = toNumber(data[key]);
  }
  event.confidence = toNumber(data.confidence);
  return event;
}

function normalizeSignalUpdate(data: unknown): SignalUpdate | null {
  if (!isRecord(data) || typeof data.signal_id !== "string") return null;
  return {
    ...data,
    confidence: toNumberOr(data.confidence),
  } as unknown as SignalUpdate;
}

/**
 * Validate a parsed WebSocket frame and convert its Decimal strings to numbers.
 *
 * Returns `null` for frames we do not recognise or whose numeric payload cannot be
 * read, so a malformed price never reaches a cache or component. Envelope shape
 * follows `WsMessage` in crates/api-server/src/websocket.rs:
 * `{ "type": "Position", "data": { ... } }`, with unit variants (Ping/Pong) carrying
 * no `data`.
 */
export function normalizeWsMessage(raw: unknown): WebSocketMessage | null {
  if (!isRecord(raw) || typeof raw.type !== "string") return null;
  const data = raw.data;
  switch (raw.type) {
    case "Position": {
      const update = normalizePositionUpdate(data);
      return update && { type: "Position", data: update };
    }
    case "Orderbook": {
      const update = normalizeOrderbookUpdate(data);
      return update && { type: "Orderbook", data: update };
    }
    case "Signal": {
      const update = normalizeSignalUpdate(data);
      return update && { type: "Signal", data: update };
    }
    case "TradeFlow": {
      const event = normalizeTradeFlowEvent(data);
      return event && { type: "TradeFlow", data: event };
    }
    case "Subscribed":
    case "Unsubscribed":
      return isRecord(data) && typeof data.channel === "string"
        ? { type: raw.type, data: { channel: data.channel } }
        : null;
    case "Error":
      return isRecord(data) &&
        typeof data.code === "string" &&
        typeof data.message === "string"
        ? { type: "Error", data: { code: data.code, message: data.message } }
        : null;
    case "Ping":
    case "Pong":
      return { type: raw.type };
    default:
      return null;
  }
}
