# Progress Log

## Session: 2026-11-09

### Phase 17: W8-2 Decode-to-Model-Input Direct Scaling
- **Status:** complete on t6 and lf1.
- `Scaler::letterbox_nv12_strided()` and `Frame::letterbox_nv12()` now read the
  decoded strided Y/UV planes directly and perform bilinear scaling plus BT.601
  RGB conversion in one output pass.
- CPU-backed and DMA-BUF-backed H.264 frames produce byte-identical 640x640 RGB
  model input for a real 1280x720 stream on both boards. No whole-frame NV12 or
  RGB intermediate allocation is required.
- `scripts/board_sdk_decode_scale.sh` locks the real decode→scale path.
- 1080p→640 direct scaling measures 4.93 ms on t6 and 5.95 ms on lf1, versus
  20.32/24.36 ms for full-frame NV12→RGB plus RGB scale.
- Workspace gates pass on both boards after adding two NV12 scaling tests:
  `1201 passed / 0 failed / 38 ignored`; rustfmt and strict rustdoc are clean.

### Phase 16: Encoder DMA-BUF Input API V1.12
- **Status:** complete on t6 and lf1 for NV12 H.264/H.265/JPEG.
- Added `DmaBufInput`, `Encoder::submit_dmabuf_frame()` and
  `rk_mpp_encoder_submit_frame_dmabuf()`. The fd is imported for the synchronous
  encode call; the original copied-input API remains unchanged.
- VEPU580 H.264/H.265, VEPU510 H.264/H.265, VEPU2 JPEG and VPU720 JPEG now
  accept the external surface directly. Stride validation requires the coded
  16-aligned video layout or JPEG's 8x2 alignment.
- t6 and lf1 pass `scripts/board_sdk_encoder_dmabuf.sh`: copied and fd input
  produce byte-identical H.264, H.265 and JPEG output on both boards.
- t6 C/C++ API gate includes fd-input runs and remains byte-identical to the C
  MPP/CLI oracles for H.264, H.265 and JPEG.
- The workspace gate is now reproducible via `scripts/board_workspace_gate.sh`;
  both boards pass `1199 passed / 0 failed / 38 ignored` with rustfmt and strict
  rustdoc.
- SDK V1.12 package, 36-symbol ABI snapshot, pkg-config, CMake and dynamic/static
  consumer gates pass on t6 and lf1.

### Phase 15: AV1 DMA-BUF Frame Output
- **Status:** complete on t6 and lf1 for 8-bit NV12.
- VDPU34x AV1 export clones the existing pooled post-processed display lease.
- VDPU383 AV1 output now comes from a reference-counted buffer pool; a slot is
  not reused while an SDK picture still owns it. DMA opt-in also skips the
  normal full-frame planar CPU readback.
- t6 AV1 DMA output matches the official C MPP NV12 oracle byte-for-byte with
  hash `7f4a0c9e…`. t6 and lf1 both write two frames / 230,400 visible bytes,
  identical to their CPU paths.
- lf1 VDPU383 retained its ten-frame 8-bit reference sequence and ten-frame
  10-bit FNV gate after the output-pool migration: both ignored hardware tests
  pass.
- Final t6/lf1 workspace gates pass with
  `1199 passed / 0 failed / 38 ignored`; rustfmt, strict rustdoc, C/C++ oracle,
  SDK install/pkg-config/CMake and dynamic/static package gates pass.

### Phase 14: VP9 DMA-BUF Frame Output
- **Status:** complete on t6 and lf1 for 8-bit NV12.
- Ordinary VP9 display pictures and `show_existing_frame` now retain independent
  leases on the pooled hardware video surface.
- t6 VP9 DMA output matches the official C MPP NV12 oracle byte-for-byte with
  hash `c5381bbf…`. t6 and lf1 both write 12 frames / 1,382,400 visible bytes,
  identical to the default CPU path.
- The IVF C consumer now writes visible rows using plane stride instead of
  treating the padded DMA allocation as a tightly packed plane.
- VP9 10-bit remains on CPU I420P10 output and rejects DMA opt-in.
- Final t6/lf1 workspace gates pass with
  `1199 passed / 0 failed / 38 ignored`; rustfmt, strict rustdoc and SDK
  install/pkg-config/CMake/dynamic/static package gates pass.

### Phase 13: H.265 DMA-BUF Frame Output
- **Status:** complete on t6 and lf1 for 8-bit 4:2:0 NV12.
- H.264 and H.265 now share a pooled `DmaBufSurface`; H.265 reordered pictures
  retain an independent lease on the hardware video surface.
- t6 H.265 DMA output matches the official C MPP NV12 oracle byte-for-byte with
  hash `be78158b…`. lf1 VDPU383 DMA and default CPU outputs match at four
  frames / 460,800 bytes.
- H.264, H.265 and MJPEG pass the shared DMA-BUF import/export gate on both
  boards; CPU frames still reject fd export.
- H.265 Main10 and 422/444 remain on CPU planar output and explicitly reject
  the DMA output flag.
- Final t6/lf1 workspace gates pass with
  `1199 passed / 0 failed / 38 ignored`; rustfmt, strict rustdoc, SDK install,
  pkg-config, CMake and dynamic/static ABI package gates pass.

### Phase 12: H.264 DMA-BUF Frame Output
- **Status:** complete on t6 and lf1.
- H.264 display-reordered pictures now clone their DPB buffer lease and retain
  it until SDK frame release.
- Slot reuse detects an extra nv12 lease reference and acquires a fresh pooled
  buffer instead of overwriting a pending display picture.
- t6 H.264 opt-in DMA output matches the C oracle byte-for-byte with hash
  `338214ea…`; lf1 DMA and default CPU outputs match each other.
- MJPEG opt-in/default and H.264 opt-in/default paths pass the shared DMA-BUF
  board gate on both boards.
- Final t6/lf1 workspace gates pass with
  `1199 passed / 0 failed / 38 ignored`; rustfmt and strict rustdoc are clean.
- SDK install, SONAME/symbol, pkg-config, CMake, dynamic-link and static-link
  package gates pass on both boards.

### Phase 11: DMA-BUF Frame Export
- **Status:** complete on t6 and lf1 for MJPEG NV12.
- Added `DmaBuf::import_fd`, `dup_fd` and `try_clone` with owned close/mmap
  semantics.
- MJPEG NV12 frames can opt into retaining the hardware dma-buf instead of
  copying to a CPU `Vec`; default frames remain contiguous CPU storage and
  cloning a DMA frame deliberately produces an independent CPU copy.
- Rust exposes `DmaBufPlane` and `Frame::duplicate_dmabuf`; C exposes
  `rk_mpp_decoder_export_frame_dmabuf()` with caller-owned fd.
- t6/lf1 passed dma-heap duplicate/import byte equality and C API Y/UV
  fd/offset/size/stride validation.
- t6 full C/C++ decoder+encoder gate retained the MJPEG NV12 hash
  `05e4894e…`.
- Workspace gates pass on both boards with
  `1199 passed / 0 failed / 38 ignored`; workspace rustfmt/rustdoc pass.

### Phase 10: rk-mpp-rs-sys Raw Bindings
- **Status:** complete on t6 and lf1.
- Added the `rk-mpp-rs-sys` workspace crate with hand-written V1.10 enums,
  constants, fixed-layout structs, opaque handles and all 34 exported C
  functions.
- Added an offline build script supporting explicit library directories,
  installed pkg-config metadata, workspace target fallback, shared linking and
  static linking.
- t6 and lf1 pass ABI layout, native symbol-linkability and initializer smoke
  tests through explicit dynamic, static and pkg-config consumers.
- Workspace `--all-targets` gates pass on both boards with
  `1197 passed / 0 failed / 37 ignored` (root 1193 + sys 4); workspace rustfmt
  and strict rustdoc pass.

### Phase 9.2: ROI, GDR and Slice Controls
- **Status:** complete on t6/VEPU580; RK3576 streaming controls are an explicit
  platform SKIP.
- Added `rk_mpp_encoder_advanced_config_v2_t` without changing the V1.9
  104-byte advanced config or its initializer.
- Added typed Rust `RoiConfig`, `GdrConfig` and `SliceSplitConfig`, plus C ROI,
  GDR and slice descriptors with fixed-size copied ROI storage.
- t6 H.264/H.265 slice-lowdelay, ROI and GDR C API outputs matched their
  validated CLI paths byte-for-byte in six cases.
- t6 V1.8 base and V1.9 advanced consumers ran against the V1.10 library.
  lf1 compiled the V1.9 consumer and reported the VEPU510 runtime control
  boundary as SKIP.
- t6 and lf1 package gates and final root `--all-targets` gates passed with
  `1193 passed / 0 failed / 37 ignored`; rustfmt and strict rustdoc passed.

### Phase 9.1: Stable Rate-Control Configuration
- **Status:** complete on t6/VEPU580; RK3576 streaming RC explicitly unsupported
- Kept the V1.8 68-byte encoder config and `rk_mpp_encoder_create()` unchanged.
- Added separately versioned `rk_mpp_encoder_rate_control_t` and
  `rk_mpp_encoder_advanced_config_t`, plus `create_advanced()` and C/Rust
  Fixed-QP/CBR/VBR/AVBR mapping for bitrate and QP bounds.
- t6 V1.8 consumer compatibility passed against the V1.9 library.
- t6 H.264 and H.265 CBR/VBR/AVBR C API outputs matched the validated CLI in
  all six cases, byte-for-byte.
- lf1's V1.8 consumer passed and advanced streaming RC was an explicit platform
  SKIP. A transient rustc SIGSEGV in an earlier retry did not recur with
  `CARGO_BUILD_JOBS=1`.
- t6 and lf1 package gates passed with API `1.9`, pkg-config, CMake
  shared/static consumers and ABI checks.
- Final t6 and lf1 `cargo test --all-targets --offline` gates each passed
  `1190 / 0 / 37`; `cargo fmt --check` and strict rustdoc passed on both.

### Phase 8: Product-Ready SDK Packaging
- **Status:** complete on lf1 and t6
- Confirmed both boards have pkg-config, CMake and readelf.
- Confirmed the current build produces working libraries but no SONAME,
  install layout, package metadata or checked ABI snapshot.
- Chose Linux ABI major 1 for SONAME and independently versioned shared-library
  filename, with relocatable pkg-config/CMake consumers.
- Added `librk_mpp_rs.so.1` SONAME and `.so.1.8.0` install naming.
- Added pkg-config, relocatable CMake shared/static targets, ABI symbol and
  C11 layout checks, install script, Rust examples, CHANGELOG, NOTICE and
  dual-license texts.
- lf1/t6 package gates pass, including external pkg-config/CMake consumers and
  real H.264 decode/encode examples.

| Test | Expected | Actual | Status |
|---|---|---|---|
| SDK package gate lf1 | SONAME/install/ABI/consumers | all pass; Rust examples decode/encode 2 frames | PASS |
| SDK package gate t6 | SONAME/install/ABI/consumers | all pass; Rust examples decode/encode 2 frames | PASS |
| Full suite after packaging | no regression | 1188 passed / 0 failed / 37 ignored; t6 lib 1044/0/0 | PASS |

## Session: 2026-11-08

### Phase 6.1: H.265 Public Decoder
- **Status:** complete on lf1 and t6
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
- **Status:** complete on lf1 and t6
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
| lf1 shared C ABI | H.264/MJPEG C harness passes | 6/6 H.264 and 4/4 MJPEG frames | PASS |
| lf1 C++ shared/static link | Both consumers run | Both smoke tests pass | PASS |
| t6 complete suite @ 80% OPP | Full suite green | 1183 passed / 0 failed / 37 ignored; lib 1039/0/0 | PASS |
| t6 C/C++ SDK gate | Dynamic/static link and byte equality | Shared and static libraries, C and C++, all checks pass | PASS |
| H.264 arbitrary chunks | 7, 127 and 313-byte chunks match C | All outputs byte-identical | PASS |
| H.264 whole stream in one packet | Match C | 6 frames, byte-identical | PASS |
| MJPEG whole stream in one packet | Match C | 4 frames, byte-identical | PASS |
| Lifecycle failures | Errors do not corrupt session | null, bad struct, double receive, post-EOS and reset checks pass | PASS |
| Malformed MJPEG recovery | Parse error then reusable session | parse + recover + full four-frame reuse pass | PASS |
| Strict rustdoc | No broken public links | `RUSTDOCFLAGS=-D warnings cargo doc --no-deps` pass | PASS |
| H.265 full gates | lf1/t6 all-targets green | 1186 passed / 0 failed / 37 ignored; t6 lib 1042/0/0 | PASS |
| H.265 C ABI | NV12/I420/I420P10 byte equality | lf1 and t6 shared/static C/C++ gates pass | PASS |
| VP9 full gates | lf1/t6 all-targets green | 1186 passed / 0 failed / 37 ignored; t6 lib 1042/0/0 | PASS |
| VP9 C ABI | Profile 0/2 packet decode | 12-frame NV12/I420 and 10-bit I420P10 byte equality on lf1/t6 | PASS |
| AV1 C ABI | RK3588 and RK3576 AV1 paths | 8-bit key+P and 10-bit temporal-unit outputs pass on lf1/t6 | PASS |
| AV1 full gates | lf1/t6 all-targets green | 1186 passed / 0 failed / 37 ignored; t6 lib 1042/0/0 | PASS |
| H.264 encoder API | VEPU580 and VEPU510 | 6-frame NV12/I420 output matches CLI oracle on lf1/t6 | PASS |
| H.264 encoder full gates | lf1/t6 all-targets green | 1188 passed / 0 failed / 37 ignored; t6 lib 1044/0/0 | PASS |
| H.265 encoder API | VEPU580 and VEPU510 | 6-frame NV12/I420 output matches CLI oracle on lf1/t6 | PASS |
| H.265 encoder full gates | lf1/t6 all-targets green | 1188 passed / 0 failed / 37 ignored; t6 lib 1044/0/0 | PASS |
| JPEG encoder API | VEPU2 and VPU720 | NV12/I420 single-frame output matches CLI on lf1/t6 | PASS |
| JPEG restart API | VEPU2 and VPU720 | restart MCU 20 output matches CLI on lf1/t6 | PASS |
| JPEG encoder full gates | lf1/t6 all-targets green | 1188 passed / 0 failed / 37 ignored; t6 lib 1044/0/0 | PASS |
| VP8 decoder API | t6 packet/alt-ref path | 12-frame NV12/I420 output matches C | PASS |
| VP8 on lf1 | C MPP platform capability | both C and Rust reject missing decoder | SKIP (hardware absent) |
| VP8 full gates | lf1/t6 all-targets green | 1188 passed / 0 failed / 37 ignored; t6 lib 1044/0/0 | PASS |
| MPEG-2/4/H.263 API | t6 VDPU2 representative streams | 10/12/12-frame NV12/I420 output matches old C oracle | PASS |
| MPEG-2/4/H.263 on lf1 | RK3576 platform capability | no VDPU2 decoder; platform SKIP | SKIP |
| Legacy decoder full gates | lf1/t6 all-targets green | 1188 passed / 0 failed / 37 ignored; t6 lib 1044/0/0 | PASS |

### Phase 6.2: VP9 Public Decoder
- **Status:** complete on lf1 and t6
- VP9 decoder already returns display-ready pictures per coded packet.
- The SDK will accept one VP9 packet (possibly a superframe) per submit and
  leave IVF/WebM demux to the caller.
- Need board gates for profile 0 8-bit and profile 2 10-bit.
- Added `RK_MPP_CODEC_VP9` and API version 1.2.
- Profile 0 12-frame NV12/I420 and profile 2 10-bit I420P10 outputs match C
  MPP byte for byte; reset/reuse and post-EOS checks pass.

### Phase 6.3: AV1 Public Decoder
- **Status:** complete on lf1 and t6
- Existing AV1 decoder already returns display-ready pictures from one temporal
  unit, including show-existing and multiple final frames.
- The C gate will reuse the IVF packet harness with `AV01` validation.
- Added `RK_MPP_CODEC_AV1` and API version 1.3.
- lf1 uses the new VDPU383 decoder front-end; t6 uses the existing AV1D_VPU
  backend through the same SDK handle.
- 8-bit key+P output matches C; t6 10-bit output matches the Rust/libaom
  grain-applied oracle, retaining the documented C film-grain bug.

### Phase 6.4.1: H.264 Public Encoder
- **Status:** complete on lf1 and t6 for exposed paths
- The existing VEPU580 H.264 path is synchronous and already returns complete
  packet boundaries, so the public handle can expose direct submit/receive
  semantics with copied input.
- Initial configuration covers NV12/I420, baseline/main/high, level, GOP, QP,
  frame rate and automatic SPS/PPS on IDR.
- Added API version 1.4, owned packet queue/release, flush/reset and C ABI.
- t6 VEPU580 and lf1 VEPU510 six-frame outputs match their respective CLI
  oracles, SHA-256 `aa6276ba…173d1` and `0e65b51a…81f33`; NV12 and I420
  inputs match each other on each board.

### Phase 6.4.2: H.265 Public Encoder
- **Status:** complete on lf1 and t6 for exposed paths
- Added H.265 to the shared encoder handle and API version 1.5.
- t6 VEPU580 supports width/height, QP and GOP; lf1 VEPU510 exposes the
  captured 320x240 QP26 GOP6 path.
- Six-frame NV12/I420 output matches platform CLI oracles, SHA-256
  `6c285115…979e9` on t6 and `8088e821…7fe397` on lf1.

### Phase 6.4.3: JPEG Public Encoder
- **Status:** complete on lf1 and t6
- Added JPEG to the shared encoder handle, quality 0–10 and restart MCU
  configuration, with API version 1.6.
- VEPU2/VPU720 is selected automatically; NV12 and I420 produce identical
  JPEG bytes. Hashes are `4f571054…7db1e` on t6 and `64153672…7ac69` on lf1.
- Restart interval 20 also matches on both boards: lf1
  `0ccf41ea…82997`, t6 `25254033…df748a`.

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
