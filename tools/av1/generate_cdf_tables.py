#!/usr/bin/env python3
"""Copy board-generated MPP CDF snapshots into the Rust crate.

The snapshots are produced by cdf_abi_probe.c against the official C MPP
headers and library. Keeping this mechanical avoids reretyping thousands of
16-bit CDF entries by hand.
"""

from pathlib import Path
import shutil


ROOT = Path(__file__).resolve().parents[2]
SOURCE = ROOT / "tests" / "data" / "av1_cdf"
DESTINATION = ROOT / "src" / "codec" / "av1" / "data"

TABLES = (
    "cdf_q000.bin",
    "cdf_q021.bin",
    "cdf_q061.bin",
    "cdf_q121.bin",
    "cdf_ndvc.bin",
)


def main() -> None:
    DESTINATION.mkdir(parents=True, exist_ok=True)
    for name in TABLES:
        source = SOURCE / name
        if not source.is_file():
            raise SystemExit(f"missing C MPP snapshot: {source}")
        shutil.copyfile(source, DESTINATION / name)


if __name__ == "__main__":
    main()
