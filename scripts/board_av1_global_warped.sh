#!/usr/bin/env bash
# Verify AV1 global-warped motion against vendor C on RK3588.
set -euo pipefail

BOARD="${BOARD:?set BOARD to the RK3588 SSH alias}"
remote_repo="${REMOTE_REPO:-\$HOME/rk-mpp-rs}"
mpp_ref="${MPP_REF:-\$HOME/rk-mpp-oracles/mpp-develop}"
fixture=tests/data/av1_inter_global_warped_320x240.ivf
expected=ce1e081f7f3001b8ca5865176c8a942ae1c10c81bd2d99340ec87b09e4a9f6f5

COPYFILE_DISABLE=1 tar --no-xattrs \
  --exclude .git --exclude target --exclude .worktrees --exclude refs \
  -cf - . | ssh "$BOARD" "tar -xf - -C \"$remote_repo\""
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd "$HOME/rk-mpp-rs" && \
  cargo build --release --offline --bin rk-mpp-rs'

ssh "$BOARD" "cd \"$remote_repo\" && sudo rm -f \
  /tmp/av1_global_warped_c.nv12 /tmp/av1_global_warped_c.yuv \
  /tmp/av1_global_warped_rust.yuv"
ssh "$BOARD" "cd \"$remote_repo\" && sudo env LD_LIBRARY_PATH=\"$mpp_ref/build/mpp\" \
  timeout 60 \"$mpp_ref/build/test/mpi_dec_test\" -i $fixture \
  -o /tmp/av1_global_warped_c.nv12 -t 16777224 -w 320 -h 240 -n 2 \
  </dev/null >/tmp/av1_global_warped_c.log 2>&1"
ssh "$BOARD" "ffmpeg -nostdin -y -hide_banner -loglevel error \
  -f rawvideo -pixel_format nv12 -video_size 320x240 \
  -i /tmp/av1_global_warped_c.nv12 -pix_fmt yuv420p -f rawvideo \
  /tmp/av1_global_warped_c.yuv"
ssh "$BOARD" "cd \"$remote_repo\" && timeout 60 \
  ./target/release/rk-mpp-rs av1dec $fixture /tmp/av1_global_warped_rust.yuv \
  >/tmp/av1_global_warped_rust.log 2>&1"
ssh "$BOARD" "test \"\$(stat -c %s /tmp/av1_global_warped_rust.yuv)\" -eq 230400"
ssh "$BOARD" "cmp -s /tmp/av1_global_warped_c.yuv /tmp/av1_global_warped_rust.yuv"
actual="$(ssh "$BOARD" "sha256sum /tmp/av1_global_warped_rust.yuv | cut -d' ' -f1")"
test "$actual" = "$expected"
printf 'PASS AV1 global-warped motion matches vendor C (%s)\n' "$actual"
