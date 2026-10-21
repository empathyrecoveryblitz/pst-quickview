import { createSupportPeer } from './peer.js';
import { validateServiceConfig, decodeQueueBytes, createRequest, acceptClaim, deriveClaimConfig, validateUpdateHandoff, queueTokenSha256, ackProof, resumedGuestProof, resumedClaimProof, verifyUpdateProof, createResumedRequest } from './queue-protocol.js';

const $ = id => document.getElementById(id);
let sessionId, peer, service, requestState, connected = false, ending = false, claiming = false;
let pollTimer, expiryTimer, queueTimer, heartbeatTimer, lastFrame, heartbeatBusy = false;
let peerSid, handoffBusy = false, handoffGrant, handoffAttempt, handoffAcknowledged = false, updateSuspended = false, resumedGrant, resolveHandoffAck, resumeTimer;
const pendingRequests = new Set();
const invoke = (command, args) => {
  if (typeof window.__TAURI_INTERNALS__?.invoke !== 'function') return Promise.reject(new Error('native'));
  return window.__TAURI_INTERNALS__.invoke(command, args);
};

async function boundedJSON(response) {
  if (!response.ok || !response.body?.getReader) throw new Error('service');
  const reader = response.body.getReader(), chunks = [];
  let length = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      length += value.byteLength;
      if (length > 64 * 1024) throw new Error('service');
      chunks.push(value);
    }
  } catch (error) {
    try { await reader.cancel(); } catch {}
    throw error;
  } finally { reader.releaseLock(); }
  const bytes = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
  return JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes));
}

async function queueRequest(path, method = 'GET', body, token = requestState?.request.token) {
  if (ending || !service || !requestState) throw new Error('stopped');
  const controller = new AbortController();
  pendingRequests.add(controller);
  const timeout = setTimeout(() => controller.abort(), 6000);
  try {
    const response = await fetch(service.origin + path, {
      method, mode: 'cors', credentials: 'omit', cache: 'no-store', redirect: 'error', referrerPolicy: 'no-referrer',
      headers: { Authorization: `Bearer ${token}`, ...(body === undefined ? {} : { 'Content-Type': 'application/json' }) },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }), signal: controller.signal,
    });
    return await boundedJSON(response);
  } finally { clearTimeout(timeout); pendingRequests.delete(controller); }
}

function bestEffortQueueStop(origin, request) {
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), 1500);
  void fetch(`${origin}/v3/requests/${request.id}/stop`, {
    method: 'POST', mode: 'cors', credentials: 'omit', cache: 'no-store', redirect: 'error', referrerPolicy: 'no-referrer', keepalive: true,
    headers: { Authorization: `Bearer ${request.token}`, 'Content-Type': 'application/json' },
    body: '{}', signal: controller.signal,
  }).catch(() => {}).finally(() => clearTimeout(timeout));
}

function bestEffortHandoffStop(origin, id, nonce, token) {
  void fetch(`${origin}/v3/requests/${id}/handoff/stop`, {
    method: 'POST', mode: 'cors', credentials: 'omit', cache: 'no-store', redirect: 'error', referrerPolicy: 'no-referrer', keepalive: true,
    headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
    body: JSON.stringify({ nonce }), signal: AbortSignal.timeout(1500),
  }).catch(() => {});
}

async function stop(reason = 'stopped') {
  if (ending) return;
  ending = true; connected = false; lastFrame = undefined;
  clearTimeout(pollTimer); clearTimeout(expiryTimer); clearTimeout(queueTimer); clearTimeout(resumeTimer); clearInterval(heartbeatTimer);
  for (const request of pendingRequests) request.abort();
  pendingRequests.clear();
  resolveHandoffAck?.(null); resolveHandoffAck = undefined;
  peer?.close();
  const grant = handoffGrant || resumedGrant;
  if (service && grant) bestEffortHandoffStop(service.origin, grant.handoff.parentId, grant.handoff.nonce, grant.resumeTicket);
  if (service && handoffAttempt) bestEffortHandoffStop(service.origin, handoffAttempt.parentId, handoffAttempt.nonce, handoffAttempt.token);
  if (service && requestState) bestEffortQueueStop(service.origin, requestState.request);
  requestState = undefined;
  handoffGrant = undefined; handoffAttempt = undefined; resumedGrant = undefined;
  handoffAcknowledged = false;
  $('stop').disabled = true;
  $('status').textContent = reason === 'expired' ? 'The approved session expired. Support is closed.' : 'Support stopped. Access is closed.';
  if (sessionId) try { await invoke('stop_support_session', { sessionId }); } catch { /* Local expiry and the short queue lease independently revoke access. */ }
}

async function pollFrames() {
  if (ending) return;
  try {
    if (connected) {
      if (!handoffBusy) {
        const pending = await invoke('poll_support_update_handoff', { sessionId });
        if (ending) return;
        if (pending) { void prepareHandoff(pending); }
      }
      if (handoffBusy || updateSuspended) { pollTimer = setTimeout(() => { void pollFrames(); }, 500); return; }
      const frame = await invoke('poll_support_frame', { sessionId });
      if (ending) return;
      if (frame && frame.id !== lastFrame && peer.sendFrame(frame)) lastFrame = frame.id;
    }
  } catch { await stop('failed'); return; }
  pollTimer = setTimeout(() => { void pollFrames(); }, 500);
}

async function prepareHandoff(pending) {
  if (ending || handoffBusy || !connected || !service || !requestState || !peerSid) return;
  handoffBusy = true; lastFrame = undefined;
  try {
    if (pending?.v !== 1 || pending.nativeSessionId !== sessionId || pending.originalExpires !== requestState.request.expires
      || typeof pending.resumeAuth !== 'string' || !/^[a-f0-9]{64}$/.test(pending.resumeAuth)) throw Error('handoff');
    const raw = validateUpdateHandoff({ v: 1, parentId: requestState.request.id, parentSid: peerSid, nonce: pending.nonce,
      targetVersion: pending.targetVersion, artifactSha256: pending.artifactSha256, nativeSessionId: sessionId,
      originalExpires: pending.originalExpires, resumeDeadline: pending.resumeDeadline }, { requireTicketHash: false });
    handoffAttempt = { parentId: raw.parentId, nonce: raw.nonce, token: requestState.request.token };
    const issued = await queueRequest(`/v3/requests/${raw.parentId}/handoff`, 'POST', raw);
    if (ending) {
      if (/^[a-f0-9]{64}$/.test(issued?.resumeTicket || '')) bestEffortHandoffStop(service.origin, raw.parentId, raw.nonce, issued.resumeTicket);
      return;
    }
    const handoff = validateUpdateHandoff(issued.handoff);
    if (typeof issued.resumeTicket !== 'string' || handoff.resumeTicketSha256 !== await queueTokenSha256(issued.resumeTicket)
      || JSON.stringify({ ...handoff, resumeTicketSha256: undefined }) !== JSON.stringify(raw)) throw Error('handoff');
    handoffGrant = { handoff, resumeTicket: issued.resumeTicket, resumeAuth: pending.resumeAuth };
    handoffAttempt = undefined;
    const acknowledgement = new Promise(resolve => { resolveHandoffAck = resolve; });
    const timeout = setTimeout(() => { resolveHandoffAck?.(null); }, 8000);
    let ack;
    try {
      if (!peer.sendUpdateHandoff({ handoff, resumeAuth: pending.resumeAuth })) throw Error('handoff');
      ack = await acknowledgement;
    } finally { clearTimeout(timeout); resolveHandoffAck = undefined; }
    if (ending || !ack || ack.nonce !== handoff.nonce || !verifyUpdateProof(await ackProof(pending.resumeAuth, handoff), ack.proof)) throw Error('handoff');
    handoffAcknowledged = true;
    const saved = await invoke('complete_support_update_handoff', { sessionId, handoff, resumeTicket: issued.resumeTicket, proof: ack.proof });
    if (ending || saved !== true) throw Error('handoff');
    if (!suspendApprovedUpdate()) throw Error('handoff');
  } catch { await stop('failed'); }
}

function suspendApprovedUpdate() {
  if (ending || !handoffGrant || !handoffAcknowledged) return false;
  if (updateSuspended) return true;
  updateSuspended = true; connected = false; clearInterval(heartbeatTimer);
  if (!peer.suspendForUpdate()) { updateSuspended = false; return false; }
  $('status').textContent = 'Update approved. Sharing and control are paused while PST QuickView restarts. Stop cancels resumption.';
  return true;
}

async function connect(config, queue) {
  if (ending) return;
  peerSid = config.sid;
  peer = await createSupportPeer({
    config, ...(queue ? { queue } : {}), onFrame: () => { void stop('failed'); },
    onAction: action => { if (!ending && connected) void invoke('support_submit_action', { sessionId, action }).catch(() => stop('failed')); },
    onState(state) {
      if (ending) return;
      if (state === 'connected') {
        if (resumedGrant) {
          if (Date.now() >= resumedGrant.handoff.resumeDeadline) { void stop('expired'); return; }
          void invoke('support_confirm_update_resume', { sessionId }).then(confirmed => {
            if (ending || confirmed !== true || !resumedGrant || Date.now() >= resumedGrant.handoff.resumeDeadline) { void stop('failed'); return; }
            clearTimeout(resumeTimer); resumedGrant = undefined; connected = true;
            $('status').textContent = 'Connected. Your approved helper can view and use the shared PST QuickView controls.';
          }).catch(() => stop('failed'));
        } else { connected = true; $('status').textContent = 'Connected. Your approved helper can view and use the shared PST QuickView controls.'; }
      }
      else if (state.startsWith('failed:')) void stop('failed');
      else if (state === 'negotiating') $('status').textContent = 'Your helper accepted. Establishing the direct encrypted connection…';
      else if (state === 'approved' || state === 'waiting') $('status').textContent = 'Approved. Waiting for your helper to start the direct connection…';
    },
    onClose: reason => { void stop(reason); },
    onUpdateHandoffAck: ack => { if (!ending) resolveHandoffAck?.(ack); },
  });
  if (ending) { peer.close(); return; }
  // Approval was explicitly recorded in the main app before this window existed.
  await peer.approve();
}

async function heartbeat() {
  if (ending || heartbeatBusy || handoffBusy || updateSuspended || !requestState) return;
  heartbeatBusy = true;
  try { await queueRequest(`/v3/requests/${requestState.request.id}/heartbeat`, 'POST', {}); }
  catch { if (!handoffBusy && !updateSuspended) await stop('service'); }
  finally { heartbeatBusy = false; }
}

async function pollClaim() {
  if (ending || claiming || !requestState) return;
  try {
    const request = requestState.request;
    const result = await queueRequest(`/v3/requests/${request.id}`);
    if (ending) return;
    if (result?.v !== 3 || result.id !== request.id || result.expires !== request.expires || !Object.hasOwn(result, 'claim')) throw new Error('service');
    if (result.claim !== null) {
      claiming = true;
      const claim = await acceptClaim(request, result.claim, service);
      if (resumedGrant) {
        const proof = await resumedClaimProof(resumedGrant.resumeAuth, resumedGrant.handoff, result.claim);
        if (!verifyUpdateProof(proof, result.claim.resumeProof)) throw Error('continuity');
      }
      if (ending || !requestState) return;
      const config = await deriveClaimConfig(claim.payload, claim.signature, requestState.privateKey, 'guest', request.token, service.origin);
      if (ending) return;
      requestState.privateKey = undefined;
      await connect(config, { origin: service.origin, requestId: request.id });
      return;
    }
  } catch { await stop('service'); return; }
  queueTimer = setTimeout(() => { void pollClaim(); }, 3000);
}

async function requestSupport(config) {
  service = validateServiceConfig({ v: config.v, origin: config.origin, operatorPublicKey: config.operatorPublicKey, operatorLabel: config.operatorLabel });
  if (!Number.isSafeInteger(config.expires) || config.expires <= Date.now() || config.expires > Date.now() + 30 * 60 * 1000) throw new Error('session');
  // Validate the pinned curve point before registering anything with the service.
  await crypto.subtle.importKey('raw', decodeQueueBytes(service.operatorPublicKey, 65), { name: 'ECDSA', namedCurve: 'P-256' }, false, ['verify']);
  if (ending) return;
  $('operator').textContent = `Approved helper: ${service.operatorLabel}.`;
  resumedGrant = config.resume;
  if (resumedGrant) {
    const handoff = validateUpdateHandoff(resumedGrant.handoff);
    if (handoff.originalExpires !== config.expires || !/^[a-f0-9]{64}$/.test(resumedGrant.resumeAuth || '')
      || handoff.resumeTicketSha256 !== await queueTokenSha256(resumedGrant.resumeTicket)) throw Error('continuity');
    requestState = await createResumedRequest(handoff);
    resumeTimer = setTimeout(() => { if (resumedGrant) void stop('expired'); }, Math.max(1, handoff.resumeDeadline - Date.now()));
  } else requestState = await createRequest({ expires: config.expires });
  if (ending) { requestState = undefined; return; }
  const created = requestState.request, origin = service.origin;
  try {
    if (resumedGrant) {
      const handoff = resumedGrant.handoff;
      const proof = await resumedGuestProof(resumedGrant.resumeAuth, handoff, created.guestPublicKey, await queueTokenSha256(created.token));
      await queueRequest(`/v3/requests/${created.id}/resume`, 'POST', { v: 1, nonce: handoff.nonce, targetVersion: handoff.targetVersion,
        artifactSha256: handoff.artifactSha256, guestPublicKey: created.guestPublicKey, token: created.token, proof }, resumedGrant.resumeTicket);
    } else await queueRequest('/v3/requests', 'POST', created);
  }
  catch (error) {
    if (ending) bestEffortQueueStop(origin, created);
    throw error;
  }
  // A delayed registration can complete after Stop's first removal request.
  if (ending) { bestEffortQueueStop(origin, created); return; }
  $('status').textContent = 'Support requested. Waiting for your approved helper. You can stop this request at any time.';
  // Keep the short server lease alive during waiting AND the direct session.
  heartbeatTimer = setInterval(() => { void heartbeat(); }, 10000);
  await heartbeat();
  if (!ending) await pollClaim();
}

$('stop').onclick = () => { void stop(); };
addEventListener('pagehide', () => { if (!updateSuspended) void stop(); });
addEventListener('pqv-support-revoked', () => { void stop(); });
addEventListener('pqv-support-update-restart', () => { if (!suspendApprovedUpdate()) void stop('failed'); });
try {
  const query = new URLSearchParams(location.search);
  sessionId = query.get('session');
  if ([...query.keys()].length !== 1 || !/^[a-f0-9]{32}$/.test(sessionId || '')) throw new Error('session');
  const config = await invoke('bootstrap_support_session', { sessionId });
  if (ending || config.role !== 'guest') throw new Error('session');
  if (!Number.isSafeInteger(config.expires) || config.expires <= Date.now() || config.expires > Date.now() + 30 * 60 * 1000) throw new Error('session');
  $('expiry').textContent = 'Session ends by ' + new Date(config.expires).toLocaleTimeString() + '.';
  expiryTimer = setTimeout(() => { void stop('expired'); }, Math.max(1, config.expires - Date.now()));
  if (config.v === 3) await requestSupport(config);
  else if (config.v === 2) { $('operator').textContent = 'Approved helper: invited technician.'; await connect(config); }
  else throw new Error('session');
  if (!ending) void pollFrames();
} catch {
  await stop('failed');
  $('status').textContent = 'This support request could not start. Return to PST QuickView to approve a new request.';
}
