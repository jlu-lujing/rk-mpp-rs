#!/usr/bin/env bash
BOARD="${BOARD:?set BOARD to a Rockchip SSH alias}"
# Compare every usable IEP2 client-28 deinterlace mode against a minimal C probe
# built on the reference library.
#
# Both sides write the raw contents of dst[0] and dst[1]. Single-output modes
# therefore also prove which slot they write and that the unused slot is zero.
#
# Usage: BOARD=my-rk3588-host scripts/board_iep_modes_vs_probe.sh
set -uo pipefail

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target --exclude Cargo.lock --exclude .worktrees --exclude refs --exclude '._*' -cf - . |
  ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release 2>&1 | tail -1'

remote_home=$(ssh "$BOARD" 'printf %s "$HOME"')
mpp_ref="${MPP_REF:-$remote_home/rk-mpp-rs/refs/mpp}"
mpp_lib="${MPP_LIB:-$mpp_ref/build/mpp}"
if ! ssh "$BOARD" "test -f $mpp_lib/librockchip_mpp.so.1 || test -f $mpp_lib/librockchip_mpp.so.0"; then
  source "$(cd "$(dirname "$0")" && pwd)/board_dependency.sh" \
    "IEP modes" "reference build missing at $mpp_lib"
fi

scp -q tools/iep2_probe.c "$BOARD":/tmp/iep2_probe.c
ssh "$BOARD" "cc -O2 -w -I$mpp_ref/inc -I$mpp_ref/mpp/vproc/inc -I$mpp_ref/osal/inc \
        /tmp/iep2_probe.c $mpp_lib/librockchip_mpp.so.1 -o /tmp/iep2_probe 2>/dev/null ||
      cc -O2 -w -I$mpp_ref/inc -I$mpp_ref/mpp/vproc/inc -I$mpp_ref/osal/inc \
        /tmp/iep2_probe.c $mpp_lib/librockchip_mpp.so.0 -o /tmp/iep2_probe"

fail=0
run_cell() {
  local name="$1" width="$2" height="$3" order="$4" mode="$5" mode_num="$6"
  local input="/tmp/rk-mpp-rs-iep-modes-${width}x${height}.yuv"
  local c_out="/tmp/iep_modes_c_$name.yuv"
  local rust_out="/tmp/iep_modes_rust_$name.yuv"
  local order_num

  [ "$order" = "BFF" ] && order_num=1 || order_num=0
  ssh "$BOARD" "python3 ~/rk-mpp-rs/scripts/gen_iep_fixture.py \
      $input $width $height 3"
  if ! ssh "$BOARD" "sudo rm -f $c_out $rust_out &&
      sudo env LD_LIBRARY_PATH=$mpp_lib timeout 120 \
        /tmp/iep2_probe $input $c_out $width $height $mode_num $order_num >/dev/null 2>&1 &&
      sudo chmod 644 $c_out &&
      cd ~/rk-mpp-rs &&
      sudo timeout 120 ./target/release/rk-mpp-rs iep-dei \
        $input $rust_out $width $height $order $mode >/dev/null &&
      sudo chmod 644 $rust_out"; then
    printf 'FAIL iep-mode/%s (job failed)\n' "$name"
    fail=1
    return
  fi

  ssh "$BOARD" "sudo cmp -s $c_out $rust_out"
  if [ $? -ne 0 ]; then
    printf 'FAIL iep-mode/%s (raw dst slots differ)\n' "$name"
    fail=1
    return
  fi
  printf 'PASS iep-mode/%s\n' "$name"
}

run_cell I5O2_TFF 720 480 TFF I5O2 1
run_cell I5O2_BFF 720 480 BFF I5O2 1
run_cell I5O1T 720 480 TFF I5O1T 2
run_cell I5O1B 720 480 TFF I5O1B 3
run_cell I2O2 1280 720 TFF I2O2 4
run_cell I1O1T 640 360 TFF I1O1T 5
run_cell I1O1B 640 360 BFF I1O1B 6
run_cell DISABLE 720 480 TFF DISABLE 0
run_cell BYPASS 720 480 TFF BYPASS 8

exit "$fail"
