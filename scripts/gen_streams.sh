#!/usr/bin/env bash
# Regenerate the H.264/H.265 test streams used by the plans.
# Usage: scripts/gen_streams.sh [source.yuv] [outdir]
set -euo pipefail
src="${1:-/tmp/src720.yuv}"
out="${2:-/tmp}"

# Multi-slice 720p, main profile, B-pyramid: exercises AU merging, ref-list
# reordering and multi-reference prediction in one stream.
ffmpeg -y -f rawvideo -pix_fmt yuv420p -s 1280x720 -r 30 -i "$src" \
  -c:v libx264 -profile:v main -bf 3 -refs 4 \
  -x264-params "slices=4:b-pyramid=normal" "$out/test720_slices.264"

# Interlaced 720p for the field/MBAFF path.
ffmpeg -y -f rawvideo -pix_fmt yuv420p -s 1280x720 -r 25 -i "$src" \
  -c:v libx264 -flags +ildct+ilme \
  -x264-params "interlaced=1" "$out/test720_i.264"

echo "wrote $out/test720_slices.264 $out/test720_i.264"
