#!/usr/bin/env bash
BOARD="${BOARD:-t6}"
# SMART_P: one VI picture at `vi_len=31`, then P pictures. The 33-frame run
# crosses the VI and its first ordinary successor, so the retained IDR reference,
# reordered slice header, vi_bit window and QP delta are all compared.
set -uo pipefail

FRAMES=33
VI_LEN=31
SIZE="320 240"
BPS_TARGET=300000
BPS_MIN=250000
BPS_MAX=350000

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target --exclude Cargo.lock -cf - . |
  ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release 2>&1 | tail -1'

mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_lib="$mpp_ref/build/mpp"
fixture=/tmp/rk-mpp-rs-smartp-320x240.nv12

if ! ssh "$BOARD" "test -f $mpp_lib/librockchip_mpp.so.1 || test -f $mpp_lib/librockchip_mpp.so.0"; then
  printf 'SKIP SMART_P (reference build missing at %s)\n' "$mpp_ref"
  exit 0
fi

scp -q tools/rc_probe.c "$BOARD":/tmp/
ssh "$BOARD" "cc -O2 -w -I$mpp_ref/inc /tmp/rc_probe.c $mpp_lib/librockchip_mpp.so.1 -o /tmp/rc_probe_smart 2>/dev/null ||
        cc -O2 -w -I$mpp_ref/inc /tmp/rc_probe.c $mpp_lib/librockchip_mpp.so.0 -o /tmp/rc_probe_smart" || true

ssh "$BOARD" "ffmpeg -v error -y -f lavfi -i testsrc=size=320x240:rate=30 -frames:v $FRAMES \
    -pix_fmt nv12 -f rawvideo -an $fixture" >/dev/null 2>&1

fail=0
for entry in cbr:1 vbr:0 avbr:2; do
  mode="${entry%%:*}"
  probe_mode="${entry##*:}"
  ours=/tmp/smartp_${mode}_ours.264
  c=/tmp/smartp_${mode}_c.264
  ssh "$BOARD" "sudo rm -f $ours $c &&
          cd ~/rk-mpp-rs && sudo ./target/release/rk-mpp-rs h264-enc $fixture $ours \
            $SIZE 26 baseline $FRAMES 60 $mode:$BPS_TARGET:$BPS_MIN:$BPS_MAX smartp:$VI_LEN >/dev/null &&
          sudo env LD_LIBRARY_PATH=$mpp_lib SMART_VI_LEN=$VI_LEN /tmp/rc_probe_smart \
            $fixture $c $SIZE $FRAMES $BPS_TARGET $BPS_MIN $BPS_MAX $probe_mode 30 0 >/dev/null 2>&1 &&
          sudo chmod 644 $ours $c"
  if ! ssh "$BOARD" "test -s $ours && test -s $c"; then
    printf 'FAIL SMART_P-%s (one side produced no stream)\n' "$mode"
    fail=1
  elif scp -q "$BOARD":$ours "$BOARD":$c /tmp/ 2>/dev/null &&
       python3 scripts/compare_streams.py "$c" "$ours"; then
    printf 'PASS SMART_P-%s (%s, %d frames, vi_len %d)\n' \
      "$mode" "$SIZE" "$FRAMES" "$VI_LEN"
  else
    printf 'FAIL SMART_P-%s (bitstreams differ)\n' "$mode"
    fail=1
  fi
done
exit "$fail"
