#!/usr/bin/env python3
"""Compare the official Rockchip MPP headers with a librk_mpp_rs.so.

Usage:
    tools/check_mpp_api_coverage.py --headers ../refs/mpp/inc \
        --library target/release/librk_mpp_rs.so
    tools/check_mpp_api_coverage.py --headers ../refs/mpp/inc \
        --symbols /tmp/nm.txt          # output of: nm -D --defined-only ...

Prints the per-module coverage and exits 1 when a *core* symbol is missing.
Core modules are the ones a normal application links against; `kcfg`,
`sys_cfg` and `task_meta` are reported separately as optional.
"""

import argparse
import glob
import os
import re
import subprocess
import sys

OPTIONAL_PREFIXES = (
    "mpp_venc_kcfg_",
    "mpp_venc_ctrl_",
    "mpp_vdec_kcfg_",
    "mpp_sys_cfg_",
    "mpp_sys_dec_buf_chk_proc",
    "mpp_task_meta_",
)


def declared(headers):
    names = set()
    for path in glob.glob(os.path.join(headers, "*.h")):
        for line in open(path, errors="ignore"):
            text = line.strip()
            if not text or text.startswith(("#", "//", "*")):
                continue
            if "(" not in text:
                continue
            match = re.match(r"^(?:[A-Za-z_][\w\s\*]*?)\b([A-Za-z_]\w*)\s*\(", text)
            if not match:
                continue
            name = match.group(1)
            if name.startswith(("mpp_", "Mpp")):
                names.add(name)
    return names


def exported(library, symbols):
    if symbols:
        text = open(symbols).read()
    else:
        out = subprocess.run(
            ["nm", "-D", "--defined-only", library],
            check=True,
            capture_output=True,
            text=True,
        )
        text = out.stdout
    return {line.split()[-1] for line in text.splitlines() if line.split()}


def group(name):
    if name.startswith("mpp_venc_"):
        return "mpp_venc"
    if name.startswith("mpp_vdec_"):
        return "mpp_vdec"
    if name.startswith("mpp_sys_"):
        return "mpp_sys"
    if name.startswith("mpp_task_"):
        return "mpp_task"
    if name.startswith("mpp_frame_"):
        return "mpp_frame"
    if name.startswith("mpp_compat_"):
        return "mpp_compat"
    if name.startswith("mpp_enc_cfg_"):
        return "mpp_enc_cfg"
    if name.startswith("mpp_dec_cfg_"):
        return "mpp_dec_cfg"
    if name.startswith("mpp_buffer_"):
        return "mpp_buffer"
    if name.startswith("mpp_packet_"):
        return "mpp_packet"
    if name.startswith("mpp_meta_"):
        return "mpp_meta"
    return "other"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--headers", required=True, help="official MPP inc/ directory")
    parser.add_argument("--library", help="librk_mpp_rs.so to inspect with nm")
    parser.add_argument("--symbols", help="text file with one exported symbol per line")
    args = parser.parse_args()

    if not args.library and not args.symbols:
        parser.error("pass --library or --symbols")

    want = declared(args.headers)
    have = exported(args.library, args.symbols)
    missing = sorted(want - have)

    rows = {}
    for name in want:
        key = group(name)
        total, covered = rows.get(key, (0, 0))
        rows[key] = (total + 1, covered + (name in have))

    print(f"{'module':<14} {'covered':>8} {'declared':>9}  status")
    core_missing = 0
    for key in sorted(rows):
        total, covered = rows[key]
        optional = any(key.startswith(p) for p in ("mpp_venc", "mpp_vdec", "mpp_sys", "mpp_task"))
        status = "OK" if covered == total else ("optional" if optional else "MISSING")
        if not optional and covered != total:
            core_missing += total - covered
        print(f"{key:<14} {covered:>8} {total:>9}  {status}")

    optional_missing = sum(1 for n in missing if n.startswith(OPTIONAL_PREFIXES))
    print(f"\ntotal declared={len(want)} covered={len(want & have)} "
          f"missing={len(missing)} (optional {optional_missing}, core {core_missing})")
    if core_missing:
        print("\nCore symbols missing:")
        for name in missing:
            if not name.startswith(OPTIONAL_PREFIXES):
                print(f"  {name}")
        return 1
    print("\ncore API: complete")
    return 0


if __name__ == "__main__":
    sys.exit(main())
