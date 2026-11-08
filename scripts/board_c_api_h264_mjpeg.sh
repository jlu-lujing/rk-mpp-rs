#!/usr/bin/env bash

# Real C/C++ consumers for the public H.264, H.265 and MJPEG decoder ABI.

set -euo pipefail

board="${BOARD:-t6}"

ssh "$board" 'mkdir -p "$HOME/rk-mpp-rs"'
COPYFILE_DISABLE=1 tar \
    --no-xattrs \
    --exclude .git \
    --exclude target \
    --exclude .worktrees \
    --exclude refs \
    -cf - . |
    ssh "$board" 'tar -xf - -C "$HOME/rk-mpp-rs"'

ssh "$board" 'bash -s' <<'REMOTE'
set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"
cd "$HOME/rk-mpp-rs"
cargo build --offline --lib --bins

for symbol in \
    rk_mpp_api_version \
    rk_mpp_api_version_string \
    rk_mpp_strerror \
    rk_mpp_decoder_config_init \
    rk_mpp_frame_init \
    rk_mpp_decoder_create \
    rk_mpp_decoder_submit_packet \
    rk_mpp_decoder_receive_frame \
    rk_mpp_frame_release \
    rk_mpp_decoder_flush \
    rk_mpp_decoder_reset \
    rk_mpp_decoder_recover \
    rk_mpp_decoder_status \
    rk_mpp_decoder_close \
    rk_jpeg_open \
    rk_jpeg_frame \
    rk_jpeg_close
do
    nm -D --defined-only target/debug/librk_mpp_rs.so |
        awk '{print $3}' | grep -qx "$symbol"
done

gcc -std=c11 -Wall -Wextra -Werror \
    -Iinclude tests/c_api_decoder.c \
    -Ltarget/debug -lrk_mpp_rs -Wl,-rpath,"$PWD/target/debug" \
    -o /tmp/rk_mpp_c_api_decoder
g++ -std=c++17 -Wall -Wextra -Werror \
    -Iinclude tests/c_api_decoder_smoke.cpp \
    -Ltarget/debug -lrk_mpp_rs -Wl,-rpath,"$PWD/target/debug" \
    -o /tmp/rk_mpp_c_api_smoke
g++ -std=c++17 -Wall -Wextra -Werror \
    -Iinclude tests/c_api_decoder_smoke.cpp \
    target/debug/librk_mpp_rs.a -ldl -lpthread -lm \
    -o /tmp/rk_mpp_c_api_smoke_static
LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_c_api_smoke
/tmp/rk_mpp_c_api_smoke_static

h264=/tmp/rk_mpp_c_api_h264.264
h265=/tmp/rk_mpp_c_api_h265.265
h265_10=/tmp/rk_mpp_c_api_h265_10.265
mjpeg_dir=/tmp/rk_mpp_c_api_mjpeg
c_h264=/tmp/rk_mpp_c_api_h264_c.nv12
c_api_h264=/tmp/rk_mpp_c_api_h264.nv12
c_api_h264_tiny=/tmp/rk_mpp_c_api_h264_tiny.nv12
c_api_h264_one_packet=/tmp/rk_mpp_c_api_h264_one_packet.nv12
c_api_h264_i420=/tmp/rk_mpp_c_api_h264.i420
c_h264_i420=/tmp/rk_mpp_c_api_h264_c.i420
c_h265=/tmp/rk_mpp_c_api_h265_c.nv12
c_api_h265=/tmp/rk_mpp_c_api_h265.nv12
c_api_h265_one_packet=/tmp/rk_mpp_c_api_h265_one_packet.nv12
c_api_h265_i420=/tmp/rk_mpp_c_api_h265.i420
c_h265_i420=/tmp/rk_mpp_c_api_h265_c.i420
c_h265_10_packed=/tmp/rk_mpp_c_api_h265_10_c.packed
c_h265_10_planar=/tmp/rk_mpp_c_api_h265_10_c.i420p10
c_api_h265_10=/tmp/rk_mpp_c_api_h265_10.i420p10
c_api_mjpeg=/tmp/rk_mpp_c_api_mjpeg.nv12
c_api_mjpeg_one_packet=/tmp/rk_mpp_c_api_mjpeg_one_packet.nv12
c_mjpeg=/tmp/rk_mpp_c_api_mjpeg_c.nv12
c_api_mjpeg_i420=/tmp/rk_mpp_c_api_mjpeg.i420
c_mjpeg_i420=/tmp/rk_mpp_c_api_mjpeg_c.i420

rm -rf "$mjpeg_dir"
mkdir -p "$mjpeg_dir"
ffmpeg -nostdin -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=640x480:rate=30" -frames:v 6 \
    -pix_fmt yuv420p -c:v libx264 -preset veryfast \
    -x264-params "keyint=30:min-keyint=30:bframes=3:scenecut=0:repeat-headers=1" \
    -f h264 "$h264"
ffmpeg -nostdin -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=320x240:rate=10" -frames:v 4 \
    -pix_fmt yuvj420p -strict unofficial -c:v mjpeg -q:v 3 \
    "$mjpeg_dir/frame_%02d.jpg"
rm -f "$h265"
for _ in 1 2 3 4; do
    cat tests/data/h265e_c_fixqp_qp26_320x240.265 >> "$h265"
done
ffmpeg -nostdin -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=256x256:rate=30" -frames:v 6 \
    -pix_fmt yuv420p10le -c:v libx265 \
    -x265-params "keyint=6:min-keyint=6:open-gop=0:bframes=3:b-pyramid=1:scenecut=0:repeat-headers=1:log-level=warning" \
    -f hevc "$h265_10"

rm -f "$c_h264"
LD_LIBRARY_PATH=/usr/local/lib:/usr/lib/aarch64-linux-gnu \
    timeout 120 /usr/local/bin/mpi_dec_test \
    -i "$h264" -t 7 -n 6 -o "$c_h264" -v q \
    < /dev/null > /tmp/rk_mpp_c_api_h264_c.log 2>&1
ffmpeg -nostdin -hide_banner -loglevel error -y \
    -f rawvideo -pixel_format nv12 -video_size 640x480 \
    -i "$c_h264" -pix_fmt yuv420p -f rawvideo "$c_h264_i420"

LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_c_api_decoder h264 nv12 313 "$c_api_h264" "$h264"
cmp "$c_h264" "$c_api_h264"
LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_c_api_decoder h264 nv12 7 "$c_api_h264_tiny" "$h264"
cmp "$c_h264" "$c_api_h264_tiny"
LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_c_api_decoder h264 nv12 1048576 \
    "$c_api_h264_one_packet" "$h264"
cmp "$c_h264" "$c_api_h264_one_packet"
LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_c_api_decoder h264 i420 127 "$c_api_h264_i420" "$h264"
cmp "$c_h264_i420" "$c_api_h264_i420"

rm -f "$c_h265"
LD_LIBRARY_PATH=/usr/local/lib:/usr/lib/aarch64-linux-gnu \
    timeout 120 /usr/local/bin/mpi_dec_test \
    -i "$h265" -t 16777220 -w 320 -h 240 -n 4 -o "$c_h265" -v q \
    < /dev/null > /tmp/rk_mpp_c_api_h265_c.log 2>&1
ffmpeg -nostdin -hide_banner -loglevel error -y \
    -f rawvideo -pixel_format nv12 -video_size 320x240 \
    -i "$c_h265" -pix_fmt yuv420p -f rawvideo "$c_h265_i420"

LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_c_api_decoder h265 nv12 37 "$c_api_h265" "$h265"
cmp "$c_h265" "$c_api_h265"
LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_c_api_decoder h265 nv12 1048576 \
    "$c_api_h265_one_packet" "$h265"
cmp "$c_h265" "$c_api_h265_one_packet"
LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_c_api_decoder h265 i420 211 "$c_api_h265_i420" "$h265"
cmp "$c_h265_i420" "$c_api_h265_i420"

rm -f "$c_h265_10_packed" "$c_h265_10_planar"
LD_LIBRARY_PATH=/usr/local/lib:/usr/lib/aarch64-linux-gnu \
    timeout 120 /usr/local/bin/mpi_dec_test \
    -i "$h265_10" -t 16777220 -w 256 -h 256 -n 6 \
    -o "$c_h265_10_packed" -v q \
    < /dev/null > /tmp/rk_mpp_c_api_h265_10_c.log 2>&1
python3 - "$c_h265_10_packed" "$c_h265_10_planar" <<'PY'
from pathlib import Path
import sys

width, height, frames = 256, 256, 6
stride = width * 2
frame_size = width * height * 2 * 3 // 2
src = Path(sys.argv[1]).read_bytes()
if len(src) != frame_size * frames:
    raise SystemExit(f"unexpected MPP byte count: {len(src)}")

out = bytearray(len(src))
half_width = width // 2
chroma_row_bytes = half_width * 2
for frame in range(frames):
    src_frame = frame * frame_size
    dst_frame = frame * frame_size
    y_bytes = width * height * 2
    out[dst_frame:dst_frame + y_bytes] = src[src_frame:src_frame + y_bytes]
    src_uv = src_frame + y_bytes
    dst_u = dst_frame + y_bytes
    dst_v = dst_u + width * height // 2
    for row in range(height // 2):
        line = src[src_uv + row * stride:src_uv + (row + 1) * stride]
        urow = bytearray(chroma_row_bytes)
        vrow = bytearray(chroma_row_bytes)
        urow[0::2] = line[0::4]
        urow[1::2] = line[1::4]
        vrow[0::2] = line[2::4]
        vrow[1::2] = line[3::4]
        u = dst_u + row * chroma_row_bytes
        v = dst_v + row * chroma_row_bytes
        out[u:u + chroma_row_bytes] = urow
        out[v:v + chroma_row_bytes] = vrow

Path(sys.argv[2]).write_bytes(out)
PY
LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_c_api_decoder h265 i420p10 53 \
    "$c_api_h265_10" "$h265_10"
cmp "$c_h265_10_planar" "$c_api_h265_10"

rm -f "$c_mjpeg"
for jpg in "$mjpeg_dir"/*.jpg; do
    raw="/tmp/rk_mpp_c_api_one_jpeg.raw"
    rm -f "$raw"
    LD_LIBRARY_PATH=/usr/local/lib:/usr/lib/aarch64-linux-gnu \
        timeout 60 /usr/local/bin/mpi_dec_test \
        -i "$jpg" -t 8 -w 320 -h 240 -n 1 -o "$raw" -v q \
        < /dev/null > /tmp/rk_mpp_c_api_mjpeg_c.log 2>&1
    cat "$raw" >> "$c_mjpeg"
done
ffmpeg -nostdin -hide_banner -loglevel error -y \
    -f rawvideo -pixel_format nv12 -video_size 320x240 \
    -i "$c_mjpeg" -pix_fmt yuv420p -f rawvideo "$c_mjpeg_i420"

LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_c_api_decoder mjpeg nv12 97 "$c_api_mjpeg" \
    "$mjpeg_dir"/*.jpg
cmp "$c_mjpeg" "$c_api_mjpeg"
LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_c_api_decoder mjpeg nv12 1048576 \
    "$c_api_mjpeg_one_packet" "$mjpeg_dir"/*.jpg
cmp "$c_mjpeg" "$c_api_mjpeg_one_packet"
LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_c_api_decoder mjpeg i420 211 "$c_api_mjpeg_i420" \
    "$mjpeg_dir"/*.jpg
cmp "$c_mjpeg_i420" "$c_api_mjpeg_i420"
sha256sum "$c_api_h264" "$c_h264" \
    "$c_api_h264_tiny" "$c_api_h264_one_packet" \
    "$c_api_h264_i420" "$c_h264_i420" \
    "$c_api_mjpeg" "$c_mjpeg" \
    "$c_api_mjpeg_one_packet" \
    "$c_api_mjpeg_i420" "$c_mjpeg_i420" \
    "$c_api_h265" "$c_h265" \
    "$c_api_h265_one_packet" \
    "$c_api_h265_i420" "$c_h265_i420" \
    "$c_api_h265_10" "$c_h265_10_planar"
printf 'PASS rk_mpp C/C++ API H.264 + H.265 + MJPEG\n'
REMOTE
