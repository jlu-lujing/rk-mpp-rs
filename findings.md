# Findings & Decisions

## Requirements

- Priority codecs: H.264 decode and MJPEG decode.
- Public API must support live packet input, persistent sessions, frame output,
  output metadata, EOS/flush/reset, and errors.
- Keep the current `rk_jpeg_open/frame/close` ABI for `rknn-fastinfer`.
- Add a versioned C header and verify actual C and C++ consumers on RK boards.
- Extend the public contract to all implemented codecs/encoders after the
  priority H.264/MJPEG path is stable.

## Research Findings

- `src/lib.rs` currently exports only `rk_jpeg_*`; it is path-based, single-image,
  RGB-letterbox oriented, and does not expose H.264.
- `codec::h264::Decoder` already provides `new`, `decode_access_unit`,
  `set_output_nv12`, `recover_session`, and a bounded `DisplayReorder`.
- `codec::jpeg` parses in-memory bytes, and `hal::jpegd::Job` builds hardware
  decode jobs for RGB888 or NV12. The missing layer is session/stream framing
  and a stable public API.
- `Cargo.toml` currently builds `rlib` and `staticlib`, not `cdylib`.
- No versioned public C header exists outside the reference tree.
- H.264 stream framing originally started a new AU at the next VCL NAL. That
  dropped SPS/PPS inserted after the previous picture. The splitter now starts
  the next AU at the first non-VCL NAL after the prior picture.
- C and C++ consumers pass against both `librk_mpp_rs.so` and
  `librk_mpp_rs.a`; all `rk_mpp_*` and legacy `rk_jpeg_*` symbols are exported.
- A whole H.264 stream or several concatenated JPEGs may be submitted in one
  packet, while byte-at-a-time fragmentation also remains supported.

## Technical Decisions

| Decision | Rationale |
|----------|-----------|
| `rk_mpp_decoder_*` V1 ABI | Clear codec-agnostic naming and future extension |
| `struct_size` on public structs | ABI evolution without silent layout breakage |
| Decoder-owned frame + explicit release | Stable pointer lifetime and no caller allocator coupling |
| Plane pointer/stride array | Covers NV12, planar YUV and future formats without reinterpretation |
| H.264 framing at VCL/AUD boundaries | Reuses the proven access-unit semantics |
| MJPEG SOI/EOI scanner | Decouples JPEG decode from files and supports concatenated streams |
| Sticky EOS plus idempotent flush | Gives callers one unambiguous end condition and allows reset/reuse |
| Packet metadata inherited by all pictures in that packet | Avoids inventing timestamps when one packet contains multiple AUs/frames |
| New V1 YUV-only output | Deterministic and cheap; legacy JPEG remains the RGB compatibility path |

## Issues Encountered

| Issue | Resolution |
|-------|------------|
| Existing root `Decoder` name is JPEG-specific | Keep it and use `sdk`/`c_api` naming for new generic surface |

## Resources

- `src/lib.rs`
- `src/codec/h264/decoder.rs`
- `src/codec/jpeg/parser.rs`
- `src/hal/jpegd.rs`
- `src/bin/h264_stream.rs`
