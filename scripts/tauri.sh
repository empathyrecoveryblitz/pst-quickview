#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TAURI_BIN="${ROOT_DIR}/node_modules/.bin/tauri"
COMMAND="${1:-}"

if [[ -z "${COMMAND}" ]]; then
  exec "${TAURI_BIN}"
fi

shift
if [[ "${COMMAND}" == "dev" ]]; then
  exec "${TAURI_BIN}" dev --config "${ROOT_DIR}/src-tauri/tauri.dev.conf.json" "$@"
fi

if [[ "${COMMAND}" == "build" ]]; then
  BUILD_HOME_DIR="${HOME:-}"
  if [[ -z "${BUILD_HOME_DIR}" || "${BUILD_HOME_DIR}" != /* || \
    "${BUILD_HOME_DIR}" == "/" || ! -d "${BUILD_HOME_DIR}" ]]; then
    printf 'A readable absolute HOME directory is required for a packaged build.\n' >&2
    exit 1
  fi
  BUILD_HOME_DIR="$(cd "${BUILD_HOME_DIR}" && pwd -P)"
  if [[ "${BUILD_HOME_DIR}" == "/" ]]; then
    printf 'The filesystem root cannot be used as HOME for a packaged build.\n' >&2
    exit 1
  fi

  BUILD_CARGO_HOME_DIR="${CARGO_HOME:-${BUILD_HOME_DIR}/.cargo}"
  if [[ "${BUILD_CARGO_HOME_DIR}" != /* ]]; then
    BUILD_CARGO_HOME_DIR="${PWD}/${BUILD_CARGO_HOME_DIR}"
  fi
  if [[ -d "${BUILD_CARGO_HOME_DIR}" ]]; then
    BUILD_CARGO_HOME_DIR="$(cd "${BUILD_CARGO_HOME_DIR}" && pwd -P)"
  fi
  if [[ "${BUILD_CARGO_HOME_DIR}" == "/" ]]; then
    printf 'The filesystem root cannot be used as CARGO_HOME for a packaged build.\n' >&2
    exit 1
  fi

  BUILD_ENCODED_RUSTFLAGS_WAS_SET="${CARGO_ENCODED_RUSTFLAGS+x}"
  BUILD_ENCODED_RUSTFLAGS="${CARGO_ENCODED_RUSTFLAGS-}"
  append_encoded_rustflag() {
    local flag="$1"
    if [[ -n "${BUILD_ENCODED_RUSTFLAGS}" ]]; then
      BUILD_ENCODED_RUSTFLAGS+=$'\x1f'
    fi
    BUILD_ENCODED_RUSTFLAGS+="${flag}"
  }

  if [[ -z "${BUILD_ENCODED_RUSTFLAGS_WAS_SET}" && -n "${RUSTFLAGS:-}" ]]; then
    BUILD_LEGACY_RUSTFLAGS="${RUSTFLAGS//$'\r'/ }"
    BUILD_LEGACY_RUSTFLAGS="${BUILD_LEGACY_RUSTFLAGS//$'\n'/ }"
    read -r -a BUILD_RUSTFLAGS <<<"${BUILD_LEGACY_RUSTFLAGS}"
    for build_rustflag in "${BUILD_RUSTFLAGS[@]}"; do
      append_encoded_rustflag "${build_rustflag}"
    done
  fi

  # rustc applies the last matching remap, so keep these broad-to-specific.
  append_encoded_rustflag "--remap-path-prefix=${BUILD_HOME_DIR}=/build/home"
  append_encoded_rustflag "--remap-path-prefix=${BUILD_CARGO_HOME_DIR}=/build/cargo"
  append_encoded_rustflag "--remap-path-prefix=${ROOT_DIR}=/build/source"
  export CARGO_ENCODED_RUSTFLAGS="${BUILD_ENCODED_RUSTFLAGS}"
  unset RUSTFLAGS
fi

exec "${TAURI_BIN}" "${COMMAND}" "$@"
