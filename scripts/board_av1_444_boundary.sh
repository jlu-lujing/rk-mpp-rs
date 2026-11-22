#!/usr/bin/env bash
# Verify the RK3588 AV1 hardware 4:4:4 boundary.
set -euo pipefail

BOARD="${BOARD:?set BOARD to the RK3588 SSH alias}"
remote_repo="${REMOTE_REPO:-\$HOME/rk-mpp-rs}"
fixture=tests/data/av1_b_pyramid_10bit_444_advanced_320x240_20.ivf

COPYFILE_DISABLE=1 tar --no-xattrs \
  --exclude .git --exclude target --exclude .worktrees --exclude refs \
  -cf - . | ssh "$BOARD" "tar -xf - -C \"$remote_repo\""
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd "$HOME/rk-mpp-rs" && \
  cargo build --release --offline --bin rk-mpp-rs'

ssh "$BOARD" "cd \"$remote_repo\" && rm -f \
  /tmp/av1_444_aom.yuv /tmp/av1_444_rust.yuv /tmp/av1_444_rust.log"
ssh "$BOARD" "cd \"$remote_repo\" && aomdec --rawvideo --output-bit-depth=10 \
  -o /tmp/av1_444_aom.yuv $fixture >/tmp/av1_444_aom.log 2>&1"
ssh "$BOARD" "test \"\$(stat -c %s /tmp/av1_444_aom.yuv)\" -eq 9216000"
if ssh "$BOARD" "cd \"$remote_repo\" && timeout 60 \
  ./target/release/rk-mpp-rs av1dec $fixture /tmp/av1_444_rust.yuv \
  >/tmp/av1_444_rust.log 2>&1"; then
  printf 'FAIL AV1 hardware unexpectedly accepted 4:4:4\n' >&2
  exit 1
fi
ssh "$BOARD" "test \"\$(stat -c %s /tmp/av1_444_rust.yuv)\" -eq 0"
ssh "$BOARD" "grep -q 'AV1 hardware output supports only 4:2:0' /tmp/av1_444_rust.log"
printf 'PASS AV1 4:4:4: libaom oracle is 9216000 bytes; hardware rejects it explicitly\n'
