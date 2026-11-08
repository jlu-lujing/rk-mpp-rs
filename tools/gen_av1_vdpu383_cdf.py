#!/usr/bin/env python3
"""Extract the VDPU383 AV1 default CDF image from official MPP source."""

import argparse
import hashlib
import pathlib
import re
import struct


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("source", type=pathlib.Path)
    parser.add_argument("output", type=pathlib.Path)
    args = parser.parse_args()

    text = args.source.read_text()
    match = re.search(
        r"g_av1d_default_prob\s*\[\s*7400\s*\]\s*=\s*\{(?P<body>.*?)\n\};",
        text,
        re.S,
    )
    if not match:
        raise SystemExit("g_av1d_default_prob[7400] was not found")
    words = [int(value, 16) for value in re.findall(r"0x([0-9a-fA-F]+)", match["body"])]
    if len(words) != 7400:
        raise SystemExit(f"expected 7400 words, got {len(words)}")
    data = b"".join(struct.pack("<I", word) for word in words)
    args.output.write_bytes(data)
    print(
        f"words={len(words)} bytes={len(data)} sha256={hashlib.sha256(data).hexdigest()}"
    )


if __name__ == "__main__":
    main()
