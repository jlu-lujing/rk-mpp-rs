#!/usr/bin/env python3
"""Compare C and Rust low-delay packet layouts from encoder logs."""

import re
import sys

C_PACKET = re.compile(r"^picture (\d+) packet (\d+): (\d+) bytes eoi=(\d+)$")
RUST_FRAME = re.compile(
    r"^(?:h264|jpeg) frame (\d+): (\d+) packet\(s\): ([\d,]*)$"
)


def c_frames(path):
    frames = {}
    for line in open(path, encoding="utf-8", errors="replace"):
        match = C_PACKET.match(line.strip())
        if not match:
            continue
        frame, part, size, eoi = map(int, match.groups())
        frames.setdefault(frame, []).append((part, size, eoi))
    return frames


def rust_frames(path):
    frames = {}
    for line in open(path, encoding="utf-8", errors="replace"):
        match = RUST_FRAME.match(line.strip())
        if not match:
            continue
        frame, count, sizes = match.groups()
        values = [] if not sizes else [int(value) for value in sizes.split(",")]
        frames[int(frame)] = values
        if len(values) != int(count):
            raise ValueError(
                f"Rust frame {frame} says {count} packets but lists {len(values)}"
            )
    return frames


def main() -> int:
    c = c_frames(sys.argv[1])
    rust = rust_frames(sys.argv[2])
    if sorted(c) != sorted(rust):
        print(f"    frame set differs: C {sorted(c)} vs Rust {sorted(rust)}")
        return 1

    first_deltas = []
    normalized_zero_eoi = 0
    for frame in sorted(c):
        c_entries = c[frame]
        c_sizes = [size for _, size, _ in c_entries]
        rust_sizes = rust[frame]
        if [part for part, _, _ in c_entries] != list(range(len(c_entries))):
            print(f"    C frame {frame} packet indices are not contiguous")
            return 1
        if not c_entries or c_entries[-1][2] != 1:
            print(f"    C frame {frame} does not end with an EOI packet")
            return 1

        # The kernel may attach the last flag to the final slice entry or emit
        # one extra zero-length EOI entry. Both end the picture at the same
        # offset; which form wins is timing-dependent and was observed to vary
        # between otherwise identical Rust runs. Normalize only that trailing
        # zero, never a zero in the middle of a picture.
        c_data_sizes = c_sizes[:-1] if c_sizes[-1] == 0 else c_sizes
        rust_data_sizes = (
            rust_sizes[:-1] if rust_sizes and rust_sizes[-1] == 0 else rust_sizes
        )
        if (c_sizes[-1] == 0) != bool(rust_sizes and rust_sizes[-1] == 0):
            normalized_zero_eoi += 1

        if len(c_data_sizes) != len(rust_data_sizes):
            print(
                f"    frame {frame} data-packet count differs: "
                f"C {len(c_data_sizes)} vs Rust {len(rust_data_sizes)}"
            )
            return 1
        if not c_data_sizes:
            print(f"    frame {frame} contains no data packets")
            return 1

        first_delta = c_data_sizes[0] - rust_data_sizes[0]
        if first_delta < 0:
            print(
                f"    frame {frame} first packet is larger in Rust: "
                f"C {c_data_sizes[0]} vs Rust {rust_data_sizes[0]}"
            )
            return 1
        first_deltas.append(first_delta)
        if c_data_sizes[1:] != rust_data_sizes[1:]:
            at = next(
                index
                for index, (left, right) in enumerate(
                    zip(c_data_sizes[1:], rust_data_sizes[1:]), start=1
                )
                if left != right
            )
            print(
                f"    frame {frame} packet {at} differs: "
                f"C {c_data_sizes[at]} vs Rust {rust_data_sizes[at]}"
            )
            return 1

    total = sum(
        len(frames) - 1 if frames[-1][1] == 0 else len(frames)
        for frames in c.values()
    )
    suffix = ""
    if normalized_zero_eoi:
        suffix = (
            f"; {normalized_zero_eoi} trailing zero-length EOI "
            "representation(s) normalized"
        )
    print(
        f"    {total} data packets over {len(c)} frames; slice boundaries match "
        f"(first-packet header deltas {first_deltas}){suffix}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
