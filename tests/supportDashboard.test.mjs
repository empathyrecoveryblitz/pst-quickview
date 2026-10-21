import test from 'node:test';
import assert from 'node:assert/strict';
import crypto from 'node:crypto';
import fs from 'node:fs/promises';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import { createOperatorDashboard, launchOperatorDashboard, loadOperatorIdentity, validateDashboardService } from '../scripts/support-dashboard.mjs';
import { createRequest, createEphemeralKeyPair, verifyClaim, deriveClaimConfig, validateServiceConfig } from '../public/support/queue-protocol.js';

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
  const calls = [], claims = new Map();
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
      const row = rows.get(claim[1]); assert.ok(row); row.status = 'claimed'; claims.set(row.id, value);
      return json({ token: controllerToken, claim: value });
    }
    const session = /^\/v3\/requests\/([a-f0-9]{32})\/(heartbeat|stop)$/.exec(parsed.pathname);
    assert.ok(session); assert.equal(init.headers.Authorization, 'Bearer ' + controllerToken);
    assert.equal(init.body, '{}');
    if (session[2] === 'stop') { rows.delete(session[1]); return json({ stopped: true }); }
    if (settings.hangHeartbeat) await new Promise((_, reject) => { init.signal.addEventListener('abort', () => reject(init.signal.reason), { once: true }); });
    if (settings.heartbeatGate) { const gate = settings.heartbeatGate; settings.heartbeatGate = null; await gate.promise; if (settings.failHeartbeat) throw new DOMException('Synthetic timeout', 'TimeoutError'); }
    if (settings.failHeartbeat) throw new DOMException('Synthetic timeout', 'TimeoutError');
    return json({ active: true });
  };
  const dashboard = await createOperatorDashboard({ service, adminToken, operatorSigningKey: signing.privateKey, assets, fetchImpl });
  t.after(() => dashboard.close());
  const request = (route, body, extra = {}) => fetch(dashboard.origin + route, {
    method: body === undefined ? 'GET' : 'POST', headers: { Origin: dashboard.origin, Authorization: 'Bearer ' + dashboard.capability, ...(body === undefined ? {} : { 'Content-Type': 'application/json' }), ...extra },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }),
  });
  const claim = async id => { const pair = await createEphemeralKeyPair(); const response = await request('/api/claim', { id, helperPublicKey: pair.publicKey }); return { pair, response }; };
  return { dashboard, request, claim, first, second, rows, calls, claims, service, signing, adminToken, controllerToken, settings };
}

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
    children = []; value = ''; textContent = ''; disabled = false; checked = false; hidden = false; className = ''; width = 980; height = 750; handlers = {};
    replaceChildren(...children) { this.children = children; if (children.length && this.id === 'target') this.value = children[0].value; }
    append(...children) { this.children.push(...children); }
    setAttribute() {}
    focus() { this.focused = true; }
    addEventListener(type, callback) { this.handlers[type] = callback; }
    getContext() { return context; }
    getBoundingClientRect() { return { left: 0, top: 0, width: this.width, height: this.height }; }
  }
  let clears = 0; const drawn = [];
  const context = { clearRect() { clears++; }, fillRect() {}, save() {}, beginPath() {}, rect() {}, clip() {}, fillText(text, x, y) { drawn.push({ text, x, y }); }, restore() {} };
  const elements = new Map(), events = new Map(), timers = new Map(), intervals = new Map(), peers = [], history = [], settings = {};
  let sequence = 0;
  const document = { getElementById(id) { if (!elements.has(id)) { const element = new Element(); element.id = id; elements.set(id, element); } return elements.get(id); }, createElement() { return new Element(); } };
  const peerFactory = async options => { const record = { options, closed: 0, sent: [], async approve() { if (settings.autoConnect !== false) options.onState('connected'); }, close() { this.closed++; options.onClose('stopped'); }, async sendAction(action) { this.sent.push(action); } }; peers.push(record); return record; };
  const browserFetch = async (route, init) => {
    const url = new URL(route, f.dashboard.origin); assert.equal(url.origin, f.dashboard.origin);
    const response = await fetch(url, { ...init, headers: { ...init.headers, Origin: f.dashboard.origin } });
    if (route === '/api/queue' && settings.queueGate) { const gate = settings.queueGate; settings.queueGate = null; gate.started.resolve(); await gate.promise; }
    return response;
  };
  const source = (await fs.readFile(new URL('../public/support/dashboard.js', import.meta.url), 'utf8')).replace(/^import .+;\n/gm, '');
  const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
  const run = new AsyncFunction('createSupportPeer', 'createEphemeralKeyPair', 'verifyClaim', 'deriveClaimConfig', 'validateServiceConfig', 'document', 'location', 'history', 'fetch', 'setInterval', 'clearInterval', 'setTimeout', 'clearTimeout', 'addEventListener', source + '\nreturn {connect,stop,refreshQueue,action,state:()=>({active,rows,ready})};');
  const ui = await run(peerFactory, createEphemeralKeyPair, verifyClaim, deriveClaimConfig, validateServiceConfig, document,
    { hash: '#' + f.dashboard.capability, pathname: '/', search: '', origin: f.dashboard.origin }, { replaceState(...args) { history.push(args); } }, browserFetch,
    (callback, duration) => { const id = ++sequence; intervals.set(id, { callback, duration }); return id; }, id => intervals.delete(id),
    (callback, duration) => { const id = ++sequence; timers.set(id, { callback, duration }); return id; }, id => timers.delete(id),
    (name, callback) => events.set(name, callback));
  return { ui, elements, peers, timers, intervals, history, events, settings, drawn, clears: () => clears };
}

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
