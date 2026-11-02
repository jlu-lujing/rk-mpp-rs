#!/usr/bin/env python3
"""Compare C MPP's AV1 DUMP_AV1_DATAS files with rk-mpp-rs images.

Generate the Rust side without touching /dev/mpp_service:

    cargo run --example av1_dump_regs -- \
        tests/data/av1_tiles_320x240_10.ivf 11 /tmp/av1-rust

Build the reference with scripts/av1_reg_dump.patch applied, run it with
DUMP_AV1_DATAS writing to /data/video, then:

    python3 tools/av1/compare_input_dumps.py /data/video /tmp/av1-rust

Register words holding dma-buf descriptors are normalized because process fd
numbers are unrelated. The address-offset maps are compared semantically.
"""

from __future__ import annotations

import argparse
import re
from pathlib import Path

WORD_LINE = re.compile(r"^(?:reg\[\s*\d+\]\s*=\s*)?([0-9a-fA-F]{8})\s*$")
OFFSET_LINE = re.compile(r"^reg\[\s*(\d+)\]\s*\+=\s*0x([0-9a-fA-F]{8})\s*$")

# Direct AV1D DMA addresses and their associated offset-decoded fields.
ADDRESS_REGISTERS = {
    65,
    67,
    69,
    71,
    73,
    75,
    77,
    79,
    81,
    83,
    85,
    89,
    91,
    94,
    95,
    99,
    101,
    103,
    105,
    107,
    109,
    111,
    113,
    133,
    135,
    137,
    139,
    141,
    143,
    145,
    147,
    166,
    167,
    168,
    169,
    171,
    173,
    175,
    177,
    179,
    183,
    326,
    328,
    505,
}


def read_words(path: Path) -> list[int]:
    words: list[int] = []
    for line in path.read_text().splitlines():
        match = WORD_LINE.match(line.strip())
        if not match:
            raise ValueError(f"{path}: unexpected line {line!r}")
        words.append(int(match.group(1), 16))
    return words


def read_offsets(path: Path) -> dict[int, int]:
    offsets: dict[int, int] = {}
    for line in path.read_text().splitlines():
        match = OFFSET_LINE.match(line.strip())
        if not match:
            raise ValueError(f"{path}: unexpected line {line!r}")
        register = int(match.group(1))
        value = int(match.group(2), 16)
        old = offsets.setdefault(register, value)
        if old != value:
            raise ValueError(
                f"{path}: register {register} has conflicting offsets "
                f"0x{old:08x} and 0x{value:08x}"
            )
    return offsets


def first_difference(left: bytes, right: bytes) -> str:
    common = min(len(left), len(right))
    for index in range(common):
        if left[index] != right[index]:
            return f"byte {index}: C={left[index]:02x} Rust={right[index]:02x}"
    if len(left) != len(right):
        return f"length C={len(left)} Rust={len(right)}"
    return "none"


def compare_bytes(
    failures: list[str],
    label: str,
    c_bytes: bytes,
    rust_path: Path,
) -> None:
    if not rust_path.exists():
        failures.append(f"{label}: missing {rust_path}")
        return
    rust_bytes = rust_path.read_bytes()
    if c_bytes == rust_bytes:
        return
    if (
        len(rust_bytes) <= len(c_bytes)
        and c_bytes.startswith(rust_bytes)
        and not any(c_bytes[len(rust_bytes) :])
    ):
        return
    failures.append(f"{label}: {first_difference(c_bytes, rust_bytes)}")


def compare_image(
    failures: list[str],
    label: str,
    c_path: Path,
    rust_path: Path,
) -> None:
    if not c_path.exists() or not rust_path.exists():
        failures.append(f"{label}: missing {c_path if not c_path.exists() else rust_path}")
        return
    compare_bytes(failures, label, c_path.read_bytes(), rust_path)


def compare_word_image(
    failures: list[str],
    label: str,
    c_path: Path,
    rust_path: Path,
) -> None:
    if not c_path.exists():
        failures.append(f"{label}: missing {c_path}")
        return
    c_bytes = b"".join(word.to_bytes(4, "little") for word in read_words(c_path))
    compare_bytes(failures, label, c_bytes, rust_path)


def compare_words(failures: list[str], label: str, c_path: Path, rust_path: Path) -> None:
    if not c_path.exists() or not rust_path.exists():
        failures.append(f"{label}: missing {c_path if not c_path.exists() else rust_path}")
        return
    c_words = read_words(c_path)
    rust_words = read_words(rust_path)
    if len(c_words) != len(rust_words):
        failures.append(
            f"{label}: words C={len(c_words)} Rust={len(rust_words)}"
        )
        return
    for index, (c_word, rust_word) in enumerate(zip(c_words, rust_words)):
        if index in ADDRESS_REGISTERS:
            continue
        if c_word != rust_word:
            failures.append(
                f"{label}: reg[{index}] C={c_word:08x} Rust={rust_word:08x}"
            )
            return


def compare_job(c_dir: Path, rust_dir: Path, job: int) -> list[str]:
    failures: list[str] = []
    compare_word_image(
        failures,
        f"job {job} global model",
        c_dir / f"global_mode_{job}.txt",
        rust_dir / f"global_model_{job:03}.bin",
    )
    compare_word_image(
        failures,
        f"job {job} tile info",
        c_dir / f"tile_info_{job}.txt",
        rust_dir / f"tile_info_{job:03}.bin",
    )
    compare_image(
        failures,
        f"job {job} stream",
        c_dir / f"stream_{job}.txt",
        rust_dir / f"stream_{job:03}.bin",
    )
    compare_word_image(
        failures,
        f"job {job} film grain",
        c_dir / f"film_grain_mem_{job}.txt",
        rust_dir / f"film_grain_mem_{job:03}.bin",
    )
    compare_word_image(
        failures,
        f"job {job} probability",
        c_dir / f"prob_tbl_{job}.txt",
        rust_dir / f"prob_in_{job:03}.bin",
    )
    compare_words(
        failures,
        f"job {job} registers",
        c_dir / f"reg_{job}_in.txt",
        rust_dir / f"reg_{job:03}_in.txt",
    )

    c_offsets = c_dir / f"offsets_{job:03}.txt"
    rust_offsets = rust_dir / f"offsets_{job:03}.txt"
    if c_offsets.exists() and rust_offsets.exists():
        c_map = read_offsets(c_offsets)
        rust_map = read_offsets(rust_offsets)
        if c_map != rust_map:
            differing = sorted(
                register
                for register in c_map.keys() | rust_map.keys()
                if c_map.get(register) != rust_map.get(register)
            )
            failures.append(
                f"job {job} offsets: "
                + ", ".join(
                    f"reg[{register}] C={c_map.get(register)} Rust={rust_map.get(register)}"
                    for register in differing[:8]
                )
            )
    return failures


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("c_dir", type=Path)
    parser.add_argument("rust_dir", type=Path)
    parser.add_argument("--jobs", type=int, default=11)
    args = parser.parse_args()

    failures: list[str] = []
    for job in range(args.jobs):
        job_failures = compare_job(args.c_dir, args.rust_dir, job)
        if job_failures:
            failures.extend(job_failures)
            print(f"FAIL job {job}")
        else:
            print(f"PASS job {job}")

    if failures:
        print("\nFailures:")
        for failure in failures:
            print(f"  {failure}")
        return 1
    print(f"\nPASS {args.jobs} AV1 jobs byte/field-identical outside dma-buf addresses")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
