import test from 'node:test';
import assert from 'node:assert/strict';
import http from 'node:http';
import fs from 'node:fs/promises';
import vm from 'node:vm';
import { createSupportServer, validSupportOrigin } from '../scripts/support-server.mjs';
import { pinggyArguments } from '../scripts/support-session.mjs';

const assets = { html: '<title>Private controller</title>', controller: 'export const controller = true;', protocol: 'export const protocol = true;', peer: 'export const peer = true;', css: 'body{margin:0}' };
const publicOrigin = 'https://synthetic-session.run.pinggy-free.link';
const appOrigin = 'tauri://localhost';
async function setup(t, options = {}) {
  const server = await createSupportServer({ assets, ...options });
  server.setGuestOrigin(publicOrigin);
  t.after(() => server.close());
  return server;
}
function request(server, role, route, { method = 'GET', token, origin, host, body, headers = {} } = {}) {
  const port = server[`${role}Port`];
  const expected = role === 'guest' ? publicOrigin : `http://127.0.0.1:${port}`;
  const payload = body === undefined ? undefined : typeof body === 'string' ? body : JSON.stringify(body);
  return new Promise((resolve, reject) => {
    const req = http.request({ hostname: '127.0.0.1', port, path: route, method, headers: {
      Host: host || new URL(expected).host,
      ...(token ? { Authorization: 'Bearer ' + token } : {}),
      ...(origin === null ? {} : { Origin: origin || expected }),
      ...(payload === undefined ? {} : { 'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(payload) }),
      ...headers,
    } }, res => {
      const parts = [];
      res.on('data', chunk => parts.push(chunk));
      res.on('end', () => {
        const text = Buffer.concat(parts).toString();
        let json;
        try { json = JSON.parse(text); } catch { /* Static assets are not JSON. */ }
        resolve({ status: res.statusCode, headers: res.headers, text, json });
      });
    });
    req.on('error', reject);
    req.end(payload);
  });
}
function envelope(server, role, seq = 1, size = 32) {
  return { v: 1, sid: server.sid, from: role, seq, expires: server.expiresAt,
    iv: Buffer.alloc(12, seq).toString('base64url'), ct: Buffer.alloc(size, seq).toString('base64url') };
}
const send = (server, role, body, options = {}) => request(server, role, '/api/send', {
  method: 'POST', token: server[`${role}Token`], body, ...options,
});

test('support origin accepts only exact HTTPS Pinggy free hosts', () => {
  assert.equal(validSupportOrigin(publicOrigin), true);
  for (const value of ['https://run.pinggy-free.link', 'http://synthetic.run.pinggy-free.link',
    'https://a.b.run.pinggy-free.link', 'https://a.run.pinggy-free.link.example',
    'https://a.run.pinggy-free.link:443', 'https://a.run.pinggy-free.link/path',
    'https://user@a.run.pinggy-free.link', 'https://-a.run.pinggy-free.link',
    'https://' + 'a'.repeat(64) + '.run.pinggy-free.link']) assert.equal(validSupportOrigin(value), false);
});

test('guest tunnel never exposes controller assets, secrets, or control endpoints', async t => {
  const server = await setup(t);
  assert.notEqual(server.controllerPort, server.guestPort);
  assert.notEqual(server.controllerToken, server.guestToken);
  const page = await request(server, 'guest', '/join', { origin: null });
  assert.equal(page.status, 200);
  for (const secret of [server.sid, server.controllerToken, server.guestToken]) assert.ok(!page.text.includes(secret));
  for (const route of ['/', '/support/controller.js', '/support/protocol.js', '/support/peer.js', '/support/session.js', '/session.json', '/api/stop']) {
    assert.equal((await request(server, 'guest', route, { token: server.controllerToken })).status, 404);
  }
  const controller = await request(server, 'controller', '/');
  assert.equal(controller.status, 200);
  assert.equal(controller.headers['cache-control'], 'no-store');
  assert.match(controller.headers['content-security-policy'], /script-src 'self'/);
  assert.match(controller.headers['permissions-policy'], /display-capture=\(\)/);
  assert.equal(controller.headers['referrer-policy'], 'no-referrer');
  assert.equal((await request(server, 'controller', '/join')).status, 404);
  assert.equal((await request(server, 'controller', '/../../etc/passwd')).status, 400);
});

test('CORS permits the app and rejects arbitrary, null, and missing guest origins', async t => {
  const server = await setup(t);
  const preflight = await request(server, 'guest', '/api/send', { method: 'OPTIONS', origin: appOrigin, headers: {
    'Access-Control-Request-Method': 'POST', 'Access-Control-Request-Headers': 'authorization, content-type, x-pinggy-no-screen',
  } });
  assert.equal(preflight.status, 204);
  assert.equal(preflight.headers['access-control-allow-origin'], appOrigin);
  assert.equal(preflight.headers.vary, 'Origin');
  assert.equal(preflight.headers['access-control-allow-credentials'], undefined);
  assert.match(preflight.headers['access-control-allow-headers'], /X-Pinggy-No-Screen/);
  assert.equal((await send(server, 'guest', envelope(server, 'guest'), { origin: appOrigin, headers: { 'Sec-Fetch-Site': 'cross-site' } })).status, 200);
  for (const origin of ['null', 'https://untrusted.example', 'http://localhost:1420', 'http://tauri.localhost', null]) {
    const result = await request(server, 'guest', '/api/poll?after=0', { origin, token: server.guestToken });
    assert.equal(result.status, 403);
    assert.equal(result.headers['access-control-allow-origin'], undefined);
  }
  assert.equal((await request(server, 'guest', '/api/send', { method: 'OPTIONS', origin: appOrigin, headers: {
    'Access-Control-Request-Method': 'DELETE', 'Access-Control-Request-Headers': 'authorization',
  } })).status, 403);
  assert.equal((await request(server, 'guest', '/api/send', { method: 'OPTIONS', origin: appOrigin, headers: {
    'Access-Control-Request-Method': 'POST', 'Access-Control-Request-Headers': 'x-unapproved',
  } })).status, 403);
  assert.equal((await request(server, 'controller', '/api/poll?after=0', { origin: appOrigin, token: server.controllerToken })).status, 403);
});

test('role credentials, Host, Origin, and cross-site requests fail closed', async t => {
  const server = await setup(t);
  for (const role of ['controller', 'guest']) {
    const token = server[`${role}Token`];
    assert.equal((await request(server, role, '/api/poll?after=0')).status, 401);
    assert.equal((await request(server, role, '/api/poll?after=0', { token: server[`${role === 'guest' ? 'controller' : 'guest'}Token`] })).status, 401);
    assert.equal((await request(server, role, '/api/poll?after=0', { token, host: 'localhost' })).status, 421);
    assert.equal((await send(server, role, envelope(server, role), { origin: null })).status, 403);
    assert.equal((await request(server, role, '/api/poll?after=0', { token, headers: { 'Sec-Fetch-Site': 'cross-site' } })).status, 403);
  }
});

test('encrypted queues are ordered, directional, and exact-retry idempotent', async t => {
  const server = await setup(t);
  const first = envelope(server, 'controller');
  assert.equal((await send(server, 'controller', first)).status, 200);
  assert.equal((await send(server, 'controller', first)).status, 200);
  assert.equal((await send(server, 'controller', { ...first, ct: Buffer.alloc(32, 9).toString('base64url') })).status, 409);
  assert.equal((await send(server, 'controller', envelope(server, 'controller', 3))).status, 409);
  assert.equal((await send(server, 'controller', envelope(server, 'controller', 2))).status, 200);
  const guest = await request(server, 'guest', '/api/poll?after=0', { token: server.guestToken, origin: appOrigin });
  assert.deepEqual(guest.json.messages.map(message => message.seq), [1, 2]);
  assert.equal(guest.json.expiresAt, server.expiresAt);
  assert.equal((await request(server, 'controller', '/api/poll?after=0', { token: server.controllerToken })).json.messages.length, 0);
});

test('server rejects plaintext frames, malformed envelopes, and oversized input', async t => {
  const server = await setup(t);
  const valid = envelope(server, 'guest');
  for (const invalid of [{ kind: 'frame', text: 'Fictional test only' }, { ...valid, frame: {} },
    { ...valid, from: 'controller' }, { ...valid, sid: '0'.repeat(32) }, { ...valid, expires: server.expiresAt + 1 },
    { ...valid, seq: 129 }, { ...valid, iv: 'A' }, { ...valid, ct: '='.repeat(22) }, null, []]) {
    assert.equal((await send(server, 'guest', invalid, { origin: appOrigin })).status, 400);
  }
  assert.equal((await send(server, 'guest', 'x'.repeat(24 * 1024 + 1), { origin: appOrigin })).status, 413);
  for (const query of ['after=-1', 'after=129', 'after=00', 'after=0&token=x', 'after=1&after=2']) {
    assert.equal((await request(server, 'guest', '/api/poll?' + query, { token: server.guestToken })).status, 400);
  }
  for (const route of ['/api/frame', '/api/action', '/api/export', '/api/files']) assert.equal((await request(server, 'guest', route)).status, 404);
});

test('expiry closes both listeners and calls shutdown once', async () => {
  let count = 0, server;
  const stopped = new Promise(async (resolve, reject) => {
    try { server = await createSupportServer({ assets, ttlMs: 80, onStop: details => { count++; resolve(details); } }); }
    catch (error) { reject(error); }
  });
  assert.deepEqual(await stopped, { reason: 'expired' });
  await server.close();
  assert.equal(count, 1);
  await assert.rejects(request(server, 'controller', '/'));
});

test('only authenticated controller can stop the service', async t => {
  let resolveStop;
  const stopped = new Promise(resolve => { resolveStop = resolve; });
  const server = await setup(t, { onStop: resolveStop });
  assert.equal((await request(server, 'guest', '/api/stop', { method: 'POST', token: server.guestToken, body: {} })).status, 404);
  assert.equal((await request(server, 'controller', '/api/stop', { method: 'POST', body: {} })).status, 401);
  assert.equal((await request(server, 'controller', '/api/stop', { method: 'POST', token: server.controllerToken, body: {} })).status, 200);
  assert.deepEqual(await stopped, { reason: 'controller_stopped' });
});

test('SSH arguments use only the designated service and explicit first-use trust', () => {
  const args = pinggyArguments({ port: 40000, knownHosts: '/tmp/synthetic-known-hosts' });
  assert.ok(args.includes('StrictHostKeyChecking=yes'));
  assert.ok(args.includes('IdentityAgent=none'));
  assert.ok(args.includes('PreferredAuthentications=none'));
  assert.ok(args.includes('GlobalKnownHostsFile=/dev/null'));
  assert.ok(args.includes('0:127.0.0.1:40000'));
  assert.ok(args.includes('http'));
  assert.ok(args.includes('free.pinggy.io'));
  assert.ok(args.includes('x:noreverseproxy'));
  assert.ok(!args.some(arg => arg === '-L' || arg === '-D' || /StrictHostKeyChecking=no/.test(arg)));
  const firstUse = pinggyArguments({ port: 40000, knownHosts: '/tmp/synthetic-known-hosts', acceptNew: true });
  assert.ok(firstUse.includes('StrictHostKeyChecking=accept-new'));
  assert.throws(() => pinggyArguments({ port: 22, knownHosts: '/tmp/synthetic-known-hosts' }));
});

test('controller clears a stale view and disables controls, resumes fresh frames, and cancels timers on Stop', async () => {
  // Run the actual controller module with only its DOM and peer boundary replaced.
  // This verifies UI lifecycle behavior without claiming a browser/network pass.
  const source = await fs.readFile(new URL('../public/support/controller.js', import.meta.url), 'utf8');
  const elements = new Map();
  const timers = new Map();
  let nextTimer = 0, callbacks, closes = 0, stops = 0, clears = 0;
  const drawing = { clearRect() { clears++; }, fillRect() {}, save() {}, beginPath() {}, rect() {}, clip() {}, fillText() {}, restore() {} };
  const element = id => {
    if (!elements.has(id)) elements.set(id, {
      value: '', hidden: false, disabled: false, checked: false, textContent: '', className: '',
      children: [], addEventListener() {}, focus() {}, select() {},
      replaceChildren(...children) { this.children = children; this.value = children[0]?.value || ''; },
      getContext() { return drawing; },
      getBoundingClientRect() { return { left: 0, top: 0, width: 980, height: 680 }; },
    });
    return elements.get(id);
  };
  const common = { v: 2, sid: 'a'.repeat(32), key: Buffer.alloc(32).toString('base64url'), expires: Date.now() + 60000, expectedExit: '1.1.1.1' };
  const guest = { ...common, role: 'guest', token: 'b'.repeat(64), origin: publicOrigin };
  const invite = publicOrigin + '/join#' + Buffer.from(JSON.stringify(guest)).toString('base64url');
  const config = { ...common, role: 'controller', token: 'c'.repeat(64), origin: 'http://127.0.0.1:40000', invite };
  const context = {
    document: { getElementById: element, createElement: () => ({ value: '', textContent: '' }) },
    location: { hash: '#' + Buffer.from(JSON.stringify(config)).toString('base64url'), origin: config.origin, pathname: '/' },
    history: { replaceState() {} }, URL, atob, Date, performance,
    AbortSignal: { timeout: () => ({}) },
    navigator: { clipboard: { writeText: async () => {} } },
    addEventListener() {},
    fetch: async () => { stops++; return { ok: true }; },
    setTimeout: (callback, delay) => { const id = ++nextTimer; timers.set(id, { callback, delay }); return id; },
    clearTimeout: id => timers.delete(id),
    validateConfig: value => value,
    createSupportPeer: async value => { callbacks = value; return { close() { closes++; }, approve: async () => true, sendAction: () => true }; },
  };
  await vm.runInNewContext('(async () => {\n' + source.replace(/^import .+;\n/gm, '') + '\n})()', context);
  assert.ok(callbacks);
  callbacks.onState('connected');
  const frame = id => ({ v: 1, id: id.repeat(32), width: 980, height: 680, items: [], truncated: false,
    targets: [{ id: 'd'.repeat(32), x: 0, y: 0, w: 100, h: 30, kind: 'click', label: 'Synthetic control' }] });
  callbacks.onFrame(frame('e'));
  assert.equal(element('controls').hidden, false);
  assert.equal(element('activate').disabled, false);
  const staleTimer = [...timers.entries()].find(([, timer]) => timer.delay === 3000);
  assert.ok(staleTimer);
  staleTimer[1].callback();
  assert.equal(element('controls').hidden, true);
  assert.equal(element('target').disabled, true);
  assert.equal(element('activate').disabled, true);
  assert.equal(element('send-input').disabled, true);
  assert.equal(element('scroll-up').disabled, true);
  assert.match(element('status').textContent, /Waiting for a fresh app view/);
  assert.ok(clears > 0);
  assert.equal(closes, 0);
  assert.equal(stops, 0);
  callbacks.onFrame(frame('f'));
  assert.equal(element('controls').hidden, false);
  assert.equal(element('activate').disabled, false);
  element('stop').onclick();
  assert.equal(element('controls').hidden, true);
  assert.equal(element('activate').disabled, true);
  assert.equal(timers.size, 0);
  assert.equal(closes, 1);
  assert.equal(stops, 1);
});
