#!/usr/bin/env bash
# H.265 (M8): an opening IDR plus one P picture against the C encoder, byte for byte.
#
# The second frame of the input is the same picture as the first, so the P is a
# still picture: no residual, a 17-byte NAL in the oracle's stream. That is the
# smallest cell that exercises the P register image - slice type, NAL type, POC,
# reference marking and the inter lambda table - without needing motion data.
#
# The first picture carries VPS/SPS/PPS and both SEIs; the C's `MppEncHeaderStatus`
# emits them once per sequence, and `h265-gop` does the same.
set -uo pipefail

mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_bin="$mpp_ref/build/test/mpi_enc_test"
mpp_lib="$mpp_ref/build/mpp"
hevc_type="${HEVC_TYPE:-16777220}"
only="${ONLY:-320x240}"
frames="${FRAMES:-2}"
qp="${QP:-26}"
fail=0

if ! ssh t6 "test -x $mpp_bin && test -f $mpp_lib/librockchip_mpp.so.1"; then
  printf 'SKIP H.265 GOP vs MPP (reference build missing at %s)\n' "$mpp_ref"
  exit 0
fi

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target -cf - . |
  ssh t6 'tar -xf - -C ~/rk-mpp-rs'
ssh t6 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release'

for size in $only; do
  width="${size%%x*}"
  height="${size##*x}"
  one="/tmp/rk-mpp-rs-h265-gop-${size}-1.nv12"
  seq="/tmp/rk-mpp-rs-h265-gop-${size}.nv12"
  c_out="/tmp/rk-mpp-rs-h265-gop-${size}-c.265"
  rust_out="/tmp/rk-mpp-rs-h265-gop-${size}-rust.265"
  ssh t6 "cd ~/rk-mpp-rs && ./target/release/rk-mpp-rs probe-frame $one $width $height" >/dev/null
  ssh t6 "sudo rm -f $seq; for i in \$(seq 1 $frames); do cat $one >> $seq; done; sudo rm -f $c_out $rust_out"
  if ! ssh t6 "sudo env LD_LIBRARY_PATH=$mpp_lib timeout 120 $mpp_bin -i $seq -o $c_out \
      -w $width -h $height -t $hevc_type -f 0 -n $frames -g 60 -rc 2 \
      -qc $qp:$qp:$qp:$qp:$qp -v q" >/dev/null 2>&1; then
    printf 'FAIL %-12s (reference encode failed)\n' "$size"; fail=1; continue
  fi
  if ! ssh t6 "cd ~/rk-mpp-rs && sudo timeout 120 ./target/release/rk-mpp-rs h265-gop $seq $rust_out $width $height $frames $qp" \
      >/tmp/h265-gop-${size}.log 2>&1; then
    printf 'FAIL %-12s (our encode failed: %s)\n' "$size" "$(tail -1 /tmp/h265-gop-${size}.log)"
    fail=1; continue
  fi
  c_size=$(ssh t6 "stat -c%s $c_out")
  r_size=$(ssh t6 "stat -c%s $rust_out")
  if ssh t6 "cmp -s $c_out $rust_out"; then
    printf 'PASS %-12s (%s bytes, %s pictures byte-identical to the C encoder)\n' \
      "$size" "$c_size" "$frames"
  else
    first=$(ssh t6 "cmp -l $c_out $rust_out 2>/dev/null | head -1")
    printf 'FAIL %-12s (C %s bytes, ours %s bytes, first difference: %s)\n' \
      "$size" "$c_size" "$r_size" "${first:-length}"
    fail=1
  fi
done

exit "$fail"
