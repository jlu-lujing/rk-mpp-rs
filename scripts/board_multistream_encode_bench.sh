#!/usr/bin/env bash

# Compare concurrent vendor C and Rust FFmpeg encoder sessions.
set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"
export PATH="$HOME/.cargo/bin:$PATH"

codec=${CODEC:-h265}
frames=${FRAMES:-300}
width=${WIDTH:-1280}
height=${HEIGHT:-720}
samples=${SAMPLES:-3}
stream_counts=${STREAM_COUNTS:-"1 2 4 6 8"}
cpuset=${CPUSET:-4-7}
ffmpeg_bin=$(command -v ffmpeg)
work=$(mktemp -d /tmp/rk-mpp-multistream-encode.XXXXXX)
overlay="$work/overlay"
trap 'rm -rf "$work"' EXIT

mkdir "$overlay"
cargo build --release --offline --lib
ln -s "$repo/target/release/librk_mpp_rs.so" \
    "$overlay/librockchip_mpp.so.1"

case "$codec" in
h264)
    ff_codec=h264
    muxer=h264
    profile=high
    ;;
h265)
    ff_codec=hevc
    muxer=hevc
    profile=main
    ;;
*)
    printf 'CODEC must be h264 or h265, got %s\n' "$codec" >&2
    exit 2
    ;;
esac

if [[ "$cpuset" == *-* ]]; then
    IFS=- read -r first_cpu last_cpu <<<"$cpuset"
    cores=()
    for ((cpu = first_cpu; cpu <= last_cpu; cpu++)); do
        cores+=("$cpu")
    done
else
    IFS=',' read -ra cores <<<"$cpuset"
fi
if [ "${#cores[@]}" -eq 0 ]; then
    printf 'CPUSET must name at least one CPU\n' >&2
    exit 2
fi

raw="$work/input.nv12"
"$ffmpeg_bin" -nostdin -hide_banner -loglevel error -y \
    -f lavfi -i "testsrc2=size=${width}x${height}:rate=30" \
    -frames:v "$frames" -pix_fmt nv12 -f rawvideo "$raw"

run_once() {
    local backend=$1
    local count=$2
    local start_ns
    local elapsed_ms
    local i cpu
    local -a pids=()
    local -a command=(
        "$ffmpeg_bin" -nostdin -hide_banner -loglevel error
        -f rawvideo -pix_fmt nv12 -s "${width}x${height}" -r 30 -i "$raw"
        -frames:v "$frames" -c:v "${ff_codec}_rkmpp"
        -rc_mode CQP -qp_init 26 -g 60 -profile:v "$profile"
        -f "$muxer" -y /dev/null
    )

    start_ns=$(date +%s%N)
    for ((i = 0; i < count; i++)); do
        cpu=${cores[$((i % ${#cores[@]}))]}
        if [ "$backend" = rust ]; then
            LD_LIBRARY_PATH="$overlay" taskset -c "$cpu" \
                "${command[@]}" >/dev/null 2>&1 &
        else
            taskset -c "$cpu" "${command[@]}" >/dev/null 2>&1 &
        fi
        pids+=("$!")
    done
    for i in "${pids[@]}"; do
        wait "$i"
    done
    elapsed_ms=$((($(date +%s%N) - start_ns) / 1000000))
    printf '%s\n' "$elapsed_ms"
}

median() {
    printf '%s\n' "$@" | sort -n |
        awk '{ value[NR] = $1 } END { print value[int((NR + 1) / 2)] }'
}

printf '%s %sx%s frames=%s samples=%s cpus=%s\n' \
    "$codec" "$width" "$height" "$frames" "$samples" "$cpuset"
printf '%-10s %10s %10s %12s %12s %10s\n' \
    streams c_ms rust_ms aggregate per_stream rust_vs_c
for count in $stream_counts; do
    c_values=()
    rust_values=()
    for _ in $(seq 1 "$samples"); do
        c_values+=("$(run_once c "$count")")
        rust_values+=("$(run_once rust "$count")")
    done
    c_ms=$(median "${c_values[@]}")
    rust_ms=$(median "${rust_values[@]}")
    awk -v count="$count" -v frames="$frames" -v c="$c_ms" -v rust="$rust_ms" 'BEGIN {
        c_fps = count * frames * 1000 / c
        r_fps = count * frames * 1000 / rust
        printf "%-10s %10d %10d %12.1f %12.1f %+9.1f%%\n",
            count "-way", c, rust, r_fps, r_fps / count, (r_fps / c_fps - 1) * 100
    }'
done
