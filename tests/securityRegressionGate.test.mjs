import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const gate = readFileSync(
  new URL("../scripts/security-regression-gate.sh", import.meta.url),
  "utf8",
);
const readpstVerifier = readFileSync(
  new URL("../scripts/verify-macos-readpst-bundle.sh", import.meta.url),
  "utf8",
);

test("security gate covers each hardened boundary without private fixtures", () => {
  for (const testFile of [
    "importIntegrity.test.mjs",
    "importProgress.test.mjs",
    "loggingPrivacy.test.mjs",
    "searchCancellation.test.mjs",
    "searchHighlight.test.mjs",
    "searchPagination.test.mjs",
    "securityResourceLimits.test.mjs",
    "standaloneRemoteContent.test.mjs",
    "tauriSecurityBoundary.test.mjs",
    "variableHeightWindow.test.mjs",
    "workspaceIntegrity.test.mjs",
  ]) {
    assert.match(gate, new RegExp(`tests/${testFile.replaceAll(".", "\\.")}`));
  }
  assert.match(gate, /cargo clippy --locked --all-targets -- -D warnings/);
  assert.match(gate, /cargo test --locked/);
  assert.match(gate, /verify-macos-readpst-bundle\.sh --source-only/);
  assert.doesNotMatch(gate, /PST_QUICKVIEW_(?:RICH|LEGACY|TREVOR|FURMAN)_MSG_FIXTURE/);
});

test("ReadPST verification never falls back to a stale packaged app", () => {
  assert.doesNotMatch(readpstVerifier, /target\/universal-apple-darwin/);
  assert.doesNotMatch(readpstVerifier, /target\/release\/bundle\/macos/);
  assert.match(readpstVerifier, /""\|--source-only/);
  assert.match(readpstVerifier, /Packaged verification requires an explicit app path/);
});

test("expensive synthetic search stress is explicit and fail closed", () => {
  assert.match(gate, /PST_QUICKVIEW_SECURITY_STRESS:-0/);
  assert.match(gate, /PST_QUICKVIEW_SECURITY_STRESS must be 0 or 1/);
  assert.match(
    gate,
    /cargo test --locked search_2_stabilization_stress_gate -- --ignored --nocapture/,
  );
  assert.match(gate, /if \[\[ "\$\{RUN_STRESS\}" == "1" \]\]/);
});
