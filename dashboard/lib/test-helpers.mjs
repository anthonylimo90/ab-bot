// Test-only loader for TypeScript modules that cannot be imported directly by
// `node --experimental-strip-types` (parameter properties, `@/` path aliases,
// extension-less relative imports). It transpiles a module with the TypeScript
// compiler, resolves sibling imports (`@/x`, `./x`) the same way, and lets a test
// replace any import with a mock. Not matched by the `lib/*.test.mjs` glob.
import { existsSync, readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import vm from 'node:vm';
import ts from 'typescript';

const nodeRequire = createRequire(import.meta.url);
const dashboardRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');

function resolveSource(specifier, fromFile) {
  const base = specifier.startsWith('@/')
    ? path.join(dashboardRoot, specifier.slice(2))
    : path.resolve(path.dirname(fromFile), specifier);
  for (const candidate of [base, `${base}.ts`, `${base}.tsx`]) {
    if (path.extname(candidate) && existsSync(candidate)) return candidate;
  }
  return null;
}

/**
 * @param {string} file Path of the module, relative to the dashboard directory.
 * @param {{ mocks?: Record<string, unknown>, globals?: Record<string, unknown> }} [options]
 *   `mocks` is keyed by the exact import specifier used in source.
 */
export function loadTsModule(file, { mocks = {}, globals = {} } = {}) {
  const cache = new Map();

  function load(absFile) {
    if (cache.has(absFile)) return cache.get(absFile).exports;
    const source = ts.transpileModule(readFileSync(absFile, 'utf8'), {
      compilerOptions: { module: ts.ModuleKind.CommonJS, target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.ReactJSX },
    }).outputText;
    const loaded = { exports: {} };
    cache.set(absFile, loaded);
    const context = {
      module: loaded,
      exports: loaded.exports,
      require: (specifier) => {
        if (specifier in mocks) return mocks[specifier];
        const resolved = resolveSource(specifier, absFile);
        return resolved ? load(resolved) : nodeRequire(specifier);
      },
      console,
      URL,
      URLSearchParams,
      setTimeout,
      clearTimeout,
      setInterval,
      clearInterval,
      process: { env: {} },
      ...globals,
    };
    vm.runInNewContext(source, context, { filename: absFile });
    return loaded.exports;
  }

  return load(path.resolve(dashboardRoot, file));
}

/** A minimal fetch Response for the API client. */
export function jsonResponse(body, status = 200) {
  return {
    ok: status >= 200 && status < 300,
    status,
    headers: new Headers({ 'content-type': 'application/json' }),
    json: async () => body,
    text: async () => JSON.stringify(body),
  };
}

/** A fetch Response whose body is text: plain, as axum's extractor rejections are, or another type. */
export function textResponse(body, status, contentType = 'text/plain; charset=utf-8') {
  return {
    ok: status >= 200 && status < 300,
    status,
    headers: new Headers({ 'content-type': contentType }),
    json: async () => {
      throw new SyntaxError('Unexpected token');
    },
    text: async () => body,
  };
}
