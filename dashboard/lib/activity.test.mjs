import { test } from 'node:test';
import assert from 'node:assert/strict';
import { signalToActivity } from './activity.ts';

// signal_type is snake_case on the wire (websocket.rs SignalType): arbitrage | alert | quant_signal.
const signal = (overrides) => ({
  signal_id: 'sig-1',
  signal_type: 'arbitrage',
  market_id: '0x1234567890abcdef1234567890abcdef',
  outcome_id: 'both',
  action: 'executed',
  confidence: 1,
  timestamp: '2026-10-21T12:00:00Z',
  metadata: {},
  ...overrides,
});

test('arbitrage signals map by action', () => {
  const cases = [
    ['executed', 'ARB_POSITION_OPENED'],
    ['execution_failed', 'ARB_EXECUTION_FAILED'],
    ['closed_via_exit', 'ARB_POSITION_CLOSED'],
    ['closed_via_resolution', 'ARB_POSITION_CLOSED'],
    ['exit', 'ARB_POSITION_CLOSED'],
    ['entry', 'ARBITRAGE_DETECTED'],
  ];
  for (const [action, expected] of cases) {
    const activity = signalToActivity(signal({ action }));
    assert.equal(activity.type, expected, action);
    assert.notEqual(activity.message, '', action);
  }
});

test('the PascalCase signal_type the dashboard used to expect no longer matches', () => {
  // Pre-fix the switch only knew "Arbitrage"; the server never sent it, so every
  // arbitrage signal fell through with an empty message.
  assert.equal(signalToActivity(signal({ signal_type: 'Arbitrage', action: 'executed' })).message, '');
  assert.equal(signalToActivity(signal({ signal_type: 'arbitrage', action: 'executed' })).type, 'ARB_POSITION_OPENED');
});

test('alert signals map exit failures and generic alerts', () => {
  const failed = signalToActivity(signal({ signal_type: 'alert', action: 'exit_failed', metadata: { reason: 'YES sell order rejected' } }));
  assert.equal(failed.type, 'ARB_EXIT_FAILED');
  assert.equal(failed.message, 'Exit failed: YES sell order rejected');

  const generic = signalToActivity(signal({ signal_type: 'alert', action: 'alert', metadata: { message: 'hello' } }));
  assert.equal(generic.type, 'ARBITRAGE_DETECTED');
  assert.equal(generic.message, 'hello');
});

test('pnl comes from string metadata and the signal id and timestamp are preserved', () => {
  const activity = signalToActivity(signal({ action: 'closed_via_exit', metadata: { realized_pnl: '12.50' } }));
  assert.equal(activity.pnl, 12.5);
  assert.equal(activity.id, 'sig-1');
  assert.equal(activity.created_at, '2026-10-21T12:00:00Z');
  assert.equal(signalToActivity(signal({ metadata: { profit: '3' } })).pnl, 3);
  assert.equal(signalToActivity(signal({ metadata: {} })).pnl, undefined);
});
