#!/usr/bin/env bash
# Board-pulled network H.264 -> C MPP vs Rust MPP decode-only benchmark.
set -euo pipefail

VIDEO_URL=${VIDEO_URL:-http://192.0.2.10:18081/demo30.m3u8}
STREAM=${STREAM:-/tmp/rfi-network-decode.264}
FRAMES=${FRAMES:-}
SAMPLES=${SAMPLES:-7}
PULL=${PULL:-auto}
RUST_BIN=${RUST_BIN:-"$HOME/rk-mpp-rs/target/release/rk-mpp-rs"}
OUT=${OUT:-"$HOME/rk-mpp-rs/target/network-decode-$(date +%Y%m%d-%H%M%S)"}

mkdir -p "$OUT"

if [[ "$PULL" == 1 || ! -s "$STREAM" ]]; then
    tmp="$STREAM.part"
    rm -f "$tmp"
    ffmpeg -nostdin -hide_banner -loglevel error -y \
        -i "$VIDEO_URL" -map 0:v:0 -c:v copy -an -f h264 "$tmp"
    mv "$tmp" "$STREAM"
fi

if [[ -z "$FRAMES" ]]; then
    progress="$OUT/frame-count.progress"
    ffmpeg -nostdin -hide_banner -loglevel quiet -nostats \
        -progress "$progress" -c:v h264_rkmpp -i "$STREAM" \
        -map 0:v:0 -fps_mode passthrough -pix_fmt yuv420p -f null -
    FRAMES=$(awk -F= '$1 == "frame" { frame = $2 } END { print frame }' "$progress")
fi
[[ "$FRAMES" =~ ^[0-9]+$ && "$FRAMES" -gt 0 ]] || {
    echo "cannot determine frame count" >&2
    exit 1
}

run_c() {
    local round=$1
    local start end
    start=$(date +%s%N)
    ffmpeg -nostdin -hide_banner -loglevel quiet \
        -c:v h264_rkmpp -i "$STREAM" -map 0:v:0 \
        -fps_mode passthrough -pix_fmt yuv420p -f rawvideo -y /dev/null
    end=$(date +%s%N)
    printf '%s\n' $(((end - start) / 1000000)) > "$OUT/c_${round}.ms"
}

run_rust() {
    local round=$1
    local start end
    start=$(date +%s%N)
    sudo -n "$RUST_BIN" h264dec "$STREAM" /dev/null \
        > "$OUT/rust_${round}.jsonish" 2> "$OUT/rust_${round}.log"
    end=$(date +%s%N)
    printf '%s\n' $(((end - start) / 1000000)) > "$OUT/rust_${round}.ms"
}

run_c warmup
run_rust warmup
for round in $(seq 1 "$SAMPLES"); do
    if (( round % 2 )); then
        run_c "$round"
        run_rust "$round"
    else
        run_rust "$round"
        run_c "$round"
    fi
done

python3 - "$OUT" "$FRAMES" "$VIDEO_URL" <<'PY'
import json
import re
import statistics
import sys
from pathlib import Path

out = Path(sys.argv[1])
frames = int(sys.argv[2])
video_url = sys.argv[3]


def values(who):
    return [
        int(path.read_text())
        for path in sorted(out.glob(f"{who}_[0-9]*.ms"))
    ]


c_ms = values("c")
rust_ms = values("rust")
rust_internal = []
for pattern in ("rust_[0-9]*.jsonish", "rust_[0-9]*.log"):
    for path in sorted(out.glob(pattern)):
        match = re.search(r" in ([0-9.]+) ms", path.read_text(errors="replace"))
        if match:
            rust_internal.append(float(match.group(1)))

summary = {
    "video_url": video_url,
    "frames": frames,
    "samples": len(c_ms),
    "c": {
        "wall_ms": c_ms,
        "median_ms": statistics.median(c_ms),
        "fps": round(frames * 1000 / statistics.median(c_ms), 2),
    },
    "rust": {
        "wall_ms": rust_ms,
        "median_ms": statistics.median(rust_ms),
        "fps": round(frames * 1000 / statistics.median(rust_ms), 2),
        "internal_ms": rust_internal,
    },
}
summary["rust_vs_c_time_percent"] = round(
    100 * (summary["rust"]["median_ms"] - summary["c"]["median_ms"])
    / summary["c"]["median_ms"],
    2,
)
(out / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
print(json.dumps(summary, indent=2))
PY
