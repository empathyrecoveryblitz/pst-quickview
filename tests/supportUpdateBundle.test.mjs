import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
const read = path => readFileSync(new URL('../' + path, import.meta.url), 'utf8');

test('fixed-bundle preservation preflight is read-only and bounded', () => {
  const source = read('src-tauri/src/update_bundle.rs').split('#[cfg(test)]')[0];
  assert.doesNotMatch(source, /fs::(?:write|remove|rename|copy|set_permissions|create)|File::create|Command::new|OpenOptions/);
  assert.match(source, /MAX_ENTRIES: usize = 16_384/);
  assert.match(source, /MAX_DEPTH: usize = 32/);
  assert.match(source, /MAX_DURATION: Duration = Duration::from_secs\(2\)/);
  for (const name of ['MacOS', 'Contents', 'app', 'pst', 'eml', 'msg', 'metadata.json', 'import-state.json', 'local-state.control']) {
    assert.ok(source.includes('"' + name + '"'));
  }
  assert.match(source, /name\.starts_with\("\.pst-quickview"\)/);
  assert.match(source, /file_type\(\)\.is_symlink\(\)/);
  assert.match(source, /!metadata\.is_file\(\) && !metadata\.is_dir\(\)/);
  assert.match(source, /canonicalize\(\)[\s\S]*?starts_with\(root\)/);
  assert.match(source, /before\.dev\(\) == after\.dev\(\)/);
  assert.match(source, /before\.ino\(\) == after\.ino\(\)/);
});

function assertHook(source) {
  const install = source.indexOf('pending.update.install(&pending.bytes)');
  assert.ok(install > 0);
  const guard = source.lastIndexOf('let install_executable = tauri::process::current_binary(&app.env())', install);
  assert.ok(guard > 0);
  const beforeInstall = source.slice(guard, install);
  assert.match(beforeInstall, /crate::update_bundle::preflight\(&install_executable\)\.map_err\(/);
  assert.match(beforeInstall, /if resume && state\.revocation_epoch\.load\(Ordering::SeqCst\) != generation/);
  assert.ok(beforeInstall.indexOf('crate::update_bundle::preflight') < beforeInstall.indexOf('if resume &&'));
}

test('installer rechecks its fixed executable and revocation after preservation scan', () => {
  const source = read('src-tauri/src/updater.rs');
  assertHook(source);
  assert.throws(() => assertHook(source.replace('crate::update_bundle::preflight(&install_executable)', 'unrelated_preflight(&install_executable)')));
  assert.throws(() => assertHook(source.replace('if resume && state.revocation_epoch.load(Ordering::SeqCst) != generation', 'if false')));
  assert.match(read('src-tauri/src/lib.rs'), /mod update_bundle;/);
});
