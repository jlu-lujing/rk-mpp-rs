#!/usr/bin/env bash
# H.264 fixed-QP IDR: our VEPU580 driver vs the C encoder, NAL by NAL.
#
# The reference is MPP built from source at the vintage of the library installed
# on t6, with tools/patches/mpi_enc_test-constrained-baseline.patch applied,
# because mpi_enc_test hard-codes High profile with CABAC and transform-8x8 in
# both that vintage and master and applies them after -cfg, so the stock tool
# cannot be asked for the Constrained Baseline job this driver produces. Point
# MPP_REF elsewhere to use a different build.
#
# `mpi_enc_test` also inserts user-data SEI NAL units of its own, which are not
# encoder output, so the compare is per NAL unit and skips type 6.
#
# Only sizes whose width *and* height are multiples of 16 are compared - 1920x1080
# is not one of them, since 1080 rounds up to 1088. `mpi_enc_test` seeks raw NV12 rows by width
# and ignores `-hstride`, so feeding it a stride-packed picture shears every row:
# at 322x242 that cost it 9053 bytes of entropy where the same frame, packed, cost
# 1159. What is coded in the padding macroblocks of a non-aligned picture is
# harness-dependent either way, so those sizes are checked functionally - decoded
# and scored by PSNR - in tests/h264_encoder_hw.rs instead of byte-compared here.
#
# Reference setup, once per board:
#   git clone https://github.com/rockchip-linux/mpp /tmp/mpp-old
#   git -C /tmp/mpp-old checkout <commit matching the installed library>
#   git -C /tmp/mpp-old apply ~/rk-mpp-rs/tools/patches/mpi_enc_test-constrained-baseline.patch
#   cmake -S /tmp/mpp-old -B /tmp/mpp-old/build -DCMAKE_BUILD_TYPE=Release
#   make -C /tmp/mpp-old/build -j8 rockchip_mpp mpi_enc_test
#
# Usage: scripts/board_h264_qp_vs_mpp.sh
set -uo pipefail

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target -cf - . |
  ssh t6 'tar -xf - -C ~/rk-mpp-rs'
ssh t6 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release'

mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_bin="$mpp_ref/build/test/mpi_enc_test"
mpp_lib="$mpp_ref/build/mpp"
fail=0

if ! ssh t6 "test -x $mpp_bin && test -f $mpp_lib/librockchip_mpp.so.0"; then
  printf 'SKIP H.264 vs MPP (reference build missing at %s)\n' "$mpp_ref"
  exit 0
fi

for size in 64x64 320x240 640x480 1280x720; do
  width="${size%%x*}"; height="${size##*x}"
  frame="/tmp/rk-mpp-rs-probe-${size}.nv12"
  ssh t6 "cd ~/rk-mpp-rs && ./target/release/rk-mpp-rs probe-frame $frame $width $height" >/dev/null

for qp in 26 18 42; do
  c_out="/tmp/rk-mpp-rs-h264-${size}-c-qp${qp}.264"
  rust_out="/tmp/rk-mpp-rs-h264-${size}-rust-qp${qp}.264"
  ssh t6 "sudo rm -f $c_out $rust_out"
  if ! ssh t6 "sudo env LD_LIBRARY_PATH=$mpp_lib timeout 120 $mpp_bin -i $frame -o $c_out \
      -w $width -h $height -t 7 -f 0 -n 1 -rc 2 -qc ${qp}:${qp}:${qp}:${qp}:${qp} -v q" \
      >/dev/null 2>&1; then
    printf 'FAIL %s qp %2d (reference encode failed)\n' "$size" "$qp"; fail=1; continue
  fi
  if ! ssh t6 "cd ~/rk-mpp-rs && timeout 120 sudo ./target/release/rk-mpp-rs h264-enc $frame $rust_out $width $height $qp" \
      >/dev/null 2>&1; then
    printf 'FAIL %s qp %2d (our encode failed)\n' "$size" "$qp"; fail=1; continue
  fi
  scp -q t6:"$c_out" t6:"$rust_out" /tmp/ 2>/dev/null
  if python3 - "${c_out##*/}" "${rust_out##*/}" <<'PY'
import re, sys

def nals(path):
    data = open(path, 'rb').read()
    pos = [m.start() for m in re.finditer(b'\x00\x00\x00\x01', data)] + [len(data)]
    out = {}
    for k in range(len(pos) - 1):
        body = data[pos[k] + 4:pos[k + 1]]
        if not body:
            continue
        unit = body[0] & 0x1f
        # SEI from the C harness is its own user data, not encoder output.
        if unit == 6:
            continue
        out.setdefault(unit, []).append(body)
    return out

c, r = nals('/tmp/' + sys.argv[1]), nals('/tmp/' + sys.argv[2])
if set(c) != set(r):
    print(f"    NAL types differ: C {sorted(c)} vs ours {sorted(r)}")
    sys.exit(1)
for unit in sorted(c):
    for a, b in zip(c[unit], r[unit]):
        if a != b:
            first = next(i for i, (x, y) in enumerate(zip(a, b)) if x != y)
            print(f"    NAL type {unit} differs at byte {first} ({len(a)} vs {len(b)})")
            sys.exit(1)
sys.exit(0)
PY
  then
    printf 'PASS %s qp %2d (SPS, PPS and IDR byte-identical to the C encoder)\n' "$size" "$qp"
  else
    printf 'FAIL %s qp %2d (bitstreams differ)\n' "$size" "$qp"; fail=1
  fi
done
done

exit "$fail"
