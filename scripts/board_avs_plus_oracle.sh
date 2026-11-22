#!/usr/bin/env bash
# Establish the RK3588 AVS+ vendor-C oracle before implementing the Rust path.
set -euo pipefail

BOARD="${BOARD:?set BOARD to the RK3588 SSH alias}"
remote_repo="${REMOTE_REPO:-\$HOME/rk-mpp-rs}"
mpp_ref="${MPP_REF:-\$HOME/rk-mpp-oracles/mpp-develop}"
fixture="${FIXTURE:-/tmp/avs_stream01.avs}"
fixture_sha=d296affc27c5977565a18d85897be82a3b1c77c9165b8928e6e5eaad698a736e
output_sha=9b9a6dc9f934a5a5617dbbd68e13dda7675fe985f578e40ce209fbed5397088b

if [ ! -s "$fixture" ]; then
  curl -L --fail --silent \
    https://samples.ffmpeg.org/AVS/issue767/stream01.avs \
    -o "$fixture"
fi
test "$(sha256sum "$fixture" | cut -d' ' -f1)" = "$fixture_sha"
scp -q "$fixture" "$BOARD":/tmp/avs_stream01.avs

ssh "$BOARD" "grep -q 'DEVICE\\[12\\]:AVSPLUS_DECHW_ID' \
  /proc/mpp_service/supports-device"
ssh "$BOARD" "sudo env LD_LIBRARY_PATH=\"$mpp_ref/build/mpp\" timeout 30 \
  \"$mpp_ref/build/test/mpi_dec_test\" -i /tmp/avs_stream01.avs \
  -o /tmp/avs_plus_oracle.yuv -t 16777221 -n 1 \
  </dev/null >/tmp/avs_plus_oracle.log 2>&1"

ssh "$BOARD" "test \"\$(stat -c %s /tmp/avs_plus_oracle.yuv)\" -eq 152064"
ssh "$BOARD" "test \"\$(sha256sum /tmp/avs_plus_oracle.yuv | cut -d' ' -f1)\" \
  = '$output_sha'"
printf 'PASS AVS+ RK3588 vendor oracle (352x288 NV12, %s)\n' "$output_sha"
