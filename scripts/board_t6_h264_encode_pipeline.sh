#!/usr/bin/env bash
# Reproduce the 1280x720 H.264 end-to-end throughput comparison used by the
# V1.12 DMA-input pipeline work. Run this script on t6 with the 80% OPP cap.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
rust_bin="${RUST_BIN:-$root/target/release/rk-mpp-rs}"
fixture="${FIXTURE:-/tmp/rk_mpp_fill_exact_720p_500.nv12}"
fixture_sha=db3d065df8ea78f9068aa397dfc6ed3bcd404d27df85b772c307d58aff556546
output_sha=0a039e02910169e9900349a16e676f580c3da9e265b154d15ebcbde3a2bc6a2e
mpp_ref="${MPP_REF:-/tmp/mpp-develop}"
mpp_bin="$mpp_ref/build/test/mpi_enc_test"
mpp_lib="$mpp_ref/build/mpp"
samples="${SAMPLES:-5}"
frames=500

median() {
    printf '%s\n' "$@" | sort -n | sed -n "$(( $# / 2 + 1 ))p"
}

fps_from_ms() {
    awk -v frames="$frames" -v ms="$1" 'BEGIN { printf "%.2f", frames * 1000 / ms }'
}

if [ "$(uname -m)" != aarch64 ] || [ ! -e /dev/mpp_service ]; then
    echo "run this benchmark on t6" >&2
    exit 2
fi
for path in "$rust_bin" "$mpp_bin" "$mpp_lib/librockchip_mpp.so.0"; do
    if [ ! -e "$path" ]; then
        echo "missing $path" >&2
        exit 2
    fi
done

big_cap="$(cat /sys/devices/system/cpu/cpufreq/policy6/scaling_max_freq)"
little_cap="$(cat /sys/devices/system/cpu/cpufreq/policy0/scaling_max_freq)"
if [ "$big_cap" -gt 1800000 ] || [ "$little_cap" -gt 1416000 ]; then
    echo "80% OPP cap is not active (little=$little_cap big=$big_cap)" >&2
    exit 2
fi

if [ ! -s "$fixture" ]; then
    echo "generating exact MPP fill fixture: $fixture"
    cc -O2 -o /tmp/rk_mpp_nv12_fill "$root/tools/mpp_nv12_fill.c"
    /tmp/rk_mpp_nv12_fill 1280 720 "$frames" "$fixture"
fi
test "$(stat -c %s "$fixture")" -eq 691200000
actual_fixture_sha="$(sha256sum "$fixture" | cut -d' ' -f1)"
if [ "$actual_fixture_sha" != "$fixture_sha" ]; then
    echo "fixture SHA mismatch: $actual_fixture_sha" >&2
    exit 1
fi

echo "fixture: $fixture"
echo "OPP cap: little=$little_cap big=$big_cap"
echo "samples: $samples"

echo "checking byte identity through the two-core pipeline"
RKMPP_INPUT_CPU=7 taskset -c 6,7 "$rust_bin" h264-enc \
    "$fixture" /tmp/rk_mpp_pipeline_check.264 1280 720 26 high "$frames" 60
actual_output_sha="$(sha256sum /tmp/rk_mpp_pipeline_check.264 | cut -d' ' -f1)"
test "$actual_output_sha" = "$output_sha"
echo "output SHA-256: $actual_output_sha"

c_values=()
single_values=()
pipeline_values=()
for _ in $(seq 1 "$samples"); do
    c_log="$(env LD_LIBRARY_PATH="$mpp_lib" taskset -c 6 "$mpp_bin" \
        -w 1280 -h 720 -t 7 -rc 2 -bps 4000000 -fps 30 -g 60 \
        -qc 26 -n "$frames" -v q -o /dev/null 2>&1)"
    c_values+=("$(printf '%s\n' "$c_log" |
        sed -n 's/.* fps \([0-9][0-9.]*\).*/\1/p' | tail -1)")

    single_log="$(taskset -c 6 "$rust_bin" h264-enc \
        "$fixture" /dev/null 1280 720 26 high "$frames" 60)"
    single_values+=("$(printf '%s\n' "$single_log" |
        sed -n 's/.*, \([0-9][0-9.]*\) ms$/\1/p' | tail -1)")

    pipeline_log="$(RKMPP_INPUT_CPU=7 taskset -c 6,7 "$rust_bin" h264-enc \
        "$fixture" /dev/null 1280 720 26 high "$frames" 60)"
    pipeline_values+=("$(printf '%s\n' "$pipeline_log" |
        sed -n 's/.*, \([0-9][0-9.]*\) ms$/\1/p' | tail -1)")
done

c_fps="$(median "${c_values[@]}")"
single_ms="$(median "${single_values[@]}")"
pipeline_ms="$(median "${pipeline_values[@]}")"
single_fps="$(fps_from_ms "$single_ms")"
pipeline_fps="$(fps_from_ms "$pipeline_ms")"

printf '%-34s %10s FPS\n' "C MPP, built-in fill, core 6" "$c_fps"
printf '%-34s %10s FPS (%s ms)\n' "Rust mmap, core 6" "$single_fps" "$single_ms"
printf '%-34s %10s FPS (%s ms)\n' "Rust pipelined, cores 6+7" "$pipeline_fps" "$pipeline_ms"
awk -v c="$c_fps" -v s="$single_fps" -v p="$pipeline_fps" 'BEGIN {
    printf "single-core Rust vs C: %+.1f%%\n", (s - c) * 100 / c
    printf "pipelined Rust vs C:  %+.1f%%\n", (p - c) * 100 / c
}'
