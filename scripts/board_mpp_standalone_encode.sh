#!/usr/bin/env bash

set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"

export PATH="$HOME/.cargo/bin:$PATH"
mpp_root="${MPP_REF:-$HOME/rk-mpp-oracles/mpp-old}"
enc_test="${ENC_TEST:-$mpp_root/build/test/mpi_enc_test}"
vendor_lib="${MPP_LIB:-$mpp_root/build/mpp}"
raw=/tmp/rk_mpp_standalone_encode_320x240.nv12
raw_multi=/tmp/rk_mpp_standalone_encode_320x240_6.nv12
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
ffmpeg -nostdin -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=320x240:rate=30" -frames:v 12 \
    -pix_fmt nv12 -f rawvideo "$raw_multi"

run_encoder_frames() {
    local library="$1"
    local output="$2"
    local coding="$3"
    local quality="$4"
    local gop="$5"
    local frames="$6"
    local input="$7"

    sudo -n rm -f "$output"
    sudo -n env LD_LIBRARY_PATH="$library" timeout 60 "$enc_test" \
        -i "$input" -o "$output" -w 320 -h 240 -n "$frames" -rc 2 -fps 30 \
        -g "$gop" -t "$coding" -qc "$quality" \
        < /dev/null > "${output}.log" 2>&1
}

run_encoder() {
    run_encoder_frames "$1" "$2" "$3" "$4" "$5" 1 "$raw"
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
            gop=1:60
            ;;
        h265)
            type=16777220
            quality=26
            suffix=265
            gop=0:60
            ;;
        mjpeg)
            type=8
            quality=80
            suffix=jpg
            gop=0:60
            ;;
    esac
    vendor=/tmp/rk_mpp_standalone_encode_${coding}_vendor.$suffix
    standalone=/tmp/rk_mpp_standalone_encode_${coding}_ours.$suffix
    run_encoder "$vendor_lib" "$vendor" "$type" "$quality" "$gop"
    run_encoder "$overlay" "$standalone" "$type" "$quality" "$gop"
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

if grep -qi rk3588 /proc/device-tree/compatible 2>/dev/null; then
    vendor=/tmp/rk_mpp_standalone_encode_h264_tsvc2_vendor.264
    standalone=/tmp/rk_mpp_standalone_encode_h264_tsvc2_ours.264
    run_encoder_frames "$vendor_lib" "$vendor" 7 26 1:60 6 "$raw_multi"
    run_encoder_frames "$overlay" "$standalone" 7 26 1:60 6 "$raw_multi"
    python3 "$repo/scripts/compare_streams.py" "$vendor" "$standalone"
    decode_and_compare h264dec "$vendor" "$standalone"
    printf 'PASS h264  six-frame TSVC2 standalone encoder NAL/reconstruction\n'

    vendor=/tmp/rk_mpp_standalone_encode_h264_tsvc2_vendor12.264
    standalone=/tmp/rk_mpp_standalone_encode_h264_tsvc2_ours12.264
    run_encoder_frames "$vendor_lib" "$vendor" 7 26 1:60 12 "$raw_multi"
    run_encoder_frames "$overlay" "$standalone" 7 26 1:60 12 "$raw_multi"
    python3 "$repo/scripts/compare_streams.py" "$vendor" "$standalone"
    decode_and_compare h264dec "$vendor" "$standalone"
    printf 'PASS h264  twelve-frame TSVC2 standalone encoder NAL/reconstruction\n'

    vendor=/tmp/rk_mpp_standalone_encode_h264_tsvc3_vendor12.264
    standalone=/tmp/rk_mpp_standalone_encode_h264_tsvc3_ours12.264
    run_encoder_frames "$vendor_lib" "$vendor" 7 26 2:60 12 "$raw_multi"
    run_encoder_frames "$overlay" "$standalone" 7 26 2:60 12 "$raw_multi"
    python3 "$repo/scripts/compare_streams.py" "$vendor" "$standalone"
    decode_and_compare h264dec "$vendor" "$standalone"
    printf 'PASS h264  twelve-frame TSVC3 standalone encoder NAL/reconstruction\n'

    vendor=/tmp/rk_mpp_standalone_encode_h264_tsvc4_vendor12.264
    standalone=/tmp/rk_mpp_standalone_encode_h264_tsvc4_ours12.264
    run_encoder_frames "$vendor_lib" "$vendor" 7 26 3:60 12 "$raw_multi"
    run_encoder_frames "$overlay" "$standalone" 7 26 3:60 12 "$raw_multi"
    python3 "$repo/scripts/compare_streams.py" "$vendor" "$standalone"
    decode_and_compare h264dec "$vendor" "$standalone"
    printf 'PASS h264  twelve-frame TSVC4 standalone encoder NAL/reconstruction\n'

    raw_long=/tmp/rk_mpp_standalone_encode_320x240_64.nv12
    ffmpeg -nostdin -y -hide_banner -loglevel error \
        -f lavfi -i "testsrc2=size=320x240:rate=30" -frames:v 64 \
        -pix_fmt nv12 -f rawvideo "$raw_long"
    vendor=/tmp/rk_mpp_standalone_encode_h264_tsvc4_gop64_vendor.264
    standalone=/tmp/rk_mpp_standalone_encode_h264_tsvc4_gop64_ours.264
    run_encoder_frames "$vendor_lib" "$vendor" 7 26 3:60 64 "$raw_long"
    run_encoder_frames "$overlay" "$standalone" 7 26 3:60 64 "$raw_long"
    python3 "$repo/scripts/compare_streams.py" "$vendor" "$standalone"
    decode_and_compare h264dec "$vendor" "$standalone"
    printf 'PASS h264  TSVC4 64-frame IGOP restart NAL/reconstruction\n'
else
    vendor=/tmp/rk_mpp_standalone_encode_h264_tsvc2_v510_vendor.264
    standalone=/tmp/rk_mpp_standalone_encode_h264_tsvc2_v510_ours.264
    run_encoder_frames "$vendor_lib" "$vendor" 7 26 1:60 12 "$raw_multi"
    run_encoder_frames "$overlay" "$standalone" 7 26 1:60 12 "$raw_multi"
    python3 "$repo/scripts/compare_streams.py" "$vendor" "$standalone"
    decode_and_compare h264dec "$vendor" "$standalone"
    printf 'PASS h264  twelve-frame TSVC2 VEPU510 NAL/reconstruction\n'

    vendor=/tmp/rk_mpp_standalone_encode_h264_tsvc3_v510_vendor.264
    standalone=/tmp/rk_mpp_standalone_encode_h264_tsvc3_v510_ours.264
    run_encoder_frames "$vendor_lib" "$vendor" 7 26 2:60 12 "$raw_multi"
    run_encoder_frames "$overlay" "$standalone" 7 26 2:60 12 "$raw_multi"
    decode_and_compare h264dec "$vendor" "$standalone"
    printf 'PASS h264  TSVC3 VEPU510 reconstruction (C prefix warm-up differs)\n'

    vendor=/tmp/rk_mpp_standalone_encode_h264_tsvc4_v510_vendor.264
    standalone=/tmp/rk_mpp_standalone_encode_h264_tsvc4_v510_ours.264
    run_encoder_frames "$overlay" "$standalone" 7 26 3:60 12 "$raw_multi"
    matched=0
    for _ in 1 2 3 4; do
        run_encoder_frames "$vendor_lib" "$vendor" 7 26 3:60 12 "$raw_multi"
        if python3 "$repo/scripts/compare_streams.py" "$vendor" "$standalone"; then
            matched=1
            break
        fi
    done
    test "$matched" -eq 1
    decode_and_compare h264dec "$vendor" "$standalone"
    printf 'PASS h264  TSVC4 VEPU510 NAL/reconstruction after C warm-up retry\n'
fi
