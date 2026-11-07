#!/usr/bin/env bash
BOARD="${BOARD:-t6}"
# H.265 low-delay packet output against the 2024-04 C oracle.
#
# The comparison covers:
#   * BY_CTU and BY_BYTE split modes;
#   * slen_fifo + one-entry POLL_HW_IRQ draining;
#   * a 40-slice picture, crossing VEPU580's 32-entry slice FIFO;
#   * one IDR and a six-picture short-term IDR+P run.
#
# The C probe reads packets on a worker thread. That avoids MPP's fixed output
# task-pool self-lock when one picture emits more packets than the pool holds.
#
# Usage: BOARD=t6 scripts/board_h265_lowdelay_vs_mpp.sh
set -uo pipefail

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target --exclude Cargo.lock --exclude .worktrees --exclude refs --exclude '._*' -cf - . |
  ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release 2>&1 | tail -1'

mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_lib="$mpp_ref/build/mpp"
if ! ssh "$BOARD" "test -f $mpp_lib/librockchip_mpp.so.1 || test -f $mpp_lib/librockchip_mpp.so.0"; then
  printf 'SKIP H.265 lowdelay (reference build missing at %s)\n' "$mpp_ref"
  exit 0
fi

scp -q tools/h265e_probe.c "$BOARD":/tmp/h265e_probe_lowdelay.c
ssh "$BOARD" "cc -O2 -w -pthread -I$mpp_ref/inc /tmp/h265e_probe_lowdelay.c \
        $mpp_lib/librockchip_mpp.so.1 -o /tmp/h265e_probe_lowdelay 2>/dev/null ||
      cc -O2 -w -pthread -I$mpp_ref/inc /tmp/h265e_probe_lowdelay.c \
        $mpp_lib/librockchip_mpp.so.0 -o /tmp/h265e_probe_lowdelay"

fail=0
run_cell() {
  if [ -n "${ONLY:-}" ] && [ "$1" != "$ONLY" ]; then
    return
  fi
  local name="$1" width="$2" height="$3" frames="$4" split_mode="$5" split_arg="$6"
  local frame="/tmp/rk-mpp-rs-h265-lowdelay-${width}x${height}-${frames}.nv12"
  local c_stream="/tmp/h265_lowdelay_c_$name.265"
  local rust_stream="/tmp/h265_lowdelay_rust_$name.265"
  local c_log="/tmp/h265_lowdelay_c_$name.log"
  local rust_log="/tmp/h265_lowdelay_rust_$name.log"
  local bps=$((width * height * 30 / 8))
  local c_ref=""
  local rust_ref=""
  local rust_tool="h265-idr"
  local rust_args="26 split:$split_mode:$split_arg splitout:lowdelay"
  local c_split_mode

  case "$split_mode" in
    byte) c_split_mode=1 ;;
    ctu) c_split_mode=2 ;;
    *)
      printf 'FAIL h265-lowdelay/%s (unknown split mode)\n' "$name"
      fail=1
      return
      ;;
  esac

  if [ "$frames" -gt 1 ]; then
    c_ref="REF_CFG=1"
    rust_ref="shortterm"
    rust_tool="h265-gop"
    rust_args="$frames 26 shortterm split:$split_mode:$split_arg splitout:lowdelay"
  fi

  ssh "$BOARD" "ffmpeg -v error -y -f lavfi -i testsrc2=size=${width}x${height}:rate=30 \
      -frames:v $frames -pix_fmt nv12 -f rawvideo -an $frame"

  if ! ssh "$BOARD" "sudo rm -f $c_stream $rust_stream $c_log $rust_log &&
      sudo env LD_LIBRARY_PATH=$mpp_lib $c_ref FRAMES=$frames GOP=60 timeout 120 \
        /tmp/h265e_probe_lowdelay $frame $c_stream $width $height 0 26 \
        $bps:1500000:2500000 split $c_split_mode $split_arg splitout 1 2>$c_log &&
      cd ~/rk-mpp-rs &&
      sudo env RK_MPP_TRACE_PACKETS=1 timeout 120 ./target/release/rk-mpp-rs \
        $rust_tool $frame $rust_stream $width $height $rust_args 2>$rust_log"; then
    printf 'FAIL h265-lowdelay/%s (encoder failed)\n' "$name"
    fail=1
    return
  fi

  scp -q "$BOARD":"$c_stream" "$BOARD":"$rust_stream" \
    "$BOARD":"$c_log" "$BOARD":"$rust_log" /tmp/
  if ! python3 scripts/compare_streams.py "/tmp/h265_lowdelay_c_$name.265" \
      "/tmp/h265_lowdelay_rust_$name.265"; then
    printf 'FAIL h265-lowdelay/%s (bitstreams differ)\n' "$name"
    fail=1
    return
  fi
  if ! python3 scripts/compare_packet_sizes.py "/tmp/h265_lowdelay_c_$name.log" \
      "/tmp/h265_lowdelay_rust_$name.log"; then
    printf 'FAIL h265-lowdelay/%s (packet boundaries differ)\n' "$name"
    fail=1
    return
  fi
  printf 'PASS h265-lowdelay/%s\n' "$name"
}

# CTU naming follows MPP_ENC_SPLIT_BY_CTU: mode 2, CTUs per slice in the argument.
run_cell ctu_8 320 240 1 ctu 8
run_cell byte_256 320 240 1 byte 256
# 640x480 is a 10x8 CTU grid; two CTUs per slice produces 40 packets, past the
# 32-entry FIFO threshold that also arms slc_done_en without low-delay output.
run_cell ctu_2_fifo 640 480 1 ctu 2
# Six distinct pictures cover the IDR packet plus the P-picture packet path.
run_cell shortterm_p6 320 240 6 ctu 8

exit "$fail"
