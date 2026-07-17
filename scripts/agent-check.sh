#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
UNIVERSAL_APP="${ROOT_DIR}/src-tauri/target/universal-apple-darwin/release/bundle/macos/PST QuickView.app"

pass() {
  printf 'PASS: %s\n' "$1"
}

fail() {
  printf 'FAIL: %s\n' "$1" >&2
}

skip() {
  printf 'SKIP: %s\n' "$1"
}

run_check() {
  local label="$1"
  shift

  printf '\n==> %s\n' "${label}"
  local status=0
  if "$@"; then
    pass "${label}"
  else
    status=$?
    fail "${label} (exit ${status})"
    exit "${status}"
  fi
}

check_git_diff() {
  git -C "${ROOT_DIR}" diff --check
}

check_cargo_fmt() {
  (
    cd "${ROOT_DIR}/src-tauri"
    cargo fmt --check
  )
}

check_cargo_check() {
  (
    cd "${ROOT_DIR}/src-tauri"
    cargo check --locked
  )
}

check_cargo_test() {
  (
    cd "${ROOT_DIR}/src-tauri"
    cargo test --locked
  )
}

check_frontend_build() {
  (
    cd "${ROOT_DIR}"
    npm run build
  )
}

check_frontend_test() {
  (cd "${ROOT_DIR}" && npm run test:frontend)
}

check_public_audit() {
  (cd "${ROOT_DIR}" && scripts/audit-public-repo.sh)
}

check_shell_syntax() {
  while IFS= read -r -d '' script; do bash -n "${script}"; done < <(find "${ROOT_DIR}/scripts" -type f -name '*.sh' -print0)
}

check_rich_fixture() {
  (
    cd "${ROOT_DIR}/src-tauri"
    PST_QUICKVIEW_RICH_MSG_FIXTURE="${PST_QUICKVIEW_RICH_MSG_FIXTURE}" \
      PST_QUICKVIEW_RICH_MSG_EXPECTED_SHA256="${PST_QUICKVIEW_RICH_MSG_EXPECTED_SHA256:-}" \
      cargo test --locked verifies_rich_msg_fixture_reconstruction \
        -- --ignored --nocapture
  )
}

check_legacy_fixture() {
  (
    cd "${ROOT_DIR}/src-tauri"
    PST_QUICKVIEW_LEGACY_MSG_FIXTURE="${PST_QUICKVIEW_LEGACY_MSG_FIXTURE}" \
      PST_QUICKVIEW_LEGACY_MSG_EXPECTED_SHA256="${PST_QUICKVIEW_LEGACY_MSG_EXPECTED_SHA256:-}" \
      cargo test --locked verifies_legacy_msg_fixture_reconstruction \
        -- --ignored --nocapture
  )
}

check_release() {
  (
    cd "${ROOT_DIR}"
    bash scripts/verify-macos-release.sh
  )
}

if ! git -C "${ROOT_DIR}" rev-parse --is-inside-work-tree >/dev/null 2>&1; then
  fail "repository root is not inside a Git worktree: ${ROOT_DIR}"
  exit 2
fi

printf 'PST QuickView acceptance checks\n'
printf 'Repository: %s\n' "${ROOT_DIR}"

run_check "git diff --check" check_git_diff
run_check "shell script syntax" check_shell_syntax
run_check "public repository audit" check_public_audit
run_check "npm run test:frontend" check_frontend_test
run_check "cargo fmt --check" check_cargo_fmt
run_check "cargo check" check_cargo_check
run_check "cargo test" check_cargo_test

PST_QUICKVIEW_RICH_MSG_FIXTURE="${PST_QUICKVIEW_RICH_MSG_FIXTURE:-${PST_QUICKVIEW_TREVOR_MSG_FIXTURE:-}}"
PST_QUICKVIEW_LEGACY_MSG_FIXTURE="${PST_QUICKVIEW_LEGACY_MSG_FIXTURE:-${PST_QUICKVIEW_FURMAN_MSG_FIXTURE:-}}"

if [[ -n "${PST_QUICKVIEW_RICH_MSG_FIXTURE}" ]]; then
  if [[ -f "${PST_QUICKVIEW_RICH_MSG_FIXTURE}" ]]; then
    run_check "Rich MSG fixture regression" check_rich_fixture
  else
    skip "Rich MSG fixture path does not exist"
  fi
else
  skip "Rich MSG fixture is not configured"
fi

if [[ -n "${PST_QUICKVIEW_LEGACY_MSG_FIXTURE}" ]]; then
  if [[ -f "${PST_QUICKVIEW_LEGACY_MSG_FIXTURE}" ]]; then
    run_check "Legacy MSG fixture regression" check_legacy_fixture
  else
    skip "Legacy MSG fixture path does not exist"
  fi
else
  skip "Legacy MSG fixture is not configured"
fi

run_check "npm run build" check_frontend_build

if [[ "${VERIFY_RELEASE:-0}" == "1" || -d "${UNIVERSAL_APP}" ]]; then
  run_check "macOS release verification" check_release
else
  skip "macOS release verification (set VERIFY_RELEASE=1 or build the universal app)"
fi

printf '\n'
pass "all acceptance checks passed"
