#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
BUILD_ROOT="${BUILD_ROOT:-/tmp/pst-quickview-readpst-build}"
LIBPST_VERSION="0.6.76"
LIBPST_URL="https://www.five-ten-sg.com/libpst/packages/libpst-${LIBPST_VERSION}.tar.gz"
LIBPST_SHA256="3d291beebbdb48d2b934608bc06195b641da63d2a8f5e0d386f2e9d6d05a0b42"
PATCH_SHA256="73c319f11c42618707476f3cffaaf3238a667f48b6b8e32945665257b953a6b0"
EXPECTED_CLANG_VERSION="Apple clang version 17.0.0 (clang-1700.0.13.5)"
EXPECTED_SDK_VERSION="15.5"
READPST_X86_64_DEPLOYMENT_TARGET="${READPST_X86_64_DEPLOYMENT_TARGET:-10.13}"
READPST_ARM64_DEPLOYMENT_TARGET="${READPST_ARM64_DEPLOYMENT_TARGET:-11.0}"

if [[ -n "${READPST_SOURCE_ARCHIVE:-}" ]]; then
  SOURCE_TAR="${READPST_SOURCE_ARCHIVE}"
elif [[ -f "${ROOT_DIR}/libpst-${LIBPST_VERSION}.tar.gz" ]]; then
  SOURCE_TAR="${ROOT_DIR}/libpst-${LIBPST_VERSION}.tar.gz"
else
  SOURCE_TAR="${BUILD_ROOT}/libpst-${LIBPST_VERSION}.tar.gz"
fi

if [[ -n "${READPST_PATCH_FILE:-}" ]]; then
  PATCH_FILE="${READPST_PATCH_FILE}"
elif [[ -f "${ROOT_DIR}/scripts/readpst-patches/0001-disable-msg-output.patch" ]]; then
  PATCH_FILE="${ROOT_DIR}/scripts/readpst-patches/0001-disable-msg-output.patch"
else
  PATCH_FILE="${ROOT_DIR}/patches/0001-disable-msg-output.patch"
fi

if [[ -n "${READPST_OUTPUT_DIR:-}" ]]; then
  OUTPUT_DIR="${READPST_OUTPUT_DIR}"
elif [[ -d "${ROOT_DIR}/src-tauri/binaries" ]]; then
  OUTPUT_DIR="${ROOT_DIR}/src-tauri/binaries"
else
  OUTPUT_DIR="${ROOT_DIR}/out"
fi

STAGING_DIR="${BUILD_ROOT}/sidecars-staging"
BUILD_JOBS="${READPST_BUILD_JOBS:-$(sysctl -n hw.ncpu)}"

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Required build command is missing: $1" >&2
    exit 1
  fi
}

sha256() {
  shasum -a 256 "$1" | awk '{print $1}'
}

version_at_least() {
  awk -v actual="$1" -v minimum="$2" 'BEGIN {
    split(actual, a, ".");
    split(minimum, m, ".");
    for (i = 1; i <= 3; i++) {
      av = (i in a) ? a[i] + 0 : 0;
      mv = (i in m) ? m[i] + 0 : 0;
      if (av > mv) exit 0;
      if (av < mv) exit 1;
    }
    exit 0;
  }'
}

validate_deployment_target() {
  local label="$1"
  local value="$2"
  local minimum="$3"
  if [[ ! "${value}" =~ ^[0-9]+([.][0-9]+){1,2}$ ]]; then
    echo "Invalid ${label} deployment target: ${value}" >&2
    echo "Expected a numeric macOS version such as ${minimum}." >&2
    exit 1
  fi
  if ! version_at_least "${value}" "${minimum}"; then
    echo "${label} deployment target ${value} is below required minimum ${minimum}." >&2
    exit 1
  fi
}

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

check_deployment_target() {
  local binary="$1"
  local arch="$2"
  local expected="$3"
  local actual
  actual="$(deployment_target_for_arch "${binary}" "${arch}")"
  if [[ -z "${actual}" ]]; then
    echo "Deployment target is missing for ${binary} (${arch})." >&2
    exit 1
  fi
  if [[ "${actual}" != "${expected}" ]]; then
    echo "Unexpected deployment target for ${binary} (${arch}): ${actual}; expected ${expected}" >&2
    exit 1
  fi
  echo "deployment target: $(basename "${binary}") ${arch} macOS ${actual}"
}

for command in awk clang clang++ file grep install lipo make otool patch sed shasum strings sysctl tail tar xcrun; do
  require_command "${command}"
done

if ! xcrun --find vtool >/dev/null 2>&1; then
  echo "Required Apple tool is missing: vtool" >&2
  exit 1
fi

validate_deployment_target "x86_64" "${READPST_X86_64_DEPLOYMENT_TARGET}" "10.13"
validate_deployment_target "arm64" "${READPST_ARM64_DEPLOYMENT_TARGET}" "11.0"

if [[ ! -f "${SOURCE_TAR}" ]]; then
  cat >&2 <<EOF
The exact libpst source archive is required at:
  ${SOURCE_TAR}

Obtain libpst-${LIBPST_VERSION}.tar.gz from the authoritative upstream URL:
  ${LIBPST_URL}

Expected SHA-256:
  ${LIBPST_SHA256}

This build script does not download source files.
EOF
  exit 1
fi

if [[ ! -f "${PATCH_FILE}" ]]; then
  echo "Required ReadPST patch is missing: ${PATCH_FILE}" >&2
  exit 1
fi

actual_source_sha="$(sha256 "${SOURCE_TAR}")"
if [[ "${actual_source_sha}" != "${LIBPST_SHA256}" ]]; then
  echo "libpst source checksum mismatch: ${actual_source_sha}" >&2
  exit 1
fi

actual_patch_sha="$(sha256 "${PATCH_FILE}")"
if [[ "${actual_patch_sha}" != "${PATCH_SHA256}" ]]; then
  echo "ReadPST patch checksum mismatch: ${actual_patch_sha}" >&2
  exit 1
fi

clang_version="$(xcrun clang --version | sed -n '1p')"
sdk_version="$(xcrun --show-sdk-version)"
if [[ "${READPST_ALLOW_TOOLCHAIN_MISMATCH:-0}" != "1" ]]; then
  if [[ "${clang_version}" != "${EXPECTED_CLANG_VERSION}" ]]; then
    echo "Unexpected Apple clang version: ${clang_version}" >&2
    echo "Expected: ${EXPECTED_CLANG_VERSION}" >&2
    exit 1
  fi
  if [[ "${sdk_version}" != "${EXPECTED_SDK_VERSION}" ]]; then
    echo "Unexpected macOS SDK version: ${sdk_version}" >&2
    echo "Expected: ${EXPECTED_SDK_VERSION}" >&2
    exit 1
  fi
fi

echo "libpst source: ${SOURCE_TAR}"
echo "libpst SHA-256: ${actual_source_sha}"
echo "local patch: ${PATCH_FILE}"
echo "patch SHA-256: ${actual_patch_sha}"
echo "compiler: ${clang_version}"
echo "SDK: ${sdk_version}"
echo "x86_64 deployment target: macOS ${READPST_X86_64_DEPLOYMENT_TARGET}"
echo "arm64 deployment target: macOS ${READPST_ARM64_DEPLOYMENT_TARGET}"

# The release build intentionally uses only Apple toolchain and base-system tools.
export PATH="/usr/bin:/bin:/usr/sbin:/sbin"
export ZERO_AR_DATE=1

rm -rf "${STAGING_DIR}"
mkdir -p "${STAGING_DIR}"

build_one() {
  local arch="$1"
  local host="$2"
  local output_name="$3"
  local deployment_target="$4"
  local work_dir="${BUILD_ROOT}/build-${arch}"
  local target_flag="-mmacosx-version-min=${deployment_target}"
  local cc="clang -arch ${arch} ${target_flag}"
  local cxx="clang++ -arch ${arch} ${target_flag}"
  local cppflags="${target_flag}"
  local cflags="-O2 ${target_flag}"
  local cxxflags="-O2 ${target_flag}"
  local ldflags="${target_flag}"

  rm -rf "${work_dir}"
  mkdir -p "${work_dir}"
  tar -xzf "${SOURCE_TAR}" -C "${work_dir}" --strip-components=1
  patch --batch --forward -d "${work_dir}" -p1 < "${PATCH_FILE}"

  (
    cd "${work_dir}"
    export MACOSX_DEPLOYMENT_TARGET="${deployment_target}"
    CC="${cc}" \
    CXX="${cxx}" \
    CPPFLAGS="${cppflags}" \
    CFLAGS="${cflags}" \
    CXXFLAGS="${cxxflags}" \
    LDFLAGS="${ldflags}" \
    LIBS='-liconv' \
    ac_cv_func_lstat_dereferences_slashed_symlink=yes \
    ac_cv_func_lstat_empty_string_bug=no \
    ac_cv_func_stat_empty_string_bug=no \
    ac_cv_func_malloc_0_nonnull=yes \
    ac_cv_func_realloc_0_nonnull=yes \
    ac_cv_func_working_mktime=yes \
    ac_cv_func_fork_works=yes \
    ac_cv_func_vfork_works=yes \
    ac_cv_func_strftime=yes \
    ac_cv_func_regexec=yes \
    ac_cv_func_iconv=yes \
    am_cv_func_iconv=yes \
    am_cv_func_iconv_works=yes \
    am_cv_lib_iconv=no \
    ./configure \
      --host="${host}" \
      --build="$(uname -m)-apple-darwin" \
      --disable-dependency-tracking \
      --disable-python \
      --disable-nls \
      --enable-static-tools \
      --disable-libpst-shared \
      --disable-shared \
      GSF_CFLAGS=' ' \
      GSF_LIBS=' ' \
      ZLIB_CFLAGS=' ' \
      ZLIB_LIBS='-lz'

    make -C src -j"${BUILD_JOBS}" readpst \
      MACOSX_DEPLOYMENT_TARGET="${deployment_target}" \
      CC="${cc}" \
      CXX="${cxx}" \
      CPPFLAGS="${cppflags}" \
      CFLAGS="${cflags}" \
      CXXFLAGS="${cxxflags}" \
      LDFLAGS="${ldflags}" \
      LIBS='-liconv'
  )

  install -m 755 "${work_dir}/src/readpst" "${STAGING_DIR}/${output_name}"
}

check_architecture() {
  local binary="$1"
  local expected="$2"
  local actual
  actual="$(lipo -archs "${binary}")"
  if [[ "${actual}" != "${expected}" ]]; then
    echo "Unexpected architecture for ${binary}: ${actual}; expected ${expected}" >&2
    exit 1
  fi
}

check_system_linkage() {
  local binary="$1"
  local dependency
  while IFS= read -r dependency; do
    [[ -z "${dependency}" ]] && continue
    case "${dependency}" in
      /usr/lib/*|/System/Library/Frameworks/*) ;;
      *)
        echo "Non-system dynamic dependency in ${binary}: ${dependency}" >&2
        exit 1
        ;;
    esac
  done < <(otool -L "${binary}" | tail -n +2 | awk '{print $1}')

  if strings "${binary}" | grep -E '/Users/|/home/' >/dev/null; then
    echo "Private build path found in ${binary}" >&2
    exit 1
  fi
}

build_one "x86_64" "x86_64-apple-darwin" \
  "readpst-x86_64-apple-darwin" "${READPST_X86_64_DEPLOYMENT_TARGET}"
build_one "arm64" "aarch64-apple-darwin" \
  "readpst-aarch64-apple-darwin" "${READPST_ARM64_DEPLOYMENT_TARGET}"

lipo -create \
  "${STAGING_DIR}/readpst-x86_64-apple-darwin" \
  "${STAGING_DIR}/readpst-aarch64-apple-darwin" \
  -output "${STAGING_DIR}/readpst-universal-apple-darwin"
chmod 755 "${STAGING_DIR}/readpst-universal-apple-darwin"

check_architecture "${STAGING_DIR}/readpst-x86_64-apple-darwin" "x86_64"
check_architecture "${STAGING_DIR}/readpst-aarch64-apple-darwin" "arm64"
universal_arches="$(lipo -archs "${STAGING_DIR}/readpst-universal-apple-darwin")"
if [[ "${universal_arches}" != "x86_64 arm64" && "${universal_arches}" != "arm64 x86_64" ]]; then
  echo "Universal ReadPST is missing an architecture: ${universal_arches}" >&2
  exit 1
fi

check_deployment_target "${STAGING_DIR}/readpst-x86_64-apple-darwin" \
  "x86_64" "${READPST_X86_64_DEPLOYMENT_TARGET}"
check_deployment_target "${STAGING_DIR}/readpst-aarch64-apple-darwin" \
  "arm64" "${READPST_ARM64_DEPLOYMENT_TARGET}"
check_deployment_target "${STAGING_DIR}/readpst-universal-apple-darwin" \
  "x86_64" "${READPST_X86_64_DEPLOYMENT_TARGET}"
check_deployment_target "${STAGING_DIR}/readpst-universal-apple-darwin" \
  "arm64" "${READPST_ARM64_DEPLOYMENT_TARGET}"

for binary in "${STAGING_DIR}"/readpst-*; do
  file "${binary}"
  otool -L "${binary}"
  check_system_linkage "${binary}"
  if ! strings "${binary}" | grep -F 'ReadPST / LibPST v%s' >/dev/null ||
    ! strings "${binary}" | grep -Fx "${LIBPST_VERSION}" >/dev/null; then
    echo "ReadPST version markers missing from ${binary}" >&2
    exit 1
  fi
done

version_output="$("${STAGING_DIR}/readpst-universal-apple-darwin" -V 2>&1)"
if [[ "${version_output}" != *"ReadPST / LibPST v${LIBPST_VERSION}"* ]]; then
  echo "Unexpected ReadPST version output: ${version_output}" >&2
  exit 1
fi

mkdir -p "${OUTPUT_DIR}"
for output_name in \
  readpst-x86_64-apple-darwin \
  readpst-aarch64-apple-darwin \
  readpst-universal-apple-darwin
do
  install -m 755 "${STAGING_DIR}/${output_name}" "${OUTPUT_DIR}/${output_name}"
done

echo "Built and verified ReadPST ${LIBPST_VERSION} sidecars in ${OUTPUT_DIR}"
