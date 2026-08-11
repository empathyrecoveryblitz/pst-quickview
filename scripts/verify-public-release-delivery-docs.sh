#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

usage() {
  printf 'Usage: %s [--root PATH]\n' "$0"
}

while (($#)); do
  case "$1" in
    --root) ROOT_DIR="$(cd "$2" && pwd)"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
  esac
done

BETA3_TAG="v0.2.0-beta.3"
BETA3_SOURCE_URL="https://github.com/empathyrecoveryblitz/pst-quickview/releases/download/v0.2.0-beta.3/readpst-corresponding-source-0.6.76.tar.gz"
BETA3_DMG_SHA256="b29ed3295e0bbbdcad4bd88621972a609c260601bda34808974c234a8785efad"
BETA3_SOURCE_SHA256="a858ea017bb80516b42b14da8b624530968c70a6daf5bfc7fad628a631a88787"
READPST_DELIVERY_DOC="${ROOT_DIR}/docs/READPST_CORRESPONDING_SOURCE.md"
RELEASE_COMPLIANCE_DOC="${ROOT_DIR}/docs/RELEASE_COMPLIANCE.md"
PUBLIC_AUDIT_DOC="${ROOT_DIR}/PUBLIC_GITHUB_AUDIT.md"
THIRD_PARTY_DOC="${ROOT_DIR}/THIRD_PARTY_NOTICES.md"
LICENSE_DECISION_DOC="${ROOT_DIR}/docs/LICENSE_DECISION.md"

if [[ ! -s "${READPST_DELIVERY_DOC}" || ! -s "${RELEASE_COMPLIANCE_DOC}" ||
  ! -s "${PUBLIC_AUDIT_DOC}" || ! -s "${THIRD_PARTY_DOC}" ||
  ! -s "${LICENSE_DECISION_DOC}" ]]; then
  printf 'Required public-delivery documentation is missing or empty.\n' >&2
  exit 1
fi

if ! grep -q '^\*\*Public delivery beside the DMG: COMPLETE\*\*$' "${READPST_DELIVERY_DOC}" ||
  ! grep -Fq "${BETA3_TAG}" "${READPST_DELIVERY_DOC}" ||
  ! grep -Fq "${BETA3_SOURCE_URL}" "${READPST_DELIVERY_DOC}" ||
  ! grep -Fq "${BETA3_DMG_SHA256}" "${READPST_DELIVERY_DOC}" ||
  ! grep -Fq "${BETA3_SOURCE_SHA256}" "${READPST_DELIVERY_DOC}" ||
  ! grep -Fq 'SHA256SUMS.txt' "${READPST_DELIVERY_DOC}" ||
  ! grep -q '^- \[x\] The verified ReadPST Corresponding Source archive and checksum are uploaded and available$' \
    "${RELEASE_COMPLIANCE_DOC}" ||
  ! grep -Fq "${BETA3_DMG_SHA256}" "${RELEASE_COMPLIANCE_DOC}" ||
  ! grep -Fq "${BETA3_SOURCE_SHA256}" "${RELEASE_COMPLIANCE_DOC}" ||
  ! grep -q 'preparation and public delivery are complete' "${PUBLIC_AUDIT_DOC}" ||
  ! grep -q 'prerelease publishes the exact' "${THIRD_PARTY_DOC}"; then
  printf 'The beta.3 COMPLETE delivery contract is missing or invalid.\n' >&2
  exit 1
fi

if grep -I -n -i -E \
  'PENDING AND RELEASE-BLOCKING|public delivery (is|remains) (still )?(not complete|pending|blocking|incomplete)|public binary publication remains blocked on the separate ReadPST|Upload the approved DMG|Verify the recorded public download URLs after upload' \
  "${READPST_DELIVERY_DOC}" "${RELEASE_COMPLIANCE_DOC}" "${PUBLIC_AUDIT_DOC}" \
  "${THIRD_PARTY_DOC}" "${LICENSE_DECISION_DOC}"; then
  printf 'Contradictory pending-delivery wording remains in active documentation.\n' >&2
  exit 1
fi

printf 'ReadPST Corresponding Source public-delivery documentation is complete and internally consistent.\n'
