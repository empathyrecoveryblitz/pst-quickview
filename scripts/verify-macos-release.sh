#!/usr/bin/env bash
set -uo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EXPECTED_VERSION="${PST_QUICKVIEW_EXPECTED_VERSION:-0.2.0-beta.2}"
EXPECTED_BUNDLE_VERSION="${PST_QUICKVIEW_EXPECTED_BUNDLE_VERSION:-0.2.0.2}"
EXPECTED_PROJECT_LICENSE="GPL-3.0-or-later"
EXPECTED_REPOSITORY="https://github.com/empathyrecoveryblitz/pst-quickview"
EXPECTED_GPL3_SHA256="3972dc9744f6499f0f9b2dbf76696f2ae7ad8af9b23dde66d6af86c9dfb36986"
EXPECTED_GPL2_SHA256="edaef632cbb643e4e7a221717a6c441a4c1a7c918e6e4d56debc3d8739b233f6"
EXPECTED_INFO_PLIST_TARGET="10.13"
EXPECTED_X86_64_TARGET="10.13"
EXPECTED_ARM64_TARGET="11.0"
EXPECTED_READPST_VERSION="0.6.76"
EXPECTED_READPST_COMPANION_URL="${PST_QUICKVIEW_EXPECTED_READPST_COMPANION_URL:-${EXPECTED_REPOSITORY}/releases/download/v${EXPECTED_VERSION}/readpst-corresponding-source-${EXPECTED_READPST_VERSION}.tar.gz}"
PUBLIC_RELEASE="${PUBLIC_RELEASE:-0}"
READPST_CORRESPONDING_SOURCE_DIR="${READPST_CORRESPONDING_SOURCE_DIR:-}"
DEFAULT_APP_PATH="${ROOT_DIR}/src-tauri/target/universal-apple-darwin/release/bundle/macos/PST QuickView.app"
APP_PATH="${1:-${DEFAULT_APP_PATH}}"
EXPECT_DMG="${EXPECT_DMG:-1}"
FAILURES=0

pass() {
  printf 'PASS: %s\n' "$1"
}

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  FAILURES=$((FAILURES + 1))
}

require_command() {
  if command -v "$1" >/dev/null 2>&1; then
    pass "required tool available: $1"
  else
    fail "required tool missing: $1"
  fi
}

check_arches() {
  local binary="$1"
  local expected="$2"
  local label="$3"
  if [[ ! -x "${binary}" ]]; then
    fail "${label} is missing or not executable: ${binary}"
    return
  fi
  local arches
  arches="$(lipo -archs "${binary}" 2>/dev/null || true)"
  case "${expected}" in
    universal)
      if [[ "${arches}" == "x86_64 arm64" || "${arches}" == "arm64 x86_64" ]]; then
        pass "${label} is universal (${arches})"
      else
        fail "${label} must contain exactly x86_64 and arm64; architectures: ${arches:-unknown}"
      fi
      ;;
    *)
      if [[ "${arches}" == "${expected}" ]]; then
        pass "${label} architecture is ${expected}"
      else
        fail "${label} expected ${expected}; architectures: ${arches:-unknown}"
      fi
      ;;
  esac
}

check_dependencies() {
  local binary="$1"
  local label="$2"
  if [[ ! -f "${binary}" ]]; then
    return
  fi
  printf '%s\n' "-- ${label}"
  file "${binary}"
  otool -L "${binary}"
  local dependency bad=""
  while IFS= read -r dependency; do
    [[ -z "${dependency}" ]] && continue
    case "${dependency}" in
      /usr/lib/*|/System/Library/Frameworks/*) ;;
      *) bad+="${dependency}"$'\n' ;;
    esac
  done < <(otool -L "${binary}" | tail -n +2 | awk '{print $1}')
  if [[ -n "${bad}" ]]; then
    fail "${label} has non-system dylib paths: ${bad%$'\n'}"
  else
    pass "${label} uses only permitted macOS system dylibs/frameworks"
  fi
}

deployment_target_for_arch() {
  local binary="$1"
  local arch="$2"
  local output
  output="$(xcrun vtool -arch "${arch}" -show-build "${binary}" 2>/dev/null)" || return 1
  awk '
    $1 == "minos" { print $2; exit }
    $1 == "version" { print $2; exit }
  ' <<<"${output}"
}

check_target_value() {
  local actual="$1"
  local expected="$2"
  local label="$3"
  if [[ -z "${actual}" ]]; then
    fail "${label} deployment target is missing or unreadable"
  elif [[ "${actual}" == "${expected}" ]]; then
    pass "${label} deployment target is macOS ${actual}"
  else
    fail "${label} deployment target is macOS ${actual}; expected ${expected}"
  fi
}

check_matching_targets() {
  local readpst_target="$1"
  local app_target="$2"
  local arch="$3"
  if [[ -n "${readpst_target}" && -n "${app_target}" && "${readpst_target}" == "${app_target}" ]]; then
    pass "ReadPST ${arch} deployment target matches the application (${app_target})"
  else
    fail "ReadPST ${arch} deployment target (${readpst_target:-unknown}) does not match the application (${app_target:-unknown})"
  fi
}

check_source_license() {
  local relative="$1" expected="$2" label="$3"
  local path="${ROOT_DIR}/${relative}"
  if [[ ! -s "${path}" ]]; then
    fail "${label} missing or empty: ${path}"
    return
  fi
  local actual
  actual="$(shasum -a 256 "${path}" | awk '{print $1}')"
  if [[ "${actual}" == "${expected}" ]]; then
    pass "${label} matches the official text"
  else
    fail "${label} hash mismatch: ${actual}"
  fi
}

check_packaged_resource() {
  local source_relative="$1" resource_relative="$2" label="$3"
  local source="${ROOT_DIR}/${source_relative}"
  local packaged="${APP_PATH}/Contents/Resources/${resource_relative}"
  if [[ ! -s "${packaged}" ]]; then
    fail "packaged ${label} is missing or empty: ${packaged}"
  elif cmp -s "${source}" "${packaged}"; then
    pass "packaged ${label} matches the repository source"
  else
    fail "packaged ${label} differs from ${source_relative}"
  fi
}

check_readpst_corresponding_source() {
  local directory="$1"
  if [[ -z "${directory}" || ! -d "${directory}" ]]; then
    fail "public release requires READPST_CORRESPONDING_SOURCE_DIR with the complete ReadPST companion"
    return
  fi
  if READPST_EXPECTED_PUBLIC_DOWNLOAD_LOCATION="${EXPECTED_READPST_COMPANION_URL}" \
    bash "${ROOT_DIR}/scripts/verify-readpst-corresponding-source.sh" \
    "${directory}" "${ROOT_DIR}"; then
    pass "ReadPST Corresponding Source companion, manifest, archive, and sidecar binding are complete"
  else
    fail "ReadPST Corresponding Source companion verification failed"
  fi
}

for tool in plutil lipo otool file grep find python3 shasum cmp xcrun; do
  require_command "${tool}"
done
if ! xcrun --find vtool >/dev/null 2>&1; then
  fail "required Apple tool missing: vtool"
fi

PACKAGE_JSON="${ROOT_DIR}/package.json"
PACKAGE_LOCK="${ROOT_DIR}/package-lock.json"
CARGO_TOML="${ROOT_DIR}/src-tauri/Cargo.toml"
CARGO_LOCK="${ROOT_DIR}/src-tauri/Cargo.lock"
TAURI_CONFIG="${ROOT_DIR}/src-tauri/tauri.conf.json"
CAPABILITY="${ROOT_DIR}/src-tauri/capabilities/default.json"
APP_SOURCE="${ROOT_DIR}/src/App.tsx"
RUST_SOURCE="${ROOT_DIR}/src-tauri/src/lib.rs"

for public_file in \
  LICENSE COPYRIGHT.md README.md SECURITY.md CONTRIBUTING.md CODE_OF_CONDUCT.md \
  PUBLIC_GITHUB_AUDIT.md THIRD_PARTY_NOTICES.md \
  LICENSES/GPL-3.0-or-later.txt LICENSES/GPL-2.0-or-later.txt \
  docs/PUBLIC_HISTORY.md docs/LICENSE_DECISION.md docs/RELEASE_COMPLIANCE.md \
  docs/READPST_CORRESPONDING_SOURCE.md \
  .github/workflows/ci.yml .github/workflows/release-check.yml
do
  if [[ -s "${ROOT_DIR}/${public_file}" ]]; then
    pass "required public file exists: ${public_file}"
  else
    fail "required public file missing or empty: ${public_file}"
  fi
done

check_source_license "LICENSE" "${EXPECTED_GPL3_SHA256}" "root GPL-3.0 license"
check_source_license "LICENSES/GPL-3.0-or-later.txt" "${EXPECTED_GPL3_SHA256}" \
  "GPL-3.0-or-later license copy"
check_source_license "LICENSES/GPL-2.0-or-later.txt" "${EXPECTED_GPL2_SHA256}" \
  "GPL-2.0-or-later license copy"
if cmp -s "${ROOT_DIR}/LICENSE" "${ROOT_DIR}/LICENSES/GPL-3.0-or-later.txt"; then
  pass "root LICENSE is byte-identical to GPL-3.0-or-later.txt"
else
  fail "root LICENSE differs from LICENSES/GPL-3.0-or-later.txt"
fi

if python3 - "${EXPECTED_VERSION}" "${EXPECTED_PROJECT_LICENSE}" "${EXPECTED_REPOSITORY}" "${PACKAGE_JSON}" "${PACKAGE_LOCK}" "${TAURI_CONFIG}" "${CARGO_TOML}" "${CARGO_LOCK}" <<'PY'
import json
import pathlib
import re
import sys

expected, expected_license, expected_repository, package_path, lock_path, tauri_path, cargo_path, cargo_lock_path = sys.argv[1:]
package = json.loads(pathlib.Path(package_path).read_text())
package_lock = json.loads(pathlib.Path(lock_path).read_text())
tauri = json.loads(pathlib.Path(tauri_path).read_text())
cargo = pathlib.Path(cargo_path).read_text()
cargo_lock = pathlib.Path(cargo_lock_path).read_text()

checks = {
    "package.json": package.get("version"),
    "package-lock.json": package_lock.get("version"),
    "package-lock root package": package_lock.get("packages", {}).get("", {}).get("version"),
    "tauri.conf.json": tauri.get("version"),
}
for label, value in checks.items():
    if value != expected:
        raise SystemExit(f"{label} version is {value!r}, expected {expected!r}")

license_checks = {
    "package.json": package.get("license"),
    "package-lock root package": package_lock.get("packages", {}).get("", {}).get("license"),
}
for label, value in license_checks.items():
    if value != expected_license:
        raise SystemExit(f"{label} license is {value!r}, expected {expected_license!r}")

package_repository = package.get("repository", {})
if package_repository.get("url", "").removesuffix(".git") != expected_repository:
    raise SystemExit("package.json repository URL does not match")

match = re.search(r'(?ms)^\[package\].*?^version\s*=\s*"([^"]+)"', cargo)
if not match or match.group(1) != expected:
    raise SystemExit("Cargo.toml package version does not match")
license_match = re.search(r'(?ms)^\[package\].*?^license\s*=\s*"([^"]+)"', cargo)
if not license_match or license_match.group(1) != expected_license:
    raise SystemExit("Cargo.toml package license does not match")
repository_match = re.search(r'(?ms)^\[package\].*?^repository\s*=\s*"([^"]+)"', cargo)
if not repository_match or repository_match.group(1).removesuffix(".git") != expected_repository:
    raise SystemExit("Cargo.toml repository URL does not match")
if not re.search(rf'(?ms)^name = "pst-quickview"\nversion = "{re.escape(expected)}"', cargo_lock):
    raise SystemExit("Cargo.lock pst-quickview package version does not match")

pins = {
    "time": "=0.3.51",
    "msg_parser": "=0.3.6",
    "cfb": "=0.7.3",
}
for name, version in pins.items():
    pattern = rf'(?m)^{re.escape(name)}\s*=\s*"{re.escape(version)}"$'
    if not re.search(pattern, cargo):
        raise SystemExit(f"dependency pin missing: {name} = {version!r}")

macos = tauri.get("bundle", {}).get("macOS", {})
if macos.get("bundleVersion") != "0.2.0.2":
    raise SystemExit("macOS bundleVersion must be 0.2.0.2")
if macos.get("hardenedRuntime") is not True:
    raise SystemExit("macOS hardenedRuntime must be true")
build = tauri.get("build", {})
if "devUrl" in build or "beforeDevCommand" in build:
    raise SystemExit("release tauri.conf.json contains development-only build settings")
expected_resources = {
    "../LICENSE": "LICENSE",
    "../THIRD_PARTY_NOTICES.md": "THIRD_PARTY_NOTICES.md",
    "../LICENSES/GPL-3.0-or-later.txt": "LICENSES/GPL-3.0-or-later.txt",
    "../LICENSES/GPL-2.0-or-later.txt": "LICENSES/GPL-2.0-or-later.txt",
}
if tauri.get("bundle", {}).get("resources") != expected_resources:
    raise SystemExit("Tauri legal-resource mapping is incomplete or unexpected")
PY
then
  pass "source versions, licenses, bundle settings, resources, and dependency pins are consistent"
else
  fail "source version or dependency-pin verification failed"
fi

if grep -q 'const appVersion = packageInfo.version;' "${APP_SOURCE}"; then
  pass "Help/About version is sourced from package.json"
else
  fail "Help/About version is not sourced from package.json"
fi

if grep -q '<dt>PST QuickView license</dt>' "${APP_SOURCE}" &&
  grep -q '<dd>GPL-3.0-or-later</dd>' "${APP_SOURCE}" &&
  grep -q '<dt>ReadPST/LibPST license</dt>' "${APP_SOURCE}" &&
  grep -q '<dd>GPL-2.0-or-later</dd>' "${APP_SOURCE}" &&
  grep -q 'invoke("reveal_project_license")' "${APP_SOURCE}" &&
  grep -q 'invoke("reveal_third_party_notices")' "${APP_SOURCE}" &&
  grep -q 'reveal_project_license,' "${RUST_SOURCE}" &&
  grep -q 'reveal_third_party_notices,' "${RUST_SOURCE}"; then
  pass "Help/About presents project and ReadPST licenses with local reveal actions"
else
  fail "Help/About licensing presentation or local reveal actions are incomplete"
fi

if [[ -d "${APP_PATH}" ]]; then
  pass "app bundle exists: ${APP_PATH}"
else
  fail "app bundle missing: ${APP_PATH}"
fi

check_packaged_resource "LICENSE" "LICENSE" "project license"
check_packaged_resource "THIRD_PARTY_NOTICES.md" "THIRD_PARTY_NOTICES.md" \
  "third-party notices"
check_packaged_resource "LICENSES/GPL-3.0-or-later.txt" \
  "LICENSES/GPL-3.0-or-later.txt" "GPL-3.0-or-later text"
check_packaged_resource "LICENSES/GPL-2.0-or-later.txt" \
  "LICENSES/GPL-2.0-or-later.txt" "GPL-2.0-or-later text"

PLIST="${APP_PATH}/Contents/Info.plist"
APP_EXECUTABLE=""
PLIST_MINIMUM=""
if [[ -f "${PLIST}" ]]; then
  if plutil -lint "${PLIST}" >/dev/null; then
    pass "Info.plist is valid"
  else
    fail "Info.plist is invalid"
  fi
  APP_EXECUTABLE="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "${PLIST}" 2>/dev/null || true)"
  PLIST_MINIMUM="$(/usr/libexec/PlistBuddy -c 'Print :LSMinimumSystemVersion' "${PLIST}" 2>/dev/null || true)"
  check_target_value "${PLIST_MINIMUM}" "${EXPECTED_INFO_PLIST_TARGET}" \
    "Info.plist LSMinimumSystemVersion"
  if python3 - "${PLIST}" "${EXPECTED_VERSION}" "${EXPECTED_BUNDLE_VERSION}" <<'PY'
import plistlib
import sys

path, expected_version, expected_bundle_version = sys.argv[1:]
with open(path, "rb") as handle:
    plist = plistlib.load(handle)

if plist.get("CFBundleShortVersionString") != expected_version:
    raise SystemExit("CFBundleShortVersionString mismatch")
if plist.get("CFBundleVersion") != expected_bundle_version:
    raise SystemExit("CFBundleVersion mismatch")
if plist.get("CFBundleName") != "PST QuickView":
    raise SystemExit("CFBundleName mismatch")

expected = {
    "eml": ("public.email-message", "RFC 822 Email Message"),
    "msg": ("com.microsoft.outlook.msg", "Microsoft Outlook Message"),
    "pst": ("com.microsoft.outlook.pst", "Microsoft Outlook Personal Storage Table"),
}
found = {}
for item in plist.get("CFBundleDocumentTypes", []):
    for extension in item.get("CFBundleTypeExtensions", []):
        found[extension.lower()] = item
for extension, (content_type, description) in expected.items():
    item = found.get(extension)
    if not item:
        raise SystemExit(f"missing document declaration for {extension}")
    if item.get("CFBundleTypeRole") != "Viewer":
        raise SystemExit(f"{extension} role is not Viewer")
    if item.get("LSHandlerRank") != "Default":
        raise SystemExit(f"{extension} handler rank is not Default")
    if content_type not in item.get("LSItemContentTypes", []):
        raise SystemExit(f"{extension} content type is missing")
    if item.get("CFBundleTypeName") != description:
        raise SystemExit(f"{extension} description mismatch")

imports = {
    item.get("UTTypeIdentifier"): item
    for item in plist.get("UTImportedTypeDeclarations", [])
}
for identifier, extension in [
    ("com.microsoft.outlook.msg", "msg"),
    ("com.microsoft.outlook.pst", "pst"),
]:
    item = imports.get(identifier)
    if not item:
        raise SystemExit(f"missing imported UTI {identifier}")
    tags = item.get("UTTypeTagSpecification", {})
    extensions = tags.get("public.filename-extension", [])
    if isinstance(extensions, str):
        extensions = [extensions]
    if extension not in extensions:
        raise SystemExit(f"imported UTI {identifier} does not declare {extension}")
PY
  then
    pass "generated Info.plist version and PST/EML/MSG declarations are correct"
  else
    fail "generated Info.plist metadata verification failed"
  fi
else
  fail "Info.plist missing: ${PLIST}"
fi

if [[ -n "${APP_EXECUTABLE}" ]]; then
  APP_BINARY="${APP_PATH}/Contents/MacOS/${APP_EXECUTABLE}"
else
  APP_BINARY="${APP_PATH}/Contents/MacOS/pst-quickview"
fi
PACKAGED_READPST="${APP_PATH}/Contents/MacOS/readpst"
check_arches "${APP_BINARY}" universal "main app executable"
check_arches "${PACKAGED_READPST}" universal "packaged readpst"
check_dependencies "${APP_BINARY}" "main app executable"
check_dependencies "${PACKAGED_READPST}" "packaged readpst"

APP_X86_64_TARGET="$(deployment_target_for_arch "${APP_BINARY}" x86_64 || true)"
APP_ARM64_TARGET="$(deployment_target_for_arch "${APP_BINARY}" arm64 || true)"
PACKAGED_READPST_X86_64_TARGET="$(deployment_target_for_arch "${PACKAGED_READPST}" x86_64 || true)"
PACKAGED_READPST_ARM64_TARGET="$(deployment_target_for_arch "${PACKAGED_READPST}" arm64 || true)"
check_target_value "${APP_X86_64_TARGET}" "${EXPECTED_X86_64_TARGET}" \
  "main app x86_64"
check_target_value "${APP_ARM64_TARGET}" "${EXPECTED_ARM64_TARGET}" \
  "main app arm64"
check_target_value "${PACKAGED_READPST_X86_64_TARGET}" "${EXPECTED_X86_64_TARGET}" \
  "packaged ReadPST x86_64"
check_target_value "${PACKAGED_READPST_ARM64_TARGET}" "${EXPECTED_ARM64_TARGET}" \
  "packaged ReadPST arm64"
check_matching_targets "${PACKAGED_READPST_X86_64_TARGET}" "${APP_X86_64_TARGET}" x86_64
check_matching_targets "${PACKAGED_READPST_ARM64_TARGET}" "${APP_ARM64_TARGET}" arm64

SOURCE_BINARIES="${ROOT_DIR}/src-tauri/binaries"
SOURCE_X86_64="${SOURCE_BINARIES}/readpst-x86_64-apple-darwin"
SOURCE_ARM64="${SOURCE_BINARIES}/readpst-aarch64-apple-darwin"
SOURCE_UNIVERSAL="${SOURCE_BINARIES}/readpst-universal-apple-darwin"
check_arches "${SOURCE_X86_64}" x86_64 "source Intel readpst"
check_arches "${SOURCE_ARM64}" arm64 "source Apple Silicon readpst"
check_arches "${SOURCE_UNIVERSAL}" universal "source universal readpst"
for sidecar in \
  "${SOURCE_X86_64}" \
  "${SOURCE_ARM64}" \
  "${SOURCE_UNIVERSAL}"
do
  check_dependencies "${sidecar}" "$(basename "${sidecar}")"
done

SOURCE_X86_64_TARGET="$(deployment_target_for_arch "${SOURCE_X86_64}" x86_64 || true)"
SOURCE_ARM64_TARGET="$(deployment_target_for_arch "${SOURCE_ARM64}" arm64 || true)"
SOURCE_UNIVERSAL_X86_64_TARGET="$(deployment_target_for_arch "${SOURCE_UNIVERSAL}" x86_64 || true)"
SOURCE_UNIVERSAL_ARM64_TARGET="$(deployment_target_for_arch "${SOURCE_UNIVERSAL}" arm64 || true)"
check_target_value "${SOURCE_X86_64_TARGET}" "${EXPECTED_X86_64_TARGET}" \
  "source Intel ReadPST"
check_target_value "${SOURCE_ARM64_TARGET}" "${EXPECTED_ARM64_TARGET}" \
  "source Apple Silicon ReadPST"
check_target_value "${SOURCE_UNIVERSAL_X86_64_TARGET}" "${EXPECTED_X86_64_TARGET}" \
  "source universal ReadPST x86_64"
check_target_value "${SOURCE_UNIVERSAL_ARM64_TARGET}" "${EXPECTED_ARM64_TARGET}" \
  "source universal ReadPST arm64"
check_matching_targets "${SOURCE_X86_64_TARGET}" "${APP_X86_64_TARGET}" x86_64
check_matching_targets "${SOURCE_ARM64_TARGET}" "${APP_ARM64_TARGET}" arm64

if [[ "${SOURCE_UNIVERSAL_X86_64_TARGET}" == "${SOURCE_X86_64_TARGET}" ]]; then
  pass "universal ReadPST x86_64 target matches the standalone sidecar"
else
  fail "universal ReadPST x86_64 target does not match the standalone sidecar"
fi
if [[ "${SOURCE_UNIVERSAL_ARM64_TARGET}" == "${SOURCE_ARM64_TARGET}" ]]; then
  pass "universal ReadPST arm64 target matches the standalone sidecar"
else
  fail "universal ReadPST arm64 target does not match the standalone sidecar"
fi

if [[ -f "${PACKAGED_READPST}" ]] &&
  cmp -s "${PACKAGED_READPST}" "${SOURCE_BINARIES}/readpst-universal-apple-darwin"; then
  pass "packaged readpst is byte-identical to the verified source universal sidecar"
else
  fail "packaged readpst differs from src-tauri/binaries/readpst-universal-apple-darwin"
fi

if [[ -x "${PACKAGED_READPST}" ]] &&
  PACKAGED_READPST_VERSION_OUTPUT="$("${PACKAGED_READPST}" -V 2>&1)" &&
  [[ "${PACKAGED_READPST_VERSION_OUTPUT}" == *"ReadPST / LibPST v${EXPECTED_READPST_VERSION}"* ]]; then
  pass "packaged ReadPST reports version ${EXPECTED_READPST_VERSION} and executes without Homebrew"
else
  fail "packaged ReadPST -V failed or reported an unexpected version"
fi

if python3 - "${CAPABILITY}" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    capability = json.load(handle)
windows = set(capability.get("windows", []))
permissions = set(capability.get("permissions", []))
required_windows = {"main", "message-preview-*"}
required_permissions = {
    "core:default",
    "core:webview:allow-create-webview-window",
    "core:window:allow-show",
    "core:window:allow-unminimize",
    "core:window:allow-set-focus",
    "core:window:allow-set-title",
}
if windows != required_windows:
    raise SystemExit(f"window scope is not narrow: {sorted(windows)}")
if not required_permissions.issubset(permissions):
    raise SystemExit("required preview-window permissions are missing")
if any("shell" in permission or "fs:" in permission for permission in permissions):
    raise SystemExit("unexpected broad shell/filesystem capability")
PY
then
  pass "Tauri capability scope is limited to main and message preview windows"
else
  fail "Tauri capability verification failed"
fi

if [[ -d "${APP_PATH}/Contents" ]]; then
  if grep -R -a -E '127\.0\.0\.1:1420|localhost:1420|/@vite/client|vite/dist/client' \
    "${APP_PATH}/Contents" >/dev/null 2>&1; then
    fail "packaged app contains a development-server reference"
  else
    pass "packaged app contains no localhost or Vite development-server references"
  fi
  if [[ -d "${ROOT_DIR}/dist" ]] && grep -R -a -E \
    '/Users/notroot|/Volumes/T7|PST_QUICKVIEW_(TREVOR|FURMAN)_MSG_FIXTURE' \
    "${ROOT_DIR}/dist" >/dev/null 2>&1; then
    fail "packaged frontend assets contain a private path or deprecated private fixture identifier"
  else
    pass "packaged frontend assets contain no configured private paths or fixture identifiers"
  fi
  if [[ -f "${APP_BINARY}" ]] &&
    grep -a -F "${EXPECTED_VERSION}" "${APP_BINARY}" >/dev/null 2>&1; then
    pass "packaged frontend contains the expected Help/About version"
  else
    fail "packaged frontend does not contain the expected Help/About version"
  fi
fi

if [[ -d "${APP_PATH}" ]]; then
  if codesign -dv --verbose=2 "${APP_PATH}" >/dev/null 2>&1; then
    printf 'WARN: app has a code signature; confirm its identity and release authorization manually.\n'
  else
    pass "app remains unsigned as documented for this beta candidate"
  fi
  printf 'WARN: notarization is not performed or claimed by this verification script.\n'
fi

BUNDLE_DIR="$(dirname "$(dirname "${APP_PATH}")")"
DMG_DIR="${BUNDLE_DIR}/dmg"
DMG_PATH="$(find "${DMG_DIR}" -maxdepth 1 -type f -name "PST QuickView_${EXPECTED_VERSION}_*.dmg" -print -quit 2>/dev/null || true)"
if [[ "${EXPECT_DMG}" == "1" ]]; then
  if [[ -n "${DMG_PATH}" && -f "${DMG_PATH}" ]]; then
    pass "versioned DMG exists: ${DMG_PATH}"
  else
    fail "versioned DMG was not found under ${DMG_DIR}"
  fi
else
  pass "DMG check disabled with EXPECT_DMG=${EXPECT_DMG}"
fi

if [[ "${PUBLIC_RELEASE}" == "1" ]]; then
  check_readpst_corresponding_source "${READPST_CORRESPONDING_SOURCE_DIR}"
else
  printf 'WARN: public ReadPST Corresponding Source gate not run; set PUBLIC_RELEASE=1 and READPST_CORRESPONDING_SOURCE_DIR before public binary publication.\n'
fi

if (( FAILURES > 0 )); then
  printf 'Release verification failed with %d required check(s).\n' "${FAILURES}" >&2
  exit 1
fi

printf 'All PST QuickView macOS release checks passed for %s.\n' "${EXPECTED_VERSION}"
