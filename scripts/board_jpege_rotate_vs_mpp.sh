#!/usr/bin/env bash
BOARD="${BOARD:-t6}"
# JPEG 90/270-degree rotation: our VEPU2 JPEG encoder vs the C encoder.
#
# Both sides get the same unrotated NV12 fixture. The C harness takes the
# rotation through the plain env name `rotation` with the MppEncRotationCfg
# value (1 = 90, 3 = 270; mpi_enc_cmd_env_get in utils/mpi_enc_utils.c),
# which mpp_enc_cfg_set_prep() applies as a width_set/height_set swap: the
# SOF dimensions come out transposed while the hardware still reads the
# input at its original geometry - exactly what our jpeg-enc rot90/rot270
# encodes by putting the rotation bits in register 74 and the transposed
# dimensions in the SOF.
#
# The C output must therefore decode as height x width.
#
# Usage: scripts/board_jpege_rotate_vs_mpp.sh
set -uo pipefail

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target --exclude Cargo.lock --exclude .worktrees --exclude refs -cf - . |
  ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release'

mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_bin="$mpp_ref/build/test/mpi_enc_test"
mpp_lib="$mpp_ref/build/mpp"
fail=0

if ! ssh "$BOARD" "test -x $mpp_bin && test -f $mpp_lib/librockchip_mpp.so.0 && test -f $mpp_lib/librockchip_mpp.so.1"; then
  printf 'SKIP JPEG rotation vs MPP (reference build missing at %s)\n' "$mpp_ref"
  exit 0
fi

# env name, MppEncRotationCfg value, our CLI token.
for rot in "1:90:rot90" "3:270:rot270"; do
  set -- ${rot//:/ }
  env_rot="$1"; degrees="$2"; cli_rot="$3"
  for size in 320x240 640x480; do
    width="${size%%x*}"; height="${size##*x}"
    frame="/tmp/rk-mpp-rs-probe-${size}.nv12"
    ssh "$BOARD" "cd ~/rk-mpp-rs && ./target/release/rk-mpp-rs probe-frame $frame $width $height" >/dev/null
    for qf in 50 90; do
      tag="${size}-qf${qf}-rot${degrees}"
      c_out="/tmp/rk-mpp-rs-jpeg-${tag}-c.jpg"
      rust_out="/tmp/rk-mpp-rs-jpeg-${tag}-rust.jpg"
      ssh "$BOARD" "sudo rm -f $c_out $rust_out"
      if ! ssh "$BOARD" "sudo env LD_LIBRARY_PATH=$mpp_lib rotation=$env_rot \
          timeout 120 $mpp_bin -i $frame -o $c_out \
          -w $width -h $height -t 8 -n 1 -qc ${qf}:1:99:0:0" \
          >/dev/null 2>&1; then
        printf 'FAIL %-28s (reference encode failed)\n' "$tag"; fail=1; continue
      fi
      if ! ssh "$BOARD" "cd ~/rk-mpp-rs && timeout 120 sudo ./target/release/rk-mpp-rs \
          jpeg-enc $frame $rust_out $width $height qf${qf} 420 nv12 $cli_rot" \
          >/dev/null 2>&1; then
        printf 'FAIL %-28s (our encode failed)\n' "$tag"; fail=1; continue
      fi
      if ! ssh "$BOARD" "sudo cmp -s $c_out $rust_out"; then
        printf 'FAIL %-28s (streams differ)\n' "$tag"
        ssh "$BOARD" "ls -l $c_out $rust_out" 2>/dev/null
        fail=1
        continue
      fi
      # The transposed SOF must be visible independently of the byte match.
      sof_dims=$(ssh "$BOARD" "sudo python3 ~/rk-mpp-rs/scripts/jpege_markers.py $rust_out" 2>/dev/null |
        sed -n 's/.*SOF \([0-9]*x[0-9]*\).*/\1/p')
      if [ "$sof_dims" = "${height}x${width}" ]; then
        printf 'PASS %-28s (byte-identical, SOF transposed to %s)\n' "$tag" "$sof_dims"
      else
        printf 'FAIL %-28s (SOF says %s, expected %sx%s)\n' "$tag" "$sof_dims" "$height" "$width"
        fail=1
      fi
    done
  done
done
exit "$fail"
