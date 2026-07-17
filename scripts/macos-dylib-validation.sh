#!/usr/bin/env bash

# Parse only indented otool dependency records. Binary and architecture headers
# are intentionally ignored, including when a runner indents a header line.
pq_parse_otool_dependencies() {
  awk '
    /^[[:space:]]+/ {
      record = $0
      sub(/^[[:space:]]+/, "", record)

      dependency = record
      if (sub(/[[:space:]]+\(compatibility version .*/, "", dependency)) {
        print dependency
      }
    }
  '
}

pq_validate_otool_dependency_output() {
  local binary="$1"
  local label="$2"
  local arch="$3"
  local output="$4"
  local dependencies dependency
  local dependency_count=0
  local failed=0

  if ! dependencies="$(printf '%s\n' "${output}" | pq_parse_otool_dependencies)"; then
    printf '%s: failed to parse dylib dependencies for %s (%s)\n' \
      "${label}" "${binary}" "${arch}" >&2
    return 1
  fi

  while IFS= read -r dependency; do
    [[ -z "${dependency}" ]] && continue
    dependency_count=$((dependency_count + 1))
    case "${dependency}" in
      /System/Library/*|/usr/lib/*) ;;
      *)
        printf '%s: prohibited dynamic dependency for %s (%s): %s\n' \
          "${label}" "${binary}" "${arch}" "${dependency}" >&2
        failed=1
        ;;
    esac
  done <<<"${dependencies}"

  if ((dependency_count == 0)); then
    printf '%s: no dynamic dependency records parsed for %s (%s)\n' \
      "${label}" "${binary}" "${arch}" >&2
    return 1
  fi

  return "${failed}"
}

pq_validate_macho_system_dependencies() {
  local binary="$1"
  local label="$2"
  local arches arch output
  local failed=0

  if ! arches="$(lipo -archs "${binary}" 2>&1)"; then
    printf '%s: lipo failed for %s: %s\n' "${label}" "${binary}" "${arches}" >&2
    return 1
  fi
  if [[ -z "${arches}" ]]; then
    printf '%s: lipo returned no architectures for %s\n' "${label}" "${binary}" >&2
    return 1
  fi

  for arch in ${arches}; do
    if ! output="$(otool -arch "${arch}" -L "${binary}" 2>&1)"; then
      printf '%s: otool failed for %s (%s): %s\n' \
        "${label}" "${binary}" "${arch}" "${output}" >&2
      failed=1
      continue
    fi
    if ! pq_validate_otool_dependency_output \
      "${binary}" "${label}" "${arch}" "${output}"; then
      failed=1
    fi
  done

  return "${failed}"
}
