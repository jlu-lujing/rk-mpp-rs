#!/usr/bin/env bash

set -euo pipefail

stream="${STREAM:-/tmp/test720.264}"
cmd="${CMD:-h264dec}"
coding="${CODING:-7}"
width="${WIDTH:-1280}"
height="${HEIGHT:-720}"
frames="${FRAMES:-120}"
stream_counts="${STREAM_COUNTS:-1 2 4 6}"
samples="${SAMPLES:-3}"
mpp_bin="${MPP_BIN:-/tmp/mpp-develop/build/test/mpi_dec_test}"
mpp_lib="${MPP_LIB:-/tmp/mpp-develop/build/mpp}"
rust_bin="${RUST_BIN:-$HOME/rk-mpp-rs/target/release/rk-mpp-rs}"
rust_dmabuf="${RUST_DMABUF:-0}"

if [ ! -f "$stream" ]; then
    printf 'missing stream: %s\n' "$stream" >&2
    exit 2
fi
if [ ! -x "$mpp_bin" ] || [ ! -x "$rust_bin" ]; then
    printf 'missing decoder binary: MPP=%s rust=%s\n' "$mpp_bin" "$rust_bin" >&2
    exit 2
fi

median() {
    local n
    n=$(printf '%s\n' "$@" | sort -n | wc -l)
    printf '%s\n' "$@" | sort -n | sed -n "$((n / 2 + 1))p"
}

run_once() {
    local backend="$1"
    local count="$2"
    local work
    local start
    local elapsed
    local failures=0
    local pid
    local i
    local -a pids=()

    work=$(mktemp -d /tmp/rk_mpp_multistream.XXXXXX)
    start=$(date +%s%N)
    for ((i = 0; i < count; i++)); do
        if [ "$backend" = c ]; then
            (
                cd "$(dirname "$mpp_bin")"
                sudo -n env LD_LIBRARY_PATH="$mpp_lib:${LD_LIBRARY_PATH:-}" \
                    timeout 180 "$mpp_bin" -i "$stream" -o /dev/null \
                    -w "$width" -h "$height" -t "$coding" -n "$frames" -v q
            ) >"$work/c-$i.log" 2>&1 &
        else
            if [ "$rust_dmabuf" = 1 ]; then
                sudo -n env RKMPP_DECODE_DMABUF=1 \
                    timeout 180 "$rust_bin" "$cmd" "$stream" /dev/null \
                    >"$work/rust-$i.log" 2>&1 &
            else
                sudo -n timeout 180 "$rust_bin" "$cmd" "$stream" /dev/null \
                    >"$work/rust-$i.log" 2>&1 &
            fi
        fi
        pids+=("$!")
    done
    for pid in "${pids[@]}"; do
        wait "$pid" || failures=$((failures + 1))
    done
    elapsed=$((($(date +%s%N) - start) / 1000000))
    if [ "$failures" -ne 0 ]; then
        printf '%s\n' "$work" >&2
        printf '%s streams failed for %s\n' "$failures" "$backend" >&2
        return 1
    fi
    rm -rf "$work"
    printf '%s\n' "$elapsed"
}

printf 'stream=%s codec=%s geometry=%sx%s frames=%s samples=%s\n' \
    "$stream" "$cmd" "$width" "$height" "$frames" "$samples"
for count in $stream_counts; do
    c_values=()
    rust_values=()
    for _ in $(seq 1 "$samples"); do
        c_values+=("$(run_once c "$count")")
        rust_values+=("$(run_once rust "$count")")
    done
    c_ms=$(median "${c_values[@]}")
    rust_ms=$(median "${rust_values[@]}")
    awk -v n="$count" -v f="$frames" -v c="$c_ms" -v r="$rust_ms" 'BEGIN {
        c_agg = n * f * 1000 / c
        r_agg = n * f * 1000 / r
        printf "%2d streams | C %8s ms %8.1f agg %7.1f each | Rust %8s ms %8.1f agg %7.1f each | Rust %+.1f%%\n",
            n, c, c_agg, c_agg / n, r, r_agg, r_agg / n, (c - r) / r * 100
    }'
done
