import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { createHash } from 'node:crypto';

const text = readFileSync(new URL('../LICENSES/DEVELOPMENT_RUNTIME_NOTICES.txt', import.meta.url), 'utf8');
const inventory = JSON.parse(text.split('BEGIN VERIFIED PACKAGE INVENTORY\n')[1].split('\nEND VERIFIED PACKAGE INVENTORY')[0]);
const sha256 = value => createHash('sha256').update(value).digest('hex');
const component = (name, version) => inventory.components.find(item => item.name === name && item.version === version);

test('updater notices retain whole integrity-bound texts and exact locked package provenance', () => {
  assert.equal(inventory.v, 1); assert.equal(inventory.scope, 'conservative-development-updater-runtime-supplement');
  assert.equal(inventory.components.length, 87);
  const lock = readFileSync(new URL('../src-tauri/Cargo.lock', import.meta.url), 'utf8').split('[[package]]');
  const npm = JSON.parse(readFileSync(new URL('../package-lock.json', import.meta.url), 'utf8')).packages['node_modules/@tauri-apps/api'];
  const seen = new Set();
  for (const item of inventory.components) {
    const id = item.ecosystem + ':' + item.name + '@' + item.version;
    assert.equal(seen.has(id), false); seen.add(id); assert.equal(typeof item.declaredLicense, 'string');
    if (item.ecosystem === 'cargo') {
      const entry = lock.find(value => value.includes('\nname = ' + JSON.stringify(item.name) + '\n') && value.includes('\nversion = ' + JSON.stringify(item.version) + '\n'));
      assert.ok(entry, id); assert.ok(entry.includes('\nchecksum = ' + JSON.stringify(item.sha256) + '\n'), id);
    } else {
      assert.equal(item.name, '@tauri-apps/api'); assert.equal(item.version, npm.version);
      assert.equal(Buffer.from(item.sha512, 'hex').toString('base64'), npm.integrity.slice('sha512-'.length));
    }
    for (const file of item.files) {
      assert.match(file.sha256, /^[a-f0-9]{64}$/);
      const start = '\n===== BEGIN NOTICE SHA256 ' + file.sha256 + ' =====\n';
      const end = '\n===== END NOTICE SHA256 ' + file.sha256 + ' =====\n';
      assert.equal(text.split(start).length, 2); assert.equal(text.split(end).length, 2);
      const value = text.split(start)[1].split(end)[0];
      assert.equal(sha256(value), file.sha256); assert.equal(Buffer.byteLength(value), file.bytes);
    }
  }
  assert.doesNotMatch(text, /\/home\/|\/Users\/|file:\/\/|PRIVATE KEY|Bearer [a-f0-9]{64}/);
});

test('new crypto and updater dependencies preserve their nested copyright and license notices', () => {
  for (const [name, version, required] of [
    ['tauri-plugin-updater', '2.14.0', ['LICENSE_MIT', 'LICENSE_APACHE-2.0']],
    ['minisign-verify', '0.3.0', ['LICENSE']],
    ['ring', '0.17.14', ['LICENSE', 'LICENSE-BoringSSL', 'LICENSE-other-bits', 'src/polyfill/once_cell/LICENSE-APACHE', 'src/polyfill/once_cell/LICENSE-MIT']],
    ['reqwest', '0.13.5', ['LICENSE-MIT', 'LICENSE-APACHE']],
    ['hmac', '0.12.1', ['LICENSE-MIT', 'LICENSE-APACHE']],
    ['security-framework', '3.7.0', ['LICENSE-MIT', 'LICENSE-APACHE']],
    ['@tauri-apps/api', '2.12.4', ['LICENSE-MIT', 'LICENSE-APACHE-2.0']],
  ]) {
    const item = component(name, version); assert.ok(item);
    for (const file of required) assert.ok(item.files.some(entry => entry.path === file), name + '/' + file);
  }
  assert.match(text, /Copyright \(c\) 2019-2025 Frank Denis/);
  assert.match(text, /Copyright \(c\) 2006-2009 Graydon Hoare/);
  assert.match(text, /Copyright \(c\) 2009-2013 Mozilla Foundation/);
});

test('native packaging includes and byte-compares the scoped runtime notices supplement', () => {
  const configuration = JSON.parse(readFileSync(new URL('../src-tauri/tauri.conf.json', import.meta.url), 'utf8'));
  assert.equal(configuration.bundle.resources['../LICENSES/DEVELOPMENT_RUNTIME_NOTICES.txt'], 'LICENSES/DEVELOPMENT_RUNTIME_NOTICES.txt');
  const gate = readFileSync(new URL('../scripts/verify-macos-release.sh', import.meta.url), 'utf8');
  assert.match(gate, /check_packaged_resource "LICENSES\/DEVELOPMENT_RUNTIME_NOTICES\.txt"/);
  assert.match(gate, /"\.\.\/LICENSES\/DEVELOPMENT_RUNTIME_NOTICES\.txt": "LICENSES\/DEVELOPMENT_RUNTIME_NOTICES\.txt"/);
  assert.match(readFileSync(new URL('../THIRD_PARTY_NOTICES.md', import.meta.url), 'utf8'), /LICENSES\/DEVELOPMENT_RUNTIME_NOTICES\.txt/);
});
