#!/usr/bin/env bash
# JPEG restart markers: our VEPU2 JPEG encoder vs the C encoder, byte by byte.
#
# The restart interval is C's MPP_ENC_SPLIT_BY_CTU knob: split_arg counts
# MCUs per band, jpege_proc_hal() rounds it up over the MCU columns into
# part_rows and sets restart_ri = mcu_hor * part_rows, which the header
# declares in DRI and the hardware honours with the RST0..RST7 cycle.
# `mpi_enc_test` takes those two through the plain env names split_mode
# (2 = BY_CTU) and split_arg (mpi_enc_cmd_env_get in utils/mpi_enc_utils.c),
# so no source patch is needed.
#
# Quality uses the harness' jpeg:q_factor path (`-qc QF:qf_min:qf_max:0:0`)
# against our qf<factor> field; the qfactor -> qtable formula is the same
# arithmetic (MPP_CLIP3(1,255) of base*q+50/100) on both sides.
#
# Sizes are 16-aligned so mpi_enc_test's width-seeked reads and our
# stride-packed frames carry the same pixels.
#
# Usage: scripts/board_jpege_restart_vs_mpp.sh
set -uo pipefail

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target -cf - . |
  ssh t6 'tar -xf - -C ~/rk-mpp-rs'
ssh t6 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release'

mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_bin="$mpp_ref/build/test/mpi_enc_test"
mpp_lib="$mpp_ref/build/mpp"
fail=0

if ! ssh t6 "test -x $mpp_bin && test -f $mpp_lib/librockchip_mpp.so.0 && test -f $mpp_lib/librockchip_mpp.so.1"; then
  printf 'SKIP JPEG restart vs MPP (reference build missing at %s)\n' "$mpp_ref"
  exit 0
fi
for size in 320x240 640x480; do
  width="${size%%x*}"; height="${size##*x}"
  mcu_w=$((width / 16)); mcu_h=$((height / 16))
  frame="/tmp/rk-mpp-rs-probe-${size}.nv12"
  ssh t6 "cd ~/rk-mpp-rs && ./target/release/rk-mpp-rs probe-frame $frame $width $height" >/dev/null
  for qf in 50 90; do
    # One MCU row, two MCU rows, and a band that is not a whole multiple of
    # columns (must round up to the next whole row like C does).
    for arg in "$mcu_w" "$((2 * mcu_w))" "$((mcu_w + 1))"; do
      rows=$(( (arg + mcu_w - 1) / mcu_w ))
      ri=$(( mcu_w * rows ))
      if [ "$rows" -ge "$mcu_h" ]; then continue; fi
      tag="${size}-qf${qf}-rst${arg}"
      c_out="/tmp/rk-mpp-rs-jpeg-${tag}-c.jpg"
      rust_out="/tmp/rk-mpp-rs-jpeg-${tag}-rust.jpg"
      ssh t6 "sudo rm -f $c_out $rust_out"
      if ! ssh t6 "sudo env LD_LIBRARY_PATH=$mpp_lib split_mode=2 split_arg=$arg \
          timeout 120 $mpp_bin -i $frame -o $c_out \
          -w $width -h $height -t 8 -n 1 -qc ${qf}:1:99:0:0" \
          >/dev/null 2>&1; then
        printf 'FAIL %-28s (reference encode failed)\n' "$tag"; fail=1; continue
      fi
      if ! ssh t6 "cd ~/rk-mpp-rs && timeout 120 sudo ./target/release/rk-mpp-rs \
          jpeg-enc $frame $rust_out $width $height qf${qf} 420 nv12 rst${arg}" \
          >/dev/null 2>&1; then
        printf 'FAIL %-28s (our encode failed)\n' "$tag"; fail=1; continue
      fi
      if ! ssh t6 "sudo cmp -s $c_out $rust_out"; then
        printf 'FAIL %-28s (streams differ: C and Rust byte sizes or content)\n' "$tag"
        ssh t6 "ls -l $c_out $rust_out" 2>/dev/null
        fail=1
        continue
      fi
      # Independently of the byte match, confirm the C-derived restart
      # contract is actually present in the stream both sides wrote.
      if ssh t6 "sudo python3 ~/rk-mpp-rs/scripts/jpege_markers.py $rust_out $((mcu_w * mcu_h)) $ri" \
          >/dev/null 2>&1; then
        printf 'PASS %-28s (byte-identical, DRI=%d, (mcu-1)/ri markers cycle FFD0..7)\n' "$tag" "$ri"
      else
        printf 'FAIL %-28s (identical bytes but the marker audit disagrees)\n' "$tag"
        ssh t6 "sudo python3 ~/rk-mpp-rs/scripts/jpege_markers.py $rust_out $((mcu_w * mcu_h)) $ri"
        fail=1
      fi
    done
  done
done
exit "$fail"
