#!/usr/bin/env bash

set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"

export PATH="$HOME/.cargo/bin:$PATH"
mpp=${MPP_REF:-$HOME/rk-mpp-oracles/mpp-old}
if [ -f "$mpp/inc/rk_mpi.h" ] && [ -f "$mpp/build/mpp/librockchip_mpp.so.1" ]; then
    inc="$mpp/inc"
    lib="$mpp/build/mpp"
elif [ -f /usr/local/include/rockchip/rk_mpi.h ] &&
    [ -f /usr/local/lib/librockchip_mpp.so.1 ]; then
    inc=/usr/local/include/rockchip
    lib=/usr/local/lib
else
    printf 'missing reference MPP headers or library\n' >&2
    exit 2
fi

rk3576=0
if grep -qi rk3576 /proc/device-tree/compatible 2>/dev/null; then
    rk3576=1
fi

raw=/tmp/rk_mpp_packet_meta_2.nv12
raw6=/tmp/rk_mpp_packet_meta_6.nv12
still_one=/tmp/rk_mpp_packet_meta_still.nv12
still=/tmp/rk_mpp_packet_meta_still_2.nv12
overlay=$(mktemp -d /tmp/rk_mpp_packet_meta.XXXXXX)
trap 'rm -rf "$overlay"' EXIT

if [ "${SKIP_BUILD:-0}" != 1 ]; then
    cargo build --release --offline --lib
fi
ln -s "$repo/target/release/librk_mpp_rs.so" \
    "$overlay/librockchip_mpp.so.1"

h264_meta_flag=()
h265_meta_flag=()
if ! grep -q bit_target_fix "$inc/mpp_rc_defs.h"; then
    h264_meta_flag+=(-DCUSTOM_RC_HAS_BIT_TARGET_FIX=0)
    h265_meta_flag+=(-DCUSTOM_RC_HAS_BIT_TARGET_FIX=0)
fi
if [ "$rk3576" -eq 1 ]; then
    h264_meta_flag+=(-DH264E_DUMP_RT_BITS)
    h265_meta_flag+=(-DH265E_DUMP_RT_BITS)
fi
cc -D_GNU_SOURCE -DH264E_PROBE_NO_ROI "${h264_meta_flag[@]}" \
    -std=c11 -O2 -Wall -Wextra -Werror \
    -pthread -I"$inc" -I"$repo/tools" "$repo/tools/h264e_probe.c" \
    "$lib/librockchip_mpp.so.1" -o /tmp/rk_mpp_packet_meta_h264
cc -D_GNU_SOURCE "${h265_meta_flag[@]}" -std=c11 -O2 -Wall -Wextra -Werror \
    -pthread -I"$inc" -I"$repo/tools" "$repo/tools/h265e_probe.c" \
    "$lib/librockchip_mpp.so.1" -o /tmp/rk_mpp_packet_meta_h265
cc -D_GNU_SOURCE -std=c11 -O2 -Wall -Wextra -Werror \
    -pthread -I"$inc" "$repo/tools/jpege_probe.c" \
    "$lib/librockchip_mpp.so.1" -o /tmp/rk_mpp_packet_meta_jpeg

ffmpeg -nostdin -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=320x240:rate=30" -frames:v 2 \
    -pix_fmt nv12 -f rawvideo "$raw"
ffmpeg -nostdin -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=320x240:rate=30" -frames:v 6 \
    -pix_fmt nv12 -f rawvideo "$raw6"
dd if="$raw" of="$still_one" bs=115200 count=1 status=none
cp "$still_one" "$still"
dd if="$still_one" of="$still" bs=115200 seek=1 conv=notrunc status=none

run_h264() {
    local library="$1"
    local prefix="$2"
    local mode="$3"
    local input="${4:-}"
    local frames="${5:-2}"
    local width="${6:-320}"
    local height="${7:-240}"

    if [ -z "$input" ]; then
        input="$raw"
        if [ "$mode" -ne 0 ]; then
            input="$still"
        fi
    fi

    sudo -n rm -f "$prefix.264" "$prefix.log" "$prefix.264.yuv"
    sudo -n env DUMP_META=1 RKMPP_ENCODER_PIPELINE=0 LD_LIBRARY_PATH="$library" \
        /tmp/rk_mpp_packet_meta_h264 "$input" "$prefix.264" \
        "$width" "$height" "$frames" "$mode" 26 seioff \
        > "$prefix.stdout" 2> "$prefix.log"
    grep ' meta:' "$prefix.log" > "$prefix.meta"
}

run_h265() {
    local library="$1"
    local prefix="$2"
    local mode="$3"
    local input="${4:-}"
    local frames="${5:-2}"
    local width="${6:-320}"
    local height="${7:-240}"

    if [ -z "$input" ]; then
        input="$raw"
        if [ "$mode" -ne 0 ]; then
            input="$still"
        fi
    fi

    sudo -n rm -f "$prefix.265" "$prefix.log" "$prefix.265.yuv"
    sudo -n env DUMP_META=1 FRAMES="$frames" GOP=60 RKMPP_ENCODER_PIPELINE=0 \
        LD_LIBRARY_PATH="$library" \
        /tmp/rk_mpp_packet_meta_h265 "$input" "$prefix.265" \
        "$width" "$height" "$mode" 26 seioff \
        > "$prefix.stdout" 2> "$prefix.log"
    grep ' meta:' "$prefix.log" > "$prefix.meta"
}

run_jpeg() {
    local library="$1"
    local prefix="$2"
    local input="${3:-$still_one}"
    local width="${4:-320}"
    local height="${5:-240}"

    sudo -n rm -f "$prefix.jpg" "$prefix.log"
    sudo -n env DUMP_META=1 LD_LIBRARY_PATH="$library" \
        /tmp/rk_mpp_packet_meta_jpeg "$input" "$prefix.jpg" \
        "$width" "$height" 80 0 0 \
        > "$prefix.stdout" 2> "$prefix.log"
    grep ' meta:' "$prefix.log" > "$prefix.meta"
}

modes=(0 1)
if [ "$rk3576" -eq 1 ]; then
    modes=(0 1 2 3)
fi
for mode in "${modes[@]}"; do
    vendor_h264="/tmp/rk_mpp_packet_meta_vendor_h264_m${mode}"
    ours_h264="/tmp/rk_mpp_packet_meta_ours_h264_m${mode}"
    run_h264 "$lib" "$vendor_h264" "$mode"
    run_h264 "$overlay" "$ours_h264" "$mode"
    cmp "$vendor_h264.meta" "$ours_h264.meta"
    cmp "$vendor_h264.264" "$ours_h264.264"
    printf 'PASS H.264 packet meta mode %d: 2/2 pictures\n' "$mode"

    vendor_h265="/tmp/rk_mpp_packet_meta_vendor_h265_m${mode}"
    ours_h265="/tmp/rk_mpp_packet_meta_ours_h265_m${mode}"
    run_h265 "$lib" "$vendor_h265" "$mode"
    run_h265 "$overlay" "$ours_h265" "$mode"
    cmp "$vendor_h265.meta" "$ours_h265.meta"
    cmp "$vendor_h265.265" "$ours_h265.265"
    sudo -n "$repo/target/release/rk-mpp-rs" hevcdec \
        "$vendor_h265.265" "$vendor_h265.265.yuv" > /dev/null
    sudo -n "$repo/target/release/rk-mpp-rs" hevcdec \
        "$ours_h265.265" "$ours_h265.265.yuv" > /dev/null
    cmp "$vendor_h265.265.yuv" "$ours_h265.265.yuv"
    printf 'PASS H.265 packet meta mode %d: 2/2 pictures\n' "$mode"
done

if [ "$rk3576" -eq 1 ]; then
    vendor_h264=/tmp/rk_mpp_packet_meta_vendor_h264_still
    ours_h264=/tmp/rk_mpp_packet_meta_ours_h264_still
    run_h264 "$lib" "$vendor_h264" 0 "$still"
    run_h264 "$overlay" "$ours_h264" 0 "$still"
    cmp "$vendor_h264.meta" "$ours_h264.meta"
    cmp "$vendor_h264.264" "$ours_h264.264"
    printf 'PASS VEPU510 H.264 fixed-QP still packet meta: 2/2 pictures\n'

    vendor_h265=/tmp/rk_mpp_packet_meta_vendor_h265_still
    ours_h265=/tmp/rk_mpp_packet_meta_ours_h265_still
    run_h265 "$lib" "$vendor_h265" 0 "$still"
    run_h265 "$overlay" "$ours_h265" 0 "$still"
    cmp "$vendor_h265.meta" "$ours_h265.meta"
    cmp "$vendor_h265.265" "$ours_h265.265"
    sudo -n "$repo/target/release/rk-mpp-rs" hevcdec \
        "$vendor_h265.265" "$vendor_h265.265.yuv" > /dev/null
    sudo -n "$repo/target/release/rk-mpp-rs" hevcdec \
        "$ours_h265.265" "$ours_h265.265.yuv" > /dev/null
    cmp "$vendor_h265.265.yuv" "$ours_h265.265.yuv"
    printf 'PASS VEPU510 H.265 fixed-QP still packet meta: 2/2 pictures\n'

    for codec in h264 h265; do
        for mode in 1 2 3; do
            vendor="/tmp/rk_mpp_packet_meta_vendor_${codec}_m${mode}_6f"
            ours="/tmp/rk_mpp_packet_meta_ours_${codec}_m${mode}_6f"
            if [ "$codec" = h264 ]; then
                run_h264 "$lib" "$vendor" "$mode" "$raw6" 6
                run_h264 "$overlay" "$ours" "$mode" "$raw6" 6
                cmp "$vendor.meta" "$ours.meta"
                cmp "$vendor.264" "$ours.264"
            else
                run_h265 "$lib" "$vendor" "$mode" "$raw6" 6
                run_h265 "$overlay" "$ours" "$mode" "$raw6" 6
                cmp "$vendor.meta" "$ours.meta"
                cmp "$vendor.265" "$ours.265"
                sudo -n "$repo/target/release/rk-mpp-rs" hevcdec \
                    "$vendor.265" "$vendor.265.yuv" > /dev/null
                sudo -n "$repo/target/release/rk-mpp-rs" hevcdec \
                    "$ours.265" "$ours.265.yuv" > /dev/null
                cmp "$vendor.265.yuv" "$ours.265.yuv"
            fi
            printf 'PASS VEPU510 %s RC mode %d: 6/6 pictures\n' "$codec" "$mode"
        done
    done
fi

if [ "$rk3576" -eq 0 ]; then
    for codec in h264 h265; do
        vendor="/tmp/rk_mpp_packet_meta_vendor_${codec}_cbr_motion"
        ours="/tmp/rk_mpp_packet_meta_ours_${codec}_cbr_motion"
        if [ "$codec" = h264 ]; then
            run_h264 "$lib" "$vendor" 1 "$raw"
            run_h264 "$overlay" "$ours" 1 "$raw"
            cmp "$vendor.meta" "$ours.meta"
            cmp "$vendor.264" "$ours.264"
        else
            run_h265 "$lib" "$vendor" 1 "$raw"
            run_h265 "$overlay" "$ours" 1 "$raw"
            cmp "$vendor.meta" "$ours.meta"
            cmp "$vendor.265" "$ours.265"
            sudo -n "$repo/target/release/rk-mpp-rs" hevcdec \
                "$vendor.265" "$vendor.265.yuv" > /dev/null
            sudo -n "$repo/target/release/rk-mpp-rs" hevcdec \
                "$ours.265" "$ours.265.yuv" > /dev/null
            cmp "$vendor.265.yuv" "$ours.265.yuv"
        fi
        printf 'PASS %s CBR moving packet meta: 2/2 pictures\n' "$codec"
    done
fi

vendor_jpeg=/tmp/rk_mpp_packet_meta_vendor_jpeg
ours_jpeg=/tmp/rk_mpp_packet_meta_ours_jpeg
run_jpeg "$lib" "$vendor_jpeg"
run_jpeg "$overlay" "$ours_jpeg"
cmp "$vendor_jpeg.meta" "$ours_jpeg.meta"
cmp "$vendor_jpeg.jpg" "$ours_jpeg.jpg"
printf 'PASS JPEG packet meta: 1/1 picture\n'

if [ "${RUN_4K:-0}" = 1 ]; then
    width=3840
    height=2160
    raw4k=/tmp/rk_mpp_packet_meta_4k_2.nv12
    still4k=/tmp/rk_mpp_packet_meta_4k_still.nv12
    ffmpeg -nostdin -y -hide_banner -loglevel error \
        -f lavfi -i "testsrc2=size=${width}x${height}:rate=30" -frames:v 2 \
        -pix_fmt nv12 -f rawvideo "$raw4k"
    frame_bytes=$((width * height * 3 / 2))
    dd if="$raw4k" of="$still4k" bs="$frame_bytes" count=1 status=none

    mode=0
    for codec in h264 h265; do
        vendor="/tmp/rk_mpp_packet_meta_vendor_${codec}_4k_m${mode}"
        ours="/tmp/rk_mpp_packet_meta_ours_${codec}_4k_m${mode}"
        if [ "$codec" = h264 ]; then
            run_h264 "$lib" "$vendor" "$mode" "$raw4k" 2 "$width" "$height"
            run_h264 "$overlay" "$ours" "$mode" "$raw4k" 2 "$width" "$height"
            cmp "$vendor.meta" "$ours.meta"
            cmp "$vendor.264" "$ours.264"
        else
            run_h265 "$lib" "$vendor" "$mode" "$raw4k" 2 "$width" "$height"
            run_h265 "$overlay" "$ours" "$mode" "$raw4k" 2 "$width" "$height"
            cmp "$vendor.meta" "$ours.meta"
            cmp "$vendor.265" "$ours.265"
            sudo -n "$repo/target/release/rk-mpp-rs" hevcdec \
                "$vendor.265" "$vendor.265.yuv" > /dev/null
            sudo -n "$repo/target/release/rk-mpp-rs" hevcdec \
                "$ours.265" "$ours.265.yuv" > /dev/null
            cmp "$vendor.265.yuv" "$ours.265.yuv"
        fi
        printf 'PASS 4K %s fixed-QP packet meta: 2/2 pictures\n' "$codec"
    done

    vendor_jpeg=/tmp/rk_mpp_packet_meta_vendor_jpeg_4k
    ours_jpeg=/tmp/rk_mpp_packet_meta_ours_jpeg_4k
    run_jpeg "$lib" "$vendor_jpeg" "$still4k" "$width" "$height"
    run_jpeg "$overlay" "$ours_jpeg" "$still4k" "$width" "$height"
    cmp "$vendor_jpeg.meta" "$ours_jpeg.meta"
    # The unchanged probe takes the vendor's automatic 4K restart-band path,
    # while the Rust MPI object path intentionally keeps one image unless split
    # is requested. Packet metadata is the comparable surface here; the direct
    # 4K JPEG stream layout gate lives in board_4k_matrix.sh.
    printf 'PASS 4K JPEG packet meta: 1/1 picture (layout differs by design)\n'
fi
