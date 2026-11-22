#!/usr/bin/env bash
# Compare the first Rust AVS I picture with vendor C on RK3588 client 12.
set -euo pipefail

BOARD="${BOARD:?set BOARD to the RK3588 SSH alias}"
remote_repo="${REMOTE_REPO:-\$HOME/rk-mpp-rs}"
mpp_ref="${MPP_REF:-\$HOME/rk-mpp-oracles/mpp-develop}"
fixture="${FIXTURE:-/tmp/avs_stream01.avs}"
fixture2="${FIXTURE2:-/tmp/avs_stream03.avs}"
fixture_sha=d296affc27c5977565a18d85897be82a3b1c77c9165b8928e6e5eaad698a736e
fixture2_sha=fa2055cfad0b1d554f2bb87ed4e02254c52b481a9b5b07d49c25a09533c83565
profile48="${PROFILE48:-/tmp/avs_plus_profile48.avs}"
profile48_sha=50d88acc3e37efa6a2112a01aebee540a317295986036e21104f98621ac8e7cc
profile48_weighted="${PROFILE48_WEIGHTED:-/tmp/avs_plus_profile48_weighted.avs}"
profile48_weighted_sha=5142083a95fb5235de025785c3c8dece67c93832e0cd70392a6fa37e373cdf99

if [ ! -s "$fixture" ]; then
  curl -L --fail --silent \
    https://samples.ffmpeg.org/AVS/issue767/stream01.avs \
    -o "$fixture"
fi
if [ ! -s "$fixture2" ]; then
  curl -L --fail --silent \
    https://samples.ffmpeg.org/AVS/issue767/stream03.avs \
    -o "$fixture2"
fi
test "$(sha256sum "$fixture" | cut -d' ' -f1)" = "$fixture_sha"
test "$(sha256sum "$fixture2" | cut -d' ' -f1)" = "$fixture2_sha"
python3 "$(dirname "$0")/gen_avs_plus_profile.py" \
  "$fixture" "$profile48" first-sequence >/dev/null
python3 "$(dirname "$0")/gen_avs_plus_profile.py" \
  "$fixture" "$profile48_weighted" weighted-base1-first >/dev/null
test "$(sha256sum "$profile48" | cut -d' ' -f1)" = "$profile48_sha"
test "$(sha256sum "$profile48_weighted" | cut -d' ' -f1)" = "$profile48_weighted_sha"
scp -q "$fixture" "$BOARD":/tmp/avs_stream01.avs
scp -q "$fixture2" "$BOARD":/tmp/avs_stream03.avs
scp -q "$profile48" "$BOARD":/tmp/avs_plus_profile48.avs
scp -q "$profile48_weighted" "$BOARD":/tmp/avs_plus_profile48_weighted.avs

COPYFILE_DISABLE=1 tar --no-xattrs \
  --exclude .git --exclude target --exclude .worktrees --exclude refs \
  -cf - . | ssh "$BOARD" "tar -xf - -C \"$remote_repo\""
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd "$HOME/rk-mpp-rs" && \
  cargo build --release --lib --bin rk-mpp-rs --example sdk_decode && \
  gcc -std=c11 -Wall -Wextra -Werror -Iinclude tests/c_api_decoder.c \
    -Ltarget/release -lrk_mpp_rs -Wl,-rpath,"$PWD/target/release" \
    -o /tmp/rk_mpp_c_api_decoder_avs && \
  g++ -std=c++17 -Wall -Wextra -Werror -Iinclude \
    tests/c_api_decoder_smoke.cpp -Ltarget/release -lrk_mpp_rs \
    -Wl,-rpath,"$PWD/target/release" -o /tmp/rk_mpp_c_api_smoke_avs && \
  /tmp/rk_mpp_c_api_smoke_avs'

ssh "$BOARD" "sudo rm -f /tmp/avs_c.nv12 /tmp/avs_c.yuv /tmp/avs_rust.yuv /tmp/avs_sdk.yuv"
ssh "$BOARD" "sudo env enable_deinterlace=0 LD_LIBRARY_PATH=\"$mpp_ref/build/mpp\" timeout 30 \
  \"$mpp_ref/build/test/mpi_dec_test\" -i /tmp/avs_stream01.avs \
  -o /tmp/avs_c.nv12 -t 16777221 -n 7 \
  </dev/null >/tmp/avs_c.log 2>&1"
ssh "$BOARD" "cd \"$remote_repo\" && sudo timeout 30 \
  ./target/release/rk-mpp-rs avsdec /tmp/avs_stream01.avs /tmp/avs_rust.yuv 7 \
  >/tmp/avs_rust.log 2>&1"
ssh "$BOARD" "cd \"$remote_repo\" && sudo timeout 30 \
  ./target/release/examples/sdk_decode avs /tmp/avs_stream01.avs /tmp/avs_sdk.yuv \
  >/tmp/avs_sdk.log 2>&1"

ssh "$BOARD" "ffmpeg -loglevel error -y -f rawvideo -pixel_format nv12 \
  -video_size 352x288 -i /tmp/avs_c.nv12 -pix_fmt yuv420p -f rawvideo \
  /tmp/avs_c.yuv"
ssh "$BOARD" "test \"\$(stat -c %s /tmp/avs_rust.yuv)\" -eq 1064448"
ssh "$BOARD" "cmp -s /tmp/avs_c.yuv /tmp/avs_rust.yuv"
ssh "$BOARD" "cmp -s /tmp/avs_rust.yuv /tmp/avs_sdk.yuv"
printf 'PASS AVS+ Rust CLI/SDK 7-frame cycle is byte-identical to vendor C (352x288 I420)\n'

ssh "$BOARD" "sudo rm -f /tmp/avs_vproc_c.nv12 /tmp/avs_vproc_c.yuv \
  /tmp/avs_vproc_r.nv12 /tmp/avs_vproc_sdk.yuv /tmp/avs_vproc_capi.yuv"
ssh "$BOARD" "sudo env LD_LIBRARY_PATH=\"$mpp_ref/build/mpp\" timeout 30 \
  \"$mpp_ref/build/test/mpi_dec_test\" -i /tmp/avs_stream01.avs \
  -o /tmp/avs_vproc_c.nv12 -t 16777221 -n 9 \
  </dev/null >/tmp/avs_vproc_c.log 2>&1"
ssh "$BOARD" "cd \"$remote_repo\" && sudo timeout 30 \
  ./target/release/rk-mpp-rs avsdec /tmp/avs_stream01.avs /tmp/avs_vproc_r.nv12 9 vproc \
  >/tmp/avs_vproc_r.log 2>&1"
ssh "$BOARD" "test \"\$(stat -c %s /tmp/avs_vproc_r.nv12)\" -eq 1368576"
ssh "$BOARD" "cmp -s /tmp/avs_vproc_c.nv12 /tmp/avs_vproc_r.nv12"
ssh "$BOARD" "ffmpeg -loglevel error -y -f rawvideo -pixel_format nv12 \
  -video_size 352x288 -i /tmp/avs_vproc_c.nv12 -pix_fmt yuv420p -f rawvideo \
  /tmp/avs_vproc_c.yuv"
ssh "$BOARD" "cd \"$remote_repo\" && sudo timeout 30 \
  ./target/release/examples/sdk_decode avs-vproc /tmp/avs_stream01.avs \
  /tmp/avs_vproc_sdk.yuv >/tmp/avs_vproc_sdk.log 2>&1"
ssh "$BOARD" "test \"\$(stat -c %s /tmp/avs_vproc_sdk.yuv)\" -eq 1368576"
ssh "$BOARD" "cmp -s /tmp/avs_vproc_c.yuv /tmp/avs_vproc_sdk.yuv"
ssh "$BOARD" "VPROC_OUTPUT=1 /tmp/rk_mpp_c_api_decoder_avs avs i420 65536 \
  /tmp/avs_vproc_capi.yuv /tmp/avs_stream01.avs \
  >/tmp/avs_vproc_capi.log 2>&1"
ssh "$BOARD" "test \"\$(stat -c %s /tmp/avs_vproc_capi.yuv)\" -eq 1368576"
ssh "$BOARD" "cmp -s /tmp/avs_vproc_c.yuv /tmp/avs_vproc_capi.yuv"
printf 'PASS AVS+ Rust 9-frame default IEP2 vproc cycle is byte-identical to vendor C\n'
printf 'PASS AVS+ public SDK vproc flag is byte-identical to vendor C\n'
printf 'PASS AVS+ public C ABI vproc flag is byte-identical to vendor C\n'

ssh "$BOARD" "sudo rm -f /tmp/avs03_c.nv12 /tmp/avs03_c.yuv /tmp/avs03_r.yuv"
ssh "$BOARD" "sudo env enable_deinterlace=0 LD_LIBRARY_PATH=\"$mpp_ref/build/mpp\" timeout 30 \
  \"$mpp_ref/build/test/mpi_dec_test\" -i /tmp/avs_stream03.avs \
  -o /tmp/avs03_c.nv12 -t 16777221 -n 6 \
  </dev/null >/tmp/avs03_c.log 2>&1"
ssh "$BOARD" "cd \"$remote_repo\" && sudo timeout 30 \
  ./target/release/rk-mpp-rs avsdec /tmp/avs_stream03.avs /tmp/avs03_r.yuv 6 \
  >/tmp/avs03_r.log 2>&1"
ssh "$BOARD" "ffmpeg -loglevel error -y -f rawvideo -pixel_format nv12 \
  -video_size 352x288 -i /tmp/avs03_c.nv12 -pix_fmt yuv420p -f rawvideo \
  /tmp/avs03_c.yuv"
ssh "$BOARD" "test \"\$(stat -c %s /tmp/avs03_r.yuv)\" -eq 912384"
ssh "$BOARD" "cmp -s /tmp/avs03_c.yuv /tmp/avs03_r.yuv"
printf 'PASS AVS+ Rust stream03 6-frame cycle is byte-identical to vendor C\n'

ssh "$BOARD" "sudo rm -f /tmp/avs03_vproc_c.nv12 /tmp/avs03_vproc_c.yuv \
  /tmp/avs03_vproc_r.nv12 /tmp/avs03_vproc_sdk.yuv"
ssh "$BOARD" "sudo env LD_LIBRARY_PATH=\"$mpp_ref/build/mpp\" timeout 30 \
  \"$mpp_ref/build/test/mpi_dec_test\" -i /tmp/avs_stream03.avs \
  -o /tmp/avs03_vproc_c.nv12 -t 16777221 -n 7 \
  </dev/null >/tmp/avs03_vproc_c.log 2>&1"
ssh "$BOARD" "cd \"$remote_repo\" && sudo timeout 30 \
  ./target/release/rk-mpp-rs avsdec /tmp/avs_stream03.avs /tmp/avs03_vproc_r.nv12 7 vproc \
  >/tmp/avs03_vproc_r.log 2>&1"
ssh "$BOARD" "test \"\$(stat -c %s /tmp/avs03_vproc_r.nv12)\" -eq 1064448"
ssh "$BOARD" "cmp -s /tmp/avs03_vproc_c.nv12 /tmp/avs03_vproc_r.nv12"
ssh "$BOARD" "ffmpeg -loglevel error -y -f rawvideo -pixel_format nv12 \
  -video_size 352x288 -i /tmp/avs03_vproc_c.nv12 -pix_fmt yuv420p -f rawvideo \
  /tmp/avs03_vproc_c.yuv"
ssh "$BOARD" "cd \"$remote_repo\" && sudo timeout 30 \
  ./target/release/examples/sdk_decode avs-vproc /tmp/avs_stream03.avs \
  /tmp/avs03_vproc_sdk.yuv >/tmp/avs03_vproc_sdk.log 2>&1"
ssh "$BOARD" "test \"\$(stat -c %s /tmp/avs03_vproc_sdk.yuv)\" -eq 1064448"
ssh "$BOARD" "cmp -s /tmp/avs03_vproc_c.yuv /tmp/avs03_vproc_sdk.yuv"
printf 'PASS AVS+ Rust stream03 7-frame default IEP2 vproc cycle is byte-identical to vendor C\n'
printf 'PASS AVS+ public SDK stream03 vproc cycle is byte-identical to vendor C\n'

ssh "$BOARD" "sudo rm -f /tmp/avs_i_c.nv12 /tmp/avs_i_c.yuv \
  /tmp/avs_i_r.yuv /tmp/avs_i_cv.nv12 /tmp/avs_i_rv.nv12"
ssh "$BOARD" "cd \"$remote_repo\" && sudo env enable_deinterlace=0 \
  LD_LIBRARY_PATH=\"$mpp_ref/build/mpp\" timeout 30 \
  \"$mpp_ref/build/test/mpi_dec_test\" -i \
  tests/data/avs_xavs_interlaced_352x288_8.avs -o /tmp/avs_i_c.nv12 \
  -t 16777221 -n 8 </dev/null >/tmp/avs_i_c.log 2>&1"
ssh "$BOARD" "cd \"$remote_repo\" && sudo timeout 30 \
  ./target/release/rk-mpp-rs avsdec \
  tests/data/avs_xavs_interlaced_352x288_8.avs /tmp/avs_i_r.yuv 8 i420 \
  >/tmp/avs_i_r.log 2>&1"
ssh "$BOARD" "ffmpeg -loglevel error -y -f rawvideo -pixel_format nv12 \
  -video_size 352x288 -i /tmp/avs_i_c.nv12 -pix_fmt yuv420p -f rawvideo \
  /tmp/avs_i_c.yuv"
ssh "$BOARD" "test \"\$(stat -c %s /tmp/avs_i_r.yuv)\" -eq 1216512"
ssh "$BOARD" "cmp -s /tmp/avs_i_c.yuv /tmp/avs_i_r.yuv"
ssh "$BOARD" "cd \"$remote_repo\" && sudo env \
  LD_LIBRARY_PATH=\"$mpp_ref/build/mpp\" timeout 30 \
  \"$mpp_ref/build/test/mpi_dec_test\" -i \
  tests/data/avs_xavs_interlaced_352x288_8.avs -o /tmp/avs_i_cv.nv12 \
  -t 16777221 -n 11 </dev/null >/tmp/avs_i_cv.log 2>&1"
ssh "$BOARD" "cd \"$remote_repo\" && sudo timeout 30 \
  ./target/release/rk-mpp-rs avsdec \
  tests/data/avs_xavs_interlaced_352x288_8.avs /tmp/avs_i_rv.nv12 11 vproc \
  >/tmp/avs_i_rv.log 2>&1"
ssh "$BOARD" "test \"\$(stat -c %s /tmp/avs_i_rv.nv12)\" -eq 1672704"
ssh "$BOARD" "cmp -s /tmp/avs_i_cv.nv12 /tmp/avs_i_rv.nv12"
printf 'PASS AVS+ interlaced-frame raw/vproc cycles are byte-identical to vendor C\n'

ssh "$BOARD" "sudo rm -f /tmp/avs48_c.nv12 /tmp/avs48_c.yuv /tmp/avs48_r.nv12 /tmp/avs48_sdk.yuv"
ssh "$BOARD" "sudo env enable_deinterlace=0 LD_LIBRARY_PATH=\"$mpp_ref/build/mpp\" timeout 30 \
  \"$mpp_ref/build/test/mpi_dec_test\" -i /tmp/avs_plus_profile48.avs \
  -o /tmp/avs48_c.nv12 -t 16777221 -n 4 \
  </dev/null >/tmp/avs48_c.log 2>&1"
ssh "$BOARD" "cd \"$remote_repo\" && sudo timeout 30 \
  ./target/release/rk-mpp-rs avsdec /tmp/avs_plus_profile48.avs /tmp/avs48_r.nv12 4 nv12 \
  >/tmp/avs48_r.log 2>&1"
ssh "$BOARD" "cd \"$remote_repo\" && sudo timeout 30 \
  ./target/release/examples/sdk_decode avs /tmp/avs_plus_profile48.avs /tmp/avs48_sdk.yuv \
  >/tmp/avs48_sdk.log 2>&1"
ssh "$BOARD" "ffmpeg -loglevel error -y -f rawvideo -pixel_format nv12 \
  -video_size 352x288 -i /tmp/avs48_c.nv12 -pix_fmt yuv420p -f rawvideo \
  /tmp/avs48_c.yuv"
ssh "$BOARD" "test \"\$(stat -c %s /tmp/avs48_r.nv12)\" -eq 608256"
ssh "$BOARD" "cmp -s /tmp/avs48_c.nv12 /tmp/avs48_r.nv12"
ssh "$BOARD" "cmp -s /tmp/avs48_c.yuv /tmp/avs48_sdk.yuv"
printf 'PASS AVS+ profile 0x48 raw CLI/SDK cycle is byte-identical to vendor C\n'

ssh "$BOARD" "sudo rm -f /tmp/avs48w_c.nv12 /tmp/avs48w_r.nv12"
ssh "$BOARD" "sudo env enable_deinterlace=0 LD_LIBRARY_PATH=\"$mpp_ref/build/mpp\" timeout 30 \
  \"$mpp_ref/build/test/mpi_dec_test\" -i /tmp/avs_plus_profile48_weighted.avs \
  -o /tmp/avs48w_c.nv12 -t 16777221 -n 4 \
  </dev/null >/tmp/avs48w_c.log 2>&1"
ssh "$BOARD" "cd \"$remote_repo\" && sudo timeout 30 \
  ./target/release/rk-mpp-rs avsdec /tmp/avs_plus_profile48_weighted.avs \
  /tmp/avs48w_r.nv12 4 nv12 >/tmp/avs48w_r.log 2>&1"
ssh "$BOARD" "test \"\$(stat -c %s /tmp/avs48w_r.nv12)\" -eq 608256"
ssh "$BOARD" "cmp -s /tmp/avs48w_c.nv12 /tmp/avs48w_r.nv12"
printf 'PASS AVS+ profile 0x48 weighted-quant cycle is byte-identical to vendor C\n'
