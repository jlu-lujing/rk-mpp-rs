#!/usr/bin/env bash
BOARD="${BOARD:-t6}"
# Slice split (BY_CTU and BY_BYTE) against the reference, one AU per picture.
#
# What the registers have to say is `setup_vepu580_split` with `split_out = 0`:
# `sli_splt` on, mode 1 for BY_CTU and 0 for BY_BYTE, `sli_max_num_m1 = 500`,
# `sli_flsh = 1`, the block count in `sli_splt_cnum_m1` or the byte cap in
# `sli_splt_byte`, `slen_fifo` down, and `slc_done_en` armed unconditionally
# for BY_BYTE and only past the 32-entry slice FIFO for BY_CTU.
#
# The output question is settled the same way on both sides: `split_out = 0`
# means the reference waits on the normal enc_done poll, reads one STATUS
# length and hands out one packet - `slen_fifo = 0` and no per-slice
# slice-length readback. So the hardware writes every slice of the picture
# contiguously into the bitstream buffer, and comparing the concatenated
# picture files byte for byte compares exactly the bytes each encoder laid
# down. Per-slice packet output is covered separately by
# `board_h264_lowdelay_vs_mpp.sh`.
#
# BY_CTU 8 blocks per slice puts 38 slices over a 320x240 picture, past the
# FIFO length, so the 38-slice cell additionally pins the `slc_done_en` bit -
# if the interrupt state made the difference the streams would not even finish.
#
# Usage: scripts/board_h264_slice_vs_mpp.sh
set -uo pipefail

SIZE="320 240"
FRAMES=6
GOP=60
fixture=/tmp/rk-mpp-rs-move-320x240.nv12

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target --exclude Cargo.lock --exclude .worktrees --exclude refs -cf - . |
  ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release 2>&1 | tail -1'

mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_lib="$mpp_ref/build/mpp"

if ! ssh "$BOARD" "test -f $mpp_lib/librockchip_mpp.so.1 || test -f $mpp_lib/librockchip_mpp.so.0"; then
  printf 'SKIP slice (reference build missing at %s)\n' "$mpp_ref"
  exit 0
fi

if ssh "$BOARD" "grep -q 'bit_target_fix' $mpp_ref/inc/mpp_rc_defs.h"; then
  rc_has_bit_target_fix=1
else
  rc_has_bit_target_fix=0
fi

scp -q tools/h264e_probe.c "$BOARD":/tmp/ 2>/dev/null
ssh "$BOARD" "cc -O2 -w -pthread -DCUSTOM_RC_HAS_BIT_TARGET_FIX=$rc_has_bit_target_fix -I$mpp_ref/inc -I$mpp_ref/utils -I$mpp_ref/osal/inc $mpp_ref/utils/mpp_enc_roi_utils.c \
        /tmp/h264e_probe.c $mpp_lib/librockchip_mpp.so.1 -o /tmp/h264e_probe 2>/dev/null ||
        cc -O2 -w -pthread -DCUSTOM_RC_HAS_BIT_TARGET_FIX=$rc_has_bit_target_fix -I$mpp_ref/inc -I$mpp_ref/utils -I$mpp_ref/osal/inc $mpp_ref/utils/mpp_enc_roi_utils.c \
        /tmp/h264e_probe.c $mpp_lib/librockchip_mpp.so.0 -o /tmp/h264e_probe" || true

ssh "$BOARD" "ffmpeg -v error -y -f lavfi -i testsrc=size=320x240:rate=30 -frames:v $FRAMES \
    -pix_fmt nv12 -f rawvideo -an $fixture" >/dev/null 2>&1

fail=0
run_cell() {
  local name="$1" ours_args="$2" probe_args="$3"
  ssh "$BOARD" "sudo rm -f /tmp/sli_ours_$name.264 /tmp/sli_c_$name.264 &&
          cd ~/rk-mpp-rs && sudo ./target/release/rk-mpp-rs h264-enc $fixture \
            /tmp/sli_ours_$name.264 $SIZE $ours_args >/dev/null &&
          sudo env LD_LIBRARY_PATH=$mpp_lib /tmp/h264e_probe $fixture \
            /tmp/sli_c_$name.264 320 240 $probe_args >/dev/null 2>&1 &&
          sudo chmod 644 /tmp/sli_ours_$name.264 /tmp/sli_c_$name.264"
  if ! ssh "$BOARD" "test -s /tmp/sli_ours_$name.264 && test -s /tmp/sli_c_$name.264"; then
    printf 'FAIL slice/%s (one side produced no stream)\n' "$name"; fail=1; return
  fi
  scp -q "$BOARD":"/tmp/sli_ours_$name.264" "$BOARD":"/tmp/sli_c_$name.264" /tmp/ 2>/dev/null
  if python3 scripts/compare_streams.py "/tmp/sli_c_$name.264" "/tmp/sli_ours_$name.264"; then
    printf 'PASS slice/%s\n' "$name"
  else
    printf 'FAIL slice/%s (bitstreams differ)\n' "$name"; fail=1
  fi
}

# BY_CTU, 96 macroblocks a slice: 300/96 is 4 slices over the picture, every
# picture in the run carries 4 (IDR) or 4 (P) slice NALs where it carried 1.
run_cell ctu_96 "26 baseline $FRAMES $GOP split:mb:96" \
  "$FRAMES 0 26 288000:1500000:2500000 split 2 96"
# BY_CTU past the slice FIFO: 8 blocks a slice, 38 slices, the branch that
# arms `slc_done_en` for BY_CTU.
run_cell ctu_8 "26 baseline $FRAMES $GOP split:mb:8" \
  "$FRAMES 0 26 288000:1500000:2500000 split 2 8"
# BY_CTU an exact divisor: 150 macroblocks, two slices per picture.
run_cell ctu_150 "26 baseline $FRAMES $GOP split:mb:150" \
  "$FRAMES 0 26 288000:1500000:2500000 split 2 150"
# BY_BYTE, 256 bytes a slice: the cap branch, mode 0, `sli_splt_byte`, and the
# always-on slice interrupt.
run_cell byte_256 "26 baseline $FRAMES $GOP split:byte:256" \
  "$FRAMES 0 26 288000:1500000:2500000 split 1 256"

exit "$fail"
