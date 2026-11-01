#!/usr/bin/env bash
# H.265 (M8): our parameter-set writer vs the C encoder, byte for byte.
#
# M8 opens with a fixed-QP single IDR. The byte-comparable part today is the
# parameter-set prefix (VPS + SPS + PPS), which the software header generator
# emits and which the Rust codec::h265::encoder writer reproduces from
# h265e_ps.c / h265e_header_gen.c. The IDR *payload* is NOT compared yet: the
# VEPU580 H.265 register image in src/hal/h265e.rs derives only the geometry,
# QP and syntax words and leaves the tuning windows (wgt, rdo, osd) at
# zero, so an IDR NAL from it is not yet byte-usable. That is the open item the
# tracer capture below feeds - exactly how H.264's "cime/rime corner" stayed
# behind a recorded template.
#
# What this script decides: PASS/FAIL on the VPS/SPS/PPS bytes per cell, and a
# labelled PENDING on the IDR. A header PASS is a real gate; the IDR line is not
# a failure but a recorded "waiting for the template".
#
# The reference is the same source-compiled MPP the H.264 comparison uses
# (MPP_REF, /tmp/mpp-old on t6). Its mpi_enc_test hard-codes profile/level for
# HEVC in this vintage, so where the harness cannot be made to leave them at the
# library defaults, fall back to tools/h265e_probe.c (same cell, defaults intact).
#
# Sizes are multiples of 16 for the same reason the H.264 matrix is: the raw NV12
# is seeked by width and -hstride is ignored.
#
# Usage:
#   scripts/board_h265_qp_vs_mpp.sh            # header compare
#   REFRESH=1 scripts/board_h265_qp_vs_mpp.sh  # also (re)capture the job template
#
# HEVC_TYPE overrides the harness coding type, and it is the decimal enum value:
# `mpi_enc_opt_t` in the vintage's utils/mpi_enc_utils.c does `atoi(next)`, so
# `0x01000004` would arrive as 0. In the vintage's inc/rk_type.h the enum runs
# Unused 0, AutoDetect 1, MPEG2 2, H263 3, MPEG4 4, WMV 5, RV 6, AVC 7, MJPEG 8,
# VP8 9, VP9 10, then VC1 = 0x01000000, FLV1, DIVX3, VP6 and **HEVC =
# 0x01000004 = 16777220**. `-t 9` is VP8, not HEVC - the first version of this
# script carried 9 and would have compared an H.265 file against a refused VP8
# encode. Confirmed on t6: 320x240, -n 1, -rc 2, -qc 26:26:26:26:26 writes 860
# bytes whose NAL types are 32 (VPS), 33 (SPS), 34 (PPS), 39, 39 (SEI) and 19
# (IDR_W_RADL).
set -uo pipefail

SKIP="${SKIP:-}"
REFRESH="${REFRESH:-}"
only="${ONLY:-headers}"
mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_bin="$mpp_ref/build/test/mpi_enc_test"
mpp_lib="$mpp_ref/build/mpp"
tracer="${TRACER:-/tmp/mpp_trace.so}"
hevc_type="${HEVC_TYPE:-16777220}"
frame_w=320
frame_h=240
fail=0

if ! ssh t6 "test -x $mpp_bin && test -f $mpp_lib/librockchip_mpp.so.1"; then
  printf 'SKIP H.265 vs MPP (reference build missing at %s)\n' "$mpp_ref"
  exit 0
fi

# Build the crate on the board (the header path is pure host code but the compare
# runs our release binary against the reference stream).
if [ -z "$SKIP" ]; then
  COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target -cf - . |
    ssh t6 'tar -xf - -C ~/rk-mpp-rs'
  ssh t6 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release'
fi

# ---- tracer template capture (REFRESH=1) -----------------------------------
# Records one fixed-QP HEVC job's register writes so the orchestrator can turn
# them into a recorded h265e template, the way tools/gen_h264e_vepu580_template.py
# did for H.264. The output is grep-only (the trace is large). This is the step
# that unblocks the IDR payload comparison in a later change.
if [ -n "$REFRESH" ]; then
  out="/tmp/rk-mpp-rs-h265-template-trace.txt"
  src="/tmp/rk-mpp-rs-h265-probe-${frame_w}x${frame_h}.nv12"
  ssh t6 "cd ~/rk-mpp-rs && ./target/release/rk-mpp-rs probe-frame $src $frame_w $frame_h" >/dev/null
  # The tracer writes each register-write block twice (before cmd=200 and a
  # standalone line); count jobs by the `write words=` lines, not the raw dump.
  ssh t6 "sudo env LD_LIBRARY_PATH=$mpp_lib LD_PRELOAD=$tracer timeout 120 \
    $mpp_bin -i $src -o /tmp/rk-mpp-rs-h265-template.265 \
    -w $frame_w -h $frame_h -t $hevc_type -f 0 -n 1 -rc 2 \
    -qc 26:26:26:26:26 -v q" >"$out" 2>&1
  printf 'captured H.265 job trace -> %s (%s bytes); convert it into a template next\n' \
    "$out" "$(wc -c <"$out" 2>/dev/null | tr -d ' ')"
fi

# ---- parameter-set comparison ----------------------------------------------
# Compare the VPS/SPS/PPS prefix with scripts/h265_hdr_cmp.py (byte prefix of the
# reference stream against the Rust file, robust to the unescaped `00 00 01` the
# SPS carries), then report the IDR as pending.
compare_headers() {
  scp -q scripts/h265_hdr_cmp.py t6:/tmp/ 2>/dev/null
  ssh t6 "python3 /tmp/h265_hdr_cmp.py /tmp/$1 /tmp/$2"
}

for size in 320x240 640x480 1280x720; do
  width="${size%%x*}"; height="${size##*x}"
  frame="/tmp/rk-mpp-rs-h265-probe-${size}.nv12"
  ssh t6 "cd ~/rk-mpp-rs && ./target/release/rk-mpp-rs probe-frame $frame $width $height" >/dev/null
  for qp in 26; do
    tag="${size}-qp${qp}"
    c_out="/tmp/rk-mpp-rs-h265-${tag}-c.265"
    rust_out="/tmp/rk-mpp-rs-h265-${tag}-rust.265"
    ssh t6 "sudo rm -f $c_out $rust_out"
    if ! ssh t6 "sudo env LD_LIBRARY_PATH=$mpp_lib timeout 120 $mpp_bin -i $frame -o $c_out \
        -w $width -h $height -t $hevc_type -f 0 -n 1 -rc 2 -qc ${qp}:${qp}:${qp}:${qp}:${qp} -v q" \
        >/dev/null 2>&1; then
      printf 'FAIL %-24s (reference encode failed)\n' "$tag"; fail=1; continue
    fi
    if ! ssh t6 "cd ~/rk-mpp-rs && timeout 120 ./target/release/rk-mpp-rs h265-enc $frame $rust_out $width $height $qp" \
        >/dev/null 2>&1; then
      printf 'FAIL %-24s (our encode failed)\n' "$tag"; fail=1; continue
    fi
    scp -q t6:"$c_out" t6:"$rust_out" /tmp/ 2>/dev/null
    if compare_headers "${c_out##*/}" "${rust_out##*/}"; then
      printf 'PASS %-24s (VPS, SPS and PPS byte-identical to the C encoder)\n' "$tag"
    else
      printf 'FAIL %-24s (parameter sets differ)\n' "$tag"; fail=1
    fi
    printf 'PENDING %-24s (IDR payload waits for the recorded VEPU580 job template)\n' "$tag"
  done
done

exit "$fail"
