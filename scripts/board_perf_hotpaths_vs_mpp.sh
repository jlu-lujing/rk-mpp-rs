#!/usr/bin/env bash
BOARD="${BOARD:-t6}"
# Hot-path performance record: decode throughput + RGB scale rows.
#
# Runs on the board (`bash scripts/board_perf_hotpaths_vs_mpp.sh`, or from
# the host with `ssh "$BOARD" 'bash ~/rk-mpp-rs/scripts/board_perf_hotpaths_vs_mpp.sh' < /dev/null`).
#
# Timing discipline (HANDOFF section 5.8):
#   * Decode passes are delegated to scripts/board_bench_vs_mpp.sh, which
#     interleaves the Rust and C binaries sample by sample and reports
#     medians. SAMPLES defaults to 11 here (the C-side script defaults to
#     5, which is below what a single stream may conclude from).
#   * The RGB scale rows are timed inside one binary (bench_scale): each
#     sample runs the vector and the scalar reference alternately, frame by
#     frame, and reports the per-sample median of each; this script then
#     takes the median across samples. One lone sample is never evidence.
#
# Nothing here routes around the hardware: poll_hw_finish waits are part of
# the timed region on both sides, exactly as in the decode bench.
set -uo pipefail

bench_scale_bin="${BENCH_SCALE_BIN:-$HOME/rk-mpp-rs/target/release/examples/bench_scale}"
samples="${SAMPLES:-11}"
scale_iters="${SCALE_ITERS:-30}"
BENCH_FILTER="${BENCH_FILTER:-}"
SKIP_DECODE="${SKIP_DECODE:-}"

median() {
  local n values
  values="$(printf '%s\n' "$@" | sort -n)"
  n=$(printf '%s\n' "$values" | wc -l)
  printf '%s\n' "$values" | sed -n "$((n / 2 + 1))p"
}

if [ "$samples" -lt 11 ]; then
  echo "SAMPLES=$samples is below the eleven-sample floor; raise it or accept the warning." >&2
fi

if [ -z "$SKIP_DECODE" ]; then
  echo "=== decode hot paths (delegated to board_bench_vs_mpp.sh, SAMPLES=$samples) ==="
  SAMPLES="$samples" BENCH_FILTER="$BENCH_FILTER" \
    bash "$(dirname "$0")/board_bench_vs_mpp.sh"
else
  echo "=== decode hot paths skipped (SKIP_DECODE set) ==="
fi

echo "=== RGB scale rows: vector vs scalar reference ==="
if [ ! -x "$bench_scale_bin" ]; then
  echo "missing $bench_scale_bin (cargo build --release --examples)" >&2
  exit 1
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
for _sample in $(seq 1 "$samples"); do
  "$bench_scale_bin" "$scale_iters" | while read -r tag name kv v scalar_ms s; do
    [ "$tag" = "case" ] || continue
    printf '%s\n' "$v" >> "$tmp/$name.vector"
    printf '%s\n' "$s" >> "$tmp/$name.scalar"
  done
done

printf '%-24s %10s %10s %8s\n' case vector_ms scalar_ms speedup
for vector_file in "$tmp"/*.vector; do
  name="$(basename "$vector_file" .vector)"
  v="$(median $(cat "$vector_file"))"
  s="$(median $(cat "$tmp/$name.scalar"))"
  awk -v n="$name" -v v="$v" -v s="$s" \
    'BEGIN { printf "%-24s %10.4f %10.4f %7.2fx\n", n, v, s, s / v }'
done

echo
echo "remember: vector_ms is the shipped path, scalar_ms the pre-NEON reference;"
echo "the ratio is host-visible scaling work only, hardware waits cancel out."
