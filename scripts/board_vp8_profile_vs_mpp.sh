#!/usr/bin/env bash
# VP8 profile 1/3 board question: does vdpu2 reproduce the profile 1/3 IDCT
# precision, and does C behave the same way?
#
# Front end (syntax, register derivation) is locked by host unit tests; this
# script is the board half. It prefers real corpus streams when the profile
# probe finds any, otherwise it derives variants by rewriting the version bits
# of every frame tag of the repo fixture - the only thing VP8 profiles change
# in the container and the syntax, so the patch is bit-faithful and both
# decoders see exactly the same packets.
#
# The judge is deliberately weak at this stage: exit code, frame count and
# byte-identity *between the two decoders*, not against a software reference,
# because the profile-1/3 software reference (libvpx IDCT tables) is a separate
# question from what this hardware emits. Record the verdict, do not gate on
# it until the probe says whether real streams exist.
#
# Usage: scripts/board_vp8_profile_vs_mpp.sh [corpus-dir]
set -euo pipefail

corpus="${1:-/home/user/rk-mpp-oracles/corpus}"
here="$(cd "$(dirname "$0")" && pwd)"

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target -cf - . |
  ssh t6 'tar -xf - -C ~/rk-mpp-rs'
ssh t6 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release'

mpp_bin=/tmp/mpp-develop/build/test/mpi_dec_test
mpp_lib=/tmp/mpp-develop/build/mpp
data='~/rk-mpp-rs/tests/data'
width=320
height=240
frames=12

# Stage 0: ask the corpus first. Anything not profile 0 there outranks the
# derived fixture because it came out of a real encoder.
scp -q "$here/board_vp8_profile_probe.py" t6:/tmp/board_vp8_profile_probe.py </dev/null
ssh t6 "python3 /tmp/board_vp8_profile_probe.py $corpus" </dev/null |
  grep -E 'v[1-7]:' || printf 'corpus carries profile 0 only; using derived fixtures\n'

ssh t6 "cat > /tmp/vp8_patch_profile.py" <<'PY'
import struct, sys
src, dst, version = sys.argv[1], sys.argv[2], int(sys.argv[3])
data = open(src, 'rb').read()
header_size = struct.unpack_from('<H', data, 6)[0]
out = bytearray(data)
offset = header_size
while offset + 12 <= len(data):
    size = struct.unpack_from('<I', data, offset)[0]
    if size == 0 or offset + 12 + size > len(data):
        break
    start = offset + 12
    out[start] = (out[start] & 0xF1) | ((version & 7) << 1)
    offset = start + size
open(dst, 'wb').write(out)
PY

fail=0
for profile in 1 3; do
  name="vp8_p${profile}_320x240_12_ip"
  stream="/tmp/${name}.ivf"
  ssh t6 "python3 /tmp/vp8_patch_profile.py \
    ~/rk-mpp-rs/tests/data/vp8_320x240_12_ip.ivf $stream $profile"

  mpp_out="/tmp/${name}_mpp.nv12"
  planar="/tmp/${name}_mpp.yuv"
  rust="/tmp/${name}_rust.yuv"
  mpp_log="/tmp/${name}_mpp.log"
  rust_log="/tmp/${name}_rust.log"

  ssh t6 "sudo rm -f $mpp_out; sudo env LD_LIBRARY_PATH=$mpp_lib timeout 120 \
    $mpp_bin -i $stream -t 9 -n $frames -o $mpp_out -v q >$mpp_log 2>&1" && mpp_rc=0 || mpp_rc=$?
  ssh t6 "sudo rm -f $rust; cd ~/rk-mpp-rs && sudo timeout 120 \
    ./target/release/rk-mpp-rs vp8dec $stream $rust >$rust_log 2>&1" && rust_rc=0 || rust_rc=$?

  ssh t6 "ffmpeg -loglevel error -y -f rawvideo -pixel_format nv12 \
    -video_size ${width}x${height} -i $mpp_out -pix_fmt yuv420p -f rawvideo $planar" \
    >/dev/null 2>&1 || true
  mpp_frames=$(ssh t6 "stat -c %s $planar 2>/dev/null || echo 0" | awk '{print int($1/115200)}')
  rust_frames=$(ssh t6 "test -f $rust && stat -c %s $rust || echo 0" | awk '{print int($1/115200)}')

  if [ "$mpp_rc" != "$rust_rc" ]; then
    printf 'FAIL profile %s: exit code differs (C %s, Rust %s)\n' "$profile" "$mpp_rc" "$rust_rc"
    fail=1
  elif [ "$mpp_frames" != "$rust_frames" ]; then
    printf 'FAIL profile %s: frame count differs (C %s, Rust %s)\n' "$profile" "$mpp_frames" "$rust_frames"
    fail=1
  elif [ "$mpp_frames" = 0 ]; then
    printf 'NOTE profile %s: both decoders produced no output (rc %s)\n' "$profile" "$mpp_rc"
  elif ssh t6 "cmp -s $planar $rust"; then
    printf 'PASS profile %s: %s frames byte-identical to C\n' "$profile" "$rust_frames"
  else
    printf 'DIFF profile %s: %s frames each but bytes differ (record, hardware question)\n' \
      "$profile" "$rust_frames"
  fi
done

# Derived key-frame scale-factor check: the two scale fields are display-only.
# libvpx (FFmpeg) and the C MPP both decode such a stream at the coded size, and so
# does this crate since the refusal was removed - the judge is byte identity.
ssh t6 "python3 - /tmp/vp8_scaled_key.ivf ~/rk-mpp-rs/tests/data/vp8_320x240_12_ip.ivf" <<'PY'
import struct, sys
dst, src = sys.argv[1], sys.argv[2]
data = open(src, 'rb').read()
header_size = struct.unpack_from('<H', data, 6)[0]
out = bytearray(data)
offset = header_size
size = struct.unpack_from('<I', data, offset)[0]
payload = offset + 12
# set the width scale bits (value 2 = half size) inside the coded-size word
word = struct.unpack_from('<H', data, payload + 6)[0]
struct.pack_into('<H', out, payload + 6, word | (2 << 14))
open(dst, 'wb').write(out)
PY
scaled_c=/tmp/vp8_scaled_key_c.nv12
scaled_cp=/tmp/vp8_scaled_key_c.yuv
scaled_r=/tmp/vp8_scaled_key_r.yuv
ssh t6 "sudo rm -f $scaled_c $scaled_cp $scaled_r"
ssh t6 "sudo env LD_LIBRARY_PATH=$mpp_lib timeout 120 $mpp_bin -i /tmp/vp8_scaled_key.ivf \
  -t 9 -n $frames -o $scaled_c -v q" >/dev/null 2>&1 || true
ssh t6 "cd ~/rk-mpp-rs && sudo timeout 120 ./target/release/rk-mpp-rs vp8dec \
  /tmp/vp8_scaled_key.ivf $scaled_r" >/dev/null 2>&1 || true
ssh t6 "ffmpeg -loglevel error -y -f rawvideo -pixel_format nv12 \
  -video_size ${width}x${height} -i $scaled_c -pix_fmt yuv420p -f rawvideo $scaled_cp" \
  >/dev/null 2>&1 || true
if ssh t6 "test -s $scaled_cp && test -s $scaled_r && cmp -s $scaled_cp $scaled_r"; then
  printf 'PASS key-frame scale: %s frames byte-identical to C at the coded size\n' "$frames"
else
  printf 'FAIL key-frame scale: exit or bytes differ from C\n'
  fail=1
fi

exit "$fail"
