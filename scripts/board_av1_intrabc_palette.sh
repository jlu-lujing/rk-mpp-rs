#!/usr/bin/env bash
# Verify AV1 screen-content IntraBC/palette against vendor C on RK3588.
set -euo pipefail

BOARD="${BOARD:?set BOARD to the RK3588 SSH alias}"
remote_repo="${REMOTE_REPO:-\$HOME/rk-mpp-rs}"
mpp_ref="${MPP_REF:-\$HOME/rk-mpp-oracles/mpp-develop}"
fixture=tests/data/av1_intrabc_palette_320x320.ivf
expected=5298dbfbaa36737e32479a6c7330240518c0bb4bea3e915e0f5f9db09f4c22ad

COPYFILE_DISABLE=1 tar --no-xattrs \
  --exclude .git --exclude target --exclude .worktrees --exclude refs \
  -cf - . | ssh "$BOARD" "tar -xf - -C \"$remote_repo\""
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd "$HOME/rk-mpp-rs" && \
  cargo build --release --offline --bin rk-mpp-rs'

ssh "$BOARD" "cd \"$remote_repo\" && sudo rm -f \
  /tmp/av1_intrabc_palette_c.nv12 /tmp/av1_intrabc_palette_c.yuv \
  /tmp/av1_intrabc_palette_rust.yuv"
ssh "$BOARD" "cd \"$remote_repo\" && sudo env LD_LIBRARY_PATH=\"$mpp_ref/build/mpp\" \
  timeout 60 \"$mpp_ref/build/test/mpi_dec_test\" -i $fixture \
  -o /tmp/av1_intrabc_palette_c.nv12 -t 16777224 -w 320 -h 320 -n 3 \
  </dev/null >/tmp/av1_intrabc_palette_c.log 2>&1"
ssh "$BOARD" "ffmpeg -nostdin -y -hide_banner -loglevel error \
  -f rawvideo -pixel_format nv12 -video_size 320x320 \
  -i /tmp/av1_intrabc_palette_c.nv12 -pix_fmt yuv420p -f rawvideo \
  /tmp/av1_intrabc_palette_c.yuv"
ssh "$BOARD" "cd \"$remote_repo\" && timeout 60 \
  ./target/release/rk-mpp-rs av1dec $fixture /tmp/av1_intrabc_palette_rust.yuv \
  >/tmp/av1_intrabc_palette_rust.log 2>&1"
ssh "$BOARD" "test \"\$(stat -c %s /tmp/av1_intrabc_palette_rust.yuv)\" -eq 460800"
ssh "$BOARD" "cmp -s /tmp/av1_intrabc_palette_c.yuv /tmp/av1_intrabc_palette_rust.yuv"
actual="$(ssh "$BOARD" "sha256sum /tmp/av1_intrabc_palette_rust.yuv | cut -d' ' -f1")"
test "$actual" = "$expected"
printf 'PASS AV1 IntraBC/palette matches vendor C (%s)\n' "$actual"
