import { test } from 'node:test';
import assert from 'node:assert/strict';
import { loadTsModule, jsonResponse, textResponse } from './test-helpers.mjs';

function loadApi(respond) {
  const requests = [];
  const fetchStub = async (url, init = {}) => {
    requests.push({ url: String(url), init });
    return respond(String(url), init);
  };
  const { api, ApiHttpError } = loadTsModule('lib/api.ts', {
    globals: { fetch: fetchStub, crypto: globalThis.crypto },
  });
  return { api, ApiHttpError, requests };
}

const marketFixture = {
  id: 'm1', question: 'Will it rain?', category: 'weather', end_date: '2026-12-31T00:00:00Z',
  active: true, yes_price: '0.45', no_price: '0.55', volume_24h: '1000', liquidity: '500',
  created_at: '2026-01-01T00:00:00Z',
};

// Requests go to the dashboard's own origin (next.config.js forwards them).
const at = (url) => new URL(url, 'http://dashboard.test');

test('getMarkets sends the search term to the backend and parses Decimal strings', async () => {
  const { api, requests } = loadApi(() => jsonResponse([marketFixture]));
  const markets = await api.getMarkets({ search: 'fed rate', category: 'politics', active: true, limit: 25, offset: 50 });

  assert.equal(requests.length, 1);
  const url = at(requests[0].url);
  assert.equal(url.pathname, '/api/v1/markets');
  assert.equal(url.searchParams.get('search'), 'fed rate');
  assert.equal(url.searchParams.get('category'), 'politics');
  assert.equal(url.searchParams.get('active'), 'true');
  assert.equal(url.searchParams.get('limit'), '25');
  assert.equal(url.searchParams.get('offset'), '50');
  assert.equal(markets[0].yes_price, 0.45);
  assert.equal(markets[0].volume_24h, 1000);
});

test('getMarkets trims the search term and omits a blank one', async () => {
  const trimmed = loadApi(() => jsonResponse([]));
  await trimmed.api.getMarkets({ search: '  btc  ' });
  assert.equal(at(trimmed.requests[0].url).searchParams.get('search'), 'btc');

  for (const search of ['', '   ', undefined]) {
    const { api, requests } = loadApi(() => jsonResponse([]));
    await api.getMarkets({ search, limit: 10 });
    const url = at(requests[0].url);
    assert.equal(url.searchParams.has('search'), false, JSON.stringify(search));
    assert.equal(url.searchParams.get('limit'), '10');
  }

  const bare = loadApi(() => jsonResponse([]));
  await bare.api.getMarkets();
  assert.equal(bare.requests[0].url, '/api/v1/markets');
});

test('getMarkets encodes special characters in the search term', async () => {
  const { api, requests } = loadApi(() => jsonResponse([]));
  await api.getMarkets({ search: 'a&b=c#d' });
  assert.match(requests[0].url, /search=a%26b%3Dc%23d/);
  assert.equal(at(requests[0].url).searchParams.get('search'), 'a&b=c#d');
});

test('requests carry the session cookie and the dashboard header, never a token', async () => {
  const { api, requests } = loadApi(() => jsonResponse([]));
  await api.getPositions().catch(() => {});
  await api.login('a@example.com', 'secret').catch(() => {});
  for (const { init } of requests) {
    assert.equal(init.credentials, 'same-origin');
    assert.equal(init.headers['X-Requested-With'], 'ab-bot-dashboard');
    assert.equal(init.headers.Authorization, undefined);
  }
});

// ---- global 401 handling --------------------------------------------------

function loadApiWithHandler(respond) {
  const loaded = loadApi(respond);
  const rejected = [];
  loaded.api.setUnauthorizedHandler((session) => rejected.push(session));
  return { ...loaded, rejected };
}

const unauthorized = () => jsonResponse({ code: 'UNAUTHORIZED', message: 'Token expired' }, 401);

test('a 401 on an authenticated request reports the session it was sent in and still throws', async () => {
  const { api, rejected } = loadApiWithHandler(unauthorized);
  api.beginSession();
  await assert.rejects(api.getPositions(), (error) => {
    assert.equal(error.name, 'ApiHttpError');
    assert.equal(error.statusCode, 401);
    assert.equal(error.isUnauthorized, true);
    return true;
  });
  assert.deepEqual(rejected, [api.currentSession()]);
});

test('the handler receives the session the request was sent in, not a newer one', async () => {
  const state = { api: null };
  const loaded = loadApiWithHandler(() => {
    // A sign-in lands while the request is in flight.
    state.api.beginSession();
    return unauthorized();
  });
  state.api = loaded.api;
  const sent = loaded.api.currentSession();
  await assert.rejects(loaded.api.getPositions());
  assert.deepEqual(loaded.rejected, [sent]);
  assert.notEqual(loaded.api.currentSession(), sent);
});

test('unauthenticated requests, other statuses and auth endpoints never trigger the handler', async () => {
  // Explicitly unauthenticated request (invite acceptance for a signed-out visitor).
  const noAuth = loadApiWithHandler(unauthorized);
  await assert.rejects(noAuth.api.acceptInvite('invite-token', {}, { auth: false }));
  assert.deepEqual(noAuth.rejected, []);

  // Credential/session endpoints answer 401 for ordinary reasons (wrong password, bad signature).
  const auth = loadApiWithHandler(unauthorized);
  await assert.rejects(auth.api.login('a@example.com', 'wrong'));
  await assert.rejects(auth.api.getCurrentUser());
  await assert.rejects(auth.api.walletVerify('msg', 'sig'));
  await assert.rejects(auth.api.logout());
  await assert.rejects(auth.api.getStreamToken());
  assert.deepEqual(auth.rejected, []);

  // Other failures are not session problems.
  for (const status of [400, 403, 404, 429, 500, 503]) {
    const other = loadApiWithHandler(() => jsonResponse({ code: 'X', message: 'nope' }, status));
    await assert.rejects(other.api.getPositions());
    assert.deepEqual(other.rejected, [], String(status));
  }
});

test('an error body in plain text is the error message', async () => {
  const plain = loadApi(() =>
    textResponse('Failed to deserialize the JSON body into the target type: max_signal_age_secs: invalid type', 422),
  );
  await assert.rejects(plain.api.getPositions(), (error) => {
    assert.equal(error.statusCode, 422);
    assert.match(error.message, /max_signal_age_secs/);
    return true;
  });
  const json = loadApi(() => jsonResponse({ code: 'BAD_REQUEST', message: 'limit must be 1–500' }, 400));
  await assert.rejects(json.api.getPositions(), (error) => error.message === 'limit must be 1–500');
  const empty = loadApi(() => textResponse('', 502));
  await assert.rejects(empty.api.getPositions(), (error) => error.message === 'HTTP 502');
  // A proxy's HTML page, and a JSON body that is not an error object, report the status.
  const html = loadApi(() => textResponse('<html><body>Bad gateway</body></html>', 502, 'text/html'));
  await assert.rejects(html.api.getPositions(), (error) => error.message === 'HTTP 502');
  // The rate limiter's 429 carries no content type.
  const limited = loadApi(() => textResponse('Too Many Requests! Wait for 12s', 429, ''));
  await assert.rejects(limited.api.getPositions(), (error) => error.message === 'Too Many Requests! Wait for 12s');
  // A page sent without a type is reported by its status.
  const untypedPage = loadApi(() => textResponse('<html><body>Gateway timeout</body></html>', 504, ''));
  await assert.rejects(untypedPage.api.getPositions(), (error) => error.message === 'HTTP 504');
  const bare = loadApi(() => jsonResponse(null, 500));
  await assert.rejects(bare.api.getPositions(), (error) => error.name === 'ApiHttpError' && error.message === 'HTTP 500');
});

test('a throwing unauthorized handler does not mask the original 401', async () => {
  const { api } = loadApi(unauthorized);
  api.setUnauthorizedHandler(() => { throw new Error('handler bug'); });
  await assert.rejects(api.getPositions(), (error) => error.statusCode === 401);
});

test('clearing the handler stops notifications', async () => {
  const { api, rejected } = loadApiWithHandler(unauthorized);
  api.setUnauthorizedHandler(null);
  await assert.rejects(api.getPositions());
  assert.deepEqual(rejected, []);
});
