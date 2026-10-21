import { test } from 'node:test';
import assert from 'node:assert/strict';
import { loadTsModule, jsonResponse } from './test-helpers.mjs';

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

test('getMarkets sends the search term to the backend and parses Decimal strings', async () => {
  const { api, requests } = loadApi(() => jsonResponse([marketFixture]));
  const markets = await api.getMarkets({ search: 'fed rate', category: 'politics', active: true, limit: 25, offset: 50 });

  assert.equal(requests.length, 1);
  const url = new URL(requests[0].url);
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
  assert.equal(new URL(trimmed.requests[0].url).searchParams.get('search'), 'btc');

  for (const search of ['', '   ', undefined]) {
    const { api, requests } = loadApi(() => jsonResponse([]));
    await api.getMarkets({ search, limit: 10 });
    const url = new URL(requests[0].url);
    assert.equal(url.searchParams.has('search'), false, JSON.stringify(search));
    assert.equal(url.searchParams.get('limit'), '10');
  }

  const bare = loadApi(() => jsonResponse([]));
  await bare.api.getMarkets();
  assert.equal(bare.requests[0].url, 'http://localhost:3001/api/v1/markets');
});

test('getMarkets encodes special characters in the search term', async () => {
  const { api, requests } = loadApi(() => jsonResponse([]));
  await api.getMarkets({ search: 'a&b=c#d' });
  assert.match(requests[0].url, /search=a%26b%3Dc%23d/);
  assert.equal(new URL(requests[0].url).searchParams.get('search'), 'a&b=c#d');
});
