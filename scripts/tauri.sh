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

exec "${TAURI_BIN}" "${COMMAND}" "$@"
