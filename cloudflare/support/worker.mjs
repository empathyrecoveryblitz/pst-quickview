import { acceptClaim, validateRequest, validateServiceConfig, validateUpdateHandoff, decodeQueueBytes } from '../../public/support/queue-protocol.js';

export const QUEUE_LIMITS = Object.freeze({ requests: 64, messages: 128, ciphertext: 256 * 1024,
  leaseMs: 30000, bodyBytes: 32 * 1024, inFlight: 32, rateKeys: 512, replayKeys: 4096,
  registrationReaders: 8, setupReaders: 192, controlReaders: 384, adminReaders: 2, controlBodyBytes: 64 });
const HEADERS = Object.freeze({ 'Content-Type': 'application/json; charset=utf-8', 'Cache-Control': 'no-store',
  'Referrer-Policy': 'no-referrer', 'X-Content-Type-Options': 'nosniff',
  'Content-Security-Policy': "default-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'",
  'Permissions-Policy': 'camera=(), microphone=(), display-capture=(), geolocation=()' });
const HEX = /^[a-f0-9]{64}$/, ID = /^[a-f0-9]{32}$/;
const text = new TextEncoder();
function reject(status) { throw status; }
function exact(value, keys) { return value !== null && typeof value === 'object' && !Array.isArray(value) && Object.keys(value).length === keys.length && keys.every(key => Object.hasOwn(value, key)); }
function hex(bytes) { return Array.from(bytes, value => value.toString(16).padStart(2, '0')).join(''); }
async function digest(value) { return hex(new Uint8Array(await crypto.subtle.digest('SHA-256', text.encode(value)))); }
function equal(left, right) {
  if (typeof left !== 'string' || typeof right !== 'string' || left.length !== right.length) return false;
  let mismatch = 0; for (let index = 0; index < left.length; index++) mismatch |= left.charCodeAt(index) ^ right.charCodeAt(index);
  return mismatch === 0;
}
function bearer(request) { const value = request.headers.get('Authorization'); return /^Bearer [a-f0-9]{64}$/.test(value || '') ? value.slice(7) : null; }
function serviceConfig(env) {
  if (!HEX.test(env.ADMIN_TOKEN || '')) reject(503);
  try { return validateServiceConfig({ v: 3, origin: env.SERVICE_ORIGIN, operatorPublicKey: env.OPERATOR_PUBLIC_KEY, operatorLabel: env.OPERATOR_LABEL || 'PST QuickView support' }); }
  catch { reject(503); }
}
function corsHeaders(request, service) {
  const origin = request.headers.get('Origin');
  if (origin === null) return {};
  const loopback = /^http:\/\/127\.0\.0\.1:([1-9]\d{3,4})$/.exec(origin);
  if (origin !== service.origin && origin !== 'tauri://localhost' && !(loopback && Number(loopback[1]) >= 1024 && Number(loopback[1]) <= 65535)) reject(403);
  return { 'Access-Control-Allow-Origin': origin, Vary: 'Origin' };
}
async function jsonBody(request, deadlineMs, maxBytes = QUEUE_LIMITS.bodyBytes) {
  if (request.headers.get('Content-Type') !== 'application/json') reject(415);
  const size = request.headers.get('Content-Length');
  if (size !== null && (!/^\d+$/.test(size) || Number(size) > maxBytes)) reject(413);
  if (!request.body) reject(400);
  const reader = request.body.getReader(); let count = 0, timer;
  const chunks = [];
  try {
    const timeout = new Promise((_, rejectTimeout) => { timer = setTimeout(() => rejectTimeout(408), deadlineMs); });
    while (true) {
      const { value, done } = await Promise.race([reader.read(), timeout]);
      if (done) break;
      count += value.byteLength; if (count > maxBytes) reject(413);
      chunks.push(value);
    }
    const bytes = new Uint8Array(count); let offset = 0;
    for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
    try { return JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes)); } catch { reject(400); }
  } finally {
    clearTimeout(timer);
    try { void reader.cancel().catch(() => {}); } catch {}
    try { reader.releaseLock(); } catch {}
  }
}
async function deleteKeys(storage, keys) { for (let start = 0; start < keys.length; start += 128) await storage.delete(keys.slice(start, start + 128)); }
async function deleteRequest(storage, id) {
  await clearMessages(storage, id);
  await storage.delete(`request:${id}`);
}
async function clearMessages(storage, id) {
  const messages = await storage.list({ prefix: `message:${id}:`, limit: 257 });
  await deleteKeys(storage, [...messages.keys()]);
}
function expired(record, now) {
  if (record.handoff) return record.expires <= now || record.handoff.resumeDeadline <= now ||
    record.controllerSeen + QUEUE_LIMITS.leaseMs <= now ||
    (record.handoffState === 'resuming' && record.guestSeen + QUEUE_LIMITS.leaseMs <= now);
  return record.expires <= now || record.guestSeen + QUEUE_LIMITS.leaseMs <= now ||
    (record.claim !== null && record.controllerSeen + QUEUE_LIMITS.leaseMs <= now);
}
function queueRow(record) {
  return { v: 3, id: record.id, guestPublicKey: record.guestPublicKey, expires: record.expires,
    status: record.handoff ? record.handoffState : record.claim ? 'claimed' : 'waiting',
    ...(record.handoff ? { handoff: record.handoff } : {}),
    ...(record.handoffState === 'resuming' ? { resumeGuestProof: record.resumeGuestProof, guestTokenSha256: record.guestHash } : {}) };
}

// Wrangler configures this class with new_sqlite_classes. The transactional KV
// API is backed by the same SQLite store; no external database or dependencies.
export class SupportQueue {
  constructor(state, env, { now = Date.now, bodyTimeoutMs = 3000 } = {}) {
    if (!Number.isInteger(bodyTimeoutMs) || bodyTimeoutMs < 1 || bodyTimeoutMs > 3000) throw new Error('Invalid body deadline.');
    this.state = state; this.env = env; this.now = now; this.pending = 0; this.tail = Promise.resolve();
    this.inFlight = 0; this.receiving = { registration: 0, setup: 0, control: 0, admin: 0 }; this.readerKeys = new Set(); this.bodyTimeoutMs = bodyTimeoutMs;
  }
  async serialized(operation) {
    if (this.pending >= QUEUE_LIMITS.inFlight) reject(429);
    this.pending++;
    const result = this.tail.then(operation);
    this.tail = result.catch(() => {});
    try { return await result; } finally { this.pending--; }
  }
  async schedule() {
    const at = this.now() + 10000, current = await this.state.storage.getAlarm();
    if (current === null || current > at) await this.state.storage.setAlarm(at);
  }
  async cleanup() {
    const now = this.now(), storage = this.state.storage;
    const requests = await storage.list({ prefix: 'request:', limit: QUEUE_LIMITS.requests + 1 });
    if (requests.size > QUEUE_LIMITS.requests) reject(503);
    for (const [key, record] of requests) if (expired(record, now)) await storage.transaction(transaction => deleteRequest(transaction, key.slice(8)));
    for (const [prefix, limit] of [['rate:', QUEUE_LIMITS.rateKeys], ['replay:', QUEUE_LIMITS.replayKeys]]) {
      const records = await storage.list({ prefix, limit: limit + 1 });
      if (records.size > limit) reject(503);
      await deleteKeys(storage, [...records].filter(([, record]) => record.expires <= now).map(([key]) => key));
    }
  }
  async alarm() {
    return this.serialized(async () => {
      await this.state.storage.deleteAlarm(); await this.cleanup();
      if ((await this.state.storage.list({ limit: 1 })).size) await this.schedule();
    });
  }
  async rate(request, create) {
    const now = this.now(), minute = Math.floor(now / 60000);
    const address = request.headers.get('CF-Connecting-IP') || 'unavailable';
    if (address.length > 64) reject(400);
    const key = await crypto.subtle.importKey('raw', text.encode(this.env.ADMIN_TOKEN), { name: 'HMAC', hash: 'SHA-256' }, false, ['sign']);
    const identity = hex(new Uint8Array(await crypto.subtle.sign('HMAC', key, text.encode(`queue-rate-v3:${minute}:${address}`))));
    const storage = this.state.storage, ipKey = `rate:${identity}`, globalKey = 'rate:global';
    await storage.transaction(async transaction => {
      const stored = await transaction.get(ipKey), global = await transaction.get(globalKey);
      const row = stored?.minute === minute ? stored : { minute, count: 0, creates: 0, expires: (minute + 1) * 60000 };
      const total = global?.minute === minute ? global : { minute, count: 0, creates: 0, expires: (minute + 1) * 60000 };
      if (row.count >= 1500 || total.count >= 20000 || (create && (row.creates >= 16 || total.creates >= 128))) reject(429);
      if (!stored && (await transaction.list({ prefix: 'rate:', limit: QUEUE_LIMITS.rateKeys })).size >= QUEUE_LIMITS.rateKeys) reject(429);
      row.count++; total.count++; if (create) { row.creates++; total.creates++; }
      await transaction.put(ipKey, row); await transaction.put(globalKey, total);
    });
  }
  async fetch(request) {
    let cors = {}, counted = false, reading;
    try {
      const maximum = QUEUE_LIMITS.inFlight + QUEUE_LIMITS.registrationReaders + QUEUE_LIMITS.setupReaders + QUEUE_LIMITS.controlReaders + QUEUE_LIMITS.adminReaders;
      if (this.inFlight >= maximum) reject(429);
      this.inFlight++; counted = true;
      const service = serviceConfig(this.env), url = new URL(request.url);
      if (url.origin !== service.origin || url.username || url.password || url.hash) reject(421);
      cors = corsHeaders(request, service);
      const admission = await this.serialized(async () => {
        await this.cleanup();
        await this.rate(request, request.method === 'POST' && url.pathname === '/v3/requests');
        await this.schedule();
        if (request.method === 'OPTIONS') {
          if (!request.headers.has('Origin')) reject(403);
          const method = request.headers.get('Access-Control-Request-Method');
          const headers = (request.headers.get('Access-Control-Request-Headers') || '').toLowerCase().split(',').map(value => value.trim()).filter(Boolean);
          if (!['GET', 'POST', 'DELETE'].includes(method) || headers.some(value => !['authorization', 'content-type'].includes(value))) reject(403);
          return new Response(null, { status: 204, headers: { ...HEADERS, ...cors, 'Access-Control-Allow-Methods': 'GET, POST, DELETE', 'Access-Control-Allow-Headers': 'Authorization, Content-Type', 'Access-Control-Max-Age': '300' } });
        }
        if (request.method !== 'POST') return null;
        let kind, key;
        if (url.pathname === '/v3/requests' && !url.search) kind = 'registration';
        else if (/^\/v3\/admin\/requests\/[a-f0-9]{32}\/(?:claim|handoff\/stop)$/.test(url.pathname) && !url.search) {
          if (!equal(bearer(request), this.env.ADMIN_TOKEN)) reject(401);
          if (url.pathname.endsWith('/handoff/stop')) { kind = 'control'; key = `${url.pathname.split('/')[4]}:admin:control:stop`; }
          else kind = 'admin';
        } else {
          const bodyRoute = /^\/v3\/requests\/([a-f0-9]{32})\/(api\/send|send|heartbeat|stop|handoff|resume|handoff\/heartbeat|handoff\/stop)$/.exec(url.pathname);
          if (!bodyRoute || url.search) return null;
          const action = bodyRoute[2];
          const { role } = ['resume', 'handoff/heartbeat', 'handoff/stop'].includes(action) ?
            await this.authenticateHandoff(request, bodyRoute[1], action) : await this.authenticate(request, bodyRoute[1]);
          kind = ['heartbeat', 'stop', 'handoff/heartbeat', 'handoff/stop'].includes(action) ? 'control' : 'setup';
          // Normal and handoff aliases share one semantic control slot. Ticket
          // and admin revocation retain independent bounded Stop authority.
          key = `${bodyRoute[1]}:${role}:${kind}${kind === 'control' ? ':' + (action.endsWith('stop') ? 'stop' : 'heartbeat') : ''}`;
        }
        if (this.receiving[kind] >= QUEUE_LIMITS[`${kind}Readers`] || (key && this.readerKeys.has(key))) reject(429);
        this.receiving[kind]++; if (key) this.readerKeys.add(key);
        reading = { kind, key };
        return reading;
      });
      if (admission instanceof Response) return admission;
      // Untrusted upload waits never hold the serialized state queue. Separate
      // reader limits reserve capacity for existing-session heartbeat and Stop.
      const preparedBody = reading ? await jsonBody(request, this.bodyTimeoutMs, reading.kind === 'control' ? QUEUE_LIMITS.controlBodyBytes : QUEUE_LIMITS.bodyBytes) : undefined;
      if (reading) { this.finishReading(reading); reading = undefined; }
      const result = await this.serialized(() => this.handle(request, url, service, preparedBody));
      if (result instanceof Response) return result;
      return new Response(JSON.stringify(result), { headers: { ...HEADERS, ...cors } });
    } catch (error) {
      const status = Number.isInteger(error) && error >= 400 && error <= 503 ? error : 400;
      return new Response(JSON.stringify({ error: 'request_rejected' }), { status, headers: { ...HEADERS, ...cors } });
    } finally {
      if (reading) this.finishReading(reading);
      if (counted) this.inFlight--;
    }
  }
  finishReading({ kind, key }) {
    this.receiving[kind]--; if (key) this.readerKeys.delete(key);
  }
  async authenticate(request, id) {
    const record = await this.state.storage.get(`request:${id}`); if (!record) reject(404);
    const token = bearer(request); if (!token) reject(401);
    const hash = await digest(token), role = equal(hash, record.guestHash) ? 'guest' : record.controllerHash && equal(hash, record.controllerHash) ? 'controller' : null;
    if (!role) reject(401);
    if (expired(record, this.now())) reject(410);
    return { record, role };
  }
  async authenticateHandoff(request, id, action) {
    const record = await this.state.storage.get(`request:${id}`); if (!record) reject(404);
    const token = bearer(request); if (!token) reject(401);
    const hash = await digest(token);
    if (expired(record, this.now())) reject(410);
    if (record.handoff && ['resume', 'handoff/stop'].includes(action) && equal(hash, record.handoff.resumeTicketSha256)) return { record, role: 'ticket' };
    if (record.handoffState === 'updating' && action !== 'resume') {
      if (equal(hash, record.guestHash)) return { record, role: 'guest' };
      if (equal(hash, record.controllerHash)) return { record, role: 'controller' };
    }
    reject(401);
  }
  async handle(request, url, service, preparedBody) {
    const storage = this.state.storage, now = this.now(), token = bearer(request);
    if (url.pathname.startsWith('/v3/admin/')) {
      if (!token || !equal(token, this.env.ADMIN_TOKEN)) reject(401);
      if (url.pathname === '/v3/admin/queue' && request.method === 'GET' && !url.search) {
        const rows = await storage.list({ prefix: 'request:', limit: QUEUE_LIMITS.requests });
        return { requests: [...rows.values()].map(queueRow) };
      }
      const stop = /^\/v3\/admin\/requests\/([a-f0-9]{32})\/handoff\/stop$/.exec(url.pathname);
      if (stop && !url.search && request.method === 'POST') {
        const record = await storage.get(`request:${stop[1]}`); if (!record) reject(404);
        if (!record.handoff) reject(409);
        if (!exact(preparedBody, ['nonce']) || preparedBody.nonce !== record.handoff.nonce) reject(400);
        await storage.transaction(transaction => deleteRequest(transaction, record.id)); return { stopped: true };
      }
      const match = /^\/v3\/admin\/requests\/([a-f0-9]{32})\/claim$/.exec(url.pathname);
      if (!match || url.search || request.method !== 'POST') reject(404);
      const record = await storage.get(`request:${match[1]}`); if (!record) reject(404);
      if (record.handoffState === 'updating') reject(409);
      if (!record.claim && (record.handoffState === 'resuming') !== Object.hasOwn(preparedBody || {}, 'resumeProof')) reject(400);
      let claim;
      try { claim = await acceptClaim({ v: 3, id: record.id, guestPublicKey: record.guestPublicKey, expires: record.expires }, preparedBody, service); }
      catch (error) { if (typeof error === 'number') throw error; reject(400); }
      return await storage.transaction(async transaction => {
        const current = await transaction.get(`request:${record.id}`); if (!current || expired(current, this.now())) reject(410);
        if (current.claim) {
          if (JSON.stringify(current.claim) !== JSON.stringify(claim)) reject(409);
          return { token: current.controllerToken, claim: current.claim };
        }
        if (current.handoff && (current.handoffState !== 'resuming' || claim.payload.sid === current.handoff.parentSid || claim.payload.helperPublicKey === current.previousHelperPublicKey)) reject(400);
        const controllerToken = hex(crypto.getRandomValues(new Uint8Array(32)));
        current.claim = claim; current.controllerToken = controllerToken; current.controllerHash = await digest(controllerToken); current.controllerSeen = this.now();
        delete current.handoff; delete current.handoffState; delete current.previousHelperPublicKey; delete current.resumeGuestProof;
        await transaction.put(`request:${record.id}`, current);
        return { token: controllerToken, claim };
      });
    }
    if (url.pathname === '/v3/requests' && request.method === 'POST' && !url.search) {
      let row; try { row = validateRequest(preparedBody, { requireToken: true }); } catch (error) { if (typeof error === 'number') throw error; reject(400); }
      if (row.expires <= now || row.expires > now + 30 * 60 * 1000) reject(400);
      const guestHash = await digest(row.token);
      await crypto.subtle.importKey('raw', decodeQueueBytes(row.guestPublicKey, 65), { name: 'ECDH', namedCurve: 'P-256' }, false, []);
      return await storage.transaction(async transaction => {
        const existing = await transaction.get(`request:${row.id}`);
        if (existing && !existing.handoff && !expired(existing, now) && equal(existing.guestHash, guestHash) && existing.guestPublicKey === row.guestPublicKey && existing.expires === row.expires) {
          return { v: 3, id: row.id, expires: row.expires, claim: existing.claim };
        }
        if (await transaction.get(`replay:${row.id}`)) reject(409);
        if ((await transaction.list({ prefix: 'request:', limit: QUEUE_LIMITS.requests })).size >= QUEUE_LIMITS.requests ||
            (await transaction.list({ prefix: 'replay:', limit: QUEUE_LIMITS.replayKeys })).size >= QUEUE_LIMITS.replayKeys) reject(429);
        await transaction.put(`replay:${row.id}`, { expires: row.expires });
        await transaction.put(`request:${row.id}`, { v: 3, id: row.id, guestPublicKey: row.guestPublicKey, expires: row.expires,
          guestHash, guestSeen: now, claim: null, controllerHash: null, controllerSeen: null,
          sequences: { guest: 0, controller: 0 }, bytes: { guest: 0, controller: 0 }, calls: { guest: 0, controller: 0 } });
        return { v: 3, id: row.id, expires: row.expires, claim: null };
      });
    }
    const transition = /^\/v3\/requests\/([a-f0-9]{32})\/(resume|handoff|handoff\/heartbeat|handoff\/stop)$/.exec(url.pathname);
    if (transition && request.method === 'POST' && !url.search) {
      const id = transition[1], action = transition[2];
      const { record, role } = action === 'handoff' ? await this.authenticate(request, id) : await this.authenticateHandoff(request, id, action);
      if (role !== 'ticket') {
        if (++record.calls[role] > 6000) reject(429);
        await storage.put(`request:${id}`, record);
      }
      if (action === 'handoff') {
        if (role !== 'guest') reject(403);
        if (!record.claim || record.handoff) reject(409);
        let base; try { base = validateUpdateHandoff(preparedBody, { requireTicketHash: false, now }); } catch { reject(400); }
        if (base.parentId !== id || base.parentSid !== record.claim.payload.sid || base.originalExpires !== record.expires) reject(400);
        const resumeTicket = hex(crypto.getRandomValues(new Uint8Array(32)));
        const handoff = { ...base, resumeTicketSha256: await digest(resumeTicket) };
        record.handoff = handoff; record.handoffState = 'updating'; record.previousHelperPublicKey = record.claim.payload.helperPublicKey;
        record.sequences = { guest: 0, controller: 0 }; record.bytes = { guest: 0, controller: 0 };
        await storage.transaction(async transaction => { await clearMessages(transaction, id); await transaction.put(`request:${id}`, record); });
        return { resumeTicket, handoff };
      }
      if (action === 'handoff/stop' || action === 'handoff/heartbeat') {
        if (!exact(preparedBody, ['nonce']) || preparedBody.nonce !== record.handoff.nonce) reject(400);
        if (action === 'handoff/stop') { await storage.transaction(transaction => deleteRequest(transaction, id)); return { stopped: true }; }
        if (role === 'controller') record.controllerSeen = now;
        else record.guestSeen = now;
        await storage.put(`request:${id}`, record); return { alive: true, expiresAt: record.expires };
      }
      if (record.handoffState !== 'updating') reject(409);
      if (!exact(preparedBody, ['v', 'nonce', 'targetVersion', 'artifactSha256', 'guestPublicKey', 'token', 'proof']) || preparedBody.v !== 1 ||
          preparedBody.nonce !== record.handoff.nonce || preparedBody.targetVersion !== record.handoff.targetVersion || preparedBody.artifactSha256 !== record.handoff.artifactSha256 ||
          !HEX.test(preparedBody.token) || !HEX.test(preparedBody.proof) || preparedBody.guestPublicKey === record.guestPublicKey || preparedBody.guestPublicKey === record.previousHelperPublicKey) reject(400);
      let key; try { key = decodeQueueBytes(preparedBody.guestPublicKey, 65); } catch { reject(400); }
      await crypto.subtle.importKey('raw', key, { name: 'ECDH', namedCurve: 'P-256' }, false, []);
      const guestHash = await digest(preparedBody.token);
      if ([record.guestHash, record.controllerHash, record.handoff.resumeTicketSha256].some(hash => equal(hash, guestHash))) reject(400);
      // HMAC proofs remain opaque: only the two approved endpoints know the
      // continuation secret. Fresh role tokens revoke all old peer authority.
      record.handoffState = 'resuming'; record.guestHash = guestHash; record.guestPublicKey = preparedBody.guestPublicKey; record.guestSeen = now;
      record.resumeGuestProof = preparedBody.proof; record.claim = null; record.controllerHash = null; delete record.controllerToken;
      record.sequences = { guest: 0, controller: 0 }; record.bytes = { guest: 0, controller: 0 }; record.calls = { guest: 0, controller: 0 };
      await storage.transaction(async transaction => { await clearMessages(transaction, id); await transaction.put(`request:${id}`, record); });
      return { v: 3, id, expires: record.expires, claim: null };
    }
    const match = /^\/v3\/requests\/([a-f0-9]{32})(?:\/(api\/send|api\/poll|send|poll|heartbeat|stop))?$/.exec(url.pathname);
    if (!match) reject(404);
    const id = match[1], action = match[2]?.replace('api/', '');
    const { record, role } = await this.authenticate(request, id);
    if (record.handoffState === 'updating' || (record.handoff && !['heartbeat', undefined].includes(action))) reject(409);
    if (++record.calls[role] > 6000) reject(429);
    await storage.put(`request:${id}`, record);
    if (!action && request.method === 'GET' && !url.search) {
      if (role !== 'guest') reject(403);
      return { v: 3, id, expires: record.expires, claim: record.claim };
    }
    if ((!action && request.method === 'DELETE') || (action === 'stop' && request.method === 'POST')) {
      if (url.search || (request.method === 'DELETE' && request.body !== null)) reject(400);
      if (request.method === 'POST' && !exact(preparedBody, [])) reject(400);
      await storage.transaction(transaction => deleteRequest(transaction, id)); return { stopped: true };
    }
    if (action === 'heartbeat' && request.method === 'POST' && !url.search) {
      if (!exact(preparedBody, [])) reject(400);
      record[role === 'guest' ? 'guestSeen' : 'controllerSeen'] = now;
      await storage.put(`request:${id}`, record); return { alive: true, expiresAt: record.expires };
    }
    if (!record.claim) reject(409);
    if (action === 'poll' && request.method === 'GET') {
      if ([...url.searchParams].length !== 1 || !/^(0|[1-9]\d{0,2})$/.test(url.searchParams.get('after') || '')) reject(400);
      const after = Number(url.searchParams.get('after')); if (after > QUEUE_LIMITS.messages) reject(400);
      const peer = role === 'guest' ? 'controller' : 'guest';
      const rows = await storage.list({ prefix: `message:${id}:${peer}:`, limit: QUEUE_LIMITS.messages });
      return { messages: [...rows.values()].filter(envelope => envelope.seq > after), expiresAt: record.expires };
    }
    if (action === 'send' && request.method === 'POST' && !url.search) {
      let envelope = preparedBody;
      if (!exact(envelope, ['v', 'sid', 'from', 'seq', 'expires', 'iv', 'ct']) || envelope.v !== 1 || envelope.sid !== record.claim.payload.sid ||
          envelope.from !== role || envelope.expires !== record.expires || !Number.isInteger(envelope.seq) || envelope.seq < 1 || envelope.seq > QUEUE_LIMITS.messages) reject(400);
      try { decodeQueueBytes(envelope.iv, 12); } catch { reject(400); }
      if (typeof envelope.ct !== 'string' || !/^[A-Za-z0-9_-]{22,27307}$/.test(envelope.ct)) reject(400);
      let ciphertext; try { ciphertext = Uint8Array.from(atob(envelope.ct.replaceAll('-', '+').replaceAll('_', '/') + '='.repeat((4 - envelope.ct.length % 4) % 4)), value => value.charCodeAt(0)); } catch { reject(400); }
      try { decodeQueueBytes(envelope.ct, ciphertext.length); } catch { reject(400); }
      if (ciphertext.length < 16 || ciphertext.length > 20 * 1024) reject(400);
      envelope = { v: envelope.v, sid: envelope.sid, from: envelope.from, seq: envelope.seq, expires: envelope.expires, iv: envelope.iv, ct: envelope.ct };
      return await storage.transaction(async transaction => {
        const current = await transaction.get(`request:${id}`); if (!current || expired(current, this.now())) reject(410);
        const key = `message:${id}:${role}:${String(envelope.seq).padStart(3, '0')}`;
        if (envelope.seq <= current.sequences[role]) {
          if (JSON.stringify(await transaction.get(key)) !== JSON.stringify(envelope)) reject(409);
          return { accepted: true };
        }
        if (envelope.seq !== current.sequences[role] + 1) reject(409);
        if (current.bytes[role] + ciphertext.length > QUEUE_LIMITS.ciphertext) reject(413);
        current.sequences[role] = envelope.seq; current.bytes[role] += ciphertext.length;
        await transaction.put(key, envelope); await transaction.put(`request:${id}`, current);
        return { accepted: true };
      });
    }
    reject(405);
  }
}

export default {
  async fetch(request, env) {
    try {
      const service = serviceConfig(env);
      if (new URL(request.url).origin !== service.origin || !env.SUPPORT_QUEUE) reject(503);
      return await env.SUPPORT_QUEUE.get(env.SUPPORT_QUEUE.idFromName('support-queue-v3')).fetch(request);
    } catch {
      return new Response('{"error":"service_unavailable"}', { status: 503, headers: HEADERS });
    }
  },
};
