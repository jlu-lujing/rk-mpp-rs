#!/usr/bin/env bash
BOARD="${BOARD:-t6}"
# Ask the board corpus which VP8 profiles actually exist before deciding
# how far the profile 1/3 implementation has to reach.
#
# Writes nothing but its stdout. Safe to run any time.
#
# Usage: scripts/board_vp8_profile_probe.sh [corpus-dir]
set -euo pipefail

corpus="${1:-~/rk-mpp-oracles/corpus}"
here="$(cd "$(dirname "$0")" && pwd)"

if ssh "$BOARD" "test -d $corpus"; then
  scp -q "$here/board_vp8_profile_probe.py" "$BOARD":/tmp/board_vp8_profile_probe.py
  ssh "$BOARD" "python3 /tmp/board_vp8_profile_probe.py $corpus"
else
  source "$(cd "$(dirname "$0")" && pwd)/board_dependency.sh" \
    "VP8 profile probe" "corpus missing at $corpus"
fi
