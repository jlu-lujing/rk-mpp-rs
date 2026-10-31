#!/usr/bin/env bash
# MPEG-2 regression: the vdpu2 path must reproduce the C HAL byte for byte.
#
# The progressive fixtures are locked against mpi_dec_test -t 2, whose NV12 dump
# is converted to planar with ffmpeg before the compare. The interlaced fixture
# is locked against ffmpeg instead: on that stream mpi_dec_test disagrees with
# ffmpeg by whole-picture differences (it also emits 40 pictures for a 6-picture
# stream once it rewinds the input), so the C HAL is not an oracle there.
# Hardware MPEG-2 is not bit-exact with software MPEG-2 either - the difference
# is IDCT rounding, so that fixture allows a per-sample difference of 2.
#
# Usage: scripts/board_regress_m2v.sh
set -euo pipefail

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target -cf - . |
  ssh t6 'tar -xf - -C ~/rk-mpp-rs'
ssh t6 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release'

fail=0
mpp_bin=/tmp/mpp-develop/build/test/mpi_dec_test
mpp_lib=/tmp/mpp-develop/build/mpp
data='~/rk-mpp-rs/tests/data'

# fixture : width : height : pictures
streams=(
  "m2v_320x240_12:320:240:10"
  "m2v_64x64_3:64:64:3"
)

# -n caps the C output at the picture count; without it mpi_dec_test rewinds
# the file and keeps decoding, which no compare can line up with.
mpp_decode() {
  local stream="$1" frames="$2" output="$3"
  ssh t6 "sudo rm -f $output"
  ssh t6 "sudo env LD_LIBRARY_PATH=$mpp_lib $mpp_bin \
    -i $stream -t 2 -n $frames -o $output -v q" >/dev/null 2>&1
}

rust_decode() {
  local stream="$1" output="$2" frames="$3" log="$4"
  ssh t6 "sudo rm -f $output $log"
  ssh t6 "cd ~/rk-mpp-rs && sudo ./target/release/rk-mpp-rs m2vdec $stream $output \
    >$log 2>&1"
  ssh t6 "grep -q 'decoded $frames frames ($frames output) in' $log"
}

if ! ssh t6 "test -x $mpp_bin && test -f $mpp_lib/librockchip_mpp.so.0"; then
  printf 'SKIP MPEG-2 (MPP decoder binary or library missing)\n'
else
  for entry in "${streams[@]}"; do
    IFS=':' read -r name width height frames <<<"$entry"
    mpp_out="/tmp/${name}_mpp.yuv"
    planar="/tmp/${name}_mpp_planar.yuv"
    rust="/tmp/${name}_rust.yuv"

    if ! mpp_decode "$data/$name.m2v" "$frames" "$mpp_out"; then
      printf 'FAIL %s.m2v (MPP decode failed)\n' "$name"
      fail=1
      continue
    fi
    if ! ssh t6 "ffmpeg -loglevel error -y -f rawvideo -pixel_format nv12 \
        -video_size ${width}x${height} -i $mpp_out \
        -pix_fmt yuv420p -f rawvideo $planar"; then
      printf 'FAIL %s.m2v (could not convert MPP output)\n' "$name"
      fail=1
      continue
    fi
    expected_bytes=$((width * height * 3 / 2 * frames))
    if ! ssh t6 "test \"\$(stat -c %s $planar)\" -eq $expected_bytes"; then
      printf 'FAIL %s.m2v (unexpected MPP output frame count)\n' "$name"
      fail=1
      continue
    fi
    if ! rust_decode "$data/$name.m2v" "$rust" "$frames" "/tmp/${name}_rust.log"; then
      printf 'FAIL %s.m2v (Rust decode failed or dropped pictures)\n' "$name"
      fail=1
      continue
    fi
    if ssh t6 "cmp -s $planar $rust"; then
      printf 'PASS %s.m2v (%s frames, byte-exact vs MPP)\n' "$name" "$frames"
    else
      printf 'FAIL %s.m2v (picture bytes differ)\n' "$name"
      fail=1
    fi
  done
fi

# Interlaced pictures: ffmpeg is the oracle, hardware rounding is allowed.
if ! ssh t6 'command -v ffmpeg >/dev/null && command -v python3 >/dev/null'; then
  printf 'SKIP m2v_320x240_ilaced.m2v (ffmpeg or python3 unavailable)\n'
elif ! ssh t6 "ffmpeg -loglevel error -y -i $data/m2v_320x240_ilaced.m2v \
    -pix_fmt yuv420p -f rawvideo /tmp/m2v_ilaced_ff.yuv" 2>/dev/null; then
  printf 'FAIL m2v_320x240_ilaced.m2v (could not decode with ffmpeg)\n'
  fail=1
elif ! rust_decode "$data/m2v_320x240_ilaced.m2v" /tmp/m2v_ilaced_rust.yuv 6 \
  /tmp/m2v_ilaced_rust.log; then
  printf 'FAIL m2v_320x240_ilaced.m2v (Rust decode failed or dropped pictures)\n'
  fail=1
elif ! ssh t6 'python3 - <<"PY"
from pathlib import Path

a = Path("/tmp/m2v_ilaced_ff.yuv").read_bytes()
b = Path("/tmp/m2v_ilaced_rust.yuv").read_bytes()
assert len(a) == len(b) == 320 * 240 * 3 // 2 * 6, (len(a), len(b))
worst = max(abs(x - y) for x, y in zip(a, b))
bad = sum(1 for x, y in zip(a, b) if x != y)
print(f"max diff {worst} over {bad} differing bytes")
raise SystemExit(0 if worst <= 2 else 1)
PY'; then
  printf 'FAIL m2v_320x240_ilaced.m2v (picture bytes differ)\n'
  fail=1
else
  printf 'PASS m2v_320x240_ilaced.m2v (6 fields, ffmpeg rounding match)\n'
fi

exit "$fail"
