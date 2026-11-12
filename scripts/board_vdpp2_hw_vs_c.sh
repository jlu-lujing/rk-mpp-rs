#!/usr/bin/env bash

# Compare RK3576 VDPP2 scale, enhancement and histogram output with vendor C.

set -euo pipefail

board="${BOARD:-lf1}"

ssh "$board" 'mkdir -p "$HOME/rk-mpp-rs"'
COPYFILE_DISABLE=1 tar \
    --no-xattrs \
    --exclude .git \
    --exclude target \
    --exclude .worktrees \
    --exclude refs \
    -cf - . |
    ssh "$board" 'tar -xf - -C "$HOME/rk-mpp-rs"'

ssh "$board" 'bash -s' <<'REMOTE'
set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"
cd "$HOME/rk-mpp-rs"
cargo build --release --offline --bin rk-mpp-rs

probe=$(./target/release/rk-mpp-rs vdpp-probe)
printf '%s\n' "$probe"
if ! grep -q 'kernel present .*: true' <<<"$probe"; then
    echo "SKIP vdpp2 hardware: client 29 is not exposed"
    exit 0
fi

mpp_src="${MPP_SRC:-}"
if [ -z "$mpp_src" ]; then
    for candidate in \
        "$HOME/rk-mpp-oracles/mpp-old" \
        "$HOME/rk-mpp-oracles/mpp-develop" \
        /tmp/mpp-old \
        /tmp/mpp-develop; do
        if [ -f "$candidate/mpp/vproc/vdpp/vdpp2.c" ]; then
            mpp_src="$candidate"
            break
        fi
    done
fi
if [ -z "$mpp_src" ] ||
    [ ! -f "$mpp_src/mpp/vproc/vdpp/vdpp2.c" ] ||
    [ ! -f "$mpp_src/mpp/vproc/vdpp/vdpp_common.c" ]; then
    echo "SKIP vdpp2 hardware: set MPP_SRC to a C MPP source tree" >&2
    exit 0
fi

mpp_lib_dir="$mpp_src/build/mpp"
if ! compgen -G "$mpp_lib_dir/librockchip_mpp.so*" >/dev/null; then
    mpp_lib_dir=/usr/local/lib
fi
if ! compgen -G "$mpp_lib_dir/librockchip_mpp.so*" >/dev/null; then
    echo "SKIP vdpp2 hardware: C MPP shared library is missing" >&2
    exit 0
fi
cc -O2 -w -pthread \
    -I"$mpp_src/inc" \
    -I"$mpp_src/mpp/vproc/vdpp" \
    -I"$mpp_src/mpp/vproc/inc" \
    -I"$mpp_src/osal/inc" \
    tools/vdpp2_probe.c \
    "$mpp_src/mpp/vproc/vdpp/vdpp2.c" \
    "$mpp_src/mpp/vproc/vdpp/vdpp_common.c" \
    -L"$mpp_lib_dir" -lrockchip_mpp \
    -lpthread -lm -Wl,-rpath,"$mpp_lib_dir" -o /tmp/vdpp2_probe_hw

ffmpeg -nostdin -hide_banner -loglevel error -y \
    -f lavfi -i "testsrc2=size=1280x720:rate=30" -frames:v 1 \
    -pix_fmt nv12 -f rawvideo /tmp/vdpp2-hw-1280x720.nv12

run_scale() {
    local name="$1"
    local c_extra="$2"
    local rust_extra="$3"

    LD_LIBRARY_PATH="$mpp_lib_dir" timeout 120 /tmp/vdpp2_probe_hw \
        /tmp/vdpp2-hw-1280x720.nv12 /tmp/vdpp2-hw-${name}-c.yuv \
        1280 720 640 360 420 $c_extra \
        > /tmp/vdpp2-hw-${name}-c.log 2>&1
    timeout 120 ./target/release/rk-mpp-rs vdpp-scale \
        /tmp/vdpp2-hw-1280x720.nv12 /tmp/vdpp2-hw-${name}-rust.yuv \
        1280 720 640 360 420 $rust_extra \
        > /tmp/vdpp2-hw-${name}-rust.log 2>&1
    cmp /tmp/vdpp2-hw-${name}-c.yuv /tmp/vdpp2-hw-${name}-rust.yuv
    printf 'PASS vdpp2-hw/%s %s bytes\n' "$name" \
        "$(stat -c %s /tmp/vdpp2-hw-${name}-c.yuv)"
}

run_scale scale-only "" scale-only
run_scale enhancement enh ""

run_split() {
    local fmt="$1"
    local luma_bytes=$((640 * 360))
    local chroma_bytes
    if [ "$fmt" = 420 ]; then
        chroma_bytes=$((320 * 180 * 2))
    else
        chroma_bytes=$((640 * 360 * 2))
    fi

    sudo rm -f /tmp/vdpp2-hw-split-c-chroma.bin
    sudo env VDPP_CHROMA_OUT=/tmp/vdpp2-hw-split-c-chroma.bin \
        LD_LIBRARY_PATH="$mpp_lib_dir" timeout 120 /tmp/vdpp2_probe_hw \
        /tmp/vdpp2-hw-1280x720.nv12 /tmp/vdpp2-hw-split-c.yuv \
        1280 720 640 360 "$fmt" split \
        > /tmp/vdpp2-hw-split-c.log 2>&1
    timeout 120 ./target/release/rk-mpp-rs vdpp-scale \
        /tmp/vdpp2-hw-1280x720.nv12 /tmp/vdpp2-hw-split-rust.yuv \
        1280 720 640 360 "$fmt" split scale-only \
        > /tmp/vdpp2-hw-split-rust.log 2>&1

    test "$(stat -c %s /tmp/vdpp2-hw-split-c-chroma.bin)" -eq "$chroma_bytes"
    cmp -n "$luma_bytes" /tmp/vdpp2-hw-split-c.yuv /tmp/vdpp2-hw-split-rust.yuv
    cmp /tmp/vdpp2-hw-split-c-chroma.bin \
        <(tail -c "$chroma_bytes" /tmp/vdpp2-hw-split-rust.yuv)
    printf 'PASS vdpp2-hw/split%s luma %s + chroma %s bytes\n' \
        "$fmt" "$luma_bytes" "$chroma_bytes"
}

run_split 420
run_split 444

sudo rm -f /tmp/vdpp2-hw-hist-c.packed
sudo env VDPP_HIST_PACKED_OUT=/tmp/vdpp2-hw-hist-c.packed \
    LD_LIBRARY_PATH="$mpp_lib_dir" timeout 120 /tmp/vdpp2_probe_hw \
    /tmp/vdpp2-hw-1280x720.nv12 /tmp/vdpp2-hw-hist-c.yuv \
    1280 720 1280 720 hist \
    > /tmp/vdpp2-hw-hist-c.log 2>&1
timeout 120 ./target/release/rk-mpp-rs vdpp-hist \
    /tmp/vdpp2-hw-1280x720.nv12 /tmp/vdpp2-hw-hist-rust 1280 720 \
    > /tmp/vdpp2-hw-hist-rust.log 2>&1

test "$(stat -c %s /tmp/vdpp2-hw-hist-c.packed)" -eq 10240
test "$(stat -c %s /tmp/vdpp2-hw-hist-rust.packed)" -eq 10240
test "$(stat -c %s /tmp/vdpp2-hw-hist-rust.local.bin)" -eq 16384
test "$(stat -c %s /tmp/vdpp2-hw-hist-rust.global.bin)" -eq 1024
cmp /tmp/vdpp2-hw-hist-c.packed /tmp/vdpp2-hw-hist-rust.packed
python3 tools/compare_vdpp2_hist.py \
    /tmp/vdpp2-hw-hist-c.packed \
    /tmp/vdpp2-hw-hist-rust.local.bin \
    /tmp/vdpp2-hw-hist-rust.global.bin \
    /tmp/vdpp2-hw-hist-rust.log
printf 'PASS vdpp2-hw/histogram packed + mean luma: %s\n' \
    "$(grep '^mean_luma_u10=' /tmp/vdpp2-hw-hist-rust.log)"
REMOTE
