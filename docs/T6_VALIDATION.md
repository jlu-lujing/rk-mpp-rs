# t6 validation — 2026-11-12

This is the current integrated validation checkpoint after moving work from
t62 back to the RK3588 reference board.

## 2026-11-12 VDPP2 register bring-up

- t6 `vdpp-probe` confirms client 29 is absent: hardware bit 29 is false,
  `INIT_CLIENT_TYPE` returns `EINVAL`, and neither register offsets nor polling
  are available.
- RK3576 VDPP2 parameter generation is now hardware-independent in
  `src/hal/vdpp2.rs`. It carries the vendor common register layout, ZME scale
  factors, all four 17x8 coefficient banks, DCI histogram words, and validated
  geometry limits.
- `scripts/board_vdpp2_regs_vs_c.sh` runs the unchanged C builder under a
  fake `mpp_service` ioctl shim on t6. Eleven cells pass: 720x480 to 1920x1080
  YUV444, 1920x1080 to 640x640 YUV420, 640x480 identity, histogram mode, and
  full default enhancement (DMSR/ES/sharp), a custom DMSR/ES profile and a
  custom SHARP profile, YUV420/YUV444 split Y/UV output and a custom ZME
  dering/blend profile, plus a custom 11x17x8 tap8/tap6 coefficient bank that
  replaces both vendor lookup tables. All compared register sections are
  identical.
- `src/service/vdpp2_session.rs` owns source/destination/optional chroma/histogram
  DMA-BUFs, submits the nine VDPP2 register sections plus address offsets, polls
  completion, and combines or exposes split Y/UV output. The packed DCI buffer is
  decoded into the vendor 4096-local-bin / 256-global-bin layout, and the U10
  mean-luma helper follows `get_avg_luma_from_hist()`.
- The targeted t6 library gate for VDPP2 is **21 passed / 0 failed / 0 ignored**
  after adding exact 9-byte local-bin packing, little-endian global bins, mean
  luma, truncated-input, legal histogram geometry, hardware-status, default
  DMSR/ES/SHARP golden-trace, ZME configurability, custom tap-bank selection,
  custom parameter-packing and split-output geometry tests. `vdpp-scale` and
  `vdpp-hist` are ready for hardware validation on an RK3576 boot image that
  actually binds client 29.
- The VDPP2 hardware assets also compile on t6: the C probe builds against the
  2024-04 C source/library, the RK3576 overlay compiles to a 267-byte DTBO, and
  the independent packed-histogram checker passes synthetic
  `4096 local / 256 global / mean=511` data. `board_vdpp2_hw_vs_c.sh` also has
  separate Y/UV comparisons for split YUV420 and YUV444, and correctly reports
  SKIP when the target board does not expose client 29.
- The lf1 DTB has `/vdpp@27961000` status `okay`, but the running kernel has
  `# CONFIG_ROCKCHIP_MPP_VDPP is not set` and `# CONFIG_OF_OVERLAY is not set`.
  No platform device or driver binds, `probe_hw_support` bit 29 is clear, and
  `INIT_CLIENT_TYPE` returns `EINVAL`; the hardware gate therefore reports
  SKIP rather than claiming pixel or histogram validation. A rebuilt kernel
  containing `mpp_vdpp` is required.

### lf1 kernel enablement investigation

- `mpp_vdpp` is a built-in `bool` driver in the Rockchip Kconfig, not a loadable
  module, and it calls non-exported MPP core symbols. A standalone
  `mpp_vdpp.ko` is therefore not a valid delivery path.
- The current lf1 boot partition (`mmcblk2p3`) was captured read-only as a
  64 MiB unsigned U-Boot FIT. It contains separate `fdt`, `kernel` and
  `resource` images; the FDT and resource can be preserved byte-for-byte.
- The board config identifies Linux **6.1.75**. The stock Rockchip 6.1.75
  snapshot predates RK3576, and the current Rockchip branch reports 6.1.118.
  The Armbian-maintained Rockchip `rk-6.1-rkr3` commit
  `00b325014004cf22a27fc67762fe2ff09163cec3` has the required combination:
  Linux 6.1.75, RK3576 DTS, and `rockchip,vdpp-rk3576`.
- `scripts/build_lf1_vdpp2_kernel.sh` now requires an exact 6.1.75/RK3576 source
  match and rejects both mismatched releases and kernels without VDPP. It built
  and validated an unsigned replacement FIT without writing to the board:
  **42,991,057 bytes**, SHA-256
  `afe90fcc8c5dbc4223321158021ee860e233b770ae8d6da5c37bcb86c548f37a`.
  The new `Image` is 41,036,288 bytes, only 34,816 bytes larger than the running
  kernel; its SHA-256 is
  `ee4141a244ddaa4f7baeda1bc7c760f0acac429e63ae4eed3f98c058d1b48300`.
  `CONFIG_ROCKCHIP_MPP_VDPP=y` is embedded and vmlinux exports
  `rockchip_vdpp_driver` with `vdpp_probe`/`vdpp_remove`. The original FDT and
  resource images are byte-identical, and the FIT preserves `rollback-index=0`.
- lf1 remains on its original boot image. The replacement is staged locally
  under `target/lf1-vdpp2/`; booting it requires explicitly approved writes to
  `mmcblk2p3`. Enabling a configfs overlay cannot fix the running kernel because
  both `CONFIG_ROCKCHIP_MPP_VDPP` and `CONFIG_OF_OVERLAY` are unset.
- `scripts/flash_lf1_vdpp2_boot.sh` defaults to a dry run. Its applied path
  verifies kernel 6.1.75, VDPP DTB status, p3 capacity, the local image hash and
  passwordless sudo; backs up p3; writes with `dd conv=fsync`; reads the exact
  image span back and verifies SHA-256 before an optional reboot. A dry run
  against lf1 passes with the staged 42,991,057-byte image.

### 2026-11-13 first lf1 VDPP boot attempt

- With explicit authorization, the staged FIT was written to `mmcblk2p3`.
  The write completed in 0.39 seconds and readback matched SHA-256
  `afe90fcc8c5dbc4223321158021ee860e233b770ae8d6da5c37bcb86c548f37a`.
- The original p3 was backed up before writing as
  `target/lf1-vdpp2/lf1-p3-backup-20261113-014828.img`, 64 MiB, SHA-256
  `11ccf48e1bb04cdf21318d72ce29da57123c6c70d5139d2ca711d2a553317d30`.
- lf1 rebooted but did not return on `192.0.2.148` within 180 seconds. A full
  `192.0.2.0/24` ICMP sweep found no board address, and no Rockchip USB loader
  device appeared on the host. The EasyTier `198.18.0.31` route accepts TCP
  locally but returns no SSH banner, so it cannot be used as a recovery shell.
- The first 6.1.75 `rk-6.1-rkr3` serve attempt is therefore **not bootable
  enough for hardware validation**. The board needs a physical power cycle
  and/or Loader/MaskROM or serial-console entry before restoring the saved p3.

## 2026-11-12 H.264 / JPEG / post-processing closeout

- H.264 encode fixed-QP remains **24/24** over 64x64, 320x240, 640x480 and
  1280x720 x Baseline/Main/High x two QPs. CBR/VBR/AVBR, hierarchical QP and
  SMART_P matrices pass.
- `board_h264_reencode_vs_mpp.sh` passes **13/13** strategy cells: same-frame
  CBR/VBR/AVBR reencode, super-frame reencode, NORMAL drop in all three RC
  modes, CAVLC and CABAC PSKIP in all three RC modes. Each cell also compares
  every submitted job's STATUS window.
- H.264 GDR is **4/4** for row-4, col-4, row-8 and row-1 over 25 frames, 35 NAL
  units per cell. Stock C is nondeterministic here: its ROI table copies
  uninitialised `force_inter`/`mode_mask`/`reserved` stack bits into DMA, and
  three fresh runs produced `d4432259…`, `a49958ff…` and `f77af73e…`.
  `tools/mpp_roi_table_fix.c` zeroes only those undeclared bits at DMA sync;
  Rust emits the same deterministic table and then matches C byte-for-byte.
- H.264 ordinary slice splitting is **4/4** and low-delay packet output is
  **5/5**. The first-packet length differs by the documented version-SEI text
  delta; complete NAL streams and every following slice packet are identical.
- JPEG encoding passes **41/41** cells across restart, 90/270 rotation,
  RGB/YUV input, low-delay slices, four-band multicore and strided planes.
- Post-processing hardware tests pass **4/4**, including client-28 binding,
  two-frame I5O2, direct decoder DMA-BUF import, and a 50x34 H.264 surface with
  horizontal stride 64 and vertical stride 48. The padded import is compared
  against an explicit padded source and both destination slots are
  byte-identical.
- IEP2 full matrices remain **9/9** modes, **6/6** formats and **5/5** PD/DECT
  cells; telecine, progressive content and 322x242/722x482/1366x768 geometry
  all match the C probe's pixels and per-frame state.
- Final t6 gate: `board_workspace_gate.sh` reports **1317 passed / 0 failed /
  45 ignored** across 59 suites, `cargo fmt --check` is clean, and workspace
  rustdoc passes with `-D warnings`. `cargo test --release --lib --offline`
  reports **1147 passed / 0 failed / 0 ignored**.

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
- The current t6 crate `--all-targets` gate is **1279 passed / 0 failed /
  41 ignored**. `scripts/board_workspace_gate.sh`, which also runs the
  `rk-mpp-rs-sys` tests and strict workspace rustdoc, reports **1283 passed /
  0 failed / 41 ignored**. `cargo fmt --all --check` and `git diff --check`
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

## 2026-11-11 AV1 software film-grain synthesis

- The independent M11 decoder now applies film grain only to displayed copies.
  `ReferenceFrameStore` keeps post-filter, pre-grain pixels and the frame's grain
  parameter set, so ordinary output and `show_existing_frame` use the historical
  frame's parameters without contaminating inter prediction.
- Stock `aomdec --output-bit-depth=10` is the oracle. The 8-bit stream passes
  **10/10 grain-applied frames**. The 10-bit stream passes **10/10 clean
  pre-grain frames** and **10/10 grain-applied frames**; all thirty frame
  projections are sample-exact after accounting for aomdec's 16-bit container
  alignment.
- The complete AV1 software-reference suite passes **359/359** on t6 at the
  persistent 80 % OPP cap.

## 2026-11-11 AV1 multi-size and 4:4:4 inter conformance

- Existing 96x96, 128x128 and 192x192 ten-frame inter fixtures now have locked
  libaom pixel hashes; each stream is **10/10** sample-exact.
- A new 320x240 profile-1 I444 key+P+P fixture matches stock `aomdec` on all
  three displayed YUV444 frames. It combines one-sided compound, inter-intra,
  OBMC, warped motion and nearby NEWMV candidates.
- The I444 stream exposed and fixed two context bugs: inter-intra admitted
  `BLOCK_8X32`/`BLOCK_32X8` when the normative range ends at `BLOCK_32X32`, and
  single-reference MV-stack scanning ignored NEWMV components in matching
  compound neighbours, producing mode context 67 instead of the reference 66.
- The complete AV1 software-reference suite is now **359/359**.

## 2026-11-11 AV1 one-column tile rows

- A new 320x320 key+P+P fixture is encoded with
  `tile_cols_log2=0` and `tile_rows_log2=1`; IntraBC, palette, CDEF, loop
  restoration and the loop filter are disabled to isolate the row-tile path.
- Both full-width row tiles restart their entropy state from the frame CDF,
  while inter-frame neighbour state follows the tile decode order. All three
  visible Y/U/V frames match stock `aomdec` sample-exactly.
- AV1 has no separate WPP flag or HEVC-style entry-point context chain. This
  tile-row shape closes the remaining AV1 item that had been tracked as WPP.

## 2026-11-11 AV1 screen-content IntraBC

- A default 320x320 screen-content key+P+P stream now exercises 20 IntraBC
  blocks and 4-color luma/chroma palettes. The reference decoder uses a
  separate `ndvc` CDF, validates integer DVs against the current tile, decodes
  the inter variable-transform tree and residual payload, then copies from the
  already reconstructed current-frame region.
- All three final Y/U/V frames match stock `aomdec` sample-exactly. The
  complete AV1 software-reference suite is **357/357**.

## 2026-11-11 AV1 B-pyramid

- A 20-frame 320x240 reordered B-pyramid is encoded with
  `--lag-in-frames=19 --auto-alt-ref=1`. It exercises hidden showable frames,
  reference-slot refreshes, order-hint reordering and repeated
  `show_existing_frame` across multiple slots.
- All 20 displayed Y/U/V frames match stock `aomdec` sample-exactly. The
  complete AV1 software-reference suite is **358/358**.

## 2026-11-11 AV1 advanced B-pyramid

- The same 20-frame reordered schedule is encoded with global/warped motion,
  OBMC, inter-intra, masked/distance-weighted compound and reference-frame MVs
  enabled. Multiple coded frames reference at least three non-zero order-hint
  slots.
- All 20 displayed Y/U/V frames match stock `aomdec` sample-exactly. The
  complete AV1 software-reference suite is **359/359**.

## 2026-11-11 AV1 ten-bit B-pyramid

- A 12-frame 320x240 10-bit `testsrc2` B-pyramid is encoded with
  `--lag-in-frames=11 --auto-alt-ref=1`. All 12 coded frames are showable, the
  order-hint history reaches seven non-zero slots while every coded frame uses
  single-reference prediction. The stream combines 10-bit inter prediction,
  residual reconstruction and reference-slot rotation.
- The Rust dump is sample-exact to stock `aomdec` for all 12 displayed Y/U/V
  frames: 2,764,800 bytes with no differing byte. The complete AV1
  software-reference suite is now **360/360**.

## 2026-11-11 AV1 advanced ten-bit B-pyramid

- A 20-frame 320x240 10-bit B-pyramid keeps hidden showable frames,
  `show_existing_frame`, global/warped motion, OBMC, inter-intra,
  masked/distance-weighted compound and reference-frame MVs enabled together.
- All 20 displayed Y/U/V frames match stock `aomdec` sample-exactly:
  4,608,000 bytes with no differing byte. The complete AV1 software-reference
  suite is now **361/361**.

## 2026-11-11 AV1 ten-bit B-pyramid deblock and CDEF

- The same 20-frame advanced 10-bit B schedule enables non-zero deblocking on
  every inter coded frame and enables CDEF for the sequence. Loop restoration
  remains disabled and is not counted as covered by this gate.
- All 20 displayed Y/U/V frames match stock `aomdec` sample-exactly:
  4,608,000 bytes with no differing byte. The complete AV1 software-reference
  suite is now **362/362**.

## 2026-11-11 AV1 ten-bit 4:4:4 advanced B-pyramid

- A 20-frame I444P10 advanced B stream retains hidden showable frames,
  `show_existing_frame`, compound prediction and the advanced inter feature
  set with no chroma subsampling.
- All 20 displayed Y/U/V frames match stock `aomdec` sample-exactly:
  9,216,000 bytes with no differing byte. The complete AV1 software-reference
  suite is now **363/363**.

## 2026-11-11 AV1 ten-bit B-pyramid with inter loop restoration

- A 20-frame advanced 10-bit B stream encoded with `--sb-size=64
  --cpu-used=0 --cq-level=20` contains a real inter frame with `uses_lr=true`
  alongside CDEF and reference reordering.
- The first run failed at coded frame 5 with `ExitSymbol/PaddingNotZero`.
  Comparing libaom and Rust narrowed it to `NEW_NEARMV`: the NEW component must
  use reference index `1 + ref_mv_idx`. The wrong MV had polluted temporal MV
  projection. After the fix, all 20 displayed Y/U/V frames match stock `aomdec`
  sample-exactly: 4,608,000 bytes with no differing byte. The complete AV1
  software-reference suite is now **365/365**.

## 2026-11-11 H.264 CABAC P-slice P_Skip

- A new 48x32 Main-profile stream decodes one CABAC IDR followed by a P picture
  whose six macroblocks are all skip, as confirmed by x264's `mb P ... skip:
  100.0%` report. The P slice uses `cabac_init_idc=0` and
  `first_mb_in_slice=0`; deblocking and weighted prediction are disabled to
  isolate CABAC P syntax.
- The focused integration gate
  `cargo test --release --test h264_reference_i32
  cabac_p_skip_copies_reference_macroblocks_matches_ffmpeg_yuv420p` passes. The
  test reconstructs the IDR first, decodes every `mb_skip_flag` in the P slice,
  copies reference picture 0 and compares both complete 4:2:0 frames. All
  4,608 bytes match FFmpeg.
- The first implementation used `decoded` instead of `decoded && !skip` for the
  skip-neighbour context terms, which moved from ctxIdx 11 to 12/13 and ended
  the slice after two macroblocks. The real fixture reduced that failure to a
  deterministic two-macroblock result; the corrected terms decode all six.
- At the persistent 80% OPP cap, `scripts/board_workspace_gate.sh` reports
  **1267 passed / 0 failed / 41 ignored**, and `cargo test --release --lib
  --offline` reports **1117 passed / 0 failed / 0 ignored**. Fixture manifest,
  rustfmt and strict workspace rustdoc pass.

## 2026-11-12 H.264 CABAC zero-motion P16

- A second 48x32 Main-profile stream is locked to a 100% I16 IDR followed by a
  P picture containing six `P_L0_16x16` macroblocks. The P slice has zero MVD,
  CBP 15/0, no deblocking, no weighted prediction and `cabac_init_idc=0`.
- The focused gate
  `cargo test --test h264_reference_i32
  cabac_p16_zero_mv_residuals_match_ffmpeg_yuv420p` passes. It consumes
  P mb_type, MVD, CBP, macroblock QP, every luma 4x4 inter residual, performs
  zero-motion reference copy plus residual reconstruction and compares both
  complete 4:2:0 frames. All 4,608 bytes match FFmpeg.
- A temporary x264 trace exposed the defect: inter CBF neighbours follow the
  I-slice rule in reverse for unavailable macroblocks. Unavailable inter
  neighbours contribute zero, while the intra path contributes a non-zero
  context flag. The first P16 block therefore moved from context 93 to 96 and
  desynchronised the third macroblock. Separate inter CBF context helpers now
  preserve both rules.
- At the persistent 80% OPP cap, `scripts/board_workspace_gate.sh` reports
  **1268 passed / 0 failed / 41 ignored**, and `cargo test --release --lib
  --offline` reports **1117 passed / 0 failed / 0 ignored**. Fixture manifest,
  rustfmt and strict workspace rustdoc pass.

## 2026-11-12 H.264 CABAC integer-motion P16

- A random-texture 48x32 source is cropped at x=0 and x=8. A temporary x264
  trace build disables P-skip and all P sub-partitions/intra modes, then forces
  each `P_L0_16x16` cache MV to `(+32, 0)` quarter-pel so the encoder computes
  residuals against that exact vector. The P frame contains six P16
  macroblocks with final MVs `(+32, 0)`: luma moves +8 pixels and 4:2:0 chroma
  moves +4 pixels.
- The focused gate `cabac_p16_integer_motion_matches_ffmpeg_yuv420p` passes. It
  covers non-zero MVD syntax and sign, P16 MVP neighbour selection, integer
  luma/chroma motion compensation, reference-edge extension and residual
  reconstruction. The test supplies FFmpeg's IDR frame as reference picture 0;
  the 2,304-byte P frame matches FFmpeg sample-exactly.
- The first 8-pixel attempt exposed a reversed MVD bypass sign: x264 declared
  `[-4,-8]` while Rust decoded `[+4,+8]`. The zero-MV gate could not observe
  this. After fixing the sign, forced `+32` vectors decode consistently and all
  six prediction offsets are non-zero.
- Fractional luma/chroma vectors, P_16x8/P_8x16/P_8x8, weighted prediction,
  deblocking and B-slice CABAC remain open.
- At the persistent 80% OPP cap, `scripts/board_workspace_gate.sh` reports
  **1269 passed / 0 failed / 41 ignored**, and `cargo test --release --lib
  --offline` reports **1117 passed / 0 failed / 0 ignored**. Fixture manifest,
  rustfmt and strict workspace rustdoc pass.

## 2026-11-12 H.264 CABAC fractional-motion P16

- The same 48x32 random-texture crop pair is encoded with normal subpel search.
  The temporary x264 build still forbids P-skip, intra modes and P
  sub-partitions, so the P frame remains six P16 macroblocks while their final
  vectors become `(-6,-9)`, `(8,1)`, `(5,1)`, `(-5,-6)` and `(7,1)`.
- The decoder implements normative quarter-pel luma interpolation for every
  horizontal, vertical and diagonal fractional combination, including the
  clipped six-tap half samples and separate `+512 >> 10` two-dimensional
  rounding. 4:2:0 chroma uses all 64 bilinear weights for its 1/8-sample
  positions.
- The first attempt exposed another H.264 state bug: the previous macroblock's
  MVD neighbour cache must store absolute magnitudes for context selection while
  the signed delta remains available for addition to the MVP. Storing only the
  signed value desynchronised the stream at macroblock 2.
- The 2,304-byte P frame matches FFmpeg sample-exactly. Fractional P16 vectors
  no longer fail; P_16x8/P_8x16/P_8x8, weighted prediction, deblocking and
  B-slice CABAC remain open.
- At the persistent 80% OPP cap, `scripts/board_workspace_gate.sh` reports
  **1270 passed / 0 failed / 41 ignored**, and `cargo test --release --lib
  --offline` reports **1117 passed / 0 failed / 0 ignored**. Fixture manifest,
  rustfmt and strict workspace rustdoc pass.

## 2026-11-12 H.264 CABAC partitioned-motion P16x8 / P8x16

- Two new 48x32 Main-profile fixtures force every P macroblock to
  `P_L0_L0_16x8` or `P_L0_L0_8x16`. x264 reports 100% of the selected
  partition and no skip; the temporary trace build exposes no intra
  macroblocks. The 16x8 stream SHA-256 is
  `4645e307b3858f9550b6cfe9d605c535d68462dca03c485295f9295f897b4c25`,
  YUV `ba4c46a50445a9a8f28c49b9cbdacd2cfd01922d7d7fb96cbcc71fc5a99f99e3`;
  the 8x16 stream is
  `0dc9850b597f1ebeb797603f0d30be9d067d69345ddd73573a42ca95ebd0cb58`,
  YUV `805cafd67d7e20345f2af1fe0e676ff24644b149c4ea499c3b2cd966cf79a47f`.
- The decoder consumes both partition MVDs and implements the normative
  A/B/C MVP and MVD context positions from partition scan order. 16x8 vectors
  include `(-32,0)`, `(16,0)`, `(32,0)`, `(-37,0)` and `(6,2)`; 8x16 vectors
  include `(-28,0)`, `(16,0)`, `(-2,17)`, `(-32,0)` and `(28,0)`. Each
  partition gets independent luma/chroma motion compensation, followed by one
  shared macroblock residual reconstruction.
- The pixel gate exposed a missing quarter-pel `(2,2)` branch in the luma
  interpolation dispatch, which fell back to the integer sample. Adding the
  normative two-dimensional half-sample path makes both complete P frames
  match FFmpeg sample-exactly.
- The H.264 CABAC integration suite is **6/6**, the focused CABAC unit suite
  is **7/7**, the crate `--all-targets` gate is **1270 passed / 0 failed /
  41 ignored**, `scripts/board_workspace_gate.sh` is **1274 passed / 0 failed
  / 41 ignored**, and release-library tests are **1119 passed / 0 failed /
  0 ignored**. Fixture manifest, rustfmt and strict workspace rustdoc pass.
- `P_8x8`, weighted prediction, deblocking and B-slice CABAC remain open.

## 2026-11-12 H.264 CABAC P_8x8 sub-macroblock partitions

- A new mixed 48x32 Main-profile stream enables P_8x8 and sub-8x8 analysis.
  The P picture contains 16x16, 8x16 and P_8x8 macroblocks; across its P_8x8
  quadrants all four `sub_mb_type` modes are present (`8x8`, `8x4`, `4x8`,
  `4x4`). Stream SHA-256 is
  `5e61cfc82df0f3c4452603bca054bc48d364b32077ff684bd9a9b6bfa296292a`,
  decoded YUV is
  `0d5542e83935c20dfd52efb9075eb4a02cb0d3a2e88a5a3cc1bbb6af973d833f`.
- The decoder consumes the P_8x8 mb_type suffix and each one-/two-/three-bin
  subpartition tree, expands every quadrant into its normative 8x8/8x4/4x8/4x4
  scan order, maintains MVD context and motion vectors in the 4x4 grid, then
  runs independent luma/chroma motion compensation and one shared macroblock
  residual reconstruction. All 4,608 decoded bytes match FFmpeg exactly.
- The H.264 CABAC integration suite is **7/7**, the focused CABAC unit suite is
  **8/8**, the crate `--all-targets` gate is **1272 passed / 0 failed /
  41 ignored**, `scripts/board_workspace_gate.sh` is **1276 passed / 0 failed
  / 41 ignored**, and release-library tests are **1120 passed / 0 failed /
  0 ignored**. Fixture manifest, rustfmt and strict workspace rustdoc pass.
- Weighted/deblock P pictures and B-slice CABAC remain open.

## 2026-11-12 H.264 CABAC explicit weighted P_L0_16x16

- A new two-frame 48x32 Main-profile stream forces explicit weighted
  prediction with luma scale/offset `3/8` at denominator 2 and U/V `7/4` at
  denominator 3. x264 reports weighted P frames for Y and UV at 100%; P-skip,
  P subpartitions, intra P and transform-8x8 stay disabled. Stream SHA-256 is
  `2f15b4d68909169af744847e567077587abf42c6841211afb08b9b1641404eb9`,
  decoded YUV is
  `346c0689eaa1a4a01fc6cded3b80c5631b206ff24aab4e1d1c8dad9e75ab3502`.
- The slice header ends at bit 71, exposing an existing alignment bug: the
  parser's `+9` CABAC rounding skipped one extra byte and started slice data
  at bit 80. It now rounds to the next byte boundary at bit 72, matching x264
  and FFmpeg.
- Explicit weights are applied after luma/chroma interpolation with the
  normative rounded scale, bit-depth-scaled offset and clipping, before the
  shared residual is added. The fixture exercises all three planes. All 4,608
  decoded bytes match FFmpeg. Weighted P_Skip uses the same sample transform
  but still needs a dedicated fixture.
- The H.264 CABAC integration suite is **8/8**, the focused CABAC unit suite is
  **9/9**, the crate `--all-targets` gate is **1274 passed / 0 failed /
  41 ignored**, `scripts/board_workspace_gate.sh` is **1278 passed / 0 failed
  / 41 ignored**, and release-library tests are **1121 passed / 0 failed /
  0 ignored**. Fixture manifest, rustfmt and strict workspace rustdoc pass.
- Weighted P_Skip, P-frame deblocking and B-slice CABAC remain open.

## 2026-11-12 H.264 CABAC P-frame deblocking

- A third 48x32 P16 fixture enables loop filtering with slice
  `alpha/beta=2/1`. It combines coded chroma edges with uncoded luma edges
  whose boundary strength is selected from neighboring 4x4 quarter-pel MVs.
  Stream SHA-256 is
  `985990b8b2578ebaa6665d971b11d447259fd5fde41f61cc722655d51dd2fa99`,
  decoded YUV is
  `f26b6364970c12acdac5fd92fe348d11092e4588305fcf1c7b94412eebeeec07`.
- `DeblockMap` now carries P-slice 4x4 MVs. P boundary strength is 3 when
  either side has coefficients, 2 for a component delta of at least four
  quarter-pels, 1 for any smaller non-zero MV delta and 0 for equal motion.
  Luma and chroma traverse vertical before horizontal edges, average QP at
  each boundary and use the existing normative filters.
- The complete deblocked P frame matches FFmpeg sample-exactly. The H.264
  CABAC integration suite is **9/9**, CABAC unit tests are **9/9**, deblock
  unit tests are **8/8**, the crate `--all-targets` gate is **1276 passed / 0
  failed / 41 ignored**, `scripts/board_workspace_gate.sh` is **1280 passed /
  0 failed / 41 ignored**, and release-library tests are **1122 passed /
  0 failed / 0 ignored**. Fixture manifest, rustfmt and strict workspace
  rustdoc pass.
- Weighted P_Skip and B-slice CABAC remain open.

## 2026-11-12 H.264 CABAC two-reference P16

- A three-frame 48x32 Main-profile stream activates two L0 references. The
  final P picture deliberately mixes the recent P and older IDR and selects
  per-macroblock references `1,0,0,0,1,0`. Stream SHA-256 is
  `091920b51cc69c454fc4fc0f27a4422281bdddd36f7df91242e7b809f63599af`,
  decoded YUV is
  `06c942f2c21525ac0415f67165dbb61651b4fb85d30154e37f6eb58469d3d53b`.
- The multi-reference decoder accepts an arbitrary active L0 list and carries
  reference indices in the 4x4 motion cache. It consumes CABAC ref_idx unary
  trees with left/top neighbor contexts, applies reference-specific
  luma/chroma motion compensation and selects the matching weighted table.
- Reference-aware MVP reproduces the x264 fallback rule: if B and C are
  unavailable, A supplies the predictor even when it points at a different
  reference. This was the missing rule behind the first 1,847-sample
  divergence; after the fix the complete 4,608-byte P frame is exact.
- The H.264 CABAC integration suite is **10/10**, CABAC unit tests are
  **10/10**, deblock unit tests are **8/8**, the crate `--all-targets` gate is
  **1278 passed / 0 failed / 41 ignored**, `scripts/board_workspace_gate.sh`
  is **1282 passed / 0 failed / 41 ignored**, and release-library tests are
  **1123 passed / 0 failed / 0 ignored**. Fixture manifest, rustfmt and strict
  workspace rustdoc pass.
- Weighted P_Skip and B-slice CABAC remain open.

## 2026-11-12 H.264 CABAC B_L0_16x16

- A new IDR+P+B stream is encoded in coding order with direct mode, B-skip, B
  subpartitions, weighted bi-prediction and deblocking disabled. A temporary
  x264 switch forces all six B macroblocks to `B_L0_16x16`; x264 reports B16
  L0 100%. Stream SHA-256 is
  `ab378bde3bdddd8b3f89cc583cc7a37c9757c69e74ef82a9f4b46f830daad822`,
  decoded YUV is
  `09e707f462e533531272be1c9c8a70cbc680644dc406defc28ccc0452c1521f7`.
- The decoder consumes B skip bins in contexts 24-26, the B type predicates in
  contexts 27-30/32, list-0 MVD and the shared inter CBP/residual tree. It
  reconstructs each macroblock from the displayed I reference through
  forward-only motion compensation. The complete displayed 48x32 B frame
  matches FFmpeg sample-exactly.
- The focused CABAC integration suite is **25/25**, CABAC unit tests are
  **10/10**, deblock unit tests are **8/8**, the crate `--all-targets` gate is
  **1279 passed / 0 failed / 41 ignored**, `scripts/board_workspace_gate.sh`
  is **1283 passed / 0 failed / 41 ignored**, and release-library tests are
  **1123 passed / 0 failed / 0 ignored**. Fixture manifest, rustfmt and strict
  workspace rustdoc pass.
- B_Direct, B_L1, B_BI, B subpartitions, weighted B and B deblocking remain
  open.

## 2026-11-12 H.264 CABAC B_L1 and B_BI 16x16

- Two new IDR+P+B streams force all six B macroblocks to `B_L1_16x16` and
  `B_BI_16x16`. The list-1 stream SHA-256 is
  `103ed3ccdf345a4b9c2e43888c44b4c5bc34fe871ff34b1c136c580b525a7ca8`,
  its decoded YUV is
  `b0e3c81cf03b889d05869db67563cba3b5e829e8a576219eaf7d4e9a6aa31cd9`,
  the bi-predicted stream is
  `e94f08bae6653b0af8a86e4d38981b45286e77c439cba2b25134f6aa868e296f`,
  and its decoded YUV is
  `e0927b98b0ae12b2857c5bdb66a8d667f78328ca65274812e03633a1974d0dc9`.
- The decoder retains independent L0/L1 4x4 motion and reference caches,
  consumes list-0 then list-1 MVD syntax when both are present, and blends the
  temporary predictions with `(L0 + L1 + 1) >> 1`. Both complete displayed
  48x32 B frames match FFmpeg sample-exactly.
- The B16 focused gate is **3/3**, the complete CABAC integration suite is
  **27/27**, CABAC unit tests are **10/10**, and deblock unit tests are
  **8/8**. The crate `--all-targets` gate is **1281 passed / 0 failed /
  41 ignored**, `scripts/board_workspace_gate.sh` is **1285 passed / 0 failed /
  41 ignored**, and release-library tests are **1123 passed / 0 failed /
  0 ignored**. Fixture manifest, rustfmt and strict workspace rustdoc pass.
- B_Direct/B_Skip, B 16x8/8x16/8x8, weighted B and B deblocking remain open.

## 2026-11-12 H.264 CABAC spatial B_Direct and B_Skip

- A new IDR+P+B stream enables spatial direct and mixes one BI macroblock with
  three `B_Direct_16x16` and two `B_Skip` macroblocks. x264 reports direct
  50%, skip 33.3% and BI 16.7%. Stream SHA-256 is
  `e4a0a2177f4b81d41b37af504f3746bcf0ad28e8720f05178f85feeb475d60a0`,
  decoded YUV is
  `5745621d236e55f3fce23d96a1bf8416ad09f887fc7667b1f00bc8b594b10306`.
- Skip and type contexts now use different neighbor predicates: `B_Skip`
  counts as skipped, while direct still counts as non-skip for the skip
  context but is excluded from the type context. Spatial direct derives L0/L1
  motion from the A/B/C neighbors, performs separate list prediction and
  applies the normative BI average. Coded direct consumes the inter residual
  tree; `B_Skip` consumes neither residual nor macroblock-QP syntax.
- The displayed 48x32 B frame matches FFmpeg sample-exactly. Temporal direct,
  B 16x8/8x16/8x8, weighted B and B deblocking remain open.
- The B16 focused gate is **4/4**, the complete CABAC integration suite is
  **28/28**, the crate `--all-targets` gate is **1282 passed / 0 failed /
  41 ignored**, `scripts/board_workspace_gate.sh` is **1286 passed / 0 failed /
  41 ignored**, and release-library tests are **1123 passed / 0 failed /
  0 ignored**. Fixture manifest, rustfmt and strict workspace rustdoc pass.

## 2026-11-12 H.264 CABAC temporal B_Direct and B_Skip

- A second IDR+P+B stream keeps the same BI/direct/skip mix but selects
  temporal direct. The coded picture POCs are 0/4/2. The test decodes the P
  picture first and supplies its 16x16 motion partitions through the reference
  frame metadata.
- Direct motion uses the normative distance scale, here 128, to project the
  colocated P list-0 vector into the B picture's L0 vector; the L1 vector is
  the projected vector minus the colocated vector. Both lists then use the
  existing fractional luma/chroma motion compensation and BI blend.
- Stream SHA-256 is
  `b7bd26b4b60abe4843d72638f4b7809e183289fede92d346352f6df559ab5932`,
  decoded YUV is
  `68c2443dbf2eece268399d7f057f3598c0e417b9786d9a5b0ae6cad256c176ef`,
  and the displayed 48x32 frame matches FFmpeg sample-exactly.
- Col-zero B references, colocated non-16x16 partitions, weighted B and B
  deblocking remain open.
- The B16 focused gate is **5/5**, the complete CABAC integration suite is
  **29/29**, the crate `--all-targets` gate is **1283 passed / 0 failed /
  41 ignored**, `scripts/board_workspace_gate.sh` is **1287 passed / 0 failed /
  41 ignored**, and release-library tests are **1123 passed / 0 failed /
  0 ignored**. Fixture manifest, rustfmt and strict workspace rustdoc pass.

## 2026-11-12 H.264 CABAC B_L0_L0 16x8 and 8x16

- Two additional IDR+P+B streams force every B macroblock to
  `B_L0_L0_16x8` and `B_L0_L0_8x16`; x264 reports B 16x8 100% and B 8x16
  100% respectively. The 16x8 stream SHA-256 is
  `9bea7354638a56f55b0de2ea52afdb907fdf99260e04a5b17bd2006ea2aa7233`
  with YUV
  `9c72387cc041a2379b432d1f2777127506c9d33c40f18fd84af8356149348c0a`.
  The 8x16 stream is
  `0e140e59a0b8b08ecfb42e67718e867fd18234a2d92beb67b1066582e9cd37e8`
  with YUV
  `4f6b51430c4f76392dadc2fdb924c03b7c328609b4da71c16a6a1c23d79033b3`.
- The B mb_type parser now consumes the normative four-bit CABAC prefix and
  maps bits 1/2 to L0 16x8/8x16. Both partitions decode MVDs in scan order,
  update separate 4x4 motion cache regions, run independent luma/chroma motion
  compensation and share one macroblock residual tree. Both displayed frames
  match FFmpeg sample-exactly.
- The B16 focused gate is **7/7**, the complete CABAC integration suite is
  **31/31**, the crate `--all-targets` gate is **1285 passed / 0 failed /
  41 ignored**,
  `scripts/board_workspace_gate.sh` is **1289 passed / 0 failed / 41 ignored**,
  and release-library tests are **1123 passed / 0 failed / 0 ignored**.
  Fixture manifest, rustfmt and strict workspace rustdoc pass.

## 2026-11-12 H.264 CABAC B_8x8 L0 subpartitions

- A third IDR+P+B stream forces every B macroblock to `B_8x8` and all four
  sub_mb_types to `B_L0_8x8`; x264 reports B_8x8 100%. Stream SHA-256 is
  `82a8ff29c22e973f21c38a129d6b2b09aa9f03562b080f94607ac696c733e71a`
  and decoded YUV is
  `cd45314da12e3987d3bd3f5ddbb1be420cd8fddd20b4f61f9173e2dc2d925a76`.
- The decoder now accepts B mb_type CABAC bits 15, consumes four
  sub_mb_type decisions in contexts 36-39, decodes four L0 MVDs in 8x8 scan
  order, updates each 4x4 motion-cache quadrant independently and applies one
  shared macroblock residual tree. The complete displayed frame matches
  FFmpeg sample-exactly.
- The B16 focused gate is **8/8**, the complete CABAC integration suite is
  **32/32**, the crate `--all-targets` gate is **1286 passed / 0 failed /
  41 ignored**, `scripts/board_workspace_gate.sh` is **1290 passed / 0 failed /
  41 ignored**, and release-library tests are **1123 passed / 0 failed /
  0 ignored**. Fixture manifest, rustfmt and strict workspace rustdoc pass.
- Direct/L1/BI subpartitions, mixed 8x4/4x8/4x4 shapes, col-zero B references
  and weighted/deblock B remain open.

## 2026-11-12 H.264 CABAC B_8x8 L1 and BI subpartitions

- Two more IDR+P+B streams keep mb_type `B_8x8` but force all four
  sub_mb_types to `B_L1_8x8` and `B_BI_8x8`; x264 reports L1 100% and BI
  100%. The L1 stream SHA-256 is
  `f92fde6bf8c020589376cc2701b245c0d71fd5c4e62c29857704b898f76eeeca`
  with YUV
  `b0fdb0307b0996bf93ebb051ff3a2798445e242f5229a59a975b7ffaa5196611`.
  The BI stream is
  `a6b7a9dbcffbdbe5b3ff5555e872c559800f3e2a800e1f1b24f338c38c3d8b2e`
  with YUV
  `52acffe0637ac105fa8790038230d8e63540f5009ed12cffb3dec3e6ba5d0161`.
- The B_8x8 parser now accepts sub_mb_type paths for L0, L1 and BI 8x8.
  List-0 MVDs are consumed for all four quadrants before list-1, each list
  updates its own 4x4 motion cache, and BI builds separate 8x8 luma/chroma
  predictions before the normative rounded average. Both displayed frames
  match FFmpeg sample-exactly.
- The B16 focused gate is **10/10**, the complete CABAC integration suite is
  **34/34**, the crate `--all-targets` gate is **1288 passed / 0 failed /
  41 ignored**, `scripts/board_workspace_gate.sh` is **1292 passed / 0 failed /
  41 ignored**, and release-library tests are **1123 passed / 0 failed /
  0 ignored**. Fixture manifest, rustfmt and strict workspace rustdoc pass.
- Direct 8x8, 8x4/4x8/4x4, col-zero B references, weighted B and B deblocking
  remain open.

## 2026-11-12 H.264 CABAC mixed L0 B_8x8 geometry

- One IDR+P+B stream keeps `B_8x8` and assigns the four quadrants to
  `B_L0_8x4`, `B_L0_4x8`, `B_L0_4x4` and `B_L0_8x8`. Stream SHA-256 is
  `9d9cc8b58a686d2065c8c183ba0f73bdb7d2ef7a030a300ec2a18683e436225d`
  and decoded YUV is
  `cd45314da12e3987d3bd3f5ddbb1be420cd8fddd20b4f61f9173e2dc2d925a76`.
- The decoder consumes the complete CABAC sub_mb_type tree and the matching
  2/2/4/1 MVD sequence in quadrant and sub-partition scan order. Every leaf
  updates its 4x4 cache cells and receives independent luma/chroma motion
  compensation before the shared residual tree. The displayed frame matches
  FFmpeg sample-exactly.
- The B16 focused gate is **11/11**, the complete CABAC integration suite is
  **35/35**, the crate `--all-targets` gate is **1289 passed / 0 failed /
  41 ignored**, `scripts/board_workspace_gate.sh` is **1293 passed / 0 failed /
  41 ignored**, and release-library tests are **1123 passed / 0 failed /
  0 ignored**. Fixture manifest, rustfmt and strict workspace rustdoc pass.
- Direct 8x8, L1/BI non-8x8 geometry, col-zero B references, weighted B and
  B deblocking remain open.

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

`cargo test --release --all-targets` currently enumerates **1295 tests**. The
regular suite passes **1254 / 0 failed / 41 ignored** on t6. The four ignored
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
