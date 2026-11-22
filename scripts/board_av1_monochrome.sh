#!/usr/bin/env bash
# Verify AV1 monochrome hardware output against the libaom display oracle.
set -euo pipefail

BOARD="${BOARD:?set BOARD to the RK3588 SSH alias}"
remote_repo="${REMOTE_REPO:-\$HOME/rk-mpp-rs}"
fixture=tests/data/av1_delta_lf_multi_mono_key_320x240.ivf
expected=df3a2fe1f370df393d17cbb4eece93021d705d70f505cc71ab6b5d58e4ee909b

COPYFILE_DISABLE=1 tar --no-xattrs \
  --exclude .git --exclude target --exclude .worktrees --exclude refs \
  -cf - . | ssh "$BOARD" "tar -xf - -C \"$remote_repo\""
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd "$HOME/rk-mpp-rs" && \
  cargo build --release --offline --bin rk-mpp-rs'

ssh "$BOARD" "cd \"$remote_repo\" && rm -f \
  /tmp/av1_mono_aom.yuv /tmp/av1_mono_rust.yuv"
ssh "$BOARD" "cd \"$remote_repo\" && aomdec --i420 \
  -o /tmp/av1_mono_aom.yuv $fixture >/tmp/av1_mono_aom.log 2>&1"
ssh "$BOARD" "cd \"$remote_repo\" && timeout 60 \
  ./target/release/rk-mpp-rs av1dec $fixture /tmp/av1_mono_rust.yuv \
  >/tmp/av1_mono_rust.log 2>&1"
ssh "$BOARD" "test \"\$(stat -c %s /tmp/av1_mono_rust.yuv)\" -eq 115200"
ssh "$BOARD" "cmp -s /tmp/av1_mono_aom.yuv /tmp/av1_mono_rust.yuv"
actual="$(ssh "$BOARD" "sha256sum /tmp/av1_mono_rust.yuv | cut -d' ' -f1")"
test "$actual" = "$expected"
printf 'PASS AV1 monochrome matches libaom display output (%s)\n' "$actual"
