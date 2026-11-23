#!/usr/bin/env bash

set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"

export PATH="$HOME/.cargo/bin:$PATH"
mpp=${MPP_REF:-$HOME/rk-mpp-oracles/mpp-old}
lib="$mpp/build/mpp"
inc="$mpp/inc"
roi_src="$mpp/utils/mpp_enc_roi_utils.c"
size=${SIZE:-320x240}
width=${size%%x*}
height=${size##*x}
frames=${FRAMES:-6}
use_roi=1
probe=/tmp/rk_mpp_force_idr_probe
raw="/tmp/rk_mpp_force_idr_${width}x${height}_${frames}.nv12"
vendor="/tmp/rk_mpp_force_idr_vendor_${width}x${height}.264"
ours="/tmp/rk_mpp_force_idr_ours_${width}x${height}.264"
overlay=$(mktemp -d /tmp/rk_mpp_force_idr.XXXXXX)
trap 'rm -rf "$overlay"' EXIT

if [ ! -f "$inc/rk_mpi.h" ] || [ ! -f "$lib/librockchip_mpp.so.1" ]; then
    inc=/usr/local/include/rockchip
    lib=/usr/local/lib
    roi_src=""
    use_roi=0
fi
if [ ! -f "$inc/rk_mpi.h" ] || [ ! -f "$lib/librockchip_mpp.so.1" ]; then
    printf 'missing MPP headers/library for force-IDR probe\n' >&2
    exit 2
fi

if [ "${SKIP_BUILD:-0}" != 1 ]; then
    cargo build --release --offline --lib --bin rk-mpp-rs
fi
ln -s "$repo/target/release/librk_mpp_rs.so" "$overlay/librockchip_mpp.so.1"

abi_defs=()
if ! grep -q bit_target_fix "$inc/mpp_rc_defs.h"; then
    abi_defs+=(-DCUSTOM_RC_HAS_BIT_TARGET_FIX=0)
fi

if [ "$use_roi" = 1 ]; then
    cc -D_GNU_SOURCE -std=c11 -O2 -Wall -Wextra -Werror -pthread \
        "${abi_defs[@]}" \
        -I"$inc" -I"$mpp/utils" -I"$mpp/osal/inc" \
        "$roi_src" "$repo/tools/h264e_probe.c" \
        "$lib/librockchip_mpp.so.1" -o "$probe"
else
    cc -D_GNU_SOURCE -DH264E_PROBE_NO_ROI -std=c11 -O2 \
        -Wall -Wextra -Werror -pthread "${abi_defs[@]}" -I"$inc" \
        "$repo/tools/h264e_probe.c" \
        "$lib/librockchip_mpp.so.1" -o "$probe"
fi

ffmpeg -nostdin -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=${width}x${height}:rate=30" -frames:v "$frames" \
    -pix_fmt nv12 -f rawvideo "$raw"

sudo -n rm -f "$vendor" "$ours" "$vendor.yuv" "$ours.yuv"
sudo -n env LD_LIBRARY_PATH="$lib" timeout 60 "$probe" \
    "$raw" "$vendor" "$width" "$height" "$frames" 0 26 288000:1500000:2500000 \
    forceidr 2,4 headereach seioff < /dev/null > "$vendor.log" 2>&1
sudo -n env LD_LIBRARY_PATH="$overlay" RKMPP_ENCODER_PIPELINE=0 \
    timeout 60 "$probe" \
    "$raw" "$ours" "$width" "$height" "$frames" 0 26 288000:1500000:2500000 \
    forceidr 2,4 headereach seioff < /dev/null > "$ours.log" 2>&1

python3 "$repo/scripts/compare_streams.py" "$vendor" "$ours"
sudo -n "$repo/target/release/rk-mpp-rs" h264dec "$vendor" "$vendor.yuv" \
    > /dev/null
sudo -n "$repo/target/release/rk-mpp-rs" h264dec "$ours" "$ours.yuv" \
    > /dev/null
cmp "$vendor.yuv" "$ours.yuv"
printf 'PASS h264 forced IDR at frames 2,4 (%s, %sx%s, %s frames)\n' \
    "$(sha256sum "$ours.yuv" | cut -d' ' -f1)" "$width" "$height" "$frames"
