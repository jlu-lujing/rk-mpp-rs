#!/usr/bin/env bash
BOARD="${BOARD:-t6}"
# IEP2 client-28 input/output format matrix against the C probe.
#
# The same format and swap are configured as both source and destination. Raw
# dst0/dst1 are compared, including the unused zero slot for single-output modes.
#
# Usage: BOARD=my-rk3588-host scripts/board_iep_formats_vs_probe.sh
set -uo pipefail

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target --exclude Cargo.lock --exclude .worktrees --exclude refs --exclude '._*' -cf - . |
  ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release 2>&1 | tail -1'

remote_home=$(ssh "$BOARD" 'printf %s "$HOME"')
mpp_ref="${MPP_REF:-$remote_home/rk-mpp-rs/refs/mpp}"
mpp_lib="${MPP_LIB:-$mpp_ref/build/mpp}"
if ! ssh "$BOARD" "test -f $mpp_lib/librockchip_mpp.so.1 || test -f $mpp_lib/librockchip_mpp.so.0"; then
  source "$(cd "$(dirname "$0")" && pwd)/board_dependency.sh" \
    "IEP formats" "reference build missing at $mpp_lib"
fi

scp -q tools/iep2_probe.c "$BOARD":/tmp/iep2_probe_formats.c
ssh "$BOARD" "cc -O2 -w -I$mpp_ref/inc -I$mpp_ref/mpp/vproc/inc -I$mpp_ref/osal/inc \
        /tmp/iep2_probe_formats.c $mpp_lib/librockchip_mpp.so.1 -o /tmp/iep2_probe_formats 2>/dev/null ||
      cc -O2 -w -I$mpp_ref/inc -I$mpp_ref/mpp/vproc/inc -I$mpp_ref/osal/inc \
        /tmp/iep2_probe_formats.c $mpp_lib/librockchip_mpp.so.0 -o /tmp/iep2_probe_formats"

fail=0
run_cell() {
  local name="$1" width="$2" height="$3" mode="$4" mode_num="$5"
  local format="$6" fmt_num="$7" swap_num="$8"
  local input="/tmp/rk-mpp-rs-iep-format-$name.yuv"
  local c_out="/tmp/iep_format_c_$name.yuv"
  local rust_out="/tmp/iep_format_rust_$name.yuv"

  ssh "$BOARD" "python3 ~/rk-mpp-rs/scripts/gen_iep_fixture.py \
      $input $width $height 3 $format"
  if ! ssh "$BOARD" "sudo rm -f $c_out $rust_out &&
      sudo env LD_LIBRARY_PATH=$mpp_lib timeout 120 \
        /tmp/iep2_probe_formats $input $c_out $width $height \
        $mode_num 0 $fmt_num $swap_num >/dev/null 2>&1 &&
      sudo chmod 644 $c_out &&
      cd ~/rk-mpp-rs &&
      sudo timeout 120 ./target/release/rk-mpp-rs iep-dei \
        $input $rust_out $width $height TFF $mode $format >/dev/null &&
      sudo chmod 644 $rust_out"; then
    printf 'FAIL iep-format/%s (job failed)\n' "$name"
    fail=1
    return
  fi

  if ! ssh "$BOARD" "sudo cmp -s $c_out $rust_out"; then
    printf 'FAIL iep-format/%s (raw dst slots differ)\n' "$name"
    fail=1
    return
  fi
  printf 'PASS iep-format/%s\n' "$name"
}

run_cell 420sp 720 480 I5O2 1 420sp 3 0
run_cell 420spvu 720 480 I5O2 1 420spvu 3 1
run_cell 420p 720 480 I1O1T 5 420p 3 3
run_cell 422sp 640 360 I5O2 1 422sp 2 0
run_cell 422spvu 640 360 I1O1T 5 422spvu 2 1
run_cell 422p 640 360 I5O2 1 422p 2 3

exit "$fail"
