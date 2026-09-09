import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
const backend = readFileSync(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8");
const loggingPolicy = readFileSync(new URL("../docs/LOGGING.md", import.meta.url), "utf8");

test("ordinary logs omit private paths and backend error text", () => {
  const importLogStart = backend.indexOf("fn initialize_import_log");
  const processOutputStart = backend.indexOf("struct BoundedProcessOutput");
  const logging = backend.slice(importLogStart, processOutputStart);

  assert.match(logging, /state=log_initialized/);
  assert.doesNotMatch(logging, /source=local_read_only|helper=bundled_readpst|readpst_version=/);
  assert.doesNotMatch(logging, /Running bundled ReadPST with isolated output/);
  assert.doesNotMatch(logging, /original_pst_path|pst_content_fingerprint|workspace_path/);
  assert.match(logging, /if error\.is_some\(\) \{ "present" \} else \{ "-" \}/);
  assert.doesNotMatch(backend, /(?:Export|EML export|Open attachment)[^\n]*error=\{\}/);
});

test("log files are bounded private regular files", () => {
  assert.match(backend, /custom_flags\(libc::O_CLOEXEC \| libc::O_NOFOLLOW\)/);
  assert.match(backend, /metadata\.nlink\(\) != 1/);
  assert.match(backend, /PRIVATE_DIRECTORY_MODE: u32 = 0o700/);
  assert.match(backend, /PRIVATE_FILE_MODE: u32 = 0o600/);
  assert.match(loggingPolicy, /rotate(?:s|d)? at 1 MiB/);
  assert.match(loggingPolicy, /rotate(?:s|d)? at 10 MiB/);
  assert.match(loggingPolicy, /rotate(?:s|d)? at 2 MiB/);
});

test("application logging starts only after immutable profile resolution", () => {
  const runStart = backend.indexOf("pub fn run()");
  const firstCommand = backend.indexOf("#[tauri::command]", runStart);
  const run = backend.slice(runStart, firstCommand);
  const setup = run.indexOf(".setup(move |app|");
  const resolution = run.indexOf("RuntimeProfile::resolve(app, &expected_identity)", setup);
  const initialization = run.indexOf("initialize_runtime_profile(profile)", resolution);
  const fileLogging = run.indexOf("append_application_log(profile.paths()", initialization);

  assert.ok(setup >= 0);
  assert.ok(setup < resolution && resolution < initialization && initialization < fileLogging);
  assert.doesNotMatch(run.slice(0, setup), /append_application_log\s*\(/);
  assert.doesNotMatch(run, /fallback[^\n]*application\.log/i);
});

test("diagnostic copying is explicit and warns before sharing", () => {
  assert.match(app, /Diagnostics can contain local file paths\. Review them before sharing\./);
  assert.match(app, /onClick=\{\(\) => void copyAppDiagnostics\(\)\}/);
  assert.doesNotMatch(app, /console\.error\([^\n]*,\s*(?:err|error)\)/);
});
