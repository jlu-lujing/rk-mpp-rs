#!/usr/bin/env bash
BOARD="${BOARD:?set BOARD to a Rockchip SSH alias}"
# VP8 regression: the vdpu2 path must reproduce the C HAL byte for byte.
#
# mpi_dec_test -t 9 dumps NV12, which goes through ffmpeg before the compare; our
# own output is already planar, so only the C dump is converted.
#
# The register stage runs the C decoder a second time with `hal_vp8d_debug=4`,
# which makes the HAL dump the image it is about to write to /data/video, and
# compares that with RFI_VP8_REG_DUMP. Only the words carrying a dma-buf handle
# are allowed to differ.
#
# Usage: scripts/board_regress_vp8.sh
set -euo pipefail

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target --exclude Cargo.lock --exclude .worktrees --exclude refs -cf - . |
  ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release'

fail=0
mpp_bin="${MPP_BIN:-/tmp/mpp-develop/build/test/mpi_dec_test}"
mpp_lib="${MPP_LIB:-/tmp/mpp-develop/build/mpp}"
data='~/rk-mpp-rs/tests/data'

# fixture : width : height : pictures : coded partitions
streams=(
  "vp8_320x240_12_ip:320:240:12"
  "vp8_320x240_12_parts:320:240:12"
  "vp8_320x240_12_aq:320:240:12"
  "vp8_150x100_9_ip:150:100:9"
  "vp8_320x240_12_altref:320:240:12"
  "vp8_320x240_24_altref:320:240:24"
)

mpp_decode() {
  local stream="$1" frames="$2" output="$3" dump_regs="$4"
  local env="LD_LIBRARY_PATH=$mpp_lib"
  ssh "$BOARD" "sudo rm -f $output"
  if [ "$dump_regs" = regs ]; then
    # The C dump appends, so the previous run has to go first.
    ssh "$BOARD" "sudo mkdir -p /data/video && sudo rm -f /data/video/reg_*.bin"
    env="$env hal_vp8d_debug=4"
  fi
  ssh "$BOARD" "sudo env $env $mpp_bin -i $stream -t 9 -n $frames -o $output -v q" >/dev/null 2>&1
}

rust_decode() {
  local stream="$1" output="$2" frames="$3" log="$4" dump_regs="$5"
  ssh "$BOARD" "sudo rm -f $output $log /tmp/our_vp8_regs.txt"
  local env=""
  [ "$dump_regs" = regs ] && env="RFI_VP8_REG_DUMP=1"
  ssh "$BOARD" "cd ~/rk-mpp-rs && sudo env $env ./target/release/rk-mpp-rs vp8dec $stream $output \
    >$log 2>&1"
  ssh "$BOARD" "grep -q 'decoded $frames frames ($frames output, 0 packets dropped)' $log"
}

if ! ssh "$BOARD" "test -x $mpp_bin && test -f $mpp_lib/librockchip_mpp.so.0"; then
  source "$(cd "$(dirname "$0")" && pwd)/board_dependency.sh" \
    "VP8 regression" "MPP decoder binary or library missing"
else
  for entry in "${streams[@]}"; do
    IFS=':' read -r name width height frames <<<"$entry"
    mpp_out="/tmp/${name}_mpp.yuv"
    planar="/tmp/${name}_mpp_planar.yuv"
    rust="/tmp/${name}_rust.yuv"

    if ! mpp_decode "$data/$name.ivf" "$frames" "$mpp_out" pixels; then
      printf 'FAIL %s.ivf (MPP decode failed)\n' "$name"
      fail=1
      continue
    fi
    if ! ssh "$BOARD" "ffmpeg -loglevel error -y -f rawvideo -pixel_format nv12 \
        -video_size ${width}x${height} -i $mpp_out \
        -pix_fmt yuv420p -f rawvideo $planar"; then
      printf 'FAIL %s.ivf (could not convert MPP output)\n' "$name"
      fail=1
      continue
    fi
    expected_bytes=$((width * height * 3 / 2 * frames))
    if ! ssh "$BOARD" "test \"\$(stat -c %s $planar)\" -eq $expected_bytes"; then
      printf 'FAIL %s.ivf (unexpected MPP output frame count)\n' "$name"
      fail=1
      continue
    fi
    if ! rust_decode "$data/$name.ivf" "$rust" "$frames" "/tmp/${name}_rust.log" pixels; then
      printf 'FAIL %s.ivf (Rust decode failed or dropped pictures)\n' "$name"
      fail=1
      continue
    fi
    if ssh "$BOARD" "cmp -s $planar $rust"; then
      printf 'PASS %s.ivf (%s frames, byte-exact vs MPP)\n' "$name" "$frames"
    else
      printf 'FAIL %s.ivf (picture bytes differ)\n' "$name"
      fail=1
    fi
  done

  # Register stage: one fixture, every job, word by word.
  name=vp8_320x240_12_ip
  if mpp_decode "$data/$name.ivf" 12 "/tmp/${name}_regs.yuv" regs &&
    rust_decode "$data/$name.ivf" "/tmp/${name}_rust_regs.yuv" 12 \
      "/tmp/${name}_rust_regs.log" regs; then
    ssh "$BOARD" 'sudo chmod -R a+r /data/video && cp -r /data/video /tmp/vp8_c_regs'
    if ssh "$BOARD" 'python3 ~/rk-mpp-rs/scripts/vp8_regdiff.py /tmp/vp8_c_regs /tmp/our_vp8_regs.txt'; then
      printf 'PASS %s.ivf (register images identical outside dma-buf handles)\n' "$name"
    else
      printf 'FAIL %s.ivf (register images differ)\n' "$name"
      fail=1
    fi
  else
    printf 'FAIL %s.ivf (register dump run failed)\n' "$name"
    fail=1
  fi
fi

exit "$fail"
