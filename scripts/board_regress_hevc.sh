#!/usr/bin/env bash
BOARD="${BOARD:-t6}"
# HEVC regression: the CLI output must already be in display order.
set -euo pipefail

COPYFILE_DISABLE=1 tar --no-xattrs --exclude .git --exclude target --exclude Cargo.lock --exclude .worktrees --exclude refs -cf - . |
  ssh "$BOARD" 'tar -xf - -C ~/rk-mpp-rs'
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd ~/rk-mpp-rs && cargo build --release'

fail=0

decode_component() {
  local input="$1"
  local output="$2"
  if ! ssh "$BOARD" "test -f $input"; then
    printf 'SKIP %s (stream missing)\n' "$input"
    return
  fi
  ssh "$BOARD" "sudo rm -f $output"
  if ! ssh "$BOARD" "cd ~/rk-mpp-rs && sudo ./target/release/rk-mpp-rs hevcdec $input $output" >/dev/null 2>&1; then
    printf 'FAIL %s (decoder exited non-zero)\n' "$input"
    fail=1
    return
  fi
}

decode_component /tmp/t720_nob.hevc /tmp/t720_nob_rust_display.yuv
if ssh "$BOARD" 'test -f /tmp/mpp_hevc_nob_planar.yuv'; then
  if ssh "$BOARD" 'cmp -s /tmp/mpp_hevc_nob_planar.yuv /tmp/t720_nob_rust_display.yuv'; then
    printf 'PASS /tmp/t720_nob.hevc\n'
  else
    printf 'FAIL /tmp/t720_nob.hevc\n'
    fail=1
  fi
else
  printf 'SKIP /tmp/t720_nob.hevc (MPP reference missing)\n'
fi

decode_component /tmp/t720.hevc /tmp/t720_rust_display.yuv
if ssh "$BOARD" 'test -f /tmp/mpp_hevc_b.yuv'; then
  if ssh "$BOARD" 'ffmpeg -loglevel error -y -f rawvideo -pixel_format nv12 -video_size 1280x720 -i /tmp/mpp_hevc_b.yuv -pix_fmt yuv420p -f rawvideo /tmp/t720_mpp_display_planar.yuv'; then
    if ssh "$BOARD" 'cmp -s /tmp/t720_mpp_display_planar.yuv /tmp/t720_rust_display.yuv'; then
      printf 'PASS /tmp/t720.hevc\n'
    else
      printf 'FAIL /tmp/t720.hevc\n'
      fail=1
    fi
  else
    printf 'FAIL /tmp/t720.hevc (could not convert MPP reference)\n'
    fail=1
  fi
else
  printf 'SKIP /tmp/t720.hevc (MPP reference missing)\n'
fi

decode_component /tmp/t1080.hevc /tmp/t1080_rust_display.yuv
if ssh "$BOARD" 'test -f /tmp/t1080_mpp_ref.yuv'; then
  if ssh "$BOARD" 'ffmpeg -loglevel error -y -f rawvideo -pixel_format nv12 -video_size 1920x1080 -i /tmp/t1080_mpp_ref.yuv -pix_fmt yuv420p -f rawvideo /tmp/t1080_mpp_display_planar.yuv'; then
    if ssh "$BOARD" 'cmp -s /tmp/t1080_mpp_display_planar.yuv /tmp/t1080_rust_display.yuv'; then
      printf 'PASS /tmp/t1080.hevc\n'
    else
      printf 'FAIL /tmp/t1080.hevc\n'
      fail=1
    fi
  else
    printf 'FAIL /tmp/t1080.hevc (could not convert MPP reference)\n'
    fail=1
  fi
else
  printf 'SKIP /tmp/t1080.hevc (MPP reference missing)\n'
fi

if ! ./scripts/gen_hevc_tiles.sh; then
  printf 'FAIL /tmp/hevc_tiles_720p.hevc (could not generate MPP tiled stream)\n'
  fail=1
elif ! ssh "$BOARD" 'test -x /tmp/mpp-develop/build/test/mpi_dec_test &&
    test -f /tmp/mpp-develop/build/mpp/librockchip_mpp.so.0'; then
  printf 'SKIP /tmp/hevc_tiles_720p.hevc (MPP decoder binary or library missing)\n'
else
  ssh "$BOARD" 'sudo rm -f /tmp/hevc_tiles_mpp.yuv \
    /tmp/hevc_tiles_mpp_planar.yuv /tmp/hevc_tiles_rust.yuv'
  ssh "$BOARD" 'sudo env LD_LIBRARY_PATH=/tmp/mpp-develop/build/mpp \
    /tmp/mpp-develop/build/test/mpi_dec_test \
    -i /tmp/hevc_tiles_720p.hevc -t 16777220 \
    -o /tmp/hevc_tiles_mpp.yuv -w 1280 -h 720 -n 30 -v q \
    >/tmp/hevc_tiles_mpp.log 2>&1'
  if ! ssh "$BOARD" 'ffmpeg -loglevel error -y -f rawvideo -pixel_format nv12 \
      -video_size 1280x720 -i /tmp/hevc_tiles_mpp.yuv \
      -pix_fmt yuv420p -f rawvideo /tmp/hevc_tiles_mpp_planar.yuv'; then
    printf 'FAIL /tmp/hevc_tiles_720p.hevc (could not convert MPP output)\n'
    fail=1
  else
    expected_bytes=$((1280 * 720 * 3 / 2 * 30))
    if ! ssh "$BOARD" "test \"\$(stat -c %s /tmp/hevc_tiles_mpp_planar.yuv)\" -eq $expected_bytes"; then
      printf 'FAIL /tmp/hevc_tiles_720p.hevc (unexpected MPP output frame count)\n'
      fail=1
    elif ! ssh "$BOARD" 'cd ~/rk-mpp-rs &&
        sudo ./target/release/rk-mpp-rs hevcdec \
        /tmp/hevc_tiles_720p.hevc /tmp/hevc_tiles_rust.yuv \
        >/tmp/hevc_tiles_rust.log 2>&1'; then
      printf 'FAIL /tmp/hevc_tiles_720p.hevc (Rust decode failed)\n'
      fail=1
    elif ! ssh "$BOARD" 'grep -q "decoded 30 frames (30 output) in" /tmp/hevc_tiles_rust.log'; then
      printf 'FAIL /tmp/hevc_tiles_720p.hevc (Rust dropped pictures)\n'
      fail=1
    elif ! ssh "$BOARD" 'cmp -s /tmp/hevc_tiles_mpp_planar.yuv /tmp/hevc_tiles_rust.yuv'; then
      printf 'FAIL /tmp/hevc_tiles_720p.hevc (picture bytes differ)\n'
      fail=1
    else
      printf 'PASS /tmp/hevc_tiles_720p.hevc (MPP auto-tile, 30 frames)\n'
    fi
  fi
fi

if ! ./scripts/gen_hevc_main10.sh; then
  printf 'FAIL /tmp/hevc_main10_256.hevc (could not generate Main10 stream)\n'
  fail=1
elif ! ssh "$BOARD" 'test -x /tmp/mpp-develop/build/test/mpi_dec_test &&
    test -f /tmp/mpp-develop/build/mpp/librockchip_mpp.so.0'; then
  printf 'SKIP /tmp/hevc_main10_256.hevc (MPP decoder binary or library missing)\n'
else
  ssh "$BOARD" 'sudo rm -f /tmp/hevc_main10_mpp.yuv \
    /tmp/hevc_main10_mpp_planar.yuv /tmp/hevc_main10_rust.yuv'
  ssh "$BOARD" 'sudo env LD_LIBRARY_PATH=/tmp/mpp-develop/build/mpp \
    /tmp/mpp-develop/build/test/mpi_dec_test \
    -i /tmp/hevc_main10_256.hevc -t 16777220 \
    -o /tmp/hevc_main10_mpp.yuv -w 256 -h 256 -n 30 -v q \
    >/tmp/hevc_main10_mpp.log 2>&1'
  if ! ssh "$BOARD" 'python3 - <<"PY"
from pathlib import Path

w, h, frames = 256, 256, 30
stride = w * 2
frame_size = w * h * 2 * 3 // 2
src = Path("/tmp/hevc_main10_mpp.yuv").read_bytes()
if len(src) != frame_size * frames:
    raise SystemExit(f"unexpected MPP byte count: {len(src)}")

out = bytearray(len(src))
half_w = w // 2
chroma_row_bytes = half_w * 2
for frame in range(frames):
    src_frame = frame * frame_size
    dst_frame = frame * frame_size
    y_bytes = w * h * 2
    out[dst_frame:dst_frame + y_bytes] = src[src_frame:src_frame + y_bytes]
    src_uv = src_frame + y_bytes
    dst_u = dst_frame + y_bytes
    dst_v = dst_u + w * h // 2
    for row in range(h // 2):
        line = src[src_uv + row * stride:src_uv + (row + 1) * stride]
        urow = bytearray(chroma_row_bytes)
        vrow = bytearray(chroma_row_bytes)
        urow[0::2] = line[0::4]
        urow[1::2] = line[1::4]
        vrow[0::2] = line[2::4]
        vrow[1::2] = line[3::4]
        u = dst_u + row * chroma_row_bytes
        v = dst_v + row * chroma_row_bytes
        out[u:u + chroma_row_bytes] = urow
        out[v:v + chroma_row_bytes] = vrow

Path("/tmp/hevc_main10_mpp_planar.yuv").write_bytes(out)
PY'; then
    printf 'FAIL /tmp/hevc_main10_256.hevc (could not convert MPP 16-bit dump)\n'
    fail=1
  elif ! ssh "$BOARD" 'cd ~/rk-mpp-rs &&
      sudo env RFI_HEVC_TRACE=1 ./target/release/rk-mpp-rs hevcdec \
      /tmp/hevc_main10_256.hevc /tmp/hevc_main10_rust.yuv \
      >/tmp/hevc_main10_rust.log 2>&1'; then
    printf 'FAIL /tmp/hevc_main10_256.hevc (Rust decode failed)\n'
    fail=1
  elif ! ssh "$BOARD" 'grep -q "decoded 30 frames (30 output) in" /tmp/hevc_main10_rust.log'; then
    printf 'FAIL /tmp/hevc_main10_256.hevc (Rust dropped pictures)\n'
    fail=1
  elif ! ssh "$BOARD" 'test "$(grep -c "err=0x0 output=true status=00000107/" \
      /tmp/hevc_main10_rust.log)" -eq 30'; then
    printf 'FAIL /tmp/hevc_main10_256.hevc (hardware status or frame error)\n'
    fail=1
  elif ! ssh "$BOARD" 'cmp -s /tmp/hevc_main10_mpp_planar.yuv /tmp/hevc_main10_rust.yuv'; then
    printf 'FAIL /tmp/hevc_main10_256.hevc (picture bytes differ)\n'
    fail=1
  else
    printf 'PASS /tmp/hevc_main10_256.hevc (Main10, 30 frames)\n'
  fi
fi

if ssh "$BOARD" 'command -v ffmpeg >/dev/null &&
    test -x /tmp/mpp-develop/build/test/mpi_enc_test &&
    test -x /tmp/mpp-develop/build/test/mpi_dec_test &&
    test -f /tmp/mpp-develop/build/mpp/librockchip_mpp.so.0 &&
    test -f /tmp/mpp-develop/test/cfg/enc/base_h265_1920x1080.json'; then
  if ! ssh "$BOARD" 'set -e
      sudo rm -f /tmp/hevc_scaling.hevc /tmp/hevc_scaling_encode.log
      sed "0,/\"scaling_list\" : 0/s//\"scaling_list\" : 1/" \
        /tmp/mpp-develop/test/cfg/enc/base_h265_1920x1080.json \
        > /tmp/mpp-develop/scaling_list_h265.json
      ffmpeg -loglevel error -y -f lavfi \
        -i testsrc2=size=1920x1080:rate=30 -frames:v 10 \
        -pix_fmt nv12 -f rawvideo /tmp/hevc_scaling_src.yuv
      sudo env LD_LIBRARY_PATH=/tmp/mpp-develop/build/mpp \
        /tmp/mpp-develop/build/test/mpi_enc_test \
        -i /tmp/hevc_scaling_src.yuv -o /tmp/hevc_scaling.hevc \
        -w 1920 -h 1080 -f 0 -t 16777220 -n 10 -v q \
        -cfg /tmp/mpp-develop/scaling_list_h265.json \
        >/tmp/hevc_scaling_encode.log 2>&1
      cd ~/rk-mpp-rs
      ./target/release/rk-mpp-rs hevcdpb /tmp/hevc_scaling.hevc \
        >/tmp/hevc_scaling_dpb.log
      grep -q "scaling_list=true" /tmp/hevc_scaling_dpb.log'; then
    printf 'FAIL /tmp/hevc_scaling.hevc (could not generate scaling-list stream)\n'
    fail=1
  else
    ssh "$BOARD" 'sudo rm -f /tmp/hevc_scaling_mpp.yuv \
      /tmp/hevc_scaling_mpp_planar.yuv /tmp/hevc_scaling_rust.yuv'
    ssh "$BOARD" 'sudo env LD_LIBRARY_PATH=/tmp/mpp-develop/build/mpp \
      /tmp/mpp-develop/build/test/mpi_dec_test \
      -i /tmp/hevc_scaling.hevc -t 16777220 \
      -o /tmp/hevc_scaling_mpp.yuv -w 1920 -h 1080 -n 10 -v q \
      >/tmp/hevc_scaling_mpp.log 2>&1'
    if ! ssh "$BOARD" 'ffmpeg -loglevel error -y -f rawvideo -pixel_format nv12 \
        -video_size 1920x1080 -i /tmp/hevc_scaling_mpp.yuv \
        -pix_fmt yuv420p -f rawvideo /tmp/hevc_scaling_mpp_planar.yuv'; then
      printf 'FAIL /tmp/hevc_scaling.hevc (could not convert MPP output)\n'
      fail=1
    else
      expected_bytes=$((1920 * 1080 * 3 / 2 * 10))
      if ! ssh "$BOARD" "test \"\$(stat -c %s /tmp/hevc_scaling_mpp_planar.yuv)\" -eq $expected_bytes"; then
        printf 'FAIL /tmp/hevc_scaling.hevc (unexpected MPP output frame count)\n'
        fail=1
      elif ! ssh "$BOARD" 'export RFI_HEVC_TRACE=1; cd ~/rk-mpp-rs &&
          sudo -E ./target/release/rk-mpp-rs hevcdec \
          /tmp/hevc_scaling.hevc /tmp/hevc_scaling_rust.yuv \
          >/tmp/hevc_scaling_rust.log 2>&1'; then
        printf 'FAIL /tmp/hevc_scaling.hevc (Rust decode failed)\n'
        fail=1
      elif ! ssh "$BOARD" 'test "$(grep -c "err=0x0 output=true status=00000107/" \
          /tmp/hevc_scaling_rust.log)" -eq 10'; then
        printf 'FAIL /tmp/hevc_scaling.hevc (hardware status or frame error)\n'
        fail=1
      elif ! ssh "$BOARD" 'cmp -s /tmp/hevc_scaling_mpp_planar.yuv /tmp/hevc_scaling_rust.yuv'; then
        printf 'FAIL /tmp/hevc_scaling.hevc (picture bytes differ)\n'
        fail=1
      else
        printf 'PASS /tmp/hevc_scaling.hevc (default scaling list, 10 frames)\n'
      fi
    fi
  fi
else
  printf 'SKIP /tmp/hevc_scaling.hevc (MPP scaling-list fixture unavailable)\n'
fi

if ssh "$BOARD" 'command -v ffmpeg >/dev/null &&
    test -x /tmp/mpp-develop/build/test/mpi_dec_test &&
    test -f /tmp/mpp-develop/build/mpp/librockchip_mpp.so.0 &&
    test -f ~/rk-mpp-rs/scripts/hevc_explicit_scalinglist.txt'; then
  if ! ssh "$BOARD" 'set -e
      ffmpeg -loglevel error -y -f lavfi \
        -i testsrc2=size=256x256:rate=30 -frames:v 10 \
        -pix_fmt yuv420p -f rawvideo /tmp/hevc_explicit_scaling_src.yuv
      ffmpeg -loglevel error -y -f rawvideo -pixel_format yuv420p \
        -video_size 256x256 -framerate 30 \
        -i /tmp/hevc_explicit_scaling_src.yuv -c:v libx265 \
        -x265-params "scaling-list=$HOME/rk-mpp-rs/scripts/hevc_explicit_scalinglist.txt:keyint=10:min-keyint=10:scenecut=0:bframes=0:log-level=warning" \
        -f hevc /tmp/hevc_explicit_scaling.hevc \
        >/tmp/hevc_explicit_scaling_encode.log 2>&1
      cd ~/rk-mpp-rs
      ./target/release/rk-mpp-rs hevcdpb /tmp/hevc_explicit_scaling.hevc \
        >/tmp/hevc_explicit_scaling_dpb.log
      grep -q "scaling_list=true" /tmp/hevc_explicit_scaling_dpb.log'; then
    printf 'FAIL /tmp/hevc_explicit_scaling.hevc (could not generate explicit scaling-list stream)\n'
    fail=1
  else
    ssh "$BOARD" 'sudo rm -f /tmp/hevc_explicit_scaling_mpp.yuv \
      /tmp/hevc_explicit_scaling_mpp_planar.yuv \
      /tmp/hevc_explicit_scaling_rust.yuv /tmp/hevc_explicit_scaling_rust.log'
    ssh "$BOARD" 'sudo env LD_LIBRARY_PATH=/tmp/mpp-develop/build/mpp \
      /tmp/mpp-develop/build/test/mpi_dec_test \
      -i /tmp/hevc_explicit_scaling.hevc -t 16777220 \
      -o /tmp/hevc_explicit_scaling_mpp.yuv -w 256 -h 256 -n 10 -v q \
      >/tmp/hevc_explicit_scaling_mpp.log 2>&1'
    if ! ssh "$BOARD" 'ffmpeg -loglevel error -y -f rawvideo -pixel_format nv12 \
        -video_size 256x256 -i /tmp/hevc_explicit_scaling_mpp.yuv \
        -pix_fmt yuv420p -f rawvideo /tmp/hevc_explicit_scaling_mpp_planar.yuv'; then
      printf 'FAIL /tmp/hevc_explicit_scaling.hevc (could not convert MPP output)\n'
      fail=1
    else
      expected_bytes=$((256 * 256 * 3 / 2 * 10))
      if ! ssh "$BOARD" "test \"\$(stat -c %s /tmp/hevc_explicit_scaling_mpp_planar.yuv)\" -eq $expected_bytes"; then
        printf 'FAIL /tmp/hevc_explicit_scaling.hevc (unexpected MPP output frame count)\n'
        fail=1
      elif ! ssh "$BOARD" 'export RFI_HEVC_TRACE=1; cd ~/rk-mpp-rs &&
          sudo -E ./target/release/rk-mpp-rs hevcdec \
          /tmp/hevc_explicit_scaling.hevc \
          /tmp/hevc_explicit_scaling_rust.yuv \
          >/tmp/hevc_explicit_scaling_rust.log 2>&1'; then
        printf 'FAIL /tmp/hevc_explicit_scaling.hevc (Rust decode failed)\n'
        fail=1
      elif ! ssh "$BOARD" 'test "$(grep -c "err=0x0 output=true status=00000107/" \
          /tmp/hevc_explicit_scaling_rust.log)" -eq 10'; then
        printf 'FAIL /tmp/hevc_explicit_scaling.hevc (hardware status or frame error)\n'
        fail=1
      elif ! ssh "$BOARD" 'cmp -s /tmp/hevc_explicit_scaling_mpp_planar.yuv \
          /tmp/hevc_explicit_scaling_rust.yuv'; then
        printf 'FAIL /tmp/hevc_explicit_scaling.hevc (picture bytes differ)\n'
        fail=1
      else
        printf 'PASS /tmp/hevc_explicit_scaling.hevc (explicit SPS list, 10 frames)\n'
      fi
    fi
  fi
else
  printf 'SKIP /tmp/hevc_explicit_scaling.hevc (explicit scaling-list fixture unavailable)\n'
fi

if ssh "$BOARD" 'command -v ffmpeg >/dev/null && command -v python3 >/dev/null &&
    test -x /tmp/mpp-develop/build/test/mpi_dec_test &&
    test -f /tmp/mpp-develop/build/mpp/librockchip_mpp.so.0 &&
    test -f ~/rk-mpp-rs/scripts/hevc_explicit_scalinglist.txt &&
    test -f ~/rk-mpp-rs/scripts/make_hevc_pps_scaling.py'; then
  if ! ssh "$BOARD" 'set -e
      ffmpeg -loglevel error -y -f lavfi \
        -i testsrc2=size=256x256:rate=30 -frames:v 10 \
        -pix_fmt yuv420p -f rawvideo /tmp/hevc_pps_scaling_src.yuv
      ffmpeg -loglevel error -y -f rawvideo -pixel_format yuv420p \
        -video_size 256x256 -framerate 30 \
        -i /tmp/hevc_pps_scaling_src.yuv -c:v libx265 \
        -x265-params "scaling-list=$HOME/rk-mpp-rs/scripts/hevc_explicit_scalinglist.txt:keyint=10:min-keyint=10:scenecut=0:bframes=0:wpp=0:log-level=warning" \
        -f hevc /tmp/hevc_pps_scaling_base.hevc \
        >/tmp/hevc_pps_scaling_encode.log 2>&1
      python3 ~/rk-mpp-rs/scripts/make_hevc_pps_scaling.py \
        /tmp/hevc_pps_scaling_base.hevc /tmp/hevc_pps_scaling.hevc
      cd ~/rk-mpp-rs
      ./target/release/rk-mpp-rs hevcdpb /tmp/hevc_pps_scaling.hevc \
        >/tmp/hevc_pps_scaling_dpb.log
      grep -q "pps scaling_list_data=true" /tmp/hevc_pps_scaling_dpb.log'; then
    printf 'FAIL /tmp/hevc_pps_scaling.hevc (could not generate explicit PPS scaling-list stream)\n'
    fail=1
  else
    ssh "$BOARD" 'sudo rm -f /tmp/hevc_pps_scaling_mpp.yuv \
      /tmp/hevc_pps_scaling_mpp_planar.yuv \
      /tmp/hevc_pps_scaling_rust.yuv /tmp/hevc_pps_scaling_rust.log'
    ssh "$BOARD" 'sudo env LD_LIBRARY_PATH=/tmp/mpp-develop/build/mpp \
      /tmp/mpp-develop/build/test/mpi_dec_test \
      -i /tmp/hevc_pps_scaling.hevc -t 16777220 \
      -o /tmp/hevc_pps_scaling_mpp.yuv -w 256 -h 256 -n 10 -v q \
      >/tmp/hevc_pps_scaling_mpp.log 2>&1'
    if ! ssh "$BOARD" 'ffmpeg -loglevel error -y -f rawvideo -pixel_format nv12 \
        -video_size 256x256 -i /tmp/hevc_pps_scaling_mpp.yuv \
        -pix_fmt yuv420p -f rawvideo /tmp/hevc_pps_scaling_mpp_planar.yuv'; then
      printf 'FAIL /tmp/hevc_pps_scaling.hevc (could not convert MPP output)\n'
      fail=1
    else
      expected_bytes=$((256 * 256 * 3 / 2 * 10))
      if ! ssh "$BOARD" "test \"\$(stat -c %s /tmp/hevc_pps_scaling_mpp_planar.yuv)\" -eq $expected_bytes"; then
        printf 'FAIL /tmp/hevc_pps_scaling.hevc (unexpected MPP output frame count)\n'
        fail=1
      elif ! ssh "$BOARD" 'export RFI_HEVC_TRACE=1; cd ~/rk-mpp-rs &&
          sudo -E ./target/release/rk-mpp-rs hevcdec \
          /tmp/hevc_pps_scaling.hevc /tmp/hevc_pps_scaling_rust.yuv \
          >/tmp/hevc_pps_scaling_rust.log 2>&1'; then
        printf 'FAIL /tmp/hevc_pps_scaling.hevc (Rust decode failed)\n'
        fail=1
      elif ! ssh "$BOARD" 'test "$(grep -c "err=0x0 output=true status=00000107/" \
          /tmp/hevc_pps_scaling_rust.log)" -eq 10'; then
        printf 'FAIL /tmp/hevc_pps_scaling.hevc (hardware status or frame error)\n'
        fail=1
      elif ! ssh "$BOARD" 'cmp -s /tmp/hevc_pps_scaling_mpp_planar.yuv \
          /tmp/hevc_pps_scaling_rust.yuv'; then
        printf 'FAIL /tmp/hevc_pps_scaling.hevc (picture bytes differ)\n'
        fail=1
      else
        printf 'PASS /tmp/hevc_pps_scaling.hevc (explicit PPS list, 10 frames)\n'
      fi
    fi
  fi
else
  printf 'SKIP /tmp/hevc_pps_scaling.hevc (explicit PPS scaling-list fixture unavailable)\n'
fi

if ssh "$BOARD" 'command -v ffmpeg >/dev/null'; then
  if ! ssh "$BOARD" 'set -e
      ffmpeg -loglevel error -y -f lavfi \
        -i testsrc2=size=256x256:rate=30 -frames:v 30 \
        -pix_fmt yuv420p -f rawvideo /tmp/hevc_slices_src.yuv
      ffmpeg -loglevel error -y -f rawvideo -pixel_format yuv420p \
        -video_size 256x256 -framerate 30 -i /tmp/hevc_slices_src.yuv \
        -c:v libx265 \
        -x265-params "keyint=10:min-keyint=10:open-gop=1:bframes=3:b-pyramid=1:scenecut=0:repeat-headers=1:slices=4:log-level=warning" \
        -f hevc /tmp/hevc_slices_open.hevc \
        >/tmp/hevc_slices_encode.log 2>&1'; then
    printf 'FAIL /tmp/hevc_slices_open.hevc (could not generate multi-slice stream)\n'
    fail=1
  elif ! ssh "$BOARD" 'test -x /tmp/mpp-develop/build/test/mpi_dec_test &&
      test -f /tmp/mpp-develop/build/mpp/librockchip_mpp.so.0'; then
    printf 'SKIP /tmp/hevc_slices_open.hevc (MPP decoder binary or library missing)\n'
  else
    ssh "$BOARD" 'sudo rm -f /tmp/hevc_slices_mpp.yuv /tmp/hevc_slices_mpp_planar.yuv \
      /tmp/hevc_slices_rust.yuv /tmp/hevc_slices_rust.log'
    ssh "$BOARD" 'sudo env LD_LIBRARY_PATH=/tmp/mpp-develop/build/mpp \
      /tmp/mpp-develop/build/test/mpi_dec_test \
      -i /tmp/hevc_slices_open.hevc -t 16777220 \
      -o /tmp/hevc_slices_mpp.yuv -w 256 -h 256 -n 30 \
      >/tmp/hevc_slices_mpp.log 2>&1'
    if ! ssh "$BOARD" 'ffmpeg -loglevel error -y -f rawvideo -pixel_format nv12 \
        -video_size 256x256 -i /tmp/hevc_slices_mpp.yuv \
        -pix_fmt yuv420p -f rawvideo /tmp/hevc_slices_mpp_planar.yuv'; then
      printf 'FAIL /tmp/hevc_slices_open.hevc (could not convert MPP output)\n'
      fail=1
    else
      # MPP marks seven damaged pictures in the first GOP and emits the same
      # 23 clean pictures as Rust.
      expected_bytes=$((256 * 256 * 3 / 2 * 23))
      if ! ssh "$BOARD" "test \"\$(stat -c %s /tmp/hevc_slices_mpp_planar.yuv)\" -eq $expected_bytes"; then
        printf 'FAIL /tmp/hevc_slices_open.hevc (unexpected MPP clean-frame count)\n'
        fail=1
      elif ! ssh "$BOARD" 'export RFI_HEVC_TRACE=1; cd ~/rk-mpp-rs &&
          sudo -E ./target/release/rk-mpp-rs hevcdec \
          /tmp/hevc_slices_open.hevc /tmp/hevc_slices_rust.yuv \
          >/tmp/hevc_slices_rust.log 2>&1'; then
        printf 'FAIL /tmp/hevc_slices_open.hevc (Rust decode failed)\n'
        fail=1
      elif ! ssh "$BOARD" 'grep -Fqx "output pocs: [0, 1, 2, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29]" /tmp/hevc_slices_rust.log'; then
        printf 'FAIL /tmp/hevc_slices_open.hevc (unexpected clean POC set)\n'
        fail=1
      elif ! ssh "$BOARD" 'cmp -s /tmp/hevc_slices_mpp_planar.yuv /tmp/hevc_slices_rust.yuv'; then
        printf 'FAIL /tmp/hevc_slices_open.hevc (clean picture bytes differ)\n'
        fail=1
      else
        printf 'PASS /tmp/hevc_slices_open.hevc (multi-slice, 23 clean frames)\n'
      fi
    fi
  fi
else
  printf 'SKIP /tmp/hevc_slices_open.hevc (ffmpeg/libx265 unavailable)\n'
fi

if ssh "$BOARD" 'command -v ffmpeg >/dev/null'; then
  if ! ssh "$BOARD" 'ffmpeg -loglevel error -y -f lavfi \
      -i testsrc2=size=1280x720:rate=30 -frames:v 120 \
      -c:v libx265 \
      -x265-params "keyint=30:min-keyint=30:open-gop=0:bframes=3:b-pyramid=1:scenecut=0:repeat-headers=1:log-level=warning" \
      -f hevc /tmp/hevc_multigop_120.hevc \
      >/tmp/hevc_multigop_encode.log 2>&1'; then
    printf 'FAIL /tmp/hevc_multigop_120.hevc (could not generate stream)\n'
    fail=1
  elif ! ssh "$BOARD" 'test -x /tmp/mpp-develop/build/test/mpi_dec_test &&
      test -f /tmp/mpp-develop/build/mpp/librockchip_mpp.so.0'; then
    printf 'SKIP /tmp/hevc_multigop_120.hevc (MPP decoder binary or library missing)\n'
  else
    ssh "$BOARD" 'sudo rm -f /tmp/hevc_multigop_mpp.yuv \
      /tmp/hevc_multigop_mpp_planar.yuv /tmp/hevc_multigop_rust.yuv'
    ssh "$BOARD" 'sudo env LD_LIBRARY_PATH=/tmp/mpp-develop/build/mpp \
      /tmp/mpp-develop/build/test/mpi_dec_test \
      -i /tmp/hevc_multigop_120.hevc -t 16777220 \
      -o /tmp/hevc_multigop_mpp.yuv -w 1280 -h 720 -n 120 -v q \
      >/tmp/hevc_multigop_mpp.log 2>&1'
    if ! ssh "$BOARD" 'ffmpeg -loglevel error -y -f rawvideo -pixel_format nv12 \
        -video_size 1280x720 -i /tmp/hevc_multigop_mpp.yuv \
        -pix_fmt yuv420p -f rawvideo /tmp/hevc_multigop_mpp_planar.yuv'; then
      printf 'FAIL /tmp/hevc_multigop_120.hevc (could not convert MPP output)\n'
      fail=1
    else
      expected_bytes=$((1280 * 720 * 3 / 2 * 120))
      if ! ssh "$BOARD" "test \"\$(stat -c %s /tmp/hevc_multigop_mpp_planar.yuv)\" -eq $expected_bytes"; then
        printf 'FAIL /tmp/hevc_multigop_120.hevc (unexpected MPP output frame count)\n'
        fail=1
      elif ! ssh "$BOARD" 'cd ~/rk-mpp-rs &&
          sudo ./target/release/rk-mpp-rs hevcdec \
          /tmp/hevc_multigop_120.hevc /tmp/hevc_multigop_rust.yuv \
          >/tmp/hevc_multigop_rust.log 2>&1'; then
        printf 'FAIL /tmp/hevc_multigop_120.hevc (Rust decode failed)\n'
        fail=1
      elif ! ssh "$BOARD" 'grep -q "decoded 120 frames (120 output) in" /tmp/hevc_multigop_rust.log'; then
        printf 'FAIL /tmp/hevc_multigop_120.hevc (Rust dropped pictures)\n'
        fail=1
      elif ! ssh "$BOARD" 'cmp -s /tmp/hevc_multigop_mpp_planar.yuv /tmp/hevc_multigop_rust.yuv'; then
        printf 'FAIL /tmp/hevc_multigop_120.hevc (picture bytes differ)\n'
        fail=1
      else
        printf 'PASS /tmp/hevc_multigop_120.hevc (four IDR epochs, 120 frames)\n'
      fi
    fi
  fi
else
  printf 'SKIP /tmp/hevc_multigop_120.hevc (ffmpeg/libx265 unavailable)\n'
fi

if ssh "$BOARD" 'test -f /tmp/t720.hevc'; then
  ssh "$BOARD" 'test -f /tmp/t720_missing_au2.hevc || python3 - <<"PY"
from pathlib import Path

data = Path("/tmp/t720.hevc").read_bytes()
starts = []
i = 0
while i + 3 <= len(data):
    if data[i:i + 3] == b"\x00\x00\x01":
        starts.append((i, 3))
        i += 3
    elif i + 4 <= len(data) and data[i:i + 4] == b"\x00\x00\x00\x01":
        starts.append((i, 4))
        i += 4
    else:
        i += 1

groups = []
current = []
has_vcl = False
for index, (offset, code_len) in enumerate(starts):
    end = starts[index + 1][0] if index + 1 < len(starts) else len(data)
    nal = data[offset:end]
    header = offset + code_len
    unit_type = (data[header] >> 1) & 0x3f if header < len(data) else -1
    first_slice = (
        unit_type <= 31
        and header + 2 < len(data)
        and data[header + 2] & 0x80 != 0
    )
    if unit_type <= 31 and first_slice and has_vcl:
        groups.append(b"".join(current))
        current = []
        has_vcl = False
    current.append(nal)
    has_vcl = has_vcl or unit_type <= 31
if current and has_vcl:
    groups.append(b"".join(current))

assert len(groups) == 10, len(groups)
Path("/tmp/t720_missing_au2.hevc").write_bytes(b"".join(groups[:2] + groups[3:]))
PY'
fi
if ssh "$BOARD" 'test -f /tmp/t720_missing_au2.hevc &&
    test -x /tmp/mpp-develop/build/test/mpi_dec_test &&
    test -f /tmp/mpp-develop/build/mpp/librockchip_mpp.so.0'; then
  ssh "$BOARD" 'sudo rm -f /tmp/t720_missing_au2_mpp.yuv \
    /tmp/t720_missing_au2_mpp_2planar.yuv'
  ssh "$BOARD" 'sudo env LD_LIBRARY_PATH=/tmp/mpp-develop/build/mpp \
    /tmp/mpp-develop/build/test/mpi_dec_test \
    -i /tmp/t720_missing_au2.hevc -t 16777220 \
    -o /tmp/t720_missing_au2_mpp.yuv -v q \
    >/tmp/t720_missing_au2_mpp.log 2>&1'
  if ! ssh "$BOARD" 'ffmpeg -loglevel error -y -f rawvideo -pixel_format nv12 \
      -video_size 1280x720 -i /tmp/t720_missing_au2_mpp.yuv \
      -pix_fmt yuv420p -f rawvideo \
      /tmp/t720_missing_au2_mpp_2planar.yuv'; then
    printf 'FAIL /tmp/t720_missing_au2.hevc (could not convert MPP output)\n'
    fail=1
  else
    expected_bytes=$((1280 * 720 * 3 / 2 * 2))
    if ! ssh "$BOARD" "test \"\$(stat -c %s /tmp/t720_missing_au2_mpp_2planar.yuv)\" -eq $expected_bytes"; then
      printf 'FAIL /tmp/t720_missing_au2.hevc (unexpected MPP output frame count)\n'
      fail=1
    elif ! ssh "$BOARD" 'export RFI_HEVC_TRACE=1; cd ~/rk-mpp-rs &&
        sudo -E ./target/release/rk-mpp-rs hevcdec \
        /tmp/t720_missing_au2.hevc /tmp/t720_missing2_rust.yuv \
        >/tmp/t720_missing2_rust.log 2>&1'; then
      printf 'FAIL /tmp/t720_missing_au2.hevc (Rust trace failed)\n'
      fail=1
    elif ! ssh "$BOARD" 'grep -Fqx "output pocs: [0, 4]" /tmp/t720_missing2_rust.log'; then
      printf 'FAIL /tmp/t720_missing_au2.hevc (unexpected clean POC set)\n'
      fail=1
    elif ! ssh "$BOARD" 'cmp -s /tmp/t720_missing_au2_mpp_2planar.yuv /tmp/t720_missing2_rust.yuv'; then
      printf 'FAIL /tmp/t720_missing_au2.hevc (clean picture bytes differ)\n'
      fail=1
    else
      printf 'PASS /tmp/t720_missing_au2.hevc (POC 0, 4)\n'
    fi
  fi
else
  printf 'SKIP /tmp/t720_missing_au2.hevc (MPP decoder binary or library missing)\n'
fi

if ssh "$BOARD" 'command -v ffmpeg >/dev/null'; then
  if ! ssh "$BOARD" 'set -e
      ffmpeg -loglevel error -y -f lavfi \
        -i testsrc2=size=128x128:rate=30 -frames:v 30 \
        -pix_fmt yuv420p -f rawvideo /tmp/hevc_cra_src.yuv
      ffmpeg -loglevel error -y -f rawvideo -pixel_format yuv420p \
        -video_size 128x128 -framerate 30 -i /tmp/hevc_cra_src.yuv \
        -c:v libx265 \
        -x265-params "keyint=10:min-keyint=10:open-gop=1:bframes=3:b-pyramid=1:scenecut=0:repeat-headers=1:log-level=warning" \
        -f hevc /tmp/hevc_cra_open.hevc \
        >/tmp/hevc_cra_encode.log 2>&1
      python3 - <<"PY"
from pathlib import Path

data = Path("/tmp/hevc_cra_open.hevc").read_bytes()
starts = []
i = 0
while i + 3 <= len(data):
    if data[i:i + 3] == b"\x00\x00\x01":
        starts.append((i, 3))
        i += 3
    elif i + 4 <= len(data) and data[i:i + 4] == b"\x00\x00\x00\x01":
        starts.append((i, 4))
        i += 4
    else:
        i += 1
types = {
    (data[offset + code_len] >> 1) & 0x3f
    for offset, code_len in starts
    if offset + code_len < len(data)
}
assert {8, 9, 21}.issubset(types), sorted(types)
PY'; then
    printf 'FAIL /tmp/hevc_cra_open.hevc (could not generate CRA/RASL stream)\n'
    fail=1
  elif ! ssh "$BOARD" 'test -x /tmp/mpp-develop/build/test/mpi_dec_test &&
      test -f /tmp/mpp-develop/build/mpp/librockchip_mpp.so.0'; then
    printf 'SKIP /tmp/hevc_cra_open.hevc (MPP decoder binary or library missing)\n'
  else
    ssh "$BOARD" 'sudo rm -f /tmp/hevc_cra_mpp.yuv /tmp/hevc_cra_mpp_planar.yuv \
      /tmp/hevc_cra_rust.yuv'
    ssh "$BOARD" 'sudo env LD_LIBRARY_PATH=/tmp/mpp-develop/build/mpp \
      /tmp/mpp-develop/build/test/mpi_dec_test \
      -i /tmp/hevc_cra_open.hevc -t 16777220 \
      -o /tmp/hevc_cra_mpp.yuv -v q \
      >/tmp/hevc_cra_mpp.log 2>&1'
    if ! ssh "$BOARD" 'ffmpeg -loglevel error -y -f rawvideo -pixel_format nv12 \
        -video_size 128x128 -i /tmp/hevc_cra_mpp.yuv \
        -pix_fmt yuv420p -f rawvideo /tmp/hevc_cra_mpp_planar.yuv'; then
      printf 'FAIL /tmp/hevc_cra_open.hevc (could not convert MPP output)\n'
      fail=1
    else
      expected_bytes=$((128 * 128 * 3 / 2 * 30))
      if ! ssh "$BOARD" "test \"\$(stat -c %s /tmp/hevc_cra_mpp_planar.yuv)\" -eq $expected_bytes"; then
        printf 'FAIL /tmp/hevc_cra_open.hevc (unexpected MPP output frame count)\n'
        fail=1
      elif ! ssh "$BOARD" 'export RFI_HEVC_TRACE=1; cd ~/rk-mpp-rs &&
          sudo -E ./target/release/rk-mpp-rs hevcdec \
          /tmp/hevc_cra_open.hevc /tmp/hevc_cra_rust.yuv \
          >/tmp/hevc_cra_rust.log 2>&1'; then
        printf 'FAIL /tmp/hevc_cra_open.hevc (Rust trace failed)\n'
        fail=1
      elif ! ssh "$BOARD" 'grep -Fqx "output pocs: [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29]" /tmp/hevc_cra_rust.log'; then
        printf 'FAIL /tmp/hevc_cra_open.hevc (unexpected display POC order)\n'
        fail=1
      elif ! ssh "$BOARD" 'cmp -s /tmp/hevc_cra_mpp_planar.yuv /tmp/hevc_cra_rust.yuv'; then
        printf 'FAIL /tmp/hevc_cra_open.hevc (picture bytes differ)\n'
        fail=1
      else
        printf 'PASS /tmp/hevc_cra_open.hevc (CRA/open-GOP, 30 frames)\n'
      fi
    fi
  fi
else
  printf 'SKIP /tmp/hevc_cra_open.hevc (ffmpeg/libx265 unavailable)\n'
fi

if ssh "$BOARD" 'command -v ffmpeg >/dev/null &&
    test -x /tmp/mpp-develop/build/test/mpi_enc_test &&
    test -f /tmp/mpp-develop/build/mpp/librockchip_mpp.so.0 &&
    test -f /tmp/mpp-develop/test/cfg/enc/ref_cfg_gop3_tsvc4.json &&
    test -f /tmp/mpp-develop/test/cfg/enc/frm_ltr_mark.json &&
    test -f /tmp/mpp-develop/test/cfg/enc/frm_ltr_use.json'; then
  if ! ssh "$BOARD" 'set -e
      sudo rm -f /tmp/hevc_ltr_320.hevc /tmp/hevc_ltr_encode.log \
        /tmp/hevc_ltr_dpb.log
      ffmpeg -loglevel error -y -f lavfi \
        -i testsrc2=size=320x320:rate=30 -frames:v 30 \
        -pix_fmt nv12 -f rawvideo /tmp/hevc_ltr_src.yuv
      sudo env LD_LIBRARY_PATH=/tmp/mpp-develop/build/mpp \
        /tmp/mpp-develop/build/test/mpi_enc_test \
        -i /tmp/hevc_ltr_src.yuv -o /tmp/hevc_ltr_320.hevc \
        -w 320 -h 320 -f 0 -t 16777220 -n 30 -v q \
        -ref_cfg /tmp/mpp-develop/test/cfg/enc/ref_cfg_gop3_tsvc4.json \
        -frm_cfg /tmp/mpp-develop/test/cfg/enc/frm_ltr_mark.json \
        -frm_cfg /tmp/mpp-develop/test/cfg/enc/frm_ltr_use.json \
        >/tmp/hevc_ltr_encode.log 2>&1
      cd ~/rk-mpp-rs
      ./target/release/rk-mpp-rs hevcdpb /tmp/hevc_ltr_320.hevc \
        >/tmp/hevc_ltr_dpb.log' ; then
    printf 'FAIL /tmp/hevc_ltr_320.hevc (could not generate MPP LTR stream)\n'
    fail=1
  elif ! ssh "$BOARD" 'grep -q "lt_present=true" /tmp/hevc_ltr_dpb.log &&
      grep -q "LongTermRefPic { poc_lsb: 4, used: true" /tmp/hevc_ltr_dpb.log'; then
    printf 'FAIL /tmp/hevc_ltr_320.hevc (stream does not exercise LTR)\n'
    fail=1
  elif ! ssh "$BOARD" 'test -x /tmp/mpp-develop/build/test/mpi_dec_test &&
      test -f /tmp/mpp-develop/build/mpp/librockchip_mpp.so.0'; then
    printf 'SKIP /tmp/hevc_ltr_320.hevc (MPP decoder binary or library missing)\n'
  else
    ssh "$BOARD" 'sudo rm -f /tmp/hevc_ltr_mpp.yuv \
      /tmp/hevc_ltr_mpp_planar.yuv /tmp/hevc_ltr_rust.yuv'
    ssh "$BOARD" 'sudo env LD_LIBRARY_PATH=/tmp/mpp-develop/build/mpp \
      /tmp/mpp-develop/build/test/mpi_dec_test \
      -i /tmp/hevc_ltr_320.hevc -t 16777220 \
      -o /tmp/hevc_ltr_mpp.yuv -w 320 -h 320 -n 30 -v q \
      >/tmp/hevc_ltr_mpp.log 2>&1'
    if ! ssh "$BOARD" 'ffmpeg -loglevel error -y -f rawvideo -pixel_format nv12 \
        -video_size 320x320 -i /tmp/hevc_ltr_mpp.yuv \
        -pix_fmt yuv420p -f rawvideo /tmp/hevc_ltr_mpp_planar.yuv'; then
      printf 'FAIL /tmp/hevc_ltr_320.hevc (could not convert MPP output)\n'
      fail=1
    else
      expected_bytes=$((320 * 320 * 3 / 2 * 30))
      if ! ssh "$BOARD" "test \"\$(stat -c %s /tmp/hevc_ltr_mpp_planar.yuv)\" -eq $expected_bytes"; then
        printf 'FAIL /tmp/hevc_ltr_320.hevc (unexpected MPP output frame count)\n'
        fail=1
      elif ! ssh "$BOARD" 'export RFI_HEVC_TRACE=1; cd ~/rk-mpp-rs &&
          sudo -E ./target/release/rk-mpp-rs hevcdec \
          /tmp/hevc_ltr_320.hevc /tmp/hevc_ltr_rust.yuv \
          >/tmp/hevc_ltr_rust.log 2>&1'; then
        printf 'FAIL /tmp/hevc_ltr_320.hevc (Rust decode failed)\n'
        fail=1
      elif ! ssh "$BOARD" 'grep -q "decoded 30 frames (30 output) in" /tmp/hevc_ltr_rust.log &&
          test "$(grep -c "err=0x0 output=true status=00000107/" \
          /tmp/hevc_ltr_rust.log)" -eq 30'; then
        printf 'FAIL /tmp/hevc_ltr_320.hevc (hardware status or frame count)\n'
        fail=1
      elif ! ssh "$BOARD" 'cmp -s /tmp/hevc_ltr_mpp_planar.yuv /tmp/hevc_ltr_rust.yuv'; then
        printf 'FAIL /tmp/hevc_ltr_320.hevc (picture bytes differ)\n'
        fail=1
      else
        printf 'PASS /tmp/hevc_ltr_320.hevc (MPP LTR mark/use, 30 frames)\n'
      fi
    fi
  fi
else
  printf 'SKIP /tmp/hevc_ltr_320.hevc (MPP LTR encoder/config unavailable)\n'
fi

dslice_fixture=/tmp/DSLICE_A_HHI_5.bit
dslice_url=https://fate-suite.ffmpeg.org/hevc-conformance/DSLICE_A_HHI_5.bit
if [[ ! -s "$dslice_fixture" ]]; then
  curl -L --fail --silent --show-error "$dslice_url" -o "$dslice_fixture" ||
    printf 'NOTE could not download dependent-slice conformance stream\n'
fi
if [[ ! -s "$dslice_fixture" ]]; then
  printf 'SKIP dependent slices (conformance stream unavailable)\n'
elif ! ssh "$BOARD" 'test -x /tmp/mpp-develop/build/test/mpi_dec_test &&
    test -f /tmp/mpp-develop/build/mpp/librockchip_mpp.so.0 &&
    command -v ffmpeg >/dev/null'; then
  printf 'SKIP dependent slices (MPP decoder or ffmpeg unavailable)\n'
else
  scp -q "$dslice_fixture" "$BOARD":/tmp/DSLICE_A_HHI_5.bit
  ssh "$BOARD" 'sudo rm -f /tmp/dslice_mpp.yuv /tmp/dslice_mpp_planar.yuv \
    /tmp/dslice_rust.yuv /tmp/dslice_rust.log /tmp/dslice_mpp.log'
  ssh "$BOARD" 'sudo env LD_LIBRARY_PATH=/tmp/mpp-develop/build/mpp \
    /tmp/mpp-develop/build/test/mpi_dec_test \
    -i /tmp/DSLICE_A_HHI_5.bit -t 16777220 \
    -o /tmp/dslice_mpp.yuv -w 1920 -h 1080 -n 50 -v q \
    >/tmp/dslice_mpp.log 2>&1'
  if ! ssh "$BOARD" 'ffmpeg -loglevel error -y -f rawvideo -pixel_format nv12 \
      -video_size 1920x1080 -i /tmp/dslice_mpp.yuv \
      -pix_fmt yuv420p -f rawvideo /tmp/dslice_mpp_planar.yuv'; then
    printf 'FAIL dependent slices (could not convert MPP output)\n'
    fail=1
  elif ! ssh "$BOARD" 'export RFI_HEVC_TRACE=1; cd ~/rk-mpp-rs &&
      sudo -E ./target/release/rk-mpp-rs hevcdec \
      /tmp/DSLICE_A_HHI_5.bit /tmp/dslice_rust.yuv \
      >/tmp/dslice_rust.log 2>&1'; then
    printf 'FAIL dependent slices (Rust decode failed)\n'
    fail=1
  elif ! ssh "$BOARD" 'grep -q "decoded 50 frames (50 output) in" /tmp/dslice_rust.log &&
      test "$(grep -c "err=0x0 output=true status=00000107/" \
      /tmp/dslice_rust.log)" -eq 50 &&
      ! grep -q "err=0x10 output=false" /tmp/dslice_rust.log'; then
    printf 'FAIL dependent slices (unexpected frame error set)\n'
    fail=1
  elif ! ssh "$BOARD" 'cmp -s /tmp/dslice_mpp_planar.yuv /tmp/dslice_rust.yuv'; then
    printf 'FAIL dependent slices (picture bytes differ)\n'
    fail=1
  else
    printf 'PASS dependent slices (JCT-VC DSLICE, 850 VCL, 50 frames)\n'
  fi
fi

exit "$fail"
