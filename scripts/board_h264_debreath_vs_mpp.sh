#!/usr/bin/env bash
BOARD="${BOARD:?set BOARD to a Rockchip SSH alias}"
# Two-pass deflicker: every IDR after the first first encodes a non-reference P
# picture into the linear pass1 buffer, then uses that reconstruction as the
# source for the real IDR. The stream and every attempted-job STATUS are the
# observable result, including the suppressed normal re-encode path.
set -uo pipefail

FRAMES=9
GOP=4
STRENGTH=12
BPS_TARGET=100000
BPS_MIN=83333
BPS_MAX=116666

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target --exclude Cargo.lock --exclude .worktrees --exclude refs -cf - . |
  ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release 2>&1 | tail -1'

mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_lib="$mpp_ref/build/mpp"
tracer="${TRACER:-/tmp/mpp_trace.so}"
fixture=/tmp/rk-mpp-rs-debreath-320x240.nv12

scp -q tools/rc_probe.c "$BOARD":/tmp/rc_probe_debreath.c
ssh "$BOARD" "cc -O2 -w -I$mpp_ref/inc /tmp/rc_probe_debreath.c \
  $mpp_lib/librockchip_mpp.so.1 -o /tmp/rc_probe_debreath 2>/dev/null ||
  cc -O2 -w -I$mpp_ref/inc /tmp/rc_probe_debreath.c \
  $mpp_lib/librockchip_mpp.so.0 -o /tmp/rc_probe_debreath" || true
ssh "$BOARD" "ffmpeg -v error -y -f lavfi -i testsrc2=size=320x240:rate=30 \
  -frames:v $FRAMES -pix_fmt nv12 -f rawvideo -an $fixture" >/dev/null 2>&1

fail=0
for entry in cbr:1 vbr:0 avbr:2; do
  mode="${entry%%:*}"
  probe_mode="${entry##*:}"
  c=/tmp/debreath_${mode}_c.264
  ours=/tmp/debreath_${mode}_ours.264
  trace=/tmp/debreath_${mode}.trace
  stats=/tmp/debreath_${mode}_ours_stats.txt
  ssh "$BOARD" "sudo rm -f $c $ours $trace $stats &&
          sudo env LD_LIBRARY_PATH=$mpp_lib LD_PRELOAD=$tracer \
            RC_GOP=$GOP DEBREATH_EN=1 DEBREATH_STRENGTH=$STRENGTH \
            /tmp/rc_probe_debreath $fixture $c 320 240 $FRAMES \
              $BPS_TARGET $BPS_MIN $BPS_MAX $probe_mode 30 0 \
            >/dev/null 2>$trace &&
          cd ~/rk-mpp-rs && sudo env RKMPP_STAT_DUMP=$stats \
            ./target/release/rk-mpp-rs h264-enc $fixture $ours \
            320 240 26 baseline $FRAMES $GOP \
            $mode:$BPS_TARGET:$BPS_MIN:$BPS_MAX reenc:1 debreath:$STRENGTH \
            >/dev/null &&
          sudo chmod 644 $c $ours $trace $stats"
  if ! ssh "$BOARD" "test -s $c && test -s $ours"; then
    printf 'FAIL debreath-%s (one side produced no stream)\n' "$mode"
    fail=1
    continue
  fi
  scp -q "$BOARD":$c "$BOARD":$ours "$BOARD":$trace "$BOARD":$stats /tmp/ 2>/dev/null
  if ! python3 scripts/compare_streams.py "$c" "$ours"; then
    printf 'FAIL debreath-%s (bitstreams differ)\n' "$mode"
    fail=1
    continue
  fi
  if ! python3 scripts/stat_compare.py "$trace" "$stats"; then
    printf 'FAIL debreath-%s (STATUS windows differ)\n' "$mode"
    fail=1
    continue
  fi
  jobs=$(grep -c "read_complete\[1\]@16384" "$stats")
  if [ "$jobs" -ne 12 ]; then
    printf 'FAIL debreath-%s (expected 12 hardware jobs, got %s)\n' "$mode" "$jobs"
    fail=1
    continue
  fi
  printf 'PASS debreath-%s (320 240, %d frames, GOP %d, %d hardware jobs)\n' \
    "$mode" "$FRAMES" "$GOP" "$jobs"
done

# CABAC exercises the same pass1 linear source and the hardware CABAC stream.
# At strength 20 the reference converges in one fewer job than baseline.
c=/tmp/debreath_cabac_c.264
ours=/tmp/debreath_cabac_ours.264
trace=/tmp/debreath_cabac.trace
stats=/tmp/debreath_cabac_ours_stats.txt
ssh "$BOARD" "sudo rm -f $c $ours $trace $stats &&
        sudo env LD_LIBRARY_PATH=$mpp_lib LD_PRELOAD=$tracer \
          RC_GOP=$GOP DEBREATH_EN=1 DEBREATH_STRENGTH=20 \
          H264_PROFILE=77 H264_CABAC=1 \
          /tmp/rc_probe_debreath $fixture $c 320 240 $FRAMES \
            $BPS_TARGET $BPS_MIN $BPS_MAX 1 30 0 \
          >/dev/null 2>$trace &&
        cd ~/rk-mpp-rs && sudo env RKMPP_STAT_DUMP=$stats \
          ./target/release/rk-mpp-rs h264-enc $fixture $ours \
          320 240 26 main $FRAMES $GOP \
          cbr:$BPS_TARGET:$BPS_MIN:$BPS_MAX reenc:1 debreath:20 >/dev/null &&
        sudo chmod 644 $c $ours $trace $stats"
if ! ssh "$BOARD" "test -s $c && test -s $ours"; then
  printf 'FAIL debreath-cabac (one side produced no stream)\n'
  exit 1
fi
scp -q "$BOARD":$c "$BOARD":$ours "$BOARD":$trace "$BOARD":$stats /tmp/ 2>/dev/null
if ! python3 scripts/compare_streams.py "$c" "$ours"; then
  printf 'FAIL debreath-cabac (bitstreams differ)\n'
  exit 1
fi
if ! python3 scripts/stat_compare.py "$trace" "$stats"; then
  printf 'FAIL debreath-cabac (STATUS windows differ)\n'
  exit 1
fi
jobs=$(grep -c "read_complete\[1\]@16384" "$stats")
if [ "$jobs" -ne 11 ]; then
  printf 'FAIL debreath-cabac (expected 11 hardware jobs, got %s)\n' "$jobs"
  exit 1
fi
printf 'PASS debreath-cabac (320 240, %d frames, GOP %d, %d hardware jobs)\n' \
  "$FRAMES" "$GOP" "$jobs"

exit "$fail"
