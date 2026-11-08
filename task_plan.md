# Task Plan: Public Codec SDK

## Goal
Provide stable, versioned Rust and C library interfaces for hardware codec use,
prioritizing persistent H.264 and MJPEG decode sessions, then extending the
same contract to the other implemented codecs and encoders.

## Current Phase
Phases 1-5 for H.264/MJPEG are complete; Phase 6 moves on to the remaining
decoders and encoders.

## Phases

### Phase 1: API Contract and Discovery
- [x] Define ownership, packet/frame lifetime, error and flush semantics.
- [x] Audit H.264 and JPEG internal APIs for reusable primitives.
- [x] Add findings and API compatibility decisions.
- **Status:** complete

### Phase 2: Versioned Header and Shared Runtime
- [x] Add a C-compatible header with fixed-width structs and API version.
- [x] Add common packet/frame queues, errors and output-format metadata.
- [x] Export `cdylib` while preserving `rlib` and `staticlib`.
- **Status:** complete

### Phase 3: H.264 Decode Backend
- [x] Implement arbitrary-chunk Annex-B packet input and bounded reordering.
- [x] Expose NV12 and planar YUV output without file-path coupling.
- [x] Implement flush, reset, EOS, status and recovery.
- **Status:** complete

### Phase 4: MJPEG Decode Backend
- [x] Implement packet-stable JPEG framing and a persistent public session.
- [x] Expose deterministic NV12 and planar YUV output.
- [x] Cover concatenated packet input, EOS and malformed/state errors.
- **Status:** complete with fresh DMA/service resources per image to avoid the
  hardware postprocessor writeback race.

### Phase 5: C/C++ Integration Gates
- [x] Compile and run a C harness against the shared library/header.
- [x] Compile and run linked C++ smoke tests for shared and static libraries.
- [x] Verify H.264 and MJPEG outputs against existing board goldens.
- **Status:** complete on lf1; final t6 gate remains in Phase 7.

### Phase 6: Other Codec Interfaces
- [ ] Extend the decoder contract to HEVC, VP9, AV1, VP8, MPEG-2, MPEG-4, H.263.
- [ ] Add the same contract to JPEG single-image decode and legacy `rk_jpeg_*`.
- [ ] Add encoder interfaces for the implemented H.264, H.265 and JPEG paths.
- **Status:** pending

### Phase 7: Full Verification and Handoff
- [x] Run host-independent gates on lf1 and t6.
- [x] Run H.264/MJPEG C/C++ consumer tests on lf1.
- [x] Run the complete suite on lf1 and the 80% OPP-capped t6 gate.
- [x] Re-run the H.264/MJPEG C/C++ consumer test on t6.
- [x] Update and commit the H.264/MJPEG SDK slice.
- **Status:** complete

## Decisions Made

| Decision | Rationale |
|----------|-----------|
| Preserve existing `rk_jpeg_*` symbols | `rknn-fastinfer` already depends on the legacy surface |
| Add `rk_mpp_*` V1 decoder API beside legacy symbols | Avoid ABI breakage while replacing path-bound JPEG calls |
| Use packet/chunk input rather than file paths | Required for cameras, containers and live streams |
| Return decoder-owned frame storage until explicit release | Avoid per-frame allocation APIs and hidden pointer invalidation |
| Keep new V1 output to NV12/I420 | Hardware JPEG RGB writeback is not deterministic; the legacy `rk_jpeg_*` RGB path remains intact |
| Recreate JPEG DMA/service resources per image | Reuse produced an eight-byte stale writeback even after service reopen |
| Copy input before returning from submit | Camera and socket buffers may be reused immediately by the caller |

## Errors Encountered

| Error | Attempt | Resolution |
|-------|---------|------------|
| H.264 SPS/PPS between pictures were dropped at an AU boundary | 1 | Move the complete non-VCL suffix into the next access unit |
