import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';

const read = path => readFileSync(new URL('../' + path, import.meta.url), 'utf8');

test('Keychain and executable-hash IPC never dispatch synchronously on the Mac UI thread', () => {
  const updater = read('src-tauri/src/updater.rs');
  const support = read('src-tauri/src/support_session.rs');
  for (const command of ['complete_support_update_handoff', 'support_resume_status', 'resume_support_after_update']) {
    assert.match(updater, new RegExp(`#\\[tauri::command\\(async\\)\\]\\s+pub\\(crate\\) fn ${command}\\(`), command);
  }
  assert.match(support, /#\[tauri::command\(async\)\]\s+pub\(crate\) fn stop_support_session\(/);
});

test('window lifecycle cleanup dispatches Keychain deletion only after immediate revocation', () => {
  const updater = read('src-tauri/src/updater.rs');
  const support = read('src-tauri/src/support_session.rs');
  const cleanup = updater.match(/pub\(crate\) fn cleanup_revoked_resume\(\) \{([\s\S]*?)\n\}/)?.[1];
  assert.ok(cleanup);
  assert.match(cleanup, /std::thread::spawn\(\|\| \{\s+let _ = delete_grant\(\);\s+\}\);/);
  const stop = support.slice(support.indexOf('fn stop_matching('), support.indexOf('pub(crate) fn handle_run_event('));
  assert.ok(stop.indexOf('revoke_resume_authority') < stop.indexOf('take_session'));
  assert.ok(stop.indexOf('window.eval(REVOKE_EVENT)') < stop.indexOf('cleanup_revoked_resume'));
});
