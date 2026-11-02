#!/usr/bin/env bash
BOARD="${BOARD:-t6}"
# H.263 regression: the vdpu2 path must reproduce the C HAL byte for byte.
#
# mpi_dec_test -t 3 dumps NV12, which goes through ffmpeg before the compare.
# Our own output is already planar, so only the C dump is converted - reading
# both as NV12 is the mistake that has cost this project time before.
#
# The formats of this profile are fixed: source format selects QCIF, CIF or 4CIF
# and the header carries no size of its own, so every fixture is one of them.
# Custom source formats and the H.263+ extensions of the PTYPE tail are refused
# by the C parser and by us, which is why nothing here exercises them.
#
# Usage: scripts/board_regress_h263.sh
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
  "h263_cif_12_ip:352:288:12"
  "h263_qcif_6_ip:176:144:6"
  "h263_4cif_10_ip:704:576:10"
)

# -n caps the C output at the picture count. The C decoder still runs the last
# couple of pictures again after that cap, but nothing more reaches the file, so
# the dumps line up.
mpp_decode() {
  local stream="$1" frames="$2" output="$3"
  ssh "$BOARD" "sudo rm -f $output"
  ssh "$BOARD" "sudo env LD_LIBRARY_PATH=$mpp_lib $mpp_bin \
    -i $stream -t 3 -n $frames -o $output -v q" >/dev/null 2>&1
}

rust_decode() {
  local stream="$1" output="$2" frames="$3" log="$4"
  ssh "$BOARD" "sudo rm -f $output $log"
  ssh "$BOARD" "cd ~/rk-mpp-rs && sudo ./target/release/rk-mpp-rs h263dec $stream $output \
    >$log 2>&1"
  ssh "$BOARD" "grep -q 'decoded $frames frames ($frames output) in' $log"
}

if ! ssh "$BOARD" "test -x $mpp_bin && test -f $mpp_lib/librockchip_mpp.so.0"; then
  printf 'SKIP H.263 (MPP decoder binary or library missing)\n'
else
  for entry in "${streams[@]}"; do
    IFS=':' read -r name width height frames <<<"$entry"
    mpp_out="/tmp/${name}_mpp.yuv"
    planar="/tmp/${name}_mpp_planar.yuv"
    rust="/tmp/${name}_rust.yuv"

    if ! mpp_decode "$data/$name.263" "$frames" "$mpp_out"; then
      printf 'FAIL %s.263 (MPP decode failed)\n' "$name"
      fail=1
      continue
    fi
    if ! ssh "$BOARD" "ffmpeg -loglevel error -y -f rawvideo -pixel_format nv12 \
        -video_size ${width}x${height} -i $mpp_out \
        -pix_fmt yuv420p -f rawvideo $planar"; then
      printf 'FAIL %s.263 (could not convert MPP output)\n' "$name"
      fail=1
      continue
    fi
    expected_bytes=$((width * height * 3 / 2 * frames))
    if ! ssh "$BOARD" "test \"\$(stat -c %s $planar)\" -eq $expected_bytes"; then
      printf 'FAIL %s.263 (unexpected MPP output frame count)\n' "$name"
      fail=1
      continue
    fi
    if ! rust_decode "$data/$name.263" "$rust" "$frames" "/tmp/${name}_rust.log"; then
      printf 'FAIL %s.263 (Rust decode failed or dropped pictures)\n' "$name"
      fail=1
      continue
    fi
    if ssh "$BOARD" "cmp -s $planar $rust"; then
      printf 'PASS %s.263 (%s frames, byte-exact vs MPP)\n' "$name" "$frames"
    else
      printf 'FAIL %s.263 (picture bytes differ)\n' "$name"
      fail=1
    fi
  done
fi

exit "$fail"
