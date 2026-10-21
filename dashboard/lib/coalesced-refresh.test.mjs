import { test } from 'node:test';
import assert from 'node:assert/strict';
import { coalescedRefresh } from './coalesced-refresh.ts';

test('bursts have a bounded cadence and retain a trailing refresh', async (t) => {
  t.mock.timers.enable({apis:['setTimeout','Date'],now:10000});
  let calls=0;
  const scheduler=coalescedRefresh(async()=>{calls++;});
  for(let i=0;i<100;i++) scheduler.notify();
  t.mock.timers.tick(0);
  await Promise.resolve();
  assert.equal(calls,1);
  for(let i=0;i<100;i++) scheduler.notify();
  t.mock.timers.tick(999);
  assert.equal(calls,1);
  t.mock.timers.tick(1);
  await Promise.resolve();
  assert.equal(calls,2);
  scheduler.dispose();
});

test('slow reads never overlap and disposing a market drops queued refreshes', async (t) => {
  t.mock.timers.enable({apis:['setTimeout','Date'],now:10000});
  let calls=0; let finish;
  const scheduler=coalescedRefresh(()=>{calls++;return new Promise(resolve=>{finish=resolve;});});
  scheduler.notify();t.mock.timers.tick(0);
  scheduler.notify();t.mock.timers.tick(5000);
  assert.equal(calls,1);
  finish();await Promise.resolve();
  t.mock.timers.tick(0);
  assert.equal(calls,2);
  scheduler.notify();scheduler.dispose();finish();await Promise.resolve();
  t.mock.timers.tick(5000);
  assert.equal(calls,2);
});
