#!/usr/bin/env bash
BOARD="${BOARD:-t6}"
# H.265 rate-control board gate: the C encoder against the crate, whole streams.
#
# Two kinds of cell:
#   * explicit decisions  - the C's per-picture bit targets and QPs are fed into
#     `h265-gop-rc`, so the RC register image, the PPS/SEI prefix and the slice
#     NALs are what is under test. This cell is the 320x240 capture's decisions,
#     so it only runs at that size unless STEPS is overridden.
#   * model decisions     - `h265-rate` lets `rc_model::Model` choose the QPs.
#     These run for every size in SIZES (default 320x240) and cover CBR, VBR and
#     AVBR plus a smart GOP (`vi_len = 1`).
#
# The reference uses an explicit quality band because its default `-qc` leaves
# init_quality at -1 and takes a derivation path this crate has not reproduced.
set -uo pipefail

mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_bin="$mpp_ref/build/test/mpi_enc_test"
mpp_lib="$mpp_ref/build/mpp"
hevc_type="${HEVC_TYPE:-16777220}"
bps="${BPS:-300000:250000:350000}"
steps="${STEPS:-26:86955,25:582}"
sizes="${SIZES:-320x240}"
qp_min=10
qp_max=51
fail=0

if ! ssh "$BOARD" "test -x $mpp_bin && test -f $mpp_lib/librockchip_mpp.so.1"; then
  source "$(cd "$(dirname "$0")" && pwd)/board_dependency.sh" \
    "H.265 CBR vs MPP" "reference build missing at $mpp_ref"
fi

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target --exclude Cargo.lock --exclude .worktrees --exclude refs -cf - . |
  ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release'

for size in $sizes; do
  width="${size%%x*}"
  height="${size##*x}"
  one="/tmp/rk-mpp-rs-h265-cbr-${size}-1.nv12"
  seq="/tmp/rk-mpp-rs-h265-cbr-${size}.nv12"
  ssh "$BOARD" "cd ~/rk-mpp-rs && ./target/release/rk-mpp-rs probe-frame $one $width $height" >/dev/null
  ssh "$BOARD" "sudo rm -f $seq; cat $one $one > $seq"

  # ---- explicit-decision cell ------------------------------------------------
  if [ "$size" = "320x240" ]; then
    c_out="/tmp/rk-mpp-rs-h265-cbr-${size}-c.265"
    rust_out="/tmp/rk-mpp-rs-h265-cbr-${size}-rust.265"
    ssh "$BOARD" "sudo rm -f $c_out $rust_out"
    if ! ssh "$BOARD" "sudo env LD_LIBRARY_PATH=$mpp_lib timeout 120 $mpp_bin -i $seq -o $c_out \
        -w $width -h $height -t $hevc_type -f 0 -n 2 -g 60 -rc 1 -bps $bps \
        -qc 26:$qp_min:$qp_max:$qp_min:$qp_max -v q" >/dev/null 2>&1; then
      printf 'FAIL %-12s (reference encode failed)\n' "$size"; fail=1
    elif ! ssh "$BOARD" "cd ~/rk-mpp-rs && sudo timeout 120 ./target/release/rk-mpp-rs h265-gop-rc \
        $seq $rust_out $width $height $steps $qp_min $qp_max $bps 26" >/tmp/h265-cbr-${size}.log 2>&1; then
      printf 'FAIL %-12s (our encode failed: %s)\n' "$size" "$(tail -1 /tmp/h265-cbr-${size}.log)"
      fail=1
    elif ssh "$BOARD" "cmp -s $c_out $rust_out"; then
      printf 'PASS %-12s (%s bytes, CBR stream byte-identical with the C decisions)\n' \
        "$size" "$(ssh "$BOARD" "stat -c%s $c_out")"
    else
      first=$(ssh "$BOARD" "cmp -l $c_out $rust_out 2>/dev/null | head -1")
      printf 'FAIL %-12s (first difference: %s)\n' "$size" "${first:-length}"
      fail=1
    fi
  fi

  # ---- model-driven cells ------------------------------------------------------
  # The vintage's `-rc` indices, checked from the emitted SEI: 0:vbr 1:cbr 2:fixqp
  # 3:avbr (the usage string's order is stale).
  for mode in cbr vbr avbr; do
    case "$mode" in
      cbr) rc_index=1 ;;
      vbr) rc_index=0 ;;
      avbr) rc_index=3 ;;
    esac
    model_c="/tmp/rk-mpp-rs-h265-${mode}-${size}-c.265"
    model_r="/tmp/rk-mpp-rs-h265-${mode}-${size}-r.265"
    ssh "$BOARD" "sudo rm -f $model_c $model_r"
    if ! ssh "$BOARD" "sudo env LD_LIBRARY_PATH=$mpp_lib timeout 120 $mpp_bin -i $seq -o $model_c \
        -w $width -h $height -t $hevc_type -f 0 -n 2 -g 0:60:0 -rc $rc_index -bps $bps \
        -qc 26:$qp_min:$qp_max:$qp_min:$qp_max -v q" >/dev/null 2>&1; then
      printf 'FAIL %-12s %-4s (reference encode failed)\n' "$size" "$mode"; fail=1; continue
    fi
    if ! ssh "$BOARD" "cd ~/rk-mpp-rs && sudo timeout 120 ./target/release/rk-mpp-rs h265-rate \
        $seq $model_r $width $height 2 $bps 26 $qp_min $qp_max $mode" \
        >/tmp/h265-${mode}-${size}.log 2>&1; then
      printf 'FAIL %-12s %-4s (our encode failed: %s)\n' "$size" "$mode" \
        "$(tail -1 /tmp/h265-${mode}-${size}.log)"; fail=1; continue
    fi
    if ssh "$BOARD" "cmp -s $model_c $model_r"; then
      printf 'PASS %-12s %-4s (%s bytes, the model picked both QPs)\n' \
        "$size" "$mode" "$(ssh "$BOARD" "stat -c%s $model_c")"
    else
      first=$(ssh "$BOARD" "cmp -l $model_c $model_r 2>/dev/null | head -1")
      printf 'FAIL %-12s %-4s (first difference: %s)\n' "$size" "$mode" "${first:-length}"
      fail=1
    fi
  done

  # ---- smart-GOP cell -----------------------------------------------------------
  vi_c="/tmp/rk-mpp-rs-h265-vi-${size}-c.265"
  vi_r="/tmp/rk-mpp-rs-h265-vi-${size}-r.265"
  ssh "$BOARD" "sudo rm -f $vi_c $vi_r"
  if ! ssh "$BOARD" "sudo env LD_LIBRARY_PATH=$mpp_lib timeout 120 $mpp_bin -i $seq -o $vi_c \
      -w $width -h $height -t $hevc_type -f 0 -n 2 -g 0:60:1 -rc 1 -bps $bps \
      -qc 26:$qp_min:$qp_max:$qp_min:$qp_max -v q" >/dev/null 2>&1; then
    printf 'FAIL %-12s vi   (reference encode failed)\n' "$size"; fail=1
  elif ! ssh "$BOARD" "cd ~/rk-mpp-rs && sudo timeout 120 ./target/release/rk-mpp-rs h265-rate \
      $seq $vi_r $width $height 2 $bps 26 $qp_min $qp_max cbr" >/tmp/h265-vi-${size}.log 2>&1; then
    printf 'FAIL %-12s vi   (our encode failed: %s)\n' "$size" "$(tail -1 /tmp/h265-vi-${size}.log)"
    fail=1
  elif ssh "$BOARD" "cmp -s $vi_c $vi_r"; then
    printf 'PASS %-12s vi   (%s bytes, smart GOP, model picked both QPs)\n' \
      "$size" "$(ssh "$BOARD" "stat -c%s $vi_c")"
  else
    first=$(ssh "$BOARD" "cmp -l $vi_c $vi_r 2>/dev/null | head -1")
    printf 'FAIL %-12s vi   (first difference: %s)\n' "$size" "${first:-length}"
    fail=1
  fi
done

exit "$fail"
