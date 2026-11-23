#!/usr/bin/env bash
# Run on an RK3588 board. Covers the 4K paths that differ materially from
# smaller frames: codec decode state, Main10/P010 output, JPEG band splitting,
# and the FFmpeg-RKMPP drop-in surface.
set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"

export PATH="$HOME/.cargo/bin:$PATH"

work=$(mktemp -d /tmp/rk-mpp-rs-4k.XXXXXX)
overlay=$(mktemp -d /tmp/rk-mpp-rs-4k-overlay.XXXXXX)
trap 'rm -rf "$work" "$overlay"' EXIT

mpp_ref="${MPP_REF:-$HOME/rk-mpp-oracles/mpp-develop}"
dec_test="${DEC_TEST:-$mpp_ref/build/test/mpi_dec_test}"
mpp_lib="${MPP_LIB:-$mpp_ref/build/mpp}"
target_dir="${CARGO_TARGET_DIR:-$repo/target}"
rust_lib="${RUST_LIB:-$target_dir/release/librk_mpp_rs.so}"
cli="${RUST_BIN:-$target_dir/release/rk-mpp-rs}"
ffmpeg_bin="${FFMPEG:-$(command -v ffmpeg || true)}"
ffprobe_bin="${FFPROBE:-$(command -v ffprobe || true)}"
vpxenc_bin="${VPXENC:-$(command -v vpxenc || true)}"

width=3840
height=2160
frame_bytes=12441600
frame_bytes_10=24883200
passed=0
failed=0

die() {
    printf 'ERROR: %s\n' "$*" >&2
    exit 2
}

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || die "missing command: $1"
}

pass_case() {
    printf 'PASS %-28s %s\n' "$1" "$2"
    passed=$((passed + 1))
}

fail_case() {
    printf 'FAIL %-28s %s\n' "$1" "$2" >&2
    failed=$((failed + 1))
}

same_file() {
    local name="$1"
    local left="$2"
    local right="$3"
    if ! cmp -s "$left" "$right"; then
        fail_case "$name" "$(basename "$left") and $(basename "$right") differ"
        sha256sum "$left" "$right" >&2
        return 1
    fi
    return 0
}

size_is() {
    local name="$1"
    local path="$2"
    local expected="$3"
    local actual
    actual=$(stat -c %s "$path" 2>/dev/null || printf missing)
    if [ "$actual" != "$expected" ]; then
        fail_case "$name" "size $actual, expected $expected"
        return 1
    fi
    return 0
}

require_cmd cargo
require_cmd python3
require_cmd timeout
[ -n "$ffmpeg_bin" ] || die "missing command: ffmpeg"
[ -n "$ffprobe_bin" ] || die "missing command: ffprobe"
require_cmd aomdec
require_cmd aomenc
[ -n "$vpxenc_bin" ] || die "missing command: vpxenc"
if ! "$vpxenc_bin" --help 2>&1 | grep -F -- "--bit-depth" >/dev/null; then
    die "vpxenc has no high-bit-depth support; set VPXENC to a build configured with --enable-vp9-highbitdepth"
fi
[ -x "$dec_test" ] || die "missing C decoder: $dec_test"
[ -f "$mpp_lib/librockchip_mpp.so.0" ] ||
    die "missing C MPP library under: $mpp_lib"

cargo build --release --offline --lib --bin rk-mpp-rs
ln -s "$rust_lib" "$overlay/librockchip_mpp.so.1"

if ! LD_LIBRARY_PATH="$overlay" ldd "$ffmpeg_bin" |
    grep -F "librockchip_mpp.so.1 => $overlay/librockchip_mpp.so.1" >/dev/null; then
    die "FFmpeg does not resolve librockchip_mpp.so.1 through the Rust overlay"
fi
for codec in h264_rkmpp hevc_rkmpp vp9_rkmpp; do
    "$ffmpeg_bin" -hide_banner -decoders 2>/dev/null |
        grep -F "$codec" >/dev/null ||
        die "installed FFmpeg has no $codec decoder"
done
for codec in h264_rkmpp hevc_rkmpp mjpeg_rkmpp; do
    "$ffmpeg_bin" -hide_banner -encoders 2>/dev/null |
        grep -F "$codec" >/dev/null ||
        die "installed FFmpeg has no $codec encoder"
done

generate_fixtures() {
    local frames

    for frames in 1 3; do
        local profile=main
        local x264_params="keyint=30:min-keyint=30:bframes=0:scenecut=0:repeat-headers=1"
        if [ "$frames" = 1 ]; then
            profile=baseline
        fi
        "$ffmpeg_bin" -nostdin -hide_banner -loglevel error -y \
            -f lavfi -i "testsrc2=size=${width}x${height}:rate=30" \
            -frames:v "$frames" -pix_fmt yuv420p \
            -c:v libx264 -preset veryfast -profile:v "$profile" \
            -x264-params "$x264_params" -f h264 \
            "$work/h264-${frames}.264"
        "$ffmpeg_bin" -nostdin -hide_banner -loglevel error -y \
            -f lavfi -i "testsrc2=size=${width}x${height}:rate=30" \
            -frames:v "$frames" -pix_fmt yuv420p \
            -c:v libx265 -preset ultrafast -profile:v main \
            -x265-params \
            "log-level=error:keyint=30:min-keyint=30:bframes=0:scenecut=0:repeat-headers=1" \
            -f hevc "$work/hevc-${frames}.265"
        "$ffmpeg_bin" -nostdin -hide_banner -loglevel error -y \
            -f lavfi -i "testsrc2=size=${width}x${height}:rate=30" \
            -frames:v "$frames" -pix_fmt yuv420p -f rawvideo \
            "$work/vp9-${frames}.yuv"
        "$vpxenc_bin" --quiet --codec=vp9 --profile=0 --passes=1 --good \
            --cpu-used=8 --threads=4 --lag-in-frames=0 --kf-max-dist=30 \
            --limit="$frames" --width="$width" --height="$height" \
            --fps=30/1 --ivf -o "$work/vp9-${frames}.ivf" \
            "$work/vp9-${frames}.yuv"
        "$ffmpeg_bin" -nostdin -hide_banner -loglevel error -y \
            -f lavfi -i "testsrc2=size=${width}x${height}:rate=30" \
            -frames:v "$frames" -pix_fmt yuv420p -f rawvideo \
            "$work/av1-${frames}.yuv"
        aomenc --quiet --limit="$frames" --width="$width" --height="$height" \
            --fps=30/1 --cpu-used=8 --passes=1 --end-usage=q --cq-level=32 \
            --profile=0 --ivf -o "$work/av1-${frames}.ivf" \
            "$work/av1-${frames}.yuv"
    done

    "$ffmpeg_bin" -nostdin -hide_banner -loglevel error -y \
        -f lavfi -i "testsrc2=size=${width}x${height}:rate=30" \
        -frames:v 1 -pix_fmt yuv420p10le \
        -c:v libx265 -preset ultrafast -profile:v main10 \
        -x265-params \
        "log-level=error:keyint=30:min-keyint=30:bframes=0:scenecut=0:repeat-headers=1" \
        -f hevc "$work/hevc-10.265"
    "$ffmpeg_bin" -nostdin -hide_banner -loglevel error -y \
        -f lavfi -i "testsrc2=size=${width}x${height}:rate=30" \
        -frames:v 1 -pix_fmt yuv420p10le -f rawvideo "$work/av1-10.yuv"
    aomenc --quiet --limit=1 --width="$width" --height="$height" \
        --fps=30/1 --cpu-used=8 --passes=1 --end-usage=q --cq-level=32 \
        --profile=0 --bit-depth=10 --input-bit-depth=10 --ivf \
        -o "$work/av1-10.ivf" "$work/av1-10.yuv"
    aomenc --quiet --limit=1 --width="$width" --height="$height" \
        --fps=30/1 --cpu-used=8 --passes=1 --end-usage=q --cq-level=32 \
        --profile=0 --film-grain-test=1 --ivf \
        -o "$work/av1-grain-8.ivf" "$work/av1-1.yuv"
    aomenc --quiet --limit=1 --width="$width" --height="$height" \
        --fps=30/1 --cpu-used=8 --passes=1 --end-usage=q --cq-level=32 \
        --profile=0 --bit-depth=10 --input-bit-depth=10 \
        --film-grain-test=1 --ivf \
        -o "$work/av1-grain-10.ivf" "$work/av1-10.yuv"
    "$vpxenc_bin" --quiet --codec=vp9 --profile=2 --bit-depth=10 \
        --input-bit-depth=10 --passes=1 --good --cpu-used=8 --threads=4 \
        --lag-in-frames=0 --kf-max-dist=30 --limit=1 --width="$width" \
        --height="$height" --fps=30/1 --ivf \
        -o "$work/vp9-10.ivf" "$work/av1-10.yuv"

    for frames in 1 3; do
        "$ffprobe_bin" -v error -select_streams v:0 -count_frames \
            -show_entries stream=width,height,nb_read_frames \
            -of default=nw=1:nk=1 "$work/h264-${frames}.264" |
            paste -sd, - |
            grep -Fx "${width},${height},${frames}" >/dev/null ||
            die "H.264 ${frames}-frame fixture has unexpected geometry/count"
    done
}

ffmpeg_decode_exact() {
    local name="$1"
    local codec="$2"
    local software="$3"
    local input="$4"
    local frames="$5"
    local pixel_format="$6"
    local bytes="$7"
    local vendor="$work/${name}-vendor.yuv"
    local rust="$work/${name}-rust.yuv"
    local oracle="$work/${name}-software.yuv"

    if ! timeout 120 "$ffmpeg_bin" -nostdin -hide_banner -loglevel error \
        -c:v "${codec}_rkmpp" -i "$input" -frames:v "$frames" \
        -fps_mode passthrough -pix_fmt "$pixel_format" -f rawvideo -y \
        "$vendor"; then
        fail_case "$name" "vendor FFmpeg decode failed"
        return
    fi
    if ! timeout 120 env LD_LIBRARY_PATH="$overlay" "$ffmpeg_bin" \
        -nostdin -hide_banner -loglevel error -c:v "${codec}_rkmpp" \
        -i "$input" -frames:v "$frames" -fps_mode passthrough \
        -pix_fmt "$pixel_format" -f rawvideo -y "$rust"; then
        fail_case "$name" "Rust FFmpeg drop-in decode failed"
        return
    fi
    if ! timeout 120 "$ffmpeg_bin" -nostdin -hide_banner -loglevel error \
        -c:v "$software" -i "$input" -frames:v "$frames" \
        -fps_mode passthrough -pix_fmt "$pixel_format" -f rawvideo -y \
        "$oracle"; then
        fail_case "$name" "software decode oracle failed"
        return
    fi
    size_is "$name" "$rust" "$bytes" || return 0
    same_file "$name" "$vendor" "$rust" || return 0
    same_file "$name" "$rust" "$oracle" || return 0
    pass_case "$name" \
        "$frames frame(s), C/Rust/software exact $(sha256sum "$rust" | cut -d' ' -f1)"
}

av1_decode_exact() {
    local name="$1"
    local input="$2"
    local frames="$3"
    local depth="$4"
    local c_raw="$work/${name}-c.raw"
    local c_planar="$work/${name}-c.yuv"
    local rust="$work/${name}-rust.yuv"
    local oracle="$work/${name}-aom.yuv"
    local expected

    if [ "$depth" = 8 ]; then
        expected=$((frame_bytes * frames))
    else
        expected=$((frame_bytes_10 * frames))
    fi

    if ! timeout 120 env LD_LIBRARY_PATH="$mpp_lib:${LD_LIBRARY_PATH:-}" \
        "$dec_test" -i "$input" -o "$c_raw" -w "$width" -h "$height" \
        -t 16777224 -f 0 -n "$frames" -v q \
        </dev/null >"$work/${name}-c.log" 2>&1; then
        fail_case "$name" "C mpi_dec_test failed"
        return
    fi
    if [ "$depth" = 8 ]; then
        if ! "$ffmpeg_bin" -nostdin -hide_banner -loglevel error -y \
            -f rawvideo -pix_fmt nv12 -s "${width}x${height}" -i "$c_raw" \
            -pix_fmt yuv420p -f rawvideo "$c_planar"; then
            fail_case "$name" "C NV12 to planar conversion failed"
            return
        fi
    elif ! python3 "$repo/scripts/p010_to_planar.py" \
        "$c_raw" "$c_planar" "$width" "$height"; then
        fail_case "$name" "C packed-10-bit to planar conversion failed"
        return
    fi
    if ! timeout 120 env LD_LIBRARY_PATH="$target_dir/release" \
        "$cli" av1dec "$input" "$rust" \
        >"$work/${name}-rust.log" 2>&1; then
        fail_case "$name" "Rust av1dec failed"
        return
    fi
    if ! timeout 120 aomdec --rawvideo --limit="$frames" \
        -o "$oracle" "$input" >"$work/${name}-aom.log" 2>&1; then
        fail_case "$name" "aomdec oracle failed"
        return
    fi
    size_is "$name" "$rust" "$expected" || return 0
    same_file "$name" "$c_planar" "$rust" || return 0
    same_file "$name" "$rust" "$oracle" || return 0
    pass_case "$name" \
        "$frames frame(s), C/Rust/aomdec exact $(sha256sum "$rust" | cut -d' ' -f1)"
}

av1_grain_exact() {
    local name="$1"
    local input="$2"
    local expected="$3"
    local rust="$work/${name}-rust.yuv"
    local oracle="$work/${name}-aom.yuv"

    if ! timeout 120 env LD_LIBRARY_PATH="$target_dir/release" \
        "$cli" av1dec "$input" "$rust" >"$work/${name}-rust.log" 2>&1; then
        fail_case "$name" "Rust av1dec failed"
        return
    fi
    if ! timeout 120 aomdec --rawvideo --limit=1 \
        -o "$oracle" "$input" >"$work/${name}-aom.log" 2>&1; then
        fail_case "$name" "aomdec oracle failed"
        return
    fi
    size_is "$name" "$rust" "$expected" || return 0
    same_file "$name" "$rust" "$oracle" || return 0
    pass_case "$name" \
        "Rust/aomdec grain synthesis exact $(sha256sum "$rust" | cut -d' ' -f1)"
}

oracle_dropin_exact() {
    local name="$1"
    local input="$2"
    local frames="$3"
    local bytes="$4"
    local vendor="$work/${name}-vendor.raw"
    local rust="$work/${name}-rust.raw"

    if ! timeout 120 env LD_LIBRARY_PATH="$mpp_lib:${LD_LIBRARY_PATH:-}" \
        "$dec_test" -i "$input" -o "$vendor" -w "$width" -h "$height" \
        -t 16777224 -f 0 -n "$frames" -v q \
        </dev/null >"$work/${name}-vendor.log" 2>&1; then
        fail_case "$name" "vendor mpi_dec_test failed"
        return
    fi
    if ! timeout 120 env LD_PRELOAD="$rust_lib" \
        LD_LIBRARY_PATH="$mpp_lib:${LD_LIBRARY_PATH:-}" \
        "$dec_test" -i "$input" -o "$rust" -w "$width" -h "$height" \
        -t 16777224 -f 0 -n "$frames" -v q \
        </dev/null >"$work/${name}-rust.log" 2>&1; then
        fail_case "$name" "Rust mpi_dec_test drop-in failed"
        return
    fi
    size_is "$name" "$rust" "$bytes" || return 0
    same_file "$name" "$vendor" "$rust" || return 0
    pass_case "$name" \
        "$frames frame(s), C/Rust drop-in exact $(sha256sum "$rust" | cut -d' ' -f1)"
}

ffmpeg_encode_exact() {
    local name="$1"
    local codec="$2"
    local muxer="$3"
    local extension="$4"
    local frames="$5"
    local vendor="$work/${name}-vendor.$extension"
    local rust="$work/${name}-rust.$extension"
    local common=(
        -nostdin -hide_banner -loglevel error
        -f lavfi -i "testsrc2=size=${width}x${height}:rate=30"
        -frames:v "$frames" -pix_fmt nv12
        -c:v "${codec}_rkmpp" -rc_mode CQP -qp_init 26 -g 30
        -f "$muxer" -y
    )

    if ! timeout 120 "$ffmpeg_bin" "${common[@]}" "$vendor"; then
        fail_case "$name" "vendor FFmpeg encode failed"
        return
    fi
    if ! timeout 120 env LD_LIBRARY_PATH="$overlay" "$ffmpeg_bin" \
        "${common[@]}" "$rust"; then
        fail_case "$name" "Rust FFmpeg drop-in encode failed"
        return
    fi
    local vendor_frames rust_frames
    vendor_frames=$("$ffprobe_bin" -v error -count_frames -select_streams v:0 \
        -show_entries stream=nb_read_frames -of default=nw=1:nk=1 "$vendor")
    rust_frames=$("$ffprobe_bin" -v error -count_frames -select_streams v:0 \
        -show_entries stream=nb_read_frames -of default=nw=1:nk=1 "$rust")
    if [ "$vendor_frames" != "$frames" ] || [ "$rust_frames" != "$frames" ]; then
        fail_case "$name" \
            "frame count vendor=$vendor_frames rust=$rust_frames, expected $frames"
        return
    fi
    same_file "$name" "$vendor" "$rust" || return 0
    pass_case "$name" \
        "$frames frames, vendor/Rust exact $(sha256sum "$rust" | cut -d' ' -f1)"
}

mjpeg_encode_exact() {
    local size="$1"
    local vendor="$work/mjpeg-${size}-vendor.jpg"
    local rust="$work/mjpeg-${size}-rust.jpg"
    local common=(
        -nostdin -hide_banner -loglevel error
        -f lavfi -i "testsrc2=size=${size}:rate=30"
        -frames:v 1 -pix_fmt nv12 -c:v mjpeg_rkmpp -f mjpeg -y
    )

    if ! timeout 120 "$ffmpeg_bin" "${common[@]}" "$vendor"; then
        fail_case "mjpeg-encode-${size}" "vendor FFmpeg encode failed"
        return
    fi
    if ! timeout 120 env LD_LIBRARY_PATH="$overlay" "$ffmpeg_bin" \
        "${common[@]}" "$rust"; then
        fail_case "mjpeg-encode-${size}" "Rust FFmpeg drop-in encode failed"
        return
    fi
    same_file "mjpeg-encode-${size}" "$vendor" "$rust" || return 0
    local dri
    dri=$(python3 "$repo/scripts/jpege_markers.py" "$rust" | sed -n '2p')
    if [ "$dri" != "DRI none" ]; then
        fail_case "mjpeg-encode-${size}" "unexpected automatic restart interval: $dri"
        return
    fi
    pass_case "mjpeg-encode-${size}" \
        "vendor/Rust exact, no DRI, $(sha256sum "$rust" | cut -d' ' -f1)"
}

mjpeg_oracle_decode_exact() {
    local input="$work/mjpeg-3840x2160-rust.jpg"
    local vendor="$work/mjpeg-decode-vendor.yuv"
    local rust="$work/mjpeg-decode-rust.yuv"

    if [ ! -f "$input" ]; then
        fail_case "mjpeg-decode-3840x2160" "missing 4K JPEG from the encode case"
        return
    fi
    if ! timeout 120 env LD_LIBRARY_PATH="$mpp_lib:${LD_LIBRARY_PATH:-}" \
        "$dec_test" -i "$input" -o "$vendor" -w "$width" -h "$height" \
        -t 8 -f 0 -n 1 -v q </dev/null \
        >"$work/mjpeg-decode-vendor.log" 2>&1; then
        fail_case "mjpeg-decode-3840x2160" "vendor mpi_dec_test failed"
        return
    fi
    if ! timeout 120 env LD_PRELOAD="$rust_lib" \
        LD_LIBRARY_PATH="$mpp_lib:${LD_LIBRARY_PATH:-}" \
        "$dec_test" -i "$input" -o "$rust" -w "$width" -h "$height" \
        -t 8 -f 0 -n 1 -v q </dev/null \
        >"$work/mjpeg-decode-rust.log" 2>&1; then
        fail_case "mjpeg-decode-3840x2160" "Rust mpi_dec_test drop-in failed"
        return
    fi
    size_is "mjpeg-decode-3840x2160" "$rust" "$frame_bytes" || return 0
    same_file "mjpeg-decode-3840x2160" "$vendor" "$rust" || return 0
    pass_case "mjpeg-decode-3840x2160" \
        "C/Rust exact $(sha256sum "$rust" | cut -d' ' -f1)"
}

printf '4K matrix: %sx%s, work=%s\n' "$width" "$height" "$work"
generate_fixtures

ffmpeg_decode_exact h264-4k-1 h264 h264 "$work/h264-1.264" 1 nv12 "$frame_bytes"
ffmpeg_decode_exact h264-4k-3 h264 h264 "$work/h264-3.264" 3 nv12 \
    "$((frame_bytes * 3))"
ffmpeg_decode_exact hevc-4k-1 hevc hevc "$work/hevc-1.265" 1 nv12 "$frame_bytes"
ffmpeg_decode_exact hevc-4k-3 hevc hevc "$work/hevc-3.265" 3 nv12 \
    "$((frame_bytes * 3))"
ffmpeg_decode_exact vp9-4k-1 vp9 vp9 "$work/vp9-1.ivf" 1 nv12 "$frame_bytes"
ffmpeg_decode_exact vp9-4k-3 vp9 vp9 "$work/vp9-3.ivf" 3 nv12 \
    "$((frame_bytes * 3))"
ffmpeg_decode_exact vp9-4k-main10 vp9 vp9 "$work/vp9-10.ivf" 1 \
    yuv420p10le "$frame_bytes_10"
ffmpeg_decode_exact hevc-4k-main10 hevc hevc "$work/hevc-10.265" 1 \
    yuv420p10le "$frame_bytes_10"

av1_decode_exact av1-4k-1 "$work/av1-1.ivf" 1 8
av1_decode_exact av1-4k-3 "$work/av1-3.ivf" 3 8
av1_decode_exact av1-4k-main10 "$work/av1-10.ivf" 1 10
av1_grain_exact av1-4k-grain8 "$work/av1-grain-8.ivf" "$frame_bytes"
av1_grain_exact av1-4k-grain10 "$work/av1-grain-10.ivf" "$frame_bytes_10"
oracle_dropin_exact av1-dropin-4k-1 "$work/av1-1.ivf" 1 "$frame_bytes"
oracle_dropin_exact av1-dropin-4k-3 "$work/av1-3.ivf" 3 \
    "$((frame_bytes * 3))"
oracle_dropin_exact av1-dropin-4k-main10 "$work/av1-10.ivf" 1 "$frame_bytes_10"

ffmpeg_encode_exact h264-4k-encode h264 h264 264 3
ffmpeg_encode_exact hevc-4k-encode hevc hevc 265 3
mjpeg_encode_exact 1920x1080
mjpeg_encode_exact 2560x1440
mjpeg_encode_exact 3840x2160
mjpeg_oracle_decode_exact

printf 'SUMMARY passed=%d failed=%d\n' "$passed" "$failed"
exit "$failed"
