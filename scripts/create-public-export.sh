#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INIT_GIT=0

if [[ "${1:-}" == "--init-git" ]]; then INIT_GIT=1; shift; fi
if (($# != 1)); then
  printf 'Usage: %s [--init-git] DESTINATION\n' "$0" >&2
  exit 2
fi

DESTINATION="$1"
[[ "${DESTINATION}" = /* ]] || DESTINATION="$(pwd)/${DESTINATION}"
if ! command -v python3 >/dev/null 2>&1; then
  printf 'python3 is required to validate the export destination safely.\n' >&2
  exit 2
fi
DESTINATION="$(python3 -c 'import os, sys; print(os.path.realpath(sys.argv[1]))' "${DESTINATION}")"
if [[ -e "${DESTINATION}" ]]; then
  printf 'Refusing to overwrite existing destination: %s\n' "${DESTINATION}" >&2
  exit 2
fi

case "${DESTINATION}/" in
  "${ROOT_DIR}/"*)
    printf 'Refusing to create an export inside the source repository: %s\n' "${DESTINATION}" >&2
    exit 2
    ;;
esac

if ! git -C "${ROOT_DIR}" ls-files --error-unmatch scripts/audit-public-repo.sh \
  >/dev/null 2>&1; then
  printf 'Refusing export: scripts/audit-public-repo.sh is not tracked yet. Commit the reviewed public-beta preparation privately first.\n' >&2
  exit 2
fi

TRACKED_LIST="$(mktemp)"
trap 'rm -f "${TRACKED_LIST}"' EXIT
REQUIRED_EXPORT_PATHS=(
  "LICENSE"
  "LICENSES/GPL-3.0-or-later.txt"
  "LICENSES/GPL-2.0-or-later.txt"
  "COPYRIGHT.md"
  "THIRD_PARTY_NOTICES.md"
  "docs/READPST_CORRESPONDING_SOURCE.md"
)
EXCLUDED_INTERNAL_PATHS=(
  "AGENTS.md"
  "LOOP_TASK.md"
  "docs/AGENT_LOOP.md"
  "scripts/codex-loop.sh"
)

is_excluded_internal_path() {
  local candidate="$1"
  local excluded

  for excluded in "${EXCLUDED_INTERNAL_PATHS[@]}"; do
    if [[ "${candidate}" == "${excluded}" ]]; then
      return 0
    fi
  done

  return 1
}

FORBIDDEN_TRACKED=0
shopt -s nocasematch
while IFS= read -r -d '' relative; do
  if is_excluded_internal_path "${relative}"; then
    continue
  fi

  case "/${relative}" in
    */.git/*|*/.agent-loop/*|*/dist/*|*/src-tauri/target/*|*/src-tauri/gen/*|\
    */.pst-quickview/*|*/.pst-quickview.noindex/*|*/logs/*|*.app/*|\
    *.pst|*.ost|*.eml|*.msg|*.log|*.dmg|*.pkg|*.sqlite|*.sqlite3|*.sqlite-wal|\
    *.sqlite-shm|*.db|*.db-wal|*.db-shm|*.p12|*.pfx|*.pem|*.key|*.p8|\
    *.mobileprovision|*.cer|*.crt|*.der|*.keychain|*.keychain-db|*/.env|*/.env.*)
      FORBIDDEN_TRACKED=$((FORBIDDEN_TRACKED + 1))
      ;;
    *) printf '%s\0' "${relative}" >>"${TRACKED_LIST}" ;;
  esac
done < <(git -C "${ROOT_DIR}" ls-files --cached -z)
shopt -u nocasematch

for relative in "${REQUIRED_EXPORT_PATHS[@]}"; do
  if [[ ! -s "${ROOT_DIR}/${relative}" ]]; then
    printf 'Refusing export: required licensing file is missing or empty: %s\n' "${relative}" >&2
    exit 1
  fi
  if ! git -C "${ROOT_DIR}" ls-files --error-unmatch "${relative}" >/dev/null 2>&1; then
    printf '%s\0' "${relative}" >>"${TRACKED_LIST}"
  fi
done

if ((FORBIDDEN_TRACKED)); then
  printf 'Refusing export: %d forbidden message/build/workspace/log/package/credential path(s) are tracked.\n' \
    "${FORBIDDEN_TRACKED}" >&2
  exit 1
fi

mkdir -p "${DESTINATION}"
(
  cd "${ROOT_DIR}"
  tar --null -T "${TRACKED_LIST}" -cf -
) | tar -xf - -C "${DESTINATION}"

for relative in "${EXCLUDED_INTERNAL_PATHS[@]}"; do
  if [[ -e "${DESTINATION}/${relative}" || -L "${DESTINATION}/${relative}" ]]; then
    printf 'Public export unexpectedly contains excluded internal loop file: %s\n' "${relative}" >&2
    exit 1
  fi
done

# Defense in depth: tracked message/workspace/build artifacts must never survive an export.
find "${DESTINATION}" \( -name .git -o -name .agent-loop -o -name dist -o -name target -o -name gen -o -name logs -o -name '.pst-quickview*' -o -name '*.app' \) -prune -exec rm -rf {} +
find "${DESTINATION}" -type f \( -iname '*.pst' -o -iname '*.ost' -o -iname '*.eml' -o -iname '*.msg' -o -iname '*.log' -o -iname '*.dmg' -o -iname '*.pkg' -o -iname '*.sqlite*' -o -iname '*.db*' -o -iname '*.p12' -o -iname '*.pfx' -o -iname '*.pem' -o -iname '*.key' -o -name '.env' -o -name '.env.*' \) -delete

for relative in "${REQUIRED_EXPORT_PATHS[@]}"; do
  if [[ ! -s "${DESTINATION}/${relative}" ]]; then
    printf 'Public export is missing required licensing file: %s\n' "${relative}" >&2
    exit 1
  fi
done

printf 'Public export destination: %s\n' "${DESTINATION}"
if bash "${DESTINATION}/scripts/audit-public-repo.sh" --root "${DESTINATION}"; then
  printf 'Public export audit result: PASS\n'
else
  status=$?
  printf 'Public export audit result: FAIL (exit %d)\n' "${status}" >&2
  exit "${status}"
fi
if ((INIT_GIT)); then
  git -C "${DESTINATION}" init
fi
printf 'Public export created and audited at: %s\n' "${DESTINATION}"
printf 'No remote was configured and nothing was pushed.\n'
