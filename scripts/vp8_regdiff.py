#!/usr/bin/env python3
"""Diff a Rust VP8 register dump against the C HAL's own dump.

    vp8_regdiff.py <c_dump_dir> <our_dump>

The C HAL (`hal_vp8d_debug=4`) appends `reg[N] 0x......` lines per job to
`/data/video/reg_<frame>.bin`; `RFI_VP8_REG_DUMP=1` writes the same words for
every job into one file with a `#` label between pictures. Words that carry a
dma-buf handle are reported separately: those can never agree between two
processes, and everything else must.
"""
import glob
import re
import sys

# Every word the C HAL fills from `mpp_buffer_get_fd`.
FD_WORDS = {61, 63, 64, 131, 136, 137, 140, 141, 142, 143, 144, 145, 146, 147, 149}


def parse_lines(lines):
    words = {}
    for line in lines:
        match = re.match(r"reg\[(\d+)\] +(\S+)", line)
        if match:
            words[int(match.group(1))] = int(match.group(2), 16)
    return words


def parse_ours(path):
    jobs, current = [], {}
    for line in open(path):
        if line.startswith("#"):
            if current:
                jobs.append(current)
            current = {}
        else:
            current.update(parse_lines([line]))
    if current:
        jobs.append(current)
    return jobs


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__)
        return 2
    c_dir, ours_path = sys.argv[1], sys.argv[2]
    paths = sorted(
        glob.glob(f"{c_dir}/reg_*.bin"), key=lambda p: int(re.search(r"_(\d+)", p).group(1))
    )
    c_jobs = [parse_lines(open(path)) for path in paths]
    our_jobs = parse_ours(ours_path)
    print(f"c jobs={len(c_jobs)} rust jobs={len(our_jobs)}")
    failures = 0
    for index in range(min(len(c_jobs), len(our_jobs))):
        c, ours = c_jobs[index], our_jobs[index]
        hard, handles = [], []
        for word in sorted(set(c) | set(ours)):
            c_word, our_word = c.get(word, 0), ours.get(word, 0)
            if c_word == our_word:
                continue
            (handles if word in FD_WORDS else hard).append(
                f"sw{word} c={c_word:#010x} rust={our_word:#010x}"
            )
        if hard:
            failures += 1
            print(f"job {index}: {len(hard)} unexpected differences, {len(handles)} fd words")
            for line in hard:
                print(f"   {line}")
        else:
            print(f"job {index}: identical outside the {len(handles)} dma-buf handle words")
    # `mpi_dec_test` ends a VP8 stream by running an earlier picture again: the
    # extra submissions are byte-identical to one the decoder already made, aside
    # from the buffers attached to it, and they produce no extra output frame.
    # H.263 and MPEG-2 have the same habit, noted in their own regressions.
    for index in range(len(our_jobs), len(c_jobs)):
        repeats = [
            earlier
            for earlier in range(len(our_jobs))
            if [
                word
                for word in set(c_jobs[index]) | set(c_jobs[earlier])
                if c_jobs[index].get(word, 0) != c_jobs[earlier].get(word, 0)
                and word not in FD_WORDS
            ]
            == []
        ]
        if repeats:
            print(f"job {index}: the end-of-stream repeat of job {repeats[0]} in C")
        else:
            failures += 1
            print(f"job {index}: a C submission with no Rust counterpart")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
