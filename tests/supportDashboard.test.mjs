import test from 'node:test';
import assert from 'node:assert/strict';
import crypto from 'node:crypto';
import fs from 'node:fs/promises';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { createOperatorDashboard, launchOperatorDashboard, loadOperatorIdentity, validateDashboardService } from '../scripts/support-dashboard.mjs';
import { createRequest, createEphemeralKeyPair, verifyClaim, deriveClaimConfig, validateServiceConfig, ackProof, validateUpdateHandoff, canonicalUpdateHandoff, resumedGuestProof, resumedClaimProof, verifyUpdateProof, queueTokenSha256, createResumedRequest } from '../public/support/queue-protocol.js';

const json = (value, status = 200) => new Response(JSON.stringify(value), { status });
const assets = { html: '<!doctype html><title>Synthetic dashboard</title>', dashboard: 'export {};', css: 'body{}', protocol: 'export {};', peer: 'export {};', queueProtocol: 'export {};' };
const pause = () => { let resolve; const promise = new Promise(done => { resolve = done; }); return { promise, resolve }; };
async function fixture(t, options = {}) {
  const signing = await crypto.webcrypto.subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, true, ['sign', 'verify']);
  const publicKey = Buffer.from(await crypto.webcrypto.subtle.exportKey('raw', signing.publicKey)).toString('base64url');
  const service = { v: 3, origin: 'https://synthetic.example.workers.dev', operatorPublicKey: publicKey, operatorLabel: 'Fictional support' };
  const adminToken = crypto.randomBytes(32).toString('hex'), controllerToken = crypto.randomBytes(32).toString('hex');
  const first = await createRequest({ expires: Date.now() + 60_000 }), second = await createRequest({ expires: Date.now() + 60_000 });
  const rows = new Map([first, second].map(({ request }) => [request.id, { v: 3, id: request.id, guestPublicKey: request.guestPublicKey, expires: request.expires, status: 'waiting' }]));
  const calls = [], claims = new Map(), tokens = new Map();
  const settings = { failQueue: false, failHeartbeat: false, mullvad: true, routeGate: null, heartbeatGate: null, ...options };
  const fetchImpl = async (url, init = {}) => {
    const parsed = new URL(url); calls.push({ origin: parsed.origin, path: parsed.pathname, init });
    if (parsed.origin === 'https://am.i.mullvad.net') {
      assert.equal(parsed.pathname, '/json'); assert.equal(init.headers, undefined);
      if (settings.routeGate) await settings.routeGate.promise;
      return json({ mullvad_exit_ip: settings.mullvad, ip: '1.1.1.1' });
    }
    assert.equal(parsed.origin, service.origin); assert.equal(init.redirect, 'error'); assert.equal(init.credentials, 'omit');
    if (parsed.pathname === '/v3/admin/queue') {
      assert.equal(init.headers.Authorization, 'Bearer ' + adminToken);
      if (settings.failQueue) throw new DOMException('Synthetic timeout', 'TimeoutError');
      const snapshot = structuredClone(settings.queueSnapshot ?? [...rows.values()]);
      if (settings.queueGate) { const gate = settings.queueGate; settings.queueGate = null; gate.started.resolve(); await gate.promise; }
      return json({ requests: snapshot });
    }
    const claim = /^\/v3\/admin\/requests\/([a-f0-9]{32})\/claim$/.exec(parsed.pathname);
    if (claim) {
      assert.equal(init.headers.Authorization, 'Bearer ' + adminToken);
      const value = JSON.parse(init.body); await verifyClaim(value, service);
      const row = rows.get(claim[1]); assert.ok(row);
      const token = row.status === 'resuming' ? crypto.randomBytes(32).toString('hex') : controllerToken;
      rows.set(row.id, { v: 3, id: row.id, guestPublicKey: row.guestPublicKey, expires: row.expires, status: 'claimed' }); claims.set(row.id, value); tokens.set(row.id, token);
      return json({ token, claim: value });
    }
    const handoff = /^\/v3\/(admin\/)?requests\/([a-f0-9]{32})\/handoff\/(heartbeat|stop)$/.exec(parsed.pathname);
    if (handoff) {
      const row = rows.get(handoff[2]); assert.ok(row);
      assert.equal(init.headers.Authorization, 'Bearer ' + (handoff[1] ? adminToken : tokens.get(row.id)));
      assert.deepEqual(JSON.parse(init.body), { nonce: row.handoff.nonce });
      if (handoff[3] === 'heartbeat' && settings.handoffHeartbeatGate) {
        const gate = settings.handoffHeartbeatGate; settings.handoffHeartbeatGate = null; gate.started.resolve(); await gate.promise;
        return json({ error: 'old_token_rotated' }, 401);
      }
      if (handoff[3] === 'stop') rows.delete(row.id);
      return json(handoff[3] === 'stop' ? { stopped: true } : { active: true });
    }
    const session = /^\/v3\/requests\/([a-f0-9]{32})\/(heartbeat|stop)$/.exec(parsed.pathname);
    assert.ok(session); assert.equal(init.headers.Authorization, 'Bearer ' + tokens.get(session[1]));
    assert.equal(init.body, '{}');
    if (session[2] === 'stop') { rows.delete(session[1]); return json({ stopped: true }); }
    if (settings.hangHeartbeat) await new Promise((_, reject) => { init.signal.addEventListener('abort', () => reject(init.signal.reason), { once: true }); });
    if (settings.heartbeatGate) { const gate = settings.heartbeatGate; settings.heartbeatGate = null; await gate.promise; if (settings.failHeartbeat) throw new DOMException('Synthetic timeout', 'TimeoutError'); }
    if (settings.failHeartbeat) throw new DOMException('Synthetic timeout', 'TimeoutError');
    return json({ active: true });
  };
  const dashboard = await createOperatorDashboard({ service, adminToken, operatorSigningKey: signing.privateKey, assets, fetchImpl, ...(options.now ? { now: options.now } : {}) });
  t.after(() => dashboard.close());
  const request = (route, body, extra = {}) => fetch(dashboard.origin + route, {
    method: body === undefined ? 'GET' : 'POST', headers: { Origin: dashboard.origin, Authorization: 'Bearer ' + dashboard.capability, ...(body === undefined ? {} : { 'Content-Type': 'application/json' }), ...extra },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
  const claim = async id => { const pair = await createEphemeralKeyPair(); const response = await request('/api/claim', { id, helperPublicKey: pair.publicKey }); return { pair, response }; };
  return { dashboard, request, claim, first, second, rows, calls, claims, service, signing, adminToken, controllerToken, settings };
}

function handoffFor(f, claim) {
  return { v: 1, parentId: claim.payload.id, parentSid: claim.payload.sid, nonce: crypto.randomBytes(16).toString('hex'), targetVersion: '0.2.0-beta.4',
    artifactSha256: '3'.repeat(64), nativeSessionId: '4'.repeat(32), originalExpires: claim.payload.expires,
    resumeDeadline: Math.min(Date.now() + 45000, claim.payload.expires), resumeTicketSha256: '5'.repeat(64) };
}
async function beginUpdate(f) {
  const initial = await f.claim(f.first.request.id); assert.equal(initial.response.status, 200);
  const original = await initial.response.json(), handoff = handoffFor(f, original.claim), resumeAuth = crypto.randomBytes(32).toString('hex');
  f.rows.set(handoff.parentId, { ...f.rows.get(handoff.parentId), status: 'updating', handoff });
  return { original, handoff, resumeAuth };
}
async function publishResuming(f, grant, changes = {}) {
  const resumed = await createResumedRequest(grant.handoff), guestTokenSha256 = await queueTokenSha256(resumed.request.token);
  const resumeGuestProof = await resumedGuestProof(grant.resumeAuth, grant.handoff, resumed.request.guestPublicKey, guestTokenSha256);
  f.rows.set(grant.handoff.parentId, { v: 3, id: resumed.request.id, guestPublicKey: resumed.request.guestPublicKey, expires: resumed.request.expires, status: 'resuming', handoff: grant.handoff, guestTokenSha256, resumeGuestProof, ...changes });
  return resumed;
}
test('update resume requires the same in-memory helper grant, proves fresh guest credentials and signs fresh keys', async t => {
  const f = await fixture(t), grant = await beginUpdate(f), id = grant.handoff.parentId;
  assert.equal((await f.request('/api/handoff', { id, handoff: grant.handoff, resumeAuth: grant.resumeAuth })).status, 200);
  assert.equal((await (await f.request('/api/queue')).json()).activeId, id);
  assert.equal((await f.request('/api/heartbeat', { id })).status, 200);
  const resumed = await publishResuming(f, grant), pair = await createEphemeralKeyPair();
  assert.equal((await f.request('/api/resume', { id, helperPublicKey: pair.publicKey, resumeAuth: '0'.repeat(64) })).status, 409);
  const response = await f.request('/api/resume', { id, helperPublicKey: pair.publicKey, resumeAuth: grant.resumeAuth }); assert.equal(response.status, 200);
  const result = await response.json(), claim = await verifyClaim(result.claim, f.service);
  assert.notEqual(result.token, grant.original.token); assert.notEqual(claim.payload.sid, grant.handoff.parentSid);
  assert.equal(claim.payload.guestPublicKey, resumed.request.guestPublicKey); assert.equal(claim.payload.expires, grant.handoff.originalExpires);
  assert.equal(verifyUpdateProof(await resumedClaimProof(grant.resumeAuth, grant.handoff, claim), claim.resumeProof), true);
  const helper = await deriveClaimConfig(claim.payload, claim.signature, pair.privateKey, 'controller', result.token, f.service.origin);
  const guest = await deriveClaimConfig(claim.payload, claim.signature, resumed.privateKey, 'guest', resumed.request.token, f.service.origin); assert.equal(helper.key, guest.key);
  assert.equal(f.calls.some(call => call.init.body?.includes(grant.resumeAuth)), false);
  assert.equal((await f.request('/api/resume', { id, helperPublicKey: pair.publicKey, resumeAuth: grant.resumeAuth })).status, 409);
  assert.equal((await f.request('/api/resumed', { id })).status, 200);
  assert.equal((await f.request('/api/stop', { id })).status, 200);
  assert.equal(f.rows.has(id), false);
});
test('pending update Stop uses a nonce-bound administrator revocation and never reuses ordinary session tokens', async t => {
  const f = await fixture(t), grant = await beginUpdate(f), id = grant.handoff.parentId;
  assert.equal((await f.request('/api/handoff', { id, handoff: grant.handoff, resumeAuth: grant.resumeAuth })).status, 200);
  await publishResuming(f, grant);
  assert.equal((await f.request('/api/stop', { id })).status, 200);
  assert.equal(f.rows.has(id), false);
  assert.equal(f.calls.some(call => call.path === `/v3/admin/requests/${id}/handoff/stop`), true);
  assert.equal((await f.request('/api/resume', { id, helperPublicKey: (await createEphemeralKeyPair()).publicKey, resumeAuth: grant.resumeAuth })).status, 409);
});
test('Stop while the resumed claim checks its route clears the grant before releasing the pending request', async t => {
  const f = await fixture(t), grant = await beginUpdate(f), id = grant.handoff.parentId;
  assert.equal((await f.request('/api/handoff', { id, handoff: grant.handoff, resumeAuth: grant.resumeAuth })).status, 200);
  await publishResuming(f, grant); const gate = pause(); f.settings.routeGate = gate;
  const pending = f.request('/api/resume', { id, helperPublicKey: (await createEphemeralKeyPair()).publicKey, resumeAuth: grant.resumeAuth });
  await settle(() => f.calls.filter(call => call.origin === 'https://am.i.mullvad.net').length === 2);
  assert.equal((await f.request('/api/stop', { id })).status, 200); assert.equal(f.rows.has(id), false);
  gate.resolve(); assert.equal((await pending).status, 409);
  assert.equal((await (await f.request('/api/queue')).json()).activeId, null);
  assert.equal(f.calls.filter(call => call.path.endsWith('/claim')).length, 1);
});
test('an expired or abandoned laptop grant cannot authorize a resumed claim', async t => {
  for (const advance of [45001, 30001]) {
    let clock = Date.now(); const f = await fixture(t, { now: () => clock }), grant = await beginUpdate(f), id = grant.handoff.parentId;
    assert.equal((await f.request('/api/handoff', { id, handoff: grant.handoff, resumeAuth: grant.resumeAuth })).status, 200);
    await publishResuming(f, grant); clock += advance;
    assert.equal((await f.request('/api/resume', { id, helperPublicKey: (await createEphemeralKeyPair()).publicKey, resumeAuth: grant.resumeAuth })).status, 409);
    assert.equal(f.rows.has(id), false);
  }
});
test('a controller heartbeat racing atomic token rotation preserves only its verified pending grant', async t => {
  const f = await fixture(t), grant = await beginUpdate(f), id = grant.handoff.parentId;
  // Publication can precede the direct handoff callback: use the exact pending
  // nonce heartbeat rather than incorrectly ending the original approval.
  assert.equal((await f.request('/api/heartbeat', { id })).status, 200);
  assert.equal((await f.request('/api/handoff', { id, handoff: grant.handoff, resumeAuth: grant.resumeAuth })).status, 200);
  const gate = { ...pause(), started: pause() }; f.settings.handoffHeartbeatGate = gate;
  const heartbeat = f.request('/api/heartbeat', { id }); await gate.started.promise;
  await publishResuming(f, grant); gate.resolve(); assert.equal((await heartbeat).status, 200);
  assert.equal((await (await f.request('/api/queue')).json()).activeId, id);
  const response = await f.request('/api/resume', { id, helperPublicKey: (await createEphemeralKeyPair()).publicKey, resumeAuth: grant.resumeAuth }); assert.equal(response.status, 200);
});

test('dashboard binds loopback, enforces capability and exact Host/Origin, and exposes no secrets', async t => {
  const f = await fixture(t);
  assert.match(f.dashboard.origin, /^http:\/\/127\.0\.0\.1:\d+$/);
  const service = await f.request('/api/service'); assert.equal(service.status, 200);
  const text = await service.text(); assert.equal(text.includes(f.adminToken), false); assert.equal(text.includes(f.controllerToken), false); assert.equal(text.includes('operatorSigningJwk'), false);
  for (const headers of [{ Authorization: '' }, { Authorization: 'Bearer ' + '0'.repeat(64) }, { Origin: 'https://unrelated.invalid' }, { Origin: 'null' }]) assert.equal((await f.request('/api/queue', undefined, headers)).status, 403);
  const result = await new Promise((resolve, reject) => { const request = http.get(f.dashboard.origin + '/api/queue', { headers: { Host: 'unrelated.invalid', Origin: f.dashboard.origin, Authorization: 'Bearer ' + f.dashboard.capability } }, response => { response.resume(); resolve(response.statusCode); }); request.on('error', reject); });
  assert.equal(result, 403);
  assert.equal((await fetch(f.dashboard.origin + '/api/queue')).status, 403);
  assert.equal((await fetch(f.dashboard.origin + '/api/queue', { headers: { 'Sec-Fetch-Site': 'same-origin', Authorization: 'Bearer ' + f.dashboard.capability } })).status, 200);
  const asset = await fetch(f.dashboard.origin + '/'); assert.equal(asset.status, 200);
  assert.match(asset.headers.get('content-security-policy'), /script-src 'self'/); assert.doesNotMatch(asset.headers.get('content-security-policy'), /unsafe-inline|unsafe-eval/);
  assert.equal(asset.headers.get('cache-control'), 'no-store'); assert.equal(asset.headers.get('referrer-policy'), 'no-referrer');
});

test('queue supports multiple requests but only one signed claim and current Mullvad route', async t => {
  const f = await fixture(t);
  const initial = await (await f.request('/api/queue')).json(); assert.equal(initial.requests.length, 2); assert.equal(initial.activeId, null);
  const first = await f.claim(f.first.request.id); assert.equal(first.response.status, 200);
  const result = await first.response.json(); const claim = await verifyClaim(result.claim, f.service);
  assert.equal(claim.payload.expectedExit, '1.1.1.1'); assert.equal(claim.payload.helperPublicKey, first.pair.publicKey);
  const helper = await deriveClaimConfig(claim.payload, claim.signature, first.pair.privateKey, 'controller', result.token, f.service.origin);
  const guest = await deriveClaimConfig(claim.payload, claim.signature, f.first.privateKey, 'guest', f.first.request.token, f.service.origin);
  assert.equal(helper.key, guest.key); assert.equal((await f.claim(f.second.request.id)).response.status, 409);
  assert.equal(f.calls.filter(call => call.origin === 'https://am.i.mullvad.net').length, 1);
  assert.equal((await f.request('/api/heartbeat', { id: f.second.request.id })).status, 409);
  assert.equal((await f.request('/api/stop', { id: f.second.request.id })).status, 409);
  assert.equal((await f.request('/api/heartbeat', { id: f.first.request.id })).status, 200);
  assert.equal((await f.request('/api/stop', { id: f.first.request.id })).status, 200);
  const after = await (await f.request('/api/queue')).json(); assert.equal(after.activeId, null); assert.equal(after.requests.length, 1);
  assert.equal((await f.claim(f.second.request.id)).response.status, 200);
});

test('claim refuses unknown requests, malformed keys, unsafe routes and unauthorized body authority', async t => {
  const f = await fixture(t, { mullvad: false });
  const pair = await createEphemeralKeyPair();
  assert.equal((await f.request('/api/claim', { id: f.first.request.id, helperPublicKey: pair.publicKey, command: 'arbitrary' })).status, 400);
  assert.equal((await f.request('/api/claim', { id: '../other', helperPublicKey: pair.publicKey })).status, 400);
  assert.equal((await f.request('/api/claim', { id: f.first.request.id, helperPublicKey: 'bad' })).status, 400);
  assert.equal((await f.claim('f'.repeat(32))).response.status, 409);
  const denied = await f.claim(f.first.request.id); assert.equal(denied.response.status, 503); assert.deepEqual(await denied.response.json(), { error: 'vpn_not_verified' });
  assert.equal(f.claims.size, 0);
  assert.equal((await f.request('/api/frame', { data: 'synthetic' })).status, 404);
  assert.equal((await f.request('/api/queue?token=synthetic')).status, 400);
  assert.equal((await f.request('/api/claim', { id: f.first.request.id, helperPublicKey: pair.publicKey }, { 'Content-Type': 'text/plain' })).status, 503);
});

test('pending claim cancellation cannot leave a usable session or claim another request concurrently', async t => {
  const gate = pause(), f = await fixture(t, { routeGate: gate });
  const pending = f.claim(f.first.request.id);
  while (!f.calls.some(call => call.origin === 'https://am.i.mullvad.net')) await new Promise(resolve => setImmediate(resolve));
  assert.equal((await f.claim(f.second.request.id)).response.status, 409);
  assert.equal((await f.request('/api/stop', { id: f.first.request.id })).status, 200);
  gate.resolve(); assert.equal((await pending).response.status, 409);
  assert.equal(f.rows.has(f.first.request.id), false);
  assert.equal((await (await f.request('/api/queue')).json()).activeId, null);
});

test('queue or heartbeat timeout clears the active session and uses only the scoped stop token', async t => {
  for (const failure of ['failQueue', 'failHeartbeat']) {
    const f = await fixture(t); assert.equal((await f.claim(f.first.request.id)).response.status, 200);
    f.settings[failure] = true;
    const response = failure === 'failQueue' ? await f.request('/api/queue') : await f.request('/api/heartbeat', { id: f.first.request.id });
    assert.equal(response.status, 503); assert.equal(f.rows.has(f.first.request.id), false);
    f.settings[failure] = false;
    assert.equal((await (await f.request('/api/queue')).json()).activeId, null);
  }
});

test('the real outbound deadline aborts a stalled heartbeat and revokes its session', { timeout: 20_000 }, async t => {
  const f = await fixture(t); assert.equal((await f.claim(f.first.request.id)).response.status, 200);
  f.settings.hangHeartbeat = true;
  assert.equal((await f.request('/api/heartbeat', { id: f.first.request.id })).status, 503);
  assert.equal(f.calls.find(call => call.path.endsWith('/heartbeat')).init.signal.aborted, true);
  assert.equal(f.rows.has(f.first.request.id), false);
});

test('late heartbeat failure from a stopped request cannot stop the next request', async t => {
  const gate = pause(), f = await fixture(t);
  assert.equal((await f.claim(f.first.request.id)).response.status, 200);
  f.settings.heartbeatGate = gate;
  const pending = f.request('/api/heartbeat', { id: f.first.request.id });
  while (!f.calls.some(call => call.path.endsWith('/heartbeat'))) await new Promise(resolve => setImmediate(resolve));
  assert.equal((await f.request('/api/stop', { id: f.first.request.id })).status, 200);
  assert.equal((await f.claim(f.second.request.id)).response.status, 200);
  f.settings.failHeartbeat = true; gate.resolve(); assert.equal((await pending).status, 503);
  f.settings.failHeartbeat = false;
  assert.equal((await (await f.request('/api/queue')).json()).activeId, f.second.request.id);
});

test('a held pre-claim queue snapshot cannot revoke a newly claimed session', async t => {
  const f = await fixture(t), gate = { ...pause(), started: pause() };
  f.settings.queueGate = gate;
  const pending = f.request('/api/queue'); await gate.started.promise;
  assert.equal((await f.claim(f.first.request.id)).response.status, 200);
  gate.resolve(); const stale = await pending; assert.equal(stale.status, 200);
  assert.equal((await stale.json()).activeId, f.first.request.id);
  assert.equal(f.rows.get(f.first.request.id)?.status, 'claimed');
});

test('private identity requires owner-only nonsymlink files and the exact pinned public key', async t => {
  const f = await fixture(t);
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'pst-dashboard-test-')); await fs.chmod(directory, 0o700);
  const configPath = path.join(directory, 'operator.json'), linkPath = path.join(directory, 'linked.json');
  t.after(async () => { await fs.unlink(linkPath).catch(() => {}); await fs.unlink(configPath).catch(() => {}); await fs.rmdir(directory); });
  const operatorSigningJwk = await crypto.webcrypto.subtle.exportKey('jwk', f.signing.privateKey);
  await fs.writeFile(configPath, JSON.stringify({ v: 3, adminToken: f.adminToken, operatorSigningJwk }), { mode: 0o600, flag: 'wx' });
  const identity = await loadOperatorIdentity(configPath, f.service); assert.equal(identity.adminToken, f.adminToken); assert.equal(identity.operatorSigningKey.extractable, false);
  await fs.chmod(configPath, 0o644); await assert.rejects(loadOperatorIdentity(configPath, f.service), /private_config_required/); await fs.chmod(configPath, 0o600);
  await fs.symlink(configPath, linkPath); await assert.rejects(loadOperatorIdentity(linkPath, f.service));
  const other = await crypto.webcrypto.subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, false, ['sign', 'verify']);
  const otherPin = Buffer.from(await crypto.webcrypto.subtle.exportKey('raw', other.publicKey)).toString('base64url');
  await assert.rejects(loadOperatorIdentity(configPath, { ...f.service, operatorPublicKey: otherPin }), /operator_key_mismatch/);
  await fs.chmod(directory, 0o755); await assert.rejects(loadOperatorIdentity(configPath, f.service), /private_config_required/); await fs.chmod(directory, 0o700);
  assert.throws(() => validateDashboardService({ ...f.service, origin: '' }), /service_not_configured/);
  assert.throws(() => validateDashboardService({ ...f.service, origin: 'https://unrelated.invalid' }), /service_not_configured/);
});

test('launcher keeps the dashboard capability in owner-only temporary state and removes it on close', async t => {
  const f = await fixture(t), directory = await fs.mkdtemp(path.join(os.tmpdir(), 'pst-dashboard-launch-test-'));
  await fs.chmod(directory, 0o700);
  const configPath = path.join(directory, 'operator.json'), servicePath = path.join(directory, 'service.json');
  t.after(async () => { await fs.unlink(configPath).catch(() => {}); await fs.unlink(servicePath).catch(() => {}); await fs.rmdir(directory); });
  const operatorSigningJwk = await crypto.webcrypto.subtle.exportKey('jwk', f.signing.privateKey);
  await fs.writeFile(configPath, JSON.stringify({ v: 3, adminToken: f.adminToken, operatorSigningJwk }), { mode: 0o600, flag: 'wx' });
  await fs.writeFile(servicePath, JSON.stringify(f.service), { mode: 0o600, flag: 'wx' });
  const launched = await launchOperatorDashboard({ configPath, servicePath });
  t.after(() => launched.close());
  const state = JSON.parse(await fs.readFile(launched.stateFile, 'utf8')), url = new URL(state.dashboardURL);
  assert.equal((await fs.stat(path.dirname(launched.stateFile))).mode & 0o777, 0o700);
  assert.equal((await fs.stat(launched.stateFile)).mode & 0o777, 0o600);
  assert.equal(url.hostname, '127.0.0.1'); assert.equal(url.search, ''); assert.match(url.hash, /^#[a-f0-9]{64}$/);
  assert.deepEqual(Object.keys(state).sort(), ['dashboardURL', 'pid']);
  await launched.close(); await assert.rejects(fs.stat(launched.stateFile), { code: 'ENOENT' });
  await assert.rejects(fetch(url.origin));
  await fs.writeFile(servicePath, JSON.stringify({ ...f.service, origin: '' }), { mode: 0o600 });
  await assert.rejects(launchOperatorDashboard({ configPath, servicePath }), /service_not_configured/);
});

test('dashboard production code has no tunnel, private key, content relay or recording path', async () => {
  const source = await fs.readFile(new URL('../public/support/dashboard.js', import.meta.url), 'utf8');
  const html = await fs.readFile(new URL('../public/support/dashboard.html', import.meta.url), 'utf8');
  assert.doesNotMatch(source + html, /ADMIN_TOKEN|adminToken|operatorSigningJwk|console\.|localStorage|sessionStorage|WebSocket|innerHTML|eval\(/);
  assert.match(source, /history\.replaceState/); assert.match(source, /await verifyClaim\(result\.claim, service\)/);
  assert.match(source, /createSupportPeer\(\{ config, queue:/); assert.match(source, /}, 10000\)/); assert.match(source, /}, 3000\)/);
  assert.match(source, /record\.peer\.sendAction/); assert.doesNotMatch(source, /api\(['"](?:frame|action)/);
  assert.match(source, /removed\.has\(row\.id\)/); assert.match(source, /active !== record/);
  assert.match(html, /id="consent" type="checkbox" disabled/); assert.match(html, /id="stop"/);
  assert.match(html, /Shared app content is not recorded by this tool\./); assert.doesNotMatch(html, /Nothing here is recorded/);
});

async function dashboardUi(f) {
  class Element {
    children = []; value = ''; textContent = ''; disabled = false; checked = false; hidden = false; className = ''; width = 980; height = 750; handlers = {}; dataset = {}; replacements = 0;
    contains(element) { return this === element || this.children.some(child => child.contains(element)); }
    replaceChildren(...children) { if (this.children.some(child => child.contains(document.activeElement))) document.activeElement = null; this.replacements++; this.children = children; if (children.length && this.id === 'target') this.value = children[0].value; }
    append(...children) { this.children.push(...children); }
    setAttribute() {}
    focus() { if (!this.disabled) { this.focused = true; document.activeElement = this; } }
    addEventListener(type, callback) { this.handlers[type] = callback; }
    getContext() { return context; }
    getBoundingClientRect() { return { left: 0, top: 0, width: this.width, height: this.height }; }
  }
  let clears = 0; const drawn = [];
  const context = { clearRect() { clears++; }, fillRect() {}, save() {}, beginPath() {}, rect() {}, clip() {}, fillText(text, x, y) { drawn.push({ text, x, y }); }, restore() {} };
  const elements = new Map(), events = new Map(), timers = new Map(), intervals = new Map(), peers = [], history = [], settings = {};
  let sequence = 0;
  const document = { activeElement: null, getElementById(id) { if (!elements.has(id)) { const element = new Element(); element.id = id; elements.set(id, element); } return elements.get(id); }, createElement() { return new Element(); } };
  const peerFactory = async options => { const record = { options, closed: 0, suspended: 0, sent: [], acks: [], async approve() { if (settings.autoConnect !== false) options.onState('connected'); }, close() { this.closed++; options.onClose('stopped'); }, suspendForUpdate() { this.suspended++; return true; }, async sendUpdateHandoffAck(ack) { this.acks.push(ack); return true; }, async sendAction(action) { this.sent.push(action); } }; peers.push(record); return record; };
  const browserFetch = async (route, init) => {
    const url = new URL(route, f.dashboard.origin); assert.equal(url.origin, f.dashboard.origin);
    const response = await fetch(url, { ...init, headers: { ...init.headers, Origin: f.dashboard.origin } });
    if (route === '/api/queue' && settings.queueGate) { const gate = settings.queueGate; settings.queueGate = null; gate.started.resolve(); await gate.promise; }
    return response;
  };
  const source = (await fs.readFile(new URL('../public/support/dashboard.js', import.meta.url), 'utf8')).replace(/^import .+;\n/gm, '');
  const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
  const run = new AsyncFunction('createSupportPeer', 'createEphemeralKeyPair', 'verifyClaim', 'deriveClaimConfig', 'validateServiceConfig', 'validateUpdateHandoff', 'canonicalUpdateHandoff', 'ackProof', 'resumedGuestProof', 'resumedClaimProof', 'verifyUpdateProof', 'document', 'location', 'history', 'fetch', 'setInterval', 'clearInterval', 'setTimeout', 'clearTimeout', 'addEventListener', source + '\nreturn {connect,stop,refreshQueue,action,state:()=>({active,rows,ready})};');
  const ui = await run(peerFactory, createEphemeralKeyPair, verifyClaim, deriveClaimConfig, validateServiceConfig, validateUpdateHandoff, canonicalUpdateHandoff, ackProof, resumedGuestProof, resumedClaimProof, verifyUpdateProof, document,
    { hash: '#' + f.dashboard.capability, pathname: '/', search: '', origin: f.dashboard.origin }, { replaceState(...args) { history.push(args); } }, browserFetch,
    (callback, duration) => { const id = ++sequence; intervals.set(id, { callback, duration }); return id; }, id => intervals.delete(id),
    (callback, duration) => { const id = ++sequence; timers.set(id, { callback, duration }); return id; }, id => timers.delete(id),
    (name, callback) => events.set(name, callback));
  return { ui, document, elements, peers, timers, intervals, history, events, settings, drawn, clears: () => clears };
}

const settle = async predicate => {
  const deadline = Date.now() + 2000;
  while (!predicate()) { if (Date.now() >= deadline) throw new Error('Synthetic transition did not settle.'); await new Promise(resolve => setImmediate(resolve)); }
};
test('actual dashboard freezes controls, keeps the row and resumes only a HMAC-bound fresh peer on the same page', async t => {
  const f = await fixture(t), browser = await dashboardUi(f), $ = id => browser.elements.get(id);
  $('consent').checked = true; await browser.ui.connect(browser.ui.state().rows[0]);
  const original = browser.ui.state().active, oldPeer = browser.peers[0], initialClaim = f.claims.get(original.id);
  const handoff = handoffFor(f, initialClaim), resumeAuth = crypto.randomBytes(32).toString('hex');
  f.rows.set(original.id, { ...f.rows.get(original.id), status: 'updating', handoff });
  const frame = { v: 1, id: 'a'.repeat(32), width: 980, height: 750, items: [], targets: [{ id: 'b'.repeat(32), x: 0, y: 0, w: 10, h: 10, kind: 'click', label: 'Fictional navigation' }], truncated: false };
  oldPeer.options.onFrame(frame); assert.equal($('activate').disabled, false);
  oldPeer.options.onUpdateHandoff({ handoff, resumeAuth });
  assert.equal(browser.ui.state().active.frame, undefined); assert.equal($('controls').hidden, true); assert.equal($('activate').disabled, true);
  await settle(() => oldPeer.acks.length === 1);
  assert.deepEqual(oldPeer.acks[0], { nonce: handoff.nonce, proof: await ackProof(resumeAuth, handoff) });
  await browser.ui.action(frame.targets[0]); oldPeer.options.onFrame(frame); assert.equal(oldPeer.sent.length, 0); assert.equal(browser.ui.state().active.frame, undefined);
  await browser.ui.refreshQueue(); assert.equal(browser.ui.state().active.id, original.id);
  assert.match($('queue').children[0].children[1].children[1].textContent, /Updating/); assert.equal($('stop').disabled, false);
  await publishResuming(f, { handoff, resumeAuth }); await browser.ui.refreshQueue();
  await settle(() => browser.ui.state().active?.connected === true && browser.ui.state().active !== original);
  const current = browser.ui.state().active;
  assert.equal(current.id, original.id); assert.notEqual(current.sid, original.sid); assert.equal(current.expires, original.expires);
  assert.equal(current.grant, undefined); assert.equal(original.grant, undefined); assert.equal(oldPeer.closed, 0); assert.equal(oldPeer.suspended, 1);
  assert.equal($('consent').checked, false); assert.equal(browser.peers.length, 2);
  oldPeer.options.onClose('stopped'); oldPeer.options.onFrame(frame); assert.equal(browser.ui.state().active, current); assert.equal(current.frame, undefined);
  browser.peers[1].options.onFrame(frame); await browser.ui.action(frame.targets[0]); assert.equal(browser.peers[1].sent.length, 1);
  await browser.ui.stop(); assert.equal(f.rows.has(current.id), false);
});
test('actual dashboard Stop during update clears the grant and prevents automatic resumption', async t => {
  const f = await fixture(t), browser = await dashboardUi(f), $ = id => browser.elements.get(id);
  $('consent').checked = true; await browser.ui.connect(browser.ui.state().rows[0]);
  const record = browser.ui.state().active, handoff = handoffFor(f, f.claims.get(record.id)), resumeAuth = crypto.randomBytes(32).toString('hex');
  f.rows.set(record.id, { ...f.rows.get(record.id), status: 'updating', handoff });
  browser.peers[0].options.onUpdateHandoff({ handoff, resumeAuth }); await settle(() => browser.peers[0].acks.length === 1);
  const grant = record.grant; await browser.ui.stop();
  assert.equal(record.grant, undefined); assert.equal(grant.resumeAuth, ''); assert.equal(f.rows.has(record.id), false);
  await browser.ui.refreshQueue(); assert.equal(browser.peers.length, 1); assert.equal(browser.ui.state().active, undefined);
});
for (const ending of ['expiry', 'service failure', 'page close']) test(`actual dashboard ${ending} clears the update grant and never reconnects another request`, async t => {
  const f = await fixture(t), browser = await dashboardUi(f), $ = id => browser.elements.get(id);
  $('consent').checked = true; await browser.ui.connect(browser.ui.state().rows[0]);
  const record = browser.ui.state().active, handoff = handoffFor(f, f.claims.get(record.id)), resumeAuth = crypto.randomBytes(32).toString('hex');
  f.rows.set(record.id, { ...f.rows.get(record.id), status: 'updating', handoff });
  browser.peers[0].options.onUpdateHandoff({ handoff, resumeAuth }); await settle(() => browser.peers[0].acks.length === 1);
  const grant = record.grant;
  if (ending === 'expiry') [...browser.timers.values()].find(timer => timer.duration > 3000 && timer.duration <= 45000).callback();
  else if (ending === 'page close') browser.events.get('pagehide')();
  else { f.settings.failQueue = true; await browser.ui.refreshQueue(); }
  await settle(() => !f.rows.has(record.id));
  assert.equal(record.grant, undefined); assert.equal(grant.resumeAuth, ''); assert.equal(browser.ui.state().active, undefined);
  assert.equal($('controls').hidden, true); assert.equal($('stop').disabled, true); assert.equal(browser.peers.length, 1);
  f.settings.failQueue = false; await browser.ui.refreshQueue(); assert.equal(browser.peers.length, 1);
});
test('forged fresh guest proof fails closed and cannot be used after grant loss', async t => {
  const f = await fixture(t), grant = await beginUpdate(f), id = grant.handoff.parentId;
  assert.equal((await f.request('/api/handoff', { id, handoff: grant.handoff, resumeAuth: grant.resumeAuth })).status, 200);
  await publishResuming(f, grant, { resumeGuestProof: '0'.repeat(64) });
  const pair = await createEphemeralKeyPair();
  assert.equal((await f.request('/api/resume', { id, helperPublicKey: pair.publicKey, resumeAuth: grant.resumeAuth })).status, 503);
  assert.equal(f.rows.has(id), false); assert.equal((await (await f.request('/api/queue')).json()).activeId, null);
  assert.equal((await f.request('/api/resume', { id, helperPublicKey: pair.publicKey, resumeAuth: grant.resumeAuth })).status, 409);
  assert.equal(f.calls.filter(call => call.path.endsWith('/claim')).length, 1);
});

test('actual dashboard preserves Connect nodes and keyboard focus across unchanged queue refreshes', async t => {
  const f = await fixture(t), browser = await dashboardUi(f), queue = browser.elements.get('queue'), consent = browser.elements.get('consent');
  consent.checked = true; consent.onchange();
  const button = queue.children[1].children[2], replacements = queue.replacements;
  button.focus(); assert.equal(browser.document.activeElement, button);
  await browser.ui.refreshQueue(); await browser.ui.refreshQueue();
  assert.equal(queue.children[1].children[2], button);
  assert.equal(browser.document.activeElement, button); assert.equal(queue.replacements, replacements);
  assert.equal(f.claims.size, 0);
});

test('actual dashboard preserves keyboard focus by request when the queue changes or safely removes it', async t => {
  const f = await fixture(t), browser = await dashboardUi(f), queue = browser.elements.get('queue'), consent = browser.elements.get('consent');
  consent.checked = true; consent.onchange();
  const selectedId = browser.ui.state().rows[1].id, button = queue.children[1].children[2]; button.focus();
  f.settings.queueSnapshot = [...f.rows.values()].reverse(); await browser.ui.refreshQueue();
  const moved = queue.children[0].children[2];
  assert.equal(browser.document.activeElement, moved); assert.equal(moved.dataset.requestId, selectedId); assert.equal(moved.disabled, false);
  f.settings.queueSnapshot = undefined; f.rows.delete(selectedId); await browser.ui.refreshQueue();
  assert.equal(browser.document.activeElement, consent); assert.equal(f.claims.size, 0);
});

test('actual dashboard ignores a held pre-connect queue response after the connection generation changes', async t => {
  const f = await fixture(t), browser = await dashboardUi(f), gate = { ...pause(), started: pause() };
  browser.settings.queueGate = gate;
  const pending = browser.ui.refreshQueue(); await gate.started.promise;
  browser.elements.get('consent').checked = true; await browser.ui.connect(browser.ui.state().rows[0]);
  const id = browser.ui.state().active.id;
  gate.resolve(); await pending;
  assert.equal(browser.ui.state().active?.id, id); assert.equal(browser.peers[0].closed, 0);
  await browser.ui.stop();
});

test('actual dashboard handlers require approval, stop synchronously, suppress stale rows and isolate the next session', async t => {
  const f = await fixture(t), browser = await dashboardUi(f), $ = id => browser.elements.get(id);
  assert.equal(browser.ui.state().ready, true); assert.equal(browser.history[0][2], '/');
  const first = browser.ui.state().rows.find(row => row.id === f.first.request.id);
  await browser.ui.connect(first); assert.equal(browser.peers.length, 0); assert.equal(f.claims.size, 0);
  $('consent').checked = true; $('consent').onchange(); await browser.ui.connect(first);
  assert.equal(browser.peers.length, 1); assert.equal($('consent').checked, false); assert.equal($('stop').disabled, false);
  const oldPeer = browser.peers[0];
  const frame = { v: 1, id: 'a'.repeat(32), width: 980, height: 750, items: [], targets: [{ id: 'b'.repeat(32), x: 10, y: 10, w: 100, h: 30, kind: 'click', label: 'Synthetic navigation' }], truncated: false };
  oldPeer.options.onFrame(frame); assert.equal($('controls').hidden, false); assert.equal($('activate').disabled, false);
  await browser.ui.action(frame.targets[0]); assert.equal(oldPeer.sent.length, 1);
  const before = browser.clears(), stopped = browser.ui.stop();
  assert.equal(browser.ui.state().active, undefined); assert.equal(oldPeer.closed, 1); assert.equal($('controls').hidden, true); assert.equal($('activate').disabled, true); assert.ok(browser.clears() > before);
  assert.equal(browser.ui.state().rows.some(row => row.id === first.id), false); await stopped;
  f.settings.queueSnapshot = [first, ...f.rows.values()]; await browser.ui.refreshQueue();
  assert.equal(browser.ui.state().rows.some(row => row.id === first.id), false); f.settings.queueSnapshot = undefined;
  const second = browser.ui.state().rows.find(row => row.id === f.second.request.id);
  $('consent').checked = true; await browser.ui.connect(second); assert.equal(browser.peers.length, 2);
  oldPeer.options.onClose('stopped'); oldPeer.options.onFrame(frame);
  assert.equal(browser.ui.state().active.id, second.id); assert.equal(browser.ui.state().active.frame, undefined);
  browser.peers[1].options.onFrame(frame);
  const freshness = [...browser.timers.values()].find(timer => timer.duration === 3000); assert.ok(freshness); freshness.callback();
  assert.equal(browser.ui.state().active.frame, undefined); assert.equal($('controls').hidden, true); assert.equal($('activate').disabled, true);
  assert.equal([...browser.intervals.values()].filter(timer => timer.duration === 10000).length, 1);
  await browser.ui.stop(); assert.equal([...browser.intervals.values()].some(timer => timer.duration === 10000), false);
});

test('actual dashboard clears queue and peer on service failure and does not reconnect without another Connect', async t => {
  const f = await fixture(t), browser = await dashboardUi(f), $ = id => browser.elements.get(id);
  $('consent').checked = true; await browser.ui.connect(browser.ui.state().rows[0]);
  f.settings.failQueue = true; await browser.ui.refreshQueue();
  assert.equal(browser.ui.state().active, undefined); assert.deepEqual(browser.ui.state().rows, []); assert.equal(browser.peers[0].closed, 1);
  assert.equal($('controls').hidden, true); assert.equal($('stop').disabled, true);
  f.settings.failQueue = false; await browser.ui.refreshQueue();
  assert.equal(browser.peers.length, 1); assert.equal(browser.ui.state().active, undefined); assert.equal($('consent').checked, false);
});

test('actual dashboard reports Connecting until verified and renders bounded multiline drawing text', async t => {
  const f = await fixture(t), browser = await dashboardUi(f), $ = id => browser.elements.get(id);
  browser.settings.autoConnect = false; $('consent').checked = true; await browser.ui.connect(browser.ui.state().rows[0]);
  assert.equal(browser.ui.state().active.connected, false);
  const note = () => $('queue').children[0].children[1].children[1].textContent;
  assert.equal(note(), 'Connecting on this laptop');
  browser.peers[0].options.onState('connected'); assert.equal(note(), 'Connected on this laptop');
  browser.peers[0].options.onFrame({ v: 1, id: 'a'.repeat(32), width: 980, height: 750,
    items: [{ kind: 'text', x: 10, y: 20, w: 200, h: 50, text: 'Fictional first\nFictional second', color: 'rgb(10, 20, 30)', size: 14, weight: 400 }], targets: [], truncated: false });
  assert.deepEqual(browser.drawn, [{ text: 'Fictional first', x: 10, y: 20 }, { text: 'Fictional second', x: 10, y: 37.5 }]);
  await browser.ui.stop();
});
