#!/usr/bin/env bash
# Verify the VEPU580 H.265 scene-tune state machine over multiple GOPs.
#
# The moving 320x240 fixture crosses the C detector's row-2 -> row-3
# transitions at frames 107 and 227. A one-frame or single-GOP gate cannot
# exercise that path, so this compares the complete 300-frame Annex-B stream.
set -euo pipefail

BOARD="${BOARD:?set BOARD to the RK3588 SSH alias}"
repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
remote_repo="${REMOTE_REPO:-\$HOME/rk-mpp-rs}"
mpp_ref="${MPP_REF:-\$HOME/rk-mpp-oracles/mpp-develop}"
fixture="${FIXTURE:-/tmp/rk_mpp_h265_scene_tune_300.nv12}"
c_stream=/tmp/rk_mpp_h265_scene_tune_300_c.265
r_stream=/tmp/rk_mpp_h265_scene_tune_300_r.265
enc_test="$mpp_ref/build/test/mpi_enc_test"
mpp_lib="$mpp_ref/build/mpp"
rust_lib="$remote_repo/target/release/librk_mpp_rs.so"

ssh "$BOARD" "ffmpeg -nostdin -y -hide_banner -loglevel error \
  -f lavfi -i testsrc2=size=320x240:rate=30 -frames:v 300 \
  -pix_fmt nv12 -f rawvideo '$fixture'"

ssh "$BOARD" "cd \"$remote_repo\" && \"\$HOME/.cargo/bin/cargo\" build --release"

ssh "$BOARD" "sudo env LD_LIBRARY_PATH=\"$mpp_lib\" timeout 120 \"$enc_test\" \
  -i '$fixture' -o '$c_stream' -w 320 -h 240 -n 300 -rc 2 -fps 30 \
  -g 0:60 -t 16777220 -qc 26 </dev/null >/tmp/rk_mpp_h265_scene_tune_300_c.log 2>&1"

ssh "$BOARD" "sudo env LD_PRELOAD=\"$rust_lib\" LD_LIBRARY_PATH=\"$mpp_lib\" \
  timeout 120 \"$enc_test\" -i '$fixture' -o '$r_stream' -w 320 -h 240 \
  -n 300 -rc 2 -fps 30 -g 0:60 -t 16777220 -qc 26 \
  </dev/null >/tmp/rk_mpp_h265_scene_tune_300_r.log 2>&1"

scp -q "$repo/scripts/compare_streams.py" \
  "$BOARD":/tmp/rk_mpp_compare_streams.py
ssh "$BOARD" "python3 /tmp/rk_mpp_compare_streams.py '$c_stream' '$r_stream'"
ssh "$BOARD" "sha256sum '$c_stream' '$r_stream'"
printf 'PASS H.265 VEPU580 scene-tune 300 frames\n'
