import { test } from 'node:test';
import assert from 'node:assert/strict';
import { safeRedirectPath } from './safe-redirect.ts';

test('same-origin root-relative paths are followed unchanged', () => {
  for (const path of ['/', '/positions', '/positions?tab=open&page=2', '/admin/workspaces#top', '/invite/abc-123', '/a/b%2Fc', '/path with space']) {
    assert.equal(safeRedirectPath(path), path, path);
  }
});

test('open-redirect payloads fall back to the default page', () => {
  const attacks = [
    '//evil.example',
    '//evil.example/path',
    '///evil.example',
    '/\\evil.example',
    '/\\/evil.example',
    '\\\\evil.example',
    '\\/evil.example',
    'https://evil.example',
    'http://evil.example/x',
    'HTTPS://EVIL.EXAMPLE',
    'javascript:alert(1)',
    'data:text/html,<script>alert(1)</script>',
    'evil.example',
    'positions',
    ' /positions',
    '',
    // Browsers strip tab/CR/LF while parsing, turning these into "//evil.example".
    '/\t/evil.example',
    '/\n/evil.example',
    '/\r/evil.example',
    '/\u0000/evil.example',
    // Dot segments that normalise to a protocol-relative path.
    '/..//evil.example',
    '/.//evil.example',
  ];
  for (const attack of attacks) {
    assert.equal(safeRedirectPath(attack), '/', JSON.stringify(attack));
  }
});

test('missing or non-string values use the fallback, which can be overridden', () => {
  assert.equal(safeRedirectPath(null), '/');
  assert.equal(safeRedirectPath(undefined), '/');
  assert.equal(safeRedirectPath(42), '/');
  assert.equal(safeRedirectPath('//evil.example', '/dashboard'), '/dashboard');
  assert.equal(safeRedirectPath(null, '/dashboard'), '/dashboard');
});

test('percent-encoded slashes stay inside the path and are allowed', () => {
  assert.equal(safeRedirectPath('/%2Fevil.example'), '/%2Fevil.example');
  assert.equal(safeRedirectPath('/%5Cevil.example'), '/%5Cevil.example');
});
