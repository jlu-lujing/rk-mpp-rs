#!/usr/bin/env bash
BOARD="${BOARD:-t6}"
# H.264 low-delay packet output against the 2024-04 C oracle.
#
# The comparison pins both halves of MPP_ENC_SPLIT_OUT_LOWDELAY:
#   * slen_fifo=1 and slice interrupts make the hardware publish one length per
#     completed slice;
#   * the caller drains each packet through EOI while the picture is running.
#
# The probe reads packets from a second thread. MPP has a fixed-size output task
# pool, so a synchronous put-then-get caller deadlocks once one picture emits
# more packets than that pool can hold. The 38-slice cell exercises this path
# and also crosses VEPU580's 32-entry slice FIFO.
#
# Usage: BOARD=t6 scripts/board_h264_lowdelay_vs_mpp.sh
set -uo pipefail

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target --exclude Cargo.lock --exclude .worktrees --exclude refs --exclude '._*' -cf - . |
  ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release 2>&1 | tail -1'

mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_lib="$mpp_ref/build/mpp"
fixture=/tmp/rk-mpp-rs-lowdelay-320x240-6.nv12
if ! ssh "$BOARD" "test -f $mpp_lib/librockchip_mpp.so.1 || test -f $mpp_lib/librockchip_mpp.so.0"; then
  printf 'SKIP lowdelay (reference build missing at %s)\n' "$mpp_ref"
  exit 0
fi

if ssh "$BOARD" "grep -q 'bit_target_fix' $mpp_ref/inc/mpp_rc_defs.h"; then
  rc_has_bit_target_fix=1
else
  rc_has_bit_target_fix=0
fi

scp -q tools/h264e_probe.c "$BOARD":/tmp/h264e_probe_low.c
ssh "$BOARD" "cc -O2 -w -pthread -DCUSTOM_RC_HAS_BIT_TARGET_FIX=$rc_has_bit_target_fix -I$mpp_ref/inc -I$mpp_ref/utils -I$mpp_ref/osal/inc \
        $mpp_ref/utils/mpp_enc_roi_utils.c /tmp/h264e_probe_low.c \
        $mpp_lib/librockchip_mpp.so.1 -o /tmp/h264e_probe_low 2>/dev/null ||
      cc -O2 -w -pthread -DCUSTOM_RC_HAS_BIT_TARGET_FIX=$rc_has_bit_target_fix -I$mpp_ref/inc -I$mpp_ref/utils -I$mpp_ref/osal/inc \
        $mpp_ref/utils/mpp_enc_roi_utils.c /tmp/h264e_probe_low.c \
        $mpp_lib/librockchip_mpp.so.0 -o /tmp/h264e_probe_low"

ssh "$BOARD" "ffmpeg -v error -y -f lavfi -i testsrc=size=320x240:rate=30 \
    -frames:v 6 -pix_fmt nv12 -f rawvideo -an $fixture"

fail=0
run_cell() {
  local name="$1" frames="$2" ours_args="$3" probe_args="$4"
  local c_stream=/tmp/ld_c_$name.264
  local rust_stream=/tmp/ld_rust_$name.264
  local c_log=/tmp/ld_c_$name.log
  local rust_log=/tmp/ld_rust_$name.log

  if ! ssh "$BOARD" "sudo rm -f $c_stream $rust_stream $c_log $rust_log &&
      sudo env LD_LIBRARY_PATH=$mpp_lib timeout 60 /tmp/h264e_probe_low \
        $fixture $c_stream 320 240 $probe_args splitout 1 \
        2>$c_log &&
      cd ~/rk-mpp-rs &&
      sudo env RK_MPP_TRACE_PACKETS=1 ./target/release/rk-mpp-rs h264-enc \
        $fixture $rust_stream 320 240 $ours_args splitout:lowdelay \
        2>$rust_log"; then
    printf 'FAIL lowdelay/%s (encoder failed)\n' "$name"
    fail=1
    return
  fi

  scp -q "$BOARD":"$c_stream" "$BOARD":"$rust_stream" \
    "$BOARD":"$c_log" "$BOARD":"$rust_log" /tmp/
  if ! python3 scripts/compare_streams.py "/tmp/ld_c_$name.264" "/tmp/ld_rust_$name.264"; then
    printf 'FAIL lowdelay/%s (bitstreams differ)\n' "$name"
    fail=1
    return
  fi
  if ! python3 scripts/compare_packet_sizes.py "/tmp/ld_c_$name.log" "/tmp/ld_rust_$name.log"; then
    printf 'FAIL lowdelay/%s (packet boundaries differ)\n' "$name"
    fail=1
    return
  fi
  printf 'PASS lowdelay/%s\n' "$name"
}

# Four slices, two slices, and a byte cap exercise both split-mode register
# branches without crossing the slice FIFO.
run_cell ctu_96 1 "26 baseline 1 60 split:mb:96" \
  "1 0 26 288000:1500000:2500000 split 2 96"
run_cell ctu_150 1 "26 baseline 1 60 split:mb:150" \
  "1 0 26 288000:1500000:2500000 split 2 150"
run_cell byte_256 1 "26 baseline 1 60 split:byte:256" \
  "1 0 26 288000:1500000:2500000 split 1 256"
# 38 slices cross the 32-entry hardware FIFO and MPP's output-task pool.
run_cell ctu_8 1 "26 baseline 1 60 split:mb:8" \
  "1 0 26 288000:1500000:2500000 split 2 8"
# Six distinct pictures cover the IDR packet plus the P-picture packet path.
run_cell p6_ctu_96 6 "26 baseline 6 60 split:mb:96" \
  "6 0 26 288000:1500000:2500000 split 2 96"

exit "$fail"
