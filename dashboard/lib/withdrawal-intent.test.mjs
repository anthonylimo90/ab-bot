import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  withdrawalIntent,
  isWithdrawalIntentSettled,
  clearWithdrawalIntent,
  WITHDRAWAL_INTENT_KEY,
} from './withdrawal-intent.ts';

test('withdrawal identity survives retry and reload, and changes only for a new intent', () => {
  const saved = new Map();
  const storage = { getItem: (key) => saved.get(key) ?? null, setItem: (key, value) => { saved.set(key,value); } };
  const first = withdrawalIntent(storage,'user/source/destination/10');
  assert.deepEqual(withdrawalIntent(storage,first.fingerprint,first),first);
  assert.deepEqual(withdrawalIntent(storage,first.fingerprint),first);
  assert.notEqual(withdrawalIntent(storage,'user/source/destination/20').id,first.id);
});

test('only terminal outcomes settle the intent; pending, submitted and unknown keep it', () => {
  for (const status of ['confirmed', 'failed', 'FAILED', ' Confirmed ']) {
    assert.equal(isWithdrawalIntentSettled(status), true, status);
  }
  for (const status of ['pending', 'submitted', 'receipt_pending', 'processing', 'unknown', '', null, undefined]) {
    assert.equal(isWithdrawalIntentSettled(status), false, String(status));
  }
});

test('after a terminal failure a retry of the same withdrawal creates a new attempt id', () => {
  const saved = new Map();
  const storage = {
    getItem: (key) => saved.get(key) ?? null,
    setItem: (key, value) => { saved.set(key, value); },
    removeItem: (key) => { saved.delete(key); },
  };
  const fingerprint = 'user/source/destination/10';
  const first = withdrawalIntent(storage, fingerprint);

  // Unresolved outcome: a retry reuses the same request id, in memory and after a reload.
  assert.equal(isWithdrawalIntentSettled('pending'), false);
  assert.equal(withdrawalIntent(storage, fingerprint, first).id, first.id);
  assert.equal(withdrawalIntent(storage, fingerprint).id, first.id);

  // Terminal failure: the page clears the persisted and in-memory intent.
  assert.equal(isWithdrawalIntentSettled('failed'), true);
  clearWithdrawalIntent(storage);
  assert.equal(saved.has(WITHDRAWAL_INTENT_KEY), false);
  const retry = withdrawalIntent(storage, fingerprint, null);
  assert.equal(retry.fingerprint, fingerprint);
  assert.notEqual(retry.id, first.id);
});

test('clearing the intent tolerates unavailable storage', () => {
  assert.doesNotThrow(() => clearWithdrawalIntent({ removeItem: () => { throw new Error('denied'); } }));
});
