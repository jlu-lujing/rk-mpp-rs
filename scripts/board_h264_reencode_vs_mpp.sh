#!/usr/bin/env bash
BOARD="${BOARD:-t6}"
# Force a CBR overshoot on the opening IDR. The reference submits the same
# picture twice (QP 26, then 46), then commits only the second payload. The
# trace and status dump also prove the RC state after each attempt.
set -uo pipefail

FRAMES=6
SIZE="320 240"
BPS_TARGET=10000
BPS_MIN=8333
BPS_MAX=11666

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target --exclude Cargo.lock -cf - . |
  ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release 2>&1 | tail -1'

mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_lib="$mpp_ref/build/mpp"
tracer="${TRACER:-/tmp/mpp_trace.so}"
fixture=/tmp/rk-mpp-rs-reenc-320x240.nv12

if ! ssh "$BOARD" "test -f $mpp_lib/librockchip_mpp.so.1 || test -f $mpp_lib/librockchip_mpp.so.0"; then
  printf 'SKIP reencode (reference build missing at %s)\n' "$mpp_ref"
  exit 0
fi
if ! ssh "$BOARD" "test -f $tracer"; then
  printf 'SKIP reencode (ioctl tracer missing at %s)\n' "$tracer"
  exit 0
fi

scp -q tools/rc_probe.c "$BOARD":/tmp/
ssh "$BOARD" "cc -O2 -w -I$mpp_ref/inc /tmp/rc_probe.c $mpp_lib/librockchip_mpp.so.1 -o /tmp/rc_probe_reenc 2>/dev/null ||
        cc -O2 -w -I$mpp_ref/inc /tmp/rc_probe.c $mpp_lib/librockchip_mpp.so.0 -o /tmp/rc_probe_reenc" || true
ssh "$BOARD" "ffmpeg -v error -y -f lavfi -i testsrc2=size=320x240:rate=30 -frames:v $FRAMES \
    -pix_fmt nv12 -f rawvideo -an $fixture" >/dev/null 2>&1

fail=0
for entry in cbr:1 vbr:0 avbr:2; do
  mode="${entry%%:*}"
  probe_mode="${entry##*:}"
  c=/tmp/reenc_${mode}_c.264
  ours=/tmp/reenc_${mode}_ours.264
  trace=/tmp/reenc_${mode}.trace
  stats=/tmp/reenc_${mode}_ours_stats.txt
  ssh "$BOARD" "sudo rm -f $c $ours $trace $stats &&
          sudo env LD_LIBRARY_PATH=$mpp_lib LD_PRELOAD=$tracer /tmp/rc_probe_reenc \
            $fixture $c $SIZE $FRAMES $BPS_TARGET $BPS_MIN $BPS_MAX $probe_mode 30 0 \
            >/dev/null 2>$trace &&
          cd ~/rk-mpp-rs && sudo env RKMPP_STAT_DUMP=$stats \
            ./target/release/rk-mpp-rs h264-enc $fixture $ours \
            $SIZE 26 baseline $FRAMES 60 \
            $mode:$BPS_TARGET:$BPS_MIN:$BPS_MAX reenc:1 >/dev/null &&
          sudo chmod 644 $c $ours $trace $stats"
  if ! ssh "$BOARD" "test -s $c && test -s $ours"; then
    printf 'FAIL reencode-%s (one side produced no stream)\n' "$mode"
    fail=1
    continue
  fi
  scp -q "$BOARD":$c "$BOARD":$ours "$BOARD":$trace "$BOARD":$stats /tmp/ 2>/dev/null
  if ! python3 scripts/compare_streams.py "$c" "$ours"; then
    printf 'FAIL reencode-%s (bitstreams differ)\n' "$mode"
    fail=1
    continue
  fi
  if ! python3 scripts/stat_compare.py "$trace" "$stats"; then
    printf 'FAIL reencode-%s (STATUS windows differ)\n' "$mode"
    fail=1
    continue
  fi
  c_jobs=$(grep -c "read_complete\[1\]@16384" "$trace")
  our_jobs=$(grep -c "read_complete\[1\]@16384" "$stats")
  if [ "$c_jobs" -le "$FRAMES" ] || [ "$our_jobs" -ne "$c_jobs" ]; then
    printf 'FAIL reencode-%s (%d frames, C=%s ours=%s hardware jobs)\n' \
      "$mode" "$FRAMES" "$c_jobs" "$our_jobs"
    fail=1
    continue
  fi
  printf 'PASS reencode-%s (%s, %d frames, %s hardware jobs)\n' \
    "$mode" "$SIZE" "$FRAMES" "$c_jobs"
done
exit "$fail"
