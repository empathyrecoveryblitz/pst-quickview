import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
const read = path => readFileSync(new URL('../' + path, import.meta.url), 'utf8');

test('independent artifact verifier pins the official crypto and cannot install or contact a service', () => {
  const cargo = read('scripts/support-update-verifier/Cargo.toml');
  const source = read('scripts/support-update-verifier/src/main.rs');
  assert.match(cargo, /minisign-verify = "=0\.3\.0"/);
  assert.match(cargo, /publish = false/);
  assert.doesNotMatch(cargo, /tauri\s*=|reqwest|tokio/);
  assert.doesNotMatch(source, /Command::new|TcpStream|UdpSocket|fs::write|File::create|PRIVATE_KEY|SIGNING_PRIVATE/);
  assert.match(source, /25 \* 1024 \* 1024/);
  assert.match(source, /deny_unknown_fields/);
  assert.match(source, /source_metadata_mismatch/);
  assert.match(source, /\.verify\(archive, &signature, true\)/);
  assert.ok(source.indexOf('.verify(archive, &signature, true)') < source.indexOf('verify_signed_version(signature.trusted_comment(), version)'));
  assert.match(source, /find_map\(\|field\| field\.strip_prefix\("version:"\)\)/);
  assert.match(source, /signed_version_missing_or_mismatched/);
  assert.match(source, /signature\.trusted_comment\(\)/);
  assert.match(source, /PASS: trusted_update_artifact_verified/);
});
