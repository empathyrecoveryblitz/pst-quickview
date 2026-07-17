#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
COMPANION_DIR="${1:-}"
REFERENCE_ROOT="${2:-${ROOT_DIR}}"
EXPECTED_PUBLIC_DOWNLOAD_LOCATION="${READPST_EXPECTED_PUBLIC_DOWNLOAD_LOCATION:-https://github.com/empathyrecoveryblitz/pst-quickview/releases/download/v0.2.0-beta.2/readpst-corresponding-source-0.6.76.tar.gz}"

if [[ -z "${COMPANION_DIR}" || ! -d "${COMPANION_DIR}" ]]; then
  echo "Usage: $0 /absolute/path/to/readpst-corresponding-source-0.6.76 [repository-root]" >&2
  exit 1
fi

COMPANION_DIR="$(cd "${COMPANION_DIR}" && pwd)"
REFERENCE_ROOT="$(cd "${REFERENCE_ROOT}" && pwd)"
ARCHIVE_PATH="${COMPANION_DIR}.tar.gz"
ARCHIVE_SHA_PATH="${ARCHIVE_PATH}.sha256"

for command in lipo python3 xcrun; do
  if ! command -v "${command}" >/dev/null 2>&1; then
    echo "FAIL: required verification command is missing: ${command}" >&2
    exit 1
  fi
done
if ! xcrun --find vtool >/dev/null 2>&1; then
  echo "FAIL: required Apple tool is missing: vtool" >&2
  exit 1
fi

python3 - \
  "${COMPANION_DIR}" \
  "${ARCHIVE_PATH}" \
  "${ARCHIVE_SHA_PATH}" \
  "${REFERENCE_ROOT}" \
  "${EXPECTED_PUBLIC_DOWNLOAD_LOCATION}" <<'PY'
from __future__ import annotations

import hashlib
import pathlib
import re
import sys
import tarfile
from urllib.parse import urlparse

companion = pathlib.Path(sys.argv[1])
archive_path = pathlib.Path(sys.argv[2])
archive_sha_path = pathlib.Path(sys.argv[3])
reference_root = pathlib.Path(sys.argv[4])
expected_public_location = sys.argv[5]

version = "0.6.76"
source_name = f"libpst-{version}.tar.gz"
source_url = f"https://www.five-ten-sg.com/libpst/packages/{source_name}"
source_sha = "3d291beebbdb48d2b934608bc06195b641da63d2a8f5e0d386f2e9d6d05a0b42"
patch_name = "0001-disable-msg-output.patch"
patch_sha = "73c319f11c42618707476f3cffaaf3238a667f48b6b8e32945665257b953a6b0"
deployment_targets = """readpst-x86_64-apple-darwin x86_64 macOS 10.13
readpst-aarch64-apple-darwin arm64 macOS 11.0
readpst-universal-apple-darwin x86_64 macOS 10.13
readpst-universal-apple-darwin arm64 macOS 11.0
"""
required = [
    "README.md",
    "MANIFEST.sha256",
    "SOURCE_URL.txt",
    "SOURCE_SHA256.txt",
    "PUBLIC_DOWNLOAD_LOCATION.txt",
    "DEPLOYMENT_TARGETS.txt",
    "COPYRIGHT_NOTICES.md",
    "LICENSE",
    source_name,
    "SIDECAR_SHA256.txt",
    "patches/README.md",
    f"patches/{patch_name}",
    "build/BUILD_INSTRUCTIONS.md",
    "build/build-readpst-sidecars.sh",
    "build/verify-macos-readpst-bundle.sh",
    "build/verify-readpst-corresponding-source.sh",
]


def fail(message: str) -> None:
    raise SystemExit(f"FAIL: {message}")


def passed(message: str) -> None:
    print(f"PASS: {message}")


def digest(path: pathlib.Path) -> str:
    value = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


for relative in required:
    path = companion / relative
    if not path.is_file() or path.stat().st_size == 0:
        fail(f"required companion file is missing or empty: {relative}")
passed("all required companion files exist and are nonempty")

if companion.name != f"readpst-corresponding-source-{version}":
    fail(f"unexpected companion directory name: {companion.name}")
passed("companion directory name records ReadPST 0.6.76")

if (companion / "SOURCE_URL.txt").read_text().strip() != source_url:
    fail("SOURCE_URL.txt does not contain the authoritative upstream URL")
if digest(companion / source_name) != source_sha:
    fail("source archive hash does not match the authoritative input")
source_record = (companion / "SOURCE_SHA256.txt").read_text().strip()
if source_record != f"{source_sha}  {source_name}":
    fail("SOURCE_SHA256.txt does not match the source archive")
passed("authoritative source URL, filename, and SHA-256 match")

if (companion / "DEPLOYMENT_TARGETS.txt").read_text() != deployment_targets:
    fail("DEPLOYMENT_TARGETS.txt does not record the required per-architecture minimums")
passed("deployment metadata records x86_64 macOS 10.13 and arm64 macOS 11.0")

with tarfile.open(companion / source_name, "r:gz") as source_tar:
    source_members = source_tar.getmembers()
    if not source_members or not all(
        member.name == f"libpst-{version}"
        or member.name.startswith(f"libpst-{version}/")
        for member in source_members
    ):
        fail("source archive does not contain the expected libpst-0.6.76 root")
    prohibited_source = [
        member.name
        for member in source_members
        if pathlib.PurePosixPath(member.name).suffix.lower()
        in {".pst", ".ost", ".eml", ".msg"}
    ]
    if prohibited_source:
        fail(f"source archive contains message-data files: {prohibited_source[:5]}")
passed("source archive structure is valid and contains no message-data fixtures")

if digest(companion / f"patches/{patch_name}") != patch_sha:
    fail("local patch hash does not match the recorded build input")
patch_text = (companion / "patches/README.md").read_text()
if patch_name not in patch_text or patch_sha not in patch_text or "No other source patch" not in patch_text:
    fail("patches/README.md does not declare the complete patch set")
passed("the exact local patch and no-other-patches declaration are present")

license_path = reference_root / "LICENSES/GPL-2.0-or-later.txt"
if not license_path.is_file() or (companion / "LICENSE").read_bytes() != license_path.read_bytes():
    fail("companion LICENSE differs from the repository's verified GPL-2.0 text")
notices = (companion / "COPYRIGHT_NOTICES.md").read_text()
if (
    "GPL-2.0-or-later" not in notices
    or not re.search(r"David\s+Smith", notices)
    or not re.search(r"510\s+Software\s+Group", notices)
):
    fail("copyright or GPL-2.0-or-later notices are incomplete")
passed("license and principal upstream notices are present")

public_location = (companion / "PUBLIC_DOWNLOAD_LOCATION.txt").read_text().strip()
if public_location != expected_public_location:
    fail(
        "public download location does not match the intended release asset; "
        f"found {public_location!r}, expected {expected_public_location!r}"
    )
parsed_location = urlparse(public_location)
if parsed_location.scheme != "https" or not parsed_location.netloc:
    fail("public download location must be an absolute HTTPS URL")
if pathlib.PurePosixPath(parsed_location.path).name != archive_path.name:
    fail("public download location does not name the companion archive")
if re.search(r"example\.(com|org|net|invalid)|localhost|placeholder|insert", public_location, re.I):
    fail("public download location contains a placeholder or non-public host")
if public_location not in (companion / "README.md").read_text():
    fail("README.md does not record PUBLIC_DOWNLOAD_LOCATION.txt")
passed("public download location matches the intended beta.2 HTTPS companion URL")

placeholder_pattern = re.compile(r"\b(TODO|TBD|PLACEHOLDER)\b|INSERT[ _-]+URL", re.I)
private_path_pattern = re.compile(r"/(?:Users|home)/[A-Za-z0-9._-]+/", re.I)
private_names = ("not" + "root",)
prohibited_suffixes = {".pst", ".ost", ".eml", ".msg", ".dmg", ".app"}
for path in companion.rglob("*"):
    relative = path.relative_to(companion)
    if path.is_symlink():
        fail(f"companion contains a symbolic link: {relative}")
    if path.is_file() and path.suffix.lower() in prohibited_suffixes:
        fail(f"companion contains a prohibited file: {relative}")
    if path.is_file() and path.name != source_name:
        data = path.read_bytes()
        if b"\0" not in data:
            text = data.decode("utf-8", errors="replace")
            if (
                relative.as_posix() != "build/verify-readpst-corresponding-source.sh"
                and placeholder_pattern.search(text)
            ):
                fail(f"unresolved placeholder in companion file: {relative}")
            if private_path_pattern.search(text) or any(name in text for name in private_names):
                fail(f"private path or user name in companion file: {relative}")
if any(path.name == ".git" for path in companion.rglob("*")):
    fail("companion contains Git metadata")
passed("companion contains no private paths, message files, release binaries, or Git metadata")

manifest_path = companion / "MANIFEST.sha256"
manifest_entries: dict[str, str] = {}
for line_number, line in enumerate(manifest_path.read_text().splitlines(), start=1):
    match = re.fullmatch(r"([0-9a-f]{64})  (.+)", line)
    if not match:
        fail(f"malformed manifest line {line_number}")
    checksum, relative = match.groups()
    if relative in manifest_entries:
        fail(f"duplicate manifest path: {relative}")
    manifest_entries[relative] = checksum

actual_files = {
    path.relative_to(companion).as_posix(): digest(path)
    for path in companion.rglob("*")
    if path.is_file() and path != manifest_path
}
if manifest_entries != actual_files:
    missing = sorted(set(actual_files) - set(manifest_entries))
    extra = sorted(set(manifest_entries) - set(actual_files))
    mismatched = sorted(
        path
        for path in set(actual_files) & set(manifest_entries)
        if actual_files[path] != manifest_entries[path]
    )
    fail(f"manifest mismatch; missing={missing}, extra={extra}, changed={mismatched}")
passed("MANIFEST.sha256 is complete and every hash matches")

reference_pairs = [
    ("build/build-readpst-sidecars.sh", "scripts/build-readpst-sidecars.sh"),
    ("build/verify-macos-readpst-bundle.sh", "scripts/verify-macos-readpst-bundle.sh"),
    ("build/verify-readpst-corresponding-source.sh", "scripts/verify-readpst-corresponding-source.sh"),
    (f"patches/{patch_name}", f"scripts/readpst-patches/{patch_name}"),
]
for companion_relative, reference_relative in reference_pairs:
    reference = reference_root / reference_relative
    if not reference.is_file() or (companion / companion_relative).read_bytes() != reference.read_bytes():
        fail(f"companion file is not the exact repository copy: {companion_relative}")
passed("patch and build/verification scripts are exact repository copies")

sidecar_record: dict[str, str] = {}
for line in (companion / "SIDECAR_SHA256.txt").read_text().splitlines():
    match = re.fullmatch(r"([0-9a-f]{64})  (readpst-[A-Za-z0-9_-]+)", line)
    if not match:
        fail("SIDECAR_SHA256.txt contains a malformed line")
    checksum, filename = match.groups()
    sidecar_record[filename] = checksum
expected_sidecars = {
    "readpst-x86_64-apple-darwin",
    "readpst-aarch64-apple-darwin",
    "readpst-universal-apple-darwin",
}
if set(sidecar_record) != expected_sidecars:
    fail("SIDECAR_SHA256.txt does not list all three exact sidecars")
for filename, expected_hash in sidecar_record.items():
    sidecar = reference_root / "src-tauri/binaries" / filename
    if not sidecar.is_file() or digest(sidecar) != expected_hash:
        fail(f"companion does not identify the current bundled sidecar: {filename}")
passed("companion is tied to the exact bundled ReadPST sidecar hashes")

if not archive_path.is_file() or archive_path.stat().st_size == 0:
    fail(f"deterministic companion archive is missing: {archive_path}")
if not archive_sha_path.is_file() or archive_sha_path.stat().st_size == 0:
    fail(f"companion archive checksum is missing: {archive_sha_path}")
archive_sha = digest(archive_path)
if archive_sha_path.read_text().strip() != f"{archive_sha}  {archive_path.name}":
    fail("companion archive SHA-256 record does not match")

with tarfile.open(archive_path, "r:gz") as companion_tar:
    members = companion_tar.getmembers()
    root = companion.name
    archive_files: dict[str, bytes] = {}
    for member in members:
        if member.issym() or member.islnk():
            fail(f"companion archive contains a link: {member.name}")
        if member.name != root and not member.name.startswith(f"{root}/"):
            fail(f"companion archive contains an unexpected root: {member.name}")
        if member.isfile():
            relative = pathlib.PurePosixPath(member.name).relative_to(root).as_posix()
            extracted = companion_tar.extractfile(member)
            if extracted is None:
                fail(f"unable to read archive member: {member.name}")
            archive_files[relative] = extracted.read()
directory_files = {
    path.relative_to(companion).as_posix(): path.read_bytes()
    for path in companion.rglob("*")
    if path.is_file()
}
if archive_files != directory_files:
    fail("companion archive contents differ from the verified directory")
passed(f"companion archive matches the directory; SHA-256 {archive_sha}")
PY

deployment_target_for_arch() {
  local binary="$1"
  local arch="$2"
  local output
  output="$(xcrun vtool -arch "${arch}" -show-build "${binary}" 2>&1)" || {
    echo "FAIL: unable to inspect ${binary} (${arch}): ${output}" >&2
    return 1
  }
  awk '
    $1 == "minos" { print $2; exit }
    $1 == "version" { print $2; exit }
  ' <<<"${output}"
}

check_sidecar() {
  local filename="$1"
  local expected_arches="$2"
  shift 2
  local binary="${REFERENCE_ROOT}/src-tauri/binaries/${filename}"
  local actual_arches
  actual_arches="$(lipo -archs "${binary}")"
  case "${expected_arches}" in
    universal)
      if [[ "${actual_arches}" != "x86_64 arm64" && "${actual_arches}" != "arm64 x86_64" ]]; then
        echo "FAIL: ${filename} architectures are ${actual_arches}; expected exactly x86_64 and arm64" >&2
        exit 1
      fi
      ;;
    *)
      if [[ "${actual_arches}" != "${expected_arches}" ]]; then
        echo "FAIL: ${filename} architectures are ${actual_arches}; expected ${expected_arches}" >&2
        exit 1
      fi
      ;;
  esac
  while [[ $# -gt 0 ]]; do
    local arch="$1"
    local expected="$2"
    local actual
    actual="$(deployment_target_for_arch "${binary}" "${arch}")"
    if [[ -z "${actual}" || "${actual}" != "${expected}" ]]; then
      echo "FAIL: ${filename} ${arch} target is ${actual:-missing}; expected ${expected}" >&2
      exit 1
    fi
    printf 'PASS: %s %s deployment target is macOS %s\n' "${filename}" "${arch}" "${actual}"
    shift 2
  done
}

check_sidecar "readpst-x86_64-apple-darwin" "x86_64" x86_64 10.13
check_sidecar "readpst-aarch64-apple-darwin" "arm64" arm64 11.0
check_sidecar "readpst-universal-apple-darwin" universal \
  x86_64 10.13 arm64 11.0

echo "ReadPST Corresponding Source verification passed."
