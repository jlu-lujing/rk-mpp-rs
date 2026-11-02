#!/usr/bin/env bash
BOARD="${BOARD:-t62}"
# H.264: our VEPU580 driver vs the C encoder, NAL by NAL, over QP and profile.
#
# The reference is MPP built from source at the vintage of the library installed
# on t6 (`MPP_REF_SRC`), with the harness' hard-coded profile rewritten per run -
# `mpi_enc_test` fixes High profile + CABAC + transform-8x8 in both that vintage
# and master and applies it after `-cfg`, so a cfg file cannot change it. See
# tools/patches/mpi_enc_test-constrained-baseline.patch for the four lines and
# why they have to be source lines.
#
# `mpi_enc_test` also inserts user-data SEI NAL units of its own, which are not
# encoder output. The compare covers the SEI NALUs too, as they are put
# there by mpp_enc_impl.cpp itself; see compare_streams.py.
#
# Only sizes whose width and height are multiples of 16 are byte-compared.
# `mpi_enc_test` seeks raw NV12 by width and ignores `-hstride`, so a
# stride-packed 322x242 frame shears into 9053 bytes of garbage where the packed
# one costs 1159, and 1920x1080 is in the same class because 1080 rounds to 1088.
# Non-multiple sizes are covered functionally - decoded and scored by PSNR - in
# tests/h264_encoder_hw.rs.
#
# Reference setup, once per board:
#   git clone https://github.com/rockchip-linux/mpp /tmp/mpp-old
#   git -C /tmp/mpp-old checkout <commit matching the installed library>
#   cmake -S /tmp/mpp-old -B /tmp/mpp-old/build -DCMAKE_BUILD_TYPE=Release
#   make -C /tmp/mpp-old/build -j8 rockchip_mpp mpi_enc_test
#
# Usage: scripts/board_h264_qp_vs_mpp.sh
set -uo pipefail

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target --exclude Cargo.lock -cf - . |
  ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release'

# No `-bps` on either side: the C harness derives a bit rate target from the
# frame size when the option is absent, and that number is printed into the
# uuid_rc_cfg SEI even under fixed QP, so letting both sides derive it tests the
# derivation instead of papering over it with one size's value.
# ONLY=sizes|gop runs one phase; the sizes phase rebuilds the reference harness
# per profile and takes minutes, so the GOP phase can be checked alone.
only="${ONLY:-all}"
mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_src="$mpp_ref/test/mpi_enc_test.c"
mpp_bin="$mpp_ref/build/test/mpi_enc_test"
mpp_lib="$mpp_ref/build/mpp"
fail=0

if ! ssh "$BOARD" "test -x $mpp_bin && test -f $mpp_lib/librockchip_mpp.so.0 && test -f $mpp_src"; then
  printf 'SKIP H.264 vs MPP (reference build missing at %s)\n' "$mpp_ref"
  exit 0
fi
ssh "$BOARD" "test -f /tmp/mpi_enc_test.c.orig || cp $mpp_src /tmp/mpi_enc_test.c.orig"

# The three profiles, spelled the way mpi_enc_test writes them.
profile_args() {
  case "$1" in
    baseline) echo 66 0 0 ;;
    main)     echo 77 1 0 ;;
    high)     echo 100 1 1 ;;
  esac
}

configure_reference_profile() {
  set -- $(profile_args "$1")
  ssh "$BOARD" "cp /tmp/mpi_enc_test.c.orig $mpp_src &&
          sed -i -e 's/\"h264:profile\", [0-9]*/\"h264:profile\", $1/' \
                 -e 's/\"h264:level\", [0-9]*/\"h264:level\", 30/' \
                 -e 's/\"h264:cabac_en\", [0-9]*/\"h264:cabac_en\", $2/' \
                 -e 's/\"h264:trans8x8\", [0-9]*/\"h264:trans8x8\", $3/' $mpp_src &&
          make -C $mpp_ref/build -j8 mpi_enc_test" >/dev/null 2>&1
}

# Compare every NAL unit, SEI ones included. `mpi_enc_impl.cpp` step 17 puts a
# uuid_version and a uuid_rc_cfg user-data SEI ahead of every IDR, so they are
# part of the access unit; only the version SEI is compared as a frame, since its
# payload is the build identity of whichever encoder ran.
compare() {
  scp -q scripts/compare_streams.py "$BOARD":/tmp/ 2>/dev/null
  ssh "$BOARD" "python3 /tmp/compare_streams.py /tmp/$1 /tmp/$2"
}

if [ "$only" = all ] || [ "$only" = sizes ]; then
for size in 64x64 320x240 640x480 1280x720; do
  width="${size%%x*}"; height="${size##*x}"
  frame="/tmp/rk-mpp-rs-probe-${size}.nv12"
  ssh "$BOARD" "cd ~/rk-mpp-rs && ./target/release/rk-mpp-rs probe-frame $frame $width $height" >/dev/null
  for profile in baseline main high; do
    for qp in 26 18; do
      tag="${size}-${profile}-qp${qp}"
      c_out="/tmp/rk-mpp-rs-h264-${tag}.264"
      rust_out="/tmp/rk-mpp-rs-h264-${tag}-rust.264"
      configure_reference_profile "$profile" || { printf 'FAIL %s (could not configure reference)\n' "$tag"; fail=1; continue; }
      ssh "$BOARD" "sudo rm -f $c_out $rust_out"
      if ! ssh "$BOARD" "sudo env LD_LIBRARY_PATH=$mpp_lib timeout 120 $mpp_bin -i $frame -o $c_out \
          -w $width -h $height -t 7 -f 0 -n 1 -rc 2 -qc ${qp}:${qp}:${qp}:${qp}:${qp} -v q" \
          >/dev/null 2>&1; then
        printf 'FAIL %-34s (reference encode failed)\n' "$tag"; fail=1; continue
      fi
      if ! ssh "$BOARD" "cd ~/rk-mpp-rs && timeout 120 sudo ./target/release/rk-mpp-rs h264-enc $frame $rust_out $width $height $qp $profile 1 60" \
          >/dev/null 2>&1; then
        printf 'FAIL %-34s (our encode failed)\n' "$tag"; fail=1; continue
      fi
      scp -q "$BOARD":"$c_out" "$BOARD":"$rust_out" /tmp/ 2>/dev/null
      if compare "${c_out##*/}" "${rust_out##*/}"; then
        printf 'PASS %-34s (SPS, PPS and IDR byte-identical to the C encoder)\n' "$tag"
      else
        printf 'FAIL %-34s (bitstreams differ)\n' "$tag"; fail=1
      fi
    done
  done
done
fi

# Multi-frame: the same comparison over a GOP, so the P path stays byte-exact and
# the slice-type dependent tables cannot silently regress. Still and moving
# fixtures, because the finetune module reads motion statistics between frames.
configure_reference_profile baseline
still6=/tmp/rk-mpp-rs-probe6-320x240.nv12
ssh "$BOARD" 'cd ~/rk-mpp-rs && ./target/release/rk-mpp-rs probe-frame /tmp/rk-mpp-rs-probe-1f.nv12 320 240 >/dev/null'
ssh "$BOARD" "rm -f $still6; for i in 1 2 3 4 5 6; do cat /tmp/rk-mpp-rs-probe-1f.nv12 >> $still6; done"
mv_fixture=/tmp/rk-mpp-rs-move-320x240.nv12
ssh "$BOARD" "ffmpeg -v error -y -f lavfi -i testsrc=size=320x240:rate=30 -frames:v 6 \
    -pix_fmt nv12 -f rawvideo -an $mv_fixture" >/dev/null 2>&1
for fixture in "$still6 still" "$mv_fixture moving"; do
  set -- $fixture
  src="$1"; label="$2"
  if ! ssh "$BOARD" "test -s $src"; then
    printf 'SKIP %-12s (source missing)\n' "$label"; continue
  fi
  for n in 2 4 6; do
    c_out="/tmp/rk-mpp-rs-gop-${label}-${n}f-c.264"
    rust_out="/tmp/rk-mpp-rs-gop-${label}-${n}f-rust.264"
    ssh "$BOARD" "sudo rm -f $c_out $rust_out"
    if ! ssh "$BOARD" "sudo env LD_LIBRARY_PATH=$mpp_lib timeout 180 $mpp_bin -i $src -o $c_out \
        -w 320 -h 240 -t 7 -f 0 -n $n -rc 2 -qc 26:26:26:26:26 -v q" >/dev/null 2>&1; then
      printf 'FAIL %-12s %df (reference encode failed)\n' "$label" "$n"; fail=1; continue
    fi
    if ! ssh "$BOARD" "cd ~/rk-mpp-rs && timeout 180 sudo ./target/release/rk-mpp-rs \
        h264-enc $src $rust_out 320 240 26 baseline $n 60" >/dev/null 2>&1; then
      printf 'FAIL %-12s %df (our encode failed)\n' "$label" "$n"; fail=1; continue
    fi
    scp -q "$BOARD":"$c_out" "$BOARD":"$rust_out" /tmp/ 2>/dev/null
    if compare "${c_out##*/}" "${rust_out##*/}"; then
      printf 'PASS %-12s %df (1 IDR + %d P byte-identical)\n' "$label" "$n" "$((n - 1))"
    else
      printf 'FAIL %-12s %df (bitstreams differ)\n' "$label" "$n"; fail=1
    fi
  done
done

# Leave the reference harness on the Baseline pin the patch documents.
configure_reference_profile baseline
exit "$fail"
