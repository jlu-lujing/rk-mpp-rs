# Task Plan: Public Codec SDK

## Goal
Provide stable, versioned Rust and C library interfaces for hardware codec use,
prioritizing persistent H.264 and MJPEG decode sessions, then extending the
same contract to the other implemented codecs and encoders.

## Current Phase
Phases 1-6.3 are complete for H.264, H.265, VP9, AV1 and MJPEG decode. The next
priority is a stable encoder contract for the implemented H.264, H.265 and JPEG
paths.

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
- **Status:** complete on lf1 and t6.

### Phase 6: Other Codec Interfaces
- [ ] Extend the decoder contract to HEVC, VP9, AV1, VP8, MPEG-2, MPEG-4, H.263.
- [ ] Add the same contract to JPEG single-image decode and legacy `rk_jpeg_*`.
- [ ] Add encoder interfaces for the implemented H.264, H.265 and JPEG paths.
- **Status:** in_progress

### Phase 6.1: H.265/HEVC Decode Backend
- [x] Add H.265 codec IDs and 10-bit planar output to the V1 header.
- [x] Implement arbitrary-chunk Annex-B framing with parameter-set carryover.
- [x] Track SPS reorder depth and emit display order across epochs.
- [x] Verify 8-bit NV12/I420 and Main10 I420P10 output against C MPP.
- **Status:** complete on lf1 and t6.

### Phase 6.2: VP9 Decode Backend
- [x] Add the VP9 codec ID and packet-oriented input rules.
- [x] Expose 8-bit NV12/I420 and 10-bit I420P10 output.
- [x] Verify profile 0/2, inter and hidden-alt-ref packets against C MPP.
- **Status:** complete on lf1 and t6.

### Phase 6.3: AV1 Decode Backend
- [x] Add the AV1 codec ID and temporal-unit packet input rules.
- [x] Expose planar 8-bit NV12/I420 and 10-bit I420P10 output.
- [x] Verify RK3588 and RK3576 VDPU383 key/inter/show-existing packets.
- **Status:** complete on lf1 and t6.

### Phase 6.4: Encoder API Contract
- [ ] Define frame input ownership, packet output and EOS/flush rules.
- [ ] Add H.264, H.265 and JPEG encoder handles to Rust and C.
- [ ] Verify fixed-QP IDR/GOP output against the existing C goldens.
- **Status:** in_progress

### Phase 7: Full Verification and Handoff
- [x] Run host-independent gates on lf1 and t6.
- [x] Run H.264/MJPEG C/C++ consumer tests on lf1.
- [x] Run the complete suite on lf1 and the 80% OPP-capped t6 gate.
- [x] Re-run the H.264/MJPEG C/C++ consumer test on t6.
- [x] Update and commit the H.264/MJPEG SDK slice.
- [x] Run lf1/t6 full gates and both C/C++ boards for H.265.
- [x] Update and commit the H.265/API 1.1 slice.
- [x] Run lf1/t6 C/C++ gates for VP9 profile 0 and profile 2.
- [x] Run final VP9 full suite and commit the API 1.2 slice.
- [x] Run lf1/t6 C/C++ gates for AV1 8-bit and 10-bit.
- [x] Run final AV1 full suite and commit the API 1.3 slice.
- **Status:** complete

## Decisions Made

| Decision | Rationale |
|----------|-----------|
| Preserve existing `rk_jpeg_*` symbols | `rknn-fastinfer` already depends on the legacy surface |
| Add `rk_mpp_*` V1 decoder API beside legacy symbols | Avoid ABI breakage while replacing path-bound JPEG calls |
| Use packet/chunk input rather than file paths | Required for cameras, containers and live streams |
| Return decoder-owned frame storage until explicit release | Avoid per-frame allocation APIs and hidden pointer invalidation |
| Keep new V1 output to NV12/I420/I420P10 | Hardware JPEG RGB writeback is not deterministic; the legacy `rk_jpeg_*` RGB path remains intact |
| Recreate JPEG DMA/service resources per image | Reuse produced an eight-byte stale writeback even after service reopen |
| Copy input before returning from submit | Camera and socket buffers may be reused immediately by the caller |

## Errors Encountered

| Error | Attempt | Resolution |
|-------|---------|------------|
| H.264 SPS/PPS between pictures were dropped at an AU boundary | 1 | Move the complete non-VCL suffix into the next access unit |
