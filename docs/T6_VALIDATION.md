# t6 validation — 2026-11-08

This is the current integrated validation checkpoint after moving work from
t62 back to the RK3588 reference board.

## 2026-11-10 encoder packet metadata

- `scripts/board_mpp_packet_meta.sh` compiles the unchanged vendor C probes and
  runs the same binaries once with vendor MPP and once with
  `librk_mpp_rs.so`, both at the persistent 80 % OPP cap.
- H.264 fixed-QP and CBR are **2/2 packets** exact for both a still pair and a
  moving `testsrc2` pair. Stream bytes are identical with SEI disabled.
- H.265 fixed-QP and CBR are also **2/2 packets** exact. The checked fields are
  `lvl64/32/16/8_inter_num`, `lvl32/16/8/4_intra_num`, `KEY_ENC_SSE`,
  `KEY_OUTPUT_PSKIP`, `KEY_OUTPUT_INTRA`, `KEY_ENC_START_QP`,
  `KEY_ENC_AVERAGE_QP`, and `KEY_MOTION_INFO`.
- The H.265 caller-owned motion-info view is 640 bytes for this fixture and its
  per-frame FNV-1a hashes match vendor: `c41b276389c5a183` for the IDR and
  `106c00c09eaed691` for the moving P picture.
- JPEG is **1/1 packet** exact: vendor reports `start_qp=80`, `avg_qp=80` for
  q-factor 80 and no `KEY_OUTPUT_INTRA`; Rust now emits the same metadata and
  byte-identical JPEG. On lf1 the VPU720 reference adds `intra=0`; the split
  formatter writes that field only for VPU720.
- The CBR matrix exposed two existing H.265 rate-control bugs. The model used
  an I-QP delta of 2 instead of the H.265 default 4, and the SEI-disabled path
  built a fixed-QP PPS instead of the rate-controlled `cu_qp_delta_enabled=1`
  form. Both are fixed; CBR stream and decoded YUV are byte-identical.
- The current t6 crate `--all-targets` gate is **1239 passed / 0 failed /
  42 ignored**. `scripts/board_workspace_gate.sh`, which also runs the
  `rk-mpp-rs-sys` tests and strict workspace rustdoc, reports **1243 passed /
  0 failed / 42 ignored**. `cargo fmt --all --check` and `git diff --check`
  are clean.
- The lf1 reduced gate is **1090 library tests passed / 0 failed** plus JPEG
  packet metadata **1/1**. VEPU510 H.264/H.265 fixed-QP moving and still
  matrices are both **2/2** per codec. Built-in CBR/VBR/AVBR are also connected:
  both codecs pass 2-frame metadata and 6-frame stream/YUV/rt_bits matrices.
  The reduced scope avoids LTO-linking 253 independent `--all-targets` test
  binaries one at a time under `CARGO_BUILD_JOBS=1`.
- `scripts/board_sdk_encoder_rc.sh` now passes all six V1.9 H.264/H.265 RC
  combinations on lf1 against the vendor probe. H.264 CBR/VBR/AVBR are 9,426 /
  10,283 / 8,660 bytes; H.265 are 11,192 / 11,795 / 9,676 bytes, all
  byte-identical. The t6 VEPU580 command remains part of the same script.

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
- The complete fixture now decodes all ten visible frames through VDPU383.
  The Rust sequence models persistent output, colmv and working-CDF buffers for
  all eight reference slots, including hidden pictures and
  `show_existing_frame`. Its primary-reference coefficient-CDF state is
  inherited exactly like C; the older current-frame `base_q_idx` shortcut
  caused the first state divergence on the fourth coded job.
- C and Rust both emit 1,152,000 bytes of 320x240 YUV420p. Frame-by-frame and
  whole-sequence comparison is byte-identical, with SHA-256
  `9d339ae3c5b270ac9059548da0d3ff5ef6cb0c94a9bcb11f5bfc93783e868138`.
  The explicit full-sequence gate is
  `scripts/board_rk3576_av1_vdpu383_sequence.sh`.
- VDPU383 10-bit output is now closed for the current C-compatible path.
  The 320x240 surface uses MPP's 704-byte packed stride and a 337,920-byte
  allocation; Rust converts it to planar little-endian u16 Y/U/V. All ten
  displayed frames total 2,304,000 bytes with SHA-256
  `f3b69b61a06d3967a89b91f11f56883588df53974d4a1b5efb8fbd9e004d49b8`,
  byte-identical to C's skip-grain output. The explicit gate is
  `scripts/board_rk3576_av1_vdpu383_10bit.sh`.
- VDPU383 FGS is a separate unresolved hardware boundary. C's VDPU383 HAL
  leaves `av1_fgs_en` and `reg134_fgs_base` clear. Enabling either raster or
  tile FGS in Rust makes the job complete without STATUS (`0x00000000`) and
  forces a decoder reset; keep the verified skip-grain path enabled until a
  register/trace oracle exists for this block.

- RK3576 H.265 now includes a 16-cell deterministic corruption matrix:
  SPS/PPS/SEI flips, start/middle/end flips in the IDR and selected P slices,
  and three final-AU truncation lengths. Fourteen cells have identical C/Rust
  output counts and byte-identical planar YUV when frames remain visible.
  A damaged SPS and a flip at the first IDR slice payload make C's
  `mpi_dec_test` wait until timeout; Rust returns promptly with zero output.
  The reproducible gate is
  `scripts/board_rk3576_h265_corrupt_vs_mpp.sh`.

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

`cargo test --release --all-targets` currently enumerates **1225 tests**. The
regular suite passes **1188 / 0 failed / 37 ignored** on t6. The four ignored
JPEG encoder hardware tests relevant to this slice were rerun separately and
pass **4/4** under the current 80% OPP cap. The two JPEG restart/rotate cases
share a temporary input name and therefore need `--test-threads=1`.

The H.264/MJPEG public SDK gate also passes on t6: C and C++ consumers link
against both the shared and static libraries, every exported API symbol is
present, lifecycle/error checks pass, and H.264 (7/127/313-byte chunks and
whole-stream input) plus MJPEG (97/211-byte chunks and four JPEGs in one packet)
match the board C MPP byte for byte in NV12 and I420.

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

W5-8 gained the missing real-STATUS combination on 2026-11-11. A positive but
unmapped input fd drives an H.265 320x240 auto-tile session through the real
submit path; t6 completes the job without `enc_done` (`int_sta=0x00000000`),
so the service closes and reopens the client fd. The encoder then zeroes the
tile line buffers, invalidates the warm-up marker, and makes the next picture
a parameter-set IDR instead of a P frame against an unknown reference epoch.
That recovered IDR is byte-identical to a newly opened encoder. The complete
`scripts/board_framework_recovery.sh` gate is **4/4**: two session replacement
checks, the H.265 tile recovery case, and the real VP8 damaged-packet STATUS
case.

W5-6 cross-session CCU submission is now board-verified as well. The t6 device
tree exposes vdec/venc/jpeg `rockchip,ccu` topology, and `BatchServer` emits one
`SET_SESSION_FD` chain for multiple real session fds. The kernel source's
`mpp_collect_msgs()` ends a task at the next delimiter and accepts `LAST_MSG`
only on the final task, so internal tasks are explicitly kept non-final. This
6.1 driver triggers both sessions but does not fill every `mpp_bat_msg.ret`;
the Rust path therefore polls each real session after the shared submit.
`tests/framework_ccu_hw.rs` opens two independent VEPU2 JPEG sessions and
encodes different 640x480 NV12 pictures through one batch. Both JPEG streams
match standalone references byte for byte. `scripts/board_framework_ccu.sh`
passes **3/3** repeated runs. The same script selects lf1's VEPU510 path; two
independent 320x240 fixed-QP IDR sessions also match their standalone streams
and pass **3/3**.

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

## Official `mpi_dec_test` compatibility

`scripts/board_mpp_official_dec_test.sh` runs the vendor `mpi_dec_test`
executable without modifying or relinking the test source. `LD_PRELOAD` routes
`mpp_create/init/destroy`, packet/frame/buffer objects and
`decode_put_packet/decode_get_frame` to rk-mpp-rs, while the native library
continues to provide its test-only log, cfg and OSAL helpers.

| case | t6 | lf1 |
|---|---|---|
| H.264 640x480, 6 B-frame pictures | PASS, byte-identical | PASS, byte-identical |
| H.265 320x240, 1 picture | PASS, byte-identical | PASS, byte-identical |
| MJPEG 320x240, 1 picture | PASS, byte-identical | PASS, byte-identical |

The compatibility layer now implements the local `MppMeta` surface used by
the official object API: s32/s64/ptr, frame/packet/buffer, default getters,
duplicate/ref/lifetime and deep copies for `KEY_USER_DATA`/
`KEY_USER_DATAS`. Advanced decode consumes packet `KEY_OUTPUT_FRAME` and
publishes `KEY_INPUT_PACKET` on the returned frame, so the preallocated frame
identity remains unchanged. `scripts/board_mpp_compat_meta.sh` passes on t6
and lf1; on t6 the unchanged native `mpp_meta_test` also passes with two
threads and ten loops. A returned frame-identity mismatch is a hard test
failure. This is a real official decoder-tool gate. The VEPU580
H.264/H.265 encoder packet kernel metadata was subsequently closed on
2026-11-10 for VEPU580 and VEPU510. VEPU510 built-in CBR/VBR/AVBR and
user-defined streaming RC callbacks both use the same per-frame session.

`scripts/board_mpp_official_enc_test.sh` now exercises the same bridge for the
vendor encoder tool. It reads `MppEncCfg` from the vendor helper object, submits
the test's NV12 `MppFrame`, and drains owned `MppPacket` objects:

The 2024 t6 harness is run with `ENC_GOP_SPEC=60` to preserve its captured
reference schedule. The 2025 lf1 harness uses the script default `0:60`, which
sets an ordinary 60-picture GOP without the absent smart-GOP ref config.

- H.265 320x240 one picture: byte-identical, SHA
  `1f00679ad21815e45c08f1fedd2873d4604cc654ccb131a6ce44ce6fd352e84a`.
- H.265 320x240 six pictures: byte-identical, SHA
  `e763e3577eff87ec4154438de2a761e35affeba3d92e897ae2742f884af03462`.
- MJPEG 320x240 one picture: byte-identical, SHA
  `08eb7c6f097f70e9e5424111a83dddef4d7cc46fc01d2772b42dc565b5388623`.
- H.264 320x240 one picture: both streams decode to identical YUV, SHA
  `9d8c6f546089d1564f8d5c0401d652dfa4c0e9b24a634dbb7aa33722324ac6fc`.
- H.264 320x240 six pictures: all SPS/PPS, rc-SEI and VCL NAL units match C
  MPP; all six decoded YUV pictures are byte-identical. Only the version-SEI
  build text differs, which the stream comparator checks for framing only.

On lf1 the unchanged official encoder tool now also passes all exposed codecs:
H.264 High one picture and a real six-picture stream compare identical NAL
sequences and decoded reconstruction (version-SEI framing only); H.265 one and
six pictures are byte-identical (`8c693d15…1efa97a7`,
`cf6b5f6a…eba2e5fd`); MJPEG is byte-identical (`93d7ac00…eab945e`). The RK3576
object path uses its own CABAC/8x8 and H.265 register templates plus a MEIW
buffer; it does not reuse the direct CLI's 4710-byte H.265 prefix format.
With `CHECK_MOTION_INFO=1`, the script also compares the caller-owned
`KEY_MOTION_INFO` buffer: H.264's 1200 bytes are byte-identical
(`655a3ef0…5955273`) and H.265's 1280 bytes are byte-identical
(`bfe492ba…09f462`).

The compatibility C consumer now also covers external buffer ownership:
caller-pointer `mpp_buffer_import`, external-group `commit/get`, fd-only import
through a memfd-backed mapping, `MppBufferInfo` round-trip, index propagation,
and pointer/read/write behavior. Internal buffers now carry a real dma-heap fd
and are recycled from the group's unused list; usage and count limits are
updated on actual allocations. These checks run as part of
`board_mpp_compat_data.sh` on lf1.

H.264/H.265/VP9/AV1/MJPEG compatibility decoders now request DMA-BUF SDK
frames. Advanced `KEY_OUTPUT_FRAME` fills the caller frame directly from the
decoder's mapped planes instead of allocating an intermediate frame/buffer and
copying twice. The unchanged `mpi_dec_test` suite still matches native MPP
byte-for-byte for H.264, H.265 and MJPEG on both boards.

H.264/H.265/VP9 and MJPEG also accept the caller frame's dma-buf as the next
hardware output surface. On lf1, the focused C advanced test verifies frame and
buffer identity plus byte-identical H.265/MJPEG output; H.264 640x480 six-frame
advanced output is byte-identical to the native C golden
(`338214ea…374b815`). The VP9 consumer now splits IVF packets one frame per
submit, keeps output surfaces alive across the reference DPB, and reserves the
caller frame only for `show_frame`. Its direct DMA-BUF path preserves the
256-odd hardware stride and 9/5 frame allocation. On both t6 and lf1, all 12
frames of `vp9_p0_lossless_320x240_12.ivf` match C MPP byte for byte
(`c5381bbf…3949a`). AV1 uses the same packet-level output contract: VDPU34x
imports the caller buffer into its display pool, while VDPU383 replaces the
frame's output lease without disturbing colmv/CDF state. Coded `show_frame`
pictures write that surface directly; `show_existing` copies from the historical
display surface because its backing reference was decoded in an earlier packet.
All 10 frames of `av1_320x240_10.ivf` match C MPP on both boards
(`88f34ea5…c5f8e`). The shared buffer pool now reuses Empty slots after
`clear()`, so repeated injection no longer hits a false `Full` at the pool's
historical slot count.

Performance gate:

- H.264 720p: C MPP 121.5 FPS, Rust 283.8 FPS (**+133.7%**)
- 1920×1080→640 RGB scale: 0.5060 ms (limit 0.65)
- 1280×720→640 RGB scale: 1.1100 ms (limit 1.15)
- 640×480→640 RGB scale: 0.4687 ms (limit 0.50)
- 1930×1080→640 RGB scale: 1.4702 ms (limit 1.65)

Sustained-motion and concurrent decode evidence uses a generated
`testsrc2` 1280x720/30 fps stream with actual inter-frame motion. H.264,
H.265 and VP9 are 120 frames; AV1 is 60 frames. Single-stream figures are the
median of nine interleaved C/Rust samples at the 80% OPP cap:

| codec | C MPP | Rust | Rust delta |
|---|---:|---:|---:|
| H.264 | 209.8 FPS | 378.0 FPS | +80.2% |
| H.265 | 228.1 FPS | 292.8 FPS | +28.4% |
| VP9 | 207.6 FPS | 244.2 FPS | +17.6% |
| AV1 | 201.3 FPS | 310.7 FPS | +54.3% |

The first concurrent matrix exposed the real regression: H.264 with planar
I420 conversion reached 413.8 FPS at one process, fell to 262.0 FPS/process at
four and 210.5 at six, making aggregate Rust throughput 10–11% slower than C.
The DMA-BUF frame path removes the per-frame CPU NV12 copy, and the H.264
decoder now skips even the redundant mapping and `DMA_BUF_SYNC` when it will
not read pixels. `board_multistream_bench.sh` then measured:

| streams | C aggregate | Rust DMA aggregate | Rust per stream | delta |
|---:|---:|---:|---:|---:|
| 1 | 186.0 FPS | 381.0 FPS | 381.0 FPS | +104.8% |
| 2 | 582.5 FPS | 854.1 FPS | 427.0 FPS | +46.6% |
| 4 | 1151.1 FPS | 1240.3 FPS | 310.1 FPS | +7.8% |
| 6 | 1541.8 FPS | 1525.4 FPS | 254.2 FPS | -1.1% |
| 8 | 1550.9 FPS | 1540.9 FPS | 192.6 FPS | -0.6% |
| 12 | 1621.6 FPS | 1629.0 FPS | 135.7 FPS | +0.5% |
| 16 | 1679.8 FPS | 1606.7 FPS | 100.4 FPS | -4.4% |
| 24 | 1696.1 FPS | 1577.2 FPS | 65.7 FPS | -7.0% |

Thus 24 independent 720p playback processes were measured above real-time
simultaneously. Extrapolating the measured 1.58k aggregate ceiling gives about
52 streams at 30 fps, but that is a throughput estimate rather than a
process-count claim. At 6+ streams both implementations are dominated by the
shared rkvdec service ceiling; the Rust-specific disadvantage is gone once
pixel conversion is removed.

Standalone drop-in decode is also closed. `scripts/board_mpp_standalone.sh`
creates an overlay directory whose only `librockchip_mpp.so.1` is a symlink to
`librk_mpp_rs.so`, verifies the unchanged vendor `mpi_dec_test` resolves that
symlink through `ldd`, and compares its output against a second run using the
real vendor library. On both t6 and lf1 all five cases match byte-for-byte:
H.264 640x480/6 frames (`338214ea…f815`), H.265 (`fcedb435…05e0`), VP9
12 frames (`c5381bbf…3949a`), AV1 10 frames (`88f34ea5…c5f8e`) and MJPEG
(`98c5f9cc…a689f8`). The local shim covers log/data symbols, environment and
time services, OSAL allocation, SoC name/type, spinlocks, `MppDecCfg`,
singleton/trie metadata and the kmpp/cfg startup objects. Logging currently
uses Rust naked ABI trampolines to preserve the variadic registers before
tail-calling a local C `vfprintf` helper. The board regression requires the
unchanged decoder log to contain the actual `320x240` and coding type rather
than unexpanded `%d` placeholders.

`scripts/board_mpp_standalone_encode.sh` exercises the same replacement on
the encoder side. The unchanged `mpi_enc_test` resolves `librockchip_mpp.so.1`
to `librk_mpp_rs.so`, creates a local key/value `MppEncCfg`, and runs
`MPP_ENC_SET_CFG` through the existing encoder bridge. On t6, H.264 and H.265
standalone streams decode byte-identically to the vendor streams
(`9d8c6f54…24ac6fc` and `c7c74e0c…eb03643`), while MJPEG is stream-identical
(`08eb7c6f…b5388623`). This closes the one-frame object path; reference-cfg
objects also survive create/reset/LT/ST add/check and the H.264 case uses
`-g 1:60`.
The legacy utility does not call `MPP_ENC_SET_REF_CFG` directly on the
multi-frame path: it stores the object at `"rc:ref_cfg"` inside `MppEncCfg`.
The bridge now reads that pointer before constructing the encoder and maps the
three TSVC2 short-term entries into the VEPU580 schedule. A six-frame
`-g 1:60` run produces the vendor alternation of disposable and reference P
pictures (`nal_ref_idc 0/2/0/2/0`), repeats the type-14 prefix exactly, shares
the reference picture's frame_num as the C DPB does, advances POC independently,
and changes the non-reference slot cycle as required. All 16 NAL units match
the vendor stream except the intentionally different version-SEI text; both
six-frame outputs decode to the same 320x240 YUV, SHA-256
`fec69971fb789cf8cf3e2798f85ccbdd01b7e22d532621114e036e672864f0a4`
(`691200` bytes). Extending the same fixture to 12 frames exercises a second
slot cycle; all 28 NAL units still match under the version-SEI rule and the
1,382,400-byte YUV output has SHA-256
`d9a3ed5528cc4b0dc5895741e12d7092eb37d47a024aaab3417dce7aecad7e63`.
TSVC3 uses four repeating short-term steps with temporal ids `2, 1, 2, 0`.
The temporal-zero step walks back to the previous temporal-zero reference,
which produces a short-term list reorder at the following reference picture.
Over the same 12-frame fixture its 28 NAL units match and the YUV SHA-256 is
`7794eb885e695c780f9e98c979677f74bac09a45f80b1deefc7f733f7488f8e2`.
TSVC4 adds a one-frame long-term reference. The opening IDR sets
`long_term_reference_flag`, frame 8 replaces LT0 with MMCO6, and later pictures
reorder the P list to `long_term_pic_num=0`. Its 12-frame stream again has 28
matching NAL units and decoded YUV SHA-256
`33923dd26ff83f4d2d59d380b372614875bd2e7f58d281f5bce062eb8db1186a`.
A 64-frame run crosses `gop=60`: the second IDR re-emits SPS/PPS and both SEIs,
the reference schedule clears, and all 136 NAL units match. Its 7,372,800-byte
YUV output has SHA-256
`a3ad1bc65ac3ee66c2014ef3e2075722c5dc7aec77b29ec6568155f0f35a0593`.
`MPP_ENC_SET_IDR_FRAME` is now executable rather than a no-op. With forced IDRs
before input frames 2 and 4, `HEADER_MODE_EACH_IDR` enabled and SEI disabled,
the official-path probe emits three IDR sections, four pictures each
(`12` NALs total). t6 and lf1 both match the C stream and decode to the same
691,200-byte YUV (`05901761…f71159` on t6). The H.265 VEPU580 probe runs the
same schedule and matches 15 NALs; its YUV SHA-256 is
`b2765277b355c52eae77bae10997ec4284c03b20d76f2b1cadfeacf0f3c56607`.
POC restarts at 1 after each forced IDR. Other encoder controls remain open.

On lf1, VEPU510 now executes the same controls. The MPP object path matches
the vendor's complete 15-NAL stream and 691,200-byte decoded YUV
(`7f3d4664…841231`). `MPP_ENC_GET_HDR_SYNC` now selects the direct
`h265e_probe` template or the `mpi_enc_test` template exactly as the caller
expects. The board gate also covers both caller motion-info capacities: 640
bytes for the probe and 1280 bytes for unchanged `mpi_enc_test`, and the
corrected `kmpp_objdef_get/register` ABI lets the preload path initialize
without a segfault.

The RC API control-plane probe matches vendor output on t6 and lf1. Both list
the same six built-in strategies (`default` for AVC/HEVC/MJPEG/VP8 and `smart`
for AVC/HEVC), filter AVC to two entries, report the initial
`default,type=0` selection, append a registered `test_h264e_rc`, select and
restore it by `GET/SET_RC_API_CURRENT`. A user-defined strategy can now drive
H.264/H.265 frames through the complete callback lifecycle.
The direct `rc_api_add`, `rc_brief_get_all` and `rc_brief_get_by_type` exports
are compared in a separate process and match vendor output as well.

`board_mpp_custom_rc.sh` registers a C-defined strategy and runs the complete
callback lifecycle through both encoders. The matrix covers H.264/H.265 x
fixed-QP/CBR/VBR/AVBR on t6 VEPU580 and lf1 VEPU510, 8/8 on each board,
plus same-frame-type and frame-type-changing reencode, 12/12 total.
On t6, H.264 fixed is YUV `6d1e7dc9…b90cd3`; H.265 fixed is
`53bf281b…ad9789e`. Their CBR/VBR/AVBR outputs are respectively
`3f34100a…f7023e` and `cad3894e…a2384e`. On lf1, the corresponding hashes
are H.264 `955dc593…f49a71` / `54d0d202…bda0` and H.265
`c27657d4…e67e` / `39c1fc34…fe8b`. Streams, decoded YUV, init configuration,
full callback field lines and hardware `bit_real/quality_real` all match the
ABI-compatible vendor library. The same-frame-type reencode case requests a
second attempt at QP30 from `check_reenc`: t6 outputs H.264 YUV `60fe9815…8ad0` and H.265
`f618041a…1246f`; lf1 outputs H.264 `771a0735…fee70` and H.265
`9fd432ba…8171`. The first hardware packet is discarded, while the same
input/reference surfaces are rerun before `frame_end`.
The frame-type-changing case starts from a P picture and requests a non-IDR
intra/IDR update. H.265 matches the vendor's hybrid P-syntax/INTRA-config/IDR-NAL
template, but that stream is a C-BUG: FFmpeg emits the first two frames
(t6 YUV `fffd339a…009d`, lf1 `ba4b21e9…5343`) and then stops with
`Could not find ref with POC 1`; the full three-frame stream remains
byte-identical to vendor. H.264 preserves the vendor's actual P slice while
the callback state reports the requested type and decodes all three frames:
t6 YUV `d2b11845…d4ac`, lf1 `0191b6b5…dc56`. Combining reencode with drop/PSKIP
remains outside this verified matrix.

`board_mpp_osd.sh` uses unchanged `mpi_enc_test` with `osd_enable=1`, a custom
256-color palette and eight index regions. H.264 matches the vendor stream and
decoded YUV byte-for-byte (`1055ca00…3b22a`). H.265 now does the same:
stream SHA `625d464e…efc838` and YUV `6f44da15…3cbd310` match exactly. The
root cause was not palette or buffer content, but H.265 replacing its offset
list after OSD setup and dropping `reg3093..3099`; appending preserves all
seven OSD offsets. A 6,144-byte buffer dump still confirms the eight expected
`0..7` index regions.

These are the current 80% OPP medians from 11 interleaved samples, so they are
directly comparable to the current functional and stress results. The older
uncapped-profile peaks are retained only as historical observations.

### H.264 file-input DMA pipeline

The supplied RK3588 `mpi_enc_test` result used MPP's built-in moving
`fill_image()` input. Reproducing that class of work at 1280x720, High profile,
QP26, GOP60 and 500 pictures under the persistent 80% OPP cap gives the
following 11-sample medians:

| path | median | throughput | vs C whole path |
|---|---:|---:|---:|
| C MPP built-in fill, core 6 | 1222 ms | 408.97 FPS | baseline |
| Rust mmap file input, core 6 | 1306.909 ms | 382.58 FPS | -6.4% |
| Rust two-surface DMA pipeline, cores 6+7 | 999.532 ms | 500.23 FPS | +22.3% |

The single-core Rust path is therefore still slightly behind C's in-memory
fill loop, but the complete file-input path is not: the old CLI path measured
about 354 FPS on the same 500-picture fixture. Two changes removed most of the
remaining software gap:

1. a regular input file is mapped and copied directly into a
   `system-uncached` DMA surface, avoiding the kernel's `read` /
   `copy_to_user` path;
2. on a multi-core affinity, a reader thread fills the next preallocated DMA
   surface while the encoder core drives VEPU580, so input movement overlaps
   the hardware job instead of extending it.

The second number deliberately uses two cores. It is a whole-pipeline result,
not a claim that VEPU580 itself became 22% faster. Both Rust modes produced
the same 2,995,229-byte bitstream with SHA-256
`0a039e02910169e9900349a16e676f580c3da9e265b154d15ebcbde3a2bc6a2e`.
The 691,200,000-byte input fixture is the byte-exact C `fill_image()` output
with SHA-256
`db3d065df8ea78f9068aa397dfc6ed3bcd404d27df85b772c307d58aff556546`.
`scripts/board_t6_h264_encode_pipeline.sh` regenerates the fixture from
`tools/mpp_nv12_fill.c`, checks both hashes and repeats the interleaved
benchmark.

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
| `scripts/board_framework_recovery.sh` | 4/4 (two close-before-open replacements, H.265 tile recovery after real `int_sta=0`, one VDPU2 damaged-packet gate) |
| `scripts/board_framework_async_vs_sync.sh` | ALL PASS: 6/6 jobs identical, `submitted=6 done=6 failed=0 cancelled=0`, avg wait 2.134 ms, avg run 4.937 ms, 9 pool allocations under a cap of 10 |
| `scripts/board_iep_modes_vs_probe.sh` | 9 PASS, 0 FAIL |
| `scripts/board_mpp_packet_meta.sh` | t6 VEPU580 H.264/H.265 fixed-QP/CBR still+moving 8/8; lf1 VEPU510 fixed-QP/CBR/VBR/AVBR 2-frame matrix plus 6-frame RC stream/YUV; JPEG 2/2; H.265 motion-info hashes exact |

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
