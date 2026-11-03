#!/usr/bin/env bash
# Regenerate tests/data/SHA256SUMS after an intentional fixture change.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

if command -v sha256sum >/dev/null; then
  HASH=(sha256sum)
elif command -v shasum >/dev/null; then
  HASH=(shasum -a 256)
else
  echo "sha256sum or shasum is required" >&2
  exit 1
fi

tmp="$(mktemp "${TMPDIR:-/tmp}/rk-mpp-fixtures.XXXXXX")"
trap 'test ! -e "$tmp" || unlink "$tmp"' EXIT
find tests/data -type f \
  ! -name SHA256SUMS ! -name SOURCES.md |
  LC_ALL=C sort |
  while IFS= read -r file; do
    "${HASH[@]}" "$file"
  done >"$tmp"
mv "$tmp" tests/data/SHA256SUMS
trap - EXIT
printf 'wrote %s\n' tests/data/SHA256SUMS
