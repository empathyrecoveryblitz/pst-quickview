import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import worker, { SupportQueue, QUEUE_LIMITS } from '../cloudflare/support/worker.mjs';
import { createRequest, createEphemeralKeyPair, createClaim, deriveClaimConfig, encodeQueueBytes } from '../public/support/queue-protocol.js';
import { createCipher } from '../public/support/protocol.js';

class FakeStorage {
  constructor() { this.data = new Map(); this.alarmAt = null; }
  async get(key) { return structuredClone(this.data.get(key)); }
  async put(key, value) {
    assert.ok(new TextEncoder().encode(JSON.stringify(value)).length < 128 * 1024);
    this.data.set(key, structuredClone(value));
  }
  async list({ prefix = '', limit = 10000 } = {}) { return new Map([...this.data].filter(([key]) => key.startsWith(prefix)).sort(([a], [b]) => a.localeCompare(b)).slice(0, limit).map(([key, value]) => [key, structuredClone(value)])); }
  async delete(keys) { let count = 0; for (const key of Array.isArray(keys) ? keys : [keys]) count += this.data.delete(key) ? 1 : 0; return count; }
  async transaction(operation) {
    const original = structuredClone(this.data);
    try { return await operation(this); } catch (error) { this.data = original; throw error; }
  }
  async getAlarm() { return this.alarmAt; }
  async setAlarm(value) { this.alarmAt = value; }
  async deleteAlarm() { this.alarmAt = null; }
}
async function setup(options = {}) {
  const operator = await crypto.subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, true, ['sign', 'verify']);
  const service = { v: 3, origin: 'https://synthetic.example.workers.dev', operatorPublicKey: encodeQueueBytes(new Uint8Array(await crypto.subtle.exportKey('raw', operator.publicKey))), operatorLabel: 'Synthetic support' };
  const env = { SERVICE_ORIGIN: service.origin, OPERATOR_PUBLIC_KEY: service.operatorPublicKey, OPERATOR_LABEL: service.operatorLabel, ADMIN_TOKEN: 'a'.repeat(64) };
  const storage = new FakeStorage(); let now = Date.now();
  const queue = new SupportQueue({ storage }, env, { now: () => now, ...options });
  const send = async (path, { method = 'GET', token, body, origin = 'tauri://localhost', headers = {} } = {}) => {
    const response = await queue.fetch(new Request(service.origin + path, { method, headers: {
      ...(origin === null ? {} : { Origin: origin }), ...(token ? { Authorization: `Bearer ${token}` } : {}),
      ...(body === undefined ? {} : { 'Content-Type': 'application/json' }), ...headers,
    }, ...(body === undefined ? {} : { body: typeof body === 'string' ? body : JSON.stringify(body) }) }));
    return { status: response.status, headers: response.headers, body: response.status === 204 ? null : await response.json() };
  };
  const register = async (options = {}) => {
    const guest = await createRequest({ expires: Date.now() + 120000 });
    assert.equal((await send('/v3/requests', { method: 'POST', body: guest.request, ...options })).status, 200);
    return guest;
  };
  const claim = async guest => {
    const helper = await createEphemeralKeyPair();
    const claim = await createClaim(guest.request, helper.publicKey, '8.8.8.8', operator.privateKey);
    const response = await send(`/v3/admin/requests/${guest.request.id}/claim`, { method: 'POST', token: env.ADMIN_TOKEN, body: claim, origin: null });
    assert.equal(response.status, 200);
    const guestConfig = await deriveClaimConfig(claim.payload, claim.signature, guest.privateKey, 'guest', guest.request.token, service.origin);
    const helperConfig = await deriveClaimConfig(claim.payload, claim.signature, helper.privateKey, 'controller', response.body.token, service.origin);
    return { helper, claim, token: response.body.token, guestCipher: await createCipher(guestConfig), helperCipher: await createCipher(helperConfig) };
  };
  return { service, env, operator, storage, queue, send, register, claim, advance: value => { now += value; } };
}

test('unconfigured Worker fails closed and configured routing uses one named Durable Object', async () => {
  assert.equal((await worker.fetch(new Request('https://synthetic.example.workers.dev/v3/admin/queue'), {})).status, 503);
  const f = await setup(); let selected;
  const env = { ...f.env, SUPPORT_QUEUE: { idFromName(name) { selected = name; return 'object'; }, get(id) { assert.equal(id, 'object'); return f.queue; } } };
  const response = await worker.fetch(new Request(f.service.origin + '/v3/admin/queue', { headers: { Authorization: `Bearer ${f.env.ADMIN_TOKEN}` } }), env);
  assert.equal(response.status, 200); assert.equal(selected, 'support-queue-v3');
  assert.equal((await worker.fetch(new Request('https://wrong.example.workers.dev/v3/admin/queue'), env)).status, 503);
});

test('anonymous queue, protected atomic claims and role credentials remain separate', async () => {
  const f = await setup(), guest = await f.register(), path = `/v3/requests/${guest.request.id}`;
  assert.equal((await f.send('/v3/admin/queue')).status, 401);
  const waiting = await f.send('/v3/admin/queue', { token: f.env.ADMIN_TOKEN, origin: null });
  assert.deepEqual(Object.keys(waiting.body.requests[0]).sort(), ['expires', 'guestPublicKey', 'id', 'status', 'v']);
  assert.equal(waiting.body.requests[0].status, 'waiting');
  assert.equal((await f.send(path, { token: guest.request.token })).body.claim, null);
  assert.equal((await f.send(path, { token: f.env.ADMIN_TOKEN })).status, 401);
  assert.equal((await f.send('/v3/requests', { method: 'POST', body: guest.request })).status, 200);
  const claimed = await f.claim(guest);
  const visible = await f.send(path, { token: guest.request.token });
  assert.deepEqual(visible.body.claim, claimed.claim); assert.equal(JSON.stringify(visible.body).includes(claimed.token), false);
  assert.equal((await f.send(path, { token: claimed.token })).status, 403);
  const retry = await f.send(`/v3/admin/requests/${guest.request.id}/claim`, { method: 'POST', token: f.env.ADMIN_TOKEN, body: claimed.claim });
  assert.equal(retry.status, 200); assert.equal(retry.body.token, claimed.token);
  const otherHelper = await createEphemeralKeyPair();
  const changed = await createClaim(guest.request, otherHelper.publicKey, '8.8.8.8', f.operator.privateKey);
  assert.equal((await f.send(`/v3/admin/requests/${guest.request.id}/claim`, { method: 'POST', token: f.env.ADMIN_TOKEN, body: changed })).status, 409);
  assert.equal((await f.send(path + '/heartbeat', { method: 'POST', token: f.env.ADMIN_TOKEN, body: {} })).status, 401);
  assert.equal((await f.send(path, { method: 'DELETE', token: f.env.ADMIN_TOKEN })).status, 401);
});

test('actual handler carries encrypted directional setup with exact retry and rejects tampering/plaintext', async () => {
  const f = await setup(), guest = await f.register(), selected = await f.claim(guest), path = `/v3/requests/${guest.request.id}`;
  const envelope = await selected.guestCipher.seal('approved', null);
  const send = body => f.send(path + '/api/send', { method: 'POST', token: guest.request.token, body });
  assert.equal((await send(envelope)).status, 200); assert.equal((await send(envelope)).status, 200);
  assert.equal((await send(Object.fromEntries(Object.entries(envelope).reverse()))).status, 200);
  assert.equal((await send({ ...envelope, ct: encodeQueueBytes(new Uint8Array(32)) })).status, 409);
  const poll = await f.send(path + '/api/poll?after=0', { token: selected.token });
  assert.equal(poll.status, 200); assert.equal(poll.body.expiresAt, guest.request.expires);
  assert.deepEqual(await selected.helperCipher.open(poll.body.messages[0]), { kind: 'approved', data: null });
  assert.deepEqual((await f.send(path + '/poll?after=0', { token: guest.request.token })).body.messages, []);
  assert.equal((await f.send(path + '/send', { method: 'POST', token: f.env.ADMIN_TOKEN, body: envelope })).status, 401);
  for (const body of [{ type: 'frame', frame: {} }, { type: 'action', action: {} }, { ...envelope, from: 'controller' }, { ...envelope, sid: 'f'.repeat(32) }, { ...envelope, seq: 3 }, { ...envelope, message: 'not allowed' }, { ...envelope, iv: envelope.iv + '=' }]) {
    assert.ok((await send(body)).status >= 400);
  }
  for (const endpoint of ['/frames', '/actions', '/upload', '/shell', '/files']) assert.equal((await f.send(path + endpoint, { method: 'POST', token: guest.request.token, body: {} })).status, 404);
  assert.equal((await f.send(path + '/api/poll?after=0&extra=1', { token: selected.token })).status, 400);
});

test('cross-request claims/envelopes, races and Stop replay cannot revive a request', async () => {
  const f = await setup(), first = await f.register(), second = await f.register(), selected = await f.claim(first);
  const secondClaim = await f.send(`/v3/admin/requests/${second.request.id}/claim`, { method: 'POST', token: f.env.ADMIN_TOKEN, body: selected.claim });
  assert.equal(secondClaim.status, 400);
  const envelope = await selected.guestCipher.seal('approved', null);
  assert.equal((await f.send(`/v3/requests/${second.request.id}/send`, { method: 'POST', token: first.request.token, body: envelope })).status, 401);
  assert.equal((await f.send(`/v3/requests/${first.request.id}/send`, { method: 'POST', token: first.request.token, body: envelope })).status, 200);
  assert.equal((await f.send(`/v3/requests/${first.request.id}/stop`, { method: 'POST', token: selected.token, body: {} })).status, 200);
  assert.equal((await f.send(`/v3/requests/${first.request.id}`, { token: first.request.token })).status, 404);
  assert.equal((await f.send('/v3/requests', { method: 'POST', body: first.request })).status, 409);
  assert.equal((await f.storage.list({ prefix: `message:${first.request.id}:` })).size, 0);
  assert.equal((await f.send(`/v3/requests/${second.request.id}`, { method: 'DELETE', token: second.request.token })).status, 200);
  assert.equal((await f.storage.list({ prefix: 'request:' })).size, 0);
});

test('guest/controller leases and absolute expiry purge live metadata and encrypted queues', async () => {
  const f = await setup(), guest = await f.register(), selected = await f.claim(guest), path = `/v3/requests/${guest.request.id}`;
  f.advance(20000);
  assert.equal((await f.send(path + '/heartbeat', { method: 'POST', token: guest.request.token, body: {} })).status, 200);
  assert.equal((await f.send(path + '/heartbeat', { method: 'POST', token: selected.token, body: {} })).status, 200);
  f.advance(15000); await f.queue.alarm();
  assert.equal((await f.send(path, { token: guest.request.token })).status, 200);
  f.advance(15001); await f.queue.alarm();
  assert.equal((await f.send(path, { token: guest.request.token })).status, 404);
  assert.equal((await f.storage.list({ prefix: 'request:' })).size, 0);
  f.advance(120000); await f.queue.alarm();
  assert.equal((await f.storage.list({ prefix: 'replay:' })).size, 0);
  assert.equal((await f.storage.list({ prefix: 'rate:' })).size, 0);
});

test('CORS/metadata/body bounds and queue creation rate fail closed without recording raw clients', async () => {
  const f = await setup();
  const preflight = await f.send('/v3/requests', { method: 'OPTIONS', headers: { 'Access-Control-Request-Method': 'POST', 'Access-Control-Request-Headers': 'authorization, content-type' } });
  assert.equal(preflight.status, 204); assert.equal(preflight.headers.get('Access-Control-Allow-Origin'), 'tauri://localhost');
  for (const origin of ['null', 'https://untrusted.invalid', 'http://127.0.0.1:80', 'http://localhost:43210']) assert.equal((await f.send('/v3/requests', { method: 'OPTIONS', origin })).status, 403);
  const oversized = await f.send('/v3/requests', { method: 'POST', body: 'x'.repeat(QUEUE_LIMITS.bodyBytes + 1) });
  assert.equal(oversized.status, 413);
  for (let index = 0; index < 16; index++) await f.register({ headers: { 'CF-Connecting-IP': 'SYNTHETIC_CLIENT_SENTINEL' } });
  const extra = await createRequest({ expires: Date.now() + 60000 });
  assert.equal((await f.send('/v3/requests', { method: 'POST', body: extra.request, headers: { 'CF-Connecting-IP': 'SYNTHETIC_CLIENT_SENTINEL' } })).status, 429);
  assert.equal(JSON.stringify([...f.storage.data]).includes('SYNTHETIC_CLIENT_SENTINEL'), false);
  const configuration = JSON.parse(readFileSync(new URL('../cloudflare/support/wrangler.jsonc', import.meta.url), 'utf8'));
  assert.deepEqual(configuration.migrations, [{ tag: 'v1', new_sqlite_classes: ['SupportQueue'] }]);
  assert.equal(configuration.observability.enabled, false); assert.equal(configuration.observability.logs.invocation_logs, false);
  assert.match(configuration.vars.SERVICE_ORIGIN, /^https:\/\/[a-z0-9-]+\.[a-z0-9-]+\.workers\.dev$/);
  assert.match(configuration.vars.OPERATOR_PUBLIC_KEY, /^[A-Za-z0-9_-]{87}$/);
  assert.equal(configuration.vars.ADMIN_TOKEN, undefined);
});

test('simultaneous differently signed claims cannot both acquire one request', async () => {
  const f = await setup(), guest = await f.register(), helpers = await Promise.all([createEphemeralKeyPair(), createEphemeralKeyPair()]);
  const claims = await Promise.all(helpers.map(helper => createClaim(guest.request, helper.publicKey, '8.8.8.8', f.operator.privateKey)));
  const results = await Promise.all(claims.map(claim => f.send(`/v3/admin/requests/${guest.request.id}/claim`, { method: 'POST', token: f.env.ADMIN_TOKEN, body: claim })));
  assert.deepEqual(results.map(result => result.status).sort(), [200, 409]);
  const visible = await f.send(`/v3/requests/${guest.request.id}`, { token: guest.request.token });
  assert.deepEqual(visible.body.claim, results.find(result => result.status === 200).body.claim);
});

test('queue and per-role ciphertext/message limits remain bounded with individual small storage values', async () => {
  const f = await setup();
  for (let index = 0; index < QUEUE_LIMITS.requests; index++) await f.register({ headers: { 'CF-Connecting-IP': `synthetic-group-${Math.floor(index / 16)}` } });
  const extra = await createRequest({ expires: Date.now() + 60000 });
  assert.equal((await f.send('/v3/requests', { method: 'POST', body: extra.request, headers: { 'CF-Connecting-IP': 'synthetic-extra' } })).status, 429);
  assert.equal((await f.storage.list({ prefix: 'request:' })).size, QUEUE_LIMITS.requests);
  const g = await setup(), guest = await g.register(), selected = await g.claim(guest), path = `/v3/requests/${guest.request.id}`;
  const envelope = (role, seq, bytes) => ({ v: 1, sid: selected.claim.payload.sid, from: role, seq, expires: guest.request.expires,
    iv: encodeQueueBytes(new Uint8Array(12)), ct: encodeQueueBytes(new Uint8Array(bytes)) });
  for (let seq = 1; seq <= 12; seq++) assert.equal((await g.send(path + '/send', { method: 'POST', token: guest.request.token, body: envelope('guest', seq, 20 * 1024) })).status, 200);
  assert.equal((await g.send(path + '/send', { method: 'POST', token: guest.request.token, body: envelope('guest', 13, 20 * 1024) })).status, 413);
  assert.equal((await g.send(path + '/send', { method: 'POST', token: selected.token, body: envelope('controller', 1, 20 * 1024) })).status, 200);
  for (let seq = 2; seq <= 128; seq++) assert.equal((await g.send(path + '/send', { method: 'POST', token: selected.token, body: envelope('controller', seq, 16) })).status, 200);
  assert.equal((await g.send(path + '/send', { method: 'POST', token: selected.token, body: envelope('controller', 129, 16) })).status, 400);
  const poll = await g.send(path + '/poll?after=0', { token: guest.request.token });
  assert.equal(poll.body.messages.length, 128); assert.equal(poll.body.messages[127].seq, 128);
  assert.equal((await g.send(path, { method: 'DELETE', token: guest.request.token })).status, 200);
  assert.equal((await g.storage.list({ prefix: 'message:' })).size, 0);
});

test('each role must keep its own lease alive and absolute expiry overrides healthy leases', async () => {
  for (const liveRole of ['guest', 'controller']) {
    const f = await setup(), guest = await f.register(), selected = await f.claim(guest), path = `/v3/requests/${guest.request.id}`;
    f.advance(20000);
    const token = liveRole === 'guest' ? guest.request.token : selected.token;
    assert.equal((await f.send(path + '/heartbeat', { method: 'POST', token, body: {} })).status, 200);
    f.advance(10001); await f.queue.alarm();
    assert.equal((await f.send(path, { token: guest.request.token })).status, 404);
  }
  const f = await setup(), guest = await f.register(), selected = await f.claim(guest), path = `/v3/requests/${guest.request.id}`;
  for (let index = 0; index < 5; index++) {
    f.advance(20000);
    for (const token of [guest.request.token, selected.token]) assert.equal((await f.send(path + '/heartbeat', { method: 'POST', token, body: {} })).status, 200);
  }
  f.advance(20001); await f.queue.alarm();
  assert.equal((await f.send(path, { token: guest.request.token })).status, 404);
});

test('slow anonymous bodies cannot block an existing heartbeat or Stop', async () => {
  const f = await setup({ bodyTimeoutMs: 500 }), guest = await f.register(), path = `/v3/requests/${guest.request.id}`;
  const streams = [], pending = []; let settled = 0;
  try {
    for (let index = 0; index < 8; index++) {
      const body = new ReadableStream({ start(controller) { streams.push(controller); controller.enqueue(new TextEncoder().encode('{"v":3')); } });
      pending.push(f.queue.fetch(new Request(f.service.origin + '/v3/requests', { method: 'POST', headers: { Origin: 'tauri://localhost', 'Content-Type': 'application/json' }, body, duplex: 'half' })).then(response => { settled++; return response; }));
    }
    for (let attempt = 0; f.queue.receiving.registration < 8 && attempt < 100; attempt++) await new Promise(resolve => setTimeout(resolve, 1));
    assert.equal(f.queue.receiving.registration, 8);
    assert.equal((await f.send(path + '/heartbeat', { method: 'POST', token: guest.request.token, body: {} })).status, 200);
    assert.equal((await f.send(path + '/stop', { method: 'POST', token: guest.request.token, body: {} })).status, 200);
    assert.equal(settled, 0); assert.equal(f.queue.receiving.registration, 8);
  } finally {
    for (const stream of streams) try { stream.close(); } catch {}
    await Promise.all(pending);
  }
  assert.equal(f.queue.inFlight, 0); assert.equal(f.queue.pending, 0);
});

test('a slow authenticated setup body cannot consume another session or its own control slot', async () => {
  const f = await setup({ bodyTimeoutMs: 500 }), first = await f.register(), second = await f.register();
  const path = `/v3/requests/${first.request.id}`; let stream, settled = false;
  const body = new ReadableStream({ start(controller) { stream = controller; } });
  const pending = f.queue.fetch(new Request(f.service.origin + path + '/send', { method: 'POST', headers: { Origin: 'tauri://localhost', Authorization: `Bearer ${first.request.token}`, 'Content-Type': 'application/json' }, body, duplex: 'half' })).then(response => { settled = true; return response; });
  try {
    for (let attempt = 0; f.queue.receiving.setup < 1 && attempt < 100; attempt++) await new Promise(resolve => setTimeout(resolve, 1));
    assert.equal(f.queue.receiving.setup, 1);
    assert.equal((await f.send(path + '/send', { method: 'POST', token: first.request.token, body: {} })).status, 429);
    assert.equal((await f.send(path + '/heartbeat', { method: 'POST', token: first.request.token, body: {} })).status, 200);
    assert.equal((await f.send(`/v3/requests/${second.request.id}/heartbeat`, { method: 'POST', token: second.request.token, body: {} })).status, 200);
    assert.equal((await f.send(path + '/stop', { method: 'POST', token: first.request.token, body: {} })).status, 200);
    assert.equal(settled, false);
  } finally { try { stream.close(); } catch {} await pending; }
  assert.equal(f.queue.readerKeys.size, 0);
});

test('body deadlines remain enforced even if an untrusted stream cancel never resolves', async () => {
  const f = await setup({ bodyTimeoutMs: 20 });
  const body = new ReadableStream({ cancel() { return new Promise(() => {}); } });
  const response = await f.queue.fetch(new Request(f.service.origin + '/v3/requests', { method: 'POST', headers: { Origin: 'tauri://localhost', 'Content-Type': 'application/json' }, body, duplex: 'half' }));
  assert.equal(response.status, 408); assert.equal(f.queue.inFlight, 0); assert.equal(f.queue.receiving.registration, 0);
});

test('a held heartbeat body cannot block authenticated Stop for that same role', async () => {
  const f = await setup({ bodyTimeoutMs: 500 }), guest = await f.register(), path = `/v3/requests/${guest.request.id}`;
  let stream;
  const body = new ReadableStream({ start(controller) { stream = controller; } });
  const pending = f.queue.fetch(new Request(f.service.origin + path + '/heartbeat', { method: 'POST', headers: { Origin: 'tauri://localhost', Authorization: `Bearer ${guest.request.token}`, 'Content-Type': 'application/json' }, body, duplex: 'half' }));
  try {
    for (let attempt = 0; f.queue.receiving.control < 1 && attempt < 100; attempt++) await new Promise(resolve => setTimeout(resolve, 1));
    assert.equal(f.queue.receiving.control, 1);
    assert.equal((await f.send(path + '/stop', { method: 'POST', token: guest.request.token, body: {} })).status, 200);
    assert.equal(f.queue.receiving.control, 1);
  } finally { try { stream.close(); } catch {} await pending; }
  assert.equal(f.queue.readerKeys.size, 0);
});
