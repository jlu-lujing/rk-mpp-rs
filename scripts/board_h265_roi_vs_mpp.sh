#!/usr/bin/env bash
BOARD="${BOARD:-t6}"
# H.265 encoder ROI/QP-map boards against the 2024-04 C oracle.
#
# The C probe links the reference's own mpp_enc_roi_utils.c, so region rounding,
# CTU marking, CU64/CU32/CU16/CU8 expansion, force-split and both QP tables are
# all produced by the source under comparison.
#
# Usage: BOARD=t6 scripts/board_h265_roi_vs_mpp.sh
set -uo pipefail

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target --exclude Cargo.lock --exclude .worktrees --exclude refs --exclude '._*' -cf - . |
  ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release 2>&1 | tail -1'

mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_lib="$mpp_ref/build/mpp"
if ! ssh "$BOARD" "test -f $mpp_lib/librockchip_mpp.so.1 || test -f $mpp_lib/librockchip_mpp.so.0"; then
  source "$(cd "$(dirname "$0")" && pwd)/board_dependency.sh" \
    "H.265 ROI" "reference build missing at $mpp_ref"
fi

scp -q tools/h265e_probe.c "$BOARD":/tmp/h265e_probe_roi.c
ssh "$BOARD" "cc -O2 -w -pthread -DH265E_ROI \
        -I$mpp_ref/inc -I$mpp_ref/utils -I$mpp_ref/osal/inc \
        $mpp_ref/utils/mpp_enc_roi_utils.c /tmp/h265e_probe_roi.c \
        $mpp_lib/librockchip_mpp.so.1 -o /tmp/h265e_probe_roi 2>/dev/null ||
      cc -O2 -w -pthread -DH265E_ROI \
        -I$mpp_ref/inc -I$mpp_ref/utils -I$mpp_ref/osal/inc \
        $mpp_ref/utils/mpp_enc_roi_utils.c /tmp/h265e_probe_roi.c \
        $mpp_lib/librockchip_mpp.so.0 -o /tmp/h265e_probe_roi"

fail=0
run_cell() {
  local name="$1" width="$2" height="$3" frames="$4"
  shift 4
  local frame="/tmp/rk-mpp-rs-h265-roi-${width}x${height}-${frames}.nv12"
  local c_stream="/tmp/h265_roi_c_$name.265"
  local rust_stream="/tmp/h265_roi_rust_$name.265"
  local c_regions=""
  local rust_regions=""
  local bps=$((width * height * 30 / 8))
  local region x y w h intra absolute qp

  for region in "$@"; do
    IFS=: read -r x y w h intra absolute qp <<< "$region"
    c_regions+=" roi $x $y $w $h $intra $absolute $qp"
    rust_regions+=" roi:$region"
  done

  ssh "$BOARD" "ffmpeg -v error -y -f lavfi -i testsrc2=size=${width}x${height}:rate=30 \
      -frames:v $frames -pix_fmt nv12 -f rawvideo -an $frame"
  if ! ssh "$BOARD" "sudo rm -f $c_stream $rust_stream &&
      sudo env LD_LIBRARY_PATH=$mpp_lib REF_CFG=1 FRAMES=$frames GOP=60 timeout 120 \
        /tmp/h265e_probe_roi $frame $c_stream $width $height 0 26 \
        $bps:1500000:2500000 $c_regions >/dev/null 2>&1 &&
      cd ~/rk-mpp-rs &&
      sudo timeout 120 ./target/release/rk-mpp-rs h265-gop \
        $frame $rust_stream $width $height $frames 26 shortterm \
        $rust_regions >/dev/null"; then
    printf 'FAIL h265-roi/%s (encoder failed)\n' "$name"
    fail=1
    return
  fi

  scp -q "$BOARD":"$c_stream" "$BOARD":"$rust_stream" /tmp/
  if ! python3 scripts/compare_streams.py "/tmp/h265_roi_c_$name.265" \
      "/tmp/h265_roi_rust_$name.265"; then
    printf 'FAIL h265-roi/%s (bitstreams differ)\n' "$name"
    fail=1
    return
  fi
  printf 'PASS h265-roi/%s\n' "$name"
}

# Relative QP over a partially covered CTU exercises force_split.
run_cell relative_partial 320 240 6 16:16:96:64:0:0:-3
# Absolute QP plus force intra covers a larger region.
run_cell absolute_intra 320 240 6 64:32:128:96:1:1:20
# Two disconnected regions must merge into the same CTU mask/table.
run_cell multi_region 320 240 6 0:0:64:64:1:0:2 128:96:160:128:0:1:30
# A larger CTU grid pins the per-CTU 64-byte/256-byte record strides.
run_cell large_640 640 480 6 64:64:320:192:1:1:20

exit "$fail"
