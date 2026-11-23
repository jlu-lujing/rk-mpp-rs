#!/usr/bin/env bash
# RK3588 4K JPEG decoder scale gate.
#
# jpegd performs the 1/2, 1/4 and 1/8 reduction in its IDCT path, so a generic
# decoder test that only asks for a full-size frame cannot cover this mode.
# This gate checks RGB888 and NV12 geometry, repeated-run determinism, and full
#/scaled RGB against FFmpeg's software pixel oracle.
set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"

: "${BOARD:?set BOARD to the SSH alias of an RK3588 board}"
export BOARD

if [ "${SKIP_BUILD:-0}" != 1 ]; then
    COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target \
        --exclude Cargo.lock --exclude .worktrees --exclude refs \
        --exclude '._*' -cf - . |
        ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
    ssh "$BOARD" \
        'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release'
fi

ssh "$BOARD" 'bash -s' <<'REMOTE'
set -euo pipefail

repo="$HOME/rk-mpp-rs"
cli="$repo/target/release/rk-mpp-rs"
work=$(mktemp -d /tmp/rk-mpp-rs-4k-jpegd.XXXXXX)
trap 'sudo -n rm -rf "$work"' EXIT

command -v ffmpeg >/dev/null
test -x "$cli"

width=3840
height=2160
passed=0

psnr_average() {
    local left="$1"
    local right="$2"
    local w="$3"
    local h="$4"
    ffmpeg -nostdin -hide_banner -loglevel info \
        -f rawvideo -pix_fmt rgb24 -s "${w}x${h}" -i "$left" \
        -f rawvideo -pix_fmt rgb24 -s "${w}x${h}" -i "$right" \
        -lavfi '[0:v][1:v]psnr' -f null - 2>&1 |
        sed -n 's/.*average:\([0-9.]*\).*/\1/p' |
        tail -1
}

ffmpeg -nostdin -hide_banner -loglevel error -y \
    -f lavfi -i "testsrc2=size=${width}x${height}:rate=1" \
    -frames:v 1 -q:v 2 "$work/source.jpg"

if ! ffprobe -v error -select_streams v:0 \
    -show_entries stream=width,height -of csv=p=0 "$work/source.jpg" |
    grep -Fx "${width},${height}" >/dev/null; then
    printf 'FAIL jpegd-scale fixture is not 4K\n' >&2
    exit 1
fi

for factor in 1 2 4 8; do
    out_width=$((width / factor))
    out_height=$((height / factor))
    rgb_bytes=$((out_width * out_height * 3))
    nv12_bytes=$((out_width * out_height * 3 / 2))

    for run in 1 2; do
        sudo -n "$cli" decode "$work/source.jpg" \
            "$work/rgb-${factor}-${run}.raw" "$factor" \
            >"$work/rgb-${factor}-${run}.log" 2>&1
        sudo -n env RKMPP_NV12=1 "$cli" decode "$work/source.jpg" \
            "$work/nv12-${factor}-${run}.raw" "$factor" \
            >"$work/nv12-${factor}-${run}.log" 2>&1
    done

    if ! cmp -s "$work/rgb-${factor}-1.raw" "$work/rgb-${factor}-2.raw"; then
        printf 'FAIL jpegd-scale rgb 1/%s is not deterministic\n' "$factor" >&2
        exit 1
    fi
    if ! cmp -s "$work/nv12-${factor}-1.raw" "$work/nv12-${factor}-2.raw"; then
        printf 'FAIL jpegd-scale nv12 1/%s is not deterministic\n' "$factor" >&2
        exit 1
    fi

    rgb_size=$(stat -c %s "$work/rgb-${factor}-1.raw")
    nv12_size=$(stat -c %s "$work/nv12-${factor}-1.raw")
    if [ "$rgb_size" -ne "$rgb_bytes" ] || [ "$nv12_size" -ne "$nv12_bytes" ]; then
        printf 'FAIL jpegd-scale geometry 1/%s rgb=%s/%s nv12=%s/%s\n' \
            "$factor" "$rgb_size" "$rgb_bytes" "$nv12_size" "$nv12_bytes" >&2
        exit 1
    fi

    # The hardware IDCT reduction is not the same filter as FFmpeg's generic
    # scaler. PSNR is used as a corruption oracle, not as a byte-exact claim.
    if [ "$factor" = 1 ]; then
        min_psnr=55
        scale_filter=area
    elif [ "$factor" = 8 ]; then
        min_psnr=25
        scale_filter=area
    else
        min_psnr=28
        scale_filter=area
    fi
    ffmpeg -nostdin -hide_banner -loglevel error -y -i "$work/source.jpg" \
        -vf "scale=${out_width}:${out_height}:flags=${scale_filter}" \
        -pix_fmt rgb24 -f rawvideo "$work/software-${factor}.rgb"
    psnr=$(psnr_average "$work/rgb-${factor}-1.raw" \
        "$work/software-${factor}.rgb" "$out_width" "$out_height")
    if [ -z "$psnr" ] || ! awk -v p="$psnr" -v min="$min_psnr" 'BEGIN { exit !(p >= min) }'; then
        printf 'FAIL jpegd-scale rgb 1/%s psnr=%s expected >=%s\n' \
            "$factor" "${psnr:-missing}" "$min_psnr" >&2
        exit 1
    fi

    hash=$(sha256sum "$work/rgb-${factor}-1.raw" | cut -d' ' -f1)
    printf 'PASS jpegd-scale 1/%-2s %4sx%-4s rgb=%s nv12=%s psnr=%s sha256=%s\n' \
        "$factor" "$out_width" "$out_height" "$rgb_size" "$nv12_size" \
        "$psnr" "${hash:0:16}"
    passed=$((passed + 1))
done

printf 'SUMMARY 4K JPEG decoder scale passed=%d failed=0\n' "$passed"
REMOTE
