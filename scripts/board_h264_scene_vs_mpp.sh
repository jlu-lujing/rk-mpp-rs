#!/usr/bin/env bash
BOARD="${BOARD:-t6}"
# Scene changes: the detector's register patch, our encoder against the reference.
#
# The fixture is a hard cut: the first half of the clip is `testsrc`, the second
# half `testsrc2`. The C encoder's per-picture block statistics push three
# large-motion scores into the `md_flag_matrix`, which raises
# `curr_scene_motion_flag` and moves the ATR weights, the search multipliers,
# the 341 quant bias and the flattened intra-cost thresholds to the
# current-motion row - `vepu580_h264e_tune_stat_update` plus
# `vepu580_h264e_tune_reg_patch`. Every previous fixture of this repository is
# a still or uniformly moving sequence, which is exactly why none of them ever
# exercised that branch.
#
# Fixed QP is deliberate: the detector's inputs are hardware block statistics,
# not the rate-control trajectory, so the cell isolates the register patch from
# everything the CBR suite already covers.
#
# The version SEI is compared for framing only, as everywhere else.
#
# Usage: scripts/board_h264_scene_vs_mpp.sh
set -uo pipefail

SIZE="320 240"
FRAMES=8
fixture=/tmp/rk-mpp-rs-cut-320x240.nv12

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target --exclude Cargo.lock --exclude .worktrees --exclude refs -cf - . |
  ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release 2>&1 | tail -1'

mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_lib="$mpp_ref/build/mpp"

if ! ssh "$BOARD" "test -f $mpp_lib/librockchip_mpp.so.1 || test -f $mpp_lib/librockchip_mpp.so.0"; then
  source "$(cd "$(dirname "$0")" && pwd)/board_dependency.sh" \
    "H.264 scene" "reference build missing at $mpp_ref"
fi

scp -q tools/h264e_probe.c "$BOARD":/tmp/ 2>/dev/null
ssh "$BOARD" "cc -O2 -w -pthread -I$mpp_ref/inc -I$mpp_ref/utils -I$mpp_ref/osal/inc $mpp_ref/utils/mpp_enc_roi_utils.c /tmp/h264e_probe.c $mpp_lib/librockchip_mpp.so.1 -o /tmp/h264e_probe 2>/dev/null ||
        cc -O2 -w -pthread -I$mpp_ref/inc -I$mpp_ref/utils -I$mpp_ref/osal/inc $mpp_ref/utils/mpp_enc_roi_utils.c /tmp/h264e_probe.c $mpp_lib/librockchip_mpp.so.0 -o /tmp/h264e_probe" || true

# The cut at frame FRAMES/2 is the point the detector should start reacting;
# both halves are 16-multiple 320x240 NV12 from the same generator family.
ssh "$BOARD" "ffmpeg -v error -y -f lavfi -i testsrc=size=320x240:rate=30 -frames:v 4 \
      -pix_fmt nv12 -f rawvideo -an /tmp/rk-mpp-rs-cut-a.nv12 &&
      ffmpeg -v error -y -f lavfi -i testsrc2=size=320x240:rate=30 -frames:v 4 \
      -pix_fmt nv12 -f rawvideo -an /tmp/rk-mpp-rs-cut-b.nv12 &&
      cp /tmp/rk-mpp-rs-cut-a.nv12 $fixture &&
      cat /tmp/rk-mpp-rs-cut-b.nv12 >> $fixture" >/dev/null 2>&1

ssh "$BOARD" "sudo rm -f /tmp/scene_ours.264 /tmp/scene_c.264 &&
        cd ~/rk-mpp-rs && sudo ./target/release/rk-mpp-rs h264-enc $fixture \
          /tmp/scene_ours.264 $SIZE 26 baseline $FRAMES 60 >/dev/null &&
        sudo env LD_LIBRARY_PATH=$mpp_lib /tmp/h264e_probe $fixture \
          /tmp/scene_c.264 320 240 $FRAMES 0 26 288000:1500000:2500000 >/dev/null 2>&1 &&
        sudo chmod 644 /tmp/scene_ours.264 /tmp/scene_c.264"

fail=0
if ! ssh "$BOARD" "test -s /tmp/scene_ours.264 && test -s /tmp/scene_c.264"; then
  printf 'FAIL scene (one side produced no stream)\n'; fail=1
else
  scp -q "$BOARD":"/tmp/scene_ours.264" "$BOARD":"/tmp/scene_c.264" /tmp/ 2>/dev/null
  if python3 scripts/compare_streams.py /tmp/scene_c.264 /tmp/scene_ours.264; then
    printf 'PASS scene (hard cut, %d frames fixed QP 26)\n' "$FRAMES"
  else
    printf 'FAIL scene (bitstreams differ on the cut sequence)\n'; fail=1
  fi
fi

exit "$fail"
