#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
output8="${1:-$root/tests/data/av1_film_grain_320x240_10.ivf}"
output10="${2:-$root/tests/data/film_grain_10bit.ivf}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

ffmpeg -hide_banner -loglevel error \
  -f lavfi -i testsrc2=size=320x240:rate=10 \
  -frames:v 10 -pix_fmt yuv420p "$work/source.y4m"

aomenc --quiet \
  --limit=10 --cpu-used=8 --end-usage=q --cq-level=30 \
  --film-grain-test=1 \
  --output="$output8" "$work/source.y4m"

ffmpeg -hide_banner -loglevel error \
  -f lavfi -i testsrc2=size=320x240:rate=10 \
  -frames:v 10 -pix_fmt yuv420p10le -f rawvideo "$work/source10.yuv"

aomenc --quiet \
  --limit=10 --cpu-used=8 --end-usage=q --cq-level=30 \
  --width=320 --height=240 --fps=10/1 \
  --bit-depth=10 --input-bit-depth=10 --film-grain-test=1 \
  --output="$output10" "$work/source10.yuv"

printf 'wrote %s and %s\n' "$output8" "$output10"
