#!/usr/bin/env bash
# Opt-in RK3588 hot-path performance gate.
#
# Usage:
#   scripts/board_perf_gate.sh
#   BOARD=my-rk3588-host PERF_SAMPLES=15 scripts/board_perf_gate.sh
#
# The checked rows and limits live in scripts/perf_baseline.tsv. Decode rows
# use the interleaved Rust-vs-C percentage from board_bench_vs_mpp.sh; scale
# rows use the shipped NEON vector_ms from board_perf_hotpaths_vs_mpp.sh.
set -euo pipefail

BOARD="${BOARD:?set BOARD to a Rockchip SSH alias}"
PERF_SAMPLES="${PERF_SAMPLES:-11}"
PERF_SCALE_ITERS="${PERF_SCALE_ITERS:-30}"
PERF_BENCH_FILTER="${PERF_BENCH_FILTER:-test720}"
HERE="$(cd "$(dirname "$0")" && pwd)"
LOG="$(mktemp "${TMPDIR:-/tmp}/rk-mpp-perf.XXXXXX")"
trap 'rm -f "$LOG"' EXIT

ssh "$BOARD" "
  export PATH=\"\$HOME/.cargo/bin:\$PATH\"
  cd \"\$HOME/rk-mpp-rs\"
  cargo build --release --examples >/dev/null 2>&1
  env BENCH_FILTER='$PERF_BENCH_FILTER' SAMPLES='$PERF_SAMPLES' \
    SCALE_ITERS='$PERF_SCALE_ITERS' \
    bash scripts/board_perf_hotpaths_vs_mpp.sh
" < /dev/null | tee "$LOG"

fail=0
while IFS=$'\t' read -r kind key limit; do
  [[ -z "${kind:-}" || "$kind" == \#* ]] && continue
  case "$kind" in
    decode)
      line="$(grep -F "$key" "$LOG" | grep 'rust speed' | tail -1 || true)"
      if [[ -z "$line" ]]; then
        printf 'FAIL perf decode %-28s missing from benchmark\n' "$key"
        fail=1
        continue
      fi
      actual="$(sed -nE 's/.*rust speed ([+-][0-9.]+)%.*/\1/p' <<<"$line")"
      if awk -v actual="$actual" -v limit="$limit" 'BEGIN { exit !(actual >= limit) }'; then
        printf 'PASS perf decode %-28s %+0.1f%% >= %+0.1f%%\n' "$key" "$actual" "$limit"
      else
        printf 'FAIL perf decode %-28s %+0.1f%% < %+0.1f%%\n' "$key" "$actual" "$limit"
        fail=1
      fi
      ;;
    scale)
      line="$(grep -F "$key" "$LOG" | grep 'x' | tail -1 || true)"
      if [[ -z "$line" ]]; then
        printf 'FAIL perf scale  %-28s missing from benchmark\n' "$key"
        fail=1
        continue
      fi
      actual="$(awk -v key="$key" '$1 == key { print $2 }' <<<"$line")"
      if awk -v actual="$actual" -v limit="$limit" 'BEGIN { exit !(actual <= limit) }'; then
        printf 'PASS perf scale  %-28s %.4f ms <= %.4f ms\n' "$key" "$actual" "$limit"
      else
        printf 'FAIL perf scale  %-28s %.4f ms > %.4f ms\n' "$key" "$actual" "$limit"
        fail=1
      fi
      ;;
    *)
      printf 'FAIL unknown perf row kind: %s\n' "$kind"
      fail=1
      ;;
  esac
done <"$HERE/perf_baseline.tsv"

exit "$fail"
