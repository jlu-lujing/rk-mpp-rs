#!/usr/bin/env bash

# Real C/C++ consumers for the public H.264, H.265 and MJPEG decoder ABI.

set -euo pipefail

board="${BOARD:?set BOARD to a Rockchip SSH alias}"

ssh "$board" 'mkdir -p "$HOME/rk-mpp-rs"'
COPYFILE_DISABLE=1 tar \
    --no-xattrs \
    --exclude .git \
    --exclude target \
    --exclude .worktrees \
    --exclude refs \
    -cf - . |
    ssh "$board" 'tar -xmf - -C "$HOME/rk-mpp-rs"'

ssh "$board" 'bash -s' <<'REMOTE'
set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"
cd "$HOME/rk-mpp-rs"
cargo build --offline --lib --bins
ln -sfn librk_mpp_rs.so target/debug/librk_mpp_rs.so.1

for symbol in \
    rk_mpp_api_version \
    rk_mpp_api_version_string \
    rk_mpp_strerror \
    rk_mpp_decoder_config_init \
    rk_mpp_frame_init \
    rk_mpp_decoder_create \
    rk_mpp_decoder_submit_packet \
    rk_mpp_decoder_receive_frame \
    rk_mpp_decoder_export_frame_dmabuf \
    rk_mpp_frame_release \
    rk_mpp_decoder_flush \
    rk_mpp_decoder_reset \
    rk_mpp_decoder_recover \
    rk_mpp_decoder_status \
    rk_mpp_decoder_close \
    rk_mpp_encoder_config_init \
    rk_mpp_encoder_advanced_config_v3_init \
    rk_mpp_encoder_h264_deblock_init \
    rk_mpp_encoder_h264_syntax_init \
    rk_mpp_encoder_h265_syntax_init \
    rk_mpp_encoded_packet_init \
    rk_mpp_encoder_create \
    rk_mpp_encoder_create_advanced_v3 \
    rk_mpp_encoder_submit_frame \
    rk_mpp_encoder_submit_frame_dmabuf \
    rk_mpp_encoder_receive_packet \
    rk_mpp_encoded_packet_release \
    rk_mpp_encoder_flush \
    rk_mpp_encoder_reset \
    rk_mpp_encoder_close \
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
gcc -std=c11 -Wall -Wextra -Werror \
    -Iinclude tests/c_api_ivf.c \
    -Ltarget/debug -lrk_mpp_rs -Wl,-rpath,"$PWD/target/debug" \
    -o /tmp/rk_mpp_c_api_ivf
gcc -std=c11 -Wall -Wextra -Werror \
    -Iinclude tests/c_api_encoder.c \
    -Ltarget/debug -lrk_mpp_rs -Wl,-rpath,"$PWD/target/debug" \
    -o /tmp/rk_mpp_c_api_encoder
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
h264_raw=/tmp/rk_mpp_c_api_encoder.nv12
h264_raw_i420=/tmp/rk_mpp_c_api_encoder.i420
h264_api=/tmp/rk_mpp_c_api_encoder.264
h264_api_dmabuf=/tmp/rk_mpp_c_api_encoder_dmabuf.264
h264_api_i420=/tmp/rk_mpp_c_api_encoder_i420.264
h264_cli=/tmp/rk_mpp_c_api_encoder_cli.264
h265_api=/tmp/rk_mpp_c_api_encoder.265
h265_api_dmabuf=/tmp/rk_mpp_c_api_encoder_dmabuf.265
h265_api_i420=/tmp/rk_mpp_c_api_encoder_i420.265
h265_cli=/tmp/rk_mpp_c_api_encoder_cli.265
jpeg_api=/tmp/rk_mpp_c_api_encoder.jpg
jpeg_api_dmabuf=/tmp/rk_mpp_c_api_encoder_dmabuf.jpg
jpeg_api_i420=/tmp/rk_mpp_c_api_encoder_i420.jpg
jpeg_cli=/tmp/rk_mpp_c_api_encoder_cli.jpg
jpeg_restart_api=/tmp/rk_mpp_c_api_encoder_restart.jpg
jpeg_restart_cli=/tmp/rk_mpp_c_api_encoder_restart_cli.jpg
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
mpeg2=tests/data/m2v_320x240_12.m2v
mpeg4=tests/data/mp4_320x240_12_ip.m4v
h263=tests/data/h263_cif_12_ip.263
legacy_ok=0
c_mpeg2=/tmp/rk_mpp_c_api_mpeg2_c.nv12
c_api_mpeg2=/tmp/rk_mpp_c_api_mpeg2.nv12
c_mpeg2_i420=/tmp/rk_mpp_c_api_mpeg2_c.i420
c_api_mpeg2_i420=/tmp/rk_mpp_c_api_mpeg2.i420
c_mpeg4=/tmp/rk_mpp_c_api_mpeg4_c.nv12
c_api_mpeg4=/tmp/rk_mpp_c_api_mpeg4.nv12
c_mpeg4_i420=/tmp/rk_mpp_c_api_mpeg4_c.i420
c_api_mpeg4_i420=/tmp/rk_mpp_c_api_mpeg4.i420
c_h263=/tmp/rk_mpp_c_api_h263_c.nv12
c_api_h263=/tmp/rk_mpp_c_api_h263.nv12
c_h263_i420=/tmp/rk_mpp_c_api_h263_c.i420
c_api_h263_i420=/tmp/rk_mpp_c_api_h263.i420
vp9_p0=tests/data/vp9_p0_lossless_320x240_12.ivf
vp9_p2=tests/data/vp9_p2_10bit_lossless_320x240_12.ivf
c_vp9_p0=/tmp/rk_mpp_c_api_vp9_p0_c.nv12
c_api_vp9_p0=/tmp/rk_mpp_c_api_vp9_p0.nv12
c_vp9_p0_i420=/tmp/rk_mpp_c_api_vp9_p0_c.i420
c_api_vp9_p0_i420=/tmp/rk_mpp_c_api_vp9_p0.i420
c_vp9_p2_packed=/tmp/rk_mpp_c_api_vp9_p2_c.packed
c_vp9_p2_planar=/tmp/rk_mpp_c_api_vp9_p2_c.i420p10
c_api_vp9_p2=/tmp/rk_mpp_c_api_vp9_p2.i420p10
av1_p0=tests/data/av1_inter_box_320x240.ivf
av1_p2=tests/data/film_grain_10bit.ivf
c_av1_p0=/tmp/rk_mpp_c_api_av1_p0_c.nv12
c_api_av1_p0=/tmp/rk_mpp_c_api_av1_p0.nv12
c_av1_p0_i420=/tmp/rk_mpp_c_api_av1_p0_c.i420
c_api_av1_p0_i420=/tmp/rk_mpp_c_api_av1_p0.i420
c_av1_p2_packed=/tmp/rk_mpp_c_api_av1_p2_c.packed
c_av1_p2_planar=/tmp/rk_mpp_c_api_av1_p2_c.i420p10
rust_av1_p2=/tmp/rk_mpp_c_api_av1_p2_rust_oracle.i420p10
c_api_av1_p2=/tmp/rk_mpp_c_api_av1_p2.i420p10
vp8=tests/data/vp8_320x240_12_altref.ivf
vp8_ok=0
c_vp8=/tmp/rk_mpp_c_api_vp8_c.nv12
c_api_vp8=/tmp/rk_mpp_c_api_vp8.nv12
c_vp8_i420=/tmp/rk_mpp_c_api_vp8_c.i420
c_api_vp8_i420=/tmp/rk_mpp_c_api_vp8.i420
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
    -f lavfi -i "testsrc2=size=320x240:rate=30" -frames:v 6 \
    -pix_fmt nv12 -f rawvideo "$h264_raw"
ffmpeg -nostdin -y -hide_banner -loglevel error \
    -f rawvideo -pixel_format nv12 -video_size 320x240 \
    -i "$h264_raw" -pix_fmt yuv420p -f rawvideo "$h264_raw_i420"
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

REQUIRE_DMABUF=1 LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
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

if ./target/debug/rk-mpp-rs h264-enc "$h264_raw" "$h264_cli" \
    320 240 26 baseline 6 6 > /tmp/rk_mpp_c_api_encoder_cli.log 2>&1; then
    :
else
    ./target/debug/rk-mpp-rs h264e510-gop6 "$h264_raw" "$h264_cli" \
        > /tmp/rk_mpp_c_api_encoder_cli.log 2>&1
fi
LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_c_api_encoder h264 nv12 "$h264_api" 320 240 26 6 6 \
    "$h264_raw"
cmp "$h264_cli" "$h264_api"
DMABUF_INPUT=1 LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_c_api_encoder h264 nv12 "$h264_api_dmabuf" 320 240 26 6 6 \
    "$h264_raw"
cmp "$h264_api" "$h264_api_dmabuf"
LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_c_api_encoder h264 i420 "$h264_api_i420" 320 240 26 6 6 \
    "$h264_raw_i420"
cmp "$h264_cli" "$h264_api_i420"

if H265_GOP=6 ./target/debug/rk-mpp-rs h265-gop "$h264_raw" "$h265_cli" \
    320 240 6 26 > /tmp/rk_mpp_c_api_encoder_h265_cli.log 2>&1; then
    :
else
    ./target/debug/rk-mpp-rs h265e510-gop6 "$h264_raw" "$h265_cli" \
        > /tmp/rk_mpp_c_api_encoder_h265_cli.log 2>&1
fi
LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_c_api_encoder h265 nv12 "$h265_api" 320 240 26 6 6 \
    "$h264_raw"
cmp "$h265_cli" "$h265_api"
DMABUF_INPUT=1 LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_c_api_encoder h265 nv12 "$h265_api_dmabuf" 320 240 26 6 6 \
    "$h264_raw"
cmp "$h265_api" "$h265_api_dmabuf"
LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_c_api_encoder h265 i420 "$h265_api_i420" 320 240 26 6 6 \
    "$h264_raw_i420"
cmp "$h265_cli" "$h265_api_i420"

./target/debug/rk-mpp-rs jpeg-enc "$h264_raw" "$jpeg_cli" 320 240 10 \
    > /tmp/rk_mpp_c_api_encoder_jpeg_cli.log 2>&1
LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_c_api_encoder jpeg nv12 "$jpeg_api" 320 240 10 1 1 \
    "$h264_raw"
cmp "$jpeg_cli" "$jpeg_api"
DMABUF_INPUT=1 LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_c_api_encoder jpeg nv12 "$jpeg_api_dmabuf" 320 240 10 1 1 \
    "$h264_raw"
cmp "$jpeg_api" "$jpeg_api_dmabuf"
LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_c_api_encoder jpeg i420 "$jpeg_api_i420" 320 240 10 1 1 \
    "$h264_raw_i420"
cmp "$jpeg_cli" "$jpeg_api_i420"
./target/debug/rk-mpp-rs jpeg-enc "$h264_raw" "$jpeg_restart_cli" \
    320 240 10 420 nv12 rst20 \
    > /tmp/rk_mpp_c_api_encoder_jpeg_restart_cli.log 2>&1
JPEG_RESTART_MCUS=20 \
LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_c_api_encoder jpeg nv12 "$jpeg_restart_api" 320 240 10 1 1 \
    "$h264_raw"
cmp "$jpeg_restart_cli" "$jpeg_restart_api"

rm -f "$c_h265"
LD_LIBRARY_PATH=/usr/local/lib:/usr/lib/aarch64-linux-gnu \
    timeout 120 /usr/local/bin/mpi_dec_test \
    -i "$h265" -t 16777220 -w 320 -h 240 -n 4 -o "$c_h265" -v q \
    < /dev/null > /tmp/rk_mpp_c_api_h265_c.log 2>&1
ffmpeg -nostdin -hide_banner -loglevel error -y \
    -f rawvideo -pixel_format nv12 -video_size 320x240 \
    -i "$c_h265" -pix_fmt yuv420p -f rawvideo "$c_h265_i420"

REQUIRE_DMABUF=1 LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_c_api_decoder h265 nv12 37 "$c_api_h265" "$h265"
cmp "$c_h265" "$c_api_h265"
EXPECT_NO_DMABUF=1 LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
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

rm -f "$c_mpeg2"
if [ ! -x /tmp/mpp-develop/build/test/mpi_dec_test ] ||
    [ ! -f /tmp/mpp-develop/build/mpp/librockchip_mpp.so.0 ]; then
    printf 'SKIP MPEG-2/MPEG-4/H.263 (legacy C MPP oracle unavailable)\n'
elif LD_LIBRARY_PATH=/tmp/mpp-develop/build/mpp \
    timeout 120 /tmp/mpp-develop/build/test/mpi_dec_test \
    -i "$mpeg2" -t 2 -w 320 -h 240 -n 10 -o "$c_mpeg2" -v q \
    < /dev/null > /tmp/rk_mpp_c_api_mpeg2_c.log 2>&1; then
    ffmpeg -nostdin -hide_banner -loglevel error -y \
        -f rawvideo -pixel_format nv12 -video_size 320x240 \
        -i "$c_mpeg2" -pix_fmt yuv420p -f rawvideo "$c_mpeg2_i420"
    LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
        /tmp/rk_mpp_c_api_decoder mpeg2 nv12 113 "$c_api_mpeg2" "$mpeg2"
    cmp "$c_mpeg2" "$c_api_mpeg2"
    LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
        /tmp/rk_mpp_c_api_decoder mpeg2 i420 211 "$c_api_mpeg2_i420" "$mpeg2"
    cmp "$c_mpeg2_i420" "$c_api_mpeg2_i420"

    rm -f "$c_mpeg4"
    LD_LIBRARY_PATH=/tmp/mpp-develop/build/mpp \
        timeout 120 /tmp/mpp-develop/build/test/mpi_dec_test \
        -i "$mpeg4" -t 4 -w 320 -h 240 -n 12 -o "$c_mpeg4" -v q \
        < /dev/null > /tmp/rk_mpp_c_api_mpeg4_c.log 2>&1
    ffmpeg -nostdin -hide_banner -loglevel error -y \
        -f rawvideo -pixel_format nv12 -video_size 320x240 \
        -i "$c_mpeg4" -pix_fmt yuv420p -f rawvideo "$c_mpeg4_i420"
    LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
        /tmp/rk_mpp_c_api_decoder mpeg4 nv12 127 "$c_api_mpeg4" "$mpeg4"
    cmp "$c_mpeg4" "$c_api_mpeg4"
    LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
        /tmp/rk_mpp_c_api_decoder mpeg4 i420 191 "$c_api_mpeg4_i420" "$mpeg4"
    cmp "$c_mpeg4_i420" "$c_api_mpeg4_i420"

    rm -f "$c_h263"
    LD_LIBRARY_PATH=/tmp/mpp-develop/build/mpp \
        timeout 120 /tmp/mpp-develop/build/test/mpi_dec_test \
        -i "$h263" -t 3 -w 352 -h 288 -n 12 -o "$c_h263" -v q \
        < /dev/null > /tmp/rk_mpp_c_api_h263_c.log 2>&1
    ffmpeg -nostdin -hide_banner -loglevel error -y \
        -f rawvideo -pixel_format nv12 -video_size 352x288 \
        -i "$c_h263" -pix_fmt yuv420p -f rawvideo "$c_h263_i420"
    LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
        /tmp/rk_mpp_c_api_decoder h263 nv12 97 "$c_api_h263" "$h263"
    cmp "$c_h263" "$c_api_h263"
    LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
        /tmp/rk_mpp_c_api_decoder h263 i420 173 "$c_api_h263_i420" "$h263"
    cmp "$c_h263_i420" "$c_api_h263_i420"
    legacy_ok=1
elif grep -q "unable to create dec .* unsupported" \
    /tmp/rk_mpp_c_api_mpeg2_c.log; then
    printf 'SKIP MPEG-2/MPEG-4/H.263 (this SoC has no VDPU2 decoder)\n'
else
    cat /tmp/rk_mpp_c_api_mpeg2_c.log >&2
    exit 1
fi

rm -f "$c_vp9_p0"
LD_LIBRARY_PATH=/usr/local/lib:/usr/lib/aarch64-linux-gnu \
    timeout 120 /usr/local/bin/mpi_dec_test \
    -i "$vp9_p0" -t 10 -w 320 -h 240 -n 12 -o "$c_vp9_p0" -v q \
    < /dev/null > /tmp/rk_mpp_c_api_vp9_p0_c.log 2>&1
ffmpeg -nostdin -hide_banner -loglevel error -y \
    -f rawvideo -pixel_format nv12 -video_size 320x240 \
    -i "$c_vp9_p0" -pix_fmt yuv420p -f rawvideo "$c_vp9_p0_i420"

REQUIRE_DMABUF=1 LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_c_api_ivf vp9 nv12 "$c_api_vp9_p0" "$vp9_p0"
cmp "$c_vp9_p0" "$c_api_vp9_p0"
EXPECT_NO_DMABUF=1 LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_c_api_ivf vp9 i420 "$c_api_vp9_p0_i420" "$vp9_p0"
cmp "$c_vp9_p0_i420" "$c_api_vp9_p0_i420"

rm -f "$c_vp9_p2_packed" "$c_vp9_p2_planar"
LD_LIBRARY_PATH=/usr/local/lib:/usr/lib/aarch64-linux-gnu \
    timeout 120 /usr/local/bin/mpi_dec_test \
    -i "$vp9_p2" -t 10 -w 320 -h 240 -n 12 \
    -o "$c_vp9_p2_packed" -v q \
    < /dev/null > /tmp/rk_mpp_c_api_vp9_p2_c.log 2>&1
python3 - "$c_vp9_p2_packed" "$c_vp9_p2_planar" <<'PY'
from pathlib import Path
import sys

width, height, frames = 320, 240, 12
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
    /tmp/rk_mpp_c_api_ivf vp9 i420p10 "$c_api_vp9_p2" "$vp9_p2"
cmp "$c_vp9_p2_planar" "$c_api_vp9_p2"

rm -f "$c_av1_p0"
LD_LIBRARY_PATH=/usr/local/lib:/usr/lib/aarch64-linux-gnu \
    timeout 120 /usr/local/bin/mpi_dec_test \
    -i "$av1_p0" -t 16777224 -w 320 -h 240 -n 2 -o "$c_av1_p0" -v q \
    < /dev/null > /tmp/rk_mpp_c_api_av1_p0_c.log 2>&1
ffmpeg -nostdin -hide_banner -loglevel error -y \
    -f rawvideo -pixel_format nv12 -video_size 320x240 \
    -i "$c_av1_p0" -pix_fmt yuv420p -f rawvideo "$c_av1_p0_i420"

REQUIRE_DMABUF=1 LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_c_api_ivf av1 nv12 "$c_api_av1_p0" "$av1_p0"
cmp "$c_av1_p0" "$c_api_av1_p0"
EXPECT_NO_DMABUF=1 LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_c_api_ivf av1 i420 "$c_api_av1_p0_i420" "$av1_p0"
cmp "$c_av1_p0_i420" "$c_api_av1_p0_i420"

rm -f "$c_av1_p2_packed" "$c_av1_p2_planar"
LD_LIBRARY_PATH=/usr/local/lib:/usr/lib/aarch64-linux-gnu \
    timeout 120 /usr/local/bin/mpi_dec_test \
    -i "$av1_p2" -t 16777224 -w 320 -h 240 -n 10 \
    -o "$c_av1_p2_packed" -v q \
    < /dev/null > /tmp/rk_mpp_c_api_av1_p2_c.log 2>&1
python3 - "$c_av1_p2_packed" "$c_av1_p2_planar" <<'PY'
from pathlib import Path
import sys

width, height, frames = 320, 240, 10
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
    /tmp/rk_mpp_c_api_ivf av1 i420p10 "$c_api_av1_p2" "$av1_p2"
rm -f "$rust_av1_p2"
if ./target/debug/rk-mpp-rs av1dec "$av1_p2" "$rust_av1_p2" \
    > /tmp/rk_mpp_c_api_av1_p2_rust.log 2>&1; then
    cmp "$rust_av1_p2" "$c_api_av1_p2"
else
    cmp "$c_av1_p2_planar" "$c_api_av1_p2"
fi

rm -f "$c_vp8"
if LD_LIBRARY_PATH=/usr/local/lib:/usr/lib/aarch64-linux-gnu \
    timeout 120 /usr/local/bin/mpi_dec_test \
    -i "$vp8" -t 9 -w 320 -h 240 -n 12 -o "$c_vp8" -v q \
    < /dev/null > /tmp/rk_mpp_c_api_vp8_c.log 2>&1; then
    ffmpeg -nostdin -hide_banner -loglevel error -y \
        -f rawvideo -pixel_format nv12 -video_size 320x240 \
        -i "$c_vp8" -pix_fmt yuv420p -f rawvideo "$c_vp8_i420"
    LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
        /tmp/rk_mpp_c_api_ivf vp8 nv12 "$c_api_vp8" "$vp8"
    cmp "$c_vp8" "$c_api_vp8"
    LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
        /tmp/rk_mpp_c_api_ivf vp8 i420 "$c_api_vp8_i420" "$vp8"
    cmp "$c_vp8_i420" "$c_api_vp8_i420"
    vp8_ok=1
elif grep -q "unable to create dec vp8 .* unsupported" \
    /tmp/rk_mpp_c_api_vp8_c.log; then
    printf 'SKIP VP8 (this SoC has no VP8 decoder)\n'
else
    cat /tmp/rk_mpp_c_api_vp8_c.log >&2
    exit 1
fi

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

REQUIRE_DMABUF=1 LD_LIBRARY_PATH="$PWD/target/debug:${LD_LIBRARY_PATH:-}" \
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
    "$c_api_h265_10" "$c_h265_10_planar" \
    "$c_api_vp9_p0" "$c_vp9_p0" \
    "$c_api_vp9_p0_i420" "$c_vp9_p0_i420" \
    "$c_api_vp9_p2" "$c_vp9_p2_planar" \
    "$c_api_av1_p0" "$c_av1_p0" \
    "$c_api_av1_p0_i420" "$c_av1_p0_i420" \
    "$c_api_av1_p2" "$c_av1_p2_planar" \
    "$h264_api" "$h264_api_i420" "$h264_cli" \
    "$h265_api" "$h265_api_i420" "$h265_cli" \
    "$jpeg_api" "$jpeg_api_i420" "$jpeg_cli" \
    "$jpeg_restart_api" "$jpeg_restart_cli"
if [ "$vp8_ok" -eq 1 ]; then
    sha256sum "$c_api_vp8" "$c_vp8" \
        "$c_api_vp8_i420" "$c_vp8_i420"
fi
if [ "$legacy_ok" -eq 1 ]; then
    sha256sum "$c_api_mpeg2" "$c_mpeg2" \
        "$c_api_mpeg2_i420" "$c_mpeg2_i420" \
        "$c_api_mpeg4" "$c_mpeg4" \
        "$c_api_mpeg4_i420" "$c_mpeg4_i420" \
        "$c_api_h263" "$c_h263" \
        "$c_api_h263_i420" "$c_h263_i420"
fi
printf 'PASS rk_mpp C/C++ decoder + encoder API\n'
REMOTE
