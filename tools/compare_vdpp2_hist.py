#!/usr/bin/env python3

"""Validate VDPP2 packed histogram expansion and U10 mean luma."""

from __future__ import annotations

import argparse
from pathlib import Path

LOCAL_BINS = 16 * 16 * 16
GLOBAL_BINS = 256
PACKED_BYTES = 10_240
LOCAL_PACKED_BYTES = LOCAL_BINS // 4 * 9


def unpack_local(packed: bytes) -> list[int]:
    local: list[int] = []
    for offset in range(0, LOCAL_PACKED_BYTES, 9):
        data = packed[offset : offset + 9]
        local.extend(
            (
                data[0] | data[1] << 8 | (data[2] & 0x03) << 16,
                data[2] >> 2 | data[3] << 6 | (data[4] & 0x0F) << 14,
                data[4] >> 4 | data[5] << 4 | (data[6] & 0x3F) << 12,
                data[6] >> 6 | data[7] << 2 | data[8] << 10,
            )
        )
    return local


def u32_values(path: Path) -> list[int]:
    data = path.read_bytes()
    if len(data) % 4:
        raise SystemExit(f"{path}: size is not a multiple of four")
    return [
        int.from_bytes(data[offset : offset + 4], "little")
        for offset in range(0, len(data), 4)
    ]


def mean_luma(global_hist: list[int]) -> int:
    luma_sum = sum(count * (index * 8 + 3) for index, count in enumerate(global_hist))
    pixel_count = sum(global_hist)
    return -1 if pixel_count == 0 else luma_sum // pixel_count // 2


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("packed", type=Path)
    parser.add_argument("local", type=Path)
    parser.add_argument("global_hist", type=Path)
    parser.add_argument("rust_log", type=Path)
    arguments = parser.parse_args()

    packed = arguments.packed.read_bytes()
    if len(packed) != PACKED_BYTES:
        raise SystemExit(f"{arguments.packed}: expected {PACKED_BYTES} bytes")

    expected_local = unpack_local(packed)
    expected_global = [
        int.from_bytes(packed[offset : offset + 4], "little")
        for offset in range(LOCAL_PACKED_BYTES, PACKED_BYTES, 4)
    ]
    rust_local = u32_values(arguments.local)
    rust_global = u32_values(arguments.global_hist)
    if rust_local != expected_local:
        raise SystemExit("Rust local histogram differs from the packed C buffer")
    if rust_global != expected_global:
        raise SystemExit("Rust global histogram differs from the packed C buffer")

    expected_mean = mean_luma(expected_global)
    rust_mean = None
    for line in arguments.rust_log.read_text().splitlines():
        if line.startswith("mean_luma_u10="):
            rust_mean = int(line.partition("=")[2])
            break
    if rust_mean != expected_mean:
        raise SystemExit(f"mean luma mismatch: Rust {rust_mean}, packed {expected_mean}")
    print(f"histogram identical: local={LOCAL_BINS} global={GLOBAL_BINS} mean={expected_mean}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
