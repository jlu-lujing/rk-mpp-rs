# Progress Log

## Session: 2026-11-21

### Phase 26: FFmpeg Compatibility and Copy Removal
- **Status:** completed copy phase; asynchronous encoder still in progress.
- Added an unchanged-FFmpeg `rkmpp` compatibility path for the newer FFmpeg
  info-change/external-buffer/control sequence.
- Added optional `rockchip_mpp.pc` and `librockchip_mpp` aliases. FFmpeg source
  configure and a minimal `make` both succeeded against the Rust library.
- Benchmarked H.264/H.265 decode and encode across C FFmpeg,
  FFmpeg+rkmpp-rs and native rkmpp-rs.
- Removed encode input CPU staging and decode packed-frame allocation/copy.
  H.264 FFmpeg+rkmpp-rs decode improved 0.901 -> 0.513 s; H.265 improved
  0.927 -> 0.517 s. Static NV12 output remains byte-identical.
- H.264/H.265 FFmpeg+rkmpp-rs encode improved about 12%; MJPEG remains exact
  on the CPU fallback.
- Fixed FFmpeg's uncached MPP DRM buffer semantics: compatibility groups now
  allocate from `system-uncached` unless `MPP_BUFFER_FLAGS_CACHABLE` is set.
  The previous cached mapping made VEPU580 read stale P-frame input even though
  CPU-visible bytes matched the source.
- Preserved SEI/header session controls when FFmpeg sends its second
  `MPP_ENC_SET_CFG` at the first frame.
- H.264 and H.265 match vendor C byte-for-byte for the 2/12-frame gates and
  60-frame single-GOP sequences at 320x240 and 1280x720.
- Repeated evidence found that VEPU580 C FFmpeg is not a stable stream oracle
  after a second GOP boundary in the same encode. Eight cold H.264 60-frame runs
  were identical, while 120-frame GOP60 runs produced four different streams
  across processes; three separately decoded samples produced three different
  YUVs. Rust is deterministic and reproduces one valid C mode; the exact gate
  therefore stops at one complete 60-frame GOP.
- H.265 now carries level into VPS/SPS, enables slice temporal MVP plus
  collocated/colmv state, and emits a fresh IDR at each GOP boundary.
- H.264 compatibility now follows FFmpeg's absent `KEY_MOTION_INFO`: MD writeback
  is disabled and the scene-tune update is delayed one picture to match the
  vendor async encoder.
- Added direct DMA-BUF versus CPU-fallback equality checks plus 12-frame and
  60-frame single-GOP exact H.264/H.265 cases to `board_ffmpeg_rkmpp.sh`.
  A second GOP is excluded from the exact gate because the C stream becomes
  process-state dependent on this hardware.
- Implemented a VEPU580 H.264 fixed-QP depth-2 session queue for the FFmpeg
  compatibility path. Pending jobs own independent batch/STATUS/output state,
  input metadata stays in submission order, reconstruction slots rotate
  `0→1→2`, and pipeline completion uses blocking `POLL_HW_FINISH`.
- H.264 60-frame single-GOP remains byte-identical at 320x240 and 1280x720. Eleven
  interleaved 300-frame 720p samples on A76 cores improved synchronous
  0.854 s / 351 FPS to depth-2 0.335 s / 896 FPS; C FFmpeg is
  0.358 s / 838 FPS.
- Added `board_ffmpeg_h264_pipeline_bench.sh` for C/sync/depth-1/depth-2
  comparison and retained only the byte-identical depth.
- Extended the session queue to VEPU580 H.265. Its three-surface short-term P
  rotation is byte-exact at depth 1; 300-frame 720p improved from 0.752 s /
  399 FPS synchronous to 0.612 s / 490 FPS. The earlier 912 FPS depth-2 result
  was unsafe: it omitted C's frame-parallel dual-core handshake and diverged
  around frame 41. Modelling `reg0193_dual_core` (`txid/rxid/txe/rxe` and
  offset 9) makes both 320x240 and 720p 60-frame single GOPs exact; 300-frame 720p is
  now 0.444 s / 676 FPS, matching C's 0.444 s / 676 FPS. H.265 depth 2 is the
  default.
- Imported MPP DMA-BUF buffers no longer mmap eagerly. The FFmpeg direct path
  keeps them idle until a C caller actually asks for a CPU pointer or falls
  back to CPU input. A 300-frame H.265 profile fell from 390.0M to 327.4M
  instructions, matching C's 327.3M, and from 428.0 to 362.9 ms task-clock.
  `strace -c` mmap calls fell from 544 to 244 while the FFmpeg and C object API
  gates remained byte-exact.
- The native fixed-QP CLI now drives the deferred hardware queue. H.265 depth 2
  is the default/byte-exact mode and improves 300-frame 720p from 438 to
  701 FPS. H.264 stays sync by default; explicit depth 2 reaches 909 FPS but
  moves scene-tune feedback one picture later, so it is not stream-identical
  to the default path.
- Rejected a pooled-surface decode variant after it regressed H.264/H.265 to
  C-level or below; no code from that experiment remains.
- Tried a high-level FFmpeg compatibility encoder worker with an input FIFO.
  The attempt could not guarantee delayed H.264/H.265 packet ownership and was
  rejected before commit.
- Kept two verified compatibility fixes from that investigation: NULL frames
  now request encoder EOS, and an empty packet queue returns `MPP_NOK`.
- Hardened `board_mpp_standalone.sh` after a reboot removed `/tmp/mpp-develop`:
  it now discovers a matching `mpi_dec_test`/library pair under
  `~/rk-mpp-oracles` and refuses to mix the system test with another oracle's
  library. The default gate is back to 5/5 after cold boot.
- Final regression: FFmpeg 6/6, official encoder/TSVC all pass, and host
  `cargo test --all-targets --offline` completes with 1199 passed, 0 failed,
  2 ignored in the root library plus all integration suites green.

## Session: 2026-11-20

### Phase 21: Public H.264 Syntax and Deblock ABI
- **Status:** complete on RK3588.
- Audited the existing stable C encoder surface. `H264DeblockConfig` and
  `H264SyntaxConfig` already exist in Rust and feed validated VEPU580/VEPU510
  paths, but there is no `rk_mpp_*` C descriptor or create entry point for them.
- Chosen direction: add an independently sized V1.13 advanced config V3,
  preserving every prior struct and initializer.
- Added field-selective H.264 syntax flags, a deblock descriptor, V3
  initialization/create functions, C11 layout tests, raw sys bindings and the
  40-symbol ABI snapshot.
- `board_sdk_encoder_controls.sh` on RK3588 passes the existing V1.9 consumer,
  six V1.10 ROI/GDR/slice cells and six new C V3 == CLI cells:
  CABAC idc 2 18,046 bytes; constraint `0x003f0025` 17,977; base PID 5
  17,977; Main forced to CAVLC 20,263; High without 8x8 17,979; deblock
  `1:-2:3` 18,098.
- RK3588 ABI, install/package, pkg-config, CMake shared/static and
  `rk-mpp-rs-sys` native smoke all pass. Host workspace gate is
  `1366 passed / 0 failed / 47 ignored`.
- The internal variadic logger accessors were renamed to
  `rk_mpprs_internal_*` so they no longer leak into the public `rk_mpp_*`
  symbol namespace.

### Phase 22: Public H.265 Syntax ABI
- **Status:** complete on RK3588.
- Reusing the V1.13 V3 trailing reserve for a 32-byte H.265 syntax descriptor
  preserves the 552-byte structure and all V1.13 offsets.
- Added field-selective SAO luma/chroma, deblock disable/beta/tc,
  loop-filter across slices/tiles and auto-tile controls to C and raw Rust.
- RK3588 passes the old V1.9/V1.10 gates plus six H.264 and three H.265
  C V3 == CLI cells. H.265 sizes are 15,618 (SAO), 15,878 (deblock) and
  15,829 (loop-filter).
- V1.14 ABI/package passes with 41 symbols and installs `.so.1.14.0`;
  `rk-mpp-rs-sys` native smoke passes.

### Phase 23: H.265 Auto-Tile Hardware Closure
- **Status:** complete on RK3588.
- A 320x240 fixture cannot exercise the VEPU580 auto-tile decision. Use the
  existing 4608x64 two-column shape and compare C V3 against CLI.
- The 4608x64 one-frame C V3 and CLI outputs are byte-identical: 8,876 bytes,
  two packets.

### Phase 24: Bounded-Memory H.264 Stream CLI
- **Status:** complete on RK3588.
- Current 50 MiB / 742-frame baseline: about 55.8 MiB peak RSS and
  1.57 s wall time over three samples.
- Current `test720.264` output hashes are planar
  `f31d2c0321ed82f6f2d4da5fed36bdce53ee355773879877de93138bf1500582`
  and NV12 `f14c7ca6b599c8930fab66f5fb4e4656e488b608bfc583aaeba919d1681cd6e2`.
- Final 4 MiB-chunk result: 742/742 frames; warm wall median
  1.576 -> 1.595 s; peak RSS 55.9 -> 17.0 MiB; planar/NV12 hashes unchanged.
- Moved the incremental scanner next to `split_access_units`, refreshed SDK
  H.264 reorder depth after SPS parsing and exposed the current depth through
  `Decoder::max_num_reorder_frames()`.
- Final host workspace gate: `1366 passed / 0 failed / 47 ignored`; rustfmt,
  diff check and strict rustdoc pass.

### Phase 25: Bounded-Memory Main `h264dec`
- **Status:** complete on RK3588.
- The primary CLI now uses the shared 4 MiB scanner while keeping its depth-3
  submission ring.
- Three interleaved 50 MiB / 742-frame pairs: wall median 0.665 -> 0.665 s,
  peak RSS 59.5 -> 20.4 MiB. The command reports 742 access units and
  742/742 output; 720p planar/NV12 hashes are unchanged.
- Final host workspace gate: `1366 passed / 0 failed / 47 ignored`; rustfmt,
  diff check and strict rustdoc pass.

## Session: 2026-11-09

### Phase 20: Official `mpi_enc_test` Object Bridge
- **Status:** RK3588 H.264/H.265/MJPEG object path matches; MJPEG also passes on RK3576.
- `MPP_ENC_SET_CFG` reads vendor `MppEncCfg` through `mpp_enc_cfg_get_s32`,
  maps it to the existing stable encoder API, and handles official header/RC
  control commands.
- `encode_put_frame` submits packed NV12 from an opaque `MppFrame`;
  `encode_get_packet` returns owned `MppPacket` data.
- RK3588 H.265 and MJPEG one-frame streams are byte-identical to vendor C MPP.
  H.264 six-frame SPS/PPS/rc-SEI/VCL and decoded reconstruction match; only the
  build-identity version SEI text differs.
- RK3576 MJPEG is byte-identical. RK3576 H.264/H.265 official parameter coverage
  remains explicitly skipped.

### Phase 19: Official `mpi_dec_test` Compatibility
- **Status:** decoder compatibility partial on RK3588 and RK3576.
- The unchanged vendor `mpi_dec_test` binary runs through `LD_PRELOAD`, with
  public MPP entry points from rk-mpp-rs and vendor log/cfg/OSAL helpers from
  the native library.
- H.264 640x480 6-frame B-frame, H.265 320x240 and MJPEG 320x240 all match the
  native output byte for byte on both boards.
- A minimal packet `MppMeta` preserves `KEY_OUTPUT_FRAME`, so advanced MJPEG
  returns the exact preallocated `MppFrame` and no longer logs an identity
  mismatch.
- Standalone linking without vendor helpers and `mpi_enc_test` remain.

### Phase 18: Official MPP Drop-in Lifecycle Foundation
- **Status:** lifecycle plus CPU decode data path complete on RK3588 and RK3576.
- Exported `mpp_create`, `mpp_init`, `mpp_destroy`,
  `mpp_check_support_format`, `mpp_show_support_format` and
  `mpp_show_color_format`.
- The exported `MppApi` table matches the vendor header's 184-byte aarch64
  layout and exposes deterministic stub functions for all data/control slots.
- A C consumer compiled against the vendor `rk_mpi.h` verifies null handling,
  supported coding types, init/reset/control contracts and destroy ownership on
  both boards. `scripts/board_mpp_compat_lifecycle.sh` is the gate.
- `MppPacket`, `MppFrame`, `MppBuffer` and H.264/H.265/MJPEG CPU NV12
  `decode_put_packet/decode_get_frame` now run through
  `scripts/board_mpp_compat_data.sh`; all three outputs match C MPP on RK3588 and
  RK3576. Meta, imported DMABUF buffers and encoder object flow remain.

### Phase 17: W8-2 Decode-to-Model-Input Direct Scaling
- **Status:** complete on RK3588 and RK3576.
- `Scaler::letterbox_nv12_strided()` and `Frame::letterbox_nv12()` now read the
  decoded strided Y/UV planes directly and perform bilinear scaling plus BT.601
  RGB conversion in one output pass.
- CPU-backed and DMA-BUF-backed H.264 frames produce byte-identical 640x640 RGB
  model input for a real 1280x720 stream on both boards. No whole-frame NV12 or
  RGB intermediate allocation is required.
- `scripts/board_sdk_decode_scale.sh` locks the real decode→scale path.
- 1080p→640 direct scaling measures 4.93 ms on RK3588 and 5.95 ms on RK3576, versus
  20.32/24.36 ms for full-frame NV12→RGB plus RGB scale.
- Workspace gates pass on both boards after adding two NV12 scaling tests:
  `1201 passed / 0 failed / 38 ignored`; rustfmt and strict rustdoc are clean.

### Phase 16: Encoder DMA-BUF Input API V1.12
- **Status:** complete on RK3588 and RK3576 for NV12 H.264/H.265/JPEG.
- Added `DmaBufInput`, `Encoder::submit_dmabuf_frame()` and
  `rk_mpp_encoder_submit_frame_dmabuf()`. The fd is imported for the synchronous
  encode call; the original copied-input API remains unchanged.
- VEPU580 H.264/H.265, VEPU510 H.264/H.265, VEPU2 JPEG and VPU720 JPEG now
  accept the external surface directly. Stride validation requires the coded
  16-aligned video layout or JPEG's 8x2 alignment.
- RK3588 and RK3576 pass `scripts/board_sdk_encoder_dmabuf.sh`: copied and fd input
  produce byte-identical H.264, H.265 and JPEG output on both boards.
- RK3588 C/C++ API gate includes fd-input runs and remains byte-identical to the C
  MPP/CLI oracles for H.264, H.265 and JPEG.
- The workspace gate is now reproducible via `scripts/board_workspace_gate.sh`;
  both boards pass `1199 passed / 0 failed / 38 ignored` with rustfmt and strict
  rustdoc.
- SDK V1.12 package, 36-symbol ABI snapshot, pkg-config, CMake and dynamic/static
  consumer gates pass on RK3588 and RK3576.

### Phase 15: AV1 DMA-BUF Frame Output
- **Status:** complete on RK3588 and RK3576 for 8-bit NV12.
- VDPU34x AV1 export clones the existing pooled post-processed display lease.
- VDPU383 AV1 output now comes from a reference-counted buffer pool; a slot is
  not reused while an SDK picture still owns it. DMA opt-in also skips the
  normal full-frame planar CPU readback.
- RK3588 AV1 DMA output matches the official C MPP NV12 oracle byte-for-byte with
  hash `7f4a0c9e…`. RK3588 and RK3576 both write two frames / 230,400 visible bytes,
  identical to their CPU paths.
- RK3576 VDPU383 retained its ten-frame 8-bit reference sequence and ten-frame
  10-bit FNV gate after the output-pool migration: both ignored hardware tests
  pass.
- Final RK3588/RK3576 workspace gates pass with
  `1199 passed / 0 failed / 38 ignored`; rustfmt, strict rustdoc, C/C++ oracle,
  SDK install/pkg-config/CMake and dynamic/static package gates pass.

### Phase 14: VP9 DMA-BUF Frame Output
- **Status:** complete on RK3588 and RK3576 for 8-bit NV12.
- Ordinary VP9 display pictures and `show_existing_frame` now retain independent
  leases on the pooled hardware video surface.
- RK3588 VP9 DMA output matches the official C MPP NV12 oracle byte-for-byte with
  hash `c5381bbf…`. RK3588 and RK3576 both write 12 frames / 1,382,400 visible bytes,
  identical to the default CPU path.
- The IVF C consumer now writes visible rows using plane stride instead of
  treating the padded DMA allocation as a tightly packed plane.
- VP9 10-bit remains on CPU I420P10 output and rejects DMA opt-in.
- Final RK3588/RK3576 workspace gates pass with
  `1199 passed / 0 failed / 38 ignored`; rustfmt, strict rustdoc and SDK
  install/pkg-config/CMake/dynamic/static package gates pass.

### Phase 13: H.265 DMA-BUF Frame Output
- **Status:** complete on RK3588 and RK3576 for 8-bit 4:2:0 NV12.
- H.264 and H.265 now share a pooled `DmaBufSurface`; H.265 reordered pictures
  retain an independent lease on the hardware video surface.
- RK3588 H.265 DMA output matches the official C MPP NV12 oracle byte-for-byte with
  hash `be78158b…`. RK3576 VDPU383 DMA and default CPU outputs match at four
  frames / 460,800 bytes.
- H.264, H.265 and MJPEG pass the shared DMA-BUF import/export gate on both
  boards; CPU frames still reject fd export.
- H.265 Main10 and 422/444 remain on CPU planar output and explicitly reject
  the DMA output flag.
- Final RK3588/RK3576 workspace gates pass with
  `1199 passed / 0 failed / 38 ignored`; rustfmt, strict rustdoc, SDK install,
  pkg-config, CMake and dynamic/static ABI package gates pass.

### Phase 12: H.264 DMA-BUF Frame Output
- **Status:** complete on RK3588 and RK3576.
- H.264 display-reordered pictures now clone their DPB buffer lease and retain
  it until SDK frame release.
- Slot reuse detects an extra nv12 lease reference and acquires a fresh pooled
  buffer instead of overwriting a pending display picture.
- RK3588 H.264 opt-in DMA output matches the C oracle byte-for-byte with hash
  `338214ea…`; RK3576 DMA and default CPU outputs match each other.
- MJPEG opt-in/default and H.264 opt-in/default paths pass the shared DMA-BUF
  board gate on both boards.
- Final RK3588/RK3576 workspace gates pass with
  `1199 passed / 0 failed / 38 ignored`; rustfmt and strict rustdoc are clean.
- SDK install, SONAME/symbol, pkg-config, CMake, dynamic-link and static-link
  package gates pass on both boards.

### Phase 11: DMA-BUF Frame Export
- **Status:** complete on RK3588 and RK3576 for MJPEG NV12.
- Added `DmaBuf::import_fd`, `dup_fd` and `try_clone` with owned close/mmap
  semantics.
- MJPEG NV12 frames can opt into retaining the hardware dma-buf instead of
  copying to a CPU `Vec`; default frames remain contiguous CPU storage and
  cloning a DMA frame deliberately produces an independent CPU copy.
- Rust exposes `DmaBufPlane` and `Frame::duplicate_dmabuf`; C exposes
  `rk_mpp_decoder_export_frame_dmabuf()` with caller-owned fd.
- RK3588/RK3576 passed dma-heap duplicate/import byte equality and C API Y/UV
  fd/offset/size/stride validation.
- RK3588 full C/C++ decoder+encoder gate retained the MJPEG NV12 hash
  `05e4894e…`.
- Workspace gates pass on both boards with
  `1199 passed / 0 failed / 38 ignored`; workspace rustfmt/rustdoc pass.

### Phase 10: rk-mpp-rs-sys Raw Bindings
- **Status:** complete on RK3588 and RK3576.
- Added the `rk-mpp-rs-sys` workspace crate with hand-written V1.10 enums,
  constants, fixed-layout structs, opaque handles and all 34 exported C
  functions.
- Added an offline build script supporting explicit library directories,
  installed pkg-config metadata, workspace target fallback, shared linking and
  static linking.
- RK3588 and RK3576 pass ABI layout, native symbol-linkability and initializer smoke
  tests through explicit dynamic, static and pkg-config consumers.
- Workspace `--all-targets` gates pass on both boards with
  `1197 passed / 0 failed / 37 ignored` (root 1193 + sys 4); workspace rustfmt
  and strict rustdoc pass.

### Phase 9.2: ROI, GDR and Slice Controls
- **Status:** complete on RK3588/VEPU580; RK3576 streaming controls are an explicit
  platform SKIP.
- Added `rk_mpp_encoder_advanced_config_v2_t` without changing the V1.9
  104-byte advanced config or its initializer.
- Added typed Rust `RoiConfig`, `GdrConfig` and `SliceSplitConfig`, plus C ROI,
  GDR and slice descriptors with fixed-size copied ROI storage.
- RK3588 H.264/H.265 slice-lowdelay, ROI and GDR C API outputs matched their
  validated CLI paths byte-for-byte in six cases.
- RK3588 V1.8 base and V1.9 advanced consumers ran against the V1.10 library.
  RK3576 compiled the V1.9 consumer and reported the VEPU510 runtime control
  boundary as SKIP.
- RK3588 and RK3576 package gates and final root `--all-targets` gates passed with
  `1193 passed / 0 failed / 37 ignored`; rustfmt and strict rustdoc passed.

### Phase 9.1: Stable Rate-Control Configuration
- **Status:** complete on RK3588/VEPU580; RK3576 streaming RC explicitly unsupported
- Kept the V1.8 68-byte encoder config and `rk_mpp_encoder_create()` unchanged.
- Added separately versioned `rk_mpp_encoder_rate_control_t` and
  `rk_mpp_encoder_advanced_config_t`, plus `create_advanced()` and C/Rust
  Fixed-QP/CBR/VBR/AVBR mapping for bitrate and QP bounds.
- RK3588 V1.8 consumer compatibility passed against the V1.9 library.
- RK3588 H.264 and H.265 CBR/VBR/AVBR C API outputs matched the validated CLI in
  all six cases, byte-for-byte.
- RK3576's V1.8 consumer passed and advanced streaming RC was an explicit platform
  SKIP. A transient rustc SIGSEGV in an earlier retry did not recur with
  `CARGO_BUILD_JOBS=1`.
- RK3588 and RK3576 package gates passed with API `1.9`, pkg-config, CMake
  shared/static consumers and ABI checks.
- Final RK3588 and RK3576 `cargo test --all-targets --offline` gates each passed
  `1190 / 0 / 37`; `cargo fmt --check` and strict rustdoc passed on both.

### Phase 8: Product-Ready SDK Packaging
- **Status:** complete on RK3576 and RK3588
- Confirmed both boards have pkg-config, CMake and readelf.
- Confirmed the current build produces working libraries but no SONAME,
  install layout, package metadata or checked ABI snapshot.
- Chose Linux ABI major 1 for SONAME and independently versioned shared-library
  filename, with relocatable pkg-config/CMake consumers.
- Added `librk_mpp_rs.so.1` SONAME and `.so.1.8.0` install naming.
- Added pkg-config, relocatable CMake shared/static targets, ABI symbol and
  C11 layout checks, install script, Rust examples, CHANGELOG, NOTICE and
  dual-license texts.
- RK3576/RK3588 package gates pass, including external pkg-config/CMake consumers and
  real H.264 decode/encode examples.

| Test | Expected | Actual | Status |
|---|---|---|---|
| SDK package gate RK3576 | SONAME/install/ABI/consumers | all pass; Rust examples decode/encode 2 frames | PASS |
| SDK package gate RK3588 | SONAME/install/ABI/consumers | all pass; Rust examples decode/encode 2 frames | PASS |
| Full suite after packaging | no regression | 1188 passed / 0 failed / 37 ignored; RK3588 lib 1044/0/0 | PASS |

## Session: 2026-11-08

### Phase 6.1: H.265 Public Decoder
- **Status:** complete on RK3576 and RK3588
- Chose H.265 next because the hardware decoder is already byte-validated and
  it reuses the persistent packet/frame ABI.
- Audited the HEVC decoder, parser, display-order and board test paths.
- Identified AU prefix carryover, bounded reorder and 10-bit format extension
  as the required API work.
- Added `RK_MPP_CODEC_H265`, `RK_MPP_PIXEL_FORMAT_I420P10` and ABI 1.1.
- Added SPS `max_num_reorder_pics` tracking and an epoch/POC streaming queue.
- H.265 8-bit NV12/I420 and Main10 I420P10 C/C++ gates pass byte-for-byte.

### Phase 1: API Contract and Discovery
- **Status:** complete
- Started with the existing committed baseline and audited public exports.
- Confirmed the current C ABI is JPEG-only and file-path bound.
- Confirmed H.264 already has reusable packet/AU, reorder and recovery logic.
- Confirmed JPEG has reusable in-memory parser and hardware job builder.

### Phases 2-5: H.264/MJPEG SDK
- **Status:** complete on RK3576 and RK3588
- Added versioned `rk_mpp_*` V1 C ABI and public Rust `sdk` module.
- Builds `cdylib`, `staticlib` and `rlib`.
- H.264 accepts arbitrary Annex-B chunks, copies input, reorders by POC and
  emits NV12/I420 with timestamps and status metadata.
- MJPEG accepts fragmented or concatenated JPEGs and emits deterministic
  NV12/I420.
- Added flush, reset, recover, status, sticky EOS and explicit frame release.
- Preserved the legacy path-based `rk_jpeg_*` ABI for `rknn-fastinfer`.

### Test Results

| Test | Expected | Actual | Status |
|------|----------|--------|--------|
| Source audit | No generic public API exists | Only legacy `rk_jpeg_*` | confirmed |
| RK3576 shared C ABI | H.264/MJPEG C harness passes | 6/6 H.264 and 4/4 MJPEG frames | PASS |
| RK3576 C++ shared/static link | Both consumers run | Both smoke tests pass | PASS |
| RK3588 complete suite @ 80% OPP | Full suite green | 1183 passed / 0 failed / 37 ignored; lib 1039/0/0 | PASS |
| RK3588 C/C++ SDK gate | Dynamic/static link and byte equality | Shared and static libraries, C and C++, all checks pass | PASS |
| H.264 arbitrary chunks | 7, 127 and 313-byte chunks match C | All outputs byte-identical | PASS |
| H.264 whole stream in one packet | Match C | 6 frames, byte-identical | PASS |
| MJPEG whole stream in one packet | Match C | 4 frames, byte-identical | PASS |
| Lifecycle failures | Errors do not corrupt session | null, bad struct, double receive, post-EOS and reset checks pass | PASS |
| Malformed MJPEG recovery | Parse error then reusable session | parse + recover + full four-frame reuse pass | PASS |
| Strict rustdoc | No broken public links | `RUSTDOCFLAGS=-D warnings cargo doc --no-deps` pass | PASS |
| H.265 full gates | RK3576/RK3588 all-targets green | 1186 passed / 0 failed / 37 ignored; RK3588 lib 1042/0/0 | PASS |
| H.265 C ABI | NV12/I420/I420P10 byte equality | RK3576 and RK3588 shared/static C/C++ gates pass | PASS |
| VP9 full gates | RK3576/RK3588 all-targets green | 1186 passed / 0 failed / 37 ignored; RK3588 lib 1042/0/0 | PASS |
| VP9 C ABI | Profile 0/2 packet decode | 12-frame NV12/I420 and 10-bit I420P10 byte equality on RK3576/RK3588 | PASS |
| AV1 C ABI | RK3588 and RK3576 AV1 paths | 8-bit key+P and 10-bit temporal-unit outputs pass on RK3576/RK3588 | PASS |
| AV1 full gates | RK3576/RK3588 all-targets green | 1186 passed / 0 failed / 37 ignored; RK3588 lib 1042/0/0 | PASS |
| H.264 encoder API | VEPU580 and VEPU510 | 6-frame NV12/I420 output matches CLI oracle on RK3576/RK3588 | PASS |
| H.264 encoder full gates | RK3576/RK3588 all-targets green | 1188 passed / 0 failed / 37 ignored; RK3588 lib 1044/0/0 | PASS |
| H.265 encoder API | VEPU580 and VEPU510 | 6-frame NV12/I420 output matches CLI oracle on RK3576/RK3588 | PASS |
| H.265 encoder full gates | RK3576/RK3588 all-targets green | 1188 passed / 0 failed / 37 ignored; RK3588 lib 1044/0/0 | PASS |
| JPEG encoder API | VEPU2 and VPU720 | NV12/I420 single-frame output matches CLI on RK3576/RK3588 | PASS |
| JPEG restart API | VEPU2 and VPU720 | restart MCU 20 output matches CLI on RK3576/RK3588 | PASS |
| JPEG encoder full gates | RK3576/RK3588 all-targets green | 1188 passed / 0 failed / 37 ignored; RK3588 lib 1044/0/0 | PASS |
| VP8 decoder API | RK3588 packet/alt-ref path | 12-frame NV12/I420 output matches C | PASS |
| VP8 on RK3576 | C MPP platform capability | both C and Rust reject missing decoder | SKIP (hardware absent) |
| VP8 full gates | RK3576/RK3588 all-targets green | 1188 passed / 0 failed / 37 ignored; RK3588 lib 1044/0/0 | PASS |
| MPEG-2/4/H.263 API | RK3588 VDPU2 representative streams | 10/12/12-frame NV12/I420 output matches old C oracle | PASS |
| MPEG-2/4/H.263 on RK3576 | RK3576 platform capability | no VDPU2 decoder; platform SKIP | SKIP |
| Legacy decoder full gates | RK3576/RK3588 all-targets green | 1188 passed / 0 failed / 37 ignored; RK3588 lib 1044/0/0 | PASS |

### Phase 6.2: VP9 Public Decoder
- **Status:** complete on RK3576 and RK3588
- VP9 decoder already returns display-ready pictures per coded packet.
- The SDK will accept one VP9 packet (possibly a superframe) per submit and
  leave IVF/WebM demux to the caller.
- Need board gates for profile 0 8-bit and profile 2 10-bit.
- Added `RK_MPP_CODEC_VP9` and API version 1.2.
- Profile 0 12-frame NV12/I420 and profile 2 10-bit I420P10 outputs match C
  MPP byte for byte; reset/reuse and post-EOS checks pass.

### Phase 6.3: AV1 Public Decoder
- **Status:** complete on RK3576 and RK3588
- Existing AV1 decoder already returns display-ready pictures from one temporal
  unit, including show-existing and multiple final frames.
- The C gate will reuse the IVF packet harness with `AV01` validation.
- Added `RK_MPP_CODEC_AV1` and API version 1.3.
- RK3576 uses the new VDPU383 decoder front-end; RK3588 uses the existing AV1D_VPU
  backend through the same SDK handle.
- 8-bit key+P output matches C; RK3588 10-bit output matches the Rust/libaom
  grain-applied oracle, retaining the documented C film-grain bug.

### Phase 6.4.1: H.264 Public Encoder
- **Status:** complete on RK3576 and RK3588 for exposed paths
- The existing VEPU580 H.264 path is synchronous and already returns complete
  packet boundaries, so the public handle can expose direct submit/receive
  semantics with copied input.
- Initial configuration covers NV12/I420, baseline/main/high, level, GOP, QP,
  frame rate and automatic SPS/PPS on IDR.
- Added API version 1.4, owned packet queue/release, flush/reset and C ABI.
- RK3588 VEPU580 and RK3576 VEPU510 six-frame outputs match their respective CLI
  oracles, SHA-256 `aa6276ba…173d1` and `0e65b51a…81f33`; NV12 and I420
  inputs match each other on each board.

### Phase 6.4.2: H.265 Public Encoder
- **Status:** complete on RK3576 and RK3588 for exposed paths
- Added H.265 to the shared encoder handle and API version 1.5.
- RK3588 VEPU580 supports width/height, QP and GOP; RK3576 VEPU510 exposes the
  captured 320x240 QP26 GOP6 path.
- Six-frame NV12/I420 output matches platform CLI oracles, SHA-256
  `6c285115…979e9` on RK3588 and `8088e821…7fe397` on RK3576.

### Phase 6.4.3: JPEG Public Encoder
- **Status:** complete on RK3576 and RK3588
- Added JPEG to the shared encoder handle, quality 0–10 and restart MCU
  configuration, with API version 1.6.
- VEPU2/VPU720 is selected automatically; NV12 and I420 produce identical
  JPEG bytes. Hashes are `4f571054…7db1e` on RK3588 and `64153672…7ac69` on RK3576.
- Restart interval 20 also matches on both boards: RK3576
  `0ccf41ea…82997`, RK3588 `25254033…df748a`.

### Phase 6.5.1: VP8 Public Decoder
- **Status:** in_progress
- Audited the existing packet decoder and its hidden-alt-ref display policy.
- The shared handle can expose one packet per submit with NV12/I420 output;
  status remains unsupported because the VP8 HAL exposes only `last_irq`.

### Errors

| Error | Resolution |
|-------|------------|
| JPEG hardware RGB reuse | Removed from V1; deterministic NV12/I420 only |
| SPS/PPS between H.264 pictures | Attached to the next AU and covered by Rust tests |
