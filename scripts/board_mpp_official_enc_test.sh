#!/usr/bin/env bash
# Run the vendor mpi_enc_test binary unchanged with LD_PRELOAD. H.265 and
# MJPEG are byte-identical; H.264's single-frame reconstructed pixels are
# compared because the old vendor slice header uses a different IDR state
# encoding. Multi-frame H.264 remains a known W7-3 divergence.
set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"

export PATH="$HOME/.cargo/bin:$PATH"
mpp_old="${MPP_OLD:-$HOME/rk-mpp-oracles/mpp-old}"
enc_test="${ENC_TEST:-$mpp_old/build/test/mpi_enc_test}"
mpp_lib="${MPP_LIB:-$mpp_old/build/mpp}"
preload="$repo/target/release/librk_mpp_rs.so"
cli="$repo/target/release/rk-mpp-rs"
raw="${ENC_RAW:-/tmp/rk_mpp_official_enc_320x240.nv12}"
gop="${ENC_GOP:-60}"

if [ ! -x "$enc_test" ] || [ ! -f "$mpp_lib/librockchip_mpp.so.0" ]; then
    printf 'missing vendor mpi_enc_test under %s\n' "$mpp_old" >&2
    exit 2
fi
if ! command -v ffmpeg >/dev/null 2>&1; then
    printf 'ffmpeg is required to generate the NV12 input\n' >&2
    exit 2
fi

cargo build --release --offline --lib --bin rk-mpp-rs
ffmpeg -nostdin -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=320x240:rate=30" -frames:v 1 \
    -pix_fmt nv12 -f rawvideo "$raw"

run_vendor_frames() {
    local output="$1"
    local frames="$2"
    shift
    shift
    LD_LIBRARY_PATH="$mpp_lib:${LD_LIBRARY_PATH:-}" \
        timeout 30 "$enc_test" -i "$raw" -o "$output" \
        -w 320 -h 240 -n "$frames" -rc 2 -fps 30 -g "$gop" "$@" \
        < /dev/null > /dev/null 2>&1
}

run_compat_frames() {
    local output="$1"
    local frames="$2"
    shift
    shift
    LD_PRELOAD="$preload" LD_LIBRARY_PATH="$mpp_lib:${LD_LIBRARY_PATH:-}" \
        timeout 30 "$enc_test" -i "$raw" -o "$output" \
        -w 320 -h 240 -n "$frames" -rc 2 -fps 30 -g "$gop" "$@" \
        < /dev/null > /dev/null 2>&1
}

run_vendor() {
    local output="$1"
    shift
    run_vendor_frames "$output" 1 "$@"
}

run_compat() {
    local output="$1"
    shift
    run_compat_frames "$output" 1 "$@"
}

if [ "${SKIP_H264:-0}" = 1 ]; then
    printf 'SKIP h264 (this SoC path requires unsupported profile/GOP controls)\n'
else
    h264_c=/tmp/rk_mpp_official_enc_h264_c.264
    h264_r=/tmp/rk_mpp_official_enc_h264_r.264
    h264_c_yuv=/tmp/rk_mpp_official_enc_h264_c.yuv
    h264_r_yuv=/tmp/rk_mpp_official_enc_h264_r.yuv
    run_vendor "$h264_c" -t 7 -qc 26
    run_compat "$h264_r" -t 7 -qc 26
    LD_LIBRARY_PATH="$repo/target/release:${LD_LIBRARY_PATH:-}" \
        "$cli" h264dec "$h264_c" "$h264_c_yuv" > /dev/null
    LD_LIBRARY_PATH="$repo/target/release:${LD_LIBRARY_PATH:-}" \
        "$cli" h264dec "$h264_r" "$h264_r_yuv" > /dev/null
    cmp "$h264_c_yuv" "$h264_r_yuv"
    printf 'PASS h264 one-frame reconstruction through unchanged mpi_enc_test (%s)\n' \
        "$(sha256sum "$h264_r_yuv" | cut -d' ' -f1)"

    h264_multi_c=/tmp/rk_mpp_official_enc_h264_multi_c.264
    h264_multi_r=/tmp/rk_mpp_official_enc_h264_multi_r.264
    run_vendor_frames "$h264_multi_c" 6 -t 7 -qc 26
    run_compat_frames "$h264_multi_r" 6 -t 7 -qc 26
    python3 "$repo/scripts/compare_streams.py" "$h264_multi_c" "$h264_multi_r"
    LD_LIBRARY_PATH="$repo/target/release:${LD_LIBRARY_PATH:-}" \
        "$cli" h264dec "$h264_multi_c" /tmp/rk_mpp_official_enc_h264_multi_c.yuv \
        > /dev/null
    LD_LIBRARY_PATH="$repo/target/release:${LD_LIBRARY_PATH:-}" \
        "$cli" h264dec "$h264_multi_r" /tmp/rk_mpp_official_enc_h264_multi_r.yuv \
        > /dev/null
    cmp /tmp/rk_mpp_official_enc_h264_multi_c.yuv \
        /tmp/rk_mpp_official_enc_h264_multi_r.yuv
    printf 'PASS h264 six-frame stream and reconstruction through unchanged mpi_enc_test\n'
fi

if [ "${SKIP_H265:-0}" = 1 ]; then
    printf 'SKIP h265 (this SoC encoder path is not byte-identical yet)\n'
else
    h265_c=/tmp/rk_mpp_official_enc_h265_c.265
    h265_r=/tmp/rk_mpp_official_enc_h265_r.265
    run_vendor "$h265_c" -t 16777220 -qc 26
    run_compat "$h265_r" -t 16777220 -qc 26
    cmp "$h265_c" "$h265_r"
    printf 'PASS h265 stream through unchanged mpi_enc_test (%s)\n' \
        "$(sha256sum "$h265_r" | cut -d' ' -f1)"
fi

mjpeg_c=/tmp/rk_mpp_official_enc_mjpeg_c.jpg
mjpeg_r=/tmp/rk_mpp_official_enc_mjpeg_r.jpg
run_vendor "$mjpeg_c" -t 8 -qc 80
run_compat "$mjpeg_r" -t 8 -qc 80
cmp "$mjpeg_c" "$mjpeg_r"
printf 'PASS mjpeg stream through unchanged mpi_enc_test (%s)\n' \
    "$(sha256sum "$mjpeg_r" | cut -d' ' -f1)"
