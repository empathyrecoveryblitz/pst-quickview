#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/pst-quickview-public-delivery-audit.XXXXXX")"

cleanup() {
  local status=$?
  case "${TMP_ROOT}" in
    "${TMPDIR:-/tmp}"/pst-quickview-public-delivery-audit.*)
      [[ -d "${TMP_ROOT}" && ! -L "${TMP_ROOT}" ]] && rm -rf -- "${TMP_ROOT}"
      ;;
  esac
  return "${status}"
}
trap cleanup EXIT

copy_delivery_fixture() {
  local destination="$1" relative
  mkdir -p "${destination}"
  for relative in \
    PUBLIC_GITHUB_AUDIT.md \
    THIRD_PARTY_NOTICES.md \
    docs/LICENSE_DECISION.md \
    docs/READPST_CORRESPONDING_SOURCE.md \
    docs/RELEASE_COMPLIANCE.md; do
    mkdir -p "${destination}/$(dirname "${relative}")"
    cp -p "${ROOT_DIR}/${relative}" "${destination}/${relative}"
  done
}

expect_pass() {
  local label="$1" fixture="$2" output
  output="${TMP_ROOT}/${label}.out"
  if bash "${ROOT_DIR}/scripts/verify-public-release-delivery-docs.sh" --root "${fixture}" \
    >"${output}" 2>&1 &&
    grep -q 'public-delivery documentation is complete and internally consistent' "${output}"; then
    printf 'PASS: %s\n' "${label}"
  else
    printf 'FAIL: %s\n' "${label}" >&2
    sed -n '1,220p' "${output}" >&2
    return 1
  fi
}

expect_fail() {
  local label="$1" fixture="$2" output
  output="${TMP_ROOT}/${label}.out"
  if bash "${ROOT_DIR}/scripts/verify-public-release-delivery-docs.sh" --root "${fixture}" \
    >"${output}" 2>&1; then
    printf 'FAIL: %s unexpectedly passed\n' "${label}" >&2
    return 1
  fi
  if grep -Eq 'COMPLETE delivery contract|Contradictory pending-delivery wording' "${output}"; then
    printf 'PASS: %s\n' "${label}"
  else
    printf 'FAIL: %s did not fail on the delivery contract\n' "${label}" >&2
    sed -n '1,220p' "${output}" >&2
    return 1
  fi
}

complete_fixture="${TMP_ROOT}/complete"
copy_delivery_fixture "${complete_fixture}"
expect_pass "complete-public-state" "${complete_fixture}"

pending_fixture="${TMP_ROOT}/pending"
cp -R "${complete_fixture}" "${pending_fixture}"
perl -0pi -e 's/\*\*Public delivery beside the DMG: COMPLETE\*\*/**Public delivery beside the DMG: PENDING AND RELEASE-BLOCKING**/' \
  "${pending_fixture}/docs/READPST_CORRESPONDING_SOURCE.md"
expect_fail "pending-state" "${pending_fixture}"

missing_fixture="${TMP_ROOT}/missing"
cp -R "${complete_fixture}" "${missing_fixture}"
perl -0pi -e 's/^\*\*Public delivery beside the DMG: COMPLETE\*\*\n//m' \
  "${missing_fixture}/docs/READPST_CORRESPONDING_SOURCE.md"
expect_fail "missing-state" "${missing_fixture}"

contradictory_fixture="${TMP_ROOT}/contradictory"
cp -R "${complete_fixture}" "${contradictory_fixture}"
printf '\nPublic delivery remains pending.\n' >>"${contradictory_fixture}/docs/READPST_CORRESPONDING_SOURCE.md"
expect_fail "contradictory-state" "${contradictory_fixture}"

printf 'Public release delivery audit regression tests passed.\n'
