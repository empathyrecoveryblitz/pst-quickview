#!/usr/bin/env node
// Explicitly authorized synthetic coordination only. Never install an update,
// start WebRTC, or read application/mail data. Never touch an existing request.
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { randomBytes } from 'node:crypto';
import { loadOperatorIdentity } from './support-dashboard.mjs';
import { createRequest, createEphemeralKeyPair, createClaim, acceptClaim, deriveClaimConfig,
  validateServiceConfig, validateUpdateHandoff, createResumedRequest, queueTokenSha256,
  ackProof, resumedGuestProof, resumedClaimProof, verifyUpdateProof } from '../public/support/queue-protocol.js';
import { createCipher, publicIPv4 } from '../public/support/protocol.js';

class GateError extends Error {
  constructor(stage) { super(stage); this.stage = stage; }
}
async function boundedJSON(response) {
  if (!response.body?.getReader) throw new GateError('service-response');
  const reader = response.body.getReader(), chunks = [];
  let length = 0;
  try {
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      length += value.byteLength;
      if (length > 64 * 1024) throw new GateError('service-response');
      chunks.push(value);
    }
    return JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(Buffer.concat(chunks)));
  } catch { throw new GateError('service-response'); }
  finally { try { void reader.cancel().catch(() => {}); } catch {} reader.releaseLock(); }
}

export async function runSyntheticUpdateGate({ service: rawService, identity, expectedExit, fetchImpl = fetch, now = Date.now } = {}) {
  const service = validateServiceConfig(rawService);
  if (!identity?.operatorSigningKey || !/^[a-f0-9]{64}$/.test(identity.adminToken || '') || !publicIPv4(expectedExit)) throw new GateError('configuration');
  let assertions = 0, stage = 'empty-queue-required', started = false, heartbeatTimer;
  const created = [], heartbeatCalls = new Set();
  const check = condition => { if (!condition) throw new GateError(stage); assertions++; };
  const call = async (route, { method = 'GET', token, body } = {}) => {
    try {
      const response = await fetchImpl(service.origin + route, { method, credentials: 'omit', redirect: 'error', cache: 'no-store',
        headers: { Origin: 'tauri://localhost', ...(token ? { Authorization: 'Bearer ' + token } : {}),
          ...(body === undefined ? {} : { 'Content-Type': 'application/json' }) },
        ...(body === undefined ? {} : { body: JSON.stringify(body) }), signal: AbortSignal.timeout(8000) });
      return { status: response.status, body: response.status === 204 ? null : await boundedJSON(response) };
    } catch { throw new GateError(stage); }
  };
  const queue = async () => {
    const response = await call('/v3/admin/queue', { token: identity.adminToken });
    check(response.status === 200 && Array.isArray(response.body?.requests) && response.body.requests.length <= 64);
    return response.body.requests;
  };
  const claim = async (request, helper, extra = {}) => {
    const signed = await createClaim(request, helper.publicKey, expectedExit, identity.operatorSigningKey);
    const body = { ...signed, ...extra };
    const response = await call(`/v3/admin/requests/${request.id}/claim`, { method: 'POST', token: identity.adminToken, body });
    return { response, signed };
  };
  const arm = async row => {
    const base = { v: 1, parentId: row.original.request.id, parentSid: row.claim.payload.sid,
      nonce: randomBytes(16).toString('hex'), targetVersion: '0.2.0-beta.3.support.2', artifactSha256: randomBytes(32).toString('hex'),
      nativeSessionId: randomBytes(16).toString('hex'), originalExpires: row.original.request.expires,
      resumeDeadline: Math.min(now() + 90000, row.original.request.expires) };
    // Save the nonce before transmission so even a lost response can be stopped.
    row.handoff = base;
    const response = await call(row.base + '/handoff', { method: 'POST', token: row.original.request.token, body: base });
    check(response.status === 200 && /^[a-f0-9]{64}$/.test(response.body?.resumeTicket || ''));
    row.resumeTicket = response.body.resumeTicket;
    row.handoff = validateUpdateHandoff(response.body.handoff);
    check(row.handoff.resumeTicketSha256 === await queueTokenSha256(row.resumeTicket));
    check(Object.keys(base).every(key => row.handoff[key] === base[key]));
  };
  const cleanup = async row => {
    // Every identifier/token here was created by this invocation. No admin
    // deletion or queue-wide cleanup endpoint is used.
    if (row.handoff) {
      const tokens = [row.resumeTicket, row.original.request.token, row.controllerToken].filter(Boolean);
      for (const token of tokens) {
        const result = await call(row.base + '/handoff/stop', { method: 'POST', token, body: { nonce: row.handoff.nonce } }).catch(() => null);
        if (result?.status === 200 || result?.status === 404) return;
      }
    }
    for (const token of [row.current.request.token, row.original.request.token, row.controllerToken].filter(Boolean)) {
      const result = await call(row.base + '/stop', { method: 'POST', token, body: {} }).catch(() => null);
      if (result?.status === 200 || result?.status === 404) return;
    }
  };
  const heartbeat = () => {
    for (const row of created) {
      const entries = [[row.base + '/heartbeat', row.current.request.token, {}]];
      if (row.controllerToken) entries.push([row.base + '/heartbeat', row.controllerToken, {}]);
      if (row.handoff && row.controllerToken) entries.push([row.base + '/handoff/heartbeat', row.controllerToken, { nonce: row.handoff.nonce }]);
      for (const [route, token, body] of entries) {
        const pending = call(route, { method: 'POST', token, body }).catch(() => {}).finally(() => heartbeatCalls.delete(pending));
        heartbeatCalls.add(pending);
      }
    }
  };
  try {
    check((await queue()).length === 0);
    started = true; stage = 'register';
    for (let index = 0; index < 3; index++) {
      const original = await createRequest({ expires: now() + 120000 });
      const row = { original, current: original, base: `/v3/requests/${original.request.id}` };
      created.push(row);
      check((await call('/v3/requests', { method: 'POST', body: original.request })).status === 200);
    }
    check((await queue()).length === 3);
    heartbeatTimer = setInterval(heartbeat, 10000);
    const row = created[0], helper = await createEphemeralKeyPair(); stage = 'original-claim';
    const selected = await claim(row.original.request, helper);
    check(selected.response.status === 200 && /^[a-f0-9]{64}$/.test(selected.response.body?.token || ''));
    row.claim = selected.signed; row.controllerToken = selected.response.body.token;
    const originalGuestConfig = await deriveClaimConfig(row.claim.payload, row.claim.signature,
      row.original.privateKey, 'guest', row.original.request.token, service.origin);
    const originalCipher = await createCipher(originalGuestConfig);
    check((await call(row.base + '/api/send', { method: 'POST', token: row.original.request.token, body: await originalCipher.seal('approved', null) })).status === 200);
    stage = 'handoff'; await arm(row);
    for (const token of [row.original.request.token, row.controllerToken]) {
      check((await call(row.base + '/api/poll?after=0', { token })).status === 409);
    }
    const updating = (await queue()).find(value => value.id === row.original.request.id);
    check(updating?.status === 'updating');
    const secret = randomBytes(32).toString('hex');
    check(verifyUpdateProof(await ackProof(secret, row.handoff), await ackProof(secret, row.handoff)));
    stage = 'fresh-guest-resume';
    row.current = await createResumedRequest(row.handoff);
    check(row.current.request.guestPublicKey !== row.original.request.guestPublicKey && row.current.request.token !== row.original.request.token);
    const guestTokenSha256 = await queueTokenSha256(row.current.request.token);
    const proof = await resumedGuestProof(secret, row.handoff, row.current.request.guestPublicKey, guestTokenSha256);
    const body = { v: 1, nonce: row.handoff.nonce, targetVersion: row.handoff.targetVersion, artifactSha256: row.handoff.artifactSha256,
      guestPublicKey: row.current.request.guestPublicKey, token: row.current.request.token, proof };
    check((await call(row.base + '/resume', { method: 'POST', token: row.resumeTicket, body })).status === 200);
    check((await call(row.base + '/resume', { method: 'POST', token: row.resumeTicket, body })).status === 409);
    for (const token of [row.original.request.token, row.controllerToken]) check((await call(row.base + '/stop', { method: 'POST', token, body: {} })).status === 401);
    const resuming = (await queue()).find(value => value.id === row.original.request.id);
    check(resuming?.status === 'resuming' && resuming.guestTokenSha256 === guestTokenSha256 && verifyUpdateProof(proof, resuming.resumeGuestProof));
    stage = 'fresh-helper-claim';
    const freshHelper = await createEphemeralKeyPair();
    const freshClaim = await createClaim(row.current.request, freshHelper.publicKey, expectedExit, identity.operatorSigningKey);
    check(freshClaim.payload.sid !== row.claim.payload.sid && freshHelper.publicKey !== helper.publicKey);
    check((await call(`/v3/admin/requests/${row.current.request.id}/claim`, { method: 'POST', token: identity.adminToken, body: freshClaim })).status === 400);
    freshClaim.resumeProof = await resumedClaimProof(secret, row.handoff, freshClaim);
    const continued = await call(`/v3/admin/requests/${row.current.request.id}/claim`, { method: 'POST', token: identity.adminToken, body: freshClaim });
    check(continued.status === 200 && continued.body.token !== row.controllerToken);
    row.controllerToken = continued.body.token;
    const received = await call(row.base, { token: row.current.request.token });
    const accepted = await acceptClaim(row.current.request, received.body.claim, service);
    check(verifyUpdateProof(await resumedClaimProof(secret, row.handoff, accepted), accepted.resumeProof));
    const guestConfig = await deriveClaimConfig(accepted.payload, accepted.signature, row.current.privateKey, 'guest', row.current.request.token, service.origin);
    const helperConfig = await deriveClaimConfig(accepted.payload, accepted.signature, freshHelper.privateKey, 'controller', row.controllerToken, service.origin);
    check(guestConfig.key === helperConfig.key && guestConfig.key !== originalGuestConfig.key && guestConfig.sid !== originalGuestConfig.sid);
    const guestCipher = await createCipher(guestConfig), helperCipher = await createCipher(helperConfig);
    check((await call(row.base + '/api/send', { method: 'POST', token: row.current.request.token, body: await guestCipher.seal('approved', null) })).status === 200);
    const poll = await call(row.base + '/api/poll?after=0', { token: row.controllerToken });
    check(poll.status === 200 && poll.body.messages.length === 1);
    const opened = await helperCipher.open(poll.body.messages[0]); check(opened.kind === 'approved' && opened.data === null);
    stage = 'continued-stop';
    check((await call(row.base + '/stop', { method: 'POST', token: row.current.request.token, body: {} })).status === 200);
    check((await call('/v3/requests', { method: 'POST', body: row.current.request })).status === 409);
    stage = 'pending-stop';
    const pending = created[1], pendingHelper = await createEphemeralKeyPair(), pendingClaim = await claim(pending.original.request, pendingHelper);
    check(pendingClaim.response.status === 200);
    pending.claim = pendingClaim.signed; pending.controllerToken = pendingClaim.response.body.token;
    await arm(pending);
    check((await call(pending.base + '/handoff/stop', { method: 'POST', token: pending.resumeTicket, body: { nonce: pending.handoff.nonce } })).status === 200);
    check((await call(pending.base + '/handoff/stop', { method: 'POST', token: pending.resumeTicket, body: { nonce: pending.handoff.nonce } })).status === 404);
  } catch (error) { throw error instanceof GateError ? error : new GateError(stage); }
  finally {
    clearInterval(heartbeatTimer);
    await Promise.allSettled(heartbeatCalls);
    for (const row of created) await cleanup(row);
    if (started) { stage = 'cleanup'; check((await queue()).length === 0); }
  }
  return { result: 'PASS', profile: 'live-synthetic-update-coordination', assertions, syntheticRequests: created.length,
    liveRequestsAfterCleanup: 0, appConnectionTested: false, updateInstallTested: false, mailboxData: false };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    if (process.argv.length !== 3 || process.argv[2] !== '--synthetic-update') throw new GateError('explicit-synthetic-mode-required');
    const service = validateServiceConfig(JSON.parse(await fs.readFile(new URL('../public/support/service.json', import.meta.url), 'utf8')));
    const privateConfig = process.env.PST_QUICKVIEW_OPERATOR_CONFIG || path.join(process.env.XDG_CONFIG_HOME || path.join(os.homedir(), '.config'), 'pst-quickview-support/operator.json');
    const identity = await loadOperatorIdentity(privateConfig, service);
    const routeResponse = await fetch('https://am.i.mullvad.net/json', { credentials: 'omit', redirect: 'error', signal: AbortSignal.timeout(8000) });
    const route = await boundedJSON(routeResponse);
    if (!routeResponse.ok || route.mullvad_exit_ip !== true || !publicIPv4(route.ip)) throw new GateError('verified-private-route-required');
    process.stdout.write(JSON.stringify(await runSyntheticUpdateGate({ service, identity, expectedExit: route.ip })) + '\n');
  } catch (error) {
    process.stderr.write(JSON.stringify({ result: 'FAIL', stage: error instanceof GateError ? error.stage : 'configuration-or-service', privateDetailsOmitted: true }) + '\n');
    process.exitCode = 1;
  }
}
