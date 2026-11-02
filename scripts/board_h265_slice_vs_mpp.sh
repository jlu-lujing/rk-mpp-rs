#!/usr/bin/env bash
BOARD="${BOARD:-t6}"
# H.265 ordinary slice split against the 2024-04 C oracle.
#
# This is the split_out=0 companion to board_h265_lowdelay_vs_mpp.sh: the
# hardware writes every slice contiguously into one access unit, slen_fifo stays
# off, and the caller polls normal completion instead of the slice FIFO.
#
# Usage: BOARD=t6 scripts/board_h265_slice_vs_mpp.sh
set -uo pipefail

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target --exclude Cargo.lock --exclude '._*' -cf - . |
  ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release 2>&1 | tail -1'

mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_lib="$mpp_ref/build/mpp"
if ! ssh "$BOARD" "test -f $mpp_lib/librockchip_mpp.so.1 || test -f $mpp_lib/librockchip_mpp.so.0"; then
  printf 'SKIP H.265 slice (reference build missing at %s)\n' "$mpp_ref"
  exit 0
fi

scp -q tools/h265e_probe.c "$BOARD":/tmp/h265e_probe_slice.c
ssh "$BOARD" "cc -O2 -w -pthread -I$mpp_ref/inc /tmp/h265e_probe_slice.c \
        $mpp_lib/librockchip_mpp.so.1 -o /tmp/h265e_probe_slice 2>/dev/null ||
      cc -O2 -w -pthread -I$mpp_ref/inc /tmp/h265e_probe_slice.c \
        $mpp_lib/librockchip_mpp.so.0 -o /tmp/h265e_probe_slice"

fail=0
run_cell() {
  local name="$1" width="$2" height="$3" split_mode="$4" split_arg="$5"
  local frame="/tmp/rk-mpp-rs-h265-slice-${width}x${height}.nv12"
  local c_stream="/tmp/h265_slice_c_$name.265"
  local rust_stream="/tmp/h265_slice_rust_$name.265"
  local bps=$((width * height * 30 / 8))
  local c_split_mode

  case "$split_mode" in
    byte) c_split_mode=1 ;;
    ctu) c_split_mode=2 ;;
    *)
      printf 'FAIL h265-slice/%s (unknown split mode)\n' "$name"
      fail=1
      return
      ;;
  esac

  ssh "$BOARD" "ffmpeg -v error -y -f lavfi -i testsrc2=size=${width}x${height}:rate=30 \
      -frames:v 1 -pix_fmt nv12 -f rawvideo -an $frame"
  if ! ssh "$BOARD" "sudo rm -f $c_stream $rust_stream &&
      sudo env LD_LIBRARY_PATH=$mpp_lib FRAMES=1 GOP=60 timeout 120 \
        /tmp/h265e_probe_slice $frame $c_stream $width $height 0 26 \
        $bps:1500000:2500000 split $c_split_mode $split_arg >/dev/null 2>&1 &&
      cd ~/rk-mpp-rs &&
      sudo timeout 120 ./target/release/rk-mpp-rs h265-idr \
        $frame $rust_stream $width $height 26 split:$split_mode:$split_arg >/dev/null"; then
    printf 'FAIL h265-slice/%s (encoder failed)\n' "$name"
    fail=1
    return
  fi

  scp -q "$BOARD":"$c_stream" "$BOARD":"$rust_stream" /tmp/
  if ! python3 scripts/compare_streams.py "/tmp/h265_slice_c_$name.265" \
      "/tmp/h265_slice_rust_$name.265"; then
    printf 'FAIL h265-slice/%s (bitstreams differ)\n' "$name"
    fail=1
    return
  fi
  printf 'PASS h265-slice/%s\n' "$name"
}

run_cell ctu_8 320 240 ctu 8
run_cell byte_256 320 240 byte 256
# Forty slices with split_out=0: no slice FIFO/interrupt, one contiguous AU.
run_cell ctu_2_40slices 640 480 ctu 2

exit "$fail"
