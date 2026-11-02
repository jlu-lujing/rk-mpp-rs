#!/usr/bin/env bash
BOARD="${BOARD:-t6}"
# Compare our rkvdec H.264 register file with the reference MPP dump.
# Usage: scripts/regdiff.sh <stream> [frame_index]
set -euo pipefail
stream="${1:?usage: regdiff.sh <stream> [frame]}"
frame="${2:-0}"
our=/tmp/our_regs.txt
ref=/tmp/mpp_regs.txt

ssh "$BOARD" "cd ~/rk-mpp-rs && cargo build --release >/dev/null && \
  sudo rm -f $our && \
  sudo env RFI_REG_DUMP=1 ./target/release/rk-mpp-rs h264dec $stream /tmp/o_rust.yuv >/dev/null"

# Normalise both sides: keep only 8-hex-digit lines, take the requested frame.
# Each frame contributes 276 words (reg8..reg283).
norm() { grep -E '^[0-9a-fA-F]{8}$' "$1" | sed -n "$((frame*276+1)),$((frame*276+276))p"; }

ssh "$BOARD" "cat $ref" | norm /dev/stdin > /tmp/ref_frame.txt
ssh "$BOARD" "cat $our" | norm /dev/stdin > /tmp/our_frame.txt
if ! diff -u /tmp/ref_frame.txt /tmp/our_frame.txt > /tmp/regdiff.txt; then
  echo "register mismatch in frame $frame:"
  head -40 /tmp/regdiff.txt
  exit 1
fi
echo "frame $frame: registers identical"
