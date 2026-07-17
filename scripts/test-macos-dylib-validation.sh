#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=macos-dylib-validation.sh
source "${ROOT_DIR}/scripts/macos-dylib-validation.sh"

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  exit 1
}

assert_parsed_dependencies() {
  local label="$1"
  local fixture="$2"
  local expected="$3"
  local actual

  actual="$(printf '%s\n' "${fixture}" | pq_parse_otool_dependencies)"
  if [[ "${actual}" != "${expected}" ]]; then
    printf 'Expected dependencies:\n%s\nActual dependencies:\n%s\n' \
      "${expected}" "${actual}" >&2
    fail "${label} dependency extraction mismatch"
  fi
  if [[ "${actual}" == *'/path/to/readpst'* ]]; then
    fail "${label} returned a binary header as a dependency"
  fi
}

thin_output='/path/to/readpst:
    /usr/lib/libz.1.dylib (compatibility version 1.0.0, current version 1.2.12)
    /usr/lib/libSystem.B.dylib (compatibility version 1.0.0, current version 1351.0.0)'
thin_expected='/usr/lib/libz.1.dylib
/usr/lib/libSystem.B.dylib'
assert_parsed_dependencies "thin output" "${thin_output}" "${thin_expected}"
pq_validate_otool_dependency_output \
  "/path/to/readpst" "thin fixture" "x86_64" "${thin_output}" ||
  fail "thin output should pass system dependency validation"
printf 'PASS: thin otool output parses only system dependency records\n'

universal_output='/path/to/readpst (architecture x86_64):
    /usr/lib/libz.1.dylib (compatibility version 1.0.0, current version 1.2.12)
    /usr/lib/libSystem.B.dylib (compatibility version 1.0.0, current version 1351.0.0)
 /path/to/readpst (architecture arm64):
    /usr/lib/libz.1.dylib (compatibility version 1.0.0, current version 1.2.12)
    /usr/lib/libSystem.B.dylib (compatibility version 1.0.0, current version 1351.0.0)'
universal_expected='/usr/lib/libz.1.dylib
/usr/lib/libSystem.B.dylib
/usr/lib/libz.1.dylib
/usr/lib/libSystem.B.dylib'
assert_parsed_dependencies \
  "universal output" "${universal_output}" "${universal_expected}"
pq_validate_otool_dependency_output \
  "/path/to/readpst" "universal fixture" "arm64" "${universal_output}" ||
  fail "universal output should pass system dependency validation"
printf 'PASS: universal architecture headers are not parsed as dependencies\n'

prohibited_output='/path/to/readpst (architecture arm64):
    /opt/homebrew/opt/example/lib/libexample.dylib (compatibility version 1.0.0, current version 1.0.0)'
if prohibited_error="$(pq_validate_otool_dependency_output \
  "/path/to/readpst" "prohibited fixture" "arm64" "${prohibited_output}" 2>&1)"; then
  fail "prohibited dependency should fail validation"
fi
if [[ "${prohibited_error}" != *'/opt/homebrew/opt/example/lib/libexample.dylib'* ]]; then
  printf '%s\n' "${prohibited_error}" >&2
  fail "prohibited dependency error did not identify the actual library"
fi
printf 'PASS: prohibited dependency fails and reports the actual install name\n'

header_only_output='/path/to/readpst (architecture arm64):'
if empty_error="$(pq_validate_otool_dependency_output \
  "/path/to/readpst" "empty fixture" "arm64" "${header_only_output}" 2>&1)"; then
  fail "header-only output should fail validation"
fi
if [[ "${empty_error}" != *'no dynamic dependency records parsed'* ]]; then
  printf '%s\n' "${empty_error}" >&2
  fail "header-only output did not report the missing dependency records"
fi
printf 'PASS: missing dependency records fail clearly\n'
