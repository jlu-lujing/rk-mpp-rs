# Progress Log

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

### Errors

| Error | Resolution |
|-------|------------|
| JPEG hardware RGB reuse | Removed from V1; deterministic NV12/I420 only |
| SPS/PPS between H.264 pictures | Attached to the next AU and covered by Rust tests |
