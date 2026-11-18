# Contributing

Thanks for looking at `rk-mpp-rs`. This project re-implements the Rockchip
media stack in Rust and compares every behaviour it claims against a C MPP
build on a real board.

## Before you start

- Read [`docs/CAPABILITY.md`](docs/CAPABILITY.md) for what is proven and on
  which board, and [`docs/DIVERGENCES.md`](docs/DIVERGENCES.md) for known
  differences from the C library.
- Host-side code must compile and test on x86_64 Linux/macOS:
  `cargo test --workspace --all-targets`.
- Anything that touches `/dev/mpp_service` is a **board test**. Mark it
  `#[ignore]` so CI and contributors without hardware can still run the host
  suite.

## Evidence rules

The project's core rule: a capability is "done" only when it is byte-exact (or
pixel-exact) against the C reference, or against an independent decoder when
the C side is proven wrong.

- Prefer a `scripts/board_*.sh` gate that compares the Rust output with the C
  oracle (and with libaom/libvpx/FFmpeg where applicable).
- Record the result and its boundary in `ROADMAP.md` and, when it changes a
  known difference, `docs/DIVERGENCES.md`.
- Do not soften a gate to make a change pass.

## Performance claims

- Pin the CPU and DDR governors.
- Interleave the two implementations sample by sample and report medians.
- Verify both sides produce identical output **before** timing them.
- One sample is never evidence; state the stream, resolution and frame count.

## Pull requests

- One logical change per PR, with the command(s) that prove it.
- `cargo fmt --all -- --check` and the host test suite must pass.
- Board-only changes: include the board script output and the board/kernel
  version.

## Commit messages

Explain what changed, why, and the evidence. Existing history is a good
template: `git log --oneline`.
