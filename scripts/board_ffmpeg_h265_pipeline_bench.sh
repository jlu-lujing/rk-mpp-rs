#!/usr/bin/env bash

# Compare vendor FFmpeg H.265 encoding with the Rust synchronous and
# byte-exact deferred paths.
set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"
export PATH="$HOME/.cargo/bin:$PATH"

samples=${SAMPLES:-5}
frames=${FRAMES:-300}
width=${WIDTH:-1280}
height=${HEIGHT:-720}
ffmpeg_bin=$(command -v ffmpeg)
work=$(mktemp -d /tmp/rk-mpp-h265-pipeline.XXXXXX)
overlay="$work/overlay"
trap 'rm -rf "$work"' EXIT

mkdir -p "$overlay"
cargo build --release --offline --lib
ln -s "$repo/target/release/librk_mpp_rs.so" \
    "$overlay/librockchip_mpp.so.1"

raw="$work/input.nv12"
"$ffmpeg_bin" -nostdin -hide_banner -loglevel error -y \
    -f lavfi -i "testsrc2=size=${width}x${height}:rate=30" \
    -frames:v "$frames" -pix_fmt nv12 -f rawvideo "$raw"

common=(-nostdin -hide_banner -loglevel error
    -f rawvideo -pix_fmt nv12 -s "${width}x${height}" -r 30 -i "$raw"
    -frames:v "$frames" -c:v hevc_rkmpp -rc_mode CQP -qp_init 26
    -g 60 -profile:v main -f hevc -y)

exact_frames=${EXACT_FRAMES:-60}
exact_common=(-nostdin -hide_banner -loglevel error
    -f rawvideo -pix_fmt nv12 -s "${width}x${height}" -r 30 -i "$raw"
    -frames:v "$exact_frames" -c:v hevc_rkmpp -rc_mode CQP -qp_init 26
    -g 60 -profile:v main -f hevc -y)

"$ffmpeg_bin" "${exact_common[@]}" "$work/exact-c.hevc"
LD_LIBRARY_PATH="$overlay" RKMPP_ENCODER_PIPELINE=0 \
    "$ffmpeg_bin" "${exact_common[@]}" "$work/exact-sync.hevc"
LD_LIBRARY_PATH="$overlay" RKMPP_ENCODER_PIPELINE_DEPTH=1 \
    "$ffmpeg_bin" "${exact_common[@]}" "$work/exact-d1.hevc"
LD_LIBRARY_PATH="$overlay" RKMPP_ENCODER_PIPELINE_DEPTH=2 \
    "$ffmpeg_bin" "${exact_common[@]}" "$work/exact-d2.hevc"
for mode in sync d1 d2; do
    if ! cmp -s "$work/exact-c.hevc" "$work/exact-$mode.hevc"; then
        printf 'FAIL path %s differs from vendor C\n' "$mode" >&2
        exit 1
    fi
done
printf 'exact %sf H.265 SHA-256: %s\n' "$exact_frames" \
    "$(sha256sum "$work/exact-c.hevc" | cut -d' ' -f1)"

TIMEFORMAT='%3R %3U %3S'
declare -A wall_values cpu_values

run_path() {
    local key=$1
    shift
    local time_file="$work/$key.time"
    { time "$@" >/dev/null 2>&1; } 2>"$time_file"
    read -r wall user sys <"$time_file"
    wall_values["$key"]+="$wall "
    cpu_values["$key"]+="$(awk -v user="$user" -v sys="$sys" \
        'BEGIN { print user + sys }') "
}

run_rust() {
    local depth=$1
    if [ "$depth" = sync ]; then
        env LD_LIBRARY_PATH="$overlay" RKMPP_ENCODER_PIPELINE=0 \
            "$ffmpeg_bin" "${common[@]}" /dev/null
    else
        env LD_LIBRARY_PATH="$overlay" RKMPP_ENCODER_PIPELINE_DEPTH="$depth" \
            "$ffmpeg_bin" "${common[@]}" /dev/null
    fi
}

for sample in $(seq 1 "$samples"); do
    run_path "c-$sample" "$ffmpeg_bin" "${common[@]}" /dev/null
    run_path "sync-$sample" run_rust sync
    run_path "d1-$sample" run_rust 1
    run_path "d2-$sample" run_rust 2
done

median() {
    printf '%s\n' "$@" | sort -n | awk '{ v[NR] = $1 } END { print v[int((NR + 1) / 2)] }'
}

printf '\n%-12s %10s %10s %13s %10s\n' path wall_s FPS CPU_s CPU_pct
for path in c sync d1 d2; do
    walls=()
    cpus=()
    for sample in $(seq 1 "$samples"); do
        read -ra values <<<"${wall_values["$path-$sample"]}"
        walls+=("${values[0]}")
        read -ra values <<<"${cpu_values["$path-$sample"]}"
        cpus+=("${values[0]}")
    done
    wall=$(median "${walls[@]}")
    cpu=$(median "${cpus[@]}")
    fps=$(awk -v frames="$frames" -v wall="$wall" 'BEGIN { printf "%.1f", frames / wall }')
    cpu_pct=$(awk -v cpu="$cpu" -v wall="$wall" 'BEGIN { printf "%.0f", cpu / wall * 100 }')
    printf '%-12s %10.3f %10s %13.3f %9s%%\n' "$path" "$wall" "$fps" "$cpu" "$cpu_pct"
done
