#!/usr/bin/env node
// Local operator identity only. Private keys never enter an application bundle.
import fs from 'node:fs/promises';
import { constants } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { randomBytes, webcrypto } from 'node:crypto';

export async function initializeOperator(directory) {
  if (!path.isAbsolute(directory)) throw Error('private_directory_required');
  await fs.mkdir(directory, { recursive: true, mode: 0o700 });
  const stat = await fs.lstat(directory);
  if (!stat.isDirectory() || stat.isSymbolicLink() || (stat.mode & 0o077)
    || (process.getuid && stat.uid !== process.getuid())) throw Error('private_directory_required');
  const keys = await webcrypto.subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, true, ['sign', 'verify']);
  const operatorSigningJwk = await webcrypto.subtle.exportKey('jwk', keys.privateKey);
  const operatorPublicKey = Buffer.from(await webcrypto.subtle.exportKey('raw', keys.publicKey)).toString('base64url');
  const configFile = path.join(directory, 'operator.json');
  const handle = await fs.open(configFile, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW, 0o600);
  try {
    await handle.writeFile(JSON.stringify({ v: 3, adminToken: randomBytes(32).toString('hex'), operatorSigningJwk }) + '\n');
    await handle.sync();
  } finally { await handle.close(); }
  return { configFile, operatorPublicKey };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  if (process.argv.length !== 3 || process.argv[2] !== '--init') {
    process.stderr.write('Usage: node scripts/setup-support-dashboard.mjs --init\nCreates a new private operator identity; never replaces an existing identity.\n');
    process.exitCode = 1;
  } else {
    const base = process.env.XDG_CONFIG_HOME || path.join(os.homedir(), '.config');
    try {
      const result = await initializeOperator(path.join(base, 'pst-quickview-support'));
      process.stdout.write(JSON.stringify({ event: 'operator_identity_ready', ...result }) + '\n');
    } catch {
      process.stderr.write('Operator setup stopped. An existing identity or unsafe permissions require attention; nothing was replaced.\n');
      process.exitCode = 1;
    }
  }
}
