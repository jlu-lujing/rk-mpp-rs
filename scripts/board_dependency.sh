#!/usr/bin/env bash
# Fail a board gate when a required oracle, tool or fixture is unavailable.
# A partial run is allowed only when ALLOW_SKIPS=1 is explicit.
set -u

label="${1:-board gate}"
reason="${2:-required dependency is missing}"

if [ "${ALLOW_SKIPS:-0}" = 1 ]; then
  printf 'SKIP %s (%s)\n' "$label" "$reason"
  exit 0
fi

printf 'FAIL %s (%s; set ALLOW_SKIPS=1 for an explicit partial run)\n' \
  "$label" "$reason" >&2
exit 2
