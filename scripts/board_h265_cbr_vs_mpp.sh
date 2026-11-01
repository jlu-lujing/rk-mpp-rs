#!/usr/bin/env bash
# H.265: the CBR encoding path against the C encoder, with the C's own decisions.
#
# This cell does not ask our model to choose the QP yet. It feeds the two
# decisions the C made on this input - IDR 26 with 86955 bits, P 25 with 582
# bits - into `h265-gop-rc`, so what is under test is everything else: the RC
# register image, the CBR PPS/SEI prefix, and both slice NALs.
#
# The reference is the harness with an explicit quality band, because its
# default `-qc` leaves init_quality at -1 and takes a derivation path this crate
# has not reproduced. With `-qc 26:10:51:10:51` both sides state the same band.
set -uo pipefail

mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_bin="$mpp_ref/build/test/mpi_enc_test"
mpp_lib="$mpp_ref/build/mpp"
hevc_type="${HEVC_TYPE:-16777220}"
bps="${BPS:-300000:250000:350000}"
steps="${STEPS:-26:86955,25:582}"
size="${SIZE:-320x240}"
qp_min=10
qp_max=51

if ! ssh t6 "test -x $mpp_bin && test -f $mpp_lib/librockchip_mpp.so.1"; then
  printf 'SKIP H.265 CBR vs MPP (reference build missing at %s)\n' "$mpp_ref"
  exit 0
fi

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target -cf - . |
  ssh t6 'tar -xf - -C ~/rk-mpp-rs'
ssh t6 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release'

width="${size%%x*}"
height="${size##*x}"
one="/tmp/rk-mpp-rs-h265-cbr-${size}-1.nv12"
seq="/tmp/rk-mpp-rs-h265-cbr-${size}.nv12"
c_out="/tmp/rk-mpp-rs-h265-cbr-${size}-c.265"
rust_out="/tmp/rk-mpp-rs-h265-cbr-${size}-rust.265"
ssh t6 "cd ~/rk-mpp-rs && ./target/release/rk-mpp-rs probe-frame $one $width $height" >/dev/null
ssh t6 "sudo rm -f $seq; cat $one $one > $seq; sudo rm -f $c_out $rust_out"

if ! ssh t6 "sudo env LD_LIBRARY_PATH=$mpp_lib timeout 120 $mpp_bin -i $seq -o $c_out \
    -w $width -h $height -t $hevc_type -f 0 -n 2 -g 60 -rc 1 -bps $bps \
    -qc 26:$qp_min:$qp_max:$qp_min:$qp_max -v q" >/dev/null 2>&1; then
  printf 'FAIL %-12s (reference encode failed)\n' "$size"; exit 1
fi
if ! ssh t6 "cd ~/rk-mpp-rs && sudo timeout 120 ./target/release/rk-mpp-rs h265-gop-rc \
    $seq $rust_out $width $height $steps $qp_min $qp_max $bps 26" >/tmp/h265-cbr-${size}.log 2>&1; then
  printf 'FAIL %-12s (our encode failed: %s)\n' "$size" "$(tail -1 /tmp/h265-cbr-${size}.log)"
  exit 1
fi

c_size=$(ssh t6 "stat -c%s $c_out")
r_size=$(ssh t6 "stat -c%s $rust_out")
if ssh t6 "cmp -s $c_out $rust_out"; then
  printf 'PASS %-12s (%s bytes, CBR stream byte-identical with the C decisions)\n' "$size" "$c_size"
else
  first=$(ssh t6 "cmp -l $c_out $rust_out 2>/dev/null | head -1")
  printf 'FAIL %-12s (C %s bytes, ours %s bytes, first difference: %s)\n' \
    "$size" "$c_size" "$r_size" "${first:-length}"
  exit 1
fi
