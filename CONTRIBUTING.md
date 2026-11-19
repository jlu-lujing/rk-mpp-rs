# Contributing

Thanks for looking at `rk-mpp-rs`. This project re-implements the Rockchip
media stack in Rust and compares every claimed behaviour against C MPP on a
real board.

## Before you start

- Read [`docs/CAPABILITY.md`](docs/CAPABILITY.md) for proven behaviour and
  board boundaries.
- Read [`docs/DIVERGENCES.md`](docs/DIVERGENCES.md) before changing a result
  that intentionally differs from C MPP.
- Keep host code portable. Every push and pull request runs the workflow in
  `.github/workflows/ci.yml`:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets
cargo test --workspace --all-targets
```

Tests that open `/dev/mpp_service`, dma-heap devices or Rockchip hardware must
remain `#[ignore]` and move their evidence into a board script.

## Evidence rules

A capability is complete only when it is byte- or pixel-exact against the C
reference, or against an independent decoder when C is separately proven
wrong.

- Prefer a `scripts/board_*.sh` gate that compares Rust with the C oracle.
- Record the result and its board/kernel boundary in `ROADMAP.md`.
- Do not weaken a regression gate to make a change pass.
- Preserve output bytes when doing performance work.

## Performance claims

- Pin the CPU and DDR governors; t6 measurements use the 80% OPP cap.
- Verify both implementations produce identical output before timing them.
- Interleave samples from both implementations and report the median.
- State stream, resolution, frame count, board and kernel.

## Pull requests

- Keep one logical change per pull request.
- Include the exact commands and evidence used to verify it.
- Update `README.md`, `ROADMAP.md`, `docs/CAPABILITY.md` and
  `docs/HANDOFF.md` when a verified capability or public API changes.
- Board-only changes must include the board script output and board/kernel
  version.
