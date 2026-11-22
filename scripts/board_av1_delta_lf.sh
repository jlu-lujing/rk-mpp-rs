#!/usr/bin/env bash
# Verify AV1 delta-LF fixtures against vendor C on RK3588.
set -euo pipefail

BOARD="${BOARD:?set BOARD to the RK3588 SSH alias}"
remote_repo="${REMOTE_REPO:-\$HOME/rk-mpp-rs}"
mpp_ref="${MPP_REF:-\$HOME/rk-mpp-oracles/mpp-develop}"

COPYFILE_DISABLE=1 tar --no-xattrs \
  --exclude .git --exclude target --exclude .worktrees --exclude refs \
  -cf - . | ssh "$BOARD" "tar -xf - -C \"$remote_repo\""
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd "$HOME/rk-mpp-rs" && \
  cargo build --release --offline --bin rk-mpp-rs'

run_case() {
  local name="$1"
  local expected="$2"
  local fixture="tests/data/$name"
  local c="/tmp/${name%.ivf}_c.nv12"
  local cy="/tmp/${name%.ivf}_c.yuv"
  local rust="/tmp/${name%.ivf}_rust.yuv"

  ssh "$BOARD" "cd \"$remote_repo\" && sudo rm -f '$c' '$cy' '$rust'"
  ssh "$BOARD" "cd \"$remote_repo\" && sudo env LD_LIBRARY_PATH=\"$mpp_ref/build/mpp\" \
    timeout 30 \"$mpp_ref/build/test/mpi_dec_test\" -i $fixture -o '$c' \
    -t 16777224 -w 320 -h 240 -n 1 </dev/null >/tmp/${name%.ivf}_c.log 2>&1"
  ssh "$BOARD" "ffmpeg -nostdin -y -hide_banner -loglevel error \
    -f rawvideo -pixel_format nv12 -video_size 320x240 -i '$c' \
    -pix_fmt yuv420p -f rawvideo '$cy'"
  ssh "$BOARD" "cd \"$remote_repo\" && timeout 30 \
    ./target/release/rk-mpp-rs av1dec $fixture '$rust' \
    >/tmp/${name%.ivf}_rust.log 2>&1"
  ssh "$BOARD" "cmp -s '$cy' '$rust'"
  local actual
  actual="$(ssh "$BOARD" "sha256sum '$rust' | cut -d' ' -f1")"
  test "$actual" = "$expected"
  printf 'PASS AV1 %-34s %s\n' "$name" "$actual"
}

run_case av1_delta_lf_320x240.ivf \
  55d42329e17e3aa1e31eee6434481e858ada9bf0ff283713132b5339cb7d84d2
run_case av1_delta_lf_multi_key_320x240.ivf \
  4fd081edb7454717ba162b3962fcb238acc6b6faaf0c22ffd31e57edb0ebda79
