#!/usr/bin/env bash

set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$repo"

export PATH="$HOME/.cargo/bin:$PATH"
mpp_root="${MPP_REF:-}"
if [ -z "$mpp_root" ]; then
    for candidate in /tmp/mpp-develop "$HOME/rk-mpp-oracles/mpp-develop" \
        "$HOME/rk-mpp-oracles/mpp-old"; do
        if [ -x "$candidate/build/test/mpi_dec_test" ] &&
            [ -f "$candidate/build/mpp/librockchip_mpp.so.1" ]; then
            mpp_root=$candidate
            break
        fi
    done
    mpp_root=${mpp_root:-/tmp/mpp-develop}
fi
dec_test="${DEC_TEST:-$mpp_root/build/test/mpi_dec_test}"
vendor_lib="${MPP_LIB:-$mpp_root/build/mpp}"
h264="${H264_STREAM:-/tmp/rk_mpp_standalone_h264.264}"
overlay=$(mktemp -d /tmp/rk_mpp_standalone.XXXXXX)
trap 'rm -rf "$overlay"' EXIT

if [ -z "${MPP_REF:-}" ] && [ -z "${DEC_TEST:-}" ] &&
    [ -z "${MPP_LIB:-}" ] && [ ! -x "$dec_test" ] &&
    [ ! -f "$vendor_lib/librockchip_mpp.so.1" ] &&
    [ -x /usr/local/bin/mpi_dec_test ] &&
    [ -f /usr/local/lib/librockchip_mpp.so.1 ]; then
    dec_test=/usr/local/bin/mpi_dec_test
    vendor_lib=/usr/local/lib
fi
if [ ! -x "$dec_test" ] || [ ! -f "$vendor_lib/librockchip_mpp.so.1" ]; then
    printf 'missing vendor mpi_dec_test or comparison library under %s\n' \
        "$mpp_root" >&2
    exit 2
fi

cargo build --release --offline --lib
ln -s "$repo/target/release/librk_mpp_rs.so" \
    "$overlay/librockchip_mpp.so.1"

if ! LD_LIBRARY_PATH="$overlay" ldd "$dec_test" |
    grep -F "librockchip_mpp.so.1 => $overlay/librockchip_mpp.so.1" >/dev/null; then
    printf 'standalone library did not replace vendor librockchip_mpp\n' >&2
    exit 1
fi

run_case() {
    local name="$1"
    local input="$2"
    local coding="$3"
    local frames="$4"
    local width="$5"
    local height="$6"
    local golden="/tmp/rk_mpp_standalone_${name}_vendor.yuv"
    local ours="/tmp/rk_mpp_standalone_${name}_ours.yuv"

    sudo -n rm -f "$golden" "$ours"
    sudo -n env LD_LIBRARY_PATH="$vendor_lib:${LD_LIBRARY_PATH:-}" \
        timeout 120 "$dec_test" -i "$input" -o "$golden" \
        -w "$width" -h "$height" -t "$coding" -f 0 -n "$frames" -v q \
        < /dev/null > "/tmp/rk_mpp_standalone_${name}_vendor.log" 2>&1
    sudo -n env LD_LIBRARY_PATH="$overlay" \
        timeout 120 "$dec_test" -i "$input" -o "$ours" \
        -w "$width" -h "$height" -t "$coding" -f 0 -n "$frames" -v q \
        < /dev/null > "/tmp/rk_mpp_standalone_${name}_ours.log" 2>&1
    grep -F "decoder test start w $width h $height type $coding" \
        "/tmp/rk_mpp_standalone_${name}_ours.log" >/dev/null
    local expected_bytes=$((width * height * 3 / 2 * frames))
    test "$(stat -c %s "$ours")" -eq "$expected_bytes"
    cmp "$golden" "$ours"
    printf 'PASS %-8s standalone decoder (%s, %s bytes)\n' \
        "$name" "$(sha256sum "$ours" | cut -d' ' -f1)" "$(stat -c %s "$ours")"
}

run_eos_case() {
    local name="$1"
    local input="$2"
    local coding="$3"
    local frames="$4"
    local width="$5"
    local height="$6"
    local golden="/tmp/rk_mpp_standalone_${name}_vendor.yuv"
    local ours="/tmp/rk_mpp_standalone_${name}_ours.yuv"

    sudo -n rm -f "$golden" "$ours"
    sudo -n env LD_LIBRARY_PATH="$vendor_lib:${LD_LIBRARY_PATH:-}" \
        timeout 120 "$dec_test" -i "$input" -o "$golden" \
        -w "$width" -h "$height" -t "$coding" -f 0 -v q \
        < /dev/null > "/tmp/rk_mpp_standalone_${name}_vendor.log" 2>&1
    sudo -n env LD_LIBRARY_PATH="$overlay" \
        timeout 120 "$dec_test" -i "$input" -o "$ours" \
        -w "$width" -h "$height" -t "$coding" -f 0 -v q \
        < /dev/null > "/tmp/rk_mpp_standalone_${name}_ours.log" 2>&1
    grep -F "decoder test start w $width h $height type $coding" \
        "/tmp/rk_mpp_standalone_${name}_ours.log" >/dev/null
    local expected_bytes=$((width * height * 3 / 2 * frames))
    test "$(stat -c %s "$ours")" -eq "$expected_bytes"
    cmp "$golden" "$ours"
    printf 'PASS %-8s standalone EOS decoder (%s, %s bytes)\n' \
        "$name" "$(sha256sum "$ours" | cut -d' ' -f1)" "$(stat -c %s "$ours")"
}

ffmpeg -nostdin -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=640x480:rate=30" -frames:v 6 \
    -pix_fmt yuv420p -c:v libx264 -preset veryfast \
    -x264-params "keyint=30:min-keyint=30:bframes=3:scenecut=0:repeat-headers=1" \
    -f h264 "$h264"
run_case h264 "$h264" 7 6 640 480

run_case h265 tests/data/h265e_c_fixqp_qp26_320x240.265 16777220 1 320 240
run_case vp8 tests/data/vp8_320x240_12_ip.ivf 9 12 320 240
run_case vp8_altref tests/data/vp8_320x240_24_altref.ivf 9 24 320 240
run_case vp9 tests/data/vp9_p0_lossless_320x240_12.ivf 10 12 320 240
run_case mpeg2 tests/data/m2v_320x240_12.m2v 2 10 320 240
run_eos_case mpeg2_eos tests/data/m2v_320x240_12.m2v 2 10 320 240
run_case mpeg4 tests/data/mp4_320x240_12_ip.m4v 4 12 320 240
run_case mpeg4_b tests/data/mp4_320x240_12_ibpb.m4v 4 12 320 240
run_case h263 tests/data/h263_cif_12_ip.263 3 12 352 288
run_case h263_qcif tests/data/h263_qcif_6_ip.263 3 6 176 144
run_case av1 tests/data/av1_320x240_10.ivf 16777224 10 320 240

mjpeg=/tmp/rk_mpp_standalone_mjpeg.jpg
ffmpeg -nostdin -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=320x240:rate=1" -frames:v 1 \
    -pix_fmt yuvj420p -strict unofficial -c:v mjpeg -q:v 3 "$mjpeg"
run_case mjpeg "$mjpeg" 8 1 320 240
