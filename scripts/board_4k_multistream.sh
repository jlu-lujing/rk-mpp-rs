#!/usr/bin/env bash
# RK3588 4K independent-decoder concurrency gate.
#
# Each stream is a separate process so the test covers hardware queue pressure,
# dma-heap ownership and startup races rather than one decoder's in-process DPB.
# Every concurrent output is compared with a single-stream C/Rust golden.
set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"

: "${BOARD:?set BOARD to the SSH alias of an RK3588 board}"
DEC_MPP_REF="${DEC_MPP_REF:-}"
MPP_BIN="${MPP_BIN:-}"
MPP_LIB="${MPP_LIB:-}"
export BOARD DEC_MPP_REF MPP_BIN MPP_LIB

if [ "${SKIP_BUILD:-0}" != 1 ]; then
    COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target \
        --exclude Cargo.lock --exclude .worktrees --exclude refs \
        --exclude '._*' -cf - . |
        ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
    ssh "$BOARD" \
        'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release'
fi

ssh "$BOARD" "bash -s -- '$MPP_BIN' '$MPP_LIB' '$DEC_MPP_REF'" <<'REMOTE'
set -euo pipefail

repo="$HOME/rk-mpp-rs"
cli="$repo/target/release/rk-mpp-rs"
mpp_root=${3:-$HOME/rk-mpp-oracles/mpp-develop}
mpp_bin=${1:-$mpp_root/build/test/mpi_dec_test}
mpp_lib=${2:-$mpp_root/build/mpp}
counts=${STREAM_COUNTS:-1 2 4 8 16}
frames=${FRAMES:-3}
width=3840
height=2160
frame_bytes=$((width * height * 3 / 2))
expected_bytes=$((frame_bytes * frames))
work=$(mktemp -d /tmp/rk-mpp-rs-4k-multistream.XXXXXX)
trap 'sudo -n rm -rf "$work"' EXIT

command -v ffmpeg >/dev/null
command -v ffprobe >/dev/null
command -v vpxenc >/dev/null
test -x "$cli"
if [ ! -x "$mpp_bin" ] || [ ! -f "$mpp_lib/librockchip_mpp.so.0" ]; then
    printf 'missing C decoder at %s or library at %s\n' "$mpp_bin" "$mpp_lib" >&2
    exit 2
fi

ffmpeg_base=(
    -nostdin -hide_banner -loglevel error
    -f lavfi -i "testsrc2=size=${width}x${height}:rate=30"
    -frames:v "$frames" -pix_fmt yuv420p
)
ffmpeg "${ffmpeg_base[@]}" -c:v libx264 -preset ultrafast \
    -x264-params "keyint=30:min-keyint=30:bframes=0:scenecut=0:repeat-headers=1" \
    -f h264 "$work/input.264"
ffmpeg "${ffmpeg_base[@]}" -c:v libx265 -preset ultrafast -profile:v main \
    -x265-params \
    "log-level=error:keyint=30:min-keyint=30:bframes=0:scenecut=0:repeat-headers=1" \
    -f hevc "$work/input.265"
ffmpeg -nostdin -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=${width}x${height}:rate=30" \
    -frames:v "$frames" -pix_fmt yuv420p -f rawvideo "$work/input-vp9.yuv"
vpxenc --quiet --codec=vp9 --profile=0 --passes=1 --good --cpu-used=8 \
    --threads=4 --lag-in-frames=0 --kf-max-dist=30 --limit="$frames" \
    --width="$width" --height="$height" --fps=30/1 --ivf \
    -o "$work/input.vp9.ivf" "$work/input-vp9.yuv"
rm -f "$work/input-vp9.yuv"

cases=(
    "h264dec h264 7 $work/input.264"
    "hevcdec h265 16777220 $work/input.265"
    "vp9dec vp9 10 $work/input.vp9.ivf"
)
passed=0

for cell in "${cases[@]}"; do
    read -r cmd name coding input <<< "$cell"
    golden="$work/${name}-golden.yuv"
    vendor="$work/${name}-vendor.yuv"

    sudo -n timeout 180 "$cli" "$cmd" "$input" "$golden" \
        >"$work/${name}-golden.log" 2>&1
    if [ "$(stat -c %s "$golden")" -ne "$expected_bytes" ]; then
        printf 'FAIL multistream/%s golden size %s expected %s\n' \
            "$name" "$(stat -c %s "$golden")" "$expected_bytes" >&2
        exit 1
    fi
    if ! sudo -n env LD_LIBRARY_PATH="$mpp_lib" timeout 180 \
        "$mpp_bin" -i "$input" -o "$vendor" -w "$width" -h "$height" \
        -t "$coding" -f 0 -n "$frames" -v q </dev/null \
        >"$work/${name}-vendor.log" 2>&1; then
        printf 'FAIL multistream/%s vendor decode failed\n' "$name" >&2
        exit 1
    fi
    if [ "$(stat -c %s "$vendor")" -ne "$expected_bytes" ]; then
        printf 'FAIL multistream/%s vendor size %s expected %s\n' \
            "$name" "$(stat -c %s "$vendor")" "$expected_bytes" >&2
        exit 1
    fi
    ffmpeg -nostdin -hide_banner -loglevel error -y \
        -f rawvideo -pix_fmt nv12 -s "${width}x${height}" -i "$vendor" \
        -pix_fmt yuv420p -f rawvideo "$work/${name}-vendor-planar.yuv"
    if ! cmp -s "$golden" "$work/${name}-vendor-planar.yuv"; then
        printf 'FAIL multistream/%s single-stream C/Rust mismatch\n' "$name" >&2
        exit 1
    fi

    if [ "$name" = h264 ] || [ "$name" = h265 ]; then
        pipeline_var=RKMPP_H264_PIPELINE
        [ "$name" = h265 ] && pipeline_var=RKMPP_HEVC_PIPELINE
        for depth in 0 1 2 3; do
            sudo -n env "$pipeline_var=$depth" timeout 180 \
                "$cli" "$cmd" "$input" "$work/${name}-pipeline-${depth}.yuv" \
                >"$work/${name}-pipeline-${depth}.log" 2>&1
            if ! cmp -s "$golden" "$work/${name}-pipeline-${depth}.yuv"; then
                printf 'FAIL multistream/%s pipeline depth %s differs\n' \
                    "$name" "$depth" >&2
                exit 1
            fi
        done
        printf 'PASS multistream/%-4s pipeline depths=0,1,2,3\n' "$name"
    fi

    if [ "$name" = h264 ]; then
        sudo -n env RKMPP_DECODE_NV12=1 timeout 180 \
            "$cli" "$cmd" "$input" "$work/h264-nv12.yuv" \
            >"$work/h264-nv12.log" 2>&1
        if ! cmp -s "$vendor" "$work/h264-nv12.yuv"; then
            printf 'FAIL multistream/h264 packed NV12 differs from C\n' >&2
            exit 1
        fi
        printf 'PASS multistream/h264 packed NV12 == C\n'
    fi

    for streams in $counts; do
        run="$work/${name}-${streams}"
        mkdir "$run"
        started=$(date +%s%N)
        pids=()
        for ((i = 0; i < streams; i++)); do
            sudo -n timeout 180 "$cli" "$cmd" "$input" "$run/$i.yuv" \
                >"$run/$i.log" 2>&1 &
            pids+=("$!")
        done
        failures=0
        for pid in "${pids[@]}"; do
            wait "$pid" || failures=$((failures + 1))
        done
        elapsed=$((($(date +%s%N) - started) / 1000000))
        mismatches=0
        for ((i = 0; i < streams; i++)); do
            if [ ! -f "$run/$i.yuv" ] ||
                [ "$(stat -c %s "$run/$i.yuv")" -ne "$expected_bytes" ] ||
                ! cmp -s "$run/$i.yuv" "$golden"; then
                mismatches=$((mismatches + 1))
            fi
        done
        if [ "$failures" -ne 0 ] || [ "$mismatches" -ne 0 ]; then
            printf 'FAIL multistream/%s streams=%s failures=%s mismatches=%s\n' \
                "$name" "$streams" "$failures" "$mismatches" >&2
            exit 1
        fi
        hash=$(sha256sum "$golden" | cut -d' ' -f1)
        printf 'PASS multistream/%-4s streams=%-2s frames=%s elapsed=%sms sha256=%s\n' \
            "$name" "$streams" "$frames" "$elapsed" "${hash:0:16}"
        passed=$((passed + 1))
        rm -rf "$run"
    done
done

printf 'SUMMARY 4K decode concurrency passed=%d failed=0\n' "$passed"
REMOTE
