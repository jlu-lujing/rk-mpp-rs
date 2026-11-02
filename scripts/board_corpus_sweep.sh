#!/usr/bin/env bash
# Decode every stream in the oracle corpus with the C reference and this crate,
# and compare the output bytes. Runs ON the board - the corpus and both binaries
# live there - and prints one verdict per stream plus a summary.
#
#   CORPUS=~/rk-mpp-oracles/corpus LIMIT=0 ~/rk-mpp-rs/scripts/board_corpus_sweep.sh
#
# Verdicts:
#   PASS       both sides decode the same frame count and the bytes match
#   NAMED:<why> one side refused / produced nothing / counts differ; the reason
#   FAIL       same count, different bytes
set -uo pipefail
# Progress must be visible while the sweep runs; a long silent stretch is the
# symptom that preceded the first board hang (2026-11-02).

corpus="${CORPUS:-$HOME/rk-mpp-oracles/corpus}"
limit="${LIMIT:-0}"
max_bytes="${MAX_BYTES:-$((256 * 1024 * 1024))}"
timeout_s="${TIMEOUT_S:-30}"
here="$(cd "$(dirname "$0")" && pwd)"
rust_bin="${RUST_BIN:-$here/../target/release/rk-mpp-rs}"
mpp_bin="${MPP_BIN:-/tmp/mpp-develop/build/test/mpi_dec_test}"
mpp_lib="${MPP_LIB:-/tmp/mpp-develop/build/mpp}"
work="${WORK:-/tmp/corpus_sweep}"
mkdir -p "$work"

pass=0; fail=0; named=0; skipped=0
count=0

codec_for() {
  local f="$1" base ext
  base="$(basename "$f")"
  ext="${base##*.}"
  case "$ext" in
    264) printf 'h264dec 7' ;;
    hevc) printf 'hevcdec 16777220' ;;
    m4v) printf 'mp4dec 4' ;;
    m2v) printf 'm2vdec 2' ;;
    263) printf 'h263dec 3' ;;
    ivf)
      local fourcc
      fourcc="$(dd if="$f" bs=1 skip=8 count=4 2>/dev/null)"
      case "$fourcc" in
        VP80) printf 'vp8dec 9' ;;
        VP90) printf 'vp9dec 10' ;;
        AV01) printf 'av1dec 16777224' ;;
        *) printf '' ;;
      esac ;;
    *) printf '' ;;
  esac
}

for f in "$corpus"/*; do
  [ -f "$f" ] || continue
  size=$(stat -c %s "$f" 2>/dev/null || echo 0)
  if [ "$size" -gt "$max_bytes" ]; then
    printf 'NAMED:%s skipped (%s bytes > MAX_BYTES)\n' "$(basename "$f")" "$size"
    named=$((named+1)); continue
  fi
  read -r cmd type <<<"$(codec_for "$f")"
  if [ -z "${cmd:-}" ]; then skipped=$((skipped+1)); continue; fi
  count=$((count+1))
  if [ "$limit" -gt 0 ] && [ "$count" -gt "$limit" ]; then break; fi

  name="$(basename "$f")"
  dims="$(ffprobe -v error -select_streams v:0 -show_entries stream=width,height \
      -of csv=p=0 "$f" 2>/dev/null | head -1)"
  width="${dims%,*}"; height="${dims#*,}"
  if [ -z "$width" ] || [ -z "$height" ] || [ "$width" = "$dims" ]; then
    printf 'NAMED:%s no ffprobe dimensions\n' "$name"; named=$((named+1)); continue
  fi

  r_out="$work/r.yuv"; c_out="$work/c.nv12"; c_planar="$work/c.yuv"
  rm -f "$r_out" "$c_out" "$c_planar"
  r_log="$work/r.log"
  sudo timeout "$timeout_s" "$rust_bin" "$cmd" "$f" "$r_out" >"$r_log" 2>&1; r_rc=$?
  frames="$(sed -nE 's/.*decoded ([0-9]+) (frames|packets).*/\1/p' "$r_log" | tail -1)"
  r_bytes=$(stat -c %s "$r_out" 2>/dev/null || echo 0)

  if [ "$r_rc" != 0 ] || [ -z "$frames" ] || [ "$frames" = 0 ]; then
    printf 'NAMED:%s rust rc=%s frames=%s\n' "$name" "$r_rc" "${frames:-0}"
    named=$((named+1)); continue
  fi

  sudo env LD_LIBRARY_PATH="$mpp_lib" timeout "$timeout_s" \
    "$mpp_bin" -i "$f" -t "$type" -n "$frames" -o "$c_out" -v q \
    >"$work/c.log" 2>&1; c_rc=$?
  if [ "$c_rc" != 0 ] || [ ! -s "$c_out" ]; then
    printf 'NAMED:%s rust ok (%s frames) C rc=%s\n' "$name" "$frames" "$c_rc"
    named=$((named+1)); continue
  fi
  ffmpeg -loglevel error -y -f rawvideo -pixel_format nv12 \
    -video_size "${width}x${height}" -i "$c_out" -pix_fmt yuv420p -f rawvideo "$c_planar" \
    >/dev/null 2>&1 || true
  c_bytes=$(stat -c %s "$c_planar" 2>/dev/null || echo 0)
  if [ "$c_bytes" = 0 ]; then
    printf 'NAMED:%s NV12->planar conversion failed (%sx%s)\n' "$name" "$width" "$height"
    named=$((named+1)); continue
  fi
  if [ "$c_bytes" != "$r_bytes" ]; then
    printf 'NAMED:%s byte count C=%s rust=%s\n' "$name" "$c_bytes" "$r_bytes"
    named=$((named+1)); continue
  fi
  if cmp -s "$c_planar" "$r_out"; then
    printf 'PASS %s (%s frames %sx%s)\n' "$name" "$frames" "$width" "$height"
    pass=$((pass+1))
  else
    first=$(cmp -l "$c_planar" "$r_out" 2>/dev/null | head -1)
    printf 'FAIL %s (%s frames %sx%s, first diff %s)\n' \
      "$name" "$frames" "$width" "$height" "${first:-?}"
    fail=$((fail+1))
  fi
done

printf 'SUMMARY pass=%s fail=%s named=%s skipped=%s\n' "$pass" "$fail" "$named" "$skipped"
[ "$fail" = 0 ]
