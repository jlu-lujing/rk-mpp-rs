#!/usr/bin/env bash

# Run on a Rockchip board. The installed FFmpeg must contain its native
# *_rkmpp wrappers and link librockchip_mpp.so.1.
set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"

export PATH="$HOME/.cargo/bin:$PATH"

ffmpeg_bin=$(command -v ffmpeg)
ffprobe_bin=$(command -v ffprobe)
overlay=$(mktemp -d /tmp/rk-mpp-rs-ffmpeg.XXXXXX)
work=$(mktemp -d /tmp/rk-mpp-rs-ffmpeg-test.XXXXXX)
trap 'rm -rf "$overlay" "$work"' EXIT

cargo build --release --offline --lib
ln -s "$repo/target/release/librk_mpp_rs.so" \
    "$overlay/librockchip_mpp.so.1"

if ! LD_LIBRARY_PATH="$overlay" ldd "$ffmpeg_bin" |
    grep -F "librockchip_mpp.so.1 => $overlay/librockchip_mpp.so.1" >/dev/null; then
    printf 'FFmpeg did not resolve librockchip_mpp.so.1 to the Rust library\n' >&2
    exit 1
fi

if ! "$ffmpeg_bin" -hide_banner -decoders 2>/dev/null |
    grep -F 'h264_rkmpp' >/dev/null; then
    printf 'installed FFmpeg has no RKMPP decoders; rebuild it with --enable-rkmpp\n' >&2
    exit 2
fi
if ! "$ffmpeg_bin" -hide_banner -encoders 2>/dev/null |
    grep -F 'h264_rkmpp' >/dev/null; then
    printf 'installed FFmpeg has no RKMPP encoders; rebuild it with --enable-rkmpp\n' >&2
    exit 2
fi

printf 'FFmpeg: %s\n' "$("$ffmpeg_bin" -hide_banner -version | head -1)"
printf 'MPP:    %s\n' "$overlay/librockchip_mpp.so.1"

fail=0
passed=0
skipped=0

fail_case() {
    printf 'FAIL %-22s %s\n' "$1" "$2" >&2
    fail=1
}

pass_case() {
    printf 'PASS %-22s %s\n' "$1" "$2"
    passed=$((passed + 1))
}

print_stream_mismatch() {
    local vendor=$1
    local ours=$2
    sha256sum "$vendor" "$ours" >&2
    cmp -l "$vendor" "$ours" 2>/dev/null | head -5 >&2 || true
}

decode_exact() {
    local name=$1
    local codec=$2
    local input=$3
    local frames=$4
    local pixel_format=$5
    local vendor="$work/${name}-vendor.yuv"
    local ours="$work/${name}-ours.yuv"
    local vendor_log="$work/${name}-vendor.log"
    local ours_log="$work/${name}-ours.log"

    if [ ! -f "$input" ]; then
        printf 'SKIP %-22s missing %s\n' "$name" "$input"
        skipped=$((skipped + 1))
        return
    fi
    if ! timeout 90 "$ffmpeg_bin" -nostdin -hide_banner -loglevel warning \
        -c:v "${codec}_rkmpp" -i "$input" -frames:v "$frames" \
        -fps_mode passthrough -pix_fmt "$pixel_format" -f rawvideo -y \
        "$vendor" >"$vendor_log" 2>&1; then
        printf 'SKIP %-22s vendor frontend cannot decode this fixture\n' "$name"
        skipped=$((skipped + 1))
        return
    fi
    if ! timeout 90 env LD_LIBRARY_PATH="$overlay" "$ffmpeg_bin" \
        -nostdin -hide_banner -loglevel warning -c:v "${codec}_rkmpp" \
        -i "$input" -frames:v "$frames" -fps_mode passthrough \
        -pix_fmt "$pixel_format" -f rawvideo -y "$ours" >"$ours_log" 2>&1; then
        fail_case "$name decode" "Rust FFmpeg frontend failed"
        tail -20 "$ours_log" >&2
        return
    fi
    if ! cmp -s "$vendor" "$ours"; then
        fail_case "$name decode" "vendor and Rust output differ"
        return
    fi
    pass_case "$name decode" \
        "$(stat -c %s "$ours") bytes, $(sha256sum "$ours" | cut -d' ' -f1)"
}

encode_case() {
    local name=$1
    local codec=$2
    local muxer=$3
    local extension=$4
    local require_exact=$5
    local frames=${6:-12}
    local size=${7:-320x240}
    local gop=${8:-30}
    local vendor="$work/${name}-vendor.$extension"
    local ours="$work/${name}-ours.$extension"
    local cpu="$work/${name}-cpu.$extension"
    local vendor_log="$work/${name}-vendor.log"
    local ours_log="$work/${name}-ours.log"
    local cpu_log="$work/${name}-cpu.log"
    local common=(-nostdin -hide_banner -loglevel warning
        -f lavfi -i "testsrc2=size=$size:rate=30" -frames:v "$frames"
        -pix_fmt nv12 -c:v "${codec}_rkmpp" -rc_mode CQP -qp_init 26
        -g "$gop" -f "$muxer" -y)

    if ! timeout 90 "$ffmpeg_bin" "${common[@]}" "$vendor" \
        >"$vendor_log" 2>&1; then
        printf 'SKIP %-22s vendor encoder failed\n' "$name"
        skipped=$((skipped + 1))
        return
    fi
    if ! timeout 90 env LD_LIBRARY_PATH="$overlay" "$ffmpeg_bin" \
        "${common[@]}" "$ours" >"$ours_log" 2>&1; then
        fail_case "$name encode" "Rust FFmpeg frontend failed"
        tail -20 "$ours_log" >&2
        return
    fi
    if ! timeout 90 env LD_LIBRARY_PATH="$overlay" RKMPP_COMPAT_CPU_INPUT=1 \
        "$ffmpeg_bin" "${common[@]}" "$cpu" >"$cpu_log" 2>&1; then
        fail_case "$name encode" "Rust FFmpeg CPU fallback failed"
        tail -20 "$cpu_log" >&2
        return
    fi
    if ! cmp -s "$ours" "$cpu"; then
        fail_case "$name encode" "DMA-BUF input and CPU fallback differ"
        return
    fi

    local vendor_frames ours_frames
    vendor_frames=$("$ffprobe_bin" -v error -count_frames -select_streams v:0 \
        -show_entries stream=nb_read_frames -of default=nw=1:nk=1 "$vendor")
    ours_frames=$("$ffprobe_bin" -v error -count_frames -select_streams v:0 \
        -show_entries stream=nb_read_frames -of default=nw=1:nk=1 "$ours")
    if [ "$vendor_frames" != "$frames" ] || [ "$ours_frames" != "$frames" ]; then
        fail_case "$name encode" \
            "expected $frames frames, got vendor=$vendor_frames ours=$ours_frames"
        return
    fi
    if [ "$require_exact" = 1 ] && ! cmp -s "$vendor" "$ours"; then
        fail_case "$name encode" "vendor and Rust bitstreams differ"
        print_stream_mismatch "$vendor" "$ours"
        return
    fi

    if cmp -s "$vendor" "$ours"; then
        pass_case "$name encode" \
            "$frames frames, exact $(sha256sum "$ours" | cut -d' ' -f1)"
    else
        pass_case "$name encode" \
            "$frames frames, valid; bitstream differs from this wrapper's vendor run"
    fi
}

h264="$work/input.264"
"$ffmpeg_bin" -nostdin -hide_banner -loglevel error -y \
    -f lavfi -i testsrc2=size=1280x720:rate=30 -frames:v 12 \
    -pix_fmt yuv420p -c:v libx264 -preset veryfast \
    -x264-params "keyint=30:min-keyint=30:bframes=0:scenecut=0:repeat-headers=1" \
    -f h264 "$h264"

decode_exact h264-decode h264 "$h264" 12 nv12
decode_exact hevc-decode hevc tests/data/h265e_c_fixqp_qp26_320x240.265 1 nv12
decode_exact vp9-decode vp9 tests/data/vp9_p0_lossless_320x240_12.ivf 12 nv12

encode_case h264-encode h264 h264 h264 1
encode_case hevc-encode hevc hevc h265 1
encode_case mjpeg-encode mjpeg mjpeg mjpeg 1
encode_case hevc-long-gop hevc hevc h265 1 60 320x240 60
encode_case h264-long-gop h264 h264 h264 1 60 320x240 60

printf 'SUMMARY passed=%d skipped=%d failed=%d\n' \
    "$passed" "$skipped" "$fail"
exit "$fail"
