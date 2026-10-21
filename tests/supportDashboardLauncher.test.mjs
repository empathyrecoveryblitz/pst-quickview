import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { webcrypto, randomBytes } from 'node:crypto';
import { EventEmitter } from 'node:events';
import { ensureDashboard, serveDashboard, inspectDashboardPointer, spawnDashboardServer, openDashboardBrowser } from '../scripts/support-dashboard-launcher.mjs';

async function fixture(t) {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'pqv-launcher-test-')); await fs.chmod(directory, 0o700);
  const configPath = path.join(directory, 'operator.json'), servicePath = path.join(directory, 'service.json'), pointerPath = path.join(directory, 'dashboard-pointer.json');
  const keys = await webcrypto.subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, true, ['sign', 'verify']);
  const service = { v: 3, origin: 'https://synthetic.example.workers.dev', operatorPublicKey: Buffer.from(await webcrypto.subtle.exportKey('raw', keys.publicKey)).toString('base64url'), operatorLabel: 'Fictional support' };
  await fs.writeFile(configPath, JSON.stringify({ v: 3, adminToken: randomBytes(32).toString('hex'), operatorSigningJwk: await webcrypto.subtle.exportKey('jwk', keys.privateKey) }), { mode: 0o600, flag: 'wx' });
  await fs.writeFile(servicePath, JSON.stringify(service), { mode: 0o600, flag: 'wx' });
  const servers = []; let spawns = 0;
  const spawnServer = async () => { spawns++; servers.push(await serveDashboard({ configPath, servicePath })); };
  t.after(async () => {
    for (const server of servers) await server.close();
    for (const name of ['operator.json', 'service.json', 'dashboard-pointer.json', 'dashboard-start.lock', 'dashboard-recovery.lock']) await fs.unlink(path.join(directory, name)).catch(() => {});
    await fs.rmdir(directory);
  });
  return { directory, configPath, servicePath, pointerPath, service, servers, spawnServer, spawns: () => spawns };
}

test('manual launcher starts once, reuses only an authenticated pinned dashboard and keeps state private', async t => {
  const f = await fixture(t);
  const first = await ensureDashboard({ ...f, waitMs: 1000 }); assert.equal(first.reused, false); assert.equal(f.spawns(), 1);
  assert.equal((await fs.stat(f.pointerPath)).mode & 0o777, 0o600);
  const second = await ensureDashboard({ ...f, waitMs: 1000 }); assert.equal(second.reused, true); assert.equal(second.stateFile, first.stateFile); assert.equal(f.spawns(), 1);
  const pointer = await inspectDashboardPointer(f.pointerPath, f.service); assert.equal(pointer.available, true);
  assert.equal(pointer.state.dashboardURL, first.dashboardURL);
  assert.match(first.dashboardURL, /^http:\/\/127\.0\.0\.1:\d+\/#(?:[a-f0-9]{64})$/);
  await assert.rejects(fs.stat(path.join(f.directory, 'dashboard-start.lock')), { code: 'ENOENT' });
});

test('simultaneous manual launches serialize and never start duplicate servers', async t => {
  const f = await fixture(t);
  const results = await Promise.all([ensureDashboard({ ...f, waitMs: 1500 }), ensureDashboard({ ...f, waitMs: 1500 })]);
  assert.equal(f.spawns(), 1); assert.equal(results[0].stateFile, results[1].stateFile);
  assert.deepEqual(results.map(result => result.reused).sort(), [false, true]);
});

test('a missing temporary state after reboot recovers its pointer without killing any process', async t => {
  const f = await fixture(t);
  const old = await ensureDashboard({ ...f, waitMs: 1000 }); await f.servers[0].close();
  await fs.writeFile(f.pointerPath, JSON.stringify({ pid: process.pid, stateFile: old.stateFile }), { mode: 0o600, flag: 'wx' });
  const recovered = await ensureDashboard({ ...f, waitMs: 1000 }); assert.equal(recovered.reused, false); assert.notEqual(recovered.stateFile, old.stateFile);
  assert.equal(f.spawns(), 2);
});

test('an unverified old process is never killed and its cleanup cannot remove the new pointer', async t => {
  const f = await fixture(t);
  const old = await ensureDashboard({ ...f, waitMs: 1000 });
  const changed = { ...f.service, operatorLabel: 'Another fictional support label' };
  await fs.writeFile(f.servicePath, JSON.stringify(changed), { mode: 0o600 });
  const replacement = await ensureDashboard({ ...f, waitMs: 1000 }); assert.equal(replacement.reused, false); assert.notEqual(replacement.stateFile, old.stateFile);
  const oldUrl = new URL(old.dashboardURL);
  const response = await fetch(oldUrl.origin + '/api/service', { headers: { Origin: oldUrl.origin, Authorization: 'Bearer ' + oldUrl.hash.slice(1) } });
  assert.equal(response.status, 200);
  await f.servers[0].close();
  assert.equal((await inspectDashboardPointer(f.pointerPath, changed)).available, true);
});

test('unsafe pointer permissions, symlinks and out-of-scope state fail closed without replacement', async t => {
  const f = await fixture(t);
  await ensureDashboard({ ...f, waitMs: 1000 }); const bytes = await fs.readFile(f.pointerPath);
  await fs.chmod(f.pointerPath, 0o644); await assert.rejects(ensureDashboard({ ...f, waitMs: 1000 }), /unsafe_local_state/); await fs.chmod(f.pointerPath, 0o600);
  await fs.unlink(f.pointerPath); await fs.symlink(f.configPath, f.pointerPath);
  await assert.rejects(ensureDashboard({ ...f, waitMs: 1000 })); await fs.unlink(f.pointerPath);
  await fs.writeFile(f.pointerPath, JSON.stringify({ pid: process.pid, stateFile: f.configPath }), { mode: 0o600, flag: 'wx' });
  await assert.rejects(ensureDashboard({ ...f, waitMs: 1000 }), /unsafe_local_state/);
  assert.equal(f.spawns(), 1); await fs.writeFile(f.pointerPath, bytes, { mode: 0o600 });
});

test('live startup lock remains untouched and waits only for the configured bounded interval', async t => {
  const f = await fixture(t), file = path.join(f.directory, 'dashboard-start.lock');
  const bytes = JSON.stringify({ pid: process.pid, nonce: randomBytes(16).toString('hex') }); await fs.writeFile(file, bytes, { mode: 0o600, flag: 'wx' });
  await assert.rejects(ensureDashboard({ ...f, waitMs: 150 }), /dashboard_start_busy/);
  assert.equal(await fs.readFile(file, 'utf8'), bytes); assert.equal(f.spawns(), 0);
});

test('detached server and manual browser opener use fixed binaries, safe argument vectors and no secret output', async () => {
  let unref = false; const calls = [];
  const fakeSpawn = (binary, args, options) => { calls.push({ binary, args, options }); const child = new EventEmitter(); child.unref = () => { unref = true; }; queueMicrotask(() => { child.emit('spawn'); child.emit('close', 0); }); return child; };
  await spawnDashboardServer('/synthetic/private/operator.json', { spawnImpl: fakeSpawn });
  assert.equal(unref, true); assert.equal(calls[0].binary, process.execPath); assert.equal(calls[0].options.detached, true); assert.equal(calls[0].options.stdio, 'ignore');
  assert.deepEqual(calls[0].args.slice(1), ['--serve', '--config', '/synthetic/private/operator.json']);
  assert.equal(calls[0].options.env.NODE_OPTIONS, undefined); assert.equal(calls[0].options.env.ADMIN_TOKEN, undefined); assert.equal(calls[0].options.shell, undefined);
  const url = 'http://127.0.0.1:12345/#' + 'a'.repeat(64); await openDashboardBrowser(url, { spawnImpl: fakeSpawn });
  assert.equal(calls[1].binary, '/usr/bin/xdg-open'); assert.deepEqual(calls[1].args, [url]); assert.equal(calls[1].options.stdio, 'ignore'); assert.equal(calls[1].options.shell, undefined);
  for (const invalid of ['https://unrelated.invalid/#' + 'a'.repeat(64), 'http://127.0.0.1:1000/#' + 'a'.repeat(64), 'http://127.0.0.1:12345/?token=' + 'a'.repeat(64)]) assert.throws(() => openDashboardBrowser(invalid, { spawnImpl: fakeSpawn }), /unsafe_local_state/);
  const source = await fs.readFile(new URL('../scripts/support-dashboard-launcher.mjs', import.meta.url), 'utf8');
  assert.match(source, /startOnly \? 'dashboard_ready'/); assert.match(source, /stateFile: result\.stateFile/);
  assert.doesNotMatch(source, /stdout\.write\([^\n]*(?:dashboardURL|adminToken|operatorSigningJwk)|autostart|\.desktop|systemctl|kill\([^,]+,\s*['"]SIG/);
});
