#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DYLIB_VALIDATION_SCRIPT="${ROOT_DIR}/scripts/macos-dylib-validation.sh"
if [[ ! -r "${DYLIB_VALIDATION_SCRIPT}" ]]; then
  echo "Required dylib validation helper is missing: ${DYLIB_VALIDATION_SCRIPT}" >&2
  exit 1
fi
# shellcheck source=macos-dylib-validation.sh
source "${DYLIB_VALIDATION_SCRIPT}"
UNIVERSAL_APP="${ROOT_DIR}/src-tauri/target/universal-apple-darwin/release/bundle/macos/PST QuickView.app"
NATIVE_APP="${ROOT_DIR}/src-tauri/target/release/bundle/macos/PST QuickView.app"
EXPECTED_VERSION="0.6.76"
EXPECTED_X86_64_TARGET="10.13"
EXPECTED_ARM64_TARGET="11.0"

if [[ -n "${1:-}" ]]; then
  APP_PATH="$1"
elif [[ -d "${UNIVERSAL_APP}" ]]; then
  APP_PATH="${UNIVERSAL_APP}"
else
  APP_PATH="${NATIVE_APP}"
fi

MACOS_DIR="${APP_PATH}/Contents/MacOS"
APP_BINARY="${MACOS_DIR}/pst-quickview"
READPST_BINARY="${MACOS_DIR}/readpst"
SOURCE_BINARIES_DIR="${ROOT_DIR}/src-tauri/binaries"
SOURCE_X86_64="${SOURCE_BINARIES_DIR}/readpst-x86_64-apple-darwin"
SOURCE_ARM64="${SOURCE_BINARIES_DIR}/readpst-aarch64-apple-darwin"
SOURCE_UNIVERSAL="${SOURCE_BINARIES_DIR}/readpst-universal-apple-darwin"

for command in awk basename cmp file grep lipo otool strings tail xcrun; do
  if ! command -v "${command}" >/dev/null 2>&1; then
    echo "Required verification command is missing: ${command}" >&2
    exit 1
  fi
done
if ! xcrun --find vtool >/dev/null 2>&1; then
  echo "Required Apple tool is missing: vtool" >&2
  exit 1
fi

host_arch="$(uname -m)"
case "${host_arch}" in
  arm64) runnable_arch="arm64" ;;
  x86_64) runnable_arch="x86_64" ;;
  *) runnable_arch="${host_arch}" ;;
esac

deployment_target_for_arch() {
  local binary="$1"
  local arch="$2"
  local output
  output="$(xcrun vtool -arch "${arch}" -show-build "${binary}" 2>&1)" || {
    echo "Unable to inspect deployment target for ${binary} (${arch}): ${output}" >&2
    return 1
  }
  awk '
    $1 == "minos" { print $2; exit }
    $1 == "version" { print $2; exit }
  ' <<<"${output}"
}

check_target() {
  local binary="$1"
  local arch="$2"
  local expected="$3"
  local label="$4"
  local actual
  actual="$(deployment_target_for_arch "${binary}" "${arch}")"
  if [[ -z "${actual}" ]]; then
    echo "${label}: missing deployment target for ${arch}" >&2
    exit 1
  fi
  if [[ "${actual}" != "${expected}" ]]; then
    echo "${label}: ${arch} requires macOS ${actual}; expected ${expected}" >&2
    exit 1
  fi
  printf 'PASS: %s: %s, macOS %s\n' "${label}" "${arch}" "${actual}"
}

check_exact_arches() {
  local binary="$1"
  local expected="$2"
  local label="$3"
  local actual
  actual="$(lipo -archs "${binary}" 2>/dev/null || true)"
  case "${expected}" in
    universal)
      if [[ "${actual}" != "x86_64 arm64" && "${actual}" != "arm64 x86_64" ]]; then
        echo "${label}: expected exactly x86_64 and arm64; found ${actual:-unknown}" >&2
        exit 1
      fi
      ;;
    *)
      if [[ "${actual}" != "${expected}" ]]; then
        echo "${label}: expected ${expected}; found ${actual:-unknown}" >&2
        exit 1
      fi
      ;;
  esac
}

check_system_links() {
  local binary="$1"
  local label="$2"
  if ! pq_validate_macho_system_dependencies "${binary}" "${label}"; then
    exit 1
  fi
  printf 'PASS: %s uses only permitted macOS system dylibs/frameworks\n' "${label}"
}

check_version_markers() {
  local binary="$1"
  local label="$2"
  if ! strings "${binary}" | grep -F 'ReadPST / LibPST v%s' >/dev/null ||
    ! strings "${binary}" | grep -Fx "${EXPECTED_VERSION}" >/dev/null; then
    echo "${label}: ReadPST ${EXPECTED_VERSION} version markers are missing" >&2
    exit 1
  fi
}

run_version_if_supported() {
  local binary="$1"
  local label="$2"
  local arches output
  arches="$(lipo -archs "${binary}" 2>/dev/null || true)"
  if [[ " ${arches} " != *" ${runnable_arch} "* ]]; then
    printf 'SKIP: %s -V on %s; architectures: %s\n' "${label}" "${host_arch}" "${arches:-unknown}"
    return
  fi
  output="$("${binary}" -V 2>&1)"
  if [[ "${output}" != *"ReadPST / LibPST v${EXPECTED_VERSION}"* ]]; then
    echo "${label}: unexpected -V output: ${output}" >&2
    exit 1
  fi
  printf 'PASS: %s reports ReadPST / LibPST v%s\n' "${label}" "${EXPECTED_VERSION}"
}

for binary in "${SOURCE_X86_64}" "${SOURCE_ARM64}" "${SOURCE_UNIVERSAL}"; do
  if [[ ! -x "${binary}" ]]; then
    echo "Missing executable ReadPST sidecar: ${binary}" >&2
    exit 1
  fi
done

check_exact_arches "${SOURCE_X86_64}" x86_64 "source Intel ReadPST"
check_exact_arches "${SOURCE_ARM64}" arm64 "source Apple Silicon ReadPST"
check_exact_arches "${SOURCE_UNIVERSAL}" universal "source universal ReadPST"
check_target "${SOURCE_X86_64}" x86_64 "${EXPECTED_X86_64_TARGET}" "source Intel ReadPST"
check_target "${SOURCE_ARM64}" arm64 "${EXPECTED_ARM64_TARGET}" "source Apple Silicon ReadPST"
check_target "${SOURCE_UNIVERSAL}" x86_64 "${EXPECTED_X86_64_TARGET}" "source universal ReadPST"
check_target "${SOURCE_UNIVERSAL}" arm64 "${EXPECTED_ARM64_TARGET}" "source universal ReadPST"

for binary in "${SOURCE_X86_64}" "${SOURCE_ARM64}" "${SOURCE_UNIVERSAL}"; do
  label="$(basename "${binary}")"
  check_system_links "${binary}" "${label}"
  check_version_markers "${binary}" "${label}"
done
run_version_if_supported "${SOURCE_X86_64}" "source Intel ReadPST"
run_version_if_supported "${SOURCE_ARM64}" "source Apple Silicon ReadPST"
run_version_if_supported "${SOURCE_UNIVERSAL}" "source universal ReadPST"

if [[ ! -d "${APP_PATH}" ]]; then
  echo "App bundle not found: ${APP_PATH}" >&2
  exit 1
fi
if [[ ! -x "${APP_BINARY}" ]]; then
  echo "App executable not found: ${APP_BINARY}" >&2
  exit 1
fi
if [[ ! -x "${READPST_BINARY}" ]]; then
  echo "Bundled ReadPST not found: ${READPST_BINARY}" >&2
  exit 1
fi

file "${APP_BINARY}"
file "${READPST_BINARY}"
check_system_links "${APP_BINARY}" "packaged app executable"
check_system_links "${READPST_BINARY}" "packaged ReadPST"
check_version_markers "${READPST_BINARY}" "packaged ReadPST"

packaged_arches="$(lipo -archs "${READPST_BINARY}")"
case "${packaged_arches}" in
  x86_64)
    expected_source="${SOURCE_X86_64}"
    check_target "${READPST_BINARY}" x86_64 "${EXPECTED_X86_64_TARGET}" "packaged ReadPST"
    ;;
  arm64)
    expected_source="${SOURCE_ARM64}"
    check_target "${READPST_BINARY}" arm64 "${EXPECTED_ARM64_TARGET}" "packaged ReadPST"
    ;;
  "x86_64 arm64"|"arm64 x86_64")
    expected_source="${SOURCE_UNIVERSAL}"
    check_target "${READPST_BINARY}" x86_64 "${EXPECTED_X86_64_TARGET}" "packaged ReadPST"
    check_target "${READPST_BINARY}" arm64 "${EXPECTED_ARM64_TARGET}" "packaged ReadPST"
    ;;
  *)
    echo "Packaged ReadPST has unsupported architectures: ${packaged_arches:-unknown}" >&2
    exit 1
    ;;
esac

if ! cmp -s "${READPST_BINARY}" "${expected_source}"; then
  echo "Packaged ReadPST differs from ${expected_source}." >&2
  exit 1
fi
printf 'PASS: packaged ReadPST is byte-identical to %s\n' "$(basename "${expected_source}")"
run_version_if_supported "${READPST_BINARY}" "packaged ReadPST"

echo "Bundled ReadPST verification passed: version, architectures, per-slice deployment targets, and system-only linkage are correct."
