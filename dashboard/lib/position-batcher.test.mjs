import { test, mock, afterEach } from 'node:test';
import assert from 'node:assert/strict';
import { createPositionBatcher } from './position-batcher.ts';

afterEach(() => mock.timers.reset());

function setup(debounceMs = 200) {
  mock.timers.enable({ apis: ['setTimeout'] });
  const delivered = [];
  const batcher = createPositionBatcher((acc) => delivered.push(acc), debounceMs);
  return { batcher, delivered };
}

test('price updates are merged per position and delivered once after the debounce', () => {
  const { batcher, delivered } = setup();
  batcher.addUpdate('p1', 'price_changed', { price: 0.5, pnl: 1, quantity: 10 });
  batcher.addUpdate('p1', 'updated', { price: 0.55 });
  batcher.addUpdate('p2', 'updated', { price: 0.3, pnl: -2, quantity: 5 });
  assert.equal(delivered.length, 0);

  mock.timers.tick(199);
  assert.equal(delivered.length, 0);
  mock.timers.tick(1);
  assert.equal(delivered.length, 1);
  assert.deepEqual([...delivered[0].updates], [
    ['p1', { price: 0.55, pnl: 1, quantity: 10 }],
    ['p2', { price: 0.3, pnl: -2, quantity: 5 }],
  ]);
  assert.deepEqual(delivered[0].opened, []);
  assert.deepEqual(delivered[0].closed, []);
});

test('opened and closed are tracked; closed supersedes a pending price update', () => {
  const { batcher, delivered } = setup();
  batcher.addUpdate('p1', 'opened');
  batcher.addUpdate('p2', 'price_changed', { price: 0.4, pnl: 0, quantity: 1 });
  batcher.addUpdate('p2', 'closed');
  mock.timers.tick(200);
  assert.deepEqual(delivered[0].opened, ['p1']);
  assert.deepEqual(delivered[0].closed, ['p2']);
  assert.equal(delivered[0].updates.size, 0);
});

test('regression: an empty flush must not strand later updates', () => {
  const { batcher, delivered } = setup();
  // An update with no payload schedules a flush but leaves nothing to deliver.
  batcher.addUpdate('p1', 'updated');
  mock.timers.tick(200);
  assert.equal(delivered.length, 0);

  // Before the fix the stale timer handle made every later update a no-op.
  batcher.addUpdate('p1', 'price_changed', { price: 0.7, pnl: 3, quantity: 2 });
  mock.timers.tick(200);
  assert.equal(delivered.length, 1);
  assert.deepEqual([...delivered[0].updates], [['p1', { price: 0.7, pnl: 3, quantity: 2 }]]);
});

test('an explicit empty flush also leaves the batcher usable', () => {
  const { batcher, delivered } = setup();
  batcher.flush();
  assert.equal(delivered.length, 0);
  batcher.addUpdate('p1', 'opened');
  mock.timers.tick(200);
  assert.equal(delivered.length, 1);
});

test('a manual flush delivers immediately and cancels the pending timer', () => {
  const { batcher, delivered } = setup();
  batcher.addUpdate('p1', 'opened');
  batcher.flush();
  assert.equal(delivered.length, 1);
  mock.timers.tick(1000);
  assert.equal(delivered.length, 1);

  // And a following update schedules a fresh flush.
  batcher.addUpdate('p2', 'closed');
  mock.timers.tick(200);
  assert.equal(delivered.length, 2);
  assert.deepEqual(delivered[1].closed, ['p2']);
});

test('dispose drops pending work and the batcher can be reused afterwards', () => {
  const { batcher, delivered } = setup();
  batcher.addUpdate('p1', 'opened');
  batcher.dispose();
  mock.timers.tick(1000);
  assert.equal(delivered.length, 0);

  batcher.addUpdate('p3', 'opened');
  mock.timers.tick(200);
  assert.deepEqual(delivered[0].opened, ['p3']);
});
