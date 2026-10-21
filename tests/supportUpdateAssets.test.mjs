import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, lstatSync, readdirSync, mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';

const root = fileURLToPath(new URL('../', import.meta.url));
const config = JSON.parse(readFileSync(new URL('../cloudflare/support/wrangler.jsonc', import.meta.url), 'utf8'));
const directory = new URL('../cloudflare/support/update-assets/', import.meta.url);

test('update assets use a dedicated directory, exact API-first routing and no HTML fallback', () => {
  assert.deepEqual(config.assets, { directory: './update-assets', binding: 'ASSETS', run_worker_first: ['/v3/*'], html_handling: 'none', not_found_handling: 'none' });
  assert.equal(lstatSync(directory).isDirectory(), true); assert.equal(lstatSync(directory).isSymbolicLink(), false);
  assert.deepEqual(config.durable_objects, { bindings: [{ name: 'SUPPORT_QUEUE', class_name: 'SupportQueue' }] });
  assert.deepEqual(config.migrations, [{ tag: 'v1', new_sqlite_classes: ['SupportQueue'] }]);
  const service = JSON.parse(readFileSync(new URL('../public/support/service.json', import.meta.url), 'utf8'));
  assert.equal(config.vars.SERVICE_ORIGIN, service.origin);
  assert.equal(config.observability.enabled, false); assert.equal(config.observability.logs.invocation_logs, false);
  assert.equal(config.preview_urls, false); assert.equal(config.send_metrics, false); assert.equal(config.logpush, false);
  assert.equal(config.vars.ADMIN_TOKEN, undefined);
});

test('public asset allowlist rejects source, credentials, API shadows and unexpected download paths', t => {
  const samples = [
    ['updates/development.json', true], ['updates/artifacts/0.2.0-beta.3.support.2/' + 'a'.repeat(64) + '.app.tar.gz', true], ['_headers', false],
    ['.gitignore', false], ['.assetsignore', false], ['index.html', false], ['404.html', false], ['_redirects', false],
    ['v3/admin/queue', false], ['v3/requests', false], ['updates/operator.json', false], ['operator.json', false],
    ['updates/artifacts/identity.key', false], ['updates/artifacts/version/archive.pst', false], ['updates/artifacts/version/archive.app.zip', false],
    ['updates/artifacts/version/deeper/archive.app.tar.gz', false], ['src/main.tsx', false], ['src-tauri/src/lib.rs', false],
  ];
  // Exercise actual gitignore semantics in a synthetic, uncommitted temporary
  // repository; the project's commit exclusions must not override asset rules.
  const synthetic = mkdtempSync(path.join(os.tmpdir(), 'pst-update-assets-test-'));
  t.after(() => rmSync(synthetic, { recursive: true, force: true }));
  assert.equal(spawnSync('git', ['init', '-q', synthetic], { encoding: 'utf8' }).status, 0);
  writeFileSync(path.join(synthetic, '.gitignore'), readFileSync(new URL('.assetsignore', directory)));
  const checked = spawnSync('git', ['-c', 'core.excludesFile=/dev/null', 'check-ignore', '--no-index', '--stdin'], {
    cwd: synthetic, input: samples.map(([name]) => name).join('\n') + '\n', encoding: 'utf8',
  });
  assert.ok([0, 1].includes(checked.status));
  const ignored = new Set(checked.stdout.trim().split('\n').filter(Boolean));
  for (const [name, allowed] of samples) assert.equal(!ignored.has(name), allowed, name);
});

test('staging placeholders cannot commit software payloads and manifest caching is disabled', () => {
  assert.deepEqual(readdirSync(directory).sort(), ['.assetsignore', '.gitignore', '_headers']);
  const ignored = readFileSync(new URL('.gitignore', directory), 'utf8');
  assert.equal(ignored, '*\n!.gitignore\n!.assetsignore\n!_headers\n');
  assert.equal(lstatSync(new URL('_headers', directory)).isFile(), true);
  assert.doesNotMatch(readFileSync(new URL('.assetsignore', directory), 'utf8'), /^!_headers\s*$/m);
  const headers = readFileSync(new URL('_headers', directory), 'utf8');
  assert.match(headers, /^\/updates\/development\.json\n  Cache-Control: no-store\n/m);
  assert.match(headers, /^\/updates\/artifacts\/\*\n  Cache-Control: public, max-age=31536000, immutable, no-transform\n/m);
  assert.doesNotMatch(headers, /Access-Control-Allow-Origin|Set-Cookie|Location:|https?:/);
  assert.equal(path.resolve(root, 'cloudflare/support', config.assets.directory), fileURLToPath(directory).replace(/\/$/, ''));
});
