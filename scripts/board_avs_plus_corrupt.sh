#!/usr/bin/env bash
# Evidence matrix for AVS+ truncated and damaged public streams.
set -uo pipefail

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

python3 - "$fixture" <<'PY'
import sys
from pathlib import Path

source = Path(sys.argv[1]).read_bytes()
variants = {
    "trunc_100": source[:100],
    "trunc_500": source[:500],
    "trunc_3000": source[:3000],
    "trunc_7000": source[:7000],
    "trunc_10000": source[:10000],
    "bad_profile": source[:5] + b"\x21" + source[6:],
}
bad_header = bytearray(source)
bad_header[0x7D] ^= 0x40
variants["bad_iheader"] = bad_header
bad_slice = bytearray(source)
bad_slice[0x200] ^= 0xFF
variants["bad_slice"] = bad_slice
for name, data in variants.items():
    Path(f"/tmp/avs_dmg_{name}.avs").write_bytes(data)
PY
scp -q /tmp/avs_dmg_*.avs "$BOARD":/tmp/

COPYFILE_DISABLE=1 tar --no-xattrs \
  --exclude .git --exclude target --exclude .worktrees --exclude refs \
  -cf - . | ssh "$BOARD" "tar -xf - -C \"$remote_repo\""
ssh "$BOARD" 'export PATH="$HOME/.cargo/bin:$PATH"; cd "$HOME/rk-mpp-rs" && \
  cargo build --release --bin rk-mpp-rs'

for name in trunc_100 trunc_500 trunc_3000 trunc_7000 trunc_10000 \
  bad_profile bad_iheader bad_slice; do
  stream="/tmp/avs_dmg_${name}.avs"
  ssh "$BOARD" "sudo rm -f /tmp/avs_dmg_c.nv12 /tmp/avs_dmg_r.nv12"
  ssh "$BOARD" "sudo env enable_deinterlace=0 LD_LIBRARY_PATH=\"$mpp_ref/build/mpp\" \
    timeout 10 \"$mpp_ref/build/test/mpi_dec_test\" -i $stream -o /tmp/avs_dmg_c.nv12 \
    -t 16777221 -n 1 </dev/null >/tmp/avs_dmg_c.log 2>&1" && c_rc=0 || c_rc=$?
  ssh "$BOARD" "cd \"$remote_repo\" && sudo timeout 10 \
    ./target/release/rk-mpp-rs avsdec $stream /tmp/avs_dmg_r.nv12 1 nv12 \
    >/tmp/avs_dmg_r.log 2>&1" && r_rc=0 || r_rc=$?
  c_size=$(ssh "$BOARD" "stat -c %s /tmp/avs_dmg_c.nv12 2>/dev/null || echo 0")
  r_size=$(ssh "$BOARD" "stat -c %s /tmp/avs_dmg_r.nv12 2>/dev/null || echo 0")
  r_msg=$(ssh "$BOARD" "head -1 /tmp/avs_dmg_r.log" | tr -d '\r')
  if [ "$c_rc" = 124 ]; then
    printf 'C-TIMEOUT %-12s Rust=%s/%s | %s\n' "$name" "$r_rc" "$r_size" "$r_msg"
  elif [ "$c_size" = "$r_size" ] && [ "$c_rc" = "$r_rc" ]; then
    printf 'SAME      %-12s rc=%s frames=%s\n' "$name" "$c_rc" "$((c_size / 152064))"
  else
    printf 'NAMED     %-12s C=%s/%s Rust=%s/%s | %s\n' \
      "$name" "$c_rc" "$c_size" "$r_rc" "$r_size" "$r_msg"
  fi
done
