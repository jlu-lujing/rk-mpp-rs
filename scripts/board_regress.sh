#!/usr/bin/env bash
# H.264 regression: sync, build on the board, decode every stream, compare
# with the reference decoder output.
# Usage: scripts/board_regress.sh
set -euo pipefail

streams=(
  "/tmp/test720.264:/tmp/o720_ffmpeg.yuv"
  "/tmp/test_hard.264:/tmp/o1080_hard_ffmpeg.yuv"
  "/tmp/test720_slices.264:/tmp/o_slices_ref_planar.yuv"
  "/tmp/test720_i.264:/tmp/o_i_ffmpeg.yuv"
  "/tmp/field_paff.264:/tmp/o_paff_ffmpeg.yuv"
  "/tmp/mr1_longterm.264:/tmp/o_mr1_ffmpeg.yuv"
  "/tmp/mr1_missing_au20.264:/tmp/o_mr1_missing_mpp_planar.yuv"
)

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target -cf - . |
  ssh t6 'tar -xf - -C ~/rk-mpp-rs'
ssh t6 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release'

fail=0
for pair in "${streams[@]}"; do
  in="${pair%%:*}"
  ref="${pair##*:}"
  out="${ref%.yuv}_rust.yuv"
  if ! ssh t6 "test -f $in"; then
    printf 'SKIP %s (stream missing)\n' "$in"
    continue
  fi
  if ! ssh t6 "test -f $ref"; then
    printf 'SKIP %s (reference output missing)\n' "$in"
    continue
  fi
  ssh t6 "sudo rm -f $out"
  if ! ssh t6 "cd ~/rk-mpp-rs && sudo ./target/release/rk-mpp-rs h264dec $in $out" >/dev/null 2>&1; then
    printf 'FAIL %s (decoder exited non-zero)\n' "$in"
    fail=1
    continue
  fi
  if ssh t6 "cmp -s $ref $out"; then
    printf 'PASS %s\n' "$in"
  else
    printf 'FAIL %s\n' "$in"
    fail=1
  fi
done
exit $fail
