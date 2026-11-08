#!/usr/bin/env python3
"""Drop one AUD-delimited access unit from an Annex-B HEVC stream."""

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


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("source", type=pathlib.Path)
    parser.add_argument("output", type=pathlib.Path)
    parser.add_argument(
        "--drop",
        type=int,
        required=True,
        help="one-based access-unit index; AU boundaries must be marked by AUD NALs",
    )
    args = parser.parse_args()

    data = args.source.read_bytes()
    starts = start_codes(data)
    if not starts or starts[0][0] != 0:
        raise SystemExit("stream does not begin at an Annex-B start code")

    nal_types = []
    for index, (offset, prefix) in enumerate(starts):
        end = starts[index + 1][0] if index + 1 < len(starts) else len(data)
        nal_types.append((offset, end, (data[offset + prefix] >> 1) & 0x3F))

    auds = [index for index, (_, _, nal_type) in enumerate(nal_types) if nal_type == 35]
    if args.drop < 1 or args.drop > len(auds):
        raise SystemExit(
            f"access-unit index {args.drop} is outside 1..={len(auds)}"
        )

    unit = auds[args.drop - 1]
    unit_end = auds[args.drop] if args.drop < len(auds) else len(nal_types)
    begin = nal_types[unit][0]
    end = nal_types[unit_end][0] if unit_end < len(nal_types) else len(data)
    if not any(nal_types[index][2] <= 31 for index in range(unit + 1, unit_end)):
        raise SystemExit(f"access unit {args.drop} has no VCL NAL")

    args.output.write_bytes(data[:begin] + data[end:])
    print(
        f"source_bytes={len(data)} output_bytes={len(data) - (end - begin)} "
        f"access_units={len(auds)} dropped={args.drop} dropped_bytes={end - begin}"
    )


if __name__ == "__main__":
    main()
