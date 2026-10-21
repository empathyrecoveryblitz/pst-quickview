import test from 'node:test';
import assert from 'node:assert/strict';
import { createCipher } from '../public/support/protocol.js';
import { createRequest, createEphemeralKeyPair, createClaim, acceptClaim, verifyClaim, deriveClaimConfig,
  canonicalClaimPayload, validateServiceConfig, validateRequest, validateClaim, encodeQueueBytes, decodeQueueBytes } from '../public/support/queue-protocol.js';

async function fixture() {
  const operator = await crypto.subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, true, ['sign', 'verify']);
  const service = { v: 3, origin: 'https://synthetic.example.workers.dev', operatorPublicKey: encodeQueueBytes(new Uint8Array(await crypto.subtle.exportKey('raw', operator.publicKey))), operatorLabel: 'Synthetic support' };
  const guest = await createRequest({ expires: Date.now() + 60000 }), helper = await createEphemeralKeyPair();
  const claim = await createClaim(guest.request, helper.publicKey, '8.8.8.8', operator.privateKey);
  return { operator, service, guest, helper, claim };
}

test('queue requests use fresh non-extractable ECDH keys and exact configured trust pins', async () => {
  const { service, guest } = await fixture();
  assert.match(guest.request.id, /^[a-f0-9]{32}$/); assert.match(guest.request.token, /^[a-f0-9]{64}$/);
  assert.equal(decodeQueueBytes(guest.request.guestPublicKey, 65)[0], 4);
  await assert.rejects(crypto.subtle.exportKey('jwk', guest.privateKey));
  const other = await createRequest({ expires: guest.request.expires });
  assert.notEqual(other.request.id, guest.request.id); assert.notEqual(other.request.token, guest.request.token);
  assert.deepEqual(validateServiceConfig(service), service);
  for (const origin of ['', 'https://example.workers.dev', 'https://a.b.c.workers.dev', 'http://a.b.workers.dev', 'https://a.b.workers.dev/', 'https://a.b.workers.dev:443', 'https://user@a.b.workers.dev', 'https://a.b.workers.dev.example', 'https://-a.b.workers.dev']) {
    assert.throws(() => validateServiceConfig({ ...service, origin }));
  }
  assert.throws(() => validateServiceConfig({ ...service, extra: true }));
  assert.throws(() => validateServiceConfig({ ...service, operatorLabel: 'unsafe\nlabel' }));
  assert.throws(() => validateRequest({ ...guest.request, extra: 'not metadata' }));
  await assert.rejects(createRequest({ expires: Date.now() + 31 * 60 * 1000 }));
});

test('signed claims canonicalize key order and derive matching direction-bound setup keys', async () => {
  const { service, guest, helper, claim } = await fixture();
  const accepted = await acceptClaim(guest.request, claim, service);
  assert.deepEqual(accepted, claim);
  const reordered = Object.fromEntries(Object.entries(claim.payload).reverse());
  assert.equal(canonicalClaimPayload(reordered), JSON.stringify(claim.payload));
  const row = { v: 3, id: guest.request.id, guestPublicKey: guest.request.guestPublicKey, expires: guest.request.expires, status: 'waiting' };
  assert.equal(validateRequest(row).id, guest.request.id);
  const guestConfig = await deriveClaimConfig(accepted.payload, accepted.signature, guest.privateKey, 'guest', guest.request.token, service.origin);
  const helperConfig = await deriveClaimConfig(accepted.payload, accepted.signature, helper.privateKey, 'controller', 'a'.repeat(64), service.origin);
  assert.equal(guestConfig.key, helperConfig.key); assert.equal(guestConfig.v, 2); assert.equal(guestConfig.sid, claim.payload.sid);
  assert.equal(JSON.stringify(claim).includes(guestConfig.key), false);
  const sender = await createCipher(guestConfig), receiver = await createCipher(helperConfig);
  assert.deepEqual(await receiver.open(await sender.seal('approved', null)), { kind: 'approved', data: null });
  const nextClaim = await createClaim(row, helper.publicKey, '8.8.8.8', (await fixture()).operator.privateKey);
  const nextConfig = await deriveClaimConfig(nextClaim.payload, nextClaim.signature, guest.privateKey, 'guest', guest.request.token, service.origin);
  assert.notEqual(nextConfig.key, guestConfig.key);
});

test('claim tampering, wrong pins, request replay, stale expiry and mismatched private keys fail closed', async () => {
  const { service, guest, helper, claim } = await fixture(), other = await fixture();
  for (const changed of [
    { ...claim, signature: claim.signature.slice(0, -1) + (claim.signature.endsWith('A') ? 'Q' : 'A') },
    { ...claim, payload: { ...claim.payload, id: 'f'.repeat(32) } },
    { ...claim, payload: { ...claim.payload, expectedExit: '1.1.1.1' } },
    { ...claim, payload: { ...claim.payload, helperPublicKey: other.helper.publicKey } },
    { ...claim, payload: { ...claim.payload, expires: Date.now() - 1 } },
    { ...claim, payload: { ...claim.payload, extra: 'not allowed' } },
    { ...claim, extra: true },
  ]) await assert.rejects(acceptClaim(guest.request, changed, service));
  await assert.rejects(verifyClaim(claim, { ...service, operatorPublicKey: other.service.operatorPublicKey }));
  await assert.rejects(acceptClaim(other.guest.request, claim, service));
  await assert.rejects(deriveClaimConfig(claim.payload, claim.signature, other.guest.privateKey, 'guest', guest.request.token, service.origin));
  await assert.rejects(deriveClaimConfig(claim.payload, claim.signature, helper.privateKey, 'guest', guest.request.token, service.origin));
  await assert.rejects(deriveClaimConfig(claim.payload, claim.signature, helper.privateKey, 'controller', 'not-token', service.origin));
  await assert.rejects(deriveClaimConfig(claim.payload, claim.signature, helper.privateKey, 'controller', 'a'.repeat(64), 'https://untrusted.invalid'));
});

test('noncanonical high-S signatures and accessor-bearing metadata are rejected', async () => {
  const { guest, claim } = await fixture();
  const bytes = decodeQueueBytes(claim.signature, 64), order = BigInt('0xffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551');
  const s = BigInt('0x' + Buffer.from(bytes.subarray(32)).toString('hex'));
  bytes.set(Buffer.from((order - s).toString(16).padStart(64, '0'), 'hex'), 32);
  assert.throws(() => validateClaim({ ...claim, signature: encodeQueueBytes(bytes) }));
  const accessor = { ...guest.request }; Object.defineProperty(accessor, 'id', { get: () => guest.request.id, enumerable: true });
  assert.throws(() => validateRequest(accessor));
});
