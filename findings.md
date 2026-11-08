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
- `codec::hevc::Decoder` accepts one complete access unit at a time and already
  returns POC, display epoch, chroma format and frame errors. Its CLI currently
  buffers the whole file and sorts afterward, so the SDK needs a bounded
  streaming reorder queue.
- HEVC `split_access_units` has the same parameter-set carryover issue as the
  old H.264 splitter: VPS/SPS/PPS after one picture must start the next AU.
- HEVC SPS currently discards `sps_max_num_reorder_pics`; the SDK needs that
  value to bound live playback latency instead of waiting until EOS.
- H.265 10-bit output is already planar little-endian 16-bit. Add an explicit
  `I420P10` pixel format rather than silently treating it as 8-bit I420.
- The H.265/HEVC C consumer passes on lf1 and t6 for 8-bit NV12/I420 and
  Main10 I420P10. The Main10 fixture has six frames with B-pyramid reordering,
  so SPS-bounded display ordering is exercised rather than only IDR output.
- VP9 already emits displayed pictures directly from `decode_packet`, including
  `show_existing_frame` and hidden-alt-ref superframes, so the SDK does not add
  another reorder queue. VP9 byte-stream packet boundaries are normative; IVF
  demux remains a caller/container responsibility.
- The VP9 C gate passes on lf1 and t6: profile 0 12-frame NV12/I420 and profile
  2 10-bit 12-frame I420P10 outputs are byte-identical to C MPP. The test feeds
  each IVF frame as one packet and exercises decoder reset/reuse.
- `codec::av1::Decoder::decode_packet` already accepts one temporal unit and
  returns zero to many display-ready pictures. It carries sequence state,
  backward CDFs, eight reference slots and `show_existing_frame` internally.
- AV1 IVF uses the `AV01` FourCC and MPP decoder type `16777224`. The 320x240
  10-bit fixture contains 10 IVF packets; the 8-bit inter-box fixture contains
  two.
- RK3576 AV1 requires a separate VDPU383 hardware path. `decoder383.rs` shares
  the AV1 packet parser while maintaining its own Session, texture slots,
  reference CDF state and planar readback.
- Verified AV1 8-bit key+P SHA-256 is `7f4a0c9e…4020f`. On lf1 VDPU383
  10-bit output matches C at `f3b69b61…49b8`; on t6 grain-applied output is
  `b75e6172…904d`, matching the Rust/libaom result while C retains its known
  missing-film-grain bug.
- Verified VP9 SHA-256 values are
  `c5381bbf…3949a` (profile 0 NV12),
  `53bc3dd4…0abea69` (profile 0 I420), and
  `1c4c63f9…9a09` (profile 2 I420P10).

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
| Extend ABI 1.1 with H.265 and `I420P10` | Adds codecs/output without changing existing struct layouts or symbols |
| Reorder H.265 only until the SPS is available | Parameter sets usually share the first AU; using the SPS value avoids a fixed conservative delay |
| Keep VP9 packet input container-neutral | Payloads are already frame-delimited; superframes are handled by the decoder |

## Issues Encountered

| Issue | Resolution |
|-------|------------|
| Existing root `Decoder` name is JPEG-specific | Keep it and use `sdk`/`c_api` naming for new generic surface |

## Resources

- `src/lib.rs`
- `src/codec/h264/decoder.rs`
- `src/codec/hevc/decoder.rs`
- `src/codec/vp9/decoder.rs`
- `src/codec/av1/decoder.rs`
- `src/codec/jpeg/parser.rs`
- `src/hal/jpegd.rs`
- `src/bin/h264_stream.rs`
