#!/usr/bin/env bash
# Widths above 4096 - the CIME extension lines - against the reference.
#
# The path under test is `setup_hal_bufs` plus `setup_vepu580_ext_line_buf`:
# when the width aligned to 64 clears 4096 the encoder allocates one buffer of
# `(aligned_w - 3072) / 64 * 480` bytes, points `ebuft_addr` (reg182) and
# `ebufb_addr` (reg183) at it, adds the buffer-size offset to reg182's
# multi-offset entry so the second half reaches `ebufb`, and tells the device
# the halves' sizes through the two RCB_INFO calls. The crate used to reject
# this geometry outright; the cells here are the reason for taking it back.
#
# The default is one IDR per cell; `FRAMES=2` adds a P picture and is what closes
# the "P above 4096 width is unverified" item - both wide cells pass with it.
#
# The SPS level bumps itself on both sides for the coded size (level 3.0 at
# 4160x64, 3.2 at 4608x256), which is also under test through the derivation
# the level-matrix run already covers.
#
# Both sides are given an explicit intra GOP of 60: `h264e_probe` hard-codes 60
# and the crate's CLI defaults to 1, so omitting the argument does not leave the
# two sides equivalent - it leaves a one-character difference in the rc-cfg SEI's
# `-g:` field and a cell that fails for the wrong reason. Pass it on both sides.
#
# Usage: FRAMES=2 scripts/board_h264_wide_vs_mpp.sh   # adds the P picture
#        scripts/board_h264_wide_vs_mpp.sh           # IDR-only cells
set -uo pipefail

FRAMES="${FRAMES:-1}"

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target -cf - . |
  ssh t6 'tar -xf - -C ~/rk-mpp-rs'
ssh t6 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release 2>&1 | tail -1'

mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_lib="$mpp_ref/build/mpp"

if ! ssh t6 "test -f $mpp_lib/librockchip_mpp.so.1 || test -f $mpp_lib/librockchip_mpp.so.0"; then
  printf 'SKIP wide (reference build missing at %s)\n' "$mpp_ref"
  exit 0
fi

scp -q tools/h264e_probe.c t6:/tmp/ 2>/dev/null
ssh t6 "cc -O2 -w -I$mpp_ref/inc -I$mpp_ref/utils -I$mpp_ref/osal/inc $mpp_ref/utils/mpp_enc_roi_utils.c \
        /tmp/h264e_probe.c $mpp_lib/librockchip_mpp.so.1 -o /tmp/h264e_probe 2>/dev/null ||
        cc -O2 -w -I$mpp_ref/inc -I$mpp_ref/utils -I$mpp_ref/osal/inc $mpp_ref/utils/mpp_enc_roi_utils.c \
        /tmp/h264e_probe.c $mpp_lib/librockchip_mpp.so.0 -o /tmp/h264e_probe" || true

fail=0
run_cell() {
  local name="$1" width="$2" height="$3"
  local fixture="/tmp/rk-mpp-rs-wide-${width}x${height}.nv12"
  # The `mpi_enc_test` target the crate derives for a caller that passes no
  # `-bps`: width * height / 8 * fps, the library's own bounds either side.
  local bps=$((width * height / 8 * 30))
  ssh t6 "ffmpeg -v error -y -f lavfi -i testsrc=size=${width}x${height}:rate=30 \
      -frames:v $FRAMES -pix_fmt nv12 -f rawvideo -an $fixture" >/dev/null 2>&1
  ssh t6 "sudo rm -f /tmp/wide_ours_$name.264 /tmp/wide_c_$name.264 &&
          cd ~/rk-mpp-rs && sudo ./target/release/rk-mpp-rs h264-enc $fixture \
            /tmp/wide_ours_$name.264 $width $height 26 baseline $FRAMES 60 >/dev/null &&
          sudo env LD_LIBRARY_PATH=$mpp_lib /tmp/h264e_probe $fixture \
            /tmp/wide_c_$name.264 $width $height $FRAMES 0 26 ${bps}:1500000:2500000 >/dev/null 2>&1 &&
          sudo chmod 644 /tmp/wide_ours_$name.264 /tmp/wide_c_$name.264"
  if ! ssh t6 "test -s /tmp/wide_ours_$name.264 && test -s /tmp/wide_c_$name.264"; then
    printf 'FAIL wide/%s (one side produced no stream)\n' "$name"; fail=1; return
  fi
  scp -q t6:"/tmp/wide_ours_$name.264" t6:"/tmp/wide_c_$name.264" /tmp/ 2>/dev/null
  if python3 scripts/compare_streams.py "/tmp/wide_c_$name.264" "/tmp/wide_ours_$name.264"; then
    printf 'PASS wide/%s\n' "$name"
  else
    printf 'FAIL wide/%s (bitstreams differ)\n' "$name"; fail=1
  fi
}

# 4160x64: the smallest even width the 64-alignment carries past 4096 - one
# 64-pixel column of extension lines (17 columns counted from 3072, 8160 bytes).
run_cell w4160x64 4160 64
# 4608x256: a real-ish coded height and 24 columns of extension (11520 bytes),
# still two orders of magnitude faster to encode than a full 8K frame.
run_cell w4608x256 4608 256

exit "$fail"
