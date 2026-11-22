#!/usr/bin/env bash
# Verify native h265-gop tune environment variables against C register traces.
set -euo pipefail

BOARD="${BOARD:?set BOARD to the RK3588 SSH alias}"
repo=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
remote_repo="${REMOTE_REPO:-\$HOME/rk-mpp-rs}"
mpp_ref="${MPP_REF:-\$HOME/rk-mpp-oracles/mpp-develop}"
probe=/tmp/h265e_probe_cli_tune
tracer=/tmp/mpp_trace.so
moving=/tmp/rk_mpp_h265_cli_tune_moving.nv12
noise=/tmp/rk_mpp_h265_cli_tune_noise.nv12

ssh "$BOARD" "ffmpeg -nostdin -y -hide_banner -loglevel error \
  -f lavfi -i testsrc2=size=320x240:rate=30 -frames:v 2 \
  -pix_fmt nv12 -f rawvideo '$moving'"
ssh "$BOARD" "ffmpeg -nostdin -y -hide_banner -loglevel error \
  -f lavfi -i 'testsrc2=size=320x240:rate=30,noise=alls=100:allf=t+u' \
  -frames:v 2 -pix_fmt nv12 -f rawvideo '$noise'"
ssh "$BOARD" "cc -O2 -w -pthread -I\"$mpp_ref/inc\" -I\"$remote_repo/tools\" \
  \"$remote_repo/tools/h265e_probe.c\" \
  \"$mpp_ref/build/mpp/librockchip_mpp.so.1\" -o '$probe'"
ssh "$BOARD" "cc -O2 -shared -fPIC \"$remote_repo/tools/mpp_ioctl_trace.c\" \
  -o '$tracer' -ldl"

run_cell() {
  local name="$1" fixture="$2" c_env="$3" rust_env="$4"
  ssh "$BOARD" "sudo env LD_LIBRARY_PATH=\"$mpp_ref/build/mpp\" LD_PRELOAD='$tracer' \
    FRAMES=2 GOP=60 $c_env timeout 120 '$probe' '$fixture' \
    /tmp/rk_mpp_h265_cli_${name}_c.265 320 240 0 26 \
    288000:1500000:2500000 headereach </dev/null >/dev/null \
    2>/tmp/rk_mpp_h265_cli_${name}_c.log"
  ssh "$BOARD" "sudo env LD_PRELOAD='$tracer' H265_GOP=60 \
    RKMPP_ENCODER_PIPELINE=0 $rust_env timeout 120 \
    \"$remote_repo/target/release/rk-mpp-rs\" h265-gop '$fixture' \
    /tmp/rk_mpp_h265_cli_${name}_r.265 320 240 2 26 \
    </dev/null >/dev/null 2>/tmp/rk_mpp_h265_cli_${name}_r.log"
  scp -q "$BOARD":/tmp/rk_mpp_h265_cli_${name}_c.log \
    "$BOARD":/tmp/rk_mpp_h265_cli_${name}_r.log /tmp/
  python3 - "$repo/tools/h265e_trace_diff.py" \
    "/tmp/rk_mpp_h265_cli_${name}_c.log" \
    "/tmp/rk_mpp_h265_cli_${name}_r.log" "$name" <<'PY'
import importlib.util
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("trace_diff", sys.argv[1])
trace_diff = importlib.util.module_from_spec(spec)
spec.loader.exec_module(trace_diff)
c = trace_diff.traced_passes(Path(sys.argv[2]))
r = trace_diff.traced_passes(Path(sys.argv[3]))
assert len(c) == len(r) == 2
for index, (left, right) in enumerate(zip(c, r)):
    assert left["WGT"] == right["WGT"], f"WGT differs at job {index}"
    assert left["RDO"] == right["RDO"], f"RDO differs at job {index}"
if sys.argv[4] == "scene":
    for index, (left, right) in enumerate(zip(c, r)):
        assert left["OSD"] == right["OSD"], f"OSD differs at job {index}"
        assert left["RC_KLUT"][1056 - 1024] == right["RC_KLUT"][1056 - 1024]
print(f"{sys.argv[4]}: WGT/RDO exact")
PY
}

run_cell scene "$noise" "SCENE_MODE=1" "H265_SCENE_MODE=1"
run_cell qbias "$moving" "QBIAS_EN=1 QBIAS_I=200 QBIAS_P=100" \
  "H265_QBIAS_EN=1 H265_QBIAS_I=200 H265_QBIAS_P=100"
run_cell vmaf "$moving" "VMAF_OPT=1" "H265_VMAF_OPT=1"

printf 'PASS H.265 native CLI tune trace\n'
