#!/usr/bin/env bash
# Compare the first Rust AVS I picture with vendor C on RK3588 client 12.
set -euo pipefail

BOARD="${BOARD:?set BOARD to the RK3588 SSH alias}"
remote_repo="${REMOTE_REPO:-\$HOME/rk-mpp-rs}"
mpp_ref="${MPP_REF:-\$HOME/rk-mpp-oracles/mpp-develop}"
fixture="${FIXTURE:-/tmp/avs_stream01.avs}"
fixture_sha=d296affc27c5977565a18d85897be82a3b1c77c9165b8928e6e5eaad698a736e

if [ ! -s "$fixture" ]; then
  curl -L --fail --silent \
    https://samples.ffmpeg.org/AVS/issue767/stream01.avs \
    -o "$fixture"
fi
test "$(sha256sum "$fixture" | cut -d' ' -f1)" = "$fixture_sha"
scp -q "$fixture" "$BOARD":/tmp/avs_stream01.avs

COPYFILE_DISABLE=1 tar --no-xattrs \
  --exclude .git --exclude target --exclude .worktrees --exclude refs \
  -cf - . | ssh "$BOARD" "tar -xf - -C \"$remote_repo\""
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd "$HOME/rk-mpp-rs" && \
  cargo build --release --bin rk-mpp-rs'

ssh "$BOARD" "sudo rm -f /tmp/avs_c.nv12 /tmp/avs_c.yuv /tmp/avs_rust.yuv"
ssh "$BOARD" "sudo env enable_deinterlace=0 LD_LIBRARY_PATH=\"$mpp_ref/build/mpp\" timeout 30 \
  \"$mpp_ref/build/test/mpi_dec_test\" -i /tmp/avs_stream01.avs \
  -o /tmp/avs_c.nv12 -t 16777221 -n 1 \
  </dev/null >/tmp/avs_c.log 2>&1"
ssh "$BOARD" "cd \"$remote_repo\" && sudo timeout 30 \
  ./target/release/rk-mpp-rs avsdec /tmp/avs_stream01.avs /tmp/avs_rust.yuv 1 \
  >/tmp/avs_rust.log 2>&1"

ssh "$BOARD" "ffmpeg -loglevel error -y -f rawvideo -pixel_format nv12 \
  -video_size 352x288 -i /tmp/avs_c.nv12 -pix_fmt yuv420p -f rawvideo \
  /tmp/avs_c.yuv"
ssh "$BOARD" "test \"\$(stat -c %s /tmp/avs_rust.yuv)\" -eq 152064"
ssh "$BOARD" "cmp -s /tmp/avs_c.yuv /tmp/avs_rust.yuv"
printf 'PASS AVS+ Rust I picture is byte-identical to vendor C (352x288 I420)\n'
