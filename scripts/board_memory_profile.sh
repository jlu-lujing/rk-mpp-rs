#!/usr/bin/env bash
# Peak-RSS ledger for the board codec entry points.
#
# Run on the board:
#   bash scripts/board_memory_profile.sh > docs/memory.tsv
#
# Debian does not ship GNU time here, so sample /proc/<pid>/status while the
# release binary is alive and retain the largest VmHWM value.
set -euo pipefail

BIN="${BIN:-$HOME/rk-mpp-rs/target/release/rk-mpp-rs}"
REPEATS="${REPEATS:-3}"
RUNNER="${RUNNER:-}"
FRAME=/tmp/rk-mpp-memory-1280x720.nv12
FRAMES=/tmp/rk-mpp-memory-1280x720x6.nv12
FRAME_BYTES=$((1280 * 720 * 3 / 2))
BUFINFO=/sys/kernel/debug/dma_buf/bufinfo

test -x "$BIN" || {
  echo "missing release binary: $BIN" >&2
  exit 1
}
command -v "$RUNNER" >/dev/null || RUNNER=
"$BIN" probe-frame "$FRAME" 1280 720 >/dev/null
: >"$FRAMES"
for frame in $(seq 0 5); do
  dd if="$FRAME" of="$FRAMES" bs="$FRAME_BYTES" seek="$frame" conv=notrunc 2>/dev/null
done
dma_available=0
if sudo test -r "$BUFINFO"; then dma_available=1; fi

dma_total_kib() {
  sudo awk '
    function hex(s, i, c, v) {
      s = tolower(s); v = 0
      for (i = 1; i <= length(s); i++) {
        c = index("0123456789abcdef", substr(s, i, 1)) - 1
        v = v * 16 + c
      }
      return v
    }
    $1 ~ /^[0-9a-fA-F]+$/ && length($1) == 8 { total += hex($1) }
    END { printf "%.0f\n", total / 1024 }
  ' "$BUFINFO" 2>/dev/null || printf '0\n'
}

peak_usage() {
  local pid="$1" baseline_dma="$2" peak=0 peak_dma=0 line current
  while true; do
    line="$(awk '$1 == "VmHWM:" { print $2 }' "/proc/$pid/status" 2>/dev/null || true)"
    if [[ -n "$line" && "$line" -gt "$peak" ]]; then
      peak="$line"
    fi
    if [[ "$dma_available" = 1 ]]; then
      current="$(dma_total_kib)"
      if [[ "$current" -gt "$baseline_dma" && $((current - baseline_dma)) -gt "$peak_dma" ]]; then
        peak_dma=$((current - baseline_dma))
      fi
    fi
    kill -0 "$pid" 2>/dev/null || break
    sleep 0.001
  done
  wait "$pid"
  printf '%s %s\n' "$peak" "$peak_dma"
}

run_case() {
  local label="$1" fixture="$2"; shift 2
  local max=0 max_dma=0 sample log baseline_dma
  baseline_dma="$(dma_total_kib)"
  for ((sample = 1; sample <= REPEATS; sample++)); do
    log="$(mktemp "${TMPDIR:-/tmp}/rk-mpp-memory.XXXXXX")"
    if [[ -n "$RUNNER" ]]; then
      "$RUNNER" "$@" >"$log" 2>&1 &
    else
      "$@" >"$log" 2>&1 &
    fi
    local pid=$!
    local peak peak_dma usage
    usage="$(peak_usage "$pid" "$baseline_dma")"
    read -r peak peak_dma <<<"$usage"
    if [[ "$peak" -gt "$max" ]]; then max="$peak"; fi
    if [[ "$peak_dma" -gt "$max_dma" ]]; then max_dma="$peak_dma"; fi
    unlink "$log"
  done
  local dma_report="$max_dma"
  if [[ "$dma_report" = 0 ]]; then dma_report=-; fi
  printf '%s\t%s\t%s\t%s\n' "$label" "$fixture" "$max" "$dma_report"
}

printf 'codec\tfixture\tpeak_rss_kib\tpeak_dma_buf_kib\n'
run_case h264-decode test720.264 "$BIN" h264dec /tmp/test720.264 /dev/null
run_case hevc-decode t720.hevc "$BIN" hevcdec /tmp/t720.hevc /dev/null
run_case vp9-decode vp90_720.ivf "$BIN" vp9dec /tmp/vp90_720.ivf /dev/null
run_case av1-decode av1_320x240_10.ivf "$BIN" av1dec /tmp/av1_320x240_10.ivf /dev/null
run_case mpeg2-decode m2v_320x240_12.m2v "$BIN" m2vdec /tmp/m2v_320x240_12.m2v /dev/null
run_case mpeg4-decode mp4_320x240_12_ip.m4v "$BIN" mp4dec /tmp/mp4_320x240_12_ip.m4v /dev/null
run_case h263-decode h263_cif_12_ip.263 "$BIN" h263dec /tmp/h263_cif_12_ip.263 /dev/null
run_case vp8-decode vp8_320x240_12_ip.ivf "$BIN" vp8dec /tmp/vp8_320x240_12_ip.ivf /dev/null
run_case h264-encode 1280x720x6.nv12 \
  "$BIN" h264-enc "$FRAMES" /dev/null 1280 720 26 baseline 6 1
run_case h265-encode 1280x720x6.nv12 \
  "$BIN" h265-gop "$FRAMES" /dev/null 1280 720 6 26
