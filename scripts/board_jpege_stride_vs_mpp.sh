#!/usr/bin/env bash
BOARD="${BOARD:?set BOARD to a Rockchip SSH alias}"
# Non-multiple-of-8 widths and non-multiple-of-16 heights.
#
# mpi_enc_test consumes a tight NV12 file and copies each row into its own
# aligned input buffer. rk-mpp-rs consumes the already-strided buffer directly.
# Generate both views from the same raw frame and require byte-identical JPEGs.
#
# Usage: scripts/board_jpege_stride_vs_mpp.sh
set -uo pipefail

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target --exclude Cargo.lock --exclude .worktrees --exclude refs --exclude '._*' -cf - . |
  ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release 2>&1 | tail -1'

mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_bin="$mpp_ref/build/test/mpi_enc_test"
mpp_lib="$mpp_ref/build/mpp"
if ! ssh "$BOARD" "test -x $mpp_bin && test -f $mpp_lib/librockchip_mpp.so.1"; then
  source "$(cd "$(dirname "$0")" && pwd)/board_dependency.sh" \
    "JPEG stride" "reference build missing at $mpp_ref"
fi

fail=0
run_cell() {
  local size="$1" qf="$2"
  local width="${size%%x*}" height="${size##*x}"
  local padded_width=$(( (width + 7) & ~7 ))
  local tight="/tmp/jpege_stride_$size-tight.nv12"
  local padded="/tmp/jpege_stride_$size-padded.nv12"
  local c_out="/tmp/jpege_stride_c_$size.jpg"
  local rust_out="/tmp/jpege_stride_rust_$size.jpg"

  ssh "$BOARD" "ffmpeg -v error -y -f lavfi -i testsrc=size=${width}x${height}:rate=30 \
      -frames:v 1 -pix_fmt nv12 -f rawvideo -an $tight &&
    python3 ~/rk-mpp-rs/scripts/pad_nv12.py \
      $tight $padded $width $height $padded_width"
  if ! ssh "$BOARD" "sudo rm -f $c_out $rust_out &&
      sudo env LD_LIBRARY_PATH=$mpp_lib timeout 120 $mpp_bin \
        -i $tight -o $c_out -w $width -h $height -t 8 -n 1 \
        -qc ${qf}:1:99:0:0 >/dev/null 2>&1 &&
      cd ~/rk-mpp-rs &&
      sudo timeout 120 ./target/release/rk-mpp-rs jpeg-enc \
        $padded $rust_out $width $height qf$qf 420 nv12 >/dev/null"; then
    printf 'FAIL stride/%s-qf%s (encoder failed)\n' "$size" "$qf"
    fail=1
    return
  fi
  if ! ssh "$BOARD" "sudo cmp -s $c_out $rust_out"; then
    printf 'FAIL stride/%s-qf%s (streams differ)\n' "$size" "$qf"
    fail=1
    return
  fi
  printf 'PASS stride/%s-qf%s\n' "$size" "$qf"
}

# Beneath the multi-core gate: align2 height and align8 width only.
run_cell 322x242 50
# Above the gate: non-8 width plus multi-core input/output offsets.
run_cell 1366x768 90

exit "$fail"
