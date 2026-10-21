// Queue metadata and authenticated key agreement only. Mail, frames and actions
// never belong in this protocol or in the coordination service.
import { publicIPv4 } from './protocol.js';

const encoder = new TextEncoder();
const ownPublicKeys = new WeakMap();
const HEX32 = /^[a-f0-9]{32}$/, HEX64 = /^[a-f0-9]{64}$/;
const LABEL = '[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?';
const ORIGIN = new RegExp(`^https://${LABEL}\\.${LABEL}\\.workers\\.dev$`);
const ORDER = BigInt('0xffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551');
const fail = () => { throw new Error('Invalid support queue data.'); };
function runtime(value = globalThis.crypto) {
  if (!value?.subtle || typeof value.getRandomValues !== 'function') fail();
  return value;
}
function exact(value, keys) {
  if (!value || typeof value !== 'object' || Array.isArray(value) || ![Object.prototype, null].includes(Object.getPrototypeOf(value))) return false;
  const actual = Reflect.ownKeys(value);
  return actual.length === keys.length && actual.every(key => typeof key === 'string' && keys.includes(key) &&
    Object.getOwnPropertyDescriptor(value, key)?.get === undefined && Object.getOwnPropertyDescriptor(value, key)?.set === undefined);
}
export function encodeQueueBytes(bytes) {
  let text = ''; for (const value of bytes) text += String.fromCharCode(value);
  return btoa(text).replaceAll('+', '-').replaceAll('/', '_').replace(/=+$/, '');
}
export function decodeQueueBytes(value, length) {
  if (typeof value !== 'string' || value.length !== Math.ceil(length * 4 / 3) || !/^[A-Za-z0-9_-]+$/.test(value)) fail();
  let bytes;
  try { bytes = Uint8Array.from(atob(value.replaceAll('-', '+').replaceAll('_', '/') + '='.repeat((4 - value.length % 4) % 4)), character => character.charCodeAt(0)); }
  catch { fail(); }
  if (bytes.length !== length || encodeQueueBytes(bytes) !== value) fail();
  return bytes;
}
function publicKey(value) { const bytes = decodeQueueBytes(value, 65); if (bytes[0] !== 4) fail(); return bytes; }
function expiry(value) {
  if (!Number.isSafeInteger(value) || value <= Date.now() || value > Date.now() + 30 * 60 * 1000) fail();
  return value;
}
function randomHex(length, crypto) {
  return Array.from(crypto.getRandomValues(new Uint8Array(length)), value => value.toString(16).padStart(2, '0')).join('');
}
export function validateServiceConfig(raw) {
  if (!exact(raw, ['v', 'origin', 'operatorPublicKey', 'operatorLabel']) || raw.v !== 3 || typeof raw.origin !== 'string' || !ORIGIN.test(raw.origin) ||
      typeof raw.operatorLabel !== 'string' || !raw.operatorLabel || raw.operatorLabel.trim() !== raw.operatorLabel || raw.operatorLabel.length > 80 || /[\u0000-\u001f\u007f]/.test(raw.operatorLabel)) fail();
  publicKey(raw.operatorPublicKey);
  return { v: 3, origin: raw.origin, operatorPublicKey: raw.operatorPublicKey, operatorLabel: raw.operatorLabel };
}
export function validateRequest(raw, { requireToken = false } = {}) {
  const base = ['v', 'id', 'guestPublicKey', 'expires'];
  const privateRequest = exact(raw, [...base, 'token']);
  const handoffRow = exact(raw, [...base, 'status', 'handoff']) && raw.status === 'updating';
  const resumeRow = exact(raw, [...base, 'status', 'handoff', 'resumeGuestProof', 'guestTokenSha256']) && raw.status === 'resuming' && HEX64.test(raw.resumeGuestProof) && HEX64.test(raw.guestTokenSha256);
  if (handoffRow || resumeRow) {
    const handoff = validateUpdateHandoff(raw.handoff);
    if (handoff.parentId !== raw.id || handoff.originalExpires !== raw.expires) fail();
  }
  const publicRequest = exact(raw, base) || (exact(raw, [...base, 'status']) && ['waiting', 'claimed'].includes(raw.status)) || handoffRow || resumeRow;
  if ((!privateRequest && (!publicRequest || requireToken)) || raw.v !== 3 || !HEX32.test(raw.id) || (privateRequest && !HEX64.test(raw.token))) fail();
  publicKey(raw.guestPublicKey); expiry(raw.expires);
  return { v: 3, id: raw.id, ...(privateRequest ? { token: raw.token } : {}), guestPublicKey: raw.guestPublicKey, expires: raw.expires };
}
export async function createEphemeralKeyPair({ crypto } = {}) {
  const api = runtime(crypto);
  const pair = await api.subtle.generateKey({ name: 'ECDH', namedCurve: 'P-256' }, false, ['deriveBits']);
  const publicKey = encodeQueueBytes(new Uint8Array(await api.subtle.exportKey('raw', pair.publicKey)));
  ownPublicKeys.set(pair.privateKey, publicKey);
  return { publicKey, privateKey: pair.privateKey };
}
export async function createRequest({ expires, crypto } = {}) {
  expiry(expires);
  const api = runtime(crypto), pair = await createEphemeralKeyPair({ crypto: api });
  return { request: { v: 3, id: randomHex(16, api), token: randomHex(32, api), guestPublicKey: pair.publicKey, expires }, privateKey: pair.privateKey };
}
export function validateClaimPayload(raw) {
  if (!exact(raw, ['v', 'id', 'guestPublicKey', 'helperPublicKey', 'expectedExit', 'expires', 'sid']) || raw.v !== 3 || !HEX32.test(raw.id) || !HEX32.test(raw.sid) || !publicIPv4(raw.expectedExit)) fail();
  publicKey(raw.guestPublicKey); publicKey(raw.helperPublicKey); expiry(raw.expires);
  if (raw.guestPublicKey === raw.helperPublicKey) fail();
  // Fixed property order is the signed wire format, independent of input order.
  return { v: 3, id: raw.id, guestPublicKey: raw.guestPublicKey, helperPublicKey: raw.helperPublicKey, expectedExit: raw.expectedExit, expires: raw.expires, sid: raw.sid };
}
export function canonicalClaimPayload(raw) { return JSON.stringify(validateClaimPayload(raw)); }
function integer(bytes) { return BigInt('0x' + Array.from(bytes, value => value.toString(16).padStart(2, '0')).join('')); }
function signatureBytes(value) {
  const bytes = decodeQueueBytes(value, 64), r = integer(bytes.subarray(0, 32)), s = integer(bytes.subarray(32));
  if (r < 1n || r >= ORDER || s < 1n || s > ORDER / 2n) fail();
  return bytes;
}
export function validateClaim(raw) {
  const resumed = exact(raw, ['payload', 'signature', 'resumeProof']);
  if ((!resumed && !exact(raw, ['payload', 'signature'])) || (resumed && !HEX64.test(raw.resumeProof))) fail();
  signatureBytes(raw.signature);
  return { payload: validateClaimPayload(raw.payload), signature: raw.signature, ...(resumed ? { resumeProof: raw.resumeProof } : {}) };
}
export async function createClaim(request, helperPublicKey, expectedExit, operatorPrivateKey) {
  const row = validateRequest(request), api = runtime();
  const payload = validateClaimPayload({ v: 3, id: row.id, guestPublicKey: row.guestPublicKey, helperPublicKey, expectedExit, expires: row.expires, sid: randomHex(16, api) });
  const bytes = new Uint8Array(await api.subtle.sign({ name: 'ECDSA', hash: 'SHA-256' }, operatorPrivateKey, encoder.encode(canonicalClaimPayload(payload))));
  if (bytes.length !== 64) fail();
  const s = integer(bytes.subarray(32));
  if (s > ORDER / 2n) {
    const low = (ORDER - s).toString(16).padStart(64, '0');
    for (let index = 0; index < 32; index++) bytes[32 + index] = Number.parseInt(low.slice(index * 2, index * 2 + 2), 16);
  }
  return validateClaim({ payload, signature: encodeQueueBytes(bytes) });
}
export async function verifyClaim(raw, service) {
  const config = validateServiceConfig(service), claim = validateClaim(raw), api = runtime();
  const key = await api.subtle.importKey('raw', publicKey(config.operatorPublicKey), { name: 'ECDSA', namedCurve: 'P-256' }, false, ['verify']);
  if (!await api.subtle.verify({ name: 'ECDSA', hash: 'SHA-256' }, key, signatureBytes(claim.signature), encoder.encode(canonicalClaimPayload(claim.payload)))) fail();
  // Reject invalid ECDH points before a claim becomes usable.
  await Promise.all([claim.payload.guestPublicKey, claim.payload.helperPublicKey].map(value => api.subtle.importKey('raw', publicKey(value), { name: 'ECDH', namedCurve: 'P-256' }, false, [])));
  return claim;
}
export async function acceptClaim(request, raw, service) {
  const row = validateRequest(request), claim = await verifyClaim(raw, service);
  if (claim.payload.id !== row.id || claim.payload.guestPublicKey !== row.guestPublicKey || claim.payload.expires !== row.expires) fail();
  return claim;
}
export async function deriveClaimConfig(payload, signature, privateKey, role, token, origin) {
  const claim = validateClaim({ payload, signature }), api = runtime();
  if (!['guest', 'controller'].includes(role) || !HEX64.test(token) || typeof origin !== 'string' || !ORIGIN.test(origin) || privateKey?.type !== 'private' || privateKey?.algorithm?.name !== 'ECDH' || privateKey.algorithm.namedCurve !== 'P-256') fail();
  const ownPublic = role === 'guest' ? claim.payload.guestPublicKey : claim.payload.helperPublicKey;
  const remotePublic = role === 'guest' ? claim.payload.helperPublicKey : claim.payload.guestPublicKey;
  const remote = await api.subtle.importKey('raw', publicKey(remotePublic), { name: 'ECDH', namedCurve: 'P-256' }, false, []);
  if (ownPublicKeys.get(privateKey) !== ownPublic) fail();
  const secret = new Uint8Array(await api.subtle.deriveBits({ name: 'ECDH', public: remote }, privateKey, 256));
  const binding = encoder.encode(JSON.stringify({ payload: claim.payload, signature: claim.signature }));
  const salt = await api.subtle.digest('SHA-256', binding);
  const material = await api.subtle.importKey('raw', secret, 'HKDF', false, ['deriveBits']); secret.fill(0);
  const key = new Uint8Array(await api.subtle.deriveBits({ name: 'HKDF', hash: 'SHA-256', salt, info: encoder.encode('pst-quickview-support-queue-v3:key') }, material, 256));
  const config = { v: 2, role, sid: claim.payload.sid, key: encodeQueueBytes(key), token, expires: claim.payload.expires, expectedExit: claim.payload.expectedExit, origin };
  key.fill(0); return config;
}

const HANDOFF_FIELDS = ['v', 'parentId', 'parentSid', 'nonce', 'targetVersion', 'artifactSha256', 'nativeSessionId', 'originalExpires', 'resumeDeadline'];
const SEMVER = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*))*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/;
export function validateUpdateHandoff(raw, { requireTicketHash = true, now = Date.now() } = {}) {
  const fields = requireTicketHash ? [...HANDOFF_FIELDS, 'resumeTicketSha256'] : HANDOFF_FIELDS;
  if (!exact(raw, fields) || raw.v !== 1 || !HEX32.test(raw.parentId) || !HEX32.test(raw.parentSid) || !HEX32.test(raw.nonce) || !HEX32.test(raw.nativeSessionId) ||
      typeof raw.targetVersion !== 'string' || raw.targetVersion.length > 128 || !SEMVER.test(raw.targetVersion) || !HEX64.test(raw.artifactSha256) ||
      !Number.isSafeInteger(now) || !Number.isSafeInteger(raw.originalExpires) || raw.originalExpires <= now || raw.originalExpires > now + 30 * 60 * 1000 ||
      !Number.isSafeInteger(raw.resumeDeadline) || raw.resumeDeadline <= now || raw.resumeDeadline > now + 90000 || raw.resumeDeadline > raw.originalExpires ||
      (requireTicketHash && !HEX64.test(raw.resumeTicketSha256))) fail();
  return Object.fromEntries(fields.map(key => [key, raw[key]]));
}
export function canonicalUpdateHandoff(raw) { return JSON.stringify(validateUpdateHandoff(raw)); }
function hexadecimal(bytes) { return Array.from(bytes, value => value.toString(16).padStart(2, '0')).join(''); }
export async function queueTokenSha256(token) {
  if (!HEX64.test(token)) fail();
  return hexadecimal(new Uint8Array(await runtime().subtle.digest('SHA-256', encoder.encode(token))));
}
async function updateProof(secret, value) {
  if (!HEX64.test(secret)) fail();
  const bytes = Uint8Array.from(secret.match(/../g), value => Number.parseInt(value, 16)), api = runtime();
  let key;
  try { key = await api.subtle.importKey('raw', bytes, { name: 'HMAC', hash: 'SHA-256' }, false, ['sign']); }
  finally { bytes.fill(0); }
  return hexadecimal(new Uint8Array(await api.subtle.sign('HMAC', key, encoder.encode(value))));
}
export async function ackProof(secret, handoff) {
  return updateProof(secret, 'pst-quickview-update-ack-v1\n' + canonicalUpdateHandoff(handoff));
}
export async function resumedGuestProof(secret, handoff, guestPublicKey, guestTokenSha256) {
  publicKey(guestPublicKey); if (!HEX64.test(guestTokenSha256)) fail();
  return updateProof(secret, 'pst-quickview-update-guest-v1\n' + canonicalUpdateHandoff(handoff) + '\n' + guestPublicKey + '\n' + guestTokenSha256);
}
export async function resumedClaimProof(secret, handoff, raw) {
  const grant = validateUpdateHandoff(handoff), claim = validateClaim(raw);
  if (claim.payload.id !== grant.parentId || claim.payload.expires !== grant.originalExpires || claim.payload.sid === grant.parentSid) fail();
  return updateProof(secret, 'pst-quickview-update-helper-v1\n' + JSON.stringify(grant) + '\n' + canonicalClaimPayload(claim.payload) + '\n' + claim.signature);
}
export function verifyUpdateProof(expected, actual) {
  if (typeof expected !== 'string' || typeof actual !== 'string' || !HEX64.test(expected) || !HEX64.test(actual)) return false;
  let difference = 0; for (let index = 0; index < 64; index++) difference |= expected.charCodeAt(index) ^ actual.charCodeAt(index);
  return difference === 0;
}
export async function createResumedRequest(handoff) {
  const grant = validateUpdateHandoff(handoff), result = await createRequest({ expires: grant.originalExpires });
  result.request.id = grant.parentId; return result;
}
