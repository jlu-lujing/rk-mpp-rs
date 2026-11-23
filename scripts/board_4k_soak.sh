#!/usr/bin/env bash
# RK3588 4K long-GOP/DPB gate. Raw decode streams are hashed through pipes so
# 30-frame 4K checks do not require hundreds of megabytes of temporary output.
set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"

export PATH="$HOME/.cargo/bin:$PATH"
target_dir="${CARGO_TARGET_DIR:-$repo/target}"
rust_lib="${RUST_LIB:-$target_dir/release/librk_mpp_rs.so}"
overlay=$(mktemp -d /tmp/rk-mpp-rs-4k-soak-overlay.XXXXXX)
work=$(mktemp -d /tmp/rk-mpp-rs-4k-soak.XXXXXX)
trap 'rm -rf "$overlay" "$work"' EXIT

width=3840
height=2160
frames=30
passed=0
failed=0

die() {
    printf 'ERROR: %s\n' "$*" >&2
    exit 2
}

pass_case() {
    printf 'PASS %-24s %s\n' "$1" "$2"
    passed=$((passed + 1))
}

fail_case() {
    printf 'FAIL %-24s %s\n' "$1" "$2" >&2
    failed=$((failed + 1))
}

command -v ffmpeg >/dev/null || die "missing ffmpeg"
command -v ffprobe >/dev/null || die "missing ffprobe"
command -v sha256sum >/dev/null || die "missing sha256sum"
command -v vpxenc >/dev/null || die "missing vpxenc"

if [ "${SKIP_BUILD:-0}" != 1 ]; then
    cargo build --release --offline --lib
fi
ln -s "$rust_lib" "$overlay/librockchip_mpp.so.1"
if ! LD_LIBRARY_PATH="$overlay" ldd "$(command -v ffmpeg)" |
    grep -F "librockchip_mpp.so.1 => $overlay/librockchip_mpp.so.1" >/dev/null; then
    die "FFmpeg did not resolve librockchip_mpp.so.1 to the Rust overlay"
fi

ffmpeg_base=(
    -nostdin -hide_banner -loglevel error
    -f lavfi -i "testsrc2=size=${width}x${height}:rate=30"
    -frames:v "$frames" -pix_fmt yuv420p
)

ffmpeg "${ffmpeg_base[@]}" -c:v libx264 -preset veryfast \
    -x264-params "keyint=30:min-keyint=30:bframes=3:scenecut=0:repeat-headers=1" \
    -f h264 "$work/h264-30.264"
ffmpeg "${ffmpeg_base[@]}" -c:v libx265 -preset ultrafast -profile:v main \
    -x265-params \
    "log-level=error:keyint=30:min-keyint=30:bframes=0:scenecut=0:repeat-headers=1" \
    -f hevc "$work/hevc-30.265"
ffmpeg -nostdin -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=${width}x${height}:rate=30" \
    -frames:v "$frames" -pix_fmt yuv420p -f rawvideo "$work/vp9-30.yuv"
vpxenc --quiet --codec=vp9 --profile=0 --passes=1 --good --cpu-used=8 \
    --threads=4 --lag-in-frames=0 --kf-max-dist=30 --limit="$frames" \
    --width="$width" --height="$height" --fps=30/1 --ivf \
    -o "$work/vp9-30.ivf" "$work/vp9-30.yuv"
rm -f "$work/vp9-30.yuv"

ffmpeg -nostdin -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=${width}x${height}:rate=30" \
    -frames:v 10 -pix_fmt yuv420p10le \
    -c:v libx265 -preset ultrafast -profile:v main10 \
    -x265-params \
    "log-level=error:keyint=30:min-keyint=30:bframes=0:scenecut=0:repeat-headers=1" \
    -f hevc "$work/hevc-10-10.265"

decode_hash() {
    local codec="$1"
    local input="$2"
    local count="$3"
    local pixel_format="$4"
    ffmpeg -nostdin -hide_banner -loglevel error -c:v "$codec" \
        -i "$input" -frames:v "$count" -fps_mode passthrough \
        -pix_fmt "$pixel_format" -f rawvideo - |
        sha256sum | cut -d' ' -f1
}

decode_case() {
    local name="$1"
    local codec="$2"
    local software="$3"
    local input="$4"
    local count="$5"
    local pixel_format="$6"
    local vendor_hash rust_hash software_hash

    if ! vendor_hash=$(decode_hash "${codec}_rkmpp" "$input" "$count" "$pixel_format"); then
        fail_case "$name" "vendor FFmpeg decode failed"
        return
    fi
    if ! rust_hash=$(LD_LIBRARY_PATH="$overlay" \
        decode_hash "${codec}_rkmpp" "$input" "$count" "$pixel_format"); then
        fail_case "$name" "Rust FFmpeg decode failed"
        return
    fi
    if ! software_hash=$(decode_hash "$software" "$input" "$count" "$pixel_format"); then
        fail_case "$name" "software FFmpeg decode failed"
        return
    fi
    if [ "$vendor_hash" != "$rust_hash" ] || [ "$rust_hash" != "$software_hash" ]; then
        fail_case "$name" \
            "hash mismatch vendor=$vendor_hash rust=$rust_hash software=$software_hash"
        return
    fi
    pass_case "$name" "$count frames, C/Rust/software exact $rust_hash"
}

encode_case() {
    local name="$1"
    local codec="$2"
    local muxer="$3"
    local extension="$4"
    local vendor="$work/${name}-vendor.$extension"
    local rust="$work/${name}-rust.$extension"
    local common=(
        -nostdin -hide_banner -loglevel error
        -f lavfi -i "testsrc2=size=${width}x${height}:rate=30"
        -frames:v "$frames" -pix_fmt nv12
        -c:v "${codec}_rkmpp" -rc_mode CQP -qp_init 26 -g 60
        -f "$muxer" -y
    )

    if ! timeout 180 ffmpeg "${common[@]}" "$vendor"; then
        fail_case "$name" "vendor FFmpeg encode failed"
        return
    fi
    if ! timeout 180 env LD_LIBRARY_PATH="$overlay" ffmpeg "${common[@]}" "$rust"; then
        fail_case "$name" "Rust FFmpeg encode failed"
        return
    fi
    if ! cmp -s "$vendor" "$rust"; then
        fail_case "$name" "vendor and Rust bitstreams differ"
        sha256sum "$vendor" "$rust" >&2
        return
    fi
    local counted
    counted=$(ffprobe -v error -count_frames -select_streams v:0 \
        -show_entries stream=nb_read_frames -of default=nw=1:nk=1 "$rust")
    if [ "$counted" != "$frames" ]; then
        fail_case "$name" "frame count $counted, expected $frames"
        return
    fi
    pass_case "$name" "$frames frames, vendor/Rust exact $(sha256sum "$rust" | cut -d' ' -f1)"
}

decode_case h264-30 h264 h264 "$work/h264-30.264" "$frames" nv12
decode_case hevc-30 hevc hevc "$work/hevc-30.265" "$frames" nv12
decode_case vp9-30 vp9 vp9 "$work/vp9-30.ivf" "$frames" nv12
decode_case hevc-main10-10 hevc hevc "$work/hevc-10-10.265" 10 yuv420p10le
encode_case h264-encode-30 h264 h264 264
encode_case hevc-encode-30 hevc hevc 265

printf 'SUMMARY 4K soak passed=%d failed=%d\n' "$passed" "$failed"
exit "$failed"
