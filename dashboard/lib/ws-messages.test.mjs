import { test } from 'node:test';
import assert from 'node:assert/strict';
import { normalizeWsMessage, isPositionUpdateType } from './wire.ts';

// Frames are shaped exactly as crates/api-server/src/websocket.rs serialises them:
// WsMessage is `#[serde(tag = "type", content = "data")]`, PositionUpdateType and
// SignalType use `rename_all = "snake_case"`, and Decimal fields are strings.

const positionFrame = (overrides = {}) => ({
  type: 'Position',
  data: {
    position_id: '7d444840-9dc0-11d1-b245-5ffdce74fad2',
    market_id: 'market1',
    update_type: 'price_changed',
    quantity: '100',
    current_price: '0.50',
    unrealized_pnl: '-1.250000',
    timestamp: '2026-10-21T12:00:00Z',
    ...overrides,
  },
});

test('Position frames: Decimal strings become numbers and every snake_case variant is accepted', () => {
  for (const updateType of ['opened', 'updated', 'closed', 'price_changed']) {
    const message = normalizeWsMessage(positionFrame({ update_type: updateType }));
    assert.equal(message?.type, 'Position', updateType);
    assert.equal(message.data.update_type, updateType);
  }
  const { data } = normalizeWsMessage(positionFrame());
  assert.equal(data.quantity, 100);
  assert.equal(data.current_price, 0.5);
  assert.equal(data.unrealized_pnl, -1.25);
  assert.equal(data.current_price.toFixed(2), '0.50');
  assert.equal(data.position_id, '7d444840-9dc0-11d1-b245-5ffdce74fad2');
  assert.equal(data.market_id, 'market1');
  assert.equal(data.timestamp, '2026-10-21T12:00:00Z');
});

test('Position frames with the old PascalCase variants or unreadable numbers are rejected', () => {
  for (const legacy of ['Opened', 'Updated', 'Closed', 'PriceChanged', 'bogus']) {
    assert.equal(normalizeWsMessage(positionFrame({ update_type: legacy })), null, legacy);
    assert.equal(isPositionUpdateType(legacy), false, legacy);
  }
  assert.equal(normalizeWsMessage(positionFrame({ current_price: 'abc' })), null);
  assert.equal(normalizeWsMessage(positionFrame({ unrealized_pnl: null })), null);
  assert.equal(normalizeWsMessage(positionFrame({ quantity: undefined })), null);
  assert.equal(normalizeWsMessage(positionFrame({ position_id: 5 })), null);
  assert.equal(normalizeWsMessage({ type: 'Position' }), null);
});

test('Orderbook frames convert prices and map a null arb_spread to undefined', () => {
  const base = { market_id: 'market1', timestamp: 't', yes_bid: '0.48', yes_ask: '0.50', no_bid: '0.48', no_ask: '0.50' };
  const withSpread = normalizeWsMessage({ type: 'Orderbook', data: { ...base, arb_spread: '0.02' } });
  assert.deepEqual(withSpread.data, { ...base, yes_bid: 0.48, yes_ask: 0.5, no_bid: 0.48, no_ask: 0.5, arb_spread: 0.02 });
  const noSpread = normalizeWsMessage({ type: 'Orderbook', data: { ...base, arb_spread: null } });
  assert.equal(noSpread.data.arb_spread, undefined);
  assert.equal(normalizeWsMessage({ type: 'Orderbook', data: { ...base, yes_bid: 'x' } }), null);
});

test('Signal frames keep snake_case signal_type and numeric confidence', () => {
  const message = normalizeWsMessage({
    type: 'Signal',
    data: {
      signal_id: 'id1', signal_type: 'arbitrage', market_id: 'm', outcome_id: 'both',
      action: 'executed', confidence: 0.9, timestamp: 't', metadata: { realized_pnl: '1.5' },
    },
  });
  assert.equal(message.type, 'Signal');
  assert.equal(message.data.signal_type, 'arbitrage');
  assert.equal(message.data.confidence, 0.9);
  assert.deepEqual(message.data.metadata, { realized_pnl: '1.5' });
  assert.equal(normalizeWsMessage({ type: 'Signal', data: {} }), null);
});

test('TradeFlow frames convert optional Decimal fields and preserve null', () => {
  const message = normalizeWsMessage({
    type: 'TradeFlow',
    data: {
      id: 'e1', occurred_at: 't', strategy: 'arb', execution_mode: 'paper', source: 'arb', market_id: 'm',
      position_id: null, signal_id: null, event_type: 'position_closed', state_from: 'closing', state_to: 'closed',
      reason: null, direction: null, confidence: 0.7, expected_edge: '0.02', observed_edge: null,
      requested_size_usd: '100', filled_size_usd: '99.5', fill_price: '0.49', realized_pnl: '-3.25',
      unrealized_pnl: null, metadata: {},
    },
  });
  const { data } = message;
  assert.equal(data.expected_edge, 0.02);
  assert.equal(data.observed_edge, null);
  assert.equal(data.requested_size_usd, 100);
  assert.equal(data.filled_size_usd, 99.5);
  assert.equal(data.fill_price, 0.49);
  assert.equal(data.realized_pnl, -3.25);
  assert.equal(data.unrealized_pnl, null);
  assert.equal(data.confidence, 0.7);
  assert.equal(data.strategy, 'arb');
  assert.equal(data.realized_pnl.toFixed(2), '-3.25');
});

test('control frames use the data envelope; unit variants carry no data', () => {
  assert.deepEqual(normalizeWsMessage({ type: 'Subscribed', data: { channel: 'positions' } }), { type: 'Subscribed', data: { channel: 'positions' } });
  assert.deepEqual(normalizeWsMessage({ type: 'Unsubscribed', data: { channel: 'positions' } }), { type: 'Unsubscribed', data: { channel: 'positions' } });
  assert.deepEqual(normalizeWsMessage({ type: 'Error', data: { code: 'LAGGED', message: 'Missed 3 positions updates' } }), { type: 'Error', data: { code: 'LAGGED', message: 'Missed 3 positions updates' } });
  assert.deepEqual(normalizeWsMessage({ type: 'Pong' }), { type: 'Pong' });
  assert.deepEqual(normalizeWsMessage({ type: 'Ping' }), { type: 'Ping' });
  // The pre-fix TypeScript shape (channel at the top level) is not what the server sends.
  assert.equal(normalizeWsMessage({ type: 'Subscribed', channel: 'positions' }), null);
});

test('non-object and unknown frames are ignored', () => {
  for (const frame of [null, undefined, 'x', 5, [], {}, { type: 5 }, { type: 'Mystery', data: {} }]) {
    assert.equal(normalizeWsMessage(frame), null, JSON.stringify(frame));
  }
});
