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

### Phase 6.2: VP9 Public Decoder
- **Status:** complete on lf1 and t6
- VP9 decoder already returns display-ready pictures per coded packet.
- The SDK will accept one VP9 packet (possibly a superframe) per submit and
  leave IVF/WebM demux to the caller.
- Need board gates for profile 0 8-bit and profile 2 10-bit.
- Added `RK_MPP_CODEC_VP9` and API version 1.2.
- Profile 0 12-frame NV12/I420 and profile 2 10-bit I420P10 outputs match C
  MPP byte for byte; reset/reuse and post-EOS checks pass.

### Errors

| Error | Resolution |
|-------|------------|
| JPEG hardware RGB reuse | Removed from V1; deterministic NV12/I420 only |
| SPS/PPS between H.264 pictures | Attached to the next AU and covered by Rust tests |
