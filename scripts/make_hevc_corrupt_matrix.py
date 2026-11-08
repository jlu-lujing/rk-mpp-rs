#!/usr/bin/env python3
"""Create deterministic H.265 bit-flip and truncation cases."""

import argparse
import pathlib


def start_codes(data: bytes) -> list[tuple[int, int]]:
    starts = []
    offset = 0
    while offset + 3 <= len(data):
        if data[offset : offset + 4] == b"\x00\x00\x00\x01":
            starts.append((offset, 4))
            offset += 4
        elif data[offset : offset + 3] == b"\x00\x00\x01":
            starts.append((offset, 3))
            offset += 3
        else:
            offset += 1
    return starts


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("source", type=pathlib.Path)
    parser.add_argument("output_dir", type=pathlib.Path)
    args = parser.parse_args()

    data = args.source.read_bytes()
    starts = start_codes(data)
    if not starts or starts[0][0] != 0:
        raise SystemExit("stream does not begin at an Annex-B start code")

    nals = []
    for index, (offset, prefix) in enumerate(starts):
        end = starts[index + 1][0] if index + 1 < len(starts) else len(data)
        nal_type = (data[offset + prefix] >> 1) & 0x3F
        nals.append((offset, end, prefix, nal_type))

    def first_of_type(nal_type: int) -> tuple[int, int, int, int]:
        for nal in nals:
            if nal[3] == nal_type:
                return nal
        raise SystemExit(f"stream has no NAL type {nal_type}")

    vcl = [nal for nal in nals if nal[3] <= 31]
    if len(vcl) != 6:
        raise SystemExit(f"expected 6 VCL NAL units, found {len(vcl)}")

    args.output_dir.mkdir(parents=True, exist_ok=True)
    cases: dict[str, int] = {}

    for name, nal_type in (("sps_mid", 33), ("pps_mid", 34), ("sei_mid", 39)):
        offset, end, prefix, _ = first_of_type(nal_type)
        cases[name] = offset + prefix + (end - offset - prefix) // 2

    for index in (0, 4):
        offset, end, prefix, _ = vcl[index]
        payload = end - offset - prefix
        for name, position in (
            ("start", 2),
            ("mid", payload // 2),
            ("end", payload - 3),
        ):
            cases[f"vcl{index + 1}_{name}"] = offset + prefix + position

    for index in (1, 2, 3, 5):
        offset, end, prefix, _ = vcl[index]
        cases[f"vcl{index + 1}_mid"] = offset + prefix + (end - offset - prefix) // 2

    for name, offset in cases.items():
        corrupt = bytearray(data)
        corrupt[offset] ^= 0x04
        path = args.output_dir / f"{name}.265"
        path.write_bytes(corrupt)
        print(f"{name} {path} flip={offset}")

    for name, cut in (("tail100", 100), ("tail500", 500), ("tail1500", 1500)):
        path = args.output_dir / f"{name}.265"
        path.write_bytes(data[:-cut])
        print(f"{name} {path} truncate={cut}")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
