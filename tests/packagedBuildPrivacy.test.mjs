import test from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const tauriWrapperUrl = new URL("../scripts/tauri.sh", import.meta.url);
const releaseVerifierUrl = new URL(
  "../scripts/verify-macos-release.sh",
  import.meta.url,
);
const releaseVerifier = readFileSync(releaseVerifierUrl, "utf8");
const encodedSeparator = "\x1f";

function createFakeRepository(t) {
  const temporaryRoot = mkdtempSync(join(tmpdir(), "pstq-build-privacy-"));
  t.after(() => rmSync(temporaryRoot, { recursive: true, force: true }));

  const repository = join(temporaryRoot, "repository source");
  const scriptsDirectory = join(repository, "scripts");
  const tauriDirectory = join(repository, "node_modules", ".bin");
  const syntheticHome = join(temporaryRoot, "private home");
  const syntheticCargoHome = join(syntheticHome, "cargo registry");
  mkdirSync(scriptsDirectory, { recursive: true });
  mkdirSync(tauriDirectory, { recursive: true });
  mkdirSync(syntheticCargoHome, { recursive: true });
  copyFileSync(fileURLToPath(tauriWrapperUrl), join(scriptsDirectory, "tauri.sh"));

  const fakeTauri = join(tauriDirectory, "tauri");
  writeFileSync(
    fakeTauri,
    `#!/usr/bin/env node
const { writeFileSync } = require("node:fs");
writeFileSync(process.env.PSTQ_CAPTURE_PATH, JSON.stringify({
  args: process.argv.slice(2),
  encoded: process.env.CARGO_ENCODED_RUSTFLAGS ?? null,
  rustflags: process.env.RUSTFLAGS ?? null,
}));
`,
  );
  chmodSync(fakeTauri, 0o755);

  return {
    repository: realpathSync(repository),
    syntheticHome: realpathSync(syntheticHome),
    syntheticCargoHome: realpathSync(syntheticCargoHome),
    temporaryRoot,
  };
}

function invokeWrapper(fixture, captureName, args, overrides) {
  const capturePath = join(fixture.temporaryRoot, captureName);
  const environment = { ...process.env };
  delete environment.CARGO_ENCODED_RUSTFLAGS;
  delete environment.RUSTFLAGS;
  Object.assign(environment, {
    HOME: fixture.syntheticHome,
    CARGO_HOME: fixture.syntheticCargoHome,
    PSTQ_CAPTURE_PATH: capturePath,
    ...overrides,
  });
  const result = spawnSync(
    "bash",
    [join(fixture.repository, "scripts", "tauri.sh"), ...args],
    { cwd: fixture.repository, env: environment, encoding: "utf8" },
  );
  assert.equal(result.status, 0, result.stderr);
  return JSON.parse(readFileSync(capturePath, "utf8"));
}

function expectedRemaps(fixture) {
  return [
    `--remap-path-prefix=${fixture.syntheticHome}=/build/home`,
    `--remap-path-prefix=${fixture.syntheticCargoHome}=/build/cargo`,
    `--remap-path-prefix=${fixture.repository}=/build/source`,
  ];
}

test("packaged builds preserve flags and remap private source prefixes", (t) => {
  const fixture = createFakeRepository(t);
  const existingFlags = ["-Cdebuginfo=1", "--cfg=existing_flag"];
  const capture = invokeWrapper(
    fixture,
    "encoded-flags.json",
    ["build", "--target", "universal-apple-darwin", "--bundles", "app"],
    {
      CARGO_ENCODED_RUSTFLAGS: existingFlags.join(encodedSeparator),
      RUSTFLAGS: "--cfg ignored_when_encoded_flags_are_present",
    },
  );

  assert.deepEqual(capture.args, [
    "build",
    "--target",
    "universal-apple-darwin",
    "--bundles",
    "app",
  ]);
  assert.equal(capture.rustflags, null);
  const encodedFlags = capture.encoded.split(encodedSeparator);
  assert.deepEqual(encodedFlags, [...existingFlags, ...expectedRemaps(fixture)]);
  for (const remap of expectedRemaps(fixture)) {
    assert.equal(encodedFlags.filter((flag) => flag === remap).length, 1);
    assert.match(remap, /=\/build\/(home|cargo|source)$/);
  }
});

test("packaged builds preserve legacy RUSTFLAGS whitespace semantics", (t) => {
  const fixture = createFakeRepository(t);
  const capture = invokeWrapper(
    fixture,
    "legacy-flags.json",
    ["build", "--target", "universal-apple-darwin"],
    { RUSTFLAGS: "--cfg legacy_flag\n-C debuginfo=1\t--cfg tabbed_flag" },
  );

  assert.equal(capture.rustflags, null);
  assert.deepEqual(capture.encoded.split(encodedSeparator), [
    "--cfg",
    "legacy_flag",
    "-C",
    "debuginfo=1",
    "--cfg",
    "tabbed_flag",
    ...expectedRemaps(fixture),
  ]);
});

test("an explicitly empty encoded flag list still suppresses legacy RUSTFLAGS", (t) => {
  const fixture = createFakeRepository(t);
  const capture = invokeWrapper(
    fixture,
    "empty-encoded-flags.json",
    ["build", "--target", "universal-apple-darwin"],
    {
      CARGO_ENCODED_RUSTFLAGS: "",
      RUSTFLAGS: "--cfg must_not_be_imported",
    },
  );

  assert.equal(capture.rustflags, null);
  assert.deepEqual(capture.encoded.split(encodedSeparator), expectedRemaps(fixture));
});

test("packaged builds reject filesystem-root remap prefixes", (t) => {
  const fixture = createFakeRepository(t);
  const wrapper = join(fixture.repository, "scripts", "tauri.sh");
  const baseEnvironment = {
    ...process.env,
    PSTQ_CAPTURE_PATH: join(fixture.temporaryRoot, "must-not-run.json"),
  };

  const rootHome = spawnSync("bash", [wrapper, "build"], {
    cwd: fixture.repository,
    env: {
      ...baseEnvironment,
      HOME: "/",
      CARGO_HOME: fixture.syntheticCargoHome,
    },
    encoding: "utf8",
  });
  assert.notEqual(rootHome.status, 0);
  assert.match(rootHome.stderr, /HOME directory is required/);

  const rootCargoHome = spawnSync("bash", [wrapper, "build"], {
    cwd: fixture.repository,
    env: {
      ...baseEnvironment,
      HOME: fixture.syntheticHome,
      CARGO_HOME: "/",
    },
    encoding: "utf8",
  });
  assert.notEqual(rootCargoHome.status, 0);
  assert.match(rootCargoHome.stderr, /filesystem root cannot be used as CARGO_HOME/);
});

test("development invocations remain outside the packaged-build remapping", (t) => {
  const fixture = createFakeRepository(t);
  const encoded = "--cfg=dev_flag";
  const rustflags = "--cfg dev_rustflag";
  const capture = invokeWrapper(fixture, "dev-flags.json", ["dev", "--port", "1420"], {
    CARGO_ENCODED_RUSTFLAGS: encoded,
    RUSTFLAGS: rustflags,
  });

  assert.deepEqual(capture.args, [
    "dev",
    "--config",
    `${fixture.repository}/src-tauri/tauri.dev.conf.json`,
    "--port",
    "1420",
  ]);
  assert.equal(capture.encoded, encoded);
  assert.equal(capture.rustflags, rustflags);
});

test("release verification fails closed on private paths in every packaged slice", () => {
  const scanSection = releaseVerifier.slice(
    releaseVerifier.indexOf("check_no_private_build_paths()"),
    releaseVerifier.indexOf("deployment_target_for_arch()"),
  );
  const pattern = scanSection.match(/grep -E '([^']+)'/)?.[1];
  assert.ok(pattern, "private build-path pattern must remain explicit and testable");

  for (const blocked of [
    "/Users/example/source/file.rs",
    "file:///Users/example/source/file.rs",
    "/home/example/source/file.rs",
    "/Volumes/private/source/file.rs",
    String.raw`C:\Users\example\source\file.rs`,
  ]) {
    assert.equal(spawnSync("grep", ["-E", pattern], { input: blocked }).status, 0);
  }
  for (const allowed of [
    "/build/home/registry/file.rs",
    "/build/cargo/registry/file.rs",
    "/build/source/src/lib.rs",
    "/System/Library/Frameworks/WebKit.framework/WebKit",
  ]) {
    assert.equal(spawnSync("grep", ["-E", pattern], { input: allowed }).status, 1);
  }

  assert.match(releaseVerifier, /for packaged_arch in x86_64 arm64/);
  assert.match(
    releaseVerifier,
    /check_no_private_build_paths "\$\{APP_BINARY\}" "\$\{packaged_arch\}"/,
  );
  assert.match(
    releaseVerifier,
    /check_no_private_build_paths "\$\{PACKAGED_READPST\}" "\$\{packaged_arch\}"/,
  );
  assert.match(releaseVerifier, /"\$\{TAURI_BUILD_WRAPPER\}"/);
  assert.doesNotMatch(scanSection, /printf[^\n]*binary/);
});

test("slice privacy scans remain fail-closed under shell errexit", (t) => {
  const temporaryRoot = mkdtempSync(join(tmpdir(), "pstq-slice-privacy-"));
  t.after(() => rmSync(temporaryRoot, { recursive: true, force: true }));
  const syntheticBinary = join(temporaryRoot, "synthetic-binary");
  writeFileSync(syntheticBinary, "synthetic");
  const scanFunction = releaseVerifier.slice(
    releaseVerifier.indexOf("check_no_private_build_paths()"),
    releaseVerifier.indexOf("deployment_target_for_arch()"),
  );
  const harness = String.raw`
set -euo pipefail
strings() {
  case "$PSTQ_STRINGS_MODE" in
    clean) printf '%s\n' '/build/source/src/lib.rs' ;;
    private) printf '%s\n' '/Users/example/source/lib.rs' ;;
    error) return 7 ;;
  esac
}
pass() { printf 'PASS:%s\n' "$1"; }
fail() { printf 'FAIL:%s\n' "$1"; }
${scanFunction}
check_no_private_build_paths "$1" x86_64 'synthetic slice'
printf 'REACHED\n'
`;

  const run = (mode) =>
    spawnSync("bash", ["-c", harness, "bash", syntheticBinary], {
      env: { ...process.env, PSTQ_STRINGS_MODE: mode },
      encoding: "utf8",
    });
  const clean = run("clean");
  assert.equal(clean.status, 0, clean.stderr);
  assert.match(clean.stdout, /PASS:synthetic slice contains no private absolute build paths/);
  assert.match(clean.stdout, /REACHED/);

  const privateResult = run("private");
  assert.equal(privateResult.status, 0, privateResult.stderr);
  assert.match(privateResult.stdout, /FAIL:synthetic slice contains a private absolute build path/);
  assert.doesNotMatch(privateResult.stdout, /Users\/example/);
  assert.match(privateResult.stdout, /REACHED/);

  const toolError = run("error");
  assert.equal(toolError.status, 0, toolError.stderr);
  assert.match(toolError.stdout, /FAIL:synthetic slice could not be inspected/);
  assert.match(toolError.stdout, /REACHED/);
});

test("alternate runtime-profile scans reject production destinations and inspection errors", (t) => {
  const temporaryRoot = mkdtempSync(join(tmpdir(), "pstq-runtime-profile-scan-"));
  t.after(() => rmSync(temporaryRoot, { recursive: true, force: true }));
  const syntheticBinary = join(temporaryRoot, "synthetic-binary");
  writeFileSync(syntheticBinary, "synthetic");
  const scanFunction = releaseVerifier.slice(
    releaseVerifier.indexOf("check_runtime_profile_contract()"),
    releaseVerifier.indexOf("deployment_target_for_arch()"),
  );
  const harness = String.raw`
set -euo pipefail
PRODUCTION_IDENTIFIER='com.local.pstquickview'
RUNTIME_PROFILE_CONTRACT='pst-quickview-runtime-profile-v1:alternate-identifier-scoped:no-production-fallback'
RUNTIME_PROFILE_IDENTITY_CONTRACT_PREFIX='pst-quickview-runtime-identity-v1:'
strings() {
  case "$PSTQ_STRINGS_MODE" in
    clean)
      printf '%s\n' "$RUNTIME_PROFILE_CONTRACT" 'pst-quickview-runtime-identity-v1:com_local_pstquickview_beta4runtimetest:static:identity-end' '/build/source/src/lib.rs'
      ;;
    legacy)
      printf '%s\n' "$RUNTIME_PROFILE_CONTRACT" 'pst-quickview-runtime-identity-v1:com_local_pstquickview_beta4runtimetest:static:identity-end' 'Library/Application Support/PST QuickView/workspaces/'
      ;;
    legacy-root)
      printf '%s\n' "$RUNTIME_PROFILE_CONTRACT" 'pst-quickview-runtime-identity-v1:com_local_pstquickview_beta4runtimetest:static:identity-end' 'Library/Application Support/PST QuickView/'
      ;;
    legacy-coalesced)
      printf '%s\n' "$RUNTIME_PROFILE_CONTRACT" 'pst-quickview-runtime-identity-v1:com_local_pstquickview_beta4runtimetest:static:identity-end' 'Library/Application Support/PST QuickViewadjacent-linker-bytes'
      ;;
    missing-contract)
      printf '%s\n' 'pst-quickview-runtime-identity-v1:com_local_pstquickview_beta4runtimetest:static:identity-end' '/build/source/src/lib.rs'
      ;;
    missing-identity)
      printf '%s\n' "$RUNTIME_PROFILE_CONTRACT" '/build/source/src/lib.rs'
      ;;
    wrong-identity)
      printf '%s\n' "$RUNTIME_PROFILE_CONTRACT" 'pst-quickview-runtime-identity-v1:com_local:pstquickview:identity-end' '/build/source/src/lib.rs'
      ;;
    extended-identity)
      printf '%s\n' "$RUNTIME_PROFILE_CONTRACT" 'pst-quickview-runtime-identity-v1:com_local_pstquickview_beta4runtimetest:staticextra:identity-end' '/build/source/src/lib.rs'
      ;;
    error) return 7 ;;
  esac
}
pass() { printf 'PASS:%s\n' "$1"; }
fail() { printf 'FAIL:%s\n' "$1"; }
${scanFunction}
check_runtime_profile_contract "$1" x86_64 "$PSTQ_EXPECTED_IDENTIFIER" 'synthetic alternate slice'
printf 'REACHED\n'
`;
  const expectedIdentifier = "com.local.pstquickview.beta4runtimetest.static";
  const run = (mode) =>
    spawnSync("bash", ["-c", harness, "bash", syntheticBinary], {
      env: {
        ...process.env,
        PSTQ_STRINGS_MODE: mode,
        PSTQ_EXPECTED_IDENTIFIER: expectedIdentifier,
      },
      encoding: "utf8",
    });

  const clean = run("clean");
  assert.equal(clean.status, 0, clean.stderr);
  assert.match(clean.stdout, /PASS:synthetic alternate slice compiled runtime identity matches the packaged identifier/);
  assert.match(clean.stdout, /PASS:synthetic alternate slice embeds the identifier-scoped runtime-profile contract/);
  assert.match(clean.stdout, /PASS:synthetic alternate slice contains no usable production-profile destination/);
  assert.match(clean.stdout, /REACHED/);

  const legacy = run("legacy");
  assert.equal(legacy.status, 0, legacy.stderr);
  assert.match(legacy.stdout, /FAIL:synthetic alternate slice contains a usable production-profile destination/);
  assert.doesNotMatch(legacy.stdout, /Application Support\/PST QuickView/);

  const legacyRoot = run("legacy-root");
  assert.equal(legacyRoot.status, 0, legacyRoot.stderr);
  assert.match(legacyRoot.stdout, /FAIL:synthetic alternate slice contains a usable production-profile destination/);

  const legacyCoalesced = run("legacy-coalesced");
  assert.equal(legacyCoalesced.status, 0, legacyCoalesced.stderr);
  assert.match(legacyCoalesced.stdout, /FAIL:synthetic alternate slice contains a usable production-profile destination/);

  const missingContract = run("missing-contract");
  assert.equal(missingContract.status, 0, missingContract.stderr);
  assert.match(missingContract.stdout, /FAIL:synthetic alternate slice does not embed the identifier-scoped runtime-profile contract/);

  const missingIdentity = run("missing-identity");
  assert.equal(missingIdentity.status, 0, missingIdentity.stderr);
  assert.match(missingIdentity.stdout, /FAIL:synthetic alternate slice compiled runtime identity does not match the packaged identifier/);

  const wrongIdentity = run("wrong-identity");
  assert.equal(wrongIdentity.status, 0, wrongIdentity.stderr);
  assert.match(wrongIdentity.stdout, /FAIL:synthetic alternate slice compiled runtime identity does not match the packaged identifier/);

  const extendedIdentity = run("extended-identity");
  assert.equal(extendedIdentity.status, 0, extendedIdentity.stderr);
  assert.match(extendedIdentity.stdout, /FAIL:synthetic alternate slice compiled runtime identity does not match the packaged identifier/);

  const error = run("error");
  assert.equal(error.status, 0, error.stderr);
  assert.match(error.stdout, /FAIL:synthetic alternate slice could not be inspected/);
  assert.match(error.stdout, /REACHED/);
});

test("recursive packaged privacy scan distinguishes matches, cleanliness, and errors", () => {
  const marker = releaseVerifier.indexOf(
    "PST_QUICKVIEW_(TREVOR|FURMAN)_MSG_FIXTURE",
  );
  const blockStart = releaseVerifier.lastIndexOf("  if grep -R -a -E", marker);
  const blockEnd = releaseVerifier.indexOf(
    '  if [[ -f "${APP_BINARY}" ]]',
    marker,
  );
  assert.notEqual(blockStart, -1);
  assert.notEqual(blockEnd, -1);
  const scanBlock = releaseVerifier.slice(blockStart, blockEnd);
  const harness = String.raw`
set -euo pipefail
grep() { return "$PSTQ_GREP_STATUS"; }
pass() { printf 'PASS:%s\n' "$1"; }
fail() { printf 'FAIL:%s\n' "$1"; }
APP_PATH=/synthetic/package
${scanBlock}
printf 'REACHED\n'
`;
  const run = (status) =>
    spawnSync("bash", ["-c", harness], {
      env: { ...process.env, PSTQ_GREP_STATUS: String(status) },
      encoding: "utf8",
    });

  const match = run(0);
  assert.equal(match.status, 0, match.stderr);
  assert.match(match.stdout, /FAIL:packaged app contains a private path/);
  assert.match(match.stdout, /REACHED/);

  const clean = run(1);
  assert.equal(clean.status, 0, clean.stderr);
  assert.match(clean.stdout, /PASS:packaged app contains no private paths/);
  assert.match(clean.stdout, /REACHED/);

  const error = run(2);
  assert.equal(error.status, 0, error.stderr);
  assert.match(error.stdout, /FAIL:packaged app privacy scan failed closed/);
  assert.match(error.stdout, /REACHED/);
});
