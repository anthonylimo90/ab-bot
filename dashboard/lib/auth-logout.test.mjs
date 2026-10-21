import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import vm from 'node:vm';
import ts from 'typescript';
const require = createRequire(import.meta.url);

function loadStore(revoke) {
  const calls = [];
  class ApiHttpError extends Error { get isUnauthorized() { return true; } }
  const api = {
    setToken: () => {},
    clearToken: () => calls.push('clear'),
    logout: async () => { calls.push('revoke'); await revoke(); },
  };
  const resetStore = { getState: () => ({ reset() {} }) };
  const mocks = {
    '@/lib/api': { __esModule: true, default: api, ApiHttpError },
    '@/lib/queryClient': { queryClient: { clear: () => calls.push('reset') } },
    '@/stores/workspace-store': { useWorkspaceStore: resetStore },
    '@/stores/wallet-store': { useWalletStore: resetStore },
    '@/stores/toast-store': { useToastStore: { getState: () => ({ error: () => calls.push('error') }) } },
  };
  const source = ts.transpileModule(readFileSync(new URL('../stores/auth-store.ts', import.meta.url), 'utf8'),
    { compilerOptions: { module: ts.ModuleKind.CommonJS } }).outputText;
  const context = { exports: {}, require: name => mocks[name] ?? require(name),
    localStorage: { getItem: () => null, setItem() {}, removeItem() {} } };
  vm.runInNewContext(source, context);
  const store = context.exports.useAuthStore;
  store.getState().setAuth('copied-token', { id: 'user' });
  calls.length = 0;
  return { store, calls };
}

test('actual auth store revokes before clearing browser session', async () => {
  let finish;
  const { store, calls } = loadStore(() => new Promise(resolve => { finish = resolve; }));
  const pending = store.getState().logout();
  assert.equal(store.getState().token, 'copied-token');
  assert.deepEqual(calls, ['revoke']);
  finish();
  assert.equal(await pending, true);
  assert.deepEqual(calls, ['revoke', 'clear', 'reset']);
  assert.equal(store.getState().token, null);
});

test('failed server revocation keeps session available for retry and reports failure', async () => {
  const { store, calls } = loadStore(async () => { throw new Error('network'); });
  assert.equal(await store.getState().logout(), false);
  assert.equal(store.getState().token, 'copied-token');
  assert.deepEqual(calls, ['revoke', 'error']);
});
