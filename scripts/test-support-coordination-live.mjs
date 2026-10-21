#!/usr/bin/env node
// Authorized synthetic service check only; never run against a nonempty queue.
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import { loadOperatorIdentity } from './support-dashboard.mjs';
import { createRequest, createEphemeralKeyPair, createClaim, acceptClaim, deriveClaimConfig } from '../public/support/queue-protocol.js';
import { createCipher, publicIPv4 } from '../public/support/protocol.js';

const [mode, configPath] = process.argv.slice(2);
if (!['--synthetic', '--synthetic-dashboard'].includes(mode) || !configPath?.startsWith('/')) throw Error('Explicit synthetic mode and an absolute private operator configuration are required.');
const service = JSON.parse(await fs.readFile(new URL('../public/support/service.json', import.meta.url), 'utf8'));
const identity = await loadOperatorIdentity(configPath, service);
const created = [];
const call = async (route, { method = 'GET', token, body, origin = 'tauri://localhost', headers = {} } = {}) => {
  const response = await fetch(service.origin + route, { method, credentials: 'omit', redirect: 'error', cache: 'no-store',
    headers: { Origin: origin, ...(token ? { Authorization: 'Bearer ' + token } : {}),
      ...(body === undefined ? {} : { 'Content-Type': 'application/json' }), ...headers },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }), signal: AbortSignal.timeout(8000) });
  const text = await response.text(); if (text.length > 64 * 1024) throw Error('Unexpected service response.');
  return { status: response.status, headers: response.headers, body: response.status === 204 ? null : JSON.parse(text) };
};
const queue = async () => {
  const response = await call('/v3/admin/queue', { token: identity.adminToken });
  assert.equal(response.status, 200); return response.body.requests;
};
let heartbeat, waiting, stopping = false;
const stop = async () => {
  if (stopping) return; stopping = true; clearInterval(heartbeat); clearInterval(waiting);
  for (const row of created) await call(`/v3/requests/${row.request.id}/stop`, { method: 'POST', token: row.request.token, body: {} }).catch(() => {});
};
try {
  assert.equal((await queue()).length, 0, 'Live check requires an empty queue.');
  assert.equal((await call('/v3/admin/queue')).status, 401);
  for (let index = 0; index < 3; index++) {
    const guest = await createRequest({ expires: Date.now() + 120000 }); created.push(guest);
    assert.equal((await call('/v3/requests', { method: 'POST', body: guest.request })).status, 200);
  }
  assert.equal((await queue()).length, 3);
  if (mode === '--synthetic-dashboard') {
    heartbeat = setInterval(() => { for (const guest of created) void call(`/v3/requests/${guest.request.id}/heartbeat`, { method: 'POST', token: guest.request.token, body: {} }).catch(() => {}); }, 10000);
    process.stdout.write(JSON.stringify({ event: 'synthetic_dashboard_requests_ready', requests: 3, mailboxData: false }) + '\n');
    await new Promise(resolve => {
      const timeout = setTimeout(resolve, 105000);
      for (const signal of ['SIGINT', 'SIGTERM']) process.once(signal, () => { clearTimeout(timeout); resolve(); });
      let sent = false;
      waiting = setInterval(() => {
        if (sent) return;
        void queue().then(async rows => {
          const claimed = rows.find(row => row.status === 'claimed');
          const owner = claimed && created.find(row => row.request.id === claimed.id);
          if (sent || !owner) return;
          sent = true;
          await call(`/v3/requests/${owner.request.id}/stop`, { method: 'POST', token: owner.request.token, body: {} });
          process.stdout.write(JSON.stringify({ event: 'synthetic_customer_stop', remainingRequests: (await queue()).length }) + '\n');
        }).catch(() => {});
      }, 500);
    });
  } else {
    const guest = created[0], helper = await createEphemeralKeyPair();
    const routeResponse = await fetch('https://am.i.mullvad.net/json', { redirect: 'error', signal: AbortSignal.timeout(8000) });
    const route = await routeResponse.json();
    assert.equal(route.mullvad_exit_ip, true); assert.equal(publicIPv4(route.ip), true);
    const claim = await createClaim(guest.request, helper.publicKey, route.ip, identity.operatorSigningKey);
    const base = `/v3/requests/${guest.request.id}`;
    const claimed = await call(`/v3/admin/requests/${guest.request.id}/claim`, { method: 'POST', token: identity.adminToken, body: claim });
    assert.equal(claimed.status, 200);
    const received = await call(base, { token: guest.request.token });
    await acceptClaim(guest.request, received.body.claim, service);
    assert.equal(Object.hasOwn(received.body, 'token'), false);
    const guestConfig = await deriveClaimConfig(claim.payload, claim.signature, guest.privateKey, 'guest', guest.request.token, service.origin);
    const helperConfig = await deriveClaimConfig(claim.payload, claim.signature, helper.privateKey, 'controller', claimed.body.token, service.origin);
    assert.equal(guestConfig.key, helperConfig.key);
    const guestCipher = await createCipher(guestConfig), helperCipher = await createCipher(helperConfig);
    const envelope = await guestCipher.seal('approved', null);
    assert.equal((await call(base + '/api/send', { method: 'POST', token: guest.request.token, body: envelope })).status, 200);
    const poll = await call(base + '/api/poll?after=0', { token: claimed.body.token });
    assert.deepEqual(await helperCipher.open(poll.body.messages[0]), { kind: 'approved', data: null });
    assert.equal((await call(base + '/api/send', { method: 'POST', token: identity.adminToken, body: envelope })).status, 401);
    assert.equal((await call(base + '/api/send', { method: 'POST', token: guest.request.token, body: { type: 'frame' } })).status, 400);
    const preflight = await call(base + '/api/send', { method: 'OPTIONS', headers: { 'Access-Control-Request-Method': 'POST', 'Access-Control-Request-Headers': 'authorization,content-type' } });
    assert.equal(preflight.status, 204); assert.equal(preflight.headers.get('access-control-allow-origin'), 'tauri://localhost');
    assert.equal((await call(base, { token: guest.request.token, origin: 'https://attacker.invalid' })).status, 403);
    assert.equal((await call(base + '/stop', { method: 'POST', token: guest.request.token, body: {} })).status, 200);
    assert.equal((await queue()).length, 2);
    assert.equal((await call(base, { token: guest.request.token })).status, 404);
    assert.equal((await call('/v3/requests', { method: 'POST', body: guest.request })).status, 409);
    process.stdout.write(JSON.stringify({ result: 'PASS', profile: 'live-synthetic-coordination', syntheticRequests: 3, appConnectionTested: false, mailboxData: false }) + '\n');
  }
} finally {
  await stop();
  process.stdout.write(JSON.stringify({ event: 'synthetic_cleanup', remainingRequests: (await queue()).length }) + '\n');
}
