#!/usr/bin/env bash

set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"

export PATH="$HOME/.cargo/bin:$PATH"
if [ "${SKIP_BUILD:-0}" != 1 ]; then
    cargo build --release --offline --lib
    cargo test --release --offline --test dma_heap_import_hw -- --ignored
fi

library="$repo/target/release/librk_mpp_rs.so"
libdir=$(mktemp -d)
jpeg_dir=$(mktemp -d)
trap 'rm -rf "$libdir" "$jpeg_dir"' EXIT
ln -sfn "$library" "$libdir/librk_mpp_rs.so"
ln -sfn "$library" "$libdir/librk_mpp_rs.so.1"

gcc -std=c11 -Wall -Wextra -Werror \
    -I"$repo/include" "$repo/tests/c_api_decoder.c" \
    -L"$libdir" -lrk_mpp_rs \
    -Wl,-rpath,"$libdir" \
    -o /tmp/rk_mpp_decoder_dmabuf
gcc -std=c11 -Wall -Wextra -Werror \
    -I"$repo/include" "$repo/tests/c_api_ivf.c" \
    -L"$libdir" -lrk_mpp_rs \
    -Wl,-rpath,"$libdir" \
    -o /tmp/rk_mpp_ivf_dmabuf

ffmpeg -nostdin -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=320x240:rate=30" -frames:v 4 \
    -pix_fmt yuvj420p -strict unofficial -c:v mjpeg -q:v 3 \
    "$jpeg_dir/frame_%02d.jpg"

REQUIRE_DMABUF=1 \
LD_LIBRARY_PATH="$libdir:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_decoder_dmabuf mjpeg nv12 97 \
        /tmp/rk_mpp_sdk_dmabuf.nv12 "$jpeg_dir"/*.jpg

test -s /tmp/rk_mpp_sdk_dmabuf.nv12
EXPECT_NO_DMABUF=1 \
LD_LIBRARY_PATH="$libdir:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_decoder_dmabuf mjpeg nv12 211 \
        /tmp/rk_mpp_sdk_cpu_frame.nv12 "$jpeg_dir"/*.jpg
test -s /tmp/rk_mpp_sdk_cpu_frame.nv12

h264=/tmp/rk_mpp_sdk_dmabuf.264
ffmpeg -nostdin -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=320x240:rate=30" -frames:v 6 \
    -pix_fmt yuv420p -c:v libx264 -preset veryfast \
    -x264-params "keyint=6:min-keyint=6:scenecut=0:repeat-headers=1" \
    -f h264 "$h264"

REQUIRE_DMABUF=1 \
LD_LIBRARY_PATH="$libdir:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_decoder_dmabuf h264 nv12 313 \
        /tmp/rk_mpp_sdk_h264_dmabuf.nv12 "$h264"
EXPECT_NO_DMABUF=1 \
LD_LIBRARY_PATH="$libdir:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_decoder_dmabuf h264 nv12 211 \
        /tmp/rk_mpp_sdk_h264_cpu.nv12 "$h264"
cmp /tmp/rk_mpp_sdk_h264_dmabuf.nv12 /tmp/rk_mpp_sdk_h264_cpu.nv12

h265=/tmp/rk_mpp_sdk_dmabuf.265
cp tests/data/h265e_c_fixqp_qp26_320x240.265 "$h265"
for _ in 1 2 3; do
    cat tests/data/h265e_c_fixqp_qp26_320x240.265 >> "$h265"
done
REQUIRE_DMABUF=1 \
LD_LIBRARY_PATH="$libdir:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_decoder_dmabuf h265 nv12 313 \
        /tmp/rk_mpp_sdk_h265_dmabuf.nv12 "$h265"
EXPECT_NO_DMABUF=1 \
LD_LIBRARY_PATH="$libdir:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_decoder_dmabuf h265 nv12 211 \
        /tmp/rk_mpp_sdk_h265_cpu.nv12 "$h265"
cmp /tmp/rk_mpp_sdk_h265_dmabuf.nv12 /tmp/rk_mpp_sdk_h265_cpu.nv12

REQUIRE_DMABUF=1 \
LD_LIBRARY_PATH="$libdir:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_ivf_dmabuf vp9 nv12 \
        /tmp/rk_mpp_sdk_vp9_dmabuf.nv12 \
        tests/data/vp9_p0_lossless_320x240_12.ivf
EXPECT_NO_DMABUF=1 \
LD_LIBRARY_PATH="$libdir:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_ivf_dmabuf vp9 nv12 \
        /tmp/rk_mpp_sdk_vp9_cpu.nv12 \
        tests/data/vp9_p0_lossless_320x240_12.ivf
cmp /tmp/rk_mpp_sdk_vp9_dmabuf.nv12 /tmp/rk_mpp_sdk_vp9_cpu.nv12

REQUIRE_DMABUF=1 \
LD_LIBRARY_PATH="$libdir:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_ivf_dmabuf av1 nv12 \
        /tmp/rk_mpp_sdk_av1_dmabuf.nv12 \
        tests/data/av1_inter_box_320x240.ivf
EXPECT_NO_DMABUF=1 \
LD_LIBRARY_PATH="$libdir:${LD_LIBRARY_PATH:-}" \
    /tmp/rk_mpp_ivf_dmabuf av1 nv12 \
        /tmp/rk_mpp_sdk_av1_cpu.nv12 \
        tests/data/av1_inter_box_320x240.ivf
cmp /tmp/rk_mpp_sdk_av1_dmabuf.nv12 /tmp/rk_mpp_sdk_av1_cpu.nv12

if [ "${RUN_4K:-0}" = 1 ]; then
    width=3840
    height=2160
    frames=3
    frame_bytes=$((width * height * 3 / 2))
    ffmpeg_base=(
        -nostdin -y -hide_banner -loglevel error
        -f lavfi -i "testsrc2=size=${width}x${height}:rate=30"
        -frames:v "$frames" -pix_fmt yuv420p
    )
    ffmpeg "${ffmpeg_base[@]}" -c:v libx264 -preset ultrafast \
        -x264-params \
        "keyint=30:min-keyint=30:bframes=0:scenecut=0:repeat-headers=1" \
        -f h264 /tmp/rk_mpp_sdk_dmabuf_4k.264
    ffmpeg "${ffmpeg_base[@]}" -c:v libx265 -preset ultrafast -profile:v main \
        -x265-params \
        "log-level=error:keyint=30:min-keyint=30:bframes=0:scenecut=0:repeat-headers=1" \
        -f hevc /tmp/rk_mpp_sdk_dmabuf_4k.265

    for codec in h264 h265; do
        input="/tmp/rk_mpp_sdk_dmabuf_4k.$([ "$codec" = h264 ] && echo 264 || echo 265)"
        REQUIRE_DMABUF=1 LD_LIBRARY_PATH="$libdir:${LD_LIBRARY_PATH:-}" \
            /tmp/rk_mpp_decoder_dmabuf "$codec" nv12 4096 \
                "/tmp/rk_mpp_sdk_${codec}_4k_dmabuf.nv12" "$input"
        EXPECT_NO_DMABUF=1 LD_LIBRARY_PATH="$libdir:${LD_LIBRARY_PATH:-}" \
            /tmp/rk_mpp_decoder_dmabuf "$codec" nv12 4096 \
                "/tmp/rk_mpp_sdk_${codec}_4k_cpu.nv12" "$input"
        test "$(stat -c %s "/tmp/rk_mpp_sdk_${codec}_4k_dmabuf.nv12")" \
            -eq $((frame_bytes * frames))
        cmp "/tmp/rk_mpp_sdk_${codec}_4k_dmabuf.nv12" \
            "/tmp/rk_mpp_sdk_${codec}_4k_cpu.nv12"
    done

    ffmpeg -nostdin -y -hide_banner -loglevel error \
        -f lavfi -i "testsrc2=size=${width}x${height}:rate=30" \
        -frames:v "$frames" -pix_fmt yuv420p -f rawvideo \
        /tmp/rk_mpp_sdk_dmabuf_4k.yuv
    vpxenc --quiet --codec=vp9 --profile=0 --passes=1 --good --cpu-used=8 \
        --threads=4 --lag-in-frames=0 --kf-max-dist=30 --limit="$frames" \
        --width="$width" --height="$height" --fps=30/1 --ivf \
        -o /tmp/rk_mpp_sdk_dmabuf_4k.vp9.ivf \
        /tmp/rk_mpp_sdk_dmabuf_4k.yuv
    aomenc --quiet --limit="$frames" --width="$width" --height="$height" \
        --fps=30/1 --cpu-used=8 --passes=1 --end-usage=q --cq-level=32 \
        --profile=0 --ivf -o /tmp/rk_mpp_sdk_dmabuf_4k.av1.ivf \
        /tmp/rk_mpp_sdk_dmabuf_4k.yuv

    for codec in vp9 av1; do
        extension=$([ "$codec" = vp9 ] && echo "vp9.ivf" || echo "av1.ivf")
        input="/tmp/rk_mpp_sdk_dmabuf_4k.$extension"
        REQUIRE_DMABUF=1 LD_LIBRARY_PATH="$libdir:${LD_LIBRARY_PATH:-}" \
            /tmp/rk_mpp_ivf_dmabuf "$codec" nv12 \
                "/tmp/rk_mpp_sdk_${codec}_4k_dmabuf.nv12" "$input"
        EXPECT_NO_DMABUF=1 LD_LIBRARY_PATH="$libdir:${LD_LIBRARY_PATH:-}" \
            /tmp/rk_mpp_ivf_dmabuf "$codec" nv12 \
                "/tmp/rk_mpp_sdk_${codec}_4k_cpu.nv12" "$input"
        test "$(stat -c %s "/tmp/rk_mpp_sdk_${codec}_4k_dmabuf.nv12")" \
            -eq $((frame_bytes * frames))
        cmp "/tmp/rk_mpp_sdk_${codec}_4k_dmabuf.nv12" \
            "/tmp/rk_mpp_sdk_${codec}_4k_cpu.nv12"
    done
fi
printf 'PASS rk-mpp-rs DMA-BUF import and H.264/H.265/VP9/AV1/MJPEG frame export\n'
