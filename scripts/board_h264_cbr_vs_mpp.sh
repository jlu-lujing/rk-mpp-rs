#!/usr/bin/env bash
BOARD="${BOARD:-t62}"
# CBR, VBR and AVBR: our encoder against the reference library, NAL by NAL.
#
# The reference is driven through `tools/rc_probe.c` rather than `mpi_enc_test`,
# because both sides then configure the same three things - CBR, the bit rate and
# GOP 60 at 30 fps - and leave the quality band at `h264e_api_v2.c`'s defaults.
# `mpi_enc_test` configures more of its own (its quality lands a first picture at
# QP 23 where the library's default is 26), so it is a harness comparison for a
# later day, not this one.
#
# The version SEI is compared for framing only: its payload is the build identity
# of whichever encoder ran. Every other NAL, the rc-cfg SEI included, is compared
# byte for byte, so the whole QP trajectory per picture is under test.
#
# Usage: scripts/board_h264_cbr_vs_mpp.sh
set -uo pipefail

BPS_TARGET=300000
BPS_MIN=250000
BPS_MAX=350000
FRAMES=6
SIZE="320 240"

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target --exclude Cargo.lock -cf - . |
  ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release 2>&1 | tail -1'

mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_lib="$mpp_ref/build/mpp"
fixture=/tmp/rk-mpp-rs-move-320x240.nv12

if ! ssh "$BOARD" "test -f $mpp_lib/librockchip_mpp.so.1 || test -f $mpp_lib/librockchip_mpp.so.0"; then
  printf 'SKIP CBR (reference build missing at %s)\n' "$mpp_ref"
  exit 0
fi

# The probe is part of this repository; build it against the reference, not the
# installed library, so both sides run the same code the crate is compared with.
ssh "$BOARD" "cc -O2 -w -I$mpp_ref/inc /tmp/rc_probe.c $mpp_lib/librockchip_mpp.so.1 -o /tmp/rc_probe 2>/dev/null ||
        cc -O2 -w -I$mpp_ref/inc /tmp/rc_probe.c $mpp_lib/librockchip_mpp.so.0 -o /tmp/rc_probe" || true
scp -q tools/rc_probe.c "$BOARD":/tmp/ 2>/dev/null

ssh "$BOARD" "ffmpeg -v error -y -f lavfi -i testsrc=size=320x240:rate=30 -frames:v $FRAMES \
    -pix_fmt nv12 -f rawvideo -an $fixture" >/dev/null 2>&1

ssh "$BOARD" "cc -O2 -w -I$mpp_ref/inc /tmp/rc_probe.c $mpp_lib/librockchip_mpp.so.1 -o /tmp/rc_probe" || true

fail=0
for entry in cbr:1 vbr:0 avbr:2; do
  mode="${entry%%:*}"; probe_mode="${entry##*:}"
  ssh "$BOARD" "sudo rm -f /tmp/rc_ours_$mode.264 /tmp/rc_c_$mode.264 &&
          cd ~/rk-mpp-rs && sudo ./target/release/rk-mpp-rs h264-enc $fixture /tmp/rc_ours_$mode.264 \
            $SIZE 26 baseline $FRAMES 60 $mode:$BPS_TARGET:$BPS_MIN:$BPS_MAX >/dev/null &&
          sudo env LD_LIBRARY_PATH=$mpp_lib /tmp/rc_probe $fixture /tmp/rc_c_$mode.264 \
            $SIZE $FRAMES $BPS_TARGET $BPS_MIN $BPS_MAX $probe_mode >/dev/null 2>&1 &&
          sudo chmod 644 /tmp/rc_ours_$mode.264 /tmp/rc_c_$mode.264"
  if ! ssh "$BOARD" "test -s /tmp/rc_ours_$mode.264 && test -s /tmp/rc_c_$mode.264"; then
    printf 'FAIL %s (one side produced no stream)\n' "$mode"; fail=1; continue
  fi
  scp -q "$BOARD":"/tmp/rc_ours_$mode.264" "$BOARD":"/tmp/rc_c_$mode.264" /tmp/ 2>/dev/null
  if python3 scripts/compare_streams.py "/tmp/rc_c_$mode.264" "/tmp/rc_ours_$mode.264"; then
    printf 'PASS %s (%s, %d frames, %s:%s:%s)\n' "$mode" "$SIZE" "$FRAMES" \
      "$BPS_TARGET" "$BPS_MIN" "$BPS_MAX"
  else
    printf 'FAIL %s (bitstreams differ)\n' "$mode"; fail=1
  fi
done
exit "$fail"
