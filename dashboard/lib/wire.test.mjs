import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  toNumber,
  toNumberOr,
  parseMarket,
  parseOrderbook,
  parseOrder,
  parseBacktestResult,
  parseCircuitBreakerConfig,
  parseCircuitBreakerStatus,
  parseRiskStatus,
  parseActivity,
  parseWorkspace,
} from './wire.ts';

// Fixtures mirror what the Rust API serialises: rust_decimal::Decimal is a JSON
// string (scale preserved, e.g. "0.50"), f64 / integers are JSON numbers.

test('toNumber converts Decimal strings and guards non-numeric input', () => {
  assert.equal(toNumber('0.50'), 0.5);
  assert.equal(toNumber('-12.340000'), -12.34);
  assert.equal(toNumber(' 7 '), 7);
  assert.equal(toNumber(3), 3);
  assert.equal(toNumber(null), null);
  assert.equal(toNumber(undefined), null);
  assert.equal(toNumber(''), null);
  assert.equal(toNumber('  '), null);
  assert.equal(toNumber('abc'), null);
  assert.equal(toNumber('NaN'), null);
  assert.equal(toNumber(Number.NaN), null);
  assert.equal(toNumber(Infinity), null);
  assert.equal(toNumber('Infinity'), null);
  assert.equal(toNumber({}), null);
  assert.equal(toNumber(true), null);
  assert.equal(toNumberOr('x', 9), 9);
  assert.equal(toNumberOr('4.5', 9), 4.5);
});

test('parseMarket keeps unknown prices unknown instead of 0', () => {
  const market = parseMarket({
    id: 'm', question: 'Who wins?', category: 'sports', end_date: null,
    active: true, yes_price: null, no_price: '0.55', volume_24h: '1', liquidity: '1',
    created_at: '2024-01-01T00:00:00Z',
  });
  assert.equal(market.yes_price, null);
  assert.equal(market.no_price, 0.55);
  assert.equal(market.end_date, null);
});

test('parseMarket turns Decimal strings into numbers that support toFixed', () => {
  const market = parseMarket({
    id: 'm1', question: 'Q?', category: 'politics', end_date: '2026-12-31T00:00:00Z',
    active: true, yes_price: '0.45', no_price: '0.55', volume_24h: '12345.670000',
    liquidity: '900', created_at: '2026-01-01T00:00:00Z',
  });
  assert.equal(market.yes_price, 0.45);
  assert.equal(market.no_price, 0.55);
  assert.equal(market.volume_24h, 12345.67);
  assert.equal(market.liquidity, 900);
  assert.equal(market.question, 'Q?');
  assert.equal((market.yes_price * 100).toFixed(1), '45.0');
});

test('parseOrderbook converts levels and spreads; level.quantity.toFixed(0) no longer throws', () => {
  const raw = {
    market_id: 'm1', timestamp: '2026-10-19T00:00:00Z',
    yes_bids: [{ price: '0.40', quantity: '10' }, { price: '0.39', quantity: '250.5' }],
    yes_asks: [{ price: '0.50', quantity: '20' }],
    no_bids: [],
    no_asks: [{ price: '0.52', quantity: '5' }],
    spread: { yes_spread: '0.10', no_spread: '0', arb_spread: '0.02' },
  };
  // Prove the original defect: string quantities have no toFixed.
  assert.throws(() => raw.yes_bids[0].quantity.toFixed(0), TypeError);

  const book = parseOrderbook(raw);
  assert.deepEqual(book.yes_bids, [{ price: 0.4, quantity: 10 }, { price: 0.39, quantity: 250.5 }]);
  assert.equal(book.yes_bids[1].quantity.toFixed(0), '251');
  assert.deepEqual(book.yes_asks, [{ price: 0.5, quantity: 20 }]);
  assert.deepEqual(book.no_bids, []);
  assert.deepEqual(book.spread, { yes_spread: 0.1, no_spread: 0, arb_spread: 0.02 });
});

test('parseOrderbook omits an absent arb_spread and drops unreadable levels', () => {
  const book = parseOrderbook({
    market_id: 'm1', timestamp: 't',
    yes_bids: [{ price: 'bad', quantity: '1' }, { price: '0.4', quantity: null }, { price: '0.3', quantity: '2' }],
    yes_asks: undefined, no_bids: [], no_asks: [],
    spread: { yes_spread: '0', no_spread: '0' },
  });
  assert.deepEqual(book.yes_bids, [{ price: 0.3, quantity: 2 }]);
  assert.deepEqual(book.yes_asks, []);
  assert.equal(book.spread.arb_spread, undefined);
  assert.equal(book.spread.yes_spread, 0);
});

test('parseOrder converts quantities and optional prices', () => {
  const order = parseOrder({
    id: 'o1', market_id: 'm1', outcome: 'yes', side: 'buy', order_type: 'limit', status: 'partially_filled',
    quantity: '100', filled_quantity: '50.5', remaining_quantity: '49.5',
    price: '0.55', avg_fill_price: '0.54', time_in_force: 'GTC',
    created_at: 't', updated_at: 't',
  });
  assert.equal(order.quantity, 100);
  assert.equal(order.filled_quantity, 50.5);
  assert.equal(order.remaining_quantity, 49.5);
  assert.equal(order.price, 0.55);
  assert.equal(order.avg_fill_price, 0.54);
  assert.equal(order.stop_price, undefined);
  assert.equal(order.status, 'partially_filled');
});

const backtestFixture = () => ({
  id: 'b1',
  strategy: { type: 'arbitrage', min_spread: '0.02', max_position: '1000' },
  start_date: '2026-01-01T00:00:00Z', end_date: '2026-02-01T00:00:00Z',
  initial_capital: '1000', final_value: '1100.50', total_return: '100.50',
  total_return_pct: '0.1005', annualized_return: '1.201', sharpe_ratio: '1.85',
  sortino_ratio: '2.4', max_drawdown: '0.05', max_drawdown_pct: '5.0',
  total_trades: 12, winning_trades: 8, losing_trades: 4, win_rate: '0.667',
  avg_win: '25.5', avg_loss: '-10.25', profit_factor: '2.5', total_fees: '3.2',
  created_at: '2026-02-01T01:00:00Z', status: 'completed', trigger_mode: 'manual',
  expectancy: '8.37', calmar_ratio: '24.0', var_95: '-0.03', cvar_95: '-0.05',
  recovery_factor: '2.0', best_trade_return: '0.12', worst_trade_return: '-0.06',
  max_consecutive_wins: 4, max_consecutive_losses: 2, avg_trade_duration_hours: '3.5',
  equity_curve: [
    { timestamp: '2026-01-01T00:00:00Z', value: '1000', drawdown_pct: '0' },
    { timestamp: '2026-01-02T00:00:00Z', value: '1010.5', drawdown_pct: '0.5' },
  ],
  trade_log: [{
    market_id: 'm1', outcome_id: 'yes', trade_type: 'buy', entry_time: 't1', exit_time: 't2',
    entry_price: '0.4500', exit_price: '0.5', quantity: '100', fees: '0.9', pnl: '4.1', return_pct: 0.0911,
  }, {
    market_id: 'm2', outcome_id: 'no', trade_type: 'buy', entry_time: 't3',
    entry_price: '0.3', quantity: '10', fees: '0.1', return_pct: null,
  }],
});

test('parseBacktestResult converts every Decimal metric so page .toFixed calls work', () => {
  const raw = backtestFixture();
  assert.throws(() => raw.sharpe_ratio.toFixed(2), TypeError);

  const result = parseBacktestResult(raw);
  for (const key of [
    'initial_capital', 'final_value', 'total_return', 'total_return_pct', 'annualized_return',
    'sharpe_ratio', 'sortino_ratio', 'max_drawdown', 'max_drawdown_pct', 'win_rate', 'avg_win',
    'avg_loss', 'profit_factor', 'total_fees', 'expectancy', 'calmar_ratio', 'var_95', 'cvar_95',
    'recovery_factor', 'best_trade_return', 'worst_trade_return', 'avg_trade_duration_hours',
  ]) {
    assert.equal(typeof result[key], 'number', key);
  }
  assert.equal(result.sharpe_ratio.toFixed(2), '1.85');
  assert.equal(result.total_return_pct.toFixed(4), '0.1005');
  assert.equal(result.avg_loss, -10.25);
  assert.deepEqual(result.strategy, { type: 'arbitrage', min_spread: 0.02, max_position: 1000 });
  assert.equal(result.total_trades, 12);
  assert.equal(result.max_consecutive_wins, 4);
  assert.equal(result.status, 'completed');
  assert.equal(result.trigger_mode, 'manual');

  assert.deepEqual(result.equity_curve, [
    { timestamp: '2026-01-01T00:00:00Z', value: 1000, drawdown_pct: 0 },
    { timestamp: '2026-01-02T00:00:00Z', value: 1010.5, drawdown_pct: 0.5 },
  ]);
  const [closed, open] = result.trade_log;
  assert.equal(closed.entry_price.toFixed(4), '0.4500');
  assert.equal(closed.exit_price, 0.5);
  assert.equal(closed.quantity.toFixed(2), '100.00');
  assert.equal(closed.pnl, 4.1);
  assert.equal(closed.return_pct, 0.0911);
  assert.equal(open.exit_price, null);
  assert.equal(open.pnl, null);
  assert.equal(open.return_pct, null);
});

test('parseBacktestResult preserves null metrics (non-finite values) instead of coercing to 0 or NaN', () => {
  const raw = backtestFixture();
  raw.sharpe_ratio = null;
  raw.sortino_ratio = null;
  raw.profit_factor = null;
  raw.calmar_ratio = null;
  raw.win_rate = 'NaN';
  raw.equity_curve[1].value = null;
  delete raw.expectancy;

  const result = parseBacktestResult(raw);
  assert.equal(result.sharpe_ratio, null);
  assert.equal(result.sortino_ratio, null);
  assert.equal(result.profit_factor, null);
  assert.equal(result.calmar_ratio, null);
  assert.equal(result.win_rate, null);
  assert.equal(result.expectancy, null);
  assert.equal(result.equity_curve[1].value, null);
  // Untouched metrics still convert.
  assert.equal(result.total_return_pct, 0.1005);
});

test('parseBacktestResult tolerates a result without optional collections', () => {
  const raw = backtestFixture();
  delete raw.equity_curve;
  delete raw.trade_log;
  const result = parseBacktestResult(raw);
  assert.equal(result.equity_curve, undefined);
  assert.equal(result.trade_log, undefined);
});

test('risk status parsers convert Decimal fields and map a missing recovery_state to null', () => {
  const status = parseRiskStatus({
    circuit_breaker: {
      tripped: false, trip_reason: null, tripped_at: null, resume_at: null,
      daily_pnl: '-125.50', peak_value: '10500.00', current_value: '10200.25',
      consecutive_losses: 1, trips_today: 0,
      config: { max_daily_loss: '2500', max_drawdown_pct: '0.20', max_consecutive_losses: 8, cooldown_minutes: 30, enabled: true },
    },
    stop_loss: { total_rules: 0, active_rules: 0, executed_rules: 0, fixed_stops: 0, percentage_stops: 0, trailing_stops: 0, time_based_stops: 0, recent_executions: [] },
  });
  const cb = status.circuit_breaker;
  assert.equal(cb.daily_pnl, -125.5);
  assert.equal(cb.peak_value, 10500);
  assert.equal(cb.current_value, 10200.25);
  assert.equal(cb.recovery_state, null);
  // An API without the newer fields: unknown, not "off" or "unseeded".
  assert.equal(cb.config.hard_kill_drawdown_pct, null);
  assert.equal(cb.equity_seeded, undefined);
  assert.equal(cb.drawdown_peak, 10500);
  assert.equal(cb.config.max_daily_loss, 2500);
  assert.equal(cb.config.max_drawdown_pct, 0.2);
  assert.equal(cb.config.max_consecutive_losses, 8);
  assert.equal(status.stop_loss.total_rules, 0);
});

test('parseCircuitBreakerStatus converts recovery state decimals', () => {
  const cb = parseCircuitBreakerStatus({
    tripped: true, trip_reason: 'manual', tripped_at: 't', resume_at: 't2',
    daily_pnl: '0', peak_value: '1', drawdown_peak: '0.9', current_value: '1', equity_seeded: true, consecutive_losses: 0, trips_today: 1,
    recovery_state: { current_stage: 1, total_stages: 4, capacity_pct: '0.25', started_at: 't', next_stage_at: null, trades_this_stage: 2, recovery_pnl: '-3.5' },
    config: { max_daily_loss: '100', max_drawdown_pct: '0.1', max_consecutive_losses: 3, cooldown_minutes: 5, enabled: false, hard_kill_drawdown_pct: '0' },
  });
  assert.equal(cb.drawdown_peak, 0.9);
  assert.equal(cb.equity_seeded, true);
  assert.equal(cb.config.hard_kill_drawdown_pct, 0);
  assert.equal(cb.recovery_state.capacity_pct, 0.25);
  assert.equal(cb.recovery_state.recovery_pnl, -3.5);
  assert.equal((cb.recovery_state.capacity_pct * 100).toFixed(0), '25');
});

test('parseCircuitBreakerConfig converts the config response', () => {
  const cfg = parseCircuitBreakerConfig({ max_daily_loss: '2500', max_drawdown_pct: '0.20', max_consecutive_losses: 8, cooldown_minutes: 30, enabled: true, hard_kill_drawdown_pct: '0.40' });
  assert.deepEqual(cfg, { max_daily_loss: 2500, max_drawdown_pct: 0.2, max_consecutive_losses: 8, cooldown_minutes: 30, enabled: true, hard_kill_drawdown_pct: 0.4 });
});

test('parseActivity converts an optional pnl and leaves other fields', () => {
  const withPnl = parseActivity({ id: '1', type: 'ARB_POSITION_CLOSED', message: 'm', pnl: '12.50', created_at: 't' });
  assert.equal(withPnl.pnl, 12.5);
  const without = parseActivity({ id: '2', type: 'POSITION_OPENED', message: 'm', created_at: 't' });
  assert.equal(without.pnl, undefined);
  assert.equal(without.message, 'm');
});

test('parseWorkspace converts budget fields and optional thresholds', () => {
  const ws = parseWorkspace({
    id: 'w', name: 'n', setup_mode: 'manual', total_budget: '10000.00', reserved_cash_pct: '0.20',
    auto_optimize_enabled: false, optimization_interval_hours: 24, min_roi_30d: '5.5', min_sharpe: null,
    arb_auto_execute: false, live_trading_enabled: false, exit_handler_enabled: false,
    inactivity_days: 7, my_role: 'owner', created_at: 't', updated_at: 't',
  });
  assert.equal(ws.total_budget, 10000);
  assert.equal(ws.reserved_cash_pct, 0.2);
  assert.equal(ws.min_roi_30d, 5.5);
  assert.equal(ws.min_sharpe, undefined);
  assert.equal(ws.min_win_rate, undefined);
  assert.equal(ws.optimization_interval_hours, 24);
});
