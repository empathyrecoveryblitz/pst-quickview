import test from 'node:test';
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { chmodSync, mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
const checker = fileURLToPath(new URL('../scripts/verify-packaged-support-pins.py', import.meta.url));
const source = readFileSync(new URL('../public/support/service.json', import.meta.url), 'utf8');
const nativeSource = readFileSync(new URL('../src-tauri/src/support_session.rs', import.meta.url), 'utf8');
const updateSource = readFileSync(new URL('../public/support/update-service.json', import.meta.url), 'utf8');
const tauriSource = readFileSync(new URL('../src-tauri/tauri.conf.json', import.meta.url), 'utf8');
const updaterSource = readFileSync(new URL('../src-tauri/src/updater.rs', import.meta.url), 'utf8');
const run = (binary, pin = source) => spawnSync('python3', ['-B', '-c', String.raw`
import importlib.util,json,sys
spec=importlib.util.spec_from_file_location("pins",sys.argv[1]);module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
value=json.load(sys.stdin)
try: module.verify_slice(value["binary"].encode(),value["pin"].encode())
except module.PinError as error: print(error);sys.exit(1)
`, checker], { input: JSON.stringify({ binary, pin }), encoding: 'utf8' });
const runBinding = (native) => spawnSync('python3', ['-B', '-c', String.raw`
import importlib.util,json,sys
spec=importlib.util.spec_from_file_location("pins",sys.argv[1]);module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
try: module.verify_native_binding(json.load(sys.stdin).encode())
except module.PinError as error: print(error);sys.exit(1)
`, checker], { input: JSON.stringify(native), encoding: 'utf8' });
test('packaged support pins fail closed on stale, unconfigured, or substituted configuration', () => {
  assert.equal(run('prefix' + source + 'suffix').status, 0);
  assert.equal(run('stale development binary').status, 1);
  const current = JSON.parse(source);
  for (const changed of [{ ...current, origin: '' }, { ...current, origin: current.origin + '/' },
    { ...current, operatorPublicKey: '' }, { ...current, operatorPublicKey: current.operatorPublicKey + '=' },
    { ...current, operatorLabel: '\n' }, { ...current, extra: true }]) {
    const pin = JSON.stringify(changed), result = run(pin, pin);
    assert.equal(result.status, 1); assert.equal(result.stdout.trim(), 'support_pin_unavailable');
  }
  assert.equal(run(source, JSON.stringify({ ...current, operatorLabel: 'Different reviewed helper' })).status, 1);
});
test('reviewed support configuration matches native duplicate, integer, control, and UTF-16 rules', () => {
  const current = JSON.parse(source);
  const validName = JSON.stringify({ ...current, operatorLabel: '\u{1f600}'.repeat(40) });
  assert.equal(run(validName, validName).status, 0);
  for (const pin of [
    source.replace(/"v"\s*:\s*3/, '"v":2,"v":3'),
    source.replace(/"v"\s*:\s*3/, '"v":3.0'),
    JSON.stringify({ ...current, operatorLabel: 'x\u0085y' }),
    JSON.stringify({ ...current, operatorLabel: 'x\u009fy' }),
    JSON.stringify({ ...current, operatorLabel: '\u{1f600}'.repeat(41) }),
    JSON.stringify({ ...current, operatorLabel: '\ud800' }),
  ]) {
    const result = run(pin, pin);
    assert.equal(result.status, 1);
    assert.equal(result.stdout.trim(), 'support_pin_unavailable');
    assert.equal(result.stderr, '');
  }
});
test('native trust pin is bound to the exact bundled configuration and typed parser', () => {
  assert.equal(runBinding(nativeSource).status, 0);
  assert.equal(runBinding(nativeSource.replace('fn service_config() -> SupportResult<ServiceConfig> {',
    'fn service_config ( )\n -> SupportResult < ServiceConfig > {\n /* harmless /* nested */ comment */')).status, 0);
  const changes = [
    nativeSource.replace('../../public/support/service.json', '../../public/support/other.json'),
    nativeSource.replace('parse_service_config(SERVICE_CONFIG)', 'parse_service_config("{}")'),
    nativeSource.replace('parse_service_config(SERVICE_CONFIG)', 'parse_service_config(OTHER_CONFIG)'),
    nativeSource.replace('const SERVICE_CONFIG:', 'const OTHER_CONFIG:'),
    nativeSource.replace('const SERVICE_CONFIG:', '// const SERVICE_CONFIG:'),
    nativeSource.replace('const SERVICE_CONFIG:', '/* const SERVICE_CONFIG:').replace('include_str!("../../public/support/service.json");', 'include_str!("../../public/support/service.json"); */'),
    nativeSource.replace('parse_service_config(SERVICE_CONFIG)', '/* parse_service_config(SERVICE_CONFIG) */ parse_service_config("{}")'),
    nativeSource + '\nconst SERVICE_CONFIG: &str = "unreviewed";\n',
  ];
  for (const changed of changes) {
    assert.notEqual(changed, nativeSource);
    const result = runBinding(changed);
    assert.equal(result.status, 1);
    assert.equal(result.stdout.trim(), 'support_source_binding_mismatch');
    assert.equal(result.stderr, '');
  }
});
test('CLI rejects a mismatched pin in either architecture without reporting paths or bytes', () => {
  const temporary = mkdtempSync(join(tmpdir(), 'pqv-support-pin-test-'));
  try {
    const bin = join(temporary, 'bin'); mkdirSync(bin);
    const lipo = join(bin, 'lipo'), app = join(temporary, 'synthetic-app');
    const calls = join(temporary, 'calls');
    writeFileSync(app, 'synthetic universal app');
    writeFileSync(lipo, String.raw`#!/usr/bin/env python3
import os,pathlib,sys
arch=sys.argv[sys.argv.index('-thin')+1]
with open(os.environ['PQV_PIN_TEST_CALLS'],'a') as output: output.write(arch+'\n')
pin=pathlib.Path(os.environ['PQV_PIN_TEST_SOURCE']).read_bytes()
pin+=pathlib.Path(os.environ['PQV_PIN_TEST_UPDATE_SOURCE']).read_bytes()
pathlib.Path(sys.argv[sys.argv.index('-output')+1]).write_bytes(b'synthetic stale slice' if arch==os.environ['PQV_PIN_TEST_BAD_ARCH'] else b'prefix'+pin+b'suffix')
`);
    chmodSync(lipo, 0o700);
    for (const badArch of ['', 'x86_64', 'arm64']) {
      writeFileSync(calls, '');
      const result = spawnSync('python3', ['-B', checker, fileURLToPath(new URL('../public/support/service.json', import.meta.url)), app], {
        encoding: 'utf8', env: { ...process.env, PATH: bin + ':' + process.env.PATH,
          PQV_PIN_TEST_CALLS: calls, PQV_PIN_TEST_SOURCE: fileURLToPath(new URL('../public/support/service.json', import.meta.url)),
          PQV_PIN_TEST_UPDATE_SOURCE: fileURLToPath(new URL('../public/support/update-service.json', import.meta.url)),
          PQV_PIN_TEST_BAD_ARCH: badArch },
      });
      assert.equal(result.status, badArch ? 1 : 0);
      if (badArch) {
        assert.equal(result.stderr.trim(), 'packaged_support_pin_mismatch');
        assert.equal(result.stdout, '');
      } else {
        assert.match(result.stdout, /^PASS: both packaged architectures/);
        assert.equal(result.stderr, '');
      }
      assert.deepEqual(readFileSync(calls, 'utf8').trim().split('\n'), badArch === 'x86_64' ? ['x86_64'] : ['x86_64', 'arm64']);
      assert.doesNotMatch(result.stdout + result.stderr, /synthetic-app|stale slice|operatorPublicKey/);
    }
  } finally { rmSync(temporary, { recursive: true, force: true }); }
});
test('Mac package gate checks current pins and tracks both queue bootstrap sources for freshness', () => {
  const gate = readFileSync(new URL('../scripts/verify-macos-release.sh', import.meta.url), 'utf8');
  assert.match(gate, /verify-packaged-support-pins\.py/);
  assert.match(gate, /SUPPORT_SERVICE_CONFIG/); assert.match(gate, /SUPPORT_QUEUE_PROTOCOL_SOURCE/);
  const result = spawnSync('python3', ['-B', checker, '/synthetic/nonexistent/private-pin', '/synthetic/nonexistent/app'], { encoding: 'utf8' });
  assert.equal(result.status, 1); assert.equal(result.stderr.trim(), 'support_pin_unavailable');
  assert.doesNotMatch(result.stderr, /nonexistent|private-pin/);
});
test('update package gate requires matching compiled pins, native verification and signed-version protection', () => {
  const inspect = (update = updateSource, tauri = tauriSource, native = updaterSource) => spawnSync('python3', ['-B', '-c', String.raw`
import importlib.util,json,sys
spec=importlib.util.spec_from_file_location("pins",sys.argv[1]);module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
value=json.load(sys.stdin)
try: module.reviewed_update_pin(value['update'].encode(),value['service'].encode(),value['tauri'].encode(),value['native'].encode())
except module.PinError as error: print(error);sys.exit(1)
`, checker], { input: JSON.stringify({ update, service: source, tauri, native }), encoding: 'utf8' });
  assert.equal(inspect().status, 0);
  const value = JSON.parse(updateSource), config = JSON.parse(tauriSource);
  for (const changed of [{ ...value, origin: 'https://other.example.workers.dev' }, { ...value, publicKey: '' }, { ...value, channel: 'unreviewed' }, { ...value, v: 1.5 }]) {
    assert.equal(inspect(JSON.stringify(changed)).status, 1);
  }
  for (const change of [{ requireSignedVersion: false }, { allowDowngrades: true }, { dangerousAcceptInvalidCerts: true }, { endpoints: ['https://other.example.workers.dev/updates/development.json'] }]) {
    const changed = structuredClone(config); Object.assign(changed.plugins.updater, change);
    assert.equal(inspect(updateSource, JSON.stringify(changed)).status, 1);
  }
  for (const native of [updaterSource.replace('../../public/support/update-service.json', '../../public/support/unreviewed.json'), updaterSource.replace('update.verify(&bytes).map_err(|_| fail())?;', '')]) {
    const result = inspect(updateSource, tauriSource, native); assert.equal(result.status, 1); assert.equal(result.stdout.trim(), 'update_pin_unavailable'); assert.equal(result.stderr, '');
  }
});
