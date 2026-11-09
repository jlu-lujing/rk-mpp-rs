#!/usr/bin/env bash

set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"

export PATH="$HOME/.cargo/bin:$PATH"
mpp_root="${MPP_REF:-$HOME/rk-mpp-oracles/mpp-old}"
enc_test="${ENC_TEST:-$mpp_root/build/test/mpi_enc_test}"
vendor_lib="${MPP_LIB:-$mpp_root/build/mpp}"
raw=/tmp/rk_mpp_standalone_encode_320x240.nv12
overlay=$(mktemp -d /tmp/rk_mpp_standalone_encode.XXXXXX)
trap 'rm -rf "$overlay"' EXIT

if [ -z "${ENC_TEST:-}" ] && [ ! -x "$enc_test" ] &&
    [ -x /usr/local/bin/mpi_enc_test ]; then
    enc_test=/usr/local/bin/mpi_enc_test
fi
if [ -z "${MPP_LIB:-}" ] && [ ! -f "$vendor_lib/librockchip_mpp.so.1" ] &&
    [ -f /usr/local/lib/librockchip_mpp.so.1 ]; then
    vendor_lib=/usr/local/lib
fi
if [ ! -x "$enc_test" ] || [ ! -f "$vendor_lib/librockchip_mpp.so.1" ]; then
    printf 'missing vendor mpi_enc_test or comparison library\n' >&2
    exit 2
fi

cargo build --release --offline --lib --bin rk-mpp-rs
ln -s "$repo/target/release/librk_mpp_rs.so" \
    "$overlay/librockchip_mpp.so.1"
if ! LD_LIBRARY_PATH="$overlay" ldd "$enc_test" |
    grep -F "librockchip_mpp.so.1 => $overlay/librockchip_mpp.so.1" >/dev/null; then
    printf 'standalone library did not replace vendor librockchip_mpp\n' >&2
    exit 1
fi

ffmpeg -nostdin -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=320x240:rate=30" -frames:v 1 \
    -pix_fmt nv12 -f rawvideo "$raw"

run_encoder() {
    local library="$1"
    local output="$2"
    local coding="$3"
    local quality="$4"

    sudo -n rm -f "$output"
    sudo -n env LD_LIBRARY_PATH="$library" timeout 60 "$enc_test" \
        -i "$raw" -o "$output" -w 320 -h 240 -n 1 -rc 2 -fps 30 \
        -g 0:60 -t "$coding" -qc "$quality" \
        < /dev/null > "${output}.log" 2>&1
}

decode_and_compare() {
    local decoder="$1"
    local vendor_stream="$2"
    local standalone_stream="$3"
    local vendor_yuv="${vendor_stream}.yuv"
    local standalone_yuv="${standalone_stream}.yuv"

    sudo -n rm -f "$vendor_yuv" "$standalone_yuv"
    sudo -n "$repo/target/release/rk-mpp-rs" "$decoder" \
        "$vendor_stream" "$vendor_yuv" >/dev/null 2>&1
    sudo -n "$repo/target/release/rk-mpp-rs" "$decoder" \
        "$standalone_stream" "$standalone_yuv" >/dev/null 2>&1
    cmp "$vendor_yuv" "$standalone_yuv"
    printf 'PASS %-5s standalone encoder reconstruction (%s)\n' \
        "$decoder" "$(sha256sum "$standalone_yuv" | cut -d' ' -f1)"
}

for coding in h264 h265 mjpeg; do
    case "$coding" in
        h264)
            type=7
            quality=26
            suffix=264
            ;;
        h265)
            type=16777220
            quality=26
            suffix=265
            ;;
        mjpeg)
            type=8
            quality=80
            suffix=jpg
            ;;
    esac
    vendor=/tmp/rk_mpp_standalone_encode_${coding}_vendor.$suffix
    standalone=/tmp/rk_mpp_standalone_encode_${coding}_ours.$suffix
    run_encoder "$vendor_lib" "$vendor" "$type" "$quality"
    run_encoder "$overlay" "$standalone" "$type" "$quality"
    case "$coding" in
        h264)
            decode_and_compare h264dec "$vendor" "$standalone"
            ;;
        h265)
            decode_and_compare hevcdec "$vendor" "$standalone"
            ;;
        mjpeg)
            cmp "$vendor" "$standalone"
            printf 'PASS mjpeg  standalone encoder stream (%s)\n' \
                "$(sha256sum "$standalone" | cut -d' ' -f1)"
            ;;
    esac
done
