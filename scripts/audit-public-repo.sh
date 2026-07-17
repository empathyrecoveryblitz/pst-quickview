#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SCAN_HISTORY=0

usage() {
  printf 'Usage: %s [--history] [--root PATH]\n' "$0"
}

while (($#)); do
  case "$1" in
    --history) SCAN_HISTORY=1; shift ;;
    --root) ROOT_DIR="$(cd "$2" && pwd)"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
  esac
done

ALLOWLIST="${ROOT_DIR}/.public-audit-allowlist"
FAILURES=0
WARNINGS=0
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "${TMP_DIR}"' EXIT

CREDENTIAL_PATTERN='-----BEGIN ([A-Z ]+)?PRIVATE KEY-----|AKIA[0-9A-Z]{16}|github_pat_[A-Za-z0-9_]{20,}|gh[pousr]_[A-Za-z0-9_]{20,}|xox[baprs]-[A-Za-z0-9-]{10,}|AIza[0-9A-Za-z_-]{30,}|sk-[A-Za-z0-9]{20,}'
AUTH_ASSIGNMENT_PATTERN="(password|passwd|client_secret|access_token|oauth_token)[[:space:]]*[:=][[:space:]]*['\"][^'\"]{8,}"
GPL3_SHA256="3972dc9744f6499f0f9b2dbf76696f2ae7ad8af9b23dde66d6af86c9dfb36986"
GPL2_SHA256="edaef632cbb643e4e7a221717a6c441a4c1a7c918e6e4d56debc3d8739b233f6"

pass() { printf 'PASS: %s\n' "$1"; }
warn() { WARNINGS=$((WARNINGS + 1)); printf 'WARN: %s\n' "$1"; }
fail() { FAILURES=$((FAILURES + 1)); printf 'FAIL: %s\n' "$1" >&2; }

verify_file_hash() {
  local relative="$1" expected="$2" label="$3"
  local path="${ROOT_DIR}/${relative}"
  if [[ ! -s "${path}" ]]; then
    fail "${label} is missing or empty: ${relative}"
    return
  fi
  local actual
  actual="$(shasum -a 256 "${path}" | awk '{print $1}')"
  if [[ "${actual}" == "${expected}" ]]; then
    pass "${label} matches the official text"
  else
    fail "${label} hash is ${actual}; expected ${expected}"
  fi
}

validate_allowlist() {
  [[ -f "${ALLOWLIST}" ]] || return 0
  local entry line_number=0
  while IFS= read -r entry; do
    line_number=$((line_number + 1))
    [[ -z "${entry}" || "${entry}" =~ ^[[:space:]]*# ]] && continue
    case "${entry}" in
      '^src-tauri/src/lib\.rs:[0-9]+:    const RICH_MSG_FIXTURE_LEGACY_ENV: &str = "PST_QUICKVIEW_TREVOR_MSG_FIXTURE";$'|\
      '^src-tauri/src/lib\.rs:[0-9]+:    const LEGACY_MSG_FIXTURE_LEGACY_ENV: &str = "PST_QUICKVIEW_FURMAN_MSG_FIXTURE";$'|\
      '^scripts/agent-check\.sh:[0-9]+:PST_QUICKVIEW_RICH_MSG_FIXTURE=.*PST_QUICKVIEW_TREVOR_MSG_FIXTURE.*$'|\
      '^scripts/agent-check\.sh:[0-9]+:PST_QUICKVIEW_LEGACY_MSG_FIXTURE=.*PST_QUICKVIEW_FURMAN_MSG_FIXTURE.*$'|\
      '^scripts/verify-macos-release\.sh:[0-9]+:.*PST_QUICKVIEW_.*MSG_FIXTURE.*$') ;;
      *)
        fail "allowlist entry at line ${line_number} is not an approved exact exception"
        return 1
        ;;
    esac
    if printf '' | grep -E "${entry}" >/dev/null 2>&1; then
      :
    elif [[ $? -eq 2 ]]; then
      fail "allowlist entry at line ${line_number} is not a valid extended regular expression"
      return 1
    fi
  done <"${ALLOWLIST}"
}

git_commit_contains() {
  local commit="$1" pattern="$2" status
  if git -C "${ROOT_DIR}" grep -l -I -E -e "${pattern}" "${commit}" -- . \
    >/dev/null 2>&1; then
    return 0
  else
    status=$?
    if ((status == 1)); then
      return 1
    fi
    fail "Git history scanner error in commit ${commit}"
    return "${status}"
  fi
}

validate_allowlist

verify_file_hash "LICENSE" "${GPL3_SHA256}" "root GPL-3.0 license"
verify_file_hash "LICENSES/GPL-3.0-or-later.txt" "${GPL3_SHA256}" "GPL-3.0-or-later license copy"
verify_file_hash "LICENSES/GPL-2.0-or-later.txt" "${GPL2_SHA256}" "GPL-2.0-or-later license copy"

if [[ -s "${ROOT_DIR}/LICENSE" && -s "${ROOT_DIR}/LICENSES/GPL-3.0-or-later.txt" ]] &&
  cmp -s "${ROOT_DIR}/LICENSE" "${ROOT_DIR}/LICENSES/GPL-3.0-or-later.txt"; then
  pass "root LICENSE is byte-identical to GPL-3.0-or-later.txt"
else
  fail "root LICENSE must be byte-identical to LICENSES/GPL-3.0-or-later.txt"
fi

if python3 - "${ROOT_DIR}" <<'PY'
import json
import pathlib
import re
import sys

root = pathlib.Path(sys.argv[1])
expected = "GPL-3.0-or-later"
package = json.loads((root / "package.json").read_text())
lock = json.loads((root / "package-lock.json").read_text())
cargo = (root / "src-tauri/Cargo.toml").read_text()

if package.get("license") != expected:
    raise SystemExit("package.json project license identifier is incorrect")
if lock.get("packages", {}).get("", {}).get("license") != expected:
    raise SystemExit("package-lock root package license identifier is incorrect")
match = re.search(r'(?ms)^\[package\].*?^license\s*=\s*"([^"]+)"', cargo)
if not match or match.group(1) != expected:
    raise SystemExit("Cargo.toml project license identifier is incorrect")
PY
then
  pass "project metadata uses GPL-3.0-or-later"
else
  fail "project license metadata verification failed"
fi

if [[ -s "${ROOT_DIR}/docs/READPST_CORRESPONDING_SOURCE.md" ]] &&
  grep -q '^\*\*Technical preparation: COMPLETE\*\*$' \
    "${ROOT_DIR}/docs/READPST_CORRESPONDING_SOURCE.md" &&
  grep -q '^\*\*Public delivery beside the DMG: PENDING AND RELEASE-BLOCKING\*\*$' \
    "${ROOT_DIR}/docs/READPST_CORRESPONDING_SOURCE.md" &&
  [[ -x "${ROOT_DIR}/scripts/prepare-readpst-corresponding-source.sh" ]] &&
  [[ -x "${ROOT_DIR}/scripts/verify-readpst-corresponding-source.sh" ]] &&
  grep -q 'PUBLIC_RELEASE=1' "${ROOT_DIR}/scripts/verify-macos-release.sh"; then
  warn "ReadPST Corresponding Source is technically prepared; public delivery beside the DMG remains blocking"
else
  fail "ReadPST Corresponding Source public-release gate is missing or not visible"
fi

GIT_TOP_LEVEL="$(git -C "${ROOT_DIR}" rev-parse --show-toplevel 2>/dev/null || true)"
if [[ -n "${GIT_TOP_LEVEL}" && "$(cd "${GIT_TOP_LEVEL}" && pwd)" == "${ROOT_DIR}" ]]; then
  git -C "${ROOT_DIR}" ls-files --cached --others --exclude-standard -z >"${TMP_DIR}/files"
else
  while IFS= read -r -d '' absolute; do
    printf '%s\0' "${absolute#"${ROOT_DIR}/"}"
  done < <(find "${ROOT_DIR}" -type f -print0) >"${TMP_DIR}/files"
fi

scan_pattern() {
  local label="$1" confidence="$2" pattern="$3" allowlist_mode="${4:-deny}" scan_policy="${5:-include_config}" output="${TMP_DIR}/matches"
  : >"${output}"
  while IFS= read -r -d '' relative; do
    local path="${relative}"
    [[ "${path}" = /* ]] || path="${ROOT_DIR}/${path}"
    [[ -f "${path}" ]] || continue
    if [[ "${scan_policy}" == "skip_config" ]]; then
      case "${path}" in
        */scripts/audit-public-repo.sh|*/.public-audit-allowlist) continue ;;
      esac
    fi
    local file_matches="${TMP_DIR}/file-matches"
    if LC_ALL=C grep -I -n -E -e "${pattern}" -- "${path}" >"${file_matches}" 2>/dev/null; then
      sed "s#^#${relative}:#" "${file_matches}" >>"${output}"
    else
      local grep_status=$?
      if ((grep_status != 1)); then
        fail "scanner error while checking ${relative} for ${label}"
        return "${grep_status}"
      fi
    fi
  done <"${TMP_DIR}/files"
  # High-confidence secret checks are never allowlisted. Privacy-warning exceptions
  # must be path-and-line anchored and pass validate_allowlist above.
  if [[ "${allowlist_mode}" == "allow" && -s "${ALLOWLIST}" ]]; then
    grep -E -v -f <(grep -v '^[[:space:]]*#' "${ALLOWLIST}" | grep -v '^[[:space:]]*$') "${output}" >"${output}.filtered" || true
    mv "${output}.filtered" "${output}"
  fi
  local count
  count="$(wc -l <"${output}" | tr -d ' ')"
  if ((count == 0)); then
    pass "${label}"
  elif [[ "${confidence}" == "fail" ]]; then
    fail "${label}: ${count} potential secret(s); review the listed path and line numbers"
    cut -d: -f1-2 "${output}" | sort -u | sed 's/^/  /' >&2
  else
    warn "${label}: ${count} finding(s); review the listed path and line numbers"
    cut -d: -f1-2 "${output}" | sort -u | sed 's/^/  /'
  fi
}

scan_pattern "high-confidence credential patterns absent" fail "${CREDENTIAL_PATTERN}"
scan_pattern "private local paths and organization identifiers absent" warn '/Users/notroot|/Volumes/T7|nyu\.edu|law\.nyu\.edu|mercury\.law\.nyu\.edu|PST_QUICKVIEW_(TREVOR|FURMAN)_MSG_FIXTURE' allow skip_config
scan_pattern "private fixture names absent" warn 'Trevor|Furman|Adam Cox|David Niedenthal|Cheryl Hark' allow skip_config
scan_pattern "hard-coded auth assignments absent" fail "${AUTH_ASSIGNMENT_PATTERN}"
scan_pattern "publication placeholder check" warn 'https://github\.com/OWNER/REPOSITORY' deny skip_config
scan_pattern "obsolete unresolved project-license language absent" fail \
  'PST QuickView currently has no project-level license|There is no root project `?LICENSE|project currently has no reuse license|currently has no project-level open-source license' \
  deny skip_config

if ((SCAN_HISTORY)); then
  if [[ -z "${GIT_TOP_LEVEL}" || "$(cd "${GIT_TOP_LEVEL}" && pwd)" != "${ROOT_DIR}" ]]; then
    warn "history scan skipped because the target has no Git history"
  else
    history_hits=0
    history_secret_hits=0
    while IFS= read -r commit; do
      privacy_status=0
      git_commit_contains "${commit}" '/Users/notroot|/Volumes/T7|nyu\.edu|PST_QUICKVIEW_(TREVOR|FURMAN)_MSG_FIXTURE|Trevor|Furman|Adam Cox' || privacy_status=$?
      case "${privacy_status}" in
        0) history_hits=$((history_hits + 1)) ;;
        1) ;;
        *) exit "${privacy_status}" ;;
      esac

      credential_status=0
      git_commit_contains "${commit}" "${CREDENTIAL_PATTERN}" || credential_status=$?
      if ((credential_status > 1)); then exit "${credential_status}"; fi
      auth_status=0
      git_commit_contains "${commit}" "${AUTH_ASSIGNMENT_PATTERN}" || auth_status=$?
      if ((auth_status > 1)); then exit "${auth_status}"; fi
      if ((credential_status == 0 || auth_status == 0)); then
        history_secret_hits=$((history_secret_hits + 1))
      fi
    done < <(git -C "${ROOT_DIR}" rev-list --all)
    if ((history_hits)); then
      warn "reachable history contains privacy-sensitive strings in ${history_hits} commit(s); use docs/PUBLIC_HISTORY.md"
    else
      pass "reachable history has no configured privacy-sensitive strings"
    fi
    if ((history_secret_hits)); then
      fail "reachable history contains high-confidence credential patterns in ${history_secret_hits} commit(s)"
    else
      pass "reachable history has no high-confidence credential patterns"
    fi
  fi
fi

printf 'Audit summary: %d failure(s), %d warning(s). No files were modified and no network was used.\n' "${FAILURES}" "${WARNINGS}"
((FAILURES == 0))
