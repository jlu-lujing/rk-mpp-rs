#!/usr/bin/env bash
BOARD="${BOARD:?set BOARD to a Rockchip SSH alias}"
# VEPU2 JPEG low-delay packet output against the 2024-04 C oracle.
#
# C's low-delay mode runs one hardware job per MCU band but produces the same
# bytes as the single job with the same restart interval. Rust keeps the single
# job and exposes packet ranges at the RST markers. This compares both the
# complete JPEG and every packet boundary.
#
# Usage: scripts/board_jpege_lowdelay_vs_mpp.sh
set -uo pipefail

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target --exclude Cargo.lock --exclude .worktrees --exclude refs --exclude '._*' -cf - . |
  ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release 2>&1 | tail -1'

mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_lib="$mpp_ref/build/mpp"
if ! ssh "$BOARD" "test -f $mpp_lib/librockchip_mpp.so.1 || test -f $mpp_lib/librockchip_mpp.so.0"; then
  source "$(cd "$(dirname "$0")" && pwd)/board_dependency.sh" \
    "JPEG lowdelay" "reference build missing at $mpp_ref"
fi

scp -q tools/jpege_probe.c "$BOARD":/tmp/jpege_probe.c
ssh "$BOARD" "cc -O2 -w -pthread -I$mpp_ref/inc -I$mpp_ref/osal/inc \
    /tmp/jpege_probe.c $mpp_lib/librockchip_mpp.so.1 -o /tmp/jpege_probe 2>/dev/null ||
  cc -O2 -w -pthread -I$mpp_ref/inc -I$mpp_ref/osal/inc \
    /tmp/jpege_probe.c $mpp_lib/librockchip_mpp.so.0 -o /tmp/jpege_probe"

fail=0
run_cell() {
  local size="$1" qf="$2" split="$3" packets="$4"
  local width="${size%%x*}" height="${size##*x}"
  local frame="/tmp/rk-mpp-rs-jpeg-ld-$size.nv12"
  local c_out="/tmp/jpege_ld_c_$size.jpg"
  local rust_out="/tmp/jpege_ld_rust_$size.jpg"
  local c_log="/tmp/jpege_ld_c_$size.log"
  local rust_log="/tmp/jpege_ld_rust_$size.log"
  local tag="$size-qf$qf-split$split"

  ssh "$BOARD" "cd ~/rk-mpp-rs && ./target/release/rk-mpp-rs probe-frame \
      $frame $width $height" >/dev/null
  if ! ssh "$BOARD" "sudo rm -f $c_out $rust_out $c_log $rust_log &&
      sudo env LD_LIBRARY_PATH=$mpp_lib timeout 60 /tmp/jpege_probe \
        $frame $c_out $width $height $qf $split 1 2>$c_log &&
      cd ~/rk-mpp-rs &&
      sudo env RK_MPP_TRACE_PACKETS=1 ./target/release/rk-mpp-rs jpeg-enc \
        $frame $rust_out $width $height qf$qf 420 nv12 rst$split \
        splitout:lowdelay 2>$rust_log"; then
    printf 'FAIL lowdelay/%s (encoder failed)\n' "$tag"
    fail=1
    return
  fi

  scp -q "$BOARD":"$c_out" "$BOARD":"$rust_out" \
    "$BOARD":"$c_log" "$BOARD":"$rust_log" /tmp/
  if ! cmp -s "/tmp/jpege_ld_c_$size.jpg" "/tmp/jpege_ld_rust_$size.jpg"; then
    printf 'FAIL lowdelay/%s (streams differ)\n' "$tag"
    fail=1
    return
  fi
  if ! python3 scripts/compare_packet_sizes.py \
      "/tmp/jpege_ld_c_$size.log" "/tmp/jpege_ld_rust_$size.log"; then
    printf 'FAIL lowdelay/%s (packet boundaries differ)\n' "$tag"
    fail=1
    return
  fi
  local actual
  actual=$(python3 - "$rust_log" <<'PY'
import re
import sys
for line in open(sys.argv[1], encoding="utf-8", errors="replace"):
    match = re.match(r"^jpeg frame 0: (\d+) packet", line)
    if match:
        print(match.group(1))
        break
PY
)
  if [ "$actual" != "$packets" ]; then
    printf 'FAIL lowdelay/%s (got %s packets, expected %s)\n' "$tag" "$actual" "$packets"
    fail=1
    return
  fi
  printf 'PASS lowdelay/%s (%s packets, byte-identical)\n' "$tag" "$packets"
}

# 20x15 MCUs: three rows per band -> five packets.
run_cell 320x240 50 60 5
# 40x30 MCUs: three rows per band -> ten packets. This also exceeds MPP's
# eight-slot output task pool, exercising the probe's concurrent packet drain.
run_cell 640x480 90 120 10

exit "$fail"
