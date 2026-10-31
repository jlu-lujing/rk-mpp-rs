#!/usr/bin/env python3
"""Insert an explicit scaling_list_data block into the first HEVC PPS."""

from __future__ import annotations

import sys


class Bits:
    def __init__(self, data: bytes = b""):
        self.bits = [int(bit) for byte in data for bit in f"{byte:08b}"]
        self.pos = 0

    def read(self, count: int) -> int:
        value = 0
        for bit in self.bits[self.pos : self.pos + count]:
            value = (value << 1) | bit
        if count and self.pos + count > len(self.bits):
            raise ValueError("truncated RBSP")
        self.pos += count
        return value

    def ue(self) -> int:
        zeros = 0
        while self.read(1) == 0:
            zeros += 1
            if zeros > 31:
                raise ValueError("invalid Exp-Golomb code")
        return (1 << zeros) - 1 + self.read(zeros)

    def se(self) -> int:
        code = self.ue()
        return (code + 1) // 2 if code & 1 else -(code // 2)


class BitWriter:
    def __init__(self):
        self.bits: list[int] = []

    def put(self, value: int, count: int) -> None:
        if value < 0 or value >= 1 << count:
            raise ValueError(f"{value} does not fit in {count} bits")
        self.bits.extend((value >> shift) & 1 for shift in range(count - 1, -1, -1))

    def ue(self, value: int) -> None:
        code = value + 1
        width = code.bit_length()
        self.bits.extend([0] * (width - 1))
        self.put(code, width)

    def se(self, value: int) -> None:
        self.ue(2 * value - 1 if value > 0 else -2 * value)

    def bytes(self) -> bytes:
        padded = self.bits + [0] * ((-len(self.bits)) % 8)
        return bytes(
            sum(padded[i + bit] << (7 - bit) for bit in range(8))
            for i in range(0, len(padded), 8)
        )


DIAG_4X4 = [0, 4, 1, 8, 5, 2, 12, 9, 6, 3, 13, 10, 7, 14, 11, 15]


def split_annex_b(data: bytes) -> list[bytes]:
    starts: list[tuple[int, int]] = []
    i = 0
    while i + 3 <= len(data):
        if data[i : i + 3] == b"\x00\x00\x01":
            starts.append((i, i + 3))
            i += 3
        elif i + 4 <= len(data) and data[i : i + 4] == b"\x00\x00\x00\x01":
            starts.append((i, i + 4))
            i += 4
        else:
            i += 1
    if not starts:
        raise ValueError("input is not Annex-B")
    nals = []
    for index, (_, payload) in enumerate(starts):
        end = starts[index + 1][0] if index + 1 < len(starts) else len(data)
        nal = data[payload:end].rstrip(b"\x00")
        if len(nal) >= 2:
            nals.append(nal)
    return nals


def unescape(payload: bytes) -> bytes:
    out = bytearray()
    zeros = 0
    for byte in payload:
        if zeros >= 2 and byte == 3:
            zeros = 0
            continue
        out.append(byte)
        zeros = zeros + 1 if byte == 0 else 0
    return bytes(out)


def escape(rbsp: bytes) -> bytes:
    out = bytearray()
    zeros = 0
    for byte in rbsp:
        if zeros >= 2 and byte <= 3:
            out.append(3)
            zeros = 0
        out.append(byte)
        zeros = zeros + 1 if byte == 0 else 0
    return bytes(out)


def skip_pps_prefix(reader: Bits) -> int:
    reader.ue()  # pps_pic_parameter_set_id
    reader.ue()  # pps_seq_parameter_set_id
    reader.read(1)  # dependent_slice_segments_enabled_flag
    reader.read(1)  # output_flag_present_flag
    reader.read(3)  # num_extra_slice_header_bits
    reader.read(1)  # sign_data_hiding_enabled_flag
    reader.read(1)  # cabac_init_present_flag
    reader.ue()  # num_ref_idx_l0_default_active_minus1
    reader.ue()  # num_ref_idx_l1_default_active_minus1
    reader.se()  # init_qp_minus26
    reader.read(1)  # constrained_intra_pred_flag
    reader.read(1)  # transform_skip_enabled_flag
    if reader.read(1):  # cu_qp_delta_enabled_flag
        reader.ue()
    reader.se()  # pps_cb_qp_offset
    reader.se()  # pps_cr_qp_offset
    reader.read(1)  # pps_slice_chroma_qp_offsets_present_flag
    reader.read(1)  # weighted_pred_flag
    reader.read(1)  # weighted_bipred_flag
    reader.read(1)  # transquant_bypass_enabled_flag
    tiles = reader.read(1)
    entropy_sync = reader.read(1)
    if tiles:
        columns = reader.ue() + 1
        rows = reader.ue() + 1
        if not reader.read(1):
            for _ in range(columns - 1):
                reader.ue()
            for _ in range(rows - 1):
                reader.ue()
        reader.read(1)  # loop_filter_across_tiles_enabled_flag
    if entropy_sync:
        reader.read(1)  # pps_loop_filter_across_substreams_enabled_flag
    reader.read(1)  # pps_loop_filter_across_slices_enabled_flag
    if reader.read(1):  # deblocking_filter_control_present_flag
        reader.read(1)  # deblocking_filter_override_enabled_flag
        if not reader.read(1):  # pps_deblocking_filter_disabled_flag
            reader.se()
            reader.se()
    return reader.pos


def scaling_data() -> list[int]:
    writer = BitWriter()
    custom_4x4 = [16, 12, 12, 11, 15, 15, 15, 15, 15, 15, 15, 16, 16, 16, 17, 18]
    for size_id in range(4):
        for matrix_id in range(0, 6, 3 if size_id == 3 else 1):
            if size_id == 0 and matrix_id == 0:
                writer.put(1, 1)  # scaling_list_pred_mode_flag
                coefficient = 8
                for position in DIAG_4X4:
                    delta = custom_4x4[position] - coefficient
                    if delta < -128:
                        delta += 256
                    elif delta > 127:
                        delta -= 256
                    writer.se(delta)
                    coefficient = (coefficient + delta + 256) % 256
            else:
                writer.put(0, 1)  # scaling_list_pred_mode_flag
                writer.ue(0)  # scaling_list_pred_matrix_id_delta
    return writer.bits


def add_pps_scaling_data(nal: bytes) -> bytes:
    if len(nal) < 4 or ((nal[0] >> 1) & 0x3F) != 34:
        raise ValueError("expected a PPS NAL")
    rbsp = unescape(nal[2:])
    reader = Bits(rbsp)
    offset = skip_pps_prefix(reader)
    if reader.read(1) != 0:
        raise ValueError("PPS already contains scaling_list_data")

    writer = BitWriter()
    writer.bits.extend(reader.bits[:offset])
    writer.put(1, 1)
    writer.bits.extend(scaling_data())
    writer.bits.extend(reader.bits[offset + 1 :])
    return nal[:2] + escape(writer.bytes())


def main() -> int:
    if len(sys.argv) != 3:
        print(f"usage: {sys.argv[0]} input.hevc output.hevc", file=sys.stderr)
        return 2
    nals = split_annex_b(open(sys.argv[1], "rb").read())
    found = False
    output = bytearray()
    for nal in nals:
        if not found and len(nal) >= 2 and ((nal[0] >> 1) & 0x3F) == 34:
            nal = add_pps_scaling_data(nal)
            found = True
        output.extend(b"\x00\x00\x00\x01")
        output.extend(nal)
    if not found:
        raise ValueError("input has no PPS")
    open(sys.argv[2], "wb").write(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
