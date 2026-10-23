import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';

const require = createRequire(import.meta.url);

async function headerMap() {
  const nextConfig = require('../next.config.js');
  const rules = await nextConfig.headers();
  assert.equal(rules.length, 1);
  assert.equal(rules[0].source, '/:path*');
  return new Map(rules[0].headers.map(({ key, value }) => [key, value]));
}

test('hardening headers are sent on every route', async () => {
  const headers = await headerMap();
  assert.equal(headers.get('X-Frame-Options'), 'DENY');
  assert.equal(headers.get('X-Content-Type-Options'), 'nosniff');
  assert.equal(headers.get('Referrer-Policy'), 'strict-origin-when-cross-origin');
});

test('the CSP is report-only so wallet connectors cannot be broken by it', async () => {
  const headers = await headerMap();
  assert.ok(headers.has('Content-Security-Policy-Report-Only'));
  assert.equal(headers.has('Content-Security-Policy'), false, 'an enforcing CSP must not be added');
  const policy = headers.get('Content-Security-Policy-Report-Only');
  assert.match(policy, /default-src 'self'/);
  assert.match(policy, /frame-ancestors 'none'/);
  assert.match(policy, /object-src 'none'/);
  // The API and WebSocket origins the app actually uses are listed.
  assert.match(policy, /connect-src [^;]*http:\/\/localhost:3000/);
  assert.match(policy, /connect-src [^;]*ws:\/\/localhost:3000/);
});

test('the X-Powered-By header is disabled', () => {
  assert.equal(require('../next.config.js').poweredByHeader, false);
});
