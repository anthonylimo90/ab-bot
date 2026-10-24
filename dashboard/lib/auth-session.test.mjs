import { test } from 'node:test';
import assert from 'node:assert/strict';
import { loadTsModule } from './test-helpers.mjs';

// The real ApiHttpError, so `instanceof` and `isUnauthorized` behave as in the app.
const { ApiHttpError } = loadTsModule('lib/api.ts', { globals: { fetch: async () => { throw new Error('unused'); } } });

function loadStore({ getCurrentUser }) {
  const calls = [];
  const toasts = [];
  let session = 0;
  const api = {
    beginSession: () => { session += 1; calls.push('new-session'); },
    currentSession: () => session,
    logout: async () => {},
    getCurrentUser,
  };
  const resetStore = { getState: () => ({ reset: () => calls.push('reset-store') }) };
  const mocks = {
    '@/lib/api': { __esModule: true, default: api, ApiHttpError },
    '@/lib/queryClient': { queryClient: { clear: () => calls.push('clear-queries') } },
    '@/stores/workspace-store': { useWorkspaceStore: resetStore },
    '@/stores/wallet-store': { useWalletStore: resetStore },
    '@/stores/toast-store': {
      useToastStore: {
        getState: () => ({
          error: (title) => toasts.push(`error:${title}`),
          warning: (title) => toasts.push(`warning:${title}`),
        }),
      },
    },
  };
  const memory = new Map();
  const localStorage = {
    getItem: (key) => memory.get(key) ?? null,
    setItem: (key, value) => { memory.set(key, value); },
    removeItem: (key) => { memory.delete(key); },
  };
  const { useAuthStore } = loadTsModule('stores/auth-store.ts', { mocks, globals: { localStorage } });
  const user = { id: 'u1', email: 'a@example.com', role: 'Trader' };
  useAuthStore.getState().setAuth(user);
  calls.length = 0;
  return { store: useAuthStore, calls, toasts, user, memory, api };
}

const failWith = (error) => async () => { throw error; };

test('the browser keeps who is signed in, never a token', () => {
  const { store, memory } = loadStore({ getCurrentUser: async () => ({}) });
  store.setState({ token: 'leaked' });
  store.getState().setAuth({ id: 'u2', email: 'b@example.com', role: 'Trader' });
  const saved = JSON.parse(memory.get('ab-bot-auth')).state;
  assert.deepEqual(Object.keys(saved), ['user']);
});

test('checkAuth keeps the remembered user on transient failures (network, 5xx, 429)', async () => {
  const failures = [
    new TypeError('fetch failed'),
    new ApiHttpError(500, 'boom'),
    new ApiHttpError(503, 'unavailable'),
    new ApiHttpError(429, 'slow down'),
  ];
  for (const failure of failures) {
    const { store, calls, user } = loadStore({ getCurrentUser: failWith(failure) });
    await store.getState().checkAuth();
    const state = store.getState();
    assert.deepEqual(state.user, user, String(failure));
    assert.equal(state.isAuthenticated, true, String(failure));
    assert.equal(state.isLoading, false, String(failure));
    assert.equal(calls.includes('new-session'), false, String(failure));
    assert.equal(calls.includes('clear-queries'), false, String(failure));
  }
});

test('checkAuth without a remembered user stays signed out after a transient failure', async () => {
  const { store } = loadStore({ getCurrentUser: failWith(new ApiHttpError(500, 'boom')) });
  store.setState({ user: null, isAuthenticated: false });
  await store.getState().checkAuth();
  assert.equal(store.getState().isAuthenticated, false);
  assert.equal(store.getState().isLoading, false);
});

test('checkAuth asks the server even with nobody remembered (the cookie may hold a session)', async () => {
  const fresh = { id: 'u1', email: 'a@example.com', role: 'Trader' };
  const { store } = loadStore({ getCurrentUser: async () => fresh });
  store.setState({ user: null, isAuthenticated: false });
  await store.getState().checkAuth();
  assert.deepEqual(store.getState().user, fresh);
  assert.equal(store.getState().isAuthenticated, true);
});

test('checkAuth clears the session only when the server answers 401', async () => {
  const { store, calls } = loadStore({ getCurrentUser: failWith(new ApiHttpError(401, 'Token expired')) });
  await store.getState().checkAuth();
  const state = store.getState();
  assert.equal(state.user, null);
  assert.equal(state.isAuthenticated, false);
  assert.equal(state.isLoading, false);
  assert.ok(calls.includes('clear-queries'));
});

test('a 401 for an older check does not clear a newer sign-in', async () => {
  let reject;
  const { store, user } = loadStore({
    getCurrentUser: () => new Promise((_, rejectCheck) => { reject = rejectCheck; }),
  });
  const pending = store.getState().checkAuth();
  // The user signs in again while the old session is still being checked.
  store.getState().setAuth(user);
  reject(new ApiHttpError(401, 'Token expired'));
  await pending;
  assert.equal(store.getState().isAuthenticated, true);
  assert.equal(store.getState().isLoading, false);
});

test('checkAuth success records the fresh user', async () => {
  const fresh = { id: 'u1', email: 'a@example.com', role: 'Viewer' };
  const { store } = loadStore({ getCurrentUser: async () => fresh });
  await store.getState().checkAuth();
  assert.deepEqual(store.getState().user, fresh);
  assert.equal(store.getState().isAuthenticated, true);
});

test('sessionExpired signs out for the rejected session, resets session context and warns once', () => {
  const { store, calls, toasts, api } = loadStore({ getCurrentUser: async () => ({}) });
  store.getState().sessionExpired(api.currentSession());
  const state = store.getState();
  assert.equal(state.user, null);
  assert.equal(state.isAuthenticated, false);
  assert.deepEqual(calls.filter((c) => c === 'new-session' || c === 'clear-queries'), ['new-session', 'clear-queries']);
  assert.deepEqual(toasts, ['warning:Session expired']);

  // Concurrent 401s from other in-flight requests are no-ops.
  store.getState().sessionExpired(api.currentSession());
  assert.deepEqual(toasts, ['warning:Session expired']);
});

test('sessionExpired ignores a 401 for a session that is no longer current', () => {
  const { store, toasts, user, api } = loadStore({ getCurrentUser: async () => ({}) });
  const old = api.currentSession();
  store.getState().setAuth(user);
  store.getState().sessionExpired(old);
  assert.equal(store.getState().isAuthenticated, true);
  assert.deepEqual(toasts, []);
});
