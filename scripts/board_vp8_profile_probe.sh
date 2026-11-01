#!/usr/bin/env bash
# Ask the board corpus which VP8 profiles actually exist before deciding
# how far the profile 1/3 implementation has to reach.
#
# Writes nothing but its stdout. Safe to run any time.
#
# Usage: scripts/board_vp8_profile_probe.sh [corpus-dir]
set -euo pipefail

corpus="${1:-/home/user/rk-mpp-oracles/corpus}"
here="$(cd "$(dirname "$0")" && pwd)"

if ssh t6 "test -d $corpus"; then
  scp -q "$here/board_vp8_profile_probe.py" t6:/tmp/board_vp8_profile_probe.py
  ssh t6 "python3 /tmp/board_vp8_profile_probe.py $corpus"
else
  printf 'SKIP corpus missing at %s\n' "$corpus"
fi
