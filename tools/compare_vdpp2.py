#!/usr/bin/env python3
"""Compare the Rust VDPP2 scale register image with a C fake-service trace."""

from __future__ import annotations

import re
import sys
from pathlib import Path


SECTIONS = {
    0x2000: "yrgb_hor_coe",
    0x2200: "yrgb_ver_coe",
    0x2400: "cbcr_hor_coe",
    0x2600: "cbcr_ver_coe",
    0x2800: "zme_common",
    0x0080: "dmsr",
    0x00E0: "hist",
    0x0100: "es",
    0x0200: "sharp",
    0x0000: "common",
}
ADDRESS_WORDS = {24, 25, 26, 27}


def rust_sections(path: Path) -> dict[str, list[int]]:
    result = {}
    for line in path.read_text().splitlines():
        if ":" not in line:
            continue
        name, values = line.split(":", 1)
        result[name] = [int(value, 16) for value in values.split()]
    return result


def c_sections(path: Path) -> dict[str, list[int]]:
    lines = path.read_text(errors="replace").splitlines()
    result = {}
    for index, line in enumerate(lines):
        if "fake cmd=200" not in line or "offset=" not in line:
            continue
        fields = dict(item.split("=", 1) for item in line.split() if "=" in item)
        offset = int(fields["offset"], 0)
        name = SECTIONS.get(offset)
        if not name:
            continue
        words = [int(value, 16) for value in lines[index + 1].split()[3:]]
        result[name] = words
    return result


def main() -> int:
    arguments = sys.argv[1:]
    histogram = arguments[:1] == ["--hist"]
    enhancement = arguments[:1] == ["--enh"]
    if histogram:
        arguments = arguments[1:]
    if enhancement:
        arguments = arguments[1:]
    if len(arguments) != 2:
        print("usage: compare_vdpp2.py [--hist] rust.dump c.trace", file=sys.stderr)
        return 2
    rust = rust_sections(Path(arguments[0]))
    c = c_sections(Path(arguments[1]))
    failed = False
    if histogram:
        names = ("common", "hist")
    elif enhancement:
        names = (
            "yrgb_hor_coe",
            "yrgb_ver_coe",
            "cbcr_hor_coe",
            "cbcr_ver_coe",
            "zme_common",
            "dmsr",
            "hist",
            "es",
            "sharp",
            "common",
        )
    else:
        names = (
            "yrgb_hor_coe",
            "yrgb_ver_coe",
            "cbcr_hor_coe",
            "cbcr_ver_coe",
            "zme_common",
            "common",
        )
    for name in names:
        left, right = rust[name], c[name]
        if len(left) != len(right):
            print(f"{name}: length {len(left)} != {len(right)}")
            failed = True
            continue
        differences = []
        for index, (a, b) in enumerate(zip(left, right)):
            if name == "common" and index in ADDRESS_WORDS:
                continue
            if name == "hist" and index in {0, 4}:
                continue
            if a != b:
                differences.append((index, a, b))
        if differences:
            failed = True
            details = ", ".join(
                f"sw{index}={a:08x}/{b:08x}" for index, a, b in differences
            )
            print(f"{name}: {len(differences)} differences: {details}")
        else:
            print(f"{name}: identical")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
