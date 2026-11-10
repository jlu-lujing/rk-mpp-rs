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

rk3576=0
case "$(tr '\0' '\n' < /proc/device-tree/compatible)" in
    *rockchip,rk3576*)
        rk3576=1
        mpp_inc=/usr/local/include/rockchip
        mpp_lib=/usr/local/lib
        cc -D_GNU_SOURCE -DH264E_PROBE_NO_ROI -std=c11 -O2 -Wall -Wextra -Werror \
            -pthread -I"$mpp_inc" -I"$repo/tools" "$repo/tools/h264e_probe.c" \
            "$mpp_lib/librockchip_mpp.so.1" -o /tmp/rk_mpp_sdk_h264_probe
        cc -D_GNU_SOURCE -std=c11 -O2 -Wall -Wextra -Werror \
            -pthread -I"$mpp_inc" -I"$repo/tools" "$repo/tools/h265e_probe.c" \
            "$mpp_lib/librockchip_mpp.so.1" -o /tmp/rk_mpp_sdk_h265_probe
        ;;
esac

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

    if [ "$rk3576" -eq 1 ]; then
        case "$mode" in
            cbr) rc_mode=1 ;;
            vbr) rc_mode=2 ;;
            avbr) rc_mode=3 ;;
        esac
        if [ "$codec" = h264 ]; then
            sudo -n env LD_LIBRARY_PATH="$mpp_lib" timeout 120 \
                /tmp/rk_mpp_sdk_h264_probe "$fixture" "$cli_out" \
                320 240 6 "$rc_mode" 26 seioff gop "$gop"
        else
            sudo -n env FRAMES=6 GOP=60 LD_LIBRARY_PATH="$mpp_lib" timeout 120 \
                /tmp/rk_mpp_sdk_h265_probe "$fixture" "$cli_out" \
                320 240 "$rc_mode" 26
        fi
    elif [ "$codec" = h264 ]; then
        target/release/rk-mpp-rs h264-enc \
            "$fixture" "$cli_out" 320 240 26 baseline 6 6 \
            "$mode:$target:$minimum:$maximum" qprc:10:51
    else
        target/release/rk-mpp-rs h265-rate \
            "$fixture" "$cli_out" 320 240 6 \
            "bps:$target:$minimum:$maximum" 26 10 51 "$mode"
    fi

    cmp "$c_api_out" "$cli_out"
    printf 'PASS %s %s C API == %s (%s bytes)\n' \
        "$codec" "$mode" \
        "$(if [ "$rk3576" -eq 1 ]; then printf vendor; else printf CLI; fi)" \
        "$(wc -c < "$c_api_out")"
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

printf 'PASS rk-mpp-rs V1.8 compatibility and V1.9 VEPU580/VEPU510 rate-control ABI\n'
