#!/usr/bin/env bash
# Verify VEPU580 H.265 IPC scene mode and its row-0 -> row-1 transition.
#
# The probe's stream framing differs from mpi_enc_test for reasons unrelated
# to scene mode, so the gate compares the scene-sensitive register sections
# job by job instead of claiming whole-stream identity.
set -euo pipefail

BOARD="${BOARD:?set BOARD to the RK3588 SSH alias}"
repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
remote_repo="${REMOTE_REPO:-\$HOME/rk-mpp-rs}"
mpp_ref="${MPP_REF:-\$HOME/rk-mpp-oracles/mpp-develop}"
frames="${FRAMES:-160}"
fixture="${FIXTURE:-/tmp/rk_mpp_h265_scene_ipc_noise.nv12}"
probe=/tmp/h265e_probe_scene_mode
tracer=/tmp/mpp_trace.so
c_trace=/tmp/rk_mpp_h265_scene_ipc_c.log
r_trace=/tmp/rk_mpp_h265_scene_ipc_r.log

ssh "$BOARD" "ffmpeg -nostdin -y -hide_banner -loglevel error \
  -f lavfi -i 'testsrc2=size=320x240:rate=30,noise=alls=100:allf=t+u' \
  -frames:v '$frames' -pix_fmt nv12 -f rawvideo '$fixture'"

ssh "$BOARD" "cc -O2 -w -pthread -I\"$mpp_ref/inc\" \"$remote_repo/tools/h265e_probe.c\" \
  \"$mpp_ref/build/mpp/librockchip_mpp.so.1\" -o '$probe'"
ssh "$BOARD" "cc -O2 -shared -fPIC \"$remote_repo/tools/mpp_ioctl_trace.c\" \
  -o '$tracer' -ldl"

ssh "$BOARD" "sudo env LD_LIBRARY_PATH=\"$mpp_ref/build/mpp\" LD_PRELOAD='$tracer' \
  FRAMES='$frames' GOP=60 SCENE_MODE=1 timeout 120 '$probe' '$fixture' \
  /tmp/rk_mpp_h265_scene_ipc_c.265 320 240 0 26 288000:1500000:2500000 headereach \
  </dev/null >/dev/null 2>'$c_trace'"

ssh "$BOARD" "sudo env LD_PRELOAD=\"$tracer:$remote_repo/target/release/librk_mpp_rs.so\" \
  LD_LIBRARY_PATH=\"$mpp_ref/build/mpp\" FRAMES='$frames' GOP=60 SCENE_MODE=1 \
  timeout 120 '$probe' '$fixture' /tmp/rk_mpp_h265_scene_ipc_r.265 320 240 0 26 \
  288000:1500000:2500000 headereach </dev/null >/dev/null 2>'$r_trace'"

scp -q "$BOARD:$c_trace" "$BOARD:$r_trace" /tmp/
python3 - "$c_trace" "$r_trace" <<'PY'
import importlib.util
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("trace_diff", "tools/h265e_trace_diff.py")
trace_diff = importlib.util.module_from_spec(spec)
spec.loader.exec_module(trace_diff)

c = trace_diff.traced_passes(Path(sys.argv[1]))
r = trace_diff.traced_passes(Path(sys.argv[2]))
assert len(c) == len(r) and len(c) == 160, (len(c), len(r))

def signature(job):
    return (
        job["RDO"][0],
        job["RDO"][2065 - 2048],
        job["WGT"][1479 - 1472],
        job["WGT"][1496 - 1472],
        job["WGT"][1498 - 1472],
    )

assert signature(c[0]) == signature(r[0]), (signature(c[0]), signature(r[0]))
assert signature(c[5]) == signature(r[5]), (signature(c[5]), signature(r[5]))
assert signature(c[0]) == (3, 0x0F0E0D10, 0x12131415, 0x304, 0x80004)
assert signature(c[5]) == (0, 0x10101010, 0x10111213, 0x300, 0x70004)

for name in ("WGT", "RDO", "OSD"):
    for index, (left, right) in enumerate(zip(c, r)):
        assert left[name] == right[name], f"{name} differs at job {index}"

for index, (left, right) in enumerate(zip(c, r)):
    assert left["RC_KLUT"][1056 - 1024] == right["RC_KLUT"][1056 - 1024], (
        f"RC_KLUT[1056] differs at job {index}"
    )

print("160 jobs: WGT/RDO/OSD exact; IPC row 0 -> row 1 transition exact")
PY

printf 'PASS H.265 VEPU580 IPC scene mode rows 0/1\n'
