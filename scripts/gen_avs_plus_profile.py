#!/usr/bin/env python3
"""Derive an AVS+ profile 0x48 stream from the public base-AVS fixture.

The slice payload is preserved. Each I/PB picture header gains the AVS+
bbv-delay extension and trailing weighting/AEC flags; both flags are disabled,
so the transformed stream remains syntax-equivalent to the source pictures.
"""

import hashlib
import sys
from pathlib import Path


def get_bits(data: bytes, start: int, count: int) -> int:
    value = 0
    for offset in range(count):
        position = start + offset
        value = (value << 1) | ((data[position // 8] >> (7 - position % 8)) & 1)
    return value


def parse_i_end(payload: bytes) -> int:
    position = 0

    def take(count: int) -> int:
        nonlocal position
        value = get_bits(payload, position, count)
        position += count
        return value

    take(16)
    if take(1):
        take(24)
    if get_bits(payload, position, 9) & 1:
        if get_bits(payload, position, 11) & 3:
            take(1)
    else:
        take(1)
    take(8)
    progressive = take(1)
    if not progressive:
        take(1)
    take(1)
    take(1)
    take(1)
    take(6)
    if not progressive:
        take(1)
    take(4)
    loop_disable = take(1)
    if not loop_disable and take(1):
        take(1)
        take(1)
    return position


def parse_pb_end(payload: bytes) -> int:
    position = 0

    def take(count: int) -> int:
        nonlocal position
        value = get_bits(payload, position, count)
        position += count
        return value

    take(16)
    picture_type = take(2)
    take(8)
    progressive = take(1)
    structure = 1
    if not progressive:
        structure = take(1)
        if not structure:
            take(1)
    take(1)
    take(1)
    take(1)
    take(6)
    if not (picture_type == 2 and structure == 1):
        take(1)
    take(1)
    take(1)
    take(2)
    take(1)
    loop_disable = take(1)
    if not loop_disable and take(1):
        take(1)
        take(1)
    return position


def append_bits(out: bytearray, position: int, value: int, count: int) -> int:
    for offset in range(count):
        bit = (value >> (count - 1 - offset)) & 1
        byte_index = position // 8
        while len(out) <= byte_index:
            out.append(0)
        if bit:
            out[byte_index] |= 1 << (7 - position % 8)
        position += 1
    return position


def transform_header(payload: bytes, end: int) -> bytes:
    out = bytearray()
    position = 0
    for start, stop in ((0, 16), (16, end)):
        for bit_position in range(start, stop):
            bit = (payload[bit_position // 8] >> (7 - bit_position % 8)) & 1
            position = append_bits(out, position, bit, 1)
        if start == 0:
            position = append_bits(out, position, 1, 1)  # marker bit
            position = append_bits(out, position, 0, 7)  # bbv delay extension
    position = append_bits(out, position, 0, 1)  # weighting_quant_flag
    position = append_bits(out, position, 0, 1)  # aec_enable
    if position % 8:
        position = append_bits(out, position, 0, 8 - position % 8)
    return bytes(out)


def derive(source: bytes) -> bytes:
    units = []
    index = 0
    while index + 3 < len(source):
        if source[index : index + 3] == b"\0\0\1":
            units.append((index, source[index + 3]))
            index += 4
        else:
            index += 1

    pictures = []
    for unit_index, (picture_start, code) in enumerate(units):
        if code not in (0xB3, 0xB6):
            continue
        slice_start = None
        for position, next_code in units[unit_index + 1 :]:
            if 0 <= next_code <= 0xAF:
                slice_start = position
                break
            if next_code in (0xB1, 0xB3, 0xB6):
                break
        if slice_start is None:
            raise ValueError(f"picture at {picture_start:#x} has no slice")
        pictures.append((picture_start, code, slice_start))

    out = bytearray()
    cursor = 0
    for picture_start, code, slice_start in pictures:
        out.extend(source[cursor : picture_start + 4])
        payload = source[picture_start + 4 : slice_start]
        end = parse_i_end(payload) if code == 0xB3 else parse_pb_end(payload)
        out.extend(transform_header(payload, end))
        cursor = slice_start
    out.extend(source[cursor:])

    for index in range(len(out) - 4):
        if out[index : index + 4] == b"\0\0\1\xb0" and out[index + 4] == 0x20:
            out[index + 4] = 0x48
    return bytes(out)


def main() -> None:
    if len(sys.argv) not in (3, 4):
        raise SystemExit(
            "usage: gen_avs_plus_profile.py <base.avs> <avs-plus.avs> [first-sequence]"
        )
    source = Path(sys.argv[1]).read_bytes()
    output = derive(source)
    if len(sys.argv) == 4 and sys.argv[3] == "first-sequence":
        video_edit = output.find(b"\0\0\1\xb7", 0x100)
        if video_edit < 0:
            raise ValueError("first sequence has no video-edit boundary")
        output = output[:video_edit] + b"\0\0\1\xb1"
    elif len(sys.argv) == 4:
        raise SystemExit(f"unknown mode: {sys.argv[3]}")
    Path(sys.argv[2]).write_bytes(output)
    print(
        f"wrote {sys.argv[2]}: {len(output)} bytes "
        f"sha256={hashlib.sha256(output).hexdigest()}"
    )


if __name__ == "__main__":
    main()
