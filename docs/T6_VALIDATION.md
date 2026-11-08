# t6 validation — 2026-11-08

This is the current integrated validation checkpoint after moving work from
t62 back to the RK3588 reference board.

## 2026-11-08 W5-4 pool closeout

- H.265 lease test passed; the HEVC board suite is **13 PASS / 1 SKIP /
  0 FAIL**. The SKIP remains the named `/tmp/t720.hevc` C-reference absence.
- AV1 lease test passed, 353 AV1 library tests passed, and the 8/10-bit film
  grain hardware output matched libaom.
- VP9 lease test passed, 18 VP9 library tests passed, and all three hardware
  matrices (show-existing, error-resilient segmentation, lossless/tiled/odd
  sizes) passed.

## 2026-11-08 RK3576 / lf1 H.264 bring-up

- Board: Luckfox Omni3576, RK3576, Linux 6.1.75, C MPP 1.0.11.
- 640×480 High profile, 12 moving frames: Rust VDPU383 and stock C MPP both
  SHA-256
  `615ee129c46e10f7fb4cb2cf8af3cb370523f7605abee7e7315e073a4421b112`.
- VDPU383 `global_cfg` and `rps` for the first two frames match the patched C
  dumps word-for-word.
- t6 RK3588 fallback from the same branch: all seven H.264 decoder fixtures
  passed, proving the VDPU34x path remains intact.
- The RK3576 H.264 edge matrix is **5/5** on lf1: 720p/B 30 frames, 1080p
  hard 90 frames, PAFF 15 frames, LTR 62 frames and a 61-AU missing-reference
  stream producing 22 output frames. The first, second, LTR and missing-
  reference rows are byte-identical to C MPP; PAFF matches FFmpeg, with the C
  harness retaining its known field-order difference.

## 2026-11-08 RK3576 / lf1 H.265 bring-up

- The VDPU383 hardware matrix is **12/12** on lf1: 320x240 single IDR, 640x480
  IPPP and B-pyramid, Main10, 2x2 tiles, real WPP at 640x360 and WPP at
  1280x720, JCT-VC DSLICE, explicit SPS/PPS scaling-list streams, a 500-frame
  LTR SPS stream, and a missing-reference stream. Every planar YUV output
  matches stock C MPP byte-for-byte.
- The WPP streams are verified by `trace_headers` with
  `entropy_coding_sync_enabled_flag = 1` and five entry points. The 1280x720
  C/Rust output SHA-256 is
  `5f124bd7c91877106abe010904de7eeeb129d17a09ddde1d0cc06818bbd3b156`.
- JCT-VC DSLICE (50 frames at 1920x1080) hashes to
  `0f28bd89c091b103b3fe3722fa9e8435d96eee414a6c33c29658b4728ec62bc7`.
  Explicit SPS/PPS scaling-list streams hash to
  `ecdaee4973148a40faa39e2dafc322507c5cc2d53e33a02fb08537e7a53668ec`
  and `66baaeb67679a189873f22fb3752caa368d598b72b94d07828f0077a3969e4d0`.
- The 176-byte VDPU383 SPS/PPS/POC packet matches the patched C dump byte-for-
  byte; its offline FNV lock is `0xe97a26406128e428`. RCB offset/length pairs,
  zero CABAC base and `global_len=0xc` are covered by the same path.
- The scaling-list table is 1360 bytes per slot and uses `sps_id * 1360` or
  `(pps_id + 16) * 1360`; the VDPU383 builder stores the six 32x32 matrices in
  the source-defined block-column-major order.
- JCT-VC `LTRPSPS_A_Qualcomm_1.bit` is 416x240 and 500 frames. Its SPS declares
  eight long-term entries and slices alternate `lt_idx_sps=4/7`; C and Rust
  both emit 500 frames with SHA-256
  `e0d7db1516ed31d3046c63c33240ec0866150774eb74b5e88f67bc186ba51bee`.
- The missing-reference row removes the sixth AUD-delimited access unit from a
  12-picture x265 stream. C reports `Could not find ref with POC 5`; C and Rust
  both emit five frames with SHA-256
  `7d67c91340892c5de09e45db43fc160f3991b8f950e30c7d2b0ff5590626a74d`.
- The separate VEPU510 H.265 encode matrix is **15/15**. C's `AUTO_TILE=1`
  produces the same 4710-byte IDR as its ordinary path on RK3576; Rust rejects
  a two-column request before DMA or hardware allocation. Tiles are therefore
  a platform boundary rather than an unimplemented register path.

## 2026-11-08 RK3576 / lf1 VDPU383 AV1 bring-up

- The official C HAL was built with `scripts/av1_vdpu383_dump.patch`; it dumped
  thirteen 672-byte global headers for the ten displayed frames of
  `av1_tiles_320x240_10.ivf`.
- `src/hal/av1d383.rs` reproduces `vdpu38x_av1d_uncomp_hdr()` including the
  C `mpp_put_bits` little-endian 64-bit packing, 5160-bit payload, 128-bit
  alignment and the 16-byte trailing reservation.
- The first keyframe is byte-identical to C: SHA-256
  `5dc26be2b7f1d2a76d135b64435c737cd62d8b6c2ab720a0fb83c83efddfbf56`
  and FNV `0x8da389d2e180d680`.
- The same keyframe's 212-word serialized `Vdpu383RegSet` is also byte-exact:
  control/statistic words, current/error/global/output/CDF/colmv addresses,
  tile strides and all eleven RCB base/length pairs match the C dump.
- The VDPU383 default CDF image is generated from the official 7400-entry u32
  table: 29,600 bytes, SHA-256
  `43474cb59fa76fa1ed95cbfb2fb46952b061e948a13581c04380e51d099c100b`.
  For 320x240, the planned working CDF is 32,672 bytes, segmentation 3,072,
  colmv 20,480 and RCB 72,448.
- `hal::av1d383::Session` submits a real keyframe through the RkvDec client
  with system DMA, CDF/RCB offsets and the 448-byte C output stride.
- Stock C MPP and Rust both emit 115,200-byte 320x240 YUV420p with SHA-256
  `6f5b544f53584c7de6d3acad5f4611ca2ee3d75ae259ab981ffca5e599b1b9fe`.
  The explicit gate is `scripts/board_rk3576_av1_vdpu383_key.sh`.
- Multi-frame reference scheduling, inter pictures, 10-bit output and film
  grain remain open.

- RK3576 H.265 is an initial core-path closure. Random bit-flip injection and
  wider conformance streams still need lf1 evidence.

## 2026-11-08 RK3576 / lf1 VP9 bring-up

- The VDPU383 VP9 matrix is **7/7**: profile 0/2 64x64, 10-bit 64x64,
  lossless 320x240, 10-bit lossless 320x240, error-resilient tiles at
  640x360, and the two adjudicated C-defect rows at 720p and 321x241.
- Regular rows match stock C MPP byte-for-byte. The 720p segmentation and
  321x241 rows match libvpx: C differs on the former and emits one fewer
  frame on the latter.
- VDPU383 uses a separate 168-byte global header, 1312-byte key-frame probe
  FIFO, eleven refined RCB regions and uncompressed `width * height / 2`
  colmv. Tile-width fields are CTU counts. The new
  `scripts/board_rk3576_vp9_vs_mpp.sh` gate passed on lf1.
- The lf1 release LTO compiler currently SIGSEGVs in LLVM during the VP9
  build; the functional matrix uses the debug binary, so no throughput claim
  is made from this run.

## 2026-11-08 RK3576 / lf1 VPU720 JPEG bring-up

- The VPU720 qfactor-50 matrix is **19/19**. NV12 passes at 320x240, 640x480,
  1280x720 and with a 320x240 60-MCU restart; 320x240 NV16, I420, NV21,
  YUV422P, NV61, YUYV, YVYU, UYVY, VYUY, NV24 and YUV444P also match the
  installed C MPP byte-for-byte. YUV400 input and 400/420/422/444 output add
  the last four rows.
- NV12 output sizes are 5,029, 12,028, 28,124 and 5,041 bytes. The 320x240
  4:2:0 variants are 5,029, 4:2:2/packed variants are 5,009, and 4:4:4 variants
  are 4,967 bytes. The Rust path uses client 20, the VPU720 43-word register
  window, a 384-byte transformed Q-table and the hardware status length as a
  byte count.
- `scripts/board_rk3576_jpege_vs_mpp.sh` is the reproducible lf1 gate. A shared
  layout model derives storage size, U/V offsets, chroma stride and UV-swap.
  Tile input remains. C's 422/444 outputs are only 623 bytes for the NV12
  fixture, and Rust matches those bytes. Low-delay is an explicit reject
  because the C VPU720 HAL has null `part_start`/`part_wait`; the reference
  probe spins on zero-length packets. The HAL also exposes neither an RGB input
  case nor a rotation register, so Rust rejects both explicitly.

## Environment

- NanoPC-T6-LTS, RK3588, Debian 11.11, kernel 6.1.57
- `librknnrt` 2.3.2, Rust 1.88.0
- sustained 80% OPP cap enabled: little `performance` @1.416 GHz, big
  `performance` @1.8 GHz (see `docs/t6-power.md`)
- Source worktree synced from the local Mac checkout; `.git`, `refs`, `target`,
  model directories and existing board data were preserved
- `/tmp/mpp-old`, `/tmp/mpp-develop` and `/tmp/mpp_trace.so` restored from
  `~/rk-mpp-oracles` with `scripts/setup_board_refs.sh`

## Tests

`cargo test --release --all-targets` currently enumerates **1207 tests**. The
regular suite passes **1172 / 0 failed / 35 ignored** on t6. The four ignored
JPEG encoder hardware tests relevant to this slice were rerun separately and
pass **4/4** under the current 80% OPP cap. The two JPEG restart/rotate cases
share a temporary input name and therefore need `--test-threads=1`.

Fixes required by the validation:

- `examples/iep_dump.rs` now supplies the new `DeinterlaceJob.pd_mode` field.
- Fixture manifest ignores macOS AppleDouble `._*` transfer metadata.
- VP8 stream-policy tests now assert the board-verified behavior: a torn final
  packet is zero-padded and decoded; a shortened middle packet is submitted and
  marks that picture with a hardware error instead of being classified as a
  dropped parser packet.
- Pooled JPEG workers now close their `MppService` before any shared dma-buf
  lease can return to the pool. The declaration order makes this hold on every
  early `?` path too. This targets the t6 kernel list corruption observed in
  `mpp_dma_find_buffer_fd()`.
- H.264 NV12 and colmv frame storage now uses separate counted pools. IDR and
  geometry changes drop all old reference surfaces before clearing idle cache,
  so the reference-less IDR still receives a fresh mapping. The seven-stream
  H.264 regression, recovery-after-replacement, and a generated 15-frame /
  three-IDR stream all match C MPP byte for byte. A dma-heap test confirms that
  dropping the frame returns `in_use` to zero and the next acquire reuses the
  same fds without another allocation.

The W5-8 recovery gate exposed a stronger t6 driver failure on 2026-11-07.
`RESET_SESSION` followed by an H.264 decode still blocked the whole board even
when a replacement fd was opened before closing the old one. Normal recovery
therefore no longer issues reset: it closes the failed fd first and then opens
and health-checks a fresh session. The reset-only diagnostic passes 32
open/reset/query/drop cycles, but it is now an explicitly destructive example
rather than part of the ignored-test suite. The close-before-open decode gate
passes on the current t6 boot.

The previous board disappearance was not thermal: package readings were
38–39°C. The prior boot log ends with CPU6 soft-lockup and RCU stall while
`rk-mpp-rs` is inside the vendor JPEG driver's
`mpp_dma_find_buffer_fd()`/`mpp_dma_import_fd()` path. After fixing the
service-before-lease lifetime, t6 completed **500 consecutive async demos**
(3000 pooled JPEG jobs, three workers) without a lockup or MPP reset. The
current 80% CPU cap reduces timing/current stress; it does not replace the
lifetime fix and is not claimed as the root-cause fix by itself.

## Board regression

The current `FULL_QP=0 PERF_GATE=1 scripts/board_regress_all.sh` run passed
every suite with `ALL SUITES OK`. The earlier full-QP run passed the entire
24-cell H.264 QP matrix, but exposed stale `/tmp` output files in the HEVC
tile/scaling/LTR generators and an empty C STATUS trace capture. Those harness
bugs were fixed by cleaning root-owned outputs before generation and checking
that the C trace is non-empty; the complete HEVC suite (13 PASS lines) and
STATUS comparison (C 2 / Rust 2, all fields equal) were then rerun green.

- reference setup
- H.264, H.265, MPEG-2, MPEG-4, H.263 and VP8 decode
- H.264 full 24-cell size/profile/QP matrix
- H.264 CBR/VBR/AVBR, debreath and STATUS
- asynchronous framework, AV1 film grain, IEP2, JPEG decode/encode and VP9
  board tests

The first pass exposed a sync-only false failure: board scripts packaged the
local `.worktrees` and `refs` trees into every transfer, so tar returned
nonzero when those large trees changed during the run. All board sync commands
now exclude both; reference trees remain managed by
`scripts/setup_board_refs.sh`. After restoring `/tmp` corpora from
`~/rk-mpp-oracles/corpus`, the H.264 and HEVC decode suites were rerun and every
case passed with no decoder SKIP.

Performance gate:

- H.264 720p: C MPP 121.5 FPS, Rust 283.8 FPS (**+133.7%**)
- 1920×1080→640 RGB scale: 0.5060 ms (limit 0.65)
- 1280×720→640 RGB scale: 1.1100 ms (limit 1.15)
- 640×480→640 RGB scale: 0.4687 ms (limit 0.50)
- 1930×1080→640 RGB scale: 1.4702 ms (limit 1.65)

These are the current 80% OPP medians from 11 interleaved samples, so they are
directly comparable to the current functional and stress results. The older
uncapped-profile peaks are retained only as historical observations.

The H.264 and HEVC `/tmp` corpora were restored from
`~/rk-mpp-oracles/corpus` after reboot. The H.264 decode suite passed 7/7 and
the HEVC suite passed every available large-profile, tile, slice, open-GOP,
Main10, scaling-list and long-term-reference case before the final performance
gate.

The async framework gate also reports per-job queue wait, execution and
turnaround time plus cumulative submitted/done/failed/cancelled counters.
On t6, six JPEG decode jobs across three workers were byte-identical to the
sync path with `submitted=6 done=6 failed=0 cancelled=0`.

## H.264 streaming output

On a 1080p30 High/B=2 stream:

- VUI reorder depth resolves to **2**
- Rust planar output is **byte-identical** to C MPP after NV12→planar
  conversion (SHA-256 `aa049b832da5…dc36c6`)
- Rust packed NV12 is **byte-identical** to C MPP NV12
  (SHA-256 `1c2475b2cb0f…a06408`)
- single-thread streaming decode median is about 0.29–0.44 s for 90 frames,
  versus about 0.88–0.90 s for the C test program

Five-process-boundary sweep, 1080p30:

| path | sustained ≥30 FPS |
| --- | ---: |
| C MPP decode | **28 streams** |
| Rust streaming decode | **26 streams** |

## AV1 distance-weighted compound

The software AV1 reference decoder now covers distance-weighted compound
prediction. The order-hint lookup selects libaom's `fwd_offset`/`bck_offset`;
the blend uses the normative truncating four-bit weighting before the shared
compound round offset and final rounding.

The checked-in fixture
`tests/data/av1_inter_distance_weighted_320x240.ivf` contains one 32x16 and one
8x8 distance block with weights `[11,5]` and `[13,3]`. On t6, all five visible
Y/U/V frames are byte-identical between this decoder and the unmodified libaom
decoder. The targeted unit test, all 12 inter-frame tests, and the complete
fixture manifest pass locally and on t6.

Inter-intra is also complete. Two isolated five-frame fixtures cover 15 smooth
inter-intra blocks and nine wedge inter-intra blocks; together they exercise
all four DC/V/H/Smooth modes plus multiple wedge indices. Every visible Y/U/V
frame is byte-exact against libaom.

OBMC now follows the normative above-first/left-second neighbour scan, reads
`use_obmc_cdf`, keeps switchable interpolation syntax for OBMC blocks, and
blends neighbour predictions with the 1–64 sample masks. The five-frame fixture
contains 32 OBMC blocks and 232 OBMC mipixels; every visible Y/U/V frame is
byte-exact against libaom.

Local warped motion now gathers same-reference neighbour samples, derives the
least-squares affine matrix and shear parameters, and runs libaom's 193-phase
eight-tap warp filter. Its isolated fixture has 10 `WARPED_CAUSAL` blocks,
including zero-MV blocks driven by projected neighbours; all five visible Y/U/V
frames are byte-exact against libaom.

Single-reference global warped motion now derives `GLOBALMV` at each block centre from
`ROTZOOM/AFFINE` matrices and reuses the same warp predictor. A two-frame
isolated ROTZOOM fixture exercises 756 global-warp blocks; both visible Y/U/V
frames are byte-exact against libaom.

Compound global warp keeps independent warp state for both references and
supports distance-weighted and wedge blends with mixed warped/identity inputs.
A four-frame isolated fixture covers three `GLOBAL_GLOBALMV` blocks; all four
visible Y/U/V frames are byte-exact against libaom.

Temporal MV prediction now stores an 8x8 motion field for each reference
picture, projects it through order hints, and merges the projected candidates
between the nearest and outer spatial scans. A three-frame isolated fixture
covers 128 inter blocks; all visible Y/U/V frames are byte-exact against
libaom.

Inter segmentation now carries primary-reference segment maps, spatial and
temporal segment-id prediction, forced reference/skip/global modes, and
per-segment ALT_Q/ALT_LF/lossless state. A three-frame isolated fixture covers
1050 nonzero segment blocks and is byte-exact against libaom on Y/U/V.

Selected tx-size context now uses an inter neighbour's block extent even when
the neighbour is not skipped, while transform-partition context keeps the
separate skip-only override. The previously failing `av1_lr_320x240.ivf`
key+P stream now decodes all 458 footprint blocks (356 inter), and both visible
Y/U/V frames are byte-exact against libaom.

The remaining AV1 software-reference gap is a wider conformance/performance
matrix.

The final `delta_lf_multi` syntax is now represented with libaom's four-row
default CDF. Color frames maintain four independent loop-filter levels;
monochrome frames consume only the two active rows. The generated CDF state is
97 tables / 19,697 values and clones all four rows through SavedCdf. A real
one-frame keyframe, produced by a temporary libaom copy with only
`DEFAULT_DELTA_LF_MULTI=1`, is accepted by the unmodified `aomdec`; the Rust
pre/deblock/CDEF/LR Y/U/V planes are byte-identical to its final YUV. The
patched encoder's second inter frame is invalid, so the fixture deliberately
contains one frame and makes no encoder-stability claim. The complete host
`--all-targets` suite passes. A monochrome counterpart with the same patched
encoder exercises `mono_chrome=1`; its Y plane is byte-identical to `aomdec`.
That fixture exposed and now covers the missing mono UV-syntax gate and the
single-plane deblock grid.

## YOLO video chain

`yolov8s.rknn`, 768×432 H.264 people video, 40 timed frames after 5 warmups,
three interleaved rounds:

The model file was not present under the t6 home after this reboot, so the
end-to-end table below was not regenerated in this validation pass. Both CLIs
were rebuilt successfully; the measurement is the last t6 result from the same
source tree.

| path | median latency | median throughput | detections |
| --- | ---: | ---: | ---: |
| C | 33.608 ms | 29.75 FPS | 3 / 40 frames |
| Rust | **32.823 ms** | **30.47 FPS** | 3 / 40 frames |

Rust is 2.34% faster and the detection statistics match in every round.

## Playback

- C: `mppvideodec → xvimagesink`
- Rust: `h264_stream NV12 → rawvideoparse → xvimagesink`
- A 12-second 1080p30 steady playback completes in 12.15 s for 8 C streams,
  12.25 s for 12 C streams and 12.33 s for 16 C streams.
- Rust completes 8 streams in 12.36 s; 12 streams take 12.90 s and 16 streams
  take 17.27 s.

Thus the current t6 playback guidance is: C up to about 16 near-real-time
streams, Rust up to 8 stable streams with 12 streams borderline. The remaining
Rust playback cost is the user-space NV12 pipe; direct DMABUF scanout is still
the next performance step.

## Post-commit re-verification (2026-11-07)

The batch that had accumulated in the worktree since the 80 % cap checkpoint is
now landed as ten commits (`6f5d644` .. `4691f29`): the H.264 VUI reorder depth
and packed-NV12 helper that `684d9c2` needed in order to build, the
close-before-open session recovery on every submit/poll path, VP8 DPB surfaces
on the shared pool, the concurrent VEPU580 session gate, AV1 reference slices
90-101, the unsafe-contract annotations and C-API overflow guard, the new
bench/probe tools, the sync-tar excludes, and the plan and checklist updates
(top level is now 94 completed / 6 open, 132 / 11 with sub-items).

Re-run against the committed tree:

| gate | result |
|---|---|
| host `cargo test --all-targets --offline` | 1149 enumerated: 1118 passed, 0 failed, 31 ignored |
| `cargo fmt --all --check`, `git diff --check` | clean |
| t6 `cargo test --release --all-targets -- --ignored --test-threads=1` | all 31 board-only tests executed, 0 failed |
| `scripts/board_regress.sh` | H.264 7/7 byte-exact |
| `scripts/board_regress_hevc.sh` | 13 PASS, 1 SKIP (`/tmp/t720.hevc` C reference output absent on this boot) |
| `scripts/board_framework_recovery.sh` | 3/3 (two close-before-open replacements, one VDPU2 damaged-packet gate) |
| `scripts/board_framework_async_vs_sync.sh` | ALL PASS: 6/6 jobs identical, `submitted=6 done=6 failed=0 cancelled=0`, avg wait 2.134 ms, avg run 4.937 ms, 9 pool allocations under a cap of 10 |
| `scripts/board_iep_modes_vs_probe.sh` | 9 PASS, 0 FAIL |

Environment for this run: kernel 6.1.57 with the persistent cap service active
(`policy0` 1,416,000 kHz, `policy4`/`policy6` 1,800,000 kHz). Two
environment-gated debug traces still present in the worktree (palette map
origin in `src/reference/av1/palette.rs`, frame-header dump in
`examples/av1_reference_dump.rs`) were removed before committing, so neither
entered history.

## Multi-tile inter re-verification (2026-11-08)

The 2x2-tile inter fixture now has a non-ignored ten-picture Y/U/V gate.  On
t6 under the persistent 80 % OPP cap, the targeted release test passed
(`1 passed`, `0 failed`) and the complete release library suite passed
(`978 passed`, `0 failed`, `0 ignored`).  The host all-target suite enumerated
1151 tests and finished with `1120 passed`, `0 failed`, `31 ignored`.
