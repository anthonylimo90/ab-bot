import { test } from 'node:test';
import assert from 'node:assert/strict';
import { queryClient, shouldRetryQuery } from './queryClient.ts';

const httpError = (statusCode) => Object.assign(new Error(`HTTP ${statusCode}`), { name: 'ApiHttpError', statusCode });

test('query retry never retries 4xx responses', () => {
  for (const status of [400, 401, 403, 404, 409, 422, 429]) {
    assert.equal(shouldRetryQuery(0, httpError(status)), false, String(status));
  }
});

test('query retry keeps a small retry budget for network errors and 5xx', () => {
  for (const error of [new TypeError('fetch failed'), httpError(500), httpError(502), httpError(503), 'weird', undefined]) {
    assert.equal(shouldRetryQuery(0, error), true, String(error));
    assert.equal(shouldRetryQuery(1, error), true, String(error));
    assert.equal(shouldRetryQuery(2, error), false, String(error));
  }
});

test('the shared query client uses the retry policy', () => {
  assert.equal(queryClient.getDefaultOptions().queries.retry, shouldRetryQuery);
});
