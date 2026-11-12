#!/usr/bin/env bash
BOARD="${BOARD:-t6}"
# Generate VDPP2 scale register images with the vendor C builder and compare
# them against the Rust HAL. A fake mpp_service ioctl shim lets this run on t6,
# which has no client 29 hardware.
set -uo pipefail

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target \
  --exclude Cargo.lock --exclude .worktrees --exclude refs --exclude '._*' \
  -cf - . | ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs &&
  cargo build --release --offline --example vdpp2_dump 2>&1 | tail -1'

mpp_ref="${MPP_REF:-/tmp/mpp-old}"
mpp_lib="$mpp_ref/build/mpp"
if ! ssh "$BOARD" "test -f $mpp_lib/librockchip_mpp.so.1"; then
  printf 'SKIP vdpp2-regs (reference build missing at %s)\n' "$mpp_lib"
  exit 0
fi

scp -q tools/mpp_vdpp_fake.c tools/vdpp2_probe.c "$BOARD":/tmp/
if ! ssh "$BOARD" "cc -O2 -Wall -Wextra -Werror -shared -fPIC /tmp/mpp_vdpp_fake.c -ldl -o /tmp/mpp_vdpp_fake.so &&
  cc -O2 -w -I$mpp_ref/inc -I$mpp_ref/mpp/vproc/vdpp -I$mpp_ref/mpp/vproc/inc -I$mpp_ref/osal/inc \
    /tmp/vdpp2_probe.c $mpp_ref/mpp/vproc/vdpp/vdpp2.c $mpp_ref/mpp/vproc/vdpp/vdpp_common.c \
    $mpp_lib/librockchip_mpp.so.1 -lpthread -lm -o /tmp/vdpp2_probe"; then
  printf 'FAIL vdpp2-regs (C probe build failed)\n'
  exit 1
fi

fail=0
run_cell() {
  local name="$1" src_w="$2" src_h="$3" dst_w="$4" dst_h="$5" fmt="$6"
  local input="/tmp/vdpp2-reg-$name.nv12"
  local trace="/tmp/vdpp2-reg-$name.trace"
  local rust="/tmp/vdpp2-reg-$name.rust"
  local compare_args=()
  if [ "$fmt" = hist ]; then
    compare_args=(--hist)
  fi
  ssh "$BOARD" "ffmpeg -v error -y -f lavfi -i testsrc2=size=${src_w}x${src_h}:rate=30 \
    -frames:v 1 -pix_fmt yuv420p -f rawvideo $input"
  if ! ssh "$BOARD" "sudo env LD_LIBRARY_PATH=$mpp_lib LD_PRELOAD=/tmp/mpp_vdpp_fake.so \
      /tmp/vdpp2_probe $input /tmp/vdpp2-reg-$name.yuv $src_w $src_h $dst_w $dst_h $fmt \
      >/dev/null 2>$trace &&
    ~/rk-mpp-rs/target/release/examples/vdpp2_dump $src_w $src_h $dst_w $dst_h $fmt scale-only >$rust"; then
    printf 'FAIL vdpp2-reg/%s (trace generation failed)\n' "$name"
    fail=1
    return
  fi
  scp -q "$BOARD":"$trace" "$BOARD":"$rust" /tmp/
  if [ "${#compare_args[@]}" -eq 0 ]; then
    if python3 tools/compare_vdpp2.py "/tmp/vdpp2-reg-$name.rust" \
        "/tmp/vdpp2-reg-$name.trace"; then
      printf 'PASS vdpp2-reg/%s\n' "$name"
    else
      printf 'FAIL vdpp2-reg/%s (register words differ)\n' "$name"
      fail=1
    fi
  elif python3 tools/compare_vdpp2.py "${compare_args[@]}" \
      "/tmp/vdpp2-reg-$name.rust" "/tmp/vdpp2-reg-$name.trace"; then
    printf 'PASS vdpp2-reg/%s\n' "$name"
  else
    printf 'FAIL vdpp2-reg/%s (register words differ)\n' "$name"
    fail=1
  fi
}

run_cell up_720_to_1080 720 480 1920 1080 444
run_cell down_1080_to_640 1920 1080 640 640 420
run_cell identity_640 640 480 640 480 420
run_cell hist_720 720 480 1920 1080 hist

run_enh_cell() {
  local name=enh_720_to_1080
  local input=/tmp/vdpp2-reg-$name.nv12
  local trace=/tmp/vdpp2-reg-$name.trace
  local rust=/tmp/vdpp2-reg-$name.rust
  ssh "$BOARD" "ffmpeg -v error -y -f lavfi -i testsrc2=size=720x480:rate=30 \
    -frames:v 1 -pix_fmt yuv420p -f rawvideo $input"
  if ! ssh "$BOARD" "sudo env LD_LIBRARY_PATH=$mpp_lib LD_PRELOAD=/tmp/mpp_vdpp_fake.so \
      /tmp/vdpp2_probe $input /tmp/vdpp2-reg-$name.yuv 720 480 1920 1080 444 enh \
      >/dev/null 2>$trace &&
    ~/rk-mpp-rs/target/release/examples/vdpp2_dump 720 480 1920 1080 444 >$rust"; then
    printf 'FAIL vdpp2-reg/%s (trace generation failed)\n' "$name"
    fail=1
    return
  fi
  scp -q "$BOARD":"$trace" "$BOARD":"$rust" /tmp/
  if python3 tools/compare_vdpp2.py --enh "/tmp/vdpp2-reg-$name.rust" \
      "/tmp/vdpp2-reg-$name.trace"; then
    printf 'PASS vdpp2-reg/%s\n' "$name"
  else
    printf 'FAIL vdpp2-reg/%s (register words differ)\n' "$name"
    fail=1
  fi
}

run_enh_cell

exit "$fail"
