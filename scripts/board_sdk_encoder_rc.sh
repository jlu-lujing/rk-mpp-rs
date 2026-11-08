#!/usr/bin/env bash

set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"

export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release --offline --lib --bin rk-mpp-rs

library="$repo/target/release/librk_mpp_rs.so"
include="$repo/include"
fixture=/tmp/rk_mpp_sdk_rc_6.nv12
libdir=$(mktemp -d)
trap 'rm -rf "$libdir"' EXIT
ln -sfn "$library" "$libdir/librk_mpp_rs.so"
ln -sfn "$library" "$libdir/librk_mpp_rs.so.1"
ffmpeg -nostdin -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=320x240:rate=30" -frames:v 6 \
    -pix_fmt nv12 -f rawvideo "$fixture"

gcc -std=c11 -Wall -Wextra -Werror \
    -I"$include" "$repo/tests/c_api_encoder.c" \
    -L"$libdir" -lrk_mpp_rs \
    -Wl,-rpath,"$libdir" \
    -o /tmp/rk_mpp_encoder_v19
gcc -std=c11 -Wall -Wextra -Werror \
    -I"$repo/tests/abi" "$repo/tests/c_api_encoder_v18.c" \
    -L"$libdir" -lrk_mpp_rs \
    -Wl,-rpath,"$libdir" \
    -o /tmp/rk_mpp_encoder_v18

/tmp/rk_mpp_encoder_v18 "$fixture" /tmp/rk_mpp_encoder_v18.264
test -s /tmp/rk_mpp_encoder_v18.264

compatible=$(tr '\0' '\n' < /proc/device-tree/compatible)
case "$compatible" in
    *rockchip,rk3576*)
        printf 'PASS rk-mpp-rs V1.8 consumer; VEPU510 streaming RC platform SKIP\n'
        exit 0
        ;;
esac

compare_case() {
    codec=$1
    mode=$2
    gop=$3
    target=300000
    minimum=250000
    maximum=350000
    c_api_out="/tmp/rk_mpp_sdk_${codec}_${mode}.bin"
    cli_out="/tmp/rk_mpp_cli_${codec}_${mode}.bin"

    RATE_CONTROL_MODE="$mode" \
    RATE_TARGET_BPS="$target" \
    RATE_MIN_BPS="$minimum" \
    RATE_MAX_BPS="$maximum" \
    RATE_INITIAL_QP=26 \
    RATE_MIN_QP=10 \
    RATE_MAX_QP=51 \
        /tmp/rk_mpp_encoder_v19 \
            "$codec" nv12 "$c_api_out" 320 240 26 6 "$gop" "$fixture"

    if [ "$codec" = h264 ]; then
        target/release/rk-mpp-rs h264-enc \
            "$fixture" "$cli_out" 320 240 26 baseline 6 6 \
            "$mode:$target:$minimum:$maximum" qprc:10:51
    else
        target/release/rk-mpp-rs h265-rate \
            "$fixture" "$cli_out" 320 240 6 \
            "bps:$target:$minimum:$maximum" 26 10 51 "$mode"
    fi

    cmp "$c_api_out" "$cli_out"
    printf 'PASS %s %s C API == CLI (%s bytes)\n' \
        "$codec" "$mode" "$(wc -c < "$c_api_out")"
}

for codec in h264 h265; do
    if [ "$codec" = h264 ]; then
        gop=6
    else
        gop=60
    fi
    for mode in cbr vbr avbr; do
        compare_case "$codec" "$mode" "$gop"
    done
done

printf 'PASS rk-mpp-rs V1.8 compatibility and V1.9 VEPU580 rate-control ABI\n'
