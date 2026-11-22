#!/usr/bin/env bash
# Verify tune:vmaf_opt against the H.265 register trace.
set -euo pipefail

BOARD="${BOARD:?set BOARD to the RK3588 SSH alias}"
repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
remote_repo="${REMOTE_REPO:-\$HOME/rk-mpp-rs}"
mpp_ref="${MPP_REF:-\$HOME/rk-mpp-oracles/mpp-develop}"
fixture="${FIXTURE:-/tmp/rk_mpp_h265_vmaf_opt.nv12}"
probe=/tmp/h265e_probe_vmaf_opt
tracer=/tmp/mpp_trace.so

ssh "$BOARD" "ffmpeg -nostdin -y -hide_banner -loglevel error \
  -f lavfi -i testsrc2=size=320x240:rate=30 -frames:v 2 \
  -pix_fmt nv12 -f rawvideo '$fixture'"
ssh "$BOARD" "cc -O2 -w -pthread -I\"$mpp_ref/inc\" -I\"$remote_repo/tools\" \
  \"$remote_repo/tools/h265e_probe.c\" \
  \"$mpp_ref/build/mpp/librockchip_mpp.so.1\" -o '$probe'"
ssh "$BOARD" "cc -O2 -shared -fPIC \"$remote_repo/tools/mpp_ioctl_trace.c\" \
  -o '$tracer' -ldl"

ssh "$BOARD" "sudo env LD_LIBRARY_PATH=\"$mpp_ref/build/mpp\" LD_PRELOAD='$tracer' \
  FRAMES=2 GOP=60 VMAF_OPT=1 timeout 120 '$probe' '$fixture' \
  /tmp/rk_mpp_h265_vmaf_c.265 320 240 0 26 288000:1500000:2500000 headereach \
  </dev/null >/dev/null 2>/tmp/rk_mpp_h265_vmaf_c.log"
ssh "$BOARD" "sudo env LD_PRELOAD=\"$tracer:$remote_repo/target/release/librk_mpp_rs.so\" \
  LD_LIBRARY_PATH=\"$mpp_ref/build/mpp\" FRAMES=2 GOP=60 VMAF_OPT=1 \
  timeout 120 '$probe' '$fixture' /tmp/rk_mpp_h265_vmaf_r.265 320 240 0 26 \
  288000:1500000:2500000 headereach </dev/null >/dev/null \
  2>/tmp/rk_mpp_h265_vmaf_r.log"

scp -q "$BOARD":/tmp/rk_mpp_h265_vmaf_c.log \
  "$BOARD":/tmp/rk_mpp_h265_vmaf_r.log /tmp/
python3 - "$repo/tools/h265e_trace_diff.py" <<'PY'
import importlib.util
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("trace_diff", sys.argv[1])
trace_diff = importlib.util.module_from_spec(spec)
spec.loader.exec_module(trace_diff)
c = trace_diff.traced_passes(Path("/tmp/rk_mpp_h265_vmaf_c.log"))
r = trace_diff.traced_passes(Path("/tmp/rk_mpp_h265_vmaf_r.log"))
assert len(c) == len(r) == 2
for index, (left, right) in enumerate(zip(c, r)):
    assert left["WGT"] == right["WGT"], f"WGT differs at job {index}"
    assert left["RDO"] == right["RDO"], f"RDO differs at job {index}"
assert c[0]["WGT"][1479 - 1472] == 0x10101010
assert c[0]["RDO"][0] == 0
assert c[0]["RDO"][2065 - 2048] == 0x10101010
assert c[0]["RDO"][2066 - 2048] == 0x00101010
assert c[0]["RDO"][2167 - 2048] == 0x001E001F
print("VMAF_OPT=1: pre-intra/ATF row3 and segment/smear exact")
PY

printf 'PASS H.265 VEPU580 vmaf_opt\n'
