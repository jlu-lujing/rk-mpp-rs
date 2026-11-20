#!/usr/bin/env bash
# Build a coverage-equivalent decoder corpus with ffmpeg plus the repo fixtures,
# then write and validate a SHA-256/container manifest.
# Runs ON the board (it only needs ffmpeg and the repo's tests/data).
#
#   OUT=~/rk-mpp-oracles/corpus-gen ~/rk-mpp-rs/scripts/gen_corpus.sh
#
# The original RK3588 corpus contained third-party conformance streams that cannot be
# regenerated; this script replaces coverage, not bytes: every decoder gets
# several sizes, both entropy modes/profiles where they exist, B pictures, tiles
# or slices, and 8/10-bit. The repo's own edge fixtures are copied in alongside.
set -uo pipefail

out="${OUT:-$HOME/rk-mpp-oracles/corpus-gen}"
here="$(cd "$(dirname "$0")" && pwd)"
data="$here/../tests/data"
failures=0
mkdir -p "$out"
printf 'writing %s\n' "$out"

src() { printf 'testsrc2=size=%s:rate=30' "$1"; }
enc() {
  local name="$1" size="$2" frames="$3"; shift 3
  if ffmpeg -hide_banner -loglevel error -y -f lavfi -i "$(src "$size")" \
      -frames:v "$frames" "$@" "$out/$name" >/dev/null 2>&1; then
    printf 'ok   %s\n' "$name"
  else
    printf 'FAIL %s\n' "$name"
    failures=$((failures+1))
  fi
}

# ---- H.264 ---------------------------------------------------------------
enc h264_64x64_baseline.264        64x64    30 -c:v libx264 -preset ultrafast -profile:v baseline -pix_fmt yuv420p -g 30
enc h264_320x240_baseline.264      320x240  30 -c:v libx264 -preset ultrafast -profile:v baseline -pix_fmt yuv420p -g 30
enc h264_320x240_main.264          320x240  30 -c:v libx264 -preset ultrafast -profile:v main -pix_fmt yuv420p -g 30
enc h264_320x240_high_bf3.264      320x240  30 -c:v libx264 -preset ultrafast -profile:v high -bf 3 -pix_fmt yuv420p -g 30
enc h264_640x480_main_bf2.264      640x480  30 -c:v libx264 -preset ultrafast -profile:v main -bf 2 -pix_fmt yuv420p -g 30
enc h264_640x480_slices.264        640x480  30 -c:v libx264 -preset ultrafast -profile:v main -pix_fmt yuv420p -x264-params slices=4 -g 30
enc h264_1280x720_high.264         1280x720 20 -c:v libx264 -preset ultrafast -profile:v high -pix_fmt yuv420p -g 30
enc h264_1920x1088_high.264        1920x1088 10 -c:v libx264 -preset ultrafast -profile:v high -pix_fmt yuv420p -g 30

# ---- H.265 ---------------------------------------------------------------
enc hevc_320x240_main.265          320x240  30 -c:v libx265 -preset ultrafast -x265-params log-level=none -pix_fmt yuv420p -g 30
enc hevc_640x480_main.265          640x480  30 -c:v libx265 -preset ultrafast -x265-params log-level=none -pix_fmt yuv420p -g 30
enc hevc_640x480_tiles.265         640x480  30 -c:v libx265 -preset ultrafast -x265-params log-level=none:tiles=2x2:wpp=1 -pix_fmt yuv420p -g 30
enc hevc_1280x720_main.265         1280x720 20 -c:v libx265 -preset ultrafast -x265-params log-level=none -pix_fmt yuv420p -g 30
enc hevc_1280x720_main10.265       1280x720 10 -c:v libx265 -preset ultrafast -x265-params log-level=none -pix_fmt yuv420p10le -g 30
enc hevc_1920x1088_main.265        1920x1088 10 -c:v libx265 -preset ultrafast -x265-params log-level=none -pix_fmt yuv420p -g 30

# ---- VP8 / VP9 -----------------------------------------------------------
enc vp8_320x240.ivf                320x240  30 -c:v libvpx -deadline realtime -cpu-used 8 -b:v 1M -g 30
enc vp8_640x480.ivf                640x480  30 -c:v libvpx -deadline realtime -cpu-used 8 -b:v 2M -g 30
enc vp9_320x240_p0.ivf             320x240  30 -c:v libvpx-vp9 -deadline realtime -cpu-used 8 -b:v 1M -g 30 -pix_fmt yuv420p
enc vp9_640x480_p0.ivf             640x480  30 -c:v libvpx-vp9 -deadline realtime -cpu-used 8 -b:v 2M -g 30 -pix_fmt yuv420p
enc vp9_1280x720_p0.ivf            1280x720 20 -c:v libvpx-vp9 -deadline realtime -cpu-used 8 -b:v 4M -g 30 -pix_fmt yuv420p
enc vp9_320x240_p2_10bit.ivf       320x240  20 -c:v libvpx-vp9 -deadline realtime -cpu-used 8 -b:v 1M -g 30 -pix_fmt yuv420p10le -profile:v 2

# ---- AV1 ---------------------------------------------------------------
# libaom-av1 is not in the board's ffmpeg build: run this section on a host
# that has one, or copy the fixtures in from tests/data.
enc av1_96x96.ivf                  96x96    10 -c:v libaom-av1 -cpu-used 8 -crf 40 -b:v 0 -pix_fmt yuv420p
enc av1_128x128.ivf                128x128  10 -c:v libaom-av1 -cpu-used 8 -crf 40 -b:v 0 -pix_fmt yuv420p
enc av1_192x192.ivf                192x192  10 -c:v libaom-av1 -cpu-used 8 -crf 40 -b:v 0 -pix_fmt yuv420p
enc av1_320x240.ivf                320x240  10 -c:v libaom-av1 -cpu-used 8 -crf 40 -b:v 0 -pix_fmt yuv420p
enc av1_640x480.ivf                640x480  10 -c:v libaom-av1 -cpu-used 8 -crf 40 -b:v 0 -pix_fmt yuv420p
enc av1_640x480_long.ivf           640x480  60 -c:v libaom-av1 -cpu-used 8 -crf 40 -b:v 0 -pix_fmt yuv420p -g 30
enc av1_1280x720.ivf               1280x720 20 -c:v libaom-av1 -cpu-used 8 -crf 40 -b:v 0 -pix_fmt yuv420p
enc av1_1920x1080.ivf              1920x1080 10 -c:v libaom-av1 -cpu-used 8 -crf 40 -b:v 0 -pix_fmt yuv420p
enc av1_320x240_p0_10bit.ivf       320x240  10 -c:v libaom-av1 -cpu-used 8 -crf 40 -b:v 0 -profile:v 0 -pix_fmt yuv420p10le
if ! python3 - "$out" <<'PY'
import struct
import sys
from pathlib import Path

out = Path(sys.argv[1])

def frames(path):
    data = path.read_bytes()
    if data[:4] != b"DKIF":
        raise SystemExit(f"{path}: missing DKIF")
    header = bytearray(data[:32])
    result = []
    offset = 32
    while offset < len(data):
        if offset + 12 > len(data):
            raise SystemExit(f"{path}: truncated frame header")
        size = struct.unpack_from("<I", data, offset)[0]
        end = offset + 12 + size
        if end > len(data):
            raise SystemExit(f"{path}: truncated frame")
        result.append(data[offset + 12:end])
        offset = end
    return header, result

def write_concat(name, paths):
    header = None
    payloads = []
    for path in paths:
        current_header, current = frames(path)
        if header is None:
            header = bytearray(current_header)
        payloads.extend(current)
    assert header is not None
    struct.pack_into("<I", header, 24, len(payloads))
    with (out / name).open("wb") as destination:
        destination.write(header)
        for index, payload in enumerate(payloads):
            destination.write(struct.pack("<IQ", len(payload), index))
            destination.write(payload)

write_concat(
    "av1_multi_sequence_320x240.ivf",
    [out / "av1_320x240.ivf", out / "av1_320x240.ivf"],
)
write_concat(
    "av1_resize_320_to_640.ivf",
    [out / "av1_320x240.ivf", out / "av1_640x480.ivf"],
)
write_concat(
    "av1_resize_640_to_320.ivf",
    [out / "av1_640x480.ivf", out / "av1_320x240.ivf"],
)
PY
then
  failures=$((failures+1))
fi

# ---- MPEG-2 / MPEG-4 / H.263 --------------------------------------------
enc m2v_320x240.m2v                320x240  30 -c:v mpeg2video -q:v 4 -g 15
enc m2v_720x576.m2v                720x576  20 -c:v mpeg2video -q:v 4 -g 15
enc m4v_320x240_bf2.m4v            320x240  30 -c:v mpeg4 -q:v 4 -bf 2 -g 30 -f m4v
enc h263_352x288.263               352x288  30 -c:v h263 -q:v 4 -g 30 -f h263

# ---- stills ------------------------------------------------------------
enc jpeg_320x240.jpg               320x240  1 -frames:v 1 -q:v 3
enc jpeg_640x480.jpg               640x480  1 -frames:v 1 -q:v 3

# ---- repo edge fixtures -------------------------------------------------
for f in "$data"/*.264 "$data"/*.265 "$data"/*.ivf "$data"/*.m2v "$data"/*.m4v "$data"/*.263; do
  [ -f "$f" ] || continue
  if ! cp -f "$f" "$out/td_$(basename "$f")"; then
    printf 'FAIL copying %s\n' "$f"
    failures=$((failures+1))
  fi
done
printf 'fixtures copied: %s\n' "$(ls "$out" | grep -c '^td_')"
printf 'corpus files: %s\n' "$(find "$out" -maxdepth 1 -type f | wc -l)"
if ! python3 "$here/check_corpus_manifest.py" --write "$out"; then
  failures=$((failures+1))
fi
printf 'SUMMARY failures=%s\n' "$failures"
[ "$failures" -eq 0 ]
