#!/usr/bin/env bash

set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"

export PATH="$HOME/.cargo/bin:$PATH"
mpp=${MPP_REF:-}
if [ -n "$mpp" ] &&
    [ -f "$mpp/inc/rk_mpi.h" ] &&
    [ -f "$mpp/build/mpp/librockchip_mpp.so.1" ]; then
    inc="$mpp/inc"
    lib="$mpp/build/mpp"
elif grep -qi rk3576 /proc/device-tree/compatible 2>/dev/null &&
    [ -f /usr/local/include/rockchip/rk_mpi.h ] &&
    [ -f /usr/local/lib/librockchip_mpp.so.1 ]; then
    inc=/usr/local/include/rockchip
    lib=/usr/local/lib
elif [ -f "$HOME/rk-mpp-oracles/mpp-old/inc/rk_mpi.h" ] &&
    [ -f "$HOME/rk-mpp-oracles/mpp-old/build/mpp/librockchip_mpp.so.1" ]; then
    inc="$HOME/rk-mpp-oracles/mpp-old/inc"
    lib="$HOME/rk-mpp-oracles/mpp-old/build/mpp"
elif [ -f /usr/local/include/rockchip/rk_mpi.h ] &&
    [ -f /usr/local/lib/librockchip_mpp.so.1 ]; then
    inc=/usr/local/include/rockchip
    lib=/usr/local/lib
else
    printf 'missing reference MPP headers or library\n' >&2
    exit 2
fi

rk3576=0
sei_off=()
if grep -qi rk3576 /proc/device-tree/compatible 2>/dev/null; then
    rk3576=1
    sei_off=(seioff)
fi
if grep -q 'bit_target_fix' "$inc/mpp_rc_defs.h"; then
    rc_has_bit_target_fix=1
else
    rc_has_bit_target_fix=0
fi

probe=/tmp/rk_mpp_custom_rc_probe
h264_probe=/tmp/rk_mpp_custom_rc_h264_probe
raw=/tmp/rk_mpp_custom_rc_2.nv12
vendor=/tmp/rk_mpp_custom_rc_vendor.265
ours=/tmp/rk_mpp_custom_rc_ours.265
vendor_log=/tmp/rk_mpp_custom_rc_vendor.log
ours_log=/tmp/rk_mpp_custom_rc_ours.log
h264_vendor=/tmp/rk_mpp_custom_rc_vendor.264
h264_ours=/tmp/rk_mpp_custom_rc_ours.264
h264_vendor_log=/tmp/rk_mpp_custom_rc_h264_vendor.log
h264_ours_log=/tmp/rk_mpp_custom_rc_h264_ours.log
h265_range_vendor=/tmp/rk_mpp_custom_rc_range_vendor.265
h265_range_ours=/tmp/rk_mpp_custom_rc_range_ours.265
h264_range_vendor=/tmp/rk_mpp_custom_rc_h264_range_vendor.264
h264_range_ours=/tmp/rk_mpp_custom_rc_h264_range_ours.264
overlay=$(mktemp -d /tmp/rk_mpp_custom_rc.XXXXXX)
trap 'rm -rf "$overlay"' EXIT

cargo build --release --offline --lib --bin rk-mpp-rs
ln -s "$repo/target/release/librk_mpp_rs.so" \
    "$overlay/librockchip_mpp.so.1"

cc -D_GNU_SOURCE -DCUSTOM_RC_HAS_BIT_TARGET_FIX="$rc_has_bit_target_fix" \
    -std=c11 -O2 -Wall -Wextra -Werror -pthread \
    -I"$inc" "$repo/tools/h265e_probe.c" \
    "$lib/librockchip_mpp.so.1" -o "$probe"
cc -D_GNU_SOURCE -DCUSTOM_RC_HAS_BIT_TARGET_FIX="$rc_has_bit_target_fix" \
    -DH264E_PROBE_NO_ROI -std=c11 -O2 -Wall -Wextra -Werror \
    -pthread -I"$inc" "$repo/tools/h264e_probe.c" \
    "$lib/librockchip_mpp.so.1" -o "$h264_probe"

ffmpeg -nostdin -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=320x240:rate=30" -frames:v 2 \
    -pix_fmt nv12 -f rawvideo "$raw"

run_probe() {
    local library="$1"
    local output="$2"
    local log="$3"

    sudo -n rm -f "$output" "$log" "$output.yuv"
    local rate="288000:1500000:2500000"
    if [ "${CUSTOM_RC_MODE:-0}" != 0 ]; then
        rate="1000000:800000:1200000"
    fi
    sudo -n env CUSTOM_RC=1 CUSTOM_RC_LOG="$log" FRAMES=2 GOP=60 \
        CUSTOM_RC_QP_MIN="${CUSTOM_RC_QP_MIN:-26}" \
        CUSTOM_RC_QP_MAX="${CUSTOM_RC_QP_MAX:-26}" \
        LD_LIBRARY_PATH="$library" timeout 60 \
        "$probe" "$raw" "$output" 320 240 "${CUSTOM_RC_MODE:-0}" 26 \
        "$rate" "${sei_off[@]}" \
        < /dev/null > "$output.stdout" 2>&1
}

compare_callback_trace() {
    local vendor_log="$1"
    local ours_log="$2"
    local label="$3"
    local vendor_init
    local ours_init
    local vendor_trace
    local ours_trace

    vendor_init=$(awk '$1 == "init" { line = $0 } END { print line }' "$vendor_log")
    ours_init=$(awk '$1 == "init" { line = $0 } END { print line }' "$ours_log")
    test -n "$vendor_init"
    test "$vendor_init" = "$ours_init" || {
        printf 'FAIL %s init config differs\nvendor: %s\nours:   %s\n' \
            "$label" "$vendor_init" "$ours_init"
        return 1
    }
    vendor_trace=$(grep -E '^(drop|frame-start|hal-start|hal-end|reenc|frame-end) ' \
        "$vendor_log")
    ours_trace=$(grep -E '^(drop|frame-start|hal-start|hal-end|reenc|frame-end) ' \
        "$ours_log")
    test -n "$vendor_trace"
    test "$vendor_trace" = "$ours_trace" || {
        printf 'FAIL %s callback order differs\nvendor:\n%s\nours:\n%s\n' \
            "$label" "$vendor_trace" "$ours_trace"
        return 1
    }
}

run_probe "$lib" "$vendor" "$vendor_log"
run_probe "$overlay" "$ours" "$ours_log"

python3 "$repo/scripts/compare_streams.py" "$vendor" "$ours"
sudo -n "$repo/target/release/rk-mpp-rs" hevcdec "$vendor" "$vendor.yuv" \
    > /dev/null
sudo -n "$repo/target/release/rk-mpp-rs" hevcdec "$ours" "$ours.yuv" \
    > /dev/null
cmp "$vendor.yuv" "$ours.yuv"
for stage in "init " "frame-start " "hal-start " "hal-end " "reenc " "frame-end "; do
    grep -q "^$stage" "$vendor_log"
    grep -q "^$stage" "$ours_log"
done
test "$(grep -c '^frame-start ' "$ours_log")" -eq 2
test "$(grep -c '^frame-end ' "$ours_log")" -eq 2
compare_callback_trace "$vendor_log" "$ours_log" H.265
printf 'PASS custom RC callbacks drive 2-frame H.265 stream (%s)\n' \
    "$(sha256sum "$ours.yuv" | cut -d' ' -f1)"

run_h264_probe() {
    local library="$1"
    local output="$2"
    local log="$3"

    local rate="288000:1500000:2500000"
    if [ "${CUSTOM_RC_MODE:-0}" != 0 ]; then
        rate="1000000:800000:1200000"
    fi
    sudo -n rm -f "$output" "$log" "$output.yuv"
    sudo -n env CUSTOM_RC=1 CUSTOM_RC_LOG="$log" \
        CUSTOM_RC_QP_MIN="${CUSTOM_RC_QP_MIN:-26}" \
        CUSTOM_RC_QP_MAX="${CUSTOM_RC_QP_MAX:-26}" \
        LD_LIBRARY_PATH="$library" timeout 60 \
        "$h264_probe" "$raw" "$output" 320 240 2 "${CUSTOM_RC_MODE:-0}" 26 \
        "$rate" "${sei_off[@]}" \
        < /dev/null > "$output.stdout" 2>&1
}

run_h264_probe "$lib" "$h264_vendor" "$h264_vendor_log"
run_h264_probe "$overlay" "$h264_ours" "$h264_ours_log"
python3 "$repo/scripts/compare_streams.py" "$h264_vendor" "$h264_ours"
sudo -n "$repo/target/release/rk-mpp-rs" h264dec \
    "$h264_vendor" "$h264_vendor.yuv" > /dev/null
sudo -n "$repo/target/release/rk-mpp-rs" h264dec \
    "$h264_ours" "$h264_ours.yuv" > /dev/null
cmp "$h264_vendor.yuv" "$h264_ours.yuv"
for stage in "init " "frame-start " "hal-start " "hal-end " "reenc " "frame-end "; do
    grep -q "^$stage" "$h264_vendor_log"
    grep -q "^$stage" "$h264_ours_log"
done
test "$(grep -c '^frame-start ' "$h264_ours_log")" -eq 2
test "$(grep -c '^frame-end ' "$h264_ours_log")" -eq 2
compare_callback_trace "$h264_vendor_log" "$h264_ours_log" H.264
printf 'PASS custom RC callbacks drive 2-frame H.264 stream (%s)\n' \
    "$(sha256sum "$h264_ours.yuv" | cut -d' ' -f1)"

CUSTOM_RC_MODE=1 CUSTOM_RC_QP_MIN=10 CUSTOM_RC_QP_MAX=51 \
    run_probe "$lib" "$h265_range_vendor" /tmp/rk_mpp_custom_rc_h265_range_vendor.log
CUSTOM_RC_MODE=1 CUSTOM_RC_QP_MIN=10 CUSTOM_RC_QP_MAX=51 \
    run_probe "$overlay" "$h265_range_ours" /tmp/rk_mpp_custom_rc_h265_range_ours.log
python3 "$repo/scripts/compare_streams.py" \
    "$h265_range_vendor" "$h265_range_ours"
sudo -n "$repo/target/release/rk-mpp-rs" hevcdec \
    "$h265_range_vendor" "$h265_range_vendor.yuv" > /dev/null
sudo -n "$repo/target/release/rk-mpp-rs" hevcdec \
    "$h265_range_ours" "$h265_range_ours.yuv" > /dev/null
cmp "$h265_range_vendor.yuv" "$h265_range_ours.yuv"
printf 'PASS custom RC callback drives the H.265 RC register path (%s)\n' \
    "$(sha256sum "$h265_range_ours.yuv" | cut -d' ' -f1)"

CUSTOM_RC_MODE=1 CUSTOM_RC_QP_MIN=10 CUSTOM_RC_QP_MAX=51 \
    run_h264_probe "$lib" "$h264_range_vendor" /tmp/rk_mpp_custom_rc_h264_range_vendor.log
CUSTOM_RC_MODE=1 CUSTOM_RC_QP_MIN=10 CUSTOM_RC_QP_MAX=51 \
    run_h264_probe "$overlay" "$h264_range_ours" /tmp/rk_mpp_custom_rc_h264_range_ours.log
python3 "$repo/scripts/compare_streams.py" \
    "$h264_range_vendor" "$h264_range_ours"
sudo -n "$repo/target/release/rk-mpp-rs" h264dec \
    "$h264_range_vendor" "$h264_range_vendor.yuv" > /dev/null
sudo -n "$repo/target/release/rk-mpp-rs" h264dec \
    "$h264_range_ours" "$h264_range_ours.yuv" > /dev/null
cmp "$h264_range_vendor.yuv" "$h264_range_ours.yuv"
printf 'PASS custom RC callback drives the H.264 RC register path (%s)\n' \
    "$(sha256sum "$h264_range_ours.yuv" | cut -d' ' -f1)"

for spec in 2:vbr 3:avbr; do
    mode=${spec%%:*}
    label=${spec##*:}
    h265_vendor=/tmp/rk_mpp_custom_rc_h265_${label}_vendor.265
    h265_ours=/tmp/rk_mpp_custom_rc_h265_${label}_ours.265
    h265_vendor_log=/tmp/rk_mpp_custom_rc_h265_${label}_vendor.log
    h265_ours_log=/tmp/rk_mpp_custom_rc_h265_${label}_ours.log
    h264_vendor=/tmp/rk_mpp_custom_rc_h264_${label}_vendor.264
    h264_ours=/tmp/rk_mpp_custom_rc_h264_${label}_ours.264
    h264_vendor_log=/tmp/rk_mpp_custom_rc_h264_${label}_vendor.log
    h264_ours_log=/tmp/rk_mpp_custom_rc_h264_${label}_ours.log

    CUSTOM_RC_MODE="$mode" CUSTOM_RC_QP_MIN=10 CUSTOM_RC_QP_MAX=51 \
        run_probe "$lib" "$h265_vendor" "$h265_vendor_log"
    CUSTOM_RC_MODE="$mode" CUSTOM_RC_QP_MIN=10 CUSTOM_RC_QP_MAX=51 \
        run_probe "$overlay" "$h265_ours" "$h265_ours_log"
    python3 "$repo/scripts/compare_streams.py" "$h265_vendor" "$h265_ours"
    sudo -n "$repo/target/release/rk-mpp-rs" hevcdec \
        "$h265_vendor" "$h265_vendor.yuv" > /dev/null
    sudo -n "$repo/target/release/rk-mpp-rs" hevcdec \
        "$h265_ours" "$h265_ours.yuv" > /dev/null
    cmp "$h265_vendor.yuv" "$h265_ours.yuv"
    compare_callback_trace "$h265_vendor_log" "$h265_ours_log" "H.265 $label"
    printf 'PASS custom RC H.265 %s (%s)\n' "$label" \
        "$(sha256sum "$h265_ours.yuv" | cut -d' ' -f1)"

    CUSTOM_RC_MODE="$mode" CUSTOM_RC_QP_MIN=10 CUSTOM_RC_QP_MAX=51 \
        run_h264_probe "$lib" "$h264_vendor" "$h264_vendor_log"
    CUSTOM_RC_MODE="$mode" CUSTOM_RC_QP_MIN=10 CUSTOM_RC_QP_MAX=51 \
        run_h264_probe "$overlay" "$h264_ours" "$h264_ours_log"
    python3 "$repo/scripts/compare_streams.py" "$h264_vendor" "$h264_ours"
    sudo -n "$repo/target/release/rk-mpp-rs" h264dec \
        "$h264_vendor" "$h264_vendor.yuv" > /dev/null
    sudo -n "$repo/target/release/rk-mpp-rs" h264dec \
        "$h264_ours" "$h264_ours.yuv" > /dev/null
    cmp "$h264_vendor.yuv" "$h264_ours.yuv"
    compare_callback_trace "$h264_vendor_log" "$h264_ours_log" "H.264 $label"
    printf 'PASS custom RC H.264 %s (%s)\n' "$label" \
        "$(sha256sum "$h264_ours.yuv" | cut -d' ' -f1)"
done
