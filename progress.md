# Progress Log

## Session: 2026-11-08

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

### Errors

| Error | Resolution |
|-------|------------|
| JPEG hardware RGB reuse | Removed from V1; deterministic NV12/I420 only |
| SPS/PPS between H.264 pictures | Attached to the next AU and covered by Rust tests |
