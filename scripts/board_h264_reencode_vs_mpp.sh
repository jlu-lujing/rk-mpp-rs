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

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target --exclude Cargo.lock --exclude .worktrees --exclude refs -cf - . |
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

# Super-frame re-encode at a high target: the thresholds, not rate drift,
# force each oversized picture through the same-frame retry path.
c=/tmp/super_reenc_c.264
ours=/tmp/super_reenc_ours.264
trace=/tmp/super_reenc.trace
stats=/tmp/super_reenc_ours_stats.txt
ssh "$BOARD" "sudo rm -f $c $ours $trace $stats &&
        sudo env LD_LIBRARY_PATH=$mpp_lib LD_PRELOAD=$tracer \
          MAX_REENC=1 SUPER_MODE=2 SUPER_I_THD=10000 SUPER_P_THD=1000 RC_PRIORITY=0 \
          /tmp/rc_probe_reenc $fixture $c $SIZE $FRAMES 300000 250000 350000 1 30 0 \
          >/dev/null 2>$trace &&
        cd ~/rk-mpp-rs && sudo env RKMPP_STAT_DUMP=$stats \
          ./target/release/rk-mpp-rs h264-enc $fixture $ours \
          $SIZE 26 baseline $FRAMES 60 \
          cbr:300000:250000:350000 reenc:1 super:2:10000:1000:0 >/dev/null &&
        sudo chmod 644 $c $ours $trace $stats"
if ! ssh "$BOARD" "test -s $c && test -s $ours"; then
  printf 'FAIL super-reenc (one side produced no stream)\n'
  exit 1
fi
scp -q "$BOARD":$c "$BOARD":$ours "$BOARD":$trace "$BOARD":$stats /tmp/ 2>/dev/null
if ! python3 scripts/compare_streams.py "$c" "$ours"; then
  printf 'FAIL super-reenc (bitstreams differ)\n'
  exit 1
fi
if ! python3 scripts/stat_compare.py "$trace" "$stats"; then
  printf 'FAIL super-reenc (STATUS windows differ)\n'
  exit 1
fi
super_jobs=$(grep -c "read_complete\[1\]@16384" "$stats")
if [ "$super_jobs" -ne 12 ]; then
  printf 'FAIL super-reenc (expected 12 hardware jobs, got %s)\n' "$super_jobs"
  exit 1
fi
printf 'PASS super-reenc (%s, %d frames, 12 hardware jobs)\n' "$SIZE" "$FRAMES"

# Super-frame NORMAL drop: every P picture is measured, then rolled back and
# emitted as a zero-length packet. The IDR is re-encoded but never dropped.
for entry in cbr:1 vbr:0 avbr:2; do
  mode="${entry%%:*}"
  probe_mode="${entry##*:}"
  c=/tmp/sdrop_${mode}_c.264
  ours=/tmp/sdrop_${mode}_ours.264
  trace=/tmp/sdrop_${mode}.trace
  stats=/tmp/sdrop_${mode}_ours_stats.txt
  ssh "$BOARD" "sudo rm -f $c $ours $trace $stats &&
          sudo env LD_LIBRARY_PATH=$mpp_lib LD_PRELOAD=$tracer \
            MAX_REENC=1 SUPER_MODE=1 SUPER_I_THD=10000 SUPER_P_THD=1000 RC_PRIORITY=0 \
            /tmp/rc_probe_reenc $fixture $c $SIZE $FRAMES 300000 250000 350000 $probe_mode 30 0 \
            >/dev/null 2>$trace &&
          cd ~/rk-mpp-rs && sudo env RKMPP_STAT_DUMP=$stats \
            ./target/release/rk-mpp-rs h264-enc $fixture $ours \
            $SIZE 26 baseline $FRAMES 60 \
            $mode:300000:250000:350000 reenc:1 super:1:10000:1000:0 >/dev/null &&
          sudo chmod 644 $c $ours $trace $stats"
  if ! ssh "$BOARD" "test -s $c && test -s $ours"; then
    printf 'FAIL super-drop-%s (one side produced no stream)\n' "$mode"
    fail=1
    continue
  fi
  scp -q "$BOARD":$c "$BOARD":$ours "$BOARD":$trace "$BOARD":$stats /tmp/ 2>/dev/null
  if ! python3 scripts/compare_streams.py "$c" "$ours"; then
    printf 'FAIL super-drop-%s (bitstreams differ)\n' "$mode"
    fail=1
    continue
  fi
  if ! python3 scripts/stat_compare.py "$trace" "$stats"; then
    printf 'FAIL super-drop-%s (STATUS windows differ)\n' "$mode"
    fail=1
    continue
  fi
  drop_jobs=$(grep -c "read_complete\[1\]@16384" "$stats")
  if [ "$drop_jobs" -ne 7 ]; then
    printf 'FAIL super-drop-%s (expected 7 hardware jobs, got %s)\n' \
      "$mode" "$drop_jobs"
    fail=1
    continue
  fi
  printf 'PASS super-drop-%s (%s, %d frames, final stream = IDR only)\n' \
    "$mode" "$SIZE" "$FRAMES"
done

# PSKIP replaces an oversized non-reference P picture with the software
# all-mb-skip slice. The 9-byte NALs, alternating accepted P pictures, final
# stream and every attempted hardware STATUS must all match.
for entry in cbr:1:7 vbr:0:8 avbr:2:9; do
  mode="${entry%%:*}"
  rest="${entry#*:}"
  probe_mode="${rest%%:*}"
  expected_jobs="${rest##*:}"
  c=/tmp/pskip_${mode}_c.264
  ours=/tmp/pskip_${mode}_ours.264
  trace=/tmp/pskip_${mode}.trace
  stats=/tmp/pskip_${mode}_ours_stats.txt
  ssh "$BOARD" "sudo rm -f $c $ours $trace $stats &&
          sudo env LD_LIBRARY_PATH=$mpp_lib LD_PRELOAD=$tracer \
            MAX_REENC=1 DROP_MODE=2 DROP_THD=0 DROP_GAP=1 \
            /tmp/rc_probe_reenc $fixture $c $SIZE $FRAMES 10000 8333 11666 $probe_mode 30 0 \
            >/dev/null 2>$trace &&
          cd ~/rk-mpp-rs && sudo env RKMPP_STAT_DUMP=$stats \
            ./target/release/rk-mpp-rs h264-enc $fixture $ours \
            $SIZE 26 baseline $FRAMES 60 \
            $mode:10000:8333:11666 reenc:1 drop:2:0:1 >/dev/null &&
          sudo chmod 644 $c $ours $trace $stats"
  if ! ssh "$BOARD" "test -s $c && test -s $ours"; then
    printf 'FAIL pskip-%s (one side produced no stream)\n' "$mode"
    fail=1
    continue
  fi
  scp -q "$BOARD":$c "$BOARD":$ours "$BOARD":$trace "$BOARD":$stats /tmp/ 2>/dev/null
  if ! python3 scripts/compare_streams.py "$c" "$ours"; then
    printf 'FAIL pskip-%s (bitstreams differ)\n' "$mode"
    fail=1
    continue
  fi
  if ! python3 scripts/stat_compare.py "$trace" "$stats"; then
    printf 'FAIL pskip-%s (STATUS windows differ)\n' "$mode"
    fail=1
    continue
  fi
  pskip_jobs=$(grep -c "read_complete\[1\]@16384" "$stats")
  if [ "$pskip_jobs" -ne "$expected_jobs" ]; then
    printf 'FAIL pskip-%s (expected %s hardware jobs, got %s)\n' \
      "$mode" "$expected_jobs" "$pskip_jobs"
    fail=1
    continue
  fi
  printf 'PASS pskip-%s (%s, %d frames, %s hardware jobs)\n' \
    "$mode" "$SIZE" "$FRAMES" "$pskip_jobs"
done

# CABAC uses one skip flag per macroblock instead of the CAVLC skip run. The
# C reference emits the same 11-byte NAL layout; the arithmetic bytes change
# per P picture position, so this is a byte-level check of the software CABAC
# writer as well as the RC decision path.
for entry in cbr:1:7 vbr:0:8 avbr:2:9; do
  mode="${entry%%:*}"
  rest="${entry#*:}"
  probe_mode="${rest%%:*}"
  expected_jobs="${rest##*:}"
  c=/tmp/pskip_cabac_${mode}_c.264
  ours=/tmp/pskip_cabac_${mode}_ours.264
  trace=/tmp/pskip_cabac_${mode}.trace
  stats=/tmp/pskip_cabac_${mode}_ours_stats.txt
  ssh "$BOARD" "sudo rm -f $c $ours $trace $stats &&
          sudo env LD_LIBRARY_PATH=$mpp_lib LD_PRELOAD=$tracer \
            H264_PROFILE=77 H264_CABAC=1 MAX_REENC=1 DROP_MODE=2 DROP_THD=0 DROP_GAP=1 \
            /tmp/rc_probe_reenc $fixture $c $SIZE $FRAMES 10000 8333 11666 $probe_mode 30 0 \
            >/dev/null 2>$trace &&
          cd ~/rk-mpp-rs && sudo env RKMPP_STAT_DUMP=$stats \
            ./target/release/rk-mpp-rs h264-enc $fixture $ours \
            $SIZE 26 main $FRAMES 60 \
            $mode:10000:8333:11666 reenc:1 drop:2:0:1 >/dev/null &&
          sudo chmod 644 $c $ours $trace $stats"
  if ! ssh "$BOARD" "test -s $c && test -s $ours"; then
    printf 'FAIL pskip-cabac-%s (one side produced no stream)\n' "$mode"
    fail=1
    continue
  fi
  scp -q "$BOARD":$c "$BOARD":$ours "$BOARD":$trace "$BOARD":$stats /tmp/ 2>/dev/null
  if ! python3 scripts/compare_streams.py "$c" "$ours"; then
    printf 'FAIL pskip-cabac-%s (bitstreams differ)\n' "$mode"
    fail=1
    continue
  fi
  if ! python3 scripts/stat_compare.py "$trace" "$stats"; then
    printf 'FAIL pskip-cabac-%s (STATUS windows differ)\n' "$mode"
    fail=1
    continue
  fi
  pskip_jobs=$(grep -c "read_complete\[1\]@16384" "$stats")
  if [ "$pskip_jobs" -ne "$expected_jobs" ]; then
    printf 'FAIL pskip-cabac-%s (expected %s hardware jobs, got %s)\n' \
      "$mode" "$expected_jobs" "$pskip_jobs"
    fail=1
    continue
  fi
  printf 'PASS pskip-cabac-%s (%s, %d frames, %s hardware jobs)\n' \
    "$mode" "$SIZE" "$FRAMES" "$pskip_jobs"
done

exit "$fail"
