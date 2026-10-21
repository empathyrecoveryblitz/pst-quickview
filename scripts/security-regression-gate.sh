#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUN_STRESS="${PST_QUICKVIEW_SECURITY_STRESS:-0}"

if [[ "${RUN_STRESS}" != "0" && "${RUN_STRESS}" != "1" ]]; then
  printf 'FAIL: PST_QUICKVIEW_SECURITY_STRESS must be 0 or 1\n' >&2
  exit 2
fi

run_check() {
  local label="$1"
  shift
  printf '\n==> %s\n' "${label}"
  "$@"
  printf 'PASS: %s\n' "${label}"
}

run_frontend_security_tests() {
  cd "${ROOT_DIR}"
  node --test \
    tests/importIntegrity.test.mjs \
    tests/importProgress.test.mjs \
    tests/localTrash.test.mjs \
    tests/loggingPrivacy.test.mjs \
    tests/runtimeProfileIsolation.test.mjs \
    tests/remoteSupport.test.mjs \
    tests/developmentUpdateUI.test.mjs \
    tests/developmentUpdateDispatch.test.mjs \
    tests/packagedCspInspection.test.mjs \
    tests/packagedSupportPins.test.mjs \
    tests/supportDashboard.test.mjs \
    tests/supportDashboardLauncher.test.mjs \
    tests/supportOperatorSetup.test.mjs \
    tests/supportProtocol.test.mjs \
    tests/supportQueueProtocol.test.mjs \
    tests/supportQueueWorker.test.mjs \
    tests/supportServer.test.mjs \
    tests/supportUpdateArtifact.test.mjs \
    tests/supportUpdateAssets.test.mjs \
    tests/supportUpdateLive.test.mjs \
    tests/supportClockSkew.test.mjs \
    tests/supportRuntimeNotices.test.mjs \
    tests/supportDevelopmentBuild.test.mjs \
    tests/supportView.test.mjs \
    tests/searchCancellation.test.mjs \
    tests/searchHighlight.test.mjs \
    tests/searchPagination.test.mjs \
    tests/securityResourceLimits.test.mjs \
    tests/standaloneRemoteContent.test.mjs \
    tests/tauriSecurityBoundary.test.mjs \
    tests/variableHeightWindow.test.mjs \
    tests/workspaceIntegrity.test.mjs
}

run_rust_format() {
  cd "${ROOT_DIR}/src-tauri"
  cargo fmt --check
}

run_rust_clippy() {
  cd "${ROOT_DIR}/src-tauri"
  cargo clippy --locked --all-targets -- -D warnings
}

run_rust_security_tests() {
  cd "${ROOT_DIR}/src-tauri"
  cargo test --locked
}

run_search_stress_gate() {
  cd "${ROOT_DIR}/src-tauri"
  cargo test --locked search_2_stabilization_stress_gate -- --ignored --nocapture
}

run_readpst_verification() {
  cd "${ROOT_DIR}"
  bash scripts/verify-macos-readpst-bundle.sh --source-only
}

printf 'PST QuickView security regression gate\n'
printf 'Repository: %s\n' "${ROOT_DIR}"

run_check "tracked diff whitespace" git -C "${ROOT_DIR}" diff --check
run_check "focused frontend security tests" run_frontend_security_tests
run_check "Rust formatting" run_rust_format
run_check "Rust Clippy" run_rust_clippy
run_check "Rust security regressions" run_rust_security_tests
run_check "bundled ReadPST verification" run_readpst_verification

if [[ "${RUN_STRESS}" == "1" ]]; then
  run_check "Search 2.0 synthetic stress gate" run_search_stress_gate
else
  printf '\nSKIP: Search 2.0 synthetic stress gate '
  printf '(set PST_QUICKVIEW_SECURITY_STRESS=1)\n'
fi

printf '\nPASS: security regression gate completed\n'
