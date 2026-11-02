#!/usr/bin/env bash
BOARD="${BOARD:-t6}"
# Damaged-stream comparison against the C decoder, on the selected board.
#
# The damage is built ON THE BOARD from the repo fixtures, per the plan: the
# corpus itself is not modified. Three shapes are run for VP8, VP9 and AV1:
#   container  - the file ends mid-frame
#   half-last  - the final packet is cut in half (missing tail data)
#   mid-flip   - one byte inside a middle packet is inverted (partition damage)
#
# Judged per cell, always reported, never silently skipped:
#   rc      exit code of each decoder (0 vs non-zero)
#   frames  output bytes / per-frame size (C dumps NV12, we dump planar;
#           both use 3/2*w*h per 8-bit frame, so the count is comparable)
# A NAMED cell means the two behaved differently; the reason column carries
# what our side said. Rust writes every picture up to the failure point
# (VP9) or none (VP8, whose CLI only prints on full success - recorded
# divergence), so frame counts on damaged input are read with that in mind.
#
# This script never fails: its output IS the evidence table.
#
# Usage: scripts/board_vp_corrupt_vs_mpp.sh
set -uo pipefail

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target --exclude Cargo.lock -cf - . |
  ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release'

mpp_bin=/tmp/mpp-develop/build/test/mpi_dec_test
mpp_lib=/tmp/mpp-develop/build/mpp

ssh "$BOARD" "cat > /tmp/vp_damage.py" <<'PY'
import struct, sys
kind, src, dst = sys.argv[1], sys.argv[2], sys.argv[3]
data = open(src, 'rb').read()
if kind == 'container':
    data = data[:-3]
else:
    header_size = struct.unpack_from('<H', data, 6)[0]
    out = bytearray(data)
    frames = []
    offset = header_size
    while offset + 12 <= len(data):
        size = struct.unpack_from('<I', data, offset)[0]
        frames.append((offset, 12 + size))
        offset += 12 + size
    if kind == 'half-last':
        start, total = frames[-1]
        half = (total - 12) // 2
        struct.pack_into('<I', out, start, half)
        out = out[:start + 12 + half]
    elif kind == 'mid-flip':
        start, total = frames[len(frames) // 2]
        payload = start + 12
        out[payload + (total - 12) // 2] ^= 0xFF
    data = bytes(out)
open(dst, 'wb').write(data)
PY

run_pair() { # codec-type frame_bytes label stream
  local type="$1" frame_bytes="$2" label="$3" stream="$4" rust_cmd="$5"
  ssh "$BOARD" "sudo rm -f /tmp/vpd_c.yuv /tmp/vpd_r.yuv"
  ssh "$BOARD" "sudo env LD_LIBRARY_PATH=$mpp_lib timeout 60 \
    $mpp_bin -i $stream -t $type -o /tmp/vpd_c.yuv" >/dev/null 2>&1 && c_rc=0 || c_rc=$?
  ssh "$BOARD" "cd ~/rk-mpp-rs && sudo timeout 60 \
    $rust_cmd $stream /tmp/vpd_r.yuv" >/dev/null 2>&1 && r_rc=0 || r_rc=$?
  c_frames=$(ssh "$BOARD" "stat -c %s /tmp/vpd_c.yuv 2>/dev/null || echo 0" | awk -v fb="$frame_bytes" '{printf "%d", $1/fb}')
  r_frames=$(ssh "$BOARD" "stat -c %s /tmp/vpd_r.yuv 2>/dev/null || echo 0" | awk -v fb="$frame_bytes" '{printf "%d", $1/fb}')
  r_msg=$(ssh "$BOARD" "cd ~/rk-mpp-rs && sudo timeout 60 \
    $rust_cmd $stream /tmp/vpd_r_msg.yuv 2>&1 >/dev/null | head -1" | tr -d '\r' || true)
  if [ "$((c_rc != 0))" = "$((r_rc != 0))" ] && [ "$c_frames" = "$r_frames" ]; then
    if [[ "$label" == av1/* ]]; then
      if ssh "$BOARD" 'ffmpeg -loglevel error -y -f rawvideo -pixel_format nv12 \
          -video_size 320x240 -i /tmp/vpd_c.yuv -pix_fmt yuv420p -f rawvideo \
          /tmp/vpd_c_planar.yuv && cmp -s /tmp/vpd_c_planar.yuv /tmp/vpd_r.yuv'; then
        printf 'SAME-PIXELS %-13s rc both %s, frames both %s\n' \
          "$label" "$c_rc" "$c_frames"
      else
        printf 'NAMED %-18s rc both %s, frames both %s, pixels differ\n' \
          "$label" "$c_rc" "$c_frames"
      fi
    else
      printf 'SAME  %-18s rc both %s, frames both %s\n' "$label" "$c_rc" "$c_frames"
    fi
  else
    printf 'NAMED %-18s rc C=%s Rust=%s | frames C=%s Rust=%s | rust said: %s\n' \
      "$label" "$c_rc" "$r_rc" "$c_frames" "$r_frames" "$r_msg"
  fi
}

if ! ssh "$BOARD" "test -x $mpp_bin"; then
  printf 'SKIP (mpi_dec_test missing)\n'
  exit 0
fi

for damage in container half-last mid-flip; do
  ssh "$BOARD" "python3 /tmp/vp_damage.py $damage ~/rk-mpp-rs/tests/data/vp8_320x240_12_ip.ivf /tmp/vpd_vp8_$damage.ivf"
  run_pair 9 115200 "vp8/$damage" "/tmp/vpd_vp8_$damage.ivf" \
    "./target/release/rk-mpp-rs vp8dec"
done
for damage in container half-last mid-flip; do
  ssh "$BOARD" "python3 /tmp/vp_damage.py $damage ~/rk-mpp-rs/tests/data/vp9_p0_64x64.ivf /tmp/vpd_vp9_$damage.ivf"
  run_pair 11 6144 "vp9/$damage" "/tmp/vpd_vp9_$damage.ivf" \
    "./target/release/rk-mpp-rs vp9dec"
done
for damage in container half-last mid-flip; do
  ssh "$BOARD" "python3 /tmp/vp_damage.py $damage ~/rk-mpp-rs/tests/data/av1_320x240_10.ivf /tmp/vpd_av1_$damage.ivf"
  run_pair 16777224 115200 "av1/$damage" "/tmp/vpd_av1_$damage.ivf" \
    "./target/release/rk-mpp-rs av1dec"
done

printf '\n(cells are evidence: file them under the ROADMAP damaged-stream entry)\n'
exit 0
