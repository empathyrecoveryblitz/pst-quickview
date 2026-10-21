#!/usr/bin/env bash
set -uo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DYLIB_VALIDATION_SCRIPT="${ROOT_DIR}/scripts/macos-dylib-validation.sh"
if [[ ! -r "${DYLIB_VALIDATION_SCRIPT}" ]]; then
  printf 'Required dylib validation helper is missing: %s\n' \
    "${DYLIB_VALIDATION_SCRIPT}" >&2
  exit 1
fi
# shellcheck source=macos-dylib-validation.sh
source "${DYLIB_VALIDATION_SCRIPT}"
EXPECTED_VERSION="${PST_QUICKVIEW_EXPECTED_VERSION:-0.2.0-beta.3}"
EXPECTED_BUNDLE_VERSION="${PST_QUICKVIEW_EXPECTED_BUNDLE_VERSION:-0.2.0.3}"
EXPECTED_PRODUCT_NAME="${PST_QUICKVIEW_EXPECTED_PRODUCT_NAME:-PST QuickView}"
EXPECTED_IDENTIFIER="${PST_QUICKVIEW_EXPECTED_IDENTIFIER:-com.local.pstquickview}"
PRODUCTION_IDENTIFIER="com.local.pstquickview"
RUNTIME_PROFILE_CONTRACT="pst-quickview-runtime-profile-v1:alternate-identifier-scoped:no-production-fallback"
RUNTIME_PROFILE_IDENTITY_CONTRACT_PREFIX="pst-quickview-runtime-identity-v1:"
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
DEFAULT_APP_PATH="${ROOT_DIR}/src-tauri/target/universal-apple-darwin/release/bundle/macos/${EXPECTED_PRODUCT_NAME}.app"
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
  if pq_validate_macho_system_dependencies "${binary}" "${label}"; then
    pass "${label} uses only permitted macOS system dylibs/frameworks"
  else
    fail "${label} has prohibited or unreadable dynamic dependencies"
  fi
}

check_no_private_build_paths() {
  local binary="$1"
  local arch="$2"
  local label="$3"
  if [[ ! -f "${binary}" ]]; then
    fail "${label} is missing; private build-path scan cannot run"
    return
  fi

  local scan_statuses
  if strings -a -arch "${arch}" -- "${binary}" |
    grep -E '(^|file://|[^[:alnum:]_./-])(/Users/[^/[:space:]]+|/home/[^/[:space:]]+|/Volumes/[^/[:space:]]+/|[A-Za-z]:\\Users\\)' \
      >/dev/null
  then
    scan_statuses=("${PIPESTATUS[@]}")
  else
    scan_statuses=("${PIPESTATUS[@]}")
  fi
  if [[ "${scan_statuses[0]}" -ne 0 ]]; then
    fail "${label} could not be inspected for private absolute build paths"
  elif [[ "${scan_statuses[1]}" -eq 0 ]]; then
    fail "${label} contains a private absolute build path"
  elif [[ "${scan_statuses[1]}" -eq 1 ]]; then
    pass "${label} contains no private absolute build paths"
  else
    fail "${label} private build-path scan failed closed"
  fi
}

check_runtime_profile_contract() {
  local binary="$1"
  local arch="$2"
  local expected_identifier="$3"
  local label="$4"
  if [[ ! -f "${binary}" ]]; then
    fail "${label} is missing; runtime-profile scan cannot run"
    return
  fi

  local expected_identity_contract
  if ! expected_identity_contract="$(
    python3 - "${RUNTIME_PROFILE_IDENTITY_CONTRACT_PREFIX}" \
      "${expected_identifier}" <<'PY'
import re
import sys

prefix, identifier = sys.argv[1:]
# The Tauri Android-name build variables are an exact encoding for the reviewed
# dot-separated alphanumeric identifier grammar. Fail closed on other bundle
# identifier forms instead of accepting a potentially lossy static binding.
if len(identifier) > 255 or not re.fullmatch(r"[A-Za-z0-9]+(?:\.[A-Za-z0-9]+)+", identifier):
    raise SystemExit(1)
segments = identifier.split(".")
android_prefix = "_".join(segments[:-1])
android_app_name = segments[-1]
print(f"{prefix}{android_prefix}:{android_app_name}:identity-end")
PY
  )"
  then
    fail "${label} expected identifier cannot be encoded for runtime-profile verification"
    return
  fi

  local scan_statuses
  if strings -a -arch "${arch}" -- "${binary}" |
    grep -F -- "${expected_identity_contract}" >/dev/null
  then
    scan_statuses=("${PIPESTATUS[@]}")
  else
    scan_statuses=("${PIPESTATUS[@]}")
  fi
  if [[ "${scan_statuses[0]}" -ne 0 ]]; then
    fail "${label} could not be inspected for its compiled runtime identity"
  elif [[ "${scan_statuses[1]}" -eq 0 ]]; then
    pass "${label} compiled runtime identity matches the packaged identifier"
  elif [[ "${scan_statuses[1]}" -eq 1 ]]; then
    fail "${label} compiled runtime identity does not match the packaged identifier"
  else
    fail "${label} compiled runtime-identity scan failed closed"
  fi

  if [[ "${expected_identifier}" == "${PRODUCTION_IDENTIFIER}" ]]; then
    pass "${label} uses the production compatibility profile contract"
    return
  fi

  if strings -a -arch "${arch}" -- "${binary}" |
    grep -F -- "${RUNTIME_PROFILE_CONTRACT}" >/dev/null
  then
    scan_statuses=("${PIPESTATUS[@]}")
  else
    scan_statuses=("${PIPESTATUS[@]}")
  fi
  if [[ "${scan_statuses[0]}" -ne 0 ]]; then
    fail "${label} could not be inspected for its runtime-profile contract"
  elif [[ "${scan_statuses[1]}" -eq 0 ]]; then
    pass "${label} embeds the identifier-scoped runtime-profile contract"
  else
    fail "${label} does not embed the identifier-scoped runtime-profile contract"
  fi

  if strings -a -arch "${arch}" -- "${binary}" |
    grep -F -- 'Library/Application Support/PST QuickView' \
      >/dev/null
  then
    scan_statuses=("${PIPESTATUS[@]}")
  else
    scan_statuses=("${PIPESTATUS[@]}")
  fi
  if [[ "${scan_statuses[0]}" -ne 0 ]]; then
    fail "${label} could not be inspected for a production-profile destination"
  elif [[ "${scan_statuses[1]}" -eq 0 ]]; then
    fail "${label} contains a usable production-profile destination"
  elif [[ "${scan_statuses[1]}" -eq 1 ]]; then
    pass "${label} contains no usable production-profile destination"
  else
    fail "${label} production-profile destination scan failed closed"
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

for tool in plutil lipo otool file grep find python3 shasum cmp strings xcrun; do
  require_command "${tool}"
done
if ! xcrun --find vtool >/dev/null 2>&1; then
  fail "required Apple tool missing: vtool"
fi

PACKAGE_JSON="${ROOT_DIR}/package.json"
PACKAGE_LOCK="${ROOT_DIR}/package-lock.json"
CARGO_TOML="${ROOT_DIR}/src-tauri/Cargo.toml"
CARGO_LOCK="${ROOT_DIR}/src-tauri/Cargo.lock"
INHERITED_TAURI_CONFIG="${TAURI_CONFIG-}"
TAURI_CONFIG="${ROOT_DIR}/src-tauri/tauri.conf.json"
CAPABILITIES_DIR="${ROOT_DIR}/src-tauri/capabilities"
BUILD_SCRIPT="${ROOT_DIR}/src-tauri/build.rs"
TAURI_BUILD_WRAPPER="${ROOT_DIR}/scripts/tauri.sh"
APP_SOURCE="${ROOT_DIR}/src/App.tsx"
RUST_SOURCE="${ROOT_DIR}/src-tauri/src/lib.rs"
RUNTIME_PROFILE_SOURCE="${ROOT_DIR}/src-tauri/src/runtime_profile.rs"
SUPPORT_RUST_SOURCE="${ROOT_DIR}/src-tauri/src/support_session.rs"
SUPPORT_PANEL_SOURCE="${ROOT_DIR}/src/RemoteSupportPanel.tsx"
SUPPORT_CONTROL_SOURCE="${ROOT_DIR}/src/RemoteSupportControl.tsx"
SUPPORT_SESSION_SOURCE="${ROOT_DIR}/public/support/session.js"
SUPPORT_PROTOCOL_SOURCE="${ROOT_DIR}/public/support/protocol.js"
SUPPORT_PEER_SOURCE="${ROOT_DIR}/public/support/peer.js"
SUPPORT_VIEW_SOURCE="${ROOT_DIR}/src/supportView.ts"
SUPPORT_SESSION_DOCUMENT="${ROOT_DIR}/public/support/session.html"

if [[ -n "${INHERITED_TAURI_CONFIG}" ]]; then
  fail "TAURI_CONFIG build override is set; packaged Tauri authority cannot be tied to the reviewed config"
else
  pass "TAURI_CONFIG build override is not set during release verification"
fi

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
if macos.get("bundleVersion") != "0.2.0.3":
    raise SystemExit("macOS bundleVersion must be 0.2.0.3")
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
  if python3 - "${PLIST}" "${EXPECTED_VERSION}" "${EXPECTED_BUNDLE_VERSION}" \
    "${EXPECTED_PRODUCT_NAME}" "${EXPECTED_IDENTIFIER}" <<'PY'
import plistlib
import sys

path, expected_version, expected_bundle_version, expected_product_name, expected_identifier = sys.argv[1:]
with open(path, "rb") as handle:
    plist = plistlib.load(handle)

if plist.get("CFBundleShortVersionString") != expected_version:
    raise SystemExit("CFBundleShortVersionString mismatch")
if plist.get("CFBundleVersion") != expected_bundle_version:
    raise SystemExit("CFBundleVersion mismatch")
if plist.get("CFBundleName") != expected_product_name:
    raise SystemExit("CFBundleName mismatch")
if plist.get("CFBundleIdentifier") != expected_identifier:
    raise SystemExit("CFBundleIdentifier mismatch")

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
for packaged_arch in x86_64 arm64; do
  check_no_private_build_paths "${APP_BINARY}" "${packaged_arch}" \
    "main app ${packaged_arch} slice"
  check_runtime_profile_contract "${APP_BINARY}" "${packaged_arch}" \
    "${EXPECTED_IDENTIFIER}" "main app ${packaged_arch} slice"
  check_no_private_build_paths "${PACKAGED_READPST}" "${packaged_arch}" \
    "packaged ReadPST ${packaged_arch} slice"
done
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

if python3 - "${CAPABILITIES_DIR}" "${TAURI_CONFIG}" "${BUILD_SCRIPT}" "${RUST_SOURCE}" "${APP_SOURCE}" <<'PY'
import json
import hashlib
import re
import sys
from pathlib import Path

capabilities_dir = Path(sys.argv[1])
tauri_config_path = Path(sys.argv[2])
build_script_path = Path(sys.argv[3])
rust_source_path = Path(sys.argv[4])
app_source_path = Path(sys.argv[5])

if tauri_config_path.is_symlink() or not tauri_config_path.is_file():
    raise SystemExit("production Tauri config must be a regular, non-symlink file")
production_config_overrides = {
    "Tauri.toml",
    "Tauri.macos.toml",
    "tauri.conf.json5",
    "tauri.macos.conf.json",
    "tauri.macos.conf.json5",
}
present_overrides = sorted(
    name for name in production_config_overrides if (tauri_config_path.parent / name).exists()
)
if present_overrides:
    raise SystemExit(f"production Tauri config override is present: {present_overrides}")

if capabilities_dir.is_symlink() or not capabilities_dir.is_dir():
    raise SystemExit("capabilities root must be a regular, non-symlink directory")

expected_entries = {
    ("main.json", "file"),
    ("message-preview.json", "file"),
    ("support-session.json", "file"),
}
actual_entries = set()
for path in capabilities_dir.rglob("*"):
    relative = path.relative_to(capabilities_dir).as_posix()
    if path.is_symlink():
        kind = "symlink"
    elif path.is_dir():
        kind = "directory"
    elif path.is_file():
        kind = "file"
    else:
        kind = "other"
    actual_entries.add((relative, kind))
if actual_entries != expected_entries:
    raise SystemExit(
        f"capability entries are not the exact reviewed set: {sorted(actual_entries)}"
    )

capabilities = {}
for filename, _ in sorted(expected_entries):
    path = capabilities_dir / filename
    with path.open(encoding="utf-8") as handle:
        capability = json.load(handle)
    capabilities[capability.get("identifier")] = capability

if set(capabilities) != {"main", "message-preview", "support-session"}:
    raise SystemExit(f"capability identifiers are unexpected: {sorted(capabilities)}")

main = capabilities["main"]
preview = capabilities["message-preview"]
support = capabilities["support-session"]
expected_capability_keys = {
    "$schema",
    "description",
    "identifier",
    "permissions",
    "windows",
}
if main.get("windows") != ["main"]:
    raise SystemExit("main capability must target exactly the main window")
if preview.get("windows") != ["message-preview-*"]:
    raise SystemExit("preview capability must target exactly message-preview-* windows")
if support.get("windows") != ["support-session-*"] or support.get("local") is not True:
    raise SystemExit("support capability must target exactly local support-session-* windows")
for identifier, capability in capabilities.items():
    reviewed_keys = expected_capability_keys | ({"local"} if identifier == "support-session" else set())
    if set(capability) != reviewed_keys:
        raise SystemExit(
            f"{identifier} capability fields are not the exact reviewed set: "
            f"{sorted(capability)}"
        )
    permissions = capability.get("permissions")
    if not isinstance(permissions, list):
        raise SystemExit(f"{identifier} capability permissions are not a list")
    if not all(isinstance(permission, str) for permission in permissions):
        raise SystemExit(f"{identifier} capability contains a scoped permission object")
    if len(permissions) != len(set(permissions)):
        raise SystemExit(f"{identifier} capability contains duplicate permissions")
    if any("*" in permission for permission in permissions):
        raise SystemExit(f"{identifier} capability contains a wildcard permission")
    if any(permission == "core:default" or "shell" in permission or "fs:" in permission
           for permission in permissions):
        raise SystemExit(f"{identifier} capability contains broad default, shell, or filesystem authority")

main_core_permissions = {
    "core:event:allow-listen",
    "core:event:allow-unlisten",
}
preview_core_permissions = {
    "core:window:allow-set-title",
}

build_source = build_script_path.read_text(encoding="utf-8")
permission_generator_start = build_source.find("fn write_app_permissions")
if permission_generator_start < 0:
    raise SystemExit("build.rs application permission generator was not found")
permission_generator_source = build_source[permission_generator_start:]
expected_permission_generator_sha256 = (
    "75fea422fd830395adb5e7564a49e012a3fd07726a0b089a737593f116f66211"
)
actual_permission_generator_sha256 = hashlib.sha256(
    permission_generator_source.encode()
).hexdigest()
if actual_permission_generator_sha256 != expected_permission_generator_sha256:
    raise SystemExit(
        "application permission generation or registration drifted from the reviewed "
        "one-to-one implementation"
    )
command_match = re.search(
    r"const APP_COMMANDS:\s*&\[&str\]\s*=\s*&\[(.*?)\];",
    build_source,
    re.DOTALL,
)
if command_match is None:
    raise SystemExit("build.rs does not declare the generated application-command ACL")
app_commands = re.findall(r'"([a-z][a-z0-9_]*)"', command_match.group(1))
if not app_commands or len(app_commands) != len(set(app_commands)):
    raise SystemExit("generated application-command ACL is empty or contains duplicates")

rust_source = rust_source_path.read_text(encoding="utf-8")
if (
    rust_source.count("tauri::generate_context!") != 1
    or re.search(
        r"\blet\s+context\s*=\s*tauri::generate_context!\(\)\s*;",
        rust_source,
    ) is None
    or "RuntimeProfileIdentity::from_config(context.config())" not in rust_source
    or re.search(r"\.build\(\s*context\s*\)", rust_source) is None
):
    raise SystemExit(
        "application must validate and build exactly once with the reviewed merged Tauri context"
    )
handler_match = re.search(
    r"\.invoke_handler\(tauri::generate_handler!\[(.*?)\]\)",
    rust_source,
    re.DOTALL,
)
if handler_match is None:
    raise SystemExit("registered Tauri command handler list was not found")
command_paths = handler_match.group(1).split(",")
if command_paths[-1].strip() == "":
    command_paths.pop()
registered_commands = []
for command_path in command_paths:
    match = re.fullmatch(r"(?:[a-z][a-z0-9_]*::)*([a-z][a-z0-9_]*)", command_path.strip())
    if match is None:
        raise SystemExit("registered application command is not an exact Rust command path")
    registered_commands.append(match.group(1))
if set(app_commands) != set(registered_commands) or len(app_commands) != len(registered_commands):
    missing = sorted(set(registered_commands) - set(app_commands))
    extra = sorted(set(app_commands) - set(registered_commands))
    raise SystemExit(f"application-command ACL drifted; missing={missing}, extra={extra}")

if "env::var_os(\"OUT_DIR\")" not in build_source:
    raise SystemExit("generated application permissions must be written under Cargo OUT_DIR")
if "AppManifest::new().permissions_path_pattern(" not in build_source:
    raise SystemExit("generated application permissions are not registered with Tauri")
if "tauri_build::try_build(" not in build_source or "tauri_build::build()" in build_source:
    raise SystemExit("Tauri build must fail closed while registering the application ACL")
if "permissions/autogenerated" in build_source:
    raise SystemExit("generated application permissions must not modify the source tree")

def app_permission(command):
    return "allow-" + command.replace("_", "-")

app_source = app_source_path.read_text(encoding="utf-8")
invoke_pattern = r'''\binvoke(?:<[^>]+>)?\s*\(\s*(["'])([a-z][a-z0-9_]*)\1'''

def frontend_invocations(source):
    commands = [match[1] for match in re.findall(invoke_pattern, source)]
    count = len(re.findall(r"\binvoke(?:<[^>]+>)?\s*\(", source))
    if len(commands) != count:
        raise SystemExit("frontend contains a dynamic or unclassified Tauri invoke call")
    return commands

support_panel_path = app_source_path.with_name("RemoteSupportPanel.tsx")
support_control_path = app_source_path.with_name("RemoteSupportControl.tsx")
support_session_path = app_source_path.parent.parent / "public" / "support" / "session.js"
support_rust_path = rust_source_path.with_name("support_session.rs")
for path in (support_panel_path, support_control_path, support_session_path, support_rust_path):
    if path.is_symlink() or not path.is_file():
        raise SystemExit("support boundary source must be a regular, non-symlink file")
support_panel_source = support_panel_path.read_text(encoding="utf-8")
support_control_source = support_control_path.read_text(encoding="utf-8")
support_session_source = support_session_path.read_text(encoding="utf-8")
support_rust_source = support_rust_path.read_text(encoding="utf-8")
wrapper_start = support_session_source.find("const invoke = (command, args) => {")
wrapper_end = support_session_source.find("\n};", wrapper_start)
if wrapper_start < 0 or wrapper_end < 0:
    raise SystemExit("support session invoke wrapper was not found")
wrapper = support_session_source[wrapper_start:wrapper_end + len("\n};")]
if support_session_source.count(wrapper) != 1:
    raise SystemExit("support session must contain exactly one reviewed invocation wrapper")
if hashlib.sha256(wrapper.strip().encode()).hexdigest() != (
    "217a03400c2ddf58c35bed262b4b68080e594794efb1036a6c4a790ff147ad16"
):
    raise SystemExit("support session invocation wrapper drifted from reviewed delegation")
support_frontend_commands = frontend_invocations(support_session_source.replace(wrapper, "", 1))
frontend_commands = [command for source in (app_source, support_panel_source, support_control_source)
                     for command in frontend_invocations(source)]
all_frontend_commands = set(frontend_commands) | set(support_frontend_commands)
if not all_frontend_commands.issubset(set(app_commands)):
    unknown = sorted(all_frontend_commands - set(app_commands))
    raise SystemExit(f"frontend invokes commands absent from the generated ACL: {unknown}")

main_support_commands = {
    "request_support_session", "support_service_status", "support_session_status", "publish_support_frame", "stop_support_session",
}
support_commands = {
    "bootstrap_support_session", "poll_support_frame", "support_submit_action", "stop_support_session",
}
if set(frontend_invocations(support_panel_source) + frontend_invocations(support_control_source)) != main_support_commands:
    raise SystemExit("main support controls invoke an unexpected command set")
if set(support_frontend_commands) != support_commands:
    raise SystemExit("support session invokes an unexpected command set")
if set(app_commands) - all_frontend_commands != {"export_original_eml", "reveal_exported_file", "start_support_session"}:
    raise SystemExit("registered unused commands drifted from the reviewed set")

preview_commands = {
    "get_preview_window_target",
    "get_source_eml_view",
    "get_standalone_message_view",
    "save_source_eml_as",
    "save_standalone_source_message_as",
    "save_printable_html_as",
    "reveal_saved_html",
    "reveal_saved_eml",
    "export_attachment",
    "open_attachment",
    "export_standalone_message_attachment",
    "open_standalone_message_attachment",
    "reveal_exported_file_for_workspace",
    "reveal_standalone_exported_file",
}
preview_source_match = re.search(
    r"function MessagePreviewBootstrap\b(.*?)\nfunction App\(\)",
    app_source,
    re.DOTALL,
)
if preview_source_match is None:
    raise SystemExit("message preview source boundary was not found")
preview_invocations = set(frontend_invocations(preview_source_match.group(1)))
if preview_invocations != preview_commands:
    missing = sorted(preview_invocations - preview_commands)
    extra = sorted(preview_commands - preview_invocations)
    raise SystemExit(f"message preview command ACL drifted; missing={missing}, extra={extra}")

preview_only_commands = {"get_preview_window_target"}
expected_main_permissions = main_core_permissions | {
    app_permission(command)
    for command in frontend_commands
    if command not in preview_only_commands
}
expected_preview_permissions = preview_core_permissions | {
    app_permission(command) for command in preview_commands
}
if set(main["permissions"]) != expected_main_permissions:
    raise SystemExit("main capability is not the exact reviewed permission set")
if set(preview["permissions"]) != expected_preview_permissions:
    raise SystemExit("message preview capability is not the exact reviewed permission set")
if set(support["permissions"]) != {app_permission(command) for command in support_commands}:
    raise SystemExit("support capability is not the exact reviewed permission set")
for command in support_commands:
    if (app_permission(command) in main["permissions"]) != (command == "stop_support_session"):
        raise SystemExit("main window inherits support-only broker authority")
    if app_permission(command) in preview["permissions"]:
        raise SystemExit("preview inherits support broker authority")
for command in main_support_commands:
    if app_permission(command) in preview["permissions"]:
        raise SystemExit("preview inherits main support authority")
if not set(preview["permissions"]).isdisjoint(main_core_permissions):
    raise SystemExit("preview capability inherits main-window core authority")
if "core:webview:allow-create-webview-window" in preview["permissions"]:
    raise SystemExit("preview windows must not create webviews")
if "core:webview:allow-create-webview-window" in main["permissions"]:
    raise SystemExit("main window must not create arbitrary webviews")
if "allow-get-preview-window-target" in main["permissions"]:
    raise SystemExit("main window must not resolve preview-only target state")
if "allow-open-message-preview-window" in preview["permissions"]:
    raise SystemExit("preview windows must not create other preview windows")
if "allow-open-message-preview-window" not in main["permissions"]:
    raise SystemExit("main window is missing the constrained preview command")
if re.search(r"new\s+WebviewWindow|WebviewWindow\.getByLabel", app_source):
    raise SystemExit("frontend retains generic WebView creation or lookup authority")
for required_source in (
    "WebviewWindowBuilder::new(",
    "WebviewUrl::App(PathBuf::from(PREVIEW_WINDOW_ROUTE))",
    'const PREVIEW_WINDOW_ROUTE: &str = "index.html?messagePreview=1"',
    'if window.label() != "main"',
    "const MAX_PREVIEW_WINDOWS: usize = 32",
):
    if required_source not in rust_source:
        raise SystemExit(f"constrained preview implementation is missing: {required_source}")
if "standaloneMessagePath=" in app_source or "previewWorkspaceId=" in app_source:
    raise SystemExit("frontend preview route contains a message path or workspace identity")

support_csp_start = support_rust_source.find("fn support_csp(")
support_csp_end = support_rust_source.find("\nfn take_session(", support_csp_start)
resource_start = support_rust_source.find(".on_web_resource_request(move |request, response| {")
resource_end_match = re.search(
    r"^[ \t]*\}\)[ \t]*\r?\n[ \t]*\.build\(\);[ \t]*\r?$",
    support_rust_source[resource_start:] if resource_start >= 0 else "",
    re.MULTILINE,
)
resource_end = resource_start + resource_end_match.start() if resource_end_match else -1
if min(support_csp_start, support_csp_end, resource_start, resource_end) < 0:
    raise SystemExit("scoped support CSP implementation was not found")
support_csp_source = support_rust_source[support_csp_start:support_csp_end]
support_csp_normalized = re.sub(r"\s+", " ", support_csp_source)
resource_source = support_rust_source[resource_start:resource_end]
for required in (
    "if !(allowed_origin(origin) || (allowed_queue_origin(origin) && pinned_origin == Some(origin)))",
    "let pinned_origin = service_config().ok().map(|config| config.origin)",
    "support_csp_for_service(existing, origin, pinned_origin.as_deref())",
    'directive.split_whitespace().next() != Some("connect-src")',
    'directives.push(format!("connect-src ipc: http://ipc.localhost {origin}"))',
    'Some(directives.join("; "))',
):
    if required not in support_csp_normalized:
        raise SystemExit("support CSP is not the exact reviewed origin-scoped connect exception")
expected_support_fallback = (
    "default-src 'none'; script-src 'self'; style-src 'self'; "
    "frame-src 'none'; frame-ancestors 'none'; object-src 'none'; "
    "media-src 'none'; base-uri 'none'; form-action 'none'"
)
fallback_literals = re.findall(r'\.unwrap_or\(\s*("(?:[^"\\]|\\.)*")\s*\)', resource_source)
if len(fallback_literals) != 1 or json.loads(fallback_literals[0]) != expected_support_fallback:
    raise SystemExit("support fallback CSP is not the exact reviewed policy")
guard = resource_source.find("if !local_document(&url, &resource_id)")
update = resource_source.find("support_csp(existing, &origin)")
if guard < 0 or update < 0 or guard >= update:
    raise SystemExit("support CSP exception is not gated to the local session document")
for required in (
    'const DOCUMENT: &str = "/support/session.html"',
    'strip_suffix(".run.pinggy-free.link")',
    '.on_navigation(move |url| local_document(url, &navigation_id))',
    'NewWindowResponse::Deny',
):
    if required not in support_rust_source:
        raise SystemExit("support navigation or origin scope drifted from reviewed bounds")
if re.search(r"WebviewUrl::External|unsafe-eval|script-src \*", support_rust_source.split("\n#[cfg(test)]")[0]):
    raise SystemExit("support window requests unreviewed navigation or active content")

with tauri_config_path.open(encoding="utf-8") as handle:
    tauri_config = json.load(handle)
security = tauri_config.get("app", {}).get("security", {})
if set(security) != {"csp"}:
    raise SystemExit(
        f"production Tauri security fields are not the exact reviewed set: {sorted(security)}"
    )
csp = security.get("csp")
if not isinstance(csp, str):
    raise SystemExit("production CSP is missing")

directives = {}
for raw_directive in csp.split(";"):
    fields = raw_directive.split()
    if not fields:
        continue
    name, values = fields[0], fields[1:]
    if name in directives:
        raise SystemExit(f"production CSP repeats {name}")
    directives[name] = values

expected_csp = {
    "default-src": ["'none'"],
    "script-src": ["'self'"],
    "style-src": ["'self'", "'unsafe-inline'"],
    "img-src": ["'self'", "data:"],
    "font-src": ["'none'"],
    "connect-src": ["ipc:", "http://ipc.localhost"],
    "object-src": ["'none'"],
    "base-uri": ["'none'"],
    "form-action": ["'none'"],
    "frame-src": ["'none'"],
    "frame-ancestors": ["'none'"],
    "child-src": ["'none'"],
    "worker-src": ["'none'"],
    "media-src": ["'none'"],
    "manifest-src": ["'none'"],
}
if directives != expected_csp:
    raise SystemExit(f"production CSP is not the exact reviewed policy: {directives}")

lower_csp = csp.lower().replace("http://ipc.localhost", "")
for forbidden in ("*", "'unsafe-eval'", "http:", "https:", "ws:", "wss:",
                  "127.0.0.1:1420", "localhost:1420", "@vite/client"):
    if forbidden in lower_csp:
        raise SystemExit(f"production CSP contains forbidden source or development origin: {forbidden}")
PY
then
  pass "Tauri CSP, application ACL, and disjoint window capabilities match the reviewed source policy"
else
  fail "Tauri security-boundary verification failed"
fi

if python3 "${ROOT_DIR}/scripts/verify-packaged-csp.py" \
  "${TAURI_CONFIG}" "${APP_BINARY}"
then
  pass "packaged executable embeds the reviewed production CSP"
else
  fail "packaged executable CSP verification failed"
fi

STALE_SECURITY_SOURCES=()
for security_source in \
  "${TAURI_CONFIG}" \
  "${BUILD_SCRIPT}" \
  "${TAURI_BUILD_WRAPPER}" \
  "${CAPABILITIES_DIR}/main.json" \
  "${CAPABILITIES_DIR}/message-preview.json" \
  "${CAPABILITIES_DIR}/support-session.json" \
  "${RUST_SOURCE}" \
  "${RUNTIME_PROFILE_SOURCE}" \
  "${SUPPORT_RUST_SOURCE}" \
  "${SUPPORT_PANEL_SOURCE}" \
  "${SUPPORT_CONTROL_SOURCE}" \
  "${SUPPORT_SESSION_SOURCE}" \
  "${SUPPORT_PROTOCOL_SOURCE}" \
  "${SUPPORT_PEER_SOURCE}" \
  "${SUPPORT_VIEW_SOURCE}" \
  "${SUPPORT_SESSION_DOCUMENT}" \
  "${APP_SOURCE}"
do
  if [[ "${security_source}" -nt "${APP_BINARY}" ]]; then
    STALE_SECURITY_SOURCES+=("${security_source#"${ROOT_DIR}/"}")
  fi
done
if [[ "${#STALE_SECURITY_SOURCES[@]}" -eq 0 ]]; then
  pass "packaged executable is not older than reviewed security-boundary sources"
else
  fail "packaged executable predates reviewed security-boundary sources: ${STALE_SECURITY_SOURCES[*]}"
fi

if [[ -d "${APP_PATH}/Contents" ]]; then
  if grep -R -a -E '127\.0\.0\.1:1420|localhost:1420|/@vite/client|vite/dist/client' \
    "${APP_PATH}/Contents" >/dev/null 2>&1; then
    fail "packaged app contains a development-server reference"
  else
    pass "packaged app contains no localhost or Vite development-server references"
  fi
  if grep -R -a -E \
    '(^|file://|[^[:alnum:]_./-])(/Users/[^/[:space:]]+|/home/[^/[:space:]]+|/Volumes/[^/[:space:]]+/|[A-Za-z]:\\Users\\)|PST_QUICKVIEW_(TREVOR|FURMAN)_MSG_FIXTURE' \
    "${APP_PATH}/Contents" >/dev/null 2>&1
  then
    PACKAGED_PRIVACY_SCAN_STATUS=0
  else
    PACKAGED_PRIVACY_SCAN_STATUS=$?
  fi
  if [[ "${PACKAGED_PRIVACY_SCAN_STATUS}" -eq 0 ]]; then
    fail "packaged app contains a private path or deprecated private fixture identifier"
  elif [[ "${PACKAGED_PRIVACY_SCAN_STATUS}" -eq 1 ]]; then
    pass "packaged app contains no private paths or deprecated private fixture identifiers"
  else
    fail "packaged app privacy scan failed closed"
  fi
  if [[ -f "${APP_BINARY}" ]] &&
    grep -a -F "${EXPECTED_VERSION}" "${APP_BINARY}" >/dev/null 2>&1; then
    pass "packaged frontend contains the expected Help/About version"
  else
    fail "packaged frontend does not contain the expected Help/About version"
  fi
fi

if [[ -d "${APP_PATH}" ]]; then
  SIGNATURE_DETAILS="$(codesign -dv --verbose=4 "${APP_PATH}" 2>&1 || true)"
  if [[ "${SIGNATURE_DETAILS}" == *'Signature=adhoc'* ]]; then
    printf 'WARN: app has an ad-hoc code signature; this is not a Developer ID distribution signature.\n'
  elif grep -q '^Authority=Developer ID Application:' <<<"${SIGNATURE_DETAILS}"; then
    printf 'WARN: app has a Developer ID Application signature; confirm its identity and release authorization manually.\n'
  elif codesign -dv --verbose=2 "${APP_PATH}" >/dev/null 2>&1; then
    printf 'WARN: app has a non-ad-hoc code signature; confirm its identity and release authorization manually.\n'
  else
    pass "app remains unsigned as documented for this beta candidate"
  fi
  printf 'WARN: code-signature status does not establish notarization; notarization is not performed or claimed by this verification script.\n'
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
