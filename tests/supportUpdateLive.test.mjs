import test from 'node:test';
import assert from 'node:assert/strict';
import { SupportQueue } from '../cloudflare/support/worker.mjs';
import { createRequest, encodeQueueBytes } from '../public/support/queue-protocol.js';
import { runSyntheticUpdateGate } from '../scripts/test-support-update-live.mjs';

class Storage {
  constructor() { this.data = new Map(); this.alarm = null; }
  async get(key) { return structuredClone(this.data.get(key)); }
  async put(key, value) { this.data.set(key, structuredClone(value)); }
  async delete(keys) { for (const key of Array.isArray(keys) ? keys : [keys]) this.data.delete(key); }
  async list({ prefix = '', limit = 10000 } = {}) { return new Map([...this.data].filter(([key]) => key.startsWith(prefix)).sort(([a], [b]) => a.localeCompare(b)).slice(0, limit).map(([key, value]) => [key, structuredClone(value)])); }
  async transaction(operation) {
    const saved = structuredClone(this.data);
    try { return await operation(this); } catch (error) { this.data = saved; throw error; }
  }
  async getAlarm() { return this.alarm; }
  async setAlarm(value) { this.alarm = value; }
  async deleteAlarm() { this.alarm = null; }
}
async function fixture() {
  const operator = await crypto.subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, true, ['sign', 'verify']);
  const service = { v: 3, origin: 'https://synthetic.synthetic.workers.dev', operatorLabel: 'Fictional helper',
    operatorPublicKey: encodeQueueBytes(new Uint8Array(await crypto.subtle.exportKey('raw', operator.publicKey))) };
  const identity = { adminToken: 'a'.repeat(64), operatorSigningKey: operator.privateKey };
  const storage = new Storage();
  const queue = new SupportQueue({ storage }, { SERVICE_ORIGIN: service.origin, OPERATOR_PUBLIC_KEY: service.operatorPublicKey,
    OPERATOR_LABEL: service.operatorLabel, ADMIN_TOKEN: identity.adminToken });
  const calls = [];
  const fetchImpl = async (url, init) => { calls.push({ route: new URL(url).pathname, method: init.method, body: init.body }); return queue.fetch(new Request(url, init)); };
  return { service, identity, storage, queue, calls, fetchImpl, expectedExit: '8.8.8.8' };
}

test('live update gate exercises real handler crypto with exactly three synthetic requests and zero leftovers', async () => {
  const f = await fixture(), result = await runSyntheticUpdateGate(f);
  assert.equal(result.result, 'PASS'); assert.equal(result.syntheticRequests, 3); assert.ok(result.assertions >= 35);
  assert.equal(result.liveRequestsAfterCleanup, 0); assert.equal(result.appConnectionTested, false); assert.equal(result.updateInstallTested, false);
  assert.equal(result.mailboxData, false);
  assert.equal((await f.storage.list({ prefix: 'request:' })).size, 0);
  assert.equal((await f.storage.list({ prefix: 'message:' })).size, 0);
  assert.equal(f.calls.filter(call => call.route === '/v3/requests' && call.method === 'POST').length, 4); // Three creates plus one rejected replay.
  assert.equal(f.calls.some(call => /"resumeAuth"|"type":"frame"|"type":"action"/.test(call.body || '')), false);
  assert.equal(f.calls.filter(call => call.route.endsWith('/resume')).length, 2); // One success and one rejected reuse.
});

test('live update gate refuses a nonempty queue without mutating any existing request', async () => {
  const f = await fixture(), existing = await createRequest({ expires: Date.now() + 120000 });
  await f.queue.fetch(new Request(f.service.origin + '/v3/requests', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(existing.request) }));
  await assert.rejects(runSyntheticUpdateGate(f), error => error.message === 'empty-queue-required');
  assert.equal(f.calls.length, 1); assert.equal(f.calls[0].method, 'GET');
  assert.equal((await f.storage.list({ prefix: 'request:' })).size, 1);
  assert.equal((await f.storage.get('request:' + existing.request.id)).id, existing.request.id);
});

test('failed resume still removes only this invocation synthetic requests and pending tickets', async () => {
  const f = await fixture();
  await assert.rejects(runSyntheticUpdateGate({ ...f, fetchImpl: (url, init) => new URL(url).pathname.endsWith('/resume')
    ? Promise.resolve(new Response('{"error":"unavailable"}', { status: 503 })) : f.fetchImpl(url, init) }), error => error.message === 'fresh-guest-resume');
  assert.equal((await f.storage.list({ prefix: 'request:' })).size, 0);
  assert.equal((await f.storage.list({ prefix: 'message:' })).size, 0);
});

test('untrusted oversized or failing service responses produce only categorical errors', async () => {
  const f = await fixture();
  for (const fetchImpl of [async () => new Response('SENSITIVE_RESPONSE'.repeat(5000)), async () => { throw Error('SENSITIVE_NETWORK_DETAIL'); }]) {
    await assert.rejects(runSyntheticUpdateGate({ ...f, fetchImpl }), error => error.message === 'empty-queue-required' && !error.message.includes('SENSITIVE'));
  }
  assert.equal((await f.storage.list({ prefix: 'request:' })).size, 0);
});
