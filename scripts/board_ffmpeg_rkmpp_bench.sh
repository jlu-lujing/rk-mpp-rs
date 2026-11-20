#!/usr/bin/env bash

# Three-way RK3588 benchmark:
#   C ffmpeg rkmpp / ffmpeg loaded with this crate's MPP ABI / native rk-mpp-rs
#
# Run on the board. Decode consumes the same 600-frame 720p streams and writes
# NV12 to /dev/null. Encode consumes the same 300-frame NV12 file with fixed QP
# 26 and GOP 60. Samples are interleaved and the path order alternates.
set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"

export PATH="$HOME/.cargo/bin:$PATH"
unset FFMPEG_RKMPP_DEC_OPT || true

samples=${SAMPLES:-5}
decode_frames=${DECODE_FRAMES:-600}
encode_frames=${ENCODE_FRAMES:-300}
width=${WIDTH:-1280}
height=${HEIGHT:-720}
ffmpeg_bin=$(command -v ffmpeg)
ffprobe_bin=$(command -v ffprobe)
rust_bin="$repo/target/release/rk-mpp-rs"
work=$(mktemp -d /tmp/rk-mpp-ffmpeg-bench.XXXXXX)
overlay="$work/overlay"
trap 'rm -rf "$work"' EXIT

mkdir -p "$overlay"
cargo build --release --offline --bin rk-mpp-rs --lib
ln -s "$repo/target/release/librk_mpp_rs.so" \
    "$overlay/librockchip_mpp.so.1"

if ! LD_LIBRARY_PATH="$overlay" ldd "$ffmpeg_bin" |
    grep -F "librockchip_mpp.so.1 => $overlay/librockchip_mpp.so.1" >/dev/null; then
    printf 'FFmpeg did not resolve librockchip_mpp.so.1 to the Rust library\n' >&2
    exit 1
fi

h264="$work/decode-h264.264"
h265="$work/decode-h265.hevc"
raw="$work/encode-$width"x"$height.nv12"

"$ffmpeg_bin" -nostdin -hide_banner -loglevel error -y \
    -f lavfi -i "testsrc2=size=$width"x"$height:rate=30" \
    -frames:v "$decode_frames" -pix_fmt nv12 -c:v h264_rkmpp \
    -rc_mode CQP -qp_init 26 -g 60 -f h264 "$h264"
"$ffmpeg_bin" -nostdin -hide_banner -loglevel error -y \
    -f lavfi -i "testsrc2=size=$width"x"$height:rate=30" \
    -frames:v "$decode_frames" -pix_fmt nv12 -c:v hevc_rkmpp \
    -rc_mode CQP -qp_init 26 -g 60 -f hevc "$h265"
"$ffmpeg_bin" -nostdin -hide_banner -loglevel error -y \
    -f lavfi -i "testsrc2=size=$width"x"$height:rate=30" \
    -frames:v "$encode_frames" -pix_fmt nv12 -f rawvideo "$raw"

check_frames() {
    local path=$1
    local expected=$2
    local actual
    actual=$("$ffprobe_bin" -v error -count_frames -select_streams v:0 \
        -show_entries stream=nb_read_frames -of default=nw=1:nk=1 "$path")
    if [ "$actual" != "$expected" ]; then
        printf '%s has %s frames, expected %s\n' "$path" "$actual" "$expected" >&2
        exit 1
    fi
}

check_frames "$h264" "$decode_frames"
check_frames "$h265" "$decode_frames"

printf 'samples=%s decode_frames=%s encode_frames=%s size=%sx%s\n' \
    "$samples" "$decode_frames" "$encode_frames" "$width" "$height"
printf 'governors:'
for policy in /sys/devices/system/cpu/cpufreq/policy*/scaling_governor; do
    printf ' %s' "$(cat "$policy")"
done
printf '\n'

TIMEFORMAT='%3R %3U %3S'
declare -A wall_values=()
declare -A cpu_values=()

run_timed() {
    local key=$1
    shift
    local time_file="$work/$key.time"
    local log_file="$work/$key.log"
    local status

    set +e
    { time "$@" >"$log_file" 2>&1; } 2>"$time_file"
    status=$?
    set -e
    if [ "$status" -ne 0 ]; then
        printf 'FAIL %s (exit %s)\n' "$key" "$status" >&2
        tail -30 "$log_file" >&2
        exit "$status"
    fi
    read -r bench_wall bench_user bench_sys <"$time_file"
    bench_cpu=$(awk -v u="$bench_user" -v s="$bench_sys" 'BEGIN { print u + s }')
}

decode_ffmpeg_c() {
    local codec=$1
    local input=$2
    local ff_codec=$codec
    [ "$codec" = h265 ] && ff_codec=hevc
    "$ffmpeg_bin" -nostdin -hide_banner -loglevel error \
        -c:v "${ff_codec}_rkmpp" -i "$input" \
        -fps_mode passthrough -pix_fmt nv12 -f rawvideo -y /dev/null
}

decode_ffmpeg_rs() {
    local codec=$1
    local input=$2
    local ff_codec=$codec
    [ "$codec" = h265 ] && ff_codec=hevc
    env LD_LIBRARY_PATH="$overlay" "$ffmpeg_bin" \
        -nostdin -hide_banner -loglevel error \
        -c:v "${ff_codec}_rkmpp" -i "$input" \
        -fps_mode passthrough -pix_fmt nv12 -f rawvideo -y /dev/null
}

decode_rust_native() {
    local codec=$1
    local input=$2
    local rust_codec=$codec
    [ "$codec" = h265 ] && rust_codec=hevc
    env RKMPP_DECODE_NV12=1 "$rust_bin" "${rust_codec}dec" "$input" /dev/null
}

encode_ffmpeg_c() {
    local codec=$1
    local ff_codec=$codec
    local muxer=$codec
    local profile=high
    if [ "$codec" = h265 ]; then
        ff_codec=hevc
        muxer=hevc
        profile=main
    fi
    "$ffmpeg_bin" -nostdin -hide_banner -loglevel error \
        -f rawvideo -pix_fmt nv12 -s "${width}x${height}" -r 30 -i "$raw" \
        -frames:v "$encode_frames" -c:v "${ff_codec}_rkmpp" \
        -rc_mode CQP -qp_init 26 -g 60 -profile:v "$profile" \
        -f "$muxer" -y /dev/null
}

encode_ffmpeg_rs() {
    local codec=$1
    local ff_codec=$codec
    local muxer=$codec
    local profile=high
    if [ "$codec" = h265 ]; then
        ff_codec=hevc
        muxer=hevc
        profile=main
    fi
    env LD_LIBRARY_PATH="$overlay" "$ffmpeg_bin" \
        -nostdin -hide_banner -loglevel error \
        -f rawvideo -pix_fmt nv12 -s "${width}x${height}" -r 30 -i "$raw" \
        -frames:v "$encode_frames" -c:v "${ff_codec}_rkmpp" \
        -rc_mode CQP -qp_init 26 -g 60 -profile:v "$profile" \
        -f "$muxer" -y /dev/null
}

encode_rust_native() {
    local codec=$1
    if [ "$codec" = h264 ]; then
        "$rust_bin" h264-enc "$raw" /dev/null "$width" "$height" \
            26 high "$encode_frames" 60
    else
        env H265_GOP=60 "$rust_bin" h265-gop "$raw" /dev/null \
            "$width" "$height" "$encode_frames" 26
    fi
}

run_path() {
    local case_key=$1
    local path=$2
    local sample=$3
    local codec=${case_key#*-}
    local key="$case_key-$path-$sample"

    case "$path" in
        c-ffmpeg)
            if [[ "$case_key" == decode-* ]]; then
                local input=$h264
                [ "$codec" = h265 ] && input=$h265
                run_timed "$key" decode_ffmpeg_c "$codec" "$input"
            else
                run_timed "$key" encode_ffmpeg_c "$codec"
            fi
            ;;
        rust-ffmpeg)
            if [[ "$case_key" == decode-* ]]; then
                local input=$h264
                [ "$codec" = h265 ] && input=$h265
                run_timed "$key" decode_ffmpeg_rs "$codec" "$input"
            else
                run_timed "$key" encode_ffmpeg_rs "$codec"
            fi
            ;;
        rust-native)
            if [[ "$case_key" == decode-* ]]; then
                local input=$h264
                [ "$codec" = h265 ] && input=$h265
                run_timed "$key" decode_rust_native "$codec" "$input"
            else
                run_timed "$key" encode_rust_native "$codec"
            fi
            ;;
    esac

    wall_values["$case_key:$path"]+="$bench_wall "
    cpu_values["$case_key:$path"]+="$bench_cpu "
}

median() {
    printf '%s\n' "$@" | sort -n |
        awk '{ value[NR] = $1 } END { print value[int((NR + 1) / 2)] }'
}

print_case() {
    local case_key=$1
    local frames=$2
    local paths=(c-ffmpeg rust-ffmpeg rust-native)
    local labels=("C ffmpeg" "ffmpeg+rkmpp-rs" "native rkmpp-rs")
    local c_wall
    local i

    read -ra values <<<"${wall_values["$case_key:c-ffmpeg"]}"
    c_wall=$(median "${values[@]}")
    printf '\nCASE %s frames=%s\n' "$case_key" "$frames"
    for i in "${!paths[@]}"; do
        local path=${paths[$i]}
        local label=${labels[$i]}
        local wall cpu fps speed cpu_pct
        read -ra values <<<"${wall_values["$case_key:$path"]}"
        wall=$(median "${values[@]}")
        read -ra values <<<"${cpu_values["$case_key:$path"]}"
        cpu=$(median "${values[@]}")
        fps=$(awk -v f="$frames" -v t="$wall" 'BEGIN { printf "%.1f", f / t }')
        speed=$(awk -v c="$c_wall" -v r="$wall" \
            'BEGIN { printf "%+.1f", (c / r - 1) * 100 }')
        cpu_pct=$(awk -v cpu="$cpu" -v wall="$wall" \
            'BEGIN { printf "%.0f", cpu / wall * 100 }')
        printf '%-18s wall=%8.3f s fps=%8s cpu=%6.3f s (%3s%%) vs_c=%7s%%\n' \
            "$label" "$wall" "$fps" "$cpu" "$cpu_pct" "$speed"
    done
}

run_case() {
    local case_key=$1
    local sample
    local path
    local forward=(c-ffmpeg rust-ffmpeg rust-native)
    local reverse=(rust-native rust-ffmpeg c-ffmpeg)

    for path in "${forward[@]}"; do
        run_path "$case_key" "$path" warmup
    done
    for sample in $(seq 1 "$samples"); do
        if [ $((sample % 2)) -eq 1 ]; then
            for path in "${forward[@]}"; do
                run_path "$case_key" "$path" "$sample"
            done
        else
            for path in "${reverse[@]}"; do
                run_path "$case_key" "$path" "$sample"
            done
        fi
    done
}

for case_key in decode-h264 decode-h265 encode-h264 encode-h265; do
    run_case "$case_key"
done

print_case decode-h264 "$decode_frames"
print_case decode-h265 "$decode_frames"
print_case encode-h264 "$encode_frames"
print_case encode-h265 "$encode_frames"
