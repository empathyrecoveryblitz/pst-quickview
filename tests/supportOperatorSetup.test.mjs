import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { webcrypto } from 'node:crypto';
import { initializeOperator } from '../scripts/setup-support-dashboard.mjs';

test('operator identity is private, never overwritten, and exports only its public pin', async t => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), 'pqv-operator-test-'));
  const directory = path.join(root, 'operator');
  t.after(() => fs.rm(root, { recursive: true }));
  const result = await initializeOperator(directory);
  assert.deepEqual(Object.keys(result).sort(), ['configFile', 'operatorPublicKey']);
  assert.equal((await fs.stat(directory)).mode & 0o777, 0o700);
  assert.equal((await fs.stat(result.configFile)).mode & 0o777, 0o600);
  const bytes = await fs.readFile(result.configFile);
  const config = JSON.parse(bytes);
  assert.match(config.adminToken, /^[a-f0-9]{64}$/);
  const key = await webcrypto.subtle.importKey('jwk', config.operatorSigningJwk, { name: 'ECDSA', namedCurve: 'P-256' }, true, ['sign']);
  const publicKey = await webcrypto.subtle.importKey('raw', Buffer.from(result.operatorPublicKey, 'base64url'), { name: 'ECDSA', namedCurve: 'P-256' }, false, ['verify']);
  const message = new TextEncoder().encode('synthetic identity check');
  const signature = await webcrypto.subtle.sign({ name: 'ECDSA', hash: 'SHA-256' }, key, message);
  assert.equal(await webcrypto.subtle.verify({ name: 'ECDSA', hash: 'SHA-256' }, publicKey, signature, message), true);
  await assert.rejects(initializeOperator(directory));
  assert.deepEqual(await fs.readFile(result.configFile), bytes);
});

test('operator initialization rejects permissive and symlinked directories', async t => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), 'pqv-operator-test-'));
  t.after(() => fs.rm(root, { recursive: true }));
  const unsafe = path.join(root, 'unsafe'), link = path.join(root, 'link');
  await fs.mkdir(unsafe, { mode: 0o755 }); await fs.chmod(unsafe, 0o755);
  await assert.rejects(initializeOperator(unsafe));
  await fs.symlink(unsafe, link);
  await assert.rejects(initializeOperator(link));
  await assert.rejects(initializeOperator('relative'));
});
