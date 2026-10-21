import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { SupportQueue } from '../cloudflare/support/worker.mjs';
import { createRequest, createEphemeralKeyPair, createClaim, encodeQueueBytes } from '../public/support/queue-protocol.js';

const read = path => readFileSync(new URL('../' + path, import.meta.url), 'utf8');
const CLIENT_NOW = 1_900_000_000_000;

class Storage {
  constructor() { this.rows = new Map(); this.alarm = null; }
  async get(key) { return structuredClone(this.rows.get(key)); }
  async put(key, value) { this.rows.set(key, structuredClone(value)); }
  async delete(keys) {
    let count = 0;
    for (const key of Array.isArray(keys) ? keys : [keys]) count += this.rows.delete(key) ? 1 : 0;
    return count;
  }
  async list({ prefix = '', limit = 10000 } = {}) {
    return new Map([...this.rows].filter(([key]) => key.startsWith(prefix)).sort(([a], [b]) => a.localeCompare(b))
      .slice(0, limit).map(([key, value]) => [key, structuredClone(value)]));
  }
  async transaction(operation) {
    const before = structuredClone(this.rows);
    try { return await operation(this); } catch (error) { this.rows = before; throw error; }
  }
  async getAlarm() { return this.alarm; }
  async setAlarm(value) { this.alarm = value; }
  async deleteAlarm() { this.alarm = null; }
}

async function fixture(serverNow) {
  const operator = await crypto.subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, true, ['sign', 'verify']);
  const env = {
    SERVICE_ORIGIN: 'https://synthetic.synthetic.workers.dev', OPERATOR_LABEL: 'Synthetic support', ADMIN_TOKEN: 'a'.repeat(64),
    OPERATOR_PUBLIC_KEY: encodeQueueBytes(new Uint8Array(await crypto.subtle.exportKey('raw', operator.publicKey))),
  };
  const storage = new Storage();
  const worker = new SupportQueue({ storage }, env, { now: () => serverNow });
  async function send(path, body, token) {
    const response = await worker.fetch(new Request(env.SERVICE_ORIGIN + path, {
      method: 'POST', headers: { Origin: 'tauri://localhost', 'Content-Type': 'application/json',
        ...(token ? { Authorization: 'Bearer ' + token } : {}) }, body: JSON.stringify(body),
    }));
    return { status: response.status, body: await response.json() };
  }
  return { storage, operator, env, send };
}

test('actual Worker accepts the request margin with a client twenty seconds ahead, not the old exact maximum', async t => {
  t.mock.method(Date, 'now', () => CLIENT_NOW);
  const f = await fixture(CLIENT_NOW - 20_000);
  const old = await createRequest({ expires: CLIENT_NOW + 30 * 60 * 1000 });
  assert.equal((await f.send('/v3/requests', old.request)).status, 400);
  assert.equal(await f.storage.get('request:' + old.request.id), undefined);
  const safe = await createRequest({ expires: CLIENT_NOW + 29 * 60 * 1000 + 30_000 });
  const accepted = await f.send('/v3/requests', safe.request);
  assert.equal(accepted.status, 200);
  assert.equal(accepted.body.expires, safe.request.expires);
  assert.equal((await f.storage.get('request:' + safe.request.id)).expires, safe.request.expires);
  assert.equal((await f.send(`/v3/requests/${safe.request.id}/stop`, {}, safe.request.token)).status, 200);
});

test('actual Worker accepts the handoff margin with a client two seconds ahead, not the old exact maximum', async t => {
  t.mock.method(Date, 'now', () => CLIENT_NOW);
  const f = await fixture(CLIENT_NOW - 2_000);
  const guest = await createRequest({ expires: CLIENT_NOW + 120_000 });
  assert.equal((await f.send('/v3/requests', guest.request)).status, 200);
  const helper = await createEphemeralKeyPair();
  const claim = await createClaim(guest.request, helper.publicKey, '8.8.8.8', f.operator.privateKey);
  assert.equal((await f.send(`/v3/admin/requests/${guest.request.id}/claim`, claim, f.env.ADMIN_TOKEN)).status, 200);
  const path = `/v3/requests/${guest.request.id}/handoff`;
  const handoff = { v: 1, parentId: guest.request.id, parentSid: claim.payload.sid, nonce: 'b'.repeat(32),
    targetVersion: '0.2.0-beta.3.support.2', artifactSha256: 'c'.repeat(64), nativeSessionId: 'd'.repeat(32),
    originalExpires: guest.request.expires, resumeDeadline: CLIENT_NOW + 90_000 };
  assert.equal((await f.send(path, handoff, guest.request.token)).status, 400);
  assert.equal((await f.storage.get('request:' + guest.request.id)).handoff, undefined);
  const safe = { ...handoff, resumeDeadline: CLIENT_NOW + 85_000 };
  const accepted = await f.send(path, safe, guest.request.token);
  assert.equal(accepted.status, 200);
  assert.equal(accepted.body.handoff.resumeDeadline, safe.resumeDeadline);
  assert.equal(accepted.body.handoff.originalExpires, guest.request.expires);
  assert.equal((await f.send(path + '/stop', { nonce: safe.nonce }, accepted.body.resumeTicket)).status, 200);
});

function assertAdvertisements(support, updater) {
  assert.match(support, /const SESSION_LIMIT_MS: u64 = 30 \* 60 \* 1000;/);
  assert.match(support, /const REQUEST_CLOCK_MARGIN_MS: u64 = 30_000;/);
  assert.match(support, /fn request_deadline_ms\(now: u64\) -> u64 \{[^}]*now\.saturating_add\(SESSION_LIMIT_MS - REQUEST_CLOCK_MARGIN_MS\)[^}]*\}/s);
  assert.match(support, /expires: request_deadline_ms\(now\)/);
  assert.match(updater, /const RESUME_MS: u64 = 90_000;/);
  assert.match(updater, /const RESUME_CLOCK_MARGIN_MS: u64 = 5_000;/);
  assert.match(updater, /fn update_resume_deadline_ms\(now: u64, original_expires: u64\) -> u64 \{[^}]*now\.saturating_add\(RESUME_MS - RESUME_CLOCK_MARGIN_MS\)\s*\.min\(original_expires\)[^}]*\}/s);
  assert.match(updater, /let deadline = update_resume_deadline_ms\(now, parent\.expires\);/);
  assert.match(updater, /handoff\.resume_deadline <= now\.saturating_add\(RESUME_MS\)/);
}

test('native advertisements use the tested margins without widening strict validators or original expiry', () => {
  const support = read('src-tauri/src/support_session.rs'), updater = read('src-tauri/src/updater.rs');
  assertAdvertisements(support, updater);
  assert.throws(() => assertAdvertisements(support.replace('REQUEST_CLOCK_MARGIN_MS: u64 = 30_000', 'REQUEST_CLOCK_MARGIN_MS: u64 = 0'), updater));
  assert.throws(() => assertAdvertisements(support.replace('expires: request_deadline_ms(now)', 'expires: now.saturating_add(SESSION_LIMIT_MS)'), updater));
  assert.throws(() => assertAdvertisements(support, updater.replace('RESUME_CLOCK_MARGIN_MS: u64 = 5_000', 'RESUME_CLOCK_MARGIN_MS: u64 = 0')));
  assert.throws(() => assertAdvertisements(support, updater.replace('let deadline = update_resume_deadline_ms(now, parent.expires);', 'let deadline = now.saturating_add(RESUME_MS).min(parent.expires);')));
});
