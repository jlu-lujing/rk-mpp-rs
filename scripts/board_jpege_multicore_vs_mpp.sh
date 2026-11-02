#!/usr/bin/env bash
BOARD="${BOARD:-t6}"
# JPEG VEPU2 multi-core band partitioning above C's 1280x720 area gate.
#
# C submits up to four register sets as streaming MULTI_MSG requests, writes
# bands 1..3 to scratch DMA buffers, then concatenates their hardware lengths.
# Rust follows the same path and compares the complete JPEG byte for byte.
#
# Usage: scripts/board_jpege_multicore_vs_mpp.sh
set -uo pipefail

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target --exclude Cargo.lock --exclude '._*' -cf - . |
  ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release 2>&1 | tail -1'

mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_bin="$mpp_ref/build/test/mpi_enc_test"
mpp_lib="$mpp_ref/build/mpp"
if ! ssh "$BOARD" "test -x $mpp_bin && test -f $mpp_lib/librockchip_mpp.so.1"; then
  printf 'SKIP JPEG multicore (reference build missing at %s)\n' "$mpp_ref"
  exit 0
fi

fail=0
run_cell() {
  local size="$1" qf="$2" ri="$3" mcUs="$4"
  local width="${size%%x*}" height="${size##*x}"
  local frame="/tmp/rk-mpp-rs-jpeg-mc-$size.nv12"
  local c_out="/tmp/jpege_mc_c_$size-qf$qf.jpg"
  local rust_out="/tmp/jpege_mc_rust_$size-qf$qf.jpg"

  ssh "$BOARD" "ffmpeg -v error -y -f lavfi -i testsrc=size=${width}x${height}:rate=30 \
      -frames:v 1 -pix_fmt nv12 -f rawvideo -an $frame"
  if ! ssh "$BOARD" "sudo rm -f $c_out $rust_out &&
      sudo env LD_LIBRARY_PATH=$mpp_lib timeout 120 $mpp_bin \
        -i $frame -o $c_out -w $width -h $height -t 8 -n 1 \
        -qc ${qf}:1:99:0:0 >/dev/null 2>&1 &&
      cd ~/rk-mpp-rs &&
      sudo timeout 120 ./target/release/rk-mpp-rs jpeg-enc \
        $frame $rust_out $width $height qf$qf 420 nv12 >/dev/null"; then
    printf 'FAIL multicore/%s-qf%s (encoder failed)\n' "$size" "$qf"
    fail=1
    return
  fi
  if ! ssh "$BOARD" "sudo cmp -s $c_out $rust_out"; then
    printf 'FAIL multicore/%s-qf%s (streams differ)\n' "$size" "$qf"
    ssh "$BOARD" "sudo stat -c '%n %s' $c_out $rust_out" 2>/dev/null
    fail=1
    return
  fi
  if ! ssh "$BOARD" "sudo python3 ~/rk-mpp-rs/scripts/jpege_markers.py \
      $rust_out $mcUs $ri >/dev/null"; then
    printf 'FAIL multicore/%s-qf%s (DRI/restart audit)\n' "$size" "$qf"
    fail=1
    return
  fi
  printf 'PASS multicore/%s-qf%s (byte-identical, DRI=%s)\n' "$size" "$qf" "$ri"
}

# 120x68 MCUs: C's automatic RK3588 split is four equal 17-row bands.
run_cell 1920x1080 50 2040 $((120 * 68))
run_cell 1920x1080 90 2040 $((120 * 68))
# 81x46 MCUs: 12+12+12+10 rows, exercising the uneven final band and y_fill.
# 1288 is the 8-byte row stride the CLI fixture carries.
run_cell 1288x722 90 972 $((81 * 46))

exit "$fail"
