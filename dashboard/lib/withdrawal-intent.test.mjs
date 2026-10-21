import { test } from 'node:test';
import assert from 'node:assert/strict';
import { withdrawalIntent } from './withdrawal-intent.ts';

test('withdrawal identity survives retry and reload, and changes only for a new intent', () => {
  const saved = new Map();
  const storage = { getItem: (key) => saved.get(key) ?? null, setItem: (key, value) => { saved.set(key,value); } };
  const first = withdrawalIntent(storage,'user/source/destination/10');
  assert.deepEqual(withdrawalIntent(storage,first.fingerprint,first),first);
  assert.deepEqual(withdrawalIntent(storage,first.fingerprint),first);
  assert.notEqual(withdrawalIntent(storage,'user/source/destination/20').id,first.id);
});
