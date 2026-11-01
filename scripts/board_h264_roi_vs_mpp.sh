#!/usr/bin/env bash
# ROI / QP map: forced-intra and QP-adjusted regions against the reference.
#
# The regions are the `mpi_enc_test` pair scaled into 320x240 - an absolute
# QP-24 region and an absolute QP-10 force-intra region overlapping it - sent
# through `tools/h264e_probe.c`, which links the reference's own
# `mpp_enc_roi_utils.c` so the region-to-CU-table conversion is byte-identical
# by construction. That is the point of the cell: the two DMA tables
# (`Vepu580RoiH264BsCfg` records and `Vepu580RoiQpCfg` words) and the
# `roi_en`/`roi_addr`/`roi_qp_addr` registers are ours, and if any bit lands
# elsewhere the forced-intra blocks and the QP trajectory of the region
# pictures diverge on the very first frame.
#
# Note what is NOT on this path: `setup_vepu580_rc_base` writes the
# picture-wide `qp_min`/`qp_max` into all eight `roi_qthd` rows whether or not
# a caller configured regions - the per-region QP never reaches those
# registers on VEPU580 - so this crate keeps those rows uniform too, and the
# comparison exercises the table path instead.
#
# The version SEI is compared for framing only, as everywhere else.
#
# Usage: scripts/board_h264_roi_vs_mpp.sh
set -uo pipefail

SIZE="320 240"
FRAMES=6
BPS="300000:250000:350000"
ROI1="48 32 128 160 0 1 24"
ROI2="160 64 96 128 1 1 10"
OURS_ROI1="roi:48:32:128:160:0:1:24"
OURS_ROI2="roi:160:64:96:128:1:1:10"
fixture=/tmp/rk-mpp-rs-move-320x240.nv12

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target -cf - . |
  ssh t6 'tar -xf - -C ~/rk-mpp-rs'
ssh t6 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release 2>&1 | tail -1'

mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_lib="$mpp_ref/build/mpp"

if ! ssh t6 "test -f $mpp_lib/librockchip_mpp.so.1 || test -f $mpp_lib/librockchip_mpp.so.0"; then
  printf 'SKIP roi (reference build missing at %s)\n' "$mpp_ref"
  exit 0
fi

# The probe is linked against the reference tree's own ROI helper; on mpp-old
# that file exists next to the same header.
scp -q tools/h264e_probe.c t6:/tmp/ 2>/dev/null
ssh t6 "cc -O2 -w -I$mpp_ref/inc -I$mpp_ref/utils $mpp_ref/utils/mpp_enc_roi_utils.c \
        /tmp/h264e_probe.c $mpp_lib/librockchip_mpp.so.1 -o /tmp/h264e_probe 2>/dev/null ||
        cc -O2 -w -I$mpp_ref/inc -I$mpp_ref/utils $mpp_ref/utils/mpp_enc_roi_utils.c \
        /tmp/h264e_probe.c $mpp_lib/librockchip_mpp.so.0 -o /tmp/h264e_probe" || true

ssh t6 "ffmpeg -v error -y -f lavfi -i testsrc=size=320x240:rate=30 -frames:v $FRAMES \
    -pix_fmt nv12 -f rawvideo -an $fixture" >/dev/null 2>&1

fail=0
run_cell() {
  local name="$1" ours_args="$2" probe_args="$3"
  ssh t6 "sudo rm -f /tmp/roi_ours_$name.264 /tmp/roi_c_$name.264 &&
          cd ~/rk-mpp-rs && sudo ./target/release/rk-mpp-rs h264-enc $fixture \
            /tmp/roi_ours_$name.264 $SIZE $ours_args >/dev/null &&
          sudo env LD_LIBRARY_PATH=$mpp_lib /tmp/h264e_probe $fixture \
            /tmp/roi_c_$name.264 320 240 $probe_args >/dev/null 2>&1 &&
          sudo chmod 644 /tmp/roi_ours_$name.264 /tmp/roi_c_$name.264"
  if ! ssh t6 "test -s /tmp/roi_ours_$name.264 && test -s /tmp/roi_c_$name.264"; then
    printf 'FAIL roi/%s (one side produced no stream)\n' "$name"; fail=1; return
  fi
  scp -q t6:"/tmp/roi_ours_$name.264" t6:"/tmp/roi_c_$name.264" /tmp/ 2>/dev/null
  if python3 scripts/compare_streams.py "/tmp/roi_c_$name.264" "/tmp/roi_ours_$name.264"; then
    printf 'PASS roi/%s\n' "$name"
  else
    printf 'FAIL roi/%s (bitstreams differ)\n' "$name"; fail=1
  fi
}

# Fixed QP 26 with both regions: the force-intra blocks and the absolute-QP
# blocks change the coded picture, and every byte of it is compared.
run_cell fixqp "26 baseline $FRAMES 60 $OURS_ROI1 $OURS_ROI2" \
  "$FRAMES 0 26 288000:1500000:2500000 roi $ROI1 roi $ROI2"
# The same regions under CBR, where the QP map rides on top of the rate
# controller's own trajectory.
run_cell cbr "auto baseline $FRAMES 60 cbr:$BPS $OURS_ROI1 $OURS_ROI2" \
  "$FRAMES 1 -1 $BPS roi $ROI1 roi $ROI2"

exit "$fail"
