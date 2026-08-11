#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SOURCE_ARCHIVE="${1:-}"
DESTINATION="${2:-}"
PUBLIC_DOWNLOAD_LOCATION="${3:-}"

VERSION="0.6.76"
SOURCE_FILENAME="libpst-${VERSION}.tar.gz"
SOURCE_URL="https://www.five-ten-sg.com/libpst/packages/${SOURCE_FILENAME}"
SOURCE_SHA256="3d291beebbdb48d2b934608bc06195b641da63d2a8f5e0d386f2e9d6d05a0b42"
COMPANION_NAME="readpst-corresponding-source-${VERSION}"
PATCH_FILENAME="0001-disable-msg-output.patch"
X86_64_DEPLOYMENT_TARGET="10.13"
ARM64_DEPLOYMENT_TARGET="11.0"
EXPECTED_PUBLIC_DOWNLOAD_LOCATION="https://github.com/empathyrecoveryblitz/pst-quickview/releases/download/v0.2.0-beta.3/${COMPANION_NAME}.tar.gz"

usage() {
  cat >&2 <<EOF
Usage:
  $0 /path/to/${SOURCE_FILENAME} /destination/${COMPANION_NAME} ${EXPECTED_PUBLIC_DOWNLOAD_LOCATION}

The source archive must be supplied explicitly. This script performs no network
access and never uploads or publishes the resulting companion.
EOF
}

if [[ $# -ne 3 || -z "${SOURCE_ARCHIVE}" || -z "${DESTINATION}" || -z "${PUBLIC_DOWNLOAD_LOCATION}" ]]; then
  usage
  exit 1
fi

for command in awk cmp find install mkdir mv python3 shasum; do
  if ! command -v "${command}" >/dev/null 2>&1; then
    echo "Required command is missing: ${command}" >&2
    exit 1
  fi
done

if [[ ! -f "${SOURCE_ARCHIVE}" || ! -r "${SOURCE_ARCHIVE}" ]]; then
  echo "Source archive is missing or unreadable: ${SOURCE_ARCHIVE}" >&2
  exit 1
fi

SOURCE_ARCHIVE="$(cd "$(dirname "${SOURCE_ARCHIVE}")" && pwd)/$(basename "${SOURCE_ARCHIVE}")"
DESTINATION_PARENT="$(dirname "${DESTINATION}")"
mkdir -p "${DESTINATION_PARENT}"
DESTINATION_PARENT="$(cd "${DESTINATION_PARENT}" && pwd)"
DESTINATION="${DESTINATION_PARENT}/$(basename "${DESTINATION}")"

if [[ "$(basename "${DESTINATION}")" != "${COMPANION_NAME}" ]]; then
  echo "Destination directory must be named ${COMPANION_NAME}" >&2
  exit 1
fi

ARCHIVE_PATH="${DESTINATION}.tar.gz"
ARCHIVE_SHA_PATH="${ARCHIVE_PATH}.sha256"
if [[ -e "${DESTINATION}" || -e "${ARCHIVE_PATH}" || -e "${ARCHIVE_SHA_PATH}" ]]; then
  echo "Refusing to overwrite an existing destination or archive:" >&2
  echo "  ${DESTINATION}" >&2
  echo "  ${ARCHIVE_PATH}" >&2
  echo "  ${ARCHIVE_SHA_PATH}" >&2
  exit 1
fi

case "${PUBLIC_DOWNLOAD_LOCATION}" in
  https://*) ;;
  *)
    echo "Public download location must be an HTTPS URL" >&2
    exit 1
    ;;
esac
if [[ "${PUBLIC_DOWNLOAD_LOCATION}" != *"/${COMPANION_NAME}.tar.gz" ]]; then
  echo "Public download location must name ${COMPANION_NAME}.tar.gz" >&2
  exit 1
fi
if [[ "${PUBLIC_DOWNLOAD_LOCATION}" =~ example\.(com|org|net|invalid)|localhost|PLACEHOLDER ]]; then
  echo "Public download location must not be a placeholder or local host" >&2
  exit 1
fi
if [[ "${PUBLIC_DOWNLOAD_LOCATION}" != "${EXPECTED_PUBLIC_DOWNLOAD_LOCATION}" ]]; then
  echo "Public download location does not match the intended beta.3 release asset:" >&2
  echo "  supplied: ${PUBLIC_DOWNLOAD_LOCATION}" >&2
  echo "  expected: ${EXPECTED_PUBLIC_DOWNLOAD_LOCATION}" >&2
  exit 1
fi

actual_source_sha="$(shasum -a 256 "${SOURCE_ARCHIVE}" | awk '{print $1}')"
if [[ "${actual_source_sha}" != "${SOURCE_SHA256}" ]]; then
  echo "Source archive SHA-256 mismatch: ${actual_source_sha}" >&2
  echo "Expected: ${SOURCE_SHA256}" >&2
  exit 1
fi

required_repository_files=(
  "LICENSES/GPL-2.0-or-later.txt"
  "scripts/build-readpst-sidecars.sh"
  "scripts/macos-dylib-validation.sh"
  "scripts/test-macos-dylib-validation.sh"
  "scripts/verify-macos-readpst-bundle.sh"
  "scripts/verify-readpst-corresponding-source.sh"
  "scripts/readpst-patches/${PATCH_FILENAME}"
  "scripts/readpst-corresponding-source/README.md.in"
  "scripts/readpst-corresponding-source/COPYRIGHT_NOTICES.md"
  "scripts/readpst-corresponding-source/PATCHES_README.md"
  "scripts/readpst-corresponding-source/BUILD_INSTRUCTIONS.md"
  "src-tauri/binaries/readpst-x86_64-apple-darwin"
  "src-tauri/binaries/readpst-aarch64-apple-darwin"
  "src-tauri/binaries/readpst-universal-apple-darwin"
)
for relative in "${required_repository_files[@]}"; do
  if [[ ! -s "${ROOT_DIR}/${relative}" ]]; then
    echo "Required repository input is missing or empty: ${relative}" >&2
    exit 1
  fi
done

TEMP_DESTINATION="${DESTINATION}.tmp.$$"
PREPARATION_COMPLETE=0
cleanup() {
  rm -rf "${TEMP_DESTINATION}"
  if [[ "${PREPARATION_COMPLETE}" != "1" ]]; then
    rm -rf "${DESTINATION}"
    rm -f "${ARCHIVE_PATH}" "${ARCHIVE_SHA_PATH}"
  fi
}
trap cleanup EXIT

mkdir -p \
  "${TEMP_DESTINATION}/patches" \
  "${TEMP_DESTINATION}/build" \
  "${TEMP_DESTINATION}/scripts"
install -m 644 "${SOURCE_ARCHIVE}" "${TEMP_DESTINATION}/${SOURCE_FILENAME}"
install -m 644 "${ROOT_DIR}/LICENSES/GPL-2.0-or-later.txt" "${TEMP_DESTINATION}/LICENSE"
install -m 644 \
  "${ROOT_DIR}/scripts/readpst-corresponding-source/COPYRIGHT_NOTICES.md" \
  "${TEMP_DESTINATION}/COPYRIGHT_NOTICES.md"
install -m 644 \
  "${ROOT_DIR}/scripts/readpst-corresponding-source/PATCHES_README.md" \
  "${TEMP_DESTINATION}/patches/README.md"
install -m 644 \
  "${ROOT_DIR}/scripts/readpst-corresponding-source/BUILD_INSTRUCTIONS.md" \
  "${TEMP_DESTINATION}/build/BUILD_INSTRUCTIONS.md"
install -m 644 \
  "${ROOT_DIR}/scripts/readpst-patches/${PATCH_FILENAME}" \
  "${TEMP_DESTINATION}/patches/${PATCH_FILENAME}"
install -m 755 \
  "${ROOT_DIR}/scripts/build-readpst-sidecars.sh" \
  "${TEMP_DESTINATION}/build/build-readpst-sidecars.sh"
install -m 755 \
  "${ROOT_DIR}/scripts/verify-macos-readpst-bundle.sh" \
  "${TEMP_DESTINATION}/build/verify-macos-readpst-bundle.sh"
install -m 755 \
  "${ROOT_DIR}/scripts/verify-readpst-corresponding-source.sh" \
  "${TEMP_DESTINATION}/build/verify-readpst-corresponding-source.sh"
install -m 644 \
  "${ROOT_DIR}/scripts/macos-dylib-validation.sh" \
  "${TEMP_DESTINATION}/scripts/macos-dylib-validation.sh"
install -m 755 \
  "${ROOT_DIR}/scripts/test-macos-dylib-validation.sh" \
  "${TEMP_DESTINATION}/scripts/test-macos-dylib-validation.sh"

python3 - \
  "${ROOT_DIR}/scripts/readpst-corresponding-source/README.md.in" \
  "${TEMP_DESTINATION}/README.md" \
  "${PUBLIC_DOWNLOAD_LOCATION}" <<'PY'
import pathlib
import sys

template_path = pathlib.Path(sys.argv[1])
output_path = pathlib.Path(sys.argv[2])
public_location = sys.argv[3]
text = template_path.read_text()
marker = "@PUBLIC_DOWNLOAD_LOCATION@"
if text.count(marker) != 1:
    raise SystemExit("README template must contain exactly one public-location marker")
output_path.write_text(text.replace(marker, public_location))
PY

printf '%s\n' "${SOURCE_URL}" > "${TEMP_DESTINATION}/SOURCE_URL.txt"
printf '%s  %s\n' "${SOURCE_SHA256}" "${SOURCE_FILENAME}" > "${TEMP_DESTINATION}/SOURCE_SHA256.txt"
printf '%s\n' "${PUBLIC_DOWNLOAD_LOCATION}" > "${TEMP_DESTINATION}/PUBLIC_DOWNLOAD_LOCATION.txt"
cat > "${TEMP_DESTINATION}/DEPLOYMENT_TARGETS.txt" <<EOF
readpst-x86_64-apple-darwin x86_64 macOS ${X86_64_DEPLOYMENT_TARGET}
readpst-aarch64-apple-darwin arm64 macOS ${ARM64_DEPLOYMENT_TARGET}
readpst-universal-apple-darwin x86_64 macOS ${X86_64_DEPLOYMENT_TARGET}
readpst-universal-apple-darwin arm64 macOS ${ARM64_DEPLOYMENT_TARGET}
EOF

SIDECAR_SHA_PATH="${TEMP_DESTINATION}/SIDECAR_SHA256.txt"
: > "${SIDECAR_SHA_PATH}"
for filename in \
  readpst-x86_64-apple-darwin \
  readpst-aarch64-apple-darwin \
  readpst-universal-apple-darwin
do
  sidecar_sha="$(shasum -a 256 "${ROOT_DIR}/src-tauri/binaries/${filename}" | awk '{print $1}')"
  printf '%s  %s\n' "${sidecar_sha}" "${filename}" >> "${SIDECAR_SHA_PATH}"
done

python3 - "${TEMP_DESTINATION}" <<'PY'
import hashlib
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
manifest = root / "MANIFEST.sha256"
lines = []
for path in sorted(root.rglob("*")):
    if not path.is_file() or path == manifest:
        continue
    checksum = hashlib.sha256(path.read_bytes()).hexdigest()
    lines.append(f"{checksum}  {path.relative_to(root).as_posix()}")
manifest.write_text("\n".join(lines) + "\n")
PY

mv "${TEMP_DESTINATION}" "${DESTINATION}"

python3 - "${DESTINATION}" "${ARCHIVE_PATH}" <<'PY'
from __future__ import annotations

import gzip
import pathlib
import tarfile
import sys

source = pathlib.Path(sys.argv[1])
output = pathlib.Path(sys.argv[2])
root_name = source.name

with output.open("wb") as raw:
    with gzip.GzipFile(filename="", mode="wb", fileobj=raw, compresslevel=9, mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode="w", format=tarfile.GNU_FORMAT) as archive:
            entries = [source] + sorted(source.rglob("*"))
            for path in entries:
                relative = pathlib.Path(root_name) if path == source else pathlib.Path(root_name) / path.relative_to(source)
                info = tarfile.TarInfo(relative.as_posix())
                info.uid = 0
                info.gid = 0
                info.uname = "root"
                info.gname = "root"
                info.mtime = 0
                if path.is_dir():
                    info.type = tarfile.DIRTYPE
                    info.mode = 0o755
                    archive.addfile(info)
                elif path.is_file():
                    info.type = tarfile.REGTYPE
                    info.mode = 0o755 if path.stat().st_mode & 0o111 else 0o644
                    info.size = path.stat().st_size
                    with path.open("rb") as handle:
                        archive.addfile(info, handle)
                else:
                    raise SystemExit(f"unsupported companion entry: {path}")
PY

archive_sha="$(shasum -a 256 "${ARCHIVE_PATH}" | awk '{print $1}')"
printf '%s  %s\n' "${archive_sha}" "$(basename "${ARCHIVE_PATH}")" > "${ARCHIVE_SHA_PATH}"

bash "${ROOT_DIR}/scripts/verify-readpst-corresponding-source.sh" \
  "${DESTINATION}" "${ROOT_DIR}"

PREPARATION_COMPLETE=1
echo "ReadPST Corresponding Source directory: ${DESTINATION}"
echo "ReadPST Corresponding Source archive: ${ARCHIVE_PATH}"
echo "ReadPST Corresponding Source archive SHA-256: ${archive_sha}"
