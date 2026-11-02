#!/usr/bin/env bash
BOARD="${BOARD:-t6}"
# H.265 (M8): the first *submitted* IDR against the C encoder, byte for byte.
#
# `h265-idr` owns every surface the job points at and goes through the same
# /dev/mpp_service batch the H.264 encoder does, so this compares whole streams:
# VPS, SPS, PPS, the two SEIs and the IDR NAL.
#
# Sizes default to the captured 320x240 cell, where the register template was
# taken. Wider cells need the geometry fields the template still pins; they are
# listed by ONLY and will say so rather than silently comparing the wrong image.
set -uo pipefail

mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_bin="$mpp_ref/build/test/mpi_enc_test"
mpp_lib="$mpp_ref/build/mpp"
hevc_type="${HEVC_TYPE:-16777220}"
only="${ONLY:-320x240}"
qp="${QP:-26}"
fail=0

if ! ssh "$BOARD" "test -x $mpp_bin && test -f $mpp_lib/librockchip_mpp.so.1"; then
  printf 'SKIP H.265 IDR vs MPP (reference build missing at %s)\n' "$mpp_ref"
  exit 0
fi

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target --exclude Cargo.lock -cf - . |
  ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release'

for size in $only; do
  width="${size%%x*}"
  height="${size##*x}"
  frame="/tmp/rk-mpp-rs-h265-idr-${size}.nv12"
  c_out="/tmp/rk-mpp-rs-h265-idr-${size}-c.265"
  rust_out="/tmp/rk-mpp-rs-h265-idr-${size}-rust.265"
  ssh "$BOARD" "cd ~/rk-mpp-rs && ./target/release/rk-mpp-rs probe-frame $frame $width $height" >/dev/null
  ssh "$BOARD" "sudo rm -f $c_out $rust_out"
  if ! ssh "$BOARD" "sudo env LD_LIBRARY_PATH=$mpp_lib timeout 120 $mpp_bin -i $frame -o $c_out \
      -w $width -h $height -t $hevc_type -f 0 -n 1 -rc 2 -qc $qp:$qp:$qp:$qp:$qp -v q" \
      >/dev/null 2>&1; then
    printf 'FAIL %-12s (reference encode failed)\n' "$size"; fail=1; continue
  fi
  if ! ssh "$BOARD" "cd ~/rk-mpp-rs && sudo timeout 120 ./target/release/rk-mpp-rs h265-idr $frame $rust_out $width $height $qp" \
      >/tmp/h265-idr-${size}.log 2>&1; then
    printf 'FAIL %-12s (our encode failed: %s)\n' "$size" "$(tail -1 /tmp/h265-idr-${size}.log)"
    fail=1; continue
  fi
  c_size=$(ssh "$BOARD" "stat -c%s $c_out")
  r_size=$(ssh "$BOARD" "stat -c%s $rust_out")
  if ssh "$BOARD" "cmp -s $c_out $rust_out"; then
    printf 'PASS %-12s (%s bytes, stream byte-identical to the C encoder)\n' "$size" "$c_size"
  else
    first=$(ssh "$BOARD" "cmp -l $c_out $rust_out 2>/dev/null | head -1")
    printf 'FAIL %-12s (C %s bytes, ours %s bytes, first difference: %s)\n' \
      "$size" "$c_size" "$r_size" "${first:-length}"
    fail=1
  fi
done

exit "$fail"
