#!/usr/bin/env bash
BOARD="${BOARD:-t6}"
# Deblocking and the unset initial quality: our encoder against the reference.
#
# Both features are configuration the existing harnesses never touch:
# `mpi_enc_test` of the 2024-04 vintage does not expose the deblocking triple,
# and the rate-control probes all run on the library's default quality, so the
# `cal_first_i_start_qp` derivation - the first picture's QP when the caller
# left `rc:qp_init` unset or at or below zero - is not on any existing
# comparison's path. The reference side therefore runs `tools/h264e_probe.c`,
# built against the same reference build, which sets exactly these knobs
# through `MPP_ENC_SET_CFG` and nothing else.
#
# The version SEI is compared for framing only, as everywhere else. Everything
# else, the slice headers in particular - which is where the deblocking fields
# land - has to match byte for byte.
#
# Usage: scripts/board_h264_deblock_vs_mpp.sh
set -uo pipefail

SIZE="320 240"
FRAMES=6
BPS="300000:250000:350000"

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target --exclude Cargo.lock --exclude .worktrees --exclude refs -cf - . |
  ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release 2>&1 | tail -1'

mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_lib="$mpp_ref/build/mpp"
fixture=/tmp/rk-mpp-rs-move-320x240.nv12

if ! ssh "$BOARD" "test -f $mpp_lib/librockchip_mpp.so.1 || test -f $mpp_lib/librockchip_mpp.so.0"; then
  source "$(cd "$(dirname "$0")" && pwd)/board_dependency.sh" \
    "H.264 deblock" "reference build missing at $mpp_ref"
fi

if ssh "$BOARD" "grep -q bit_target_fix $mpp_ref/inc/mpp_rc_defs.h"; then
  rc_has_bit_target_fix=1
else
  rc_has_bit_target_fix=0
fi

# The probe links the reference's own ROI helper only for symbol availability;
# these cells do not use it. Build once, tolerate either library SONAME.
scp -q tools/h264e_probe.c "$BOARD":/tmp/ 2>/dev/null
ssh "$BOARD" "cc -O2 -w -pthread -DCUSTOM_RC_HAS_BIT_TARGET_FIX=$rc_has_bit_target_fix -I$mpp_ref/inc -I$mpp_ref/utils -I$mpp_ref/osal/inc $mpp_ref/utils/mpp_enc_roi_utils.c /tmp/h264e_probe.c $mpp_lib/librockchip_mpp.so.1 -o /tmp/h264e_probe 2>/dev/null ||
        cc -O2 -w -pthread -DCUSTOM_RC_HAS_BIT_TARGET_FIX=$rc_has_bit_target_fix -I$mpp_ref/inc -I$mpp_ref/utils -I$mpp_ref/osal/inc $mpp_ref/utils/mpp_enc_roi_utils.c /tmp/h264e_probe.c $mpp_lib/librockchip_mpp.so.0 -o /tmp/h264e_probe" || true

ssh "$BOARD" "rm -rf /tmp/rk-mpp-rs-dblk-overlay &&
    mkdir -p /tmp/rk-mpp-rs-dblk-overlay &&
    for name in librockchip_mpp.so librockchip_mpp.so.1 librk_mpp_rs.so librk_mpp_rs.so.1; do
        ln -s \$HOME/rk-mpp-rs/target/release/librk_mpp_rs.so \
            /tmp/rk-mpp-rs-dblk-overlay/\$name;
    done &&
    cc -O2 -w -pthread -DCUSTOM_RC_HAS_BIT_TARGET_FIX=$rc_has_bit_target_fix \
        -I$mpp_ref/inc -I$mpp_ref/utils -I$mpp_ref/osal/inc \
        $mpp_ref/utils/mpp_enc_roi_utils.c /tmp/h264e_probe.c \
        -L/tmp/rk-mpp-rs-dblk-overlay -lrockchip_mpp \
        -Wl,-rpath,/tmp/rk-mpp-rs-dblk-overlay \
        -o /tmp/h264e_cfg_probe_rust"

ssh "$BOARD" "ffmpeg -v error -y -f lavfi -i testsrc=size=320x240:rate=30 -frames:v $FRAMES \
    -pix_fmt nv12 -f rawvideo -an $fixture" >/dev/null 2>&1

fail=0
run_cell() {
  local name="$1" ours_args="$2" probe_args="$3"
  ssh "$BOARD" "sudo rm -f /tmp/dblk_ours_$name.264 /tmp/dblk_c_$name.264 &&
          cd ~/rk-mpp-rs && sudo ./target/release/rk-mpp-rs h264-enc $fixture \
            /tmp/dblk_ours_$name.264 $SIZE $ours_args >/dev/null &&
          sudo env LD_LIBRARY_PATH=$mpp_lib /tmp/h264e_probe $fixture \
            /tmp/dblk_c_$name.264 320 240 $probe_args >/dev/null 2>&1 &&
          sudo chmod 644 /tmp/dblk_ours_$name.264 /tmp/dblk_c_$name.264"
  if ! ssh "$BOARD" "test -s /tmp/dblk_ours_$name.264 && test -s /tmp/dblk_c_$name.264"; then
    printf 'FAIL %s (one side produced no stream)\n' "$name"; fail=1; return
  fi
  scp -q "$BOARD":"/tmp/dblk_ours_$name.264" "$BOARD":"/tmp/dblk_c_$name.264" /tmp/ 2>/dev/null
  if python3 scripts/compare_streams.py "/tmp/dblk_c_$name.264" "/tmp/dblk_ours_$name.264"; then
    printf 'PASS %s\n' "$name"
  else
    printf 'FAIL %s (bitstreams differ)\n' "$name"; fail=1
  fi
}

# Fixed QP 26, single IDR, GOP 1: the slice header is the only thing these
# cells can move, which is exactly the register pair under test.
# `0 0 0` is the all-zeros control the existing matrix covers.
run_cell fixqp_dblk_default "26 baseline 1 60" \
  "1 0 26 288000:1500000:2500000 dblk 0 0 0"
run_cell fixqp_dblk_alpha   "26 baseline 1 60 dblk:1:-3:4" \
  "1 0 26 288000:1500000:2500000 dblk 1 -3 4"
run_cell fixqp_dblk_disable "26 baseline 1 60 dblk:2:2:5" \
  "1 0 26 288000:1500000:2500000 dblk 2 2 5"

run_object_cell() {
  local name="$1" ours_args="$2" probe_args="$3"
  ssh "$BOARD" "sudo rm -f /tmp/dblk_ours_$name.264 /tmp/dblk_obj_$name.264 &&
          cd ~/rk-mpp-rs && sudo ./target/release/rk-mpp-rs h264-enc $fixture \
            /tmp/dblk_ours_$name.264 $SIZE $ours_args >/dev/null &&
          sudo env LD_LIBRARY_PATH=/tmp/rk-mpp-rs-dblk-overlay \
            /tmp/h264e_cfg_probe_rust $fixture /tmp/dblk_obj_$name.264 \
            320 240 $probe_args >/dev/null 2>&1 &&
          sudo chmod 644 /tmp/dblk_ours_$name.264 /tmp/dblk_obj_$name.264"
  if ! ssh "$BOARD" "test -s /tmp/dblk_ours_$name.264 &&
      test -s /tmp/dblk_obj_$name.264"; then
    printf 'FAIL object-mapping/%s (one side produced no stream)\n' "$name"
    fail=1
    return
  fi
  scp -q "$BOARD":"/tmp/dblk_ours_$name.264" \
    "$BOARD":"/tmp/dblk_obj_$name.264" /tmp/ 2>/dev/null
  if python3 scripts/compare_streams.py \
      "/tmp/dblk_ours_$name.264" "/tmp/dblk_obj_$name.264"; then
    printf 'PASS object-mapping/%s\n' "$name"
  else
    printf 'FAIL object-mapping/%s (bitstreams differ)\n' "$name"
    fail=1
  fi
}

run_object_cell fixqp_dblk_alpha "26 baseline 1 60 dblk:1:-3:4" \
  "1 0 26 288000:1500000:2500000 dblk 1 -3 4"
run_object_cell fixqp_dblk_disable "26 baseline 1 60 dblk:2:2:5" \
  "1 0 26 288000:1500000:2500000 dblk 2 2 5"

# The unset initial quality, on CBR: `auto` on our side, qp -1 on the probe's.
# The whole QP trajectory is under test from picture zero: the reference's
# first picture is QP 23 (320x240, 300 blocks, 86956-bit budget) where the
# library default would be 26.
run_cell cbr_auto_qp  "auto baseline $FRAMES 60 cbr:$BPS" \
  "$FRAMES 1 -1 $BPS"
run_cell vbr_auto_qp  "auto baseline $FRAMES 60 vbr:$BPS" \
  "$FRAMES 2 -1 $BPS"

exit "$fail"
