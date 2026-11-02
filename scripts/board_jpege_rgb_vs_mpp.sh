#!/usr/bin/env bash
BOARD="${BOARD:-t62}"
# JPEG RGB input: our VEPU2 JPEG encoder vs the C encoder.
#
# VEPU2 converts RGB to YCbCr in hardware (colour-conversion coefficients
# in registers 95..97, component masks in 98, input swap bits in 105), so
# an RGB input encodes directly with no RGA pass. The accepted formats are
# exactly the rows of vepu_common.c's vepu_rgb_cfg table that map to a
# real VepuFmt: the 16-bit 565/555/444 family, ARGB/ABGR/BGRA/RGBA8888
# and RGB/BGR101010. The 24-bit packed RGB888/BGR888 rows are
# VEPU_FMT_BUTT: VEPU2 cannot read them, and this script must not start
# pretending otherwise.
#
# `mpi_enc_test` selects the input format with `-f <format hex>`:
# RGB565 = 0x10000, BGRA8888 = 0x1000C, ARGB8888 = 0x1000A
# (MppFrameFormat in inc/mpp_frame.h). The fixture is one raw frame at the
# VEPU 8-pixel-aligned stride both sides compute, produced by
# scripts/gen_rgb_fixture.py so the two encoders see identical bytes.
#
# Usage: scripts/board_jpege_rgb_vs_mpp.sh
set -uo pipefail

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target --exclude Cargo.lock -cf - . |
  ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release'

mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_bin="$mpp_ref/build/test/mpi_enc_test"
mpp_lib="$mpp_ref/build/mpp"
fail=0

if ! ssh "$BOARD" "test -x $mpp_bin && test -f $mpp_lib/librockchip_mpp.so.0 && test -f $mpp_lib/librockchip_mpp.so.1"; then
  printf 'SKIP JPEG RGB vs MPP (reference build missing at %s)\n' "$mpp_ref"
  exit 0
fi

# format token, -f argument (MppFrameFormat hex)
for fmt in "rgb565:0x10000" "argb8888:0x1000A" "bgra8888:0x1000C"; do
  IFS=":" read -r name fmt_hex <<< "$fmt"
  for size in 320x240 640x480; do
    width="${size%%x*}"; height="${size##*x}"
    case "$name" in
      rgb565) stride=$((width * 2));;
      *)      stride=$((width * 4));;
    esac
    frame="/tmp/rk-mpp-rs-probe-${size}-${name}.bin"
    ssh "$BOARD" "python3 ~/rk-mpp-rs/scripts/gen_rgb_fixture.py $frame $width $height $name" >/dev/null
    for qf in 50 90; do
      tag="${size}-${name}-qf${qf}"
      c_out="/tmp/rk-mpp-rs-jpeg-${tag}-c.jpg"
      rust_out="/tmp/rk-mpp-rs-jpeg-${tag}-rust.jpg"
      ssh "$BOARD" "sudo rm -f $c_out $rust_out"
      if ! ssh "$BOARD" "sudo env LD_LIBRARY_PATH=$mpp_lib \
          timeout 120 $mpp_bin -i $frame -o $c_out \
          -w $width -h $height -f $fmt_hex -t 8 -n 1 -qc ${qf}:1:99:0:0" \
          >/dev/null 2>&1; then
        printf 'FAIL %-28s (reference encode failed)\n' "$tag"; fail=1; continue
      fi
      if ! ssh "$BOARD" "cd ~/rk-mpp-rs && timeout 120 sudo ./target/release/rk-mpp-rs \
          jpeg-enc $frame $rust_out $width $height qf${qf} 420 $name" \
          >/dev/null 2>&1; then
        printf 'FAIL %-28s (our encode failed)\n' "$tag"; fail=1; continue
      fi
      if ! ssh "$BOARD" "sudo cmp -s $c_out $rust_out"; then
        printf 'FAIL %-28s (streams differ; stride used was %d)\n' "$tag" "$stride"
        ssh "$BOARD" "ls -l $c_out $rust_out" 2>/dev/null
        fail=1
        continue
      fi
      printf 'PASS %-28s (byte-identical through the %s colour path)\n' "$tag" "$name"
    done
  done
done

# The byte-identical gate above is the verdict. tests/jpeg_restart_rotate_rgb.rs
# carries the ignored hardware cases that additionally decode an RGB stream
# with FFmpeg on the board.
exit "$fail"
