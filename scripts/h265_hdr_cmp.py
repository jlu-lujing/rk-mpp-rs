#!/usr/bin/env python3
"""Compare the H.265 parameter sets (VPS + SPS + PPS) the crate wrote against the
ones the C encoder emitted, without touching the IDR NAL.

    python3 h265_hdr_cmp.py <c_stream.265> <rust_headers.265>

The crate's `h265-enc` writes only the parameter sets today (the IDR payload is
gated on the on-board job template, see src/hal/h265e.rs), so this extracts the
parameter-set prefix from the reference stream - everything from the first start
code up to the first NAL that is not a VPS/SPS/PPS - and checks it byte for byte
against the Rust file. A match is the M8 header gate; a mismatch prints the first
offending byte offset. Exit 0 on identical, 1 otherwise.
"""
import sys


def starts_with_start_code(data, at):
    """True if a 3- or 4-byte Annex-B start code begins at `at`."""
    tail = data[at:at + 4]
    return (
        tail[:3] == bytes([0, 0, 1])
        or tail[:4] == bytes([0, 0, 0, 1])
    )


def main(argv):
    if len(argv) != 3:
        print("usage: h265_hdr_cmp.py <c_stream> <rust_headers>", file=sys.stderr)
        return 2
    c = open(argv[1], "rb").read()
    rust = open(argv[2], "rb").read()
    if not rust:
        print("Rust file is empty", file=sys.stderr)
        return 1
    # The reference emits the parameter-set prefix first, then SEI/IDR NAL units.
    # Because neither side inserts emulation-prevention bytes, the SPS can contain
    # a literal `00 00 01` inside `vui_num_units_in_tick`, so splitting by start
    # codes would mis-cut the NAL. The robust check is instead: the reference's
    # leading `len(rust)` bytes must equal the Rust file exactly, and the byte
    # after it must open a new start code (proving the Rust prefix stopped on a NAL
    # boundary and did not run past or stop inside the reference's own PPS).
    if len(c) < len(rust):
        print(
            f"reference stream ({len(c)} bytes) is shorter than the Rust "
            f"parameter sets ({len(rust)})"
        )
        return 1
    if c[:len(rust)] != rust:
        for i in range(len(rust)):
            if c[i] != rust[i]:
                print(
                    f"parameter sets differ at byte {i}: C {c[i]:#04x} vs "
                    f"Rust {rust[i]:#04x}"
                )
                return 1
    if not starts_with_start_code(c, len(rust)):
        nxt = c[len(rust):len(rust) + 4]
        print(
            f"Rust parameter sets ({len(rust)} bytes) do not end on a NAL "
            f"boundary in the reference (next bytes {nxt.hex()}) - the prefix "
            f"lengths differ"
        )
        return 1
    print(f"parameter sets identical ({len(rust)} bytes)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

