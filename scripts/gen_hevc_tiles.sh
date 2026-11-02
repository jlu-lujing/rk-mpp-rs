#!/usr/bin/env bash
BOARD="${BOARD:-t6}"
# Generate the H.265 tile fixture with MPP's encoder. This board's libx265
# build does not expose tile-columns/tile-rows.
set -euo pipefail

base_cfg=/tmp/mpp-develop/test/cfg/enc/base_h265_1920x1080.json
enc=/tmp/mpp-develop/build/test/mpi_enc_test
libdir=/tmp/mpp-develop/build/mpp

if ! ssh "$BOARD" "test -f $base_cfg && test -x $enc"; then
  printf 'MPP H.265 encoder is unavailable on t6\n' >&2
  exit 1
fi

ssh "$BOARD" 'ffmpeg -loglevel error -y -f lavfi \
  -i testsrc2=size=1280x720:rate=30 -frames:v 30 \
  -pix_fmt nv12 -f rawvideo /tmp/hevc_tiles_src_nv12.yuv'

ssh "$BOARD" "sed \
  -e 's/\"auto_tile\" : 0/\"auto_tile\" : 1/' \
  -e 's/\"width\" : 1920/\"width\" : 1280/' \
  -e 's/\"height\" : 1080/\"height\" : 720/' \
  $base_cfg > /tmp/hevc_tiles_enc.json"

ssh "$BOARD" "sudo env LD_LIBRARY_PATH=$libdir $enc \
  -i /tmp/hevc_tiles_src_nv12.yuv -o /tmp/hevc_tiles_720p.hevc \
  -w 1280 -h 720 -n 30 -f 0 -t 16777220 \
  -cfg /tmp/hevc_tiles_enc.json -v q \
  >/tmp/hevc_tiles_encode.log 2>&1"
