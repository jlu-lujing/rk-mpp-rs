#!/usr/bin/env bash
BOARD="${BOARD:-t62}"
# MPEG-4 Part 2 regression: the vdpu2 path must reproduce the C HAL byte for byte.
#
# mpi_dec_test -t 4 dumps NV12, which goes through ffmpeg before the compare.
# That conversion has bitten this project before: our own output is already
# planar, so re-reading it as NV12 keeps luma and scrambles chroma. Only the C
# dump is converted.
#
# The C decoder reuses one scratch buffer for every B picture and releases a
# reference only when the next one arrives, so an I picture reaches the output
# after the B pictures decoded against it. Our reference rotation reproduces
# that order, which is what makes a byte compare possible at all.
#
# Usage: scripts/board_regress_mp4.sh
set -euo pipefail

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target --exclude Cargo.lock -cf - . |
  ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release'

fail=0
mpp_bin=/tmp/mpp-develop/build/test/mpi_dec_test
mpp_lib=/tmp/mpp-develop/build/mpp
data='~/rk-mpp-rs/tests/data'

# fixture : width : height : pictures
streams=(
  "mp4_320x240_12_ip:320:240:12"
  "mp4_320x240_12_ibpb:320:240:12"
  "mp4_150x100_9_ip:150:100:9"
)

# -n caps the C output at the picture count; without it mpi_dec_test rewinds
# the file and keeps decoding, which no compare can line up with.
mpp_decode() {
  local stream="$1" frames="$2" output="$3"
  ssh "$BOARD" "sudo rm -f $output"
  ssh "$BOARD" "sudo env LD_LIBRARY_PATH=$mpp_lib $mpp_bin \
    -i $stream -t 4 -n $frames -o $output -v q" >/dev/null 2>&1
}

rust_decode() {
  local stream="$1" output="$2" frames="$3" log="$4"
  ssh "$BOARD" "sudo rm -f $output $log"
  ssh "$BOARD" "cd ~/rk-mpp-rs && sudo ./target/release/rk-mpp-rs mp4dec $stream $output \
    >$log 2>&1"
  ssh "$BOARD" "grep -q 'decoded $frames frames ($frames output) in' $log"
}

if ! ssh "$BOARD" "test -x $mpp_bin && test -f $mpp_lib/librockchip_mpp.so.0"; then
  printf 'SKIP MPEG-4 (MPP decoder binary or library missing)\n'
else
  for entry in "${streams[@]}"; do
    IFS=':' read -r name width height frames <<<"$entry"
    mpp_out="/tmp/${name}_mpp.yuv"
    planar="/tmp/${name}_mpp_planar.yuv"
    rust="/tmp/${name}_rust.yuv"

    if ! mpp_decode "$data/$name.m4v" "$frames" "$mpp_out"; then
      printf 'FAIL %s.m4v (MPP decode failed)\n' "$name"
      fail=1
      continue
    fi
    if ! ssh "$BOARD" "ffmpeg -loglevel error -y -f rawvideo -pixel_format nv12 \
        -video_size ${width}x${height} -i $mpp_out \
        -pix_fmt yuv420p -f rawvideo $planar"; then
      printf 'FAIL %s.m4v (could not convert MPP output)\n' "$name"
      fail=1
      continue
    fi
    expected_bytes=$((width * height * 3 / 2 * frames))
    if ! ssh "$BOARD" "test \"\$(stat -c %s $planar)\" -eq $expected_bytes"; then
      printf 'FAIL %s.m4v (unexpected MPP output frame count)\n' "$name"
      fail=1
      continue
    fi
    if ! rust_decode "$data/$name.m4v" "$rust" "$frames" "/tmp/${name}_rust.log"; then
      printf 'FAIL %s.m4v (Rust decode failed or dropped pictures)\n' "$name"
      fail=1
      continue
    fi
    if ssh "$BOARD" "cmp -s $planar $rust"; then
      printf 'PASS %s.m4v (%s frames, byte-exact vs MPP)\n' "$name" "$frames"
    else
      printf 'FAIL %s.m4v (picture bytes differ)\n' "$name"
      fail=1
    fi
  done
fi

exit "$fail"
