#!/usr/bin/env bash
BOARD="${BOARD:?set BOARD to a Rockchip SSH alias}"
# H.265 fixed-QP GDR / intra refresh against the C reference.
#
# h265e_probe now supplies both the refresh RC keys and an explicit short-term
# MPP_ENC_SET_REF_CFG. That makes C's ref scheduler set is_i_refresh and
# is_i_recovery at IGOP boundaries, exercising the real HAL refresh path.
#
# Usage: scripts/board_h265_gdr_vs_mpp.sh
set -uo pipefail

if [ "${SKIP_BUILD:-0}" != 1 ]; then
  COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target --exclude Cargo.lock --exclude .worktrees --exclude refs --exclude '._*' -cf - . |
    ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
  ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release 2>&1 | tail -1'
fi

mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_lib="$mpp_ref/build/mpp"
if ! ssh "$BOARD" "test -f $mpp_lib/librockchip_mpp.so.1"; then
  source "$(cd "$(dirname "$0")" && pwd)/board_dependency.sh" \
    "H.265 GDR" "reference build missing at $mpp_ref"
fi

if ssh "$BOARD" "grep -q 'bit_target_fix' $mpp_ref/inc/mpp_rc_defs.h"; then
  rc_has_bit_target_fix=1
else
  rc_has_bit_target_fix=0
fi
scp -q tools/h265e_probe.c tools/custom_rc_probe.h "$BOARD":/tmp/
scp -q scripts/compare_streams.py "$BOARD":/tmp/
ssh "$BOARD" "cp /tmp/h265e_probe.c /tmp/h265e_probe_gdr.c &&
    cc -O2 -w -pthread -DCUSTOM_RC_HAS_BIT_TARGET_FIX=$rc_has_bit_target_fix -I/tmp -I$mpp_ref/inc /tmp/h265e_probe_gdr.c \
    $mpp_lib/librockchip_mpp.so.1 -o /tmp/h265e_probe_gdr"

frames="${FRAMES:-8}"
gop="${GOP:-4}"
fail=0
run_cell() {
  local size="$1" mode="$2" num="$3"
  local width="${size%%x*}" height="${size##*x}"
  local bps=$((width * height * 30 / 8))
  local cli_mode=$([ "$mode" = 0 ] && printf row || printf col)
  local frame="/tmp/rk-mpp-rs-h265-gdr-$size.nv12"
  local c_out="/tmp/h265_gdr_c_$size-$cli_mode$num.265"
  local rust_out="/tmp/h265_gdr_rust_$size-$cli_mode$num.265"

  ssh "$BOARD" "ffmpeg -v error -y -f lavfi -i testsrc=size=${width}x${height}:rate=30 \
      -frames:v $frames -pix_fmt nv12 -f rawvideo -an $frame"
  if ! ssh "$BOARD" "sudo rm -f $c_out $rust_out &&
      sudo env LD_LIBRARY_PATH=$mpp_lib REF_CFG=1 REFRESH_EN=1 \
        REFRESH_MODE=$mode REFRESH_NUM=$num GOP=$gop FRAMES=$frames \
        timeout 120 /tmp/h265e_probe_gdr $frame $c_out $width $height 0 26 \
        $bps:1500000:2500000 >/dev/null 2>&1 &&
      cd ~/rk-mpp-rs &&
      sudo env H265_GOP=$gop timeout 120 ./target/release/rk-mpp-rs h265-gop \
        $frame $rust_out $width $height $frames 26 $cli_mode:$num >/dev/null"; then
    printf 'FAIL h265-gdr/%s-%s%s (encoder failed)\n' "$size" "$cli_mode" "$num"
    fail=1
    return
  fi
  if ! ssh "$BOARD" "python3 /tmp/compare_streams.py $c_out $rust_out" \
      >/dev/null 2>&1 < /dev/null; then
    printf 'FAIL h265-gdr/%s-%s%s (streams differ)\n' "$size" "$cli_mode" "$num"
    ssh "$BOARD" "sudo stat -c '%n %s' $c_out $rust_out" 2>/dev/null
    fail=1
    return
  fi
  if ! ssh "$BOARD" "sudo python3 ~/rk-mpp-rs/scripts/check_h265_gdr.py $rust_out"; then
    printf 'FAIL h265-gdr/%s-%s%s (GDR marker audit)\n' "$size" "$cli_mode" "$num"
    fail=1
    return
  fi
  printf 'PASS h265-gdr/%s-%s%s\n' "$size" "$cli_mode" "$num"
}

run_cell 320x240 0 1
run_cell 320x240 1 1
run_cell 640x480 0 2
run_cell 640x480 1 2
if [ "${RUN_4K:-0}" = 1 ]; then
  run_cell 3840x2160 0 1
  run_cell 3840x2160 1 1
fi

exit "$fail"
