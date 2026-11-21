#!/usr/bin/env bash

# Compare native Rust H.264/H.265 encoder queues with vendor C FFmpeg.
set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"
export PATH="$HOME/.cargo/bin:$PATH"

samples=${SAMPLES:-11}
frames=${FRAMES:-300}
width=${WIDTH:-1280}
height=${HEIGHT:-720}
cpuset=${CPUSET:-4-7}
ffmpeg_bin=$(command -v ffmpeg)
ffprobe_bin=$(command -v ffprobe)
rust_bin="$repo/target/release/rk-mpp-rs"
task_prefix=()
if [ -n "$cpuset" ]; then
    task_prefix=(taskset -c "$cpuset")
fi
work=$(mktemp -d /tmp/rk-mpp-native-pipeline.XXXXXX)
trap 'rm -rf "$work"' EXIT

cargo build --release --offline --bin rk-mpp-rs --lib
raw="$work/input.nv12"
"$ffmpeg_bin" -nostdin -hide_banner -loglevel error -y \
    -f lavfi -i "testsrc2=size=${width}x${height}:rate=30" \
    -frames:v "$frames" -pix_fmt nv12 -f rawvideo "$raw"

run_native() {
    local codec=$1
    local mode=$2
    local prefix=(env)
    case "$mode" in
    sync)
        prefix+=(RKMPP_ENCODER_PIPELINE=0)
        ;;
    d1)
        prefix+=(RKMPP_ENCODER_PIPELINE=1 RKMPP_ENCODER_PIPELINE_DEPTH=1)
        ;;
    d2)
        prefix+=(RKMPP_ENCODER_PIPELINE=1 RKMPP_ENCODER_PIPELINE_DEPTH=2)
        ;;
    esac
    if [ "$codec" = h264 ]; then
        "${task_prefix[@]}" "${prefix[@]}" "$rust_bin" h264-enc "$raw" /dev/null \
            "$width" "$height" 26 high "$frames" 60
    else
        "${task_prefix[@]}" "${prefix[@]}" H265_GOP=60 "$rust_bin" h265-gop "$raw" /dev/null \
            "$width" "$height" "$frames" 26
    fi
}

run_c() {
    local codec=$1
    local ff_codec=$codec
    local muxer=$codec
    local profile=high
    if [ "$codec" = h265 ]; then
        ff_codec=hevc
        muxer=hevc
        profile=main
    fi
    "${task_prefix[@]}" "$ffmpeg_bin" -nostdin -hide_banner -loglevel error \
        -f rawvideo -pix_fmt nv12 -s "${width}x${height}" -r 30 -i "$raw" \
        -frames:v "$frames" -c:v "${ff_codec}_rkmpp" \
        -rc_mode CQP -qp_init 26 -g 60 -profile:v "$profile" \
        -f "$muxer" -y /dev/null
}

TIMEFORMAT='%3R %3U %3S'
declare -A wall_values cpu_values

run_path() {
    local key=$1
    shift
    local time_file="$work/$key.time"
    { time "$@" >/dev/null 2>&1; } 2>"$time_file"
    read -r wall user sys <"$time_file"
    wall_values["$key"]+="$wall "
    cpu_values["$key"]+="$(
        awk -v user="$user" -v sys="$sys" 'BEGIN { print user + sys }'
    ) "
}

run_mode() {
    local codec=$1
    local mode=$2
    [ "$mode" = c ] && run_c "$codec" || run_native "$codec" "$mode"
}

for sample in $(seq 1 "$samples"); do
    if [ $((sample % 2)) -eq 1 ]; then
        modes=(c sync d1 d2)
    else
        modes=(d2 d1 sync c)
    fi
    for codec in h264 h265; do
        for mode in "${modes[@]}"; do
            run_path "$codec-$mode-$sample" run_mode "$codec" "$mode"
        done
    done
done

median() {
    printf '%s\n' "$@" | sort -n |
        awk '{ value[NR] = $1 } END { print value[int((NR + 1) / 2)] }'
}

printf '%-22s %10s %10s %10s\n' path wall_s FPS CPU_s
for codec in h264 h265; do
    for mode in c sync d1 d2; do
        walls=()
        cpus=()
        for sample in $(seq 1 "$samples"); do
            read -ra values <<<"${wall_values["$codec-$mode-$sample"]}"
            walls+=("${values[0]}")
            read -ra values <<<"${cpu_values["$codec-$mode-$sample"]}"
            cpus+=("${values[0]}")
        done
        wall=$(median "${walls[@]}")
        cpu=$(median "${cpus[@]}")
        fps=$(awk -v frames="$frames" -v wall="$wall" \
            'BEGIN { printf "%.1f", frames / wall }')
        printf '%-22s %10.3f %10s %10.3f\n' \
            "$codec-$mode" "$wall" "$fps" "$cpu"
    done
done

exact="$work/exact"
mkdir "$exact"
exact_size=320x240
exact_seen=
for exact_dim in "$exact_size" "${width}x${height}"; do
    if [ "$exact_dim" = "$exact_seen" ]; then
        continue
    fi
    exact_seen=$exact_dim
    exact_raw="$exact/$exact_dim.nv12"
    "$ffmpeg_bin" -nostdin -hide_banner -loglevel error -y \
        -f lavfi -i "testsrc2=size=$exact_dim:rate=30" \
        -frames:v "$frames" -pix_fmt nv12 -f rawvideo "$exact_raw"
    exact_width=${exact_dim%x*}
    exact_height=${exact_dim#*x}
    for codec in h264 h265; do
        for mode in sync d1 d2; do
            output="$exact/$codec-$mode-$exact_dim"
            if [ "$codec" = h264 ]; then
                env RKMPP_ENCODER_PIPELINE="$([ "$mode" = sync ] && printf 0 || printf 1)" \
                    RKMPP_ENCODER_PIPELINE_DEPTH="${mode#d}" \
                    "$rust_bin" h264-enc "$exact_raw" "$output" \
                    "$exact_width" "$exact_height" 26 high "$frames" 60 \
                    >/dev/null 2>&1
            else
                env RKMPP_ENCODER_PIPELINE="$([ "$mode" = sync ] && printf 0 || printf 1)" \
                    RKMPP_ENCODER_PIPELINE_DEPTH="${mode#d}" H265_GOP=60 \
                    "$rust_bin" h265-gop "$exact_raw" "$output" \
                    "$exact_width" "$exact_height" "$frames" 26 \
                    >/dev/null 2>&1
            fi
        done
    done

    cmp "$exact/h264-sync-$exact_dim" "$exact/h264-d1-$exact_dim"
    cmp "$exact/h265-sync-$exact_dim" "$exact/h265-d1-$exact_dim"
    cmp "$exact/h265-sync-$exact_dim" "$exact/h265-d2-$exact_dim"
    h264_d2_frames=$(
        "$ffprobe_bin" -v error -count_frames -select_streams v:0 \
            -show_entries stream=nb_read_frames -of default=nw=1:nk=1 \
            "$exact/h264-d2-$exact_dim"
    )
    [ "$h264_d2_frames" = "$frames" ]
done
printf '\nexact: H.264 d1, H.265 d1/d2 match sync for %s frames at %s and %sx%s; H.264 d2 decodes every frame\n' \
    "$frames" "$exact_size" "$width" "$height"
