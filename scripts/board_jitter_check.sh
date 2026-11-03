#!/usr/bin/env bash
# Check that representative C MPP byte-exact oracles are deterministic.
#
# Two independent runs of the same source-built C binary must produce the same
# artifact bytes and SHA-256. This protects the differential gates from using
# a nondeterministic reference, independent of our Rust implementation.
set -euo pipefail

BOARD="${BOARD:-t6}"
DEC_BIN=/tmp/mpp-develop/build/test/mpi_dec_test
DEC_LIB=/tmp/mpp-develop/build/mpp
ENC_BIN=/tmp/mpp-old/build/test/mpi_enc_test
ENC_LIB=/tmp/mpp-old/build/mpp
SRC=/tmp/rk-mpp-jitter-320x240.nv12
fail=0

remote() {
  ssh "$BOARD" "$1"
}

if ! remote "test -x $DEC_BIN && test -f $DEC_LIB/librockchip_mpp.so.0 &&
    test -x $ENC_BIN && test -f $ENC_LIB/librockchip_mpp.so.0"; then
  echo "reference binaries missing; run scripts/setup_board_refs.sh first" >&2
  exit 1
fi
remote 'cd ~/rk-mpp-rs && ./target/release/rk-mpp-rs probe-frame \
  /tmp/rk-mpp-jitter-320x240.nv12 320 240 >/dev/null'

compare_pair() {
  local label="$1" a="$2" b="$3" ha hb
  remote "sudo cmp -s $a $b" || {
    printf 'FAIL %-20s byte mismatch\n' "$label"
    fail=1
    return
  }
  ha="$(remote "sudo sha256sum $a | cut -d' ' -f1")"
  hb="$(remote "sudo sha256sum $b | cut -d' ' -f1")"
  if [[ "$ha" != "$hb" ]]; then
    printf 'FAIL %-20s sha mismatch\n' "$label"
    fail=1
    return
  fi
  printf 'PASS %-20s %s\n' "$label" "$ha"
}

remote "sudo rm -f /tmp/rk-mpp-jitter-h264-a.yuv /tmp/rk-mpp-jitter-h264-b.yuv"
for suffix in a b; do
  remote "sudo env LD_LIBRARY_PATH=$DEC_LIB timeout 120 $DEC_BIN \
    -i /tmp/test720.264 -o /tmp/rk-mpp-jitter-h264-$suffix.yuv \
    -w 1280 -h 720 -t 7 -n 0 -v q >/dev/null 2>&1"
done
compare_pair h264-decode \
  /tmp/rk-mpp-jitter-h264-a.yuv /tmp/rk-mpp-jitter-h264-b.yuv

remote "sudo rm -f /tmp/rk-mpp-jitter-hevc-a.yuv /tmp/rk-mpp-jitter-hevc-b.yuv"
for suffix in a b; do
  remote "sudo env LD_LIBRARY_PATH=$DEC_LIB timeout 120 $DEC_BIN \
    -i /tmp/t720.hevc -o /tmp/rk-mpp-jitter-hevc-$suffix.yuv \
    -w 1280 -h 720 -t 16777220 -n 0 -v q >/dev/null 2>&1"
done
compare_pair hevc-decode \
  /tmp/rk-mpp-jitter-hevc-a.yuv /tmp/rk-mpp-jitter-hevc-b.yuv

remote "sudo rm -f /tmp/rk-mpp-jitter-h264-a.264 /tmp/rk-mpp-jitter-h264-b.264"
for suffix in a b; do
  remote "sudo env LD_LIBRARY_PATH=$ENC_LIB timeout 120 $ENC_BIN \
    -i $SRC -o /tmp/rk-mpp-jitter-h264-$suffix.264 \
    -w 320 -h 240 -t 7 -f 0 -n 1 -rc 2 \
    -qc 26:26:26:26:26 -v q >/dev/null 2>&1"
done
compare_pair h264-encode \
  /tmp/rk-mpp-jitter-h264-a.264 /tmp/rk-mpp-jitter-h264-b.264

exit "$fail"
