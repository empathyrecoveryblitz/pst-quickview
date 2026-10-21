#!/usr/bin/env node
// Run the synthetic coordination gate in Cloudflare's actual local runtime.
// Miniflare is an isolated deployment tool, not an application dependency.
import assert from 'node:assert/strict';
import path from 'node:path';
import { pathToFileURL } from 'node:url';
import { createRequest, createEphemeralKeyPair, createClaim, acceptClaim,
  deriveClaimConfig, encodeQueueBytes } from '../public/support/queue-protocol.js';
import { createCipher } from '../public/support/protocol.js';

const [modulePath, bundlePath] = process.argv.slice(2);
if (!modulePath || !bundlePath || !path.isAbsolute(modulePath) || !path.isAbsolute(bundlePath)) {
  throw Error('Pass absolute paths for the installed Miniflare module and dry-run Worker bundle.');
}
const { Miniflare, convertV4MiniflareOptions } = await import(pathToFileURL(modulePath));
const operator = await crypto.subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, true, ['sign', 'verify']);
const service = { v: 3, origin: 'https://synthetic.example.workers.dev',
  operatorPublicKey: encodeQueueBytes(new Uint8Array(await crypto.subtle.exportKey('raw', operator.publicKey))),
  operatorLabel: 'Synthetic support' };
const admin = 'a'.repeat(64);
const runtime = new Miniflare(convertV4MiniflareOptions({ rootPath: path.dirname(bundlePath), modules: true, scriptPath: bundlePath, compatibilityDate: '2026-10-20',
  durableObjects: { SUPPORT_QUEUE: { className: 'SupportQueue', useSQLite: true } },
  bindings: { SERVICE_ORIGIN: service.origin, OPERATOR_PUBLIC_KEY: service.operatorPublicKey,
    OPERATOR_LABEL: service.operatorLabel, ADMIN_TOKEN: admin } }));
let assertions = 0;
const check = (actual, expected) => { assert.deepEqual(actual, expected); assertions++; };
const send = async (route, { method = 'GET', token, body, origin = 'tauri://localhost', headers = {} } = {}) => {
  const response = await runtime.dispatchFetch(service.origin + route, { method,
    headers: { Origin: origin, ...(token ? { Authorization: 'Bearer ' + token } : {}),
      ...(body === undefined ? {} : { 'Content-Type': 'application/json' }), ...headers },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }) });
  return { status: response.status, headers: response.headers,
    body: response.status === 204 ? null : await response.json() };
};
try {
  check((await send('/v3/admin/queue')).status, 401);
  const requests = await Promise.all(Array.from({ length: 3 }, () => createRequest({ expires: Date.now() + 120000 })));
  for (const row of requests) check((await send('/v3/requests', { method: 'POST', body: row.request })).status, 200);
  check((await send('/v3/admin/queue', { token: admin })).body.requests.length, 3);
  const guest = requests[0], helper = await createEphemeralKeyPair();
  const claim = await createClaim(guest.request, helper.publicKey, '8.8.8.8', operator.privateKey);
  const claimed = await send(`/v3/admin/requests/${guest.request.id}/claim`, { method: 'POST', token: admin, body: claim });
  check(claimed.status, 200);
  const guestView = await send(`/v3/requests/${guest.request.id}`, { token: guest.request.token });
  await acceptClaim(guest.request, guestView.body.claim, service); assertions++;
  check(Object.hasOwn(guestView.body, 'token'), false);
  const guestConfig = await deriveClaimConfig(claim.payload, claim.signature, guest.privateKey, 'guest', guest.request.token, service.origin);
  const helperConfig = await deriveClaimConfig(claim.payload, claim.signature, helper.privateKey, 'controller', claimed.body.token, service.origin);
  check(guestConfig.key, helperConfig.key);
  const guestCipher = await createCipher(guestConfig), helperCipher = await createCipher(helperConfig);
  const envelope = await guestCipher.seal('approved', null);
  const base = `/v3/requests/${guest.request.id}`;
  check((await send(base + '/api/send', { method: 'POST', token: guest.request.token, body: envelope })).status, 200);
  const polled = await send(base + '/api/poll?after=0', { token: claimed.body.token });
  check(await helperCipher.open(polled.body.messages[0]), { kind: 'approved', data: null });
  check((await send(base + '/api/send', { method: 'POST', token: admin, body: envelope })).status, 401);
  check((await send(base + '/api/send', { method: 'POST', token: guest.request.token, body: { type: 'frame', text: 'Fictional' } })).status, 400);
  const preflight = await send(base + '/api/send', { method: 'OPTIONS', headers: {
    'Access-Control-Request-Method': 'POST', 'Access-Control-Request-Headers': 'authorization,content-type' } });
  check(preflight.status, 204); check(preflight.headers.get('Access-Control-Allow-Origin'), 'tauri://localhost');
  check((await send(base, { token: guest.request.token, origin: 'https://attacker.invalid' })).status, 403);
  check((await send(base + '/stop', { method: 'POST', token: guest.request.token, body: {} })).status, 200);
  check((await send('/v3/admin/queue', { token: admin })).body.requests.length, 2);
  check((await send(base, { token: guest.request.token })).status, 404);
  check((await send('/v3/requests', { method: 'POST', body: guest.request })).status, 409);
  for (const row of requests.slice(1)) check((await send(`/v3/requests/${row.request.id}/stop`, { method: 'POST', token: row.request.token, body: {} })).status, 200);
  check((await send('/v3/admin/queue', { token: admin })).body.requests.length, 0);
  process.stdout.write(JSON.stringify({ result: 'PASS', assertions, runtime: 'workerd', syntheticRequests: 3, liveRequestsAfterCleanup: 0 }) + '\n');
} finally { await runtime.dispose(); }
