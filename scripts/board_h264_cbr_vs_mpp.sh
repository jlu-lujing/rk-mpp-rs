#!/usr/bin/env bash
BOARD="${BOARD:?set BOARD to a Rockchip SSH alias}"
# CBR, VBR and AVBR: our encoder against the reference library, NAL by NAL.
#
# The reference is driven through `tools/rc_probe.c`. The first three cells keep
# the library's default quality band (8..48, qp_init 26). `cbr-mpi` reproduces
# `mpi_enc_test`'s additional settings exactly: qp_init=-1 and a 10..51 band for
# both general and intra QPs. The same settings reach Rust as `auto` plus
# `qprc:10:51`; both streams converge on the harness' first-picture QP.
#
# The version SEI is compared for framing only: its payload is the build identity
# of whichever encoder ran. Every other NAL, the rc-cfg SEI included, is compared
# byte for byte, so the whole QP trajectory per picture is under test.
#
# Usage: scripts/board_h264_cbr_vs_mpp.sh
set -uo pipefail

BPS_TARGET="${BPS_TARGET:-300000}"
BPS_MIN="${BPS_MIN:-250000}"
BPS_MAX="${BPS_MAX:-350000}"
FRAMES="${FRAMES:-6}"
SIZE="${SIZE:-320 240}"
HIER="1:2:3:4:1:1:1:1"
read -r width height <<< "$SIZE"

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target --exclude Cargo.lock --exclude .worktrees --exclude refs -cf - . |
  ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release 2>&1 | tail -1'

mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_lib="$mpp_ref/build/mpp"
fixture="${FIXTURE:-/tmp/rk-mpp-rs-move-${width}x${height}.nv12}"

if ! ssh "$BOARD" "test -f $mpp_lib/librockchip_mpp.so.1 || test -f $mpp_lib/librockchip_mpp.so.0"; then
  source "$(cd "$(dirname "$0")" && pwd)/board_dependency.sh" \
    "H.264 CBR" "reference build missing at $mpp_ref"
fi

# The probe is part of this repository; build it against the reference, not the
# installed library, so both sides run the same code the crate is compared with.
scp -q tools/rc_probe.c "$BOARD":/tmp/
ssh "$BOARD" "cc -O2 -w -I$mpp_ref/inc /tmp/rc_probe.c $mpp_lib/librockchip_mpp.so.1 -o /tmp/rc_probe 2>/dev/null ||
        cc -O2 -w -I$mpp_ref/inc /tmp/rc_probe.c $mpp_lib/librockchip_mpp.so.0 -o /tmp/rc_probe" || true

ssh "$BOARD" "ffmpeg -v error -y -f lavfi -i testsrc=size=${width}x${height}:rate=30 -frames:v $FRAMES \
    -pix_fmt nv12 -f rawvideo -an $fixture" >/dev/null 2>&1

fail=0
for entry in cbr:1:0 vbr:0:0 avbr:2:0 cbr-mpi:1:1; do
  mode="${entry%%:*}"
  rest="${entry#*:}"
  probe_mode="${rest%%:*}"
  mpi_quality="${rest##*:}"
  rust_mode="${mode%-mpi}"
  quality=26
  quality_extra=
  if [ "$mpi_quality" -eq 1 ]; then
    quality=auto
    quality_extra=qprc:10:51
  fi
  ssh "$BOARD" "sudo rm -f /tmp/rc_ours_$mode.264 /tmp/rc_c_$mode.264 &&
          cd ~/rk-mpp-rs && sudo ./target/release/rk-mpp-rs h264-enc $fixture /tmp/rc_ours_$mode.264 \
            $SIZE $quality baseline $FRAMES 60 $rust_mode:$BPS_TARGET:$BPS_MIN:$BPS_MAX $quality_extra >/dev/null &&
          sudo env LD_LIBRARY_PATH=$mpp_lib /tmp/rc_probe $fixture /tmp/rc_c_$mode.264 \
            $SIZE $FRAMES $BPS_TARGET $BPS_MIN $BPS_MAX $probe_mode 30 $mpi_quality >/dev/null 2>&1 &&
          sudo chmod 644 /tmp/rc_ours_$mode.264 /tmp/rc_c_$mode.264"
  if ! ssh "$BOARD" "test -s /tmp/rc_ours_$mode.264 && test -s /tmp/rc_c_$mode.264"; then
    printf 'FAIL %s (one side produced no stream)\n' "$mode"; fail=1; continue
  fi
  if scp -q "$BOARD":"/tmp/rc_c_$mode.264" /tmp/ &&
     scp -q "$BOARD":"/tmp/rc_ours_$mode.264" /tmp/ &&
     python3 scripts/compare_streams.py "/tmp/rc_c_$mode.264" "/tmp/rc_ours_$mode.264"; then
    printf 'PASS %s (%s, %d frames, %s:%s:%s)\n' "$mode" "$SIZE" "$FRAMES" \
      "$BPS_TARGET" "$BPS_MIN" "$BPS_MAX"
  else
    printf 'FAIL %s (bitstreams differ)\n' "$mode"; fail=1
  fi
done

# Hierarchical QP: the counters walk the reference's nested layer schedule.
# Run all three rate controllers so the layer offset is not accidentally tied to
# CBR's particular QP trajectory.
for entry in cbr:1 vbr:0 avbr:2; do
  mode="${entry%%:*}"
  probe_mode="${entry##*:}"
  ssh "$BOARD" "sudo rm -f /tmp/rc_ours_hier-$mode.264 /tmp/rc_c_hier-$mode.264 &&
          cd ~/rk-mpp-rs && sudo ./target/release/rk-mpp-rs h264-enc $fixture /tmp/rc_ours_hier-$mode.264 \
            $SIZE 26 baseline $FRAMES 60 $mode:$BPS_TARGET:$BPS_MIN:$BPS_MAX hierq:$HIER >/dev/null &&
          sudo env LD_LIBRARY_PATH=$mpp_lib /tmp/rc_probe $fixture /tmp/rc_c_hier-$mode.264 \
            $SIZE $FRAMES $BPS_TARGET $BPS_MIN $BPS_MAX $probe_mode 30 0 $HIER >/dev/null 2>&1 &&
          sudo chmod 644 /tmp/rc_ours_hier-$mode.264 /tmp/rc_c_hier-$mode.264"
  if ! ssh "$BOARD" "test -s /tmp/rc_ours_hier-$mode.264 && test -s /tmp/rc_c_hier-$mode.264"; then
    printf 'FAIL hier-%s (one side produced no stream)\n' "$mode"
    fail=1
  elif scp -q "$BOARD":/tmp/rc_c_hier-$mode.264 /tmp/ &&
       scp -q "$BOARD":/tmp/rc_ours_hier-$mode.264 /tmp/ &&
       python3 scripts/compare_streams.py /tmp/rc_c_hier-$mode.264 /tmp/rc_ours_hier-$mode.264; then
    printf 'PASS hier-%s (%s, %d frames, deltas %s)\n' "$mode" "$SIZE" "$FRAMES" "$HIER"
  else
    printf 'FAIL hier-%s (bitstreams differ)\n' "$mode"
    fail=1
  fi
done

exit "$fail"
