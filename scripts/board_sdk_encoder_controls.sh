#!/usr/bin/env bash

set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"

export PATH="$HOME/.cargo/bin:$PATH"
cargo build --release --offline --lib --bin rk-mpp-rs

library="$repo/target/release/librk_mpp_rs.so"
fixture=/tmp/rk_mpp_sdk_controls_6.nv12
libdir=$(mktemp -d)
trap 'rm -rf "$libdir"' EXIT
ln -sfn "$library" "$libdir/librk_mpp_rs.so"
ln -sfn "$library" "$libdir/librk_mpp_rs.so.1"

ffmpeg -nostdin -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=320x240:rate=30" -frames:v 6 \
    -pix_fmt nv12 -f rawvideo "$fixture"

gcc -std=c11 -Wall -Wextra -Werror \
    -I"$repo/include" "$repo/tests/c_api_encoder.c" \
    -L"$libdir" -lrk_mpp_rs \
    -Wl,-rpath,"$libdir" \
    -o /tmp/rk_mpp_encoder_controls
gcc -std=c11 -Wall -Wextra -Werror \
    -I"$repo/tests/abi" "$repo/tests/c_api_encoder_v19.c" \
    -L"$libdir" -lrk_mpp_rs \
    -Wl,-rpath,"$libdir" \
    -o /tmp/rk_mpp_encoder_v19

compatible=$(tr '\0' '\n' < /proc/device-tree/compatible)
case "$compatible" in
    *rockchip,rk3576*)
        printf 'PASS V1.9 advanced consumer build; VEPU510 runtime controls platform SKIP\n'
        exit 0
        ;;
esac

/tmp/rk_mpp_encoder_v19 "$fixture" /tmp/rk_mpp_encoder_v19.264
test -s /tmp/rk_mpp_encoder_v19.264

compare_control() {
    codec=$1
    control=$2
    gop=$3
    c_api_out="/tmp/rk_mpp_sdk_${codec}_${control}.bin"
    cli_out="/tmp/rk_mpp_cli_${codec}_${control}.bin"

    case "$control" in
        slice)
            if [ "$codec" = h264 ]; then
                slice_arg=96
                cli_option="split:mb:$slice_arg splitout:lowdelay"
            else
                slice_arg=8
                cli_option="split:ctu:$slice_arg splitout:lowdelay"
            fi
            CONTROL_MODE=slice SLICE_MODE=ctu SLICE_ARG="$slice_arg" \
            SLICE_LOW_DELAY=1 \
                /tmp/rk_mpp_encoder_controls \
                    "$codec" nv12 "$c_api_out" 320 240 26 6 "$gop" "$fixture"
            ;;
        roi)
            CONTROL_MODE=roi ROI_X=32 ROI_Y=32 ROI_WIDTH=128 ROI_HEIGHT=96 \
            ROI_QP=-6 ROI_FORCE_INTRA=0 ROI_ABSOLUTE_QP=0 \
                /tmp/rk_mpp_encoder_controls \
                    "$codec" nv12 "$c_api_out" 320 240 26 6 "$gop" "$fixture"
            cli_option="roi:32:32:128:96:0:0:-6"
            ;;
        gdr)
            CONTROL_MODE=gdr GDR_MODE=row GDR_REFRESH=2 \
                /tmp/rk_mpp_encoder_controls \
                    "$codec" nv12 "$c_api_out" 320 240 26 6 "$gop" "$fixture"
            if [ "$codec" = h264 ]; then
                cli_option="gdr:row:2"
            else
                cli_option="row:2"
            fi
            ;;
        *)
            printf 'unknown control: %s\n' "$control" >&2
            exit 2
            ;;
    esac

    if [ "$codec" = h264 ]; then
        # shellcheck disable=SC2086
        target/release/rk-mpp-rs h264-enc \
            "$fixture" "$cli_out" 320 240 26 baseline 6 "$gop" $cli_option
    else
        # shellcheck disable=SC2086
        H265_GOP="$gop" target/release/rk-mpp-rs h265-gop \
            "$fixture" "$cli_out" 320 240 6 26 $cli_option
    fi

    cmp "$c_api_out" "$cli_out"
    printf 'PASS %s %s C API == CLI (%s bytes)\n' \
        "$codec" "$control" "$(wc -c < "$c_api_out")"
}

compare_h264_syntax() {
    local name=$1
    local c_api_out="/tmp/rk_mpp_sdk_h264_syntax_${name}.264"
    local cli_out="/tmp/rk_mpp_cli_h264_syntax_${name}.264"
    local -a c_env cli_options
    local profile=main

    case "$name" in
        cabac)
            c_env=(
                H264_PROFILE=77
                H264_CABAC=1
                H264_TRANS8X8=0
                H264_CABAC_IDC=2
            )
            cli_options=(cabac_idc:2)
            ;;
        constraint)
            c_env=(
                H264_PROFILE=77
                H264_CABAC=1
                H264_TRANS8X8=0
                H264_CONSTRAINT_SET=0x003f0025
            )
            cli_options=(constraint_set:0x003f0025)
            ;;
        base-pid)
            c_env=(H264_PROFILE=77 H264_BASE_LAYER_PID=5)
            cli_options=(base_layer_pid:5)
            ;;
        main-cavlc)
            c_env=(H264_PROFILE=77 H264_CABAC=0 H264_TRANS8X8=0)
            cli_options=(cabac:0 trans8x8:0)
            ;;
        high-no-8x8)
            profile=high
            c_env=(H264_PROFILE=100 H264_CABAC=1 H264_TRANS8X8=0)
            cli_options=(trans8x8:0)
            ;;
        deblock)
            c_env=(H264_PROFILE=77 DBLK_DISABLE=1 DBLK_ALPHA=-2 DBLK_BETA=3)
            cli_options=(dblk:1:-2:3)
            ;;
        *)
            printf 'unknown H.264 syntax case: %s\n' "$name" >&2
            exit 2
            ;;
    esac

    env CONTROL_MODE=h264-syntax "${c_env[@]}" \
        /tmp/rk_mpp_encoder_controls \
            h264 nv12 "$c_api_out" 320 240 26 6 6 "$fixture"
    target/release/rk-mpp-rs h264-enc \
        "$fixture" "$cli_out" 320 240 26 "$profile" 6 6 "${cli_options[@]}"
    cmp "$c_api_out" "$cli_out"
    printf 'PASS H.264 syntax %s C V3 == CLI (%s bytes)\n' \
        "$name" "$(wc -c < "$c_api_out")"
}

for codec in h264 h265; do
    compare_control "$codec" slice 6
    compare_control "$codec" roi 6
    compare_control "$codec" gdr 4
done
for syntax in cabac constraint base-pid main-cavlc high-no-8x8 deblock; do
    compare_h264_syntax "$syntax"
done

printf 'PASS rk-mpp-rs V1.13 ROI/GDR/slice/H.264 syntax controls\n'
