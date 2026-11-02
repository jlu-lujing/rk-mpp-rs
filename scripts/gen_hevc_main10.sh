#!/usr/bin/env bash
BOARD="${BOARD:-t62}"
# Generate a reproducible 10-bit HEVC stream with the board's libx265.
set -euo pipefail

if ! ssh "$BOARD" 'command -v ffmpeg >/dev/null'; then
  printf 'ffmpeg is unavailable on t6\n' >&2
  exit 1
fi

ssh "$BOARD" 'ffmpeg -loglevel error -y -f lavfi \
  -i testsrc2=size=256x256:rate=30:duration=1 \
  -pix_fmt yuv420p10le \
  -c:v libx265 \
  -x265-params "keyint=10:min-keyint=10:open-gop=0:bframes=3:b-pyramid=1:scenecut=0:repeat-headers=1:log-level=warning" \
  -f hevc /tmp/hevc_main10_256.hevc \
  >/tmp/hevc_main10_encode.log 2>&1'
