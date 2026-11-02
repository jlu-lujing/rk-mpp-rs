# Reimplementing the full MPP surface in Rust

The official Rockchip MPP is ~296k lines of C/C++ (decoders 43k, encoders 10k,
HAL 143k, framework 29k, osal/driver 18k). This file tracks every feature
point, what it costs in the original, and where we stand.

Scope is the whole library, not only what YOLO needs: all decoders, all
encoders, post-processing, and the framework features (async tasks, fences,
multi-core, secure mode).

Companions: [README.md](README.md) for the current state at a glance, and
[docs/HANDOFF.md](docs/HANDOFF.md) for how to pick the work up - board toolchain,
debug switches, traps, and the ordered next steps.

## Scope decision (2026-11-02)

**Untrusted stream geometry (2026-11-02).** After the first corpus sweep lost the
board, every decoder now validates the parsed coded size before it writes a
register or allocates a buffer: zero, or anything above 8192 in either axis, is
refused with an `InvalidInput` error. The check is shared
(`hal::check_decode_geometry`), applied in `h264d`, `hevc`, `vp9d`, `av1d` and
through `vdpu2::run_job` for the VDPU2 codecs, and unit-tested. It does not change
any valid stream. The board-side re-run is done on t62 (same SoC, kernel 6.1.141):
VP8 profile/scale gate 3/3, VP9 matrix 7 cells (with its two adjudicated C bugs),
the VDPU2 regressions (VP8 6 fixtures + register images, H.263 3, MPEG-4 3, MPEG-2
3) and the H.265 rate-control matrix 13/13 all pass, and an HEVC decode matches.
`p_slice_mmco.264` is a bare P slice with no SPS/PPS, so it is not a standalone
decode input; the C harness emits a frame from it while the crate refuses, which is
a fixture-usage difference rather than a decoder result.

**Corpus quarantine ordering (2026-11-02).** While the AV1 tile reset was open,
its `QUARANTINE` decision happened before either decode binary was invoked; an
offline stub run with both binaries pointed at `false` reported
`named=1, skipped=0`, and a two-name test proved both corpus and `tests/data`
spellings were covered. The root cause was subsequently fixed and the tile
fixture was removed from quarantine to serve as the regular AV1 sweep cell.
The two proven film-grain `C-BUG` rows are still skipped at the same point
instead of being decoded twice.

**Corpus integrity before decode (2026-11-02).** `gen_corpus.sh` now writes a
deterministic `corpus-manifest.json`, and `board_corpus_sweep.sh` validates it
before either decoder is invoked. The checker records size/SHA-256/codec/
container for every file and rejects malformed IVF FourCCs, Annex-B streams
without a start code, MP4 containers masquerading as `.m4v`, and damaged H.263/
MPEG-2 headers. It wrote and rechecked the 142 tracked `tests/data` files,
rejected a one-byte mutation, and aborted a manifest-backed sweep before any
decode. Quarantined fixtures still report `named=1, skipped=0`, with the
manifest itself excluded from decoder counts.

**Full regenerated corpus on t62 (2026-11-02).** `corpus-gen4` contains 66
validated payloads (6,160,351 bytes) and the complete sweep finished with
`pass=60 fail=0 named=6 skipped=0`. The named set is the bare P-slice fixture,
two AV1 film-grain C bugs, two VP9 C bugs and interlaced MPEG-2 C rewind/IDCT
rounding. JPEG is decoded with `-t 8 -w/-h -f RGB888` and compared directly.
AV1 includes long GOP, 720p, 1080p, Main10, a repeated sequence header and
320->640 / 640->320 resolution changes; all are normal PASS rows.

The project is in **pixel/byte-exact imitation + output-preserving performance** mode:

- A result that differs from the C reference is a defect, not an improvement, unless
  the C behaviour has been separately proven to be a bug.
- New feature frontier work (tiles/WPP/SAO, B frames, multi-reference scheduling,
  refresh/GDR allocation, fuzz-only edges) is **frozen** until the implemented paths
  are closed to byte/pixel equality. `hal::h265e`'s GDR table remains opt-in and
  explicitly unverified (the C vintage cannot be made to emit a refresh picture
  through the public config, so there is no oracle).
- Performance work is allowed only when it does not change one byte of output, and
  the same stream must be compared before/after.
- A deliberate divergence is kept only with the C-side evidence that it is wrong,
  and recorded in this file.

## Board capability map (NanoPC-T6-LTS, mask `0x1009321a`)

Measured with `rk-mpp-rs probe`, which asks two different questions per block:
whether the kernel's `MPP_CMD_PROBE_HW_SUPPORT` mask has the bit, and whether
`MPP_CMD_INIT_CLIENT_TYPE` actually binds a session. A block that is in the mask
but has no driver behind it cannot be driven - the C tools say so as
"client N driver is not ready!" and the ioctl answers `EINVAL`.

| client | block | mask | session | drives |
|---:|---|---|---|---|
| 1 | vdpu2 | yes | binds | H.264, MPEG2, MPEG4, H.263, VP8 |
| 3 | vdpu2 post-processor | yes | binds | nothing in this source - see M9 |
| 4 | av1d | yes | binds | AV1 |
| 9 | rkvdec | yes | binds | H.264, H.265, VP9, AVS2 |
| 12 | AVS+ decoder | yes | binds | AVS+ (no conformance stream available) |
| 13 | jpegd | yes | binds | baseline JPEG decode, with its own post-processor |
| 16 | rkv-enc | yes | binds | H.264, H.265 encode |
| 19 | vepu JPEG | yes | binds | JPEG encode |
| 28 | IEP | yes | binds | deinterlace and YUV scale - the M9 engine on this board |
| 0, 2 | vdpu1 and its PP | no | `EINVAL` | — |
| 8 | standalone HEVC decoder | no | `EINVAL` | — |
| 17, 18, 20, 24 | vepu1, vepu2, jpeg_enc, vepu22 | no | `EINVAL` | — |
| 29 | VDPP | no | `EINVAL` | the scale / rotate / CSC / sharpen engine, absent here |

Every bound client supports `MPP_CMD_SET_REG_ADDR_OFFSET` and
`MPP_CMD_POLL_HW_FINISH`.

**Scope decision (2026-11-01): RK3588 only.** Every "done", "byte-exact" and
"faster" claim below is a statement about NanoPC-T6-LTS and the hardware blocks it
exposes. The code probes capability and layers per block rather than baking in a
board, so other SoCs may run - but they are unverified and unclaimed. Extending
elsewhere is W12 in [docs/PLAN.md](docs/PLAN.md).

## Milestones

| # | deliverable | official LOC | status |
|---|---|---:|---|
| M0 | `mpp_service` client: all `MPP_CMD_*`, MULTI_MSG batches, RCB, IOVA, poll, probe | 1.0k | **done** |
| M0 | JPEG decode (`jpegd`): parser, hw tables, registers, RGB888/NV12, 1/2–1/8 downscale, scaler | 2.9k | **done** |
| M0 | JPEG decode beats MPP by ~30 % (no buf_slot/meta layer); 0.96 ms @ 640p | — | **done** |
| M1 | H.264 syntax: Annex-B, RBSP, Exp-Golomb, SPS/PPS/slice header, scaling lists | 3.5k | **done** |
| M1 | H.264 register map generated from `hal_h264d_rkv_reg.h` (80 words, 122 fields) | 0.25k | **done** |
| M2 | H.264 decode on rkvdec: reg writer, DPB, reference lists, MMCO, MBAFF, field-coded, error propagation | 6.0k | **frame/MBAFF/PAFF/long-term/error-path bit-exact** |
| M3 | H.265 decode: syntax (VPS/SPS/PPS, RPS, scaling lists, tiles, WPP) + rkvdec HAL | 12k | **Main/Main10/scaling-list/long-term hardware path bit-exact; t6 rkvdec rejects 4:2:2/4:4:4; SCC pending** |
| M4 | VP9 decode (segmentation, loop filter, superframes) | 8k | **partial; profile 0/2 320x240 and 720p error-resilient temporal-segmentation streams are bit-exact to libvpx; edge-case matrix and conformance pending** |
| M5 | AV1 decode (frame headers, OBU, film grain) | 14k | **Main 8/10-bit and film-grain hardware paths bit-exact on 320x240 fixtures; wider conformance/performance matrix pending** |
| M6 | MPEG2 / MPEG4 / H.263 / VP8 / AVS+ / AVS2 decode | 14k | **MPEG-2, MPEG-4, H.263 and VP8 decode done (syntax, shared vdpu2 register file, DPB, `rk-mpp-rs m2vdec` / `mp4dec` / `h263dec` / `vp8dec`): byte-exact vs C MPP on every fixture, including MPEG-4 IPBB, all three H.263 frame sizes and VP8 hidden reference frames. Field-coded PAFF unverified. AVS+ / AVS2 todo** |
| M7 | H.264 encode (rkvenc): RC (CBR/VBR), intra/inter, deblock, ROI, GDR, slice output | 15k | **IDR byte-identical over 24 cells of size x profile x QP, and P pictures byte-identical at 320x240 (6 GOP cells: 1 IDR + up to 5 P, still and moving), SEI NAL units included, with zero non-address register differences in all seven sections. CBR, VBR *and* AVBR rate control are ported, wired and byte-exact against the reference library - registers, bit allocation and quantiser, six pictures NAL for NAL in each mode; ROI/QP map, deblock, the start-QP derivation and scene detection are ported with board evidence, GDR is byte-identical over four cells, slice split is byte-identical over four cells (BY_CTU at 96/8/150 blocks and BY_BYTE at 256, the 8-block cell putting 38 slices past the 32-entry FIFO), per-slice low-delay packet output is byte-identical over five cells (including that 38-slice FIFO crossing and a six-picture IDR+P stream), and the >4096 CIME extension lines over two (4160x64, 4608x256). P pictures match at eleven sizes spanning the FBC-header threshold - the divergence past 640x480 was the constant FBC body offset, since fixed. The implemented H.264 encode surface is closed; new scheduling/recoding modes remain frozen feature work.** |
| M8 | H.265 encode (rkvenc) + JPEG encode (vepu) | 12k | **JPEG baseline 4:2:0, restart markers, rotation, packed YUV/RGB input, low-delay packet output and RK3588 default four-band multi-core partitioning are board-byte-exact; rotated multi-core remains. H.265 fixed-QP IDR/P GOP and CBR/VBR/AVBR are board-byte-exact. VI/refresh scheduling, tiles/WPP/SAO, B frames and multi-reference scheduling remain feature work.** |
| M9 | Post-processor: deinterlace, scale, format conversion | 8k | **IEP2 I5O2 deinterlace is byte-identical to the C `iep2_test` on t6 (720x480, 1,036,800 bytes); scale/crop and the other modes are untouched. VDPP (client 29) is absent on this board. See the note below** |
| M10 | Framework: task queue, async completion, dma fences, multi-core CCU, secure mode | 29k | **task queue with completion callbacks and the reference-counted buffer pool are ported (host state machines plus three ignored board assertions); fences, multi-core CCU and secure mode are surveyed and blocked on this kernel/board - see the entries below** |
| M11 | Software reference decoders (our own ground truth, no ffmpeg) | — | **in progress** |

Future work is planned in full in [docs/PLAN.md](docs/PLAN.md): thirteen
workstreams covering every open item here, their dependencies, phase exits, the
acceptance matrix and the risks. This file stays the evidence ledger; the plan says
in what order the evidence gets produced.

## Feature checklist (per official capability)

### Decoders
- [x] JPEG baseline, 4:2:0/4:2:2/4:4:4/gray, NV12 + RGB888 output
- [x] JPEG 1/2, 1/4, 1/8 hardware IDCT downscale for RGB888 and NV12.
      `bus.jpg` (640x640, SHA-256 `fb4914d1…4014e16`) produces
      320x320, 160x160 and 80x80 frames. Full-size output is 44.13 dB
      RGB PSNR versus libjpeg-turbo; 1/2, 1/4 and 1/8 are 38.64, 36.16 and
      33.77 dB. The scaled frames are visually clean and repeatable. C MPP's
      scale setter is still marked TODO, so libjpeg-turbo is the reference
      rather than C MPP's unscaled default. On t6, submit+poll remains about
      0.94 ms because entropy decoding dominates; the performance benefit is
      reduced output bandwidth and avoiding the later RGB resample.
- [x] DMA output readback invalidates the CPU mapping with
      `DMA_BUF_SYNC_START | DMA_BUF_SYNC_READ` and closes it with
      `DMA_BUF_SYNC_END | DMA_BUF_SYNC_READ`. Missing that ioctl made
      `err=0` hardware jobs return RGB frames with horizontal cache-line
      bands. The hardware regression test locks full, 1/2, 1/4, 1/8 and
      half-size NV12 output hashes.
- [x] H.264 syntax (SPS/PPS/slice, CABAC-aware header, B-frames, weighted pred)
- [x] H.264 decode on rkvdec is bit-exact for progressive High/Main,
      B-pyramid, multiple slices and MBAFF:
      1080p Hard 90/90 frames, 720p Main 30/30 frames, multi-slice 10/10
      frames, MBAFF 10/10 frames. Throughput on those streams is in the
      Verification table (287 fps at 720p, 230 fps at 1080p), with zero
      hardware errors
- [x] frame buffer pool: buffers whose DPB entry is gone are reused, so a
      90-frame 1080p stream keeps ~7 frame buffers instead of 90 without
      changing the bit-exact output
- [x] H.264 field-coded pictures: PAFF `field_paff.264` 15/15 frames match
      FFmpeg byte-for-byte (`2e6bb03e…39a2f`)
- [x] H.264 long-term references: MR1 conformance stream (`MR1_BT_A.h264`)
      176x144, 62/62 frames match FFmpeg byte-for-byte
      (`006f1add…f0028`), including MMCO-3 replacement, MMCO-4 bounds and
      MMCO-6 current-picture marking
- [x] H.264 missing-reference detection, DPB error propagation and output
      filtering: a stream with access unit 20 removed produces the same 22
      clean pictures as MPP (`2ee3a695…c2f`); hardware status errors are
      mapped to `FRAME_ERR_DEC_HW_ERR` (`0x100`)
- [x] H.265 syntax: VPS/SPS/PPS, profile_tier_level, short-term RPS
      (explicit + predicted), slice segment header, POC and reference lists —
      validated on x265 720p/1080p streams
- [x] H.265 8-bit Main decode on rkvdec: 720p P and B streams are
      byte-identical to MPP after display-order output; 1080p 5/5 is also
      byte-identical. POC/IDR-epoch display sorting, vdpu34x hardware status
      mapping and missing/error-reference propagation are implemented
- [x] H.265 CRA/open-GOP and RASL: a 30-frame x265 stream with CRA, RASL_R and
      RASL_N NAL units is byte-identical to MPP in display order; CRA preserves
      POC continuity while IDR/BLA reset it. A CRA clears reference marks but
      retains its DPB entries until the following RASL RPS selects the
      pre-CRA pictures it still needs; removing them at the CRA dropped six
      frames (POC 7..9 and 17..19)
- [x] H.265 dependent slice segments: the JCT-VC `DSLICE_A_HHI_5` stream has
      1920x1080, 50 pictures, 903 NAL units and 850 VCL NAL units. Rust now
      parses dependent and independent non-first segments, validates the CTB
      address and feeds all segments in one AU. All 50 pictures are clean and
      the 155,520,000-byte planar output is byte-identical to MPP
- [x] H.265 multi-slice pictures: a four-slice x265 B/open-GOP stream emits the
      same 23 clean pictures as MPP and is byte-identical after NV12-to-planar
      comparison; both decoders reject the same seven damaged pictures
- [x] H.265 WPP/PPS compatibility: MPP's PPS parser does not consume
      `pps_loop_filter_across_substreams_enabled_flag`. The Rust parser mirrors
      that behavior, producing packet bytes 24/25 as `03 40` instead of the
      misaligned `02 b0`; a regression test locks this MPP-specific quirk
- [x] H.265 multi-IDR DPB lifetime: IDR/BLA resets now clear the previous
      POC epoch's references. A 120-frame stream spanning four IDR epochs emits
      all 120 pictures and matches MPP byte-for-byte; the previous code leaked
      old frames into RPS lookup after AU31 and dropped all but 36 pictures
- [x] H.265 Main10 on rkvdec: a 256x256 30-frame x265 stream decodes
      `30/30`, with every job reporting `reg224=0x107` and `err=0`. The
      hardware stores 10 bits per sample in `align(width,64)*bit_depth/8`
      bytes per row; converting that packed layout to planar `yuv420p10le`
      produces the same SHA-256 as the C MPP output (`0bfb5b36…ca818`)
- [x] H.265 scaling lists: MPP default matrices and explicit SPS list data
      are byte-identical to MPP on the board. The missing 1360-byte hardware
      table originally made an enabled-list stream output 0 frames
      (`err=0x100`, status `0x23`). Rust now parses SPS/PPS list data, follows
      MPP's matrix inheritance and `SCALE_4X4_COPY` semantics, builds the
      vdpu34x table, programs `reg180` and sets `scanlist_addr_valid_en`.
      A 1920x1080 default-matrix stream emits 10/10 frames and 31,104,000
      bytes with SHA-256 `8ace9ea6…e49e73a0`; a 256x256 explicit-SPS stream
      emits 10/10 frames and 983,040 bytes with SHA-256
      `ecdaee49…7a53668ec`. A second fixture inserts explicit
      `pps_scaling_list_data_present_flag=1` data and also emits 10/10 frames
      with SHA-256 `49a240f2…fa007`. All three match MPP byte-for-byte, with
      every job reporting `err=0` and status `0x107`
- [x] H.265 long-term references: an MPP-encoded 320x320 30-frame stream with
      `enc_mark_ltr`/`enc_use_ltr` metadata produces 30/30 clean pictures.
      Rust and MPP output are both 4,608,000 bytes with SHA-256
      `cc9994ca…64ab5`; SPS and slice short-term RPS selection, SPS long-term
      entries and mixed ST/LT DPB lists are covered by parser, DPB and packet
      tests
- [x] H.265 chroma-depth boundary and frame layout: the parser follows MPP's
      8/10-bit limit, rejects mismatched luma/chroma depths and explicitly
      rejects 12-bit. `Frame` now carries `chroma_format_idc`, so allocation
      uses MPP's 4:0:0/4:2:0/4:2:2/4:4:4 size factors (`1`, `3/2`, `2`, `3`)
      and output conversion no longer assumes 4:2:0
- [ ] H.265 4:2:2/4:4:4 on a SoC whose rkvdec accepts them. C MPP's syntax and
      allocator contain 8/10-bit 4:2:2/4:4:4 paths, but the NanoPC-T6 board
      rejects all four generated fixtures with 0 output frames, hardware
      status `0x123`, and kernel `mpp_rkvdec2 ... resetting for err 0x123`.
      C MPP rejects the same streams, so this is a platform capability boundary,
      not evidence that the Rust job is correct
- [ ] H.265 SCC. The MPP source contains no palette/screen-content decode path
- [x] H.265 tiles: an MPP-encoded 1280x720 two-column stream decodes
      30/30 frames byte-identically to MPP. Explicit non-uniform column/row
      geometry is preserved through the 112-byte hardware packet and covered
      by parser/packet tests; no non-uniform board fixture is available yet
- [ ] VP9 full decode matrix. Profile 0 and Profile 2 10-bit 64x64
      inter/intra-only fixtures decode 12/12 frames with `err=0` and status
      `0x107`. A larger 320x240 30-frame fixture decodes 30/30 frames for both
      profiles with `err=0`; every Y/U/V sample matches the FFmpeg reference.
      Profile 0 output is 3,456,000 bytes with SHA-256
      `d72d3d033a535bb383fca4b983945253c09c7b776a5238416b0cf11b9714aaf7`;
      profile 2 planar `yuv420p10le` output is 6,912,000 bytes with SHA-256
      `09f243ec9117d450ca0e596387cb1538e39fa6ac988ae101032896e2c5f2b4ff`.
      The lossless and tiled edge matrix also matches FFmpeg byte-for-byte:
      Profile 0 lossless is 1,382,400 bytes with SHA-256
      `53bc3dd40dea22c058b7b38c697eb8c7912c127af44f3a5db26cefc0a0abea69`;
      Profile 2 10-bit lossless is 2,764,800 bytes with SHA-256
      `1c4c63f9b70845c1c3922dc64430d04570f2ad94156781cde931b11bfdd89a09`;
      and a 640x360 error-resilient stream with two tile columns is
      4,147,200 bytes with SHA-256
      `ffca5b5ac5ab80b1de42214d3b31bb674a91fdc71c05e9957d39c22c52995f4f`.
      A 321x241 lossless stream also matches FFmpeg byte-for-byte
      (232,646 bytes, SHA-256
      `8ac48f11837399790243f67695f8b2b98d637c0f82a5ae7688a16df9be70beca`),
      covering odd chroma dimensions and VP9's padded DMA stride.
      The earlier corrupt-first-frame and POC 8/26 timeout were traced to MPP's
      unusual segmentation sentinel bytes: when `update_map` is set but a tree
      or temporal-prediction probability is not updated, MPP stores `0xff`
      rather than zero. Rust now preserves those values in `PicParams` and the
      probability packet, with a focused regression test. The hardware path
      also uses MPP's 9/5 default frame-buffer size, checks the five command
      capability ranges before submitting optional commands, and no longer
      sends unsupported `MPP_CMD_SET_RCB_INFO` (`0x203`) to the T6 kernel.
      The original hardware timeout from compressed-stream bases
      (`reg128/129`) left at zero, intra-only probability-context reset order
      and packed 10-bit output conversion remain board-verified. Automatic
      alt-ref decoding is also board-verified: the 60-frame hidden-alt-ref
      stream now reports 60 jobs with `err=0`, every status `0x107`, and
      6,912,000 bytes matching MPP and FFmpeg byte-for-byte (SHA-256
      `fc2e400e…abc0cd4`). The dropped-frame/timeout bug was a colmv lifetime
      error: while the previous frame was hidden, Rust reused the colmv
      surface that `reg170` still referenced. `previous_colmv_fd` and
      `last_colmv_fd` now mirror MPP's `pre_mv_base_addr`/`mv_base_addr` pair,
      and both stay excluded from frame-buffer reuse. All 66 task
      address-equality graphs and all 66 selected probability-context images
      now match the C HAL. Additional board cases now pass against C MPP
      after converting its NV12 output to planar YUV: a real keyframe followed
      by a two-byte `show_existing_frame` packet repeats the cached 64x64
      picture (`7ae4a0a9…aebf7`); a 320x240 → 384x256 → 320x240 keyframe
      transition keeps all three frames bit-exact (`3580aae5…0277`); an
      official 1280x720 30-frame resize stream transitions 1280x720 →
      640x360.
      All 30 probability flag/delta images are byte-identical between C and
      Rust, and all 137 non-address register fields compared for every frame
      are identical; only the expected DMA-buffer FDs differ. The apparent
      frame-5 transform/residual mismatch was traced to stale segment maps,
      not deblocking or inverse-transform precision: every frame in the stream
      is error-resilient, so VP9 resets past segmentation state before decoding.
      The upstream C HAL flipped the ping-pong buffers without clearing them,
      leaving an old segment id visible to temporal SIP prediction and changing
      the target block from segment 0/Q165 to segment 1/Q100. Rust now zeroes
      both segment-map halves on intra-only and error-resilient frames, matching
      libvpx's `vp9_setup_past_independence()`. The 6-frame 1280x720 regression
      then matches libvpx byte-for-byte in every Y/U/V plane, and the existing
      64x64 `show_existing_frame` hardware test still passes. 321x241 output
      now uses ceiling chroma dimensions; and a 384x256 two-column tile stream
      is bit-exact.
      VP9 reference `errinfo` is also propagated into dependent
      frames as `FRAME_ERR_DEC_INVALID`; previously only the current hardware
      status was checked. Profile 1/3 remain unsupported because C MPP rejects
      them before the HAL. Corrupt-stream behavior and performance remain
      before closing M4
- [x] **VP8 profiles 1 and 3.** The corpus probe (`scripts/board_vp8_profile_probe.sh`)
      established that every VP8 stream in `~/rk-mpp-oracles/corpus` is profile 0
      with no key-frame scale factors, so the comparison runs on derived fixtures:
      the frame-tag version bits of `vp8_320x240_12_ip.ivf` patched to 1 and to 3.
      The VP8-mode syntax turns out to be version-opaque in the reference - all
      version-conditional code sits in the dead `vp7_header_parser` - and the
      vdpu2 derivation (bilinear on any low bit, the tap bank only when the low
      two bits are clear, the extra MV range off for version three alone) is
      pinned for all eight version values.
      `scripts/board_vp8_profile_vs_mpp.sh`: profile 1 and profile 3, twelve
      frames each, byte-identical to `mpi_dec_test -t 9`. The key-frame scale
      bits were a refusal on our side; libvpx and C both decode such a stream at
      the coded size (the bits are display-only), so the parser now ignores them
      too. The script derives a scaled key frame (`width scale = 2`) and compares
      twelve frames: **byte-identical to C**.
 - [x] **The damaged-stream policy, adjudicated against the board's C harness**
       (`scripts/board_vp_corrupt_vs_mpp.sh`: container truncation, half of the
       last packet, a mid-stream bit flip, on both decoders). What the table
       settled, and what changed because of it: the VP8 CLI now writes every
       picture it decoded instead of nothing on damage, and a torn container
       tail is the *packets'* business, not the run's - a truncated last packet
       is padded back to its declared size with the zeros the reference's own
       buffer copy leaves there and still decodes, and a failed-picture exit
       code was wrong where `mpi_dec_test` prints the failure and exits 0.
       Result: vp8/container, vp8/half-last and vp8/mid-flip SAME (both sides
       rc 0, twelve frames), and vp9/container SAME (both rc 255, nothing). The
       remaining named rows are listed below, not hidden.
  - [x] **The VP9 boundary matrix, adjudicated against libvpx.** The remaining VP9
       cells are no longer "ours to fix": both were decided by decoding the same
       fixture with FFmpeg's libvpx and comparing three ways.
       * `vp9_p0_720p_errres_seg` (1280x720, error-resilient temporal segmentation,
         6 frames): Rust equals libvpx byte-for-byte on every frame (SHA-256
         `0f0ff0da8e78ff82a7284820e699f942c16a5e38c31e91bee6ace958ba0deb51`); C
         differs on frames 4 and 5 by **106 + 107 bytes, all in the V plane, max
         delta 3** (SHA-256
         `5b737017e39fe105170cfa1dc8039589ea9d48138c0a5918a568c9704b79cac1`). The
         register images match word for word, so this is a C-side state/filter bug
         and the crate keeps the libvpx result.
       * `vp9_p0_lossless_321x241_2`: libvpx and Rust both produce **2** frames; the
         C harness emits 1. Rust's output hash is the existing libvpx hash
         `8ac48f11837399790243f67695f8b2b98d637c0f82a5ae7688a16df9be70beca`, so C
         loses the second picture on this odd-dimension stream.
       `scripts/board_vp9_matrix.sh` now reports those two cells as
       `KNOWN-C-BUG` with the reason instead of `FAIL`/`NAMED`, so a real regression
       cannot hide behind them.
       The remaining VP9 rows stay as measured divergences with their reasons: the
       damaged-stream cells where C aborts the run and Rust recovers the surviving
       pictures (`vp9/half-last`, `vp9/mid-flip`).
- [ ] AV1 Main, film grain
  - [x] IVF packet boundaries, OBU headers/LEB128 and complete OBU splitting
  - [x] Sequence header, operating points, color config and tool flags
  - [x] Uncompressed frame header, reference state and tile geometry
  - [x] `OBU_FRAME` tile-group alignment and payload offsets match the
        official C MPP `av1d_debug=2` bit counts for every encoded frame in
        the 320x240 fixture
  - [x] a real 2x2 tile fixture matches C MPP's 11 frame-header bit counts and
        all 44 tile byte ranges; this includes the hidden `order_hint=8` frame
        in addition to the ten displayed pictures
  - [x] packet-level frame assembly: sequence updates, multiple coded frames
        per packet, `show_existing_frame`, tile-group accumulation, and
        repeated/redundant frame-header handling
  - [x] exact 12,256-byte CDF ABI and reference-slot store/load, with all four
        q-context snapshots byte-identical to C MPP on t6
  - [x] exact 3240-byte `DXVA_PicParams_AV1` ABI and all 11 real hardware
        jobs in the 2x2 tile fixture byte-identical to C MPP, including RK3588
        reference remapping, skip-mode references, split-frame offsets and
        ABI padding
  - [x] Rust's own AV1 texture-slot replay, rather than C's texture indices,
        independently reproduces all 11 jobs' current slot, seven logical
        reference mappings and pre-refresh reference map. It includes the
        hidden order-hint-8 frame and the later return to physical slot 4.
        `av1_dump_regs` now uses this replay, and its 11 regenerated PicParams
        images are still byte-identical to the C snapshots.
  - [x] t62/6.1.141 first-job execution. The root cause was two omitted filter
        DMA bases: C programs CDEF at `reg[85]` and loop restoration at
        `reg[91]`; Rust left both zero. The resulting STATUS was `0x2109`
        (`sw_dec_bus_int`) and reset `mpp_av1dec`. Scheduling both bases, using
        C's uncached DMA32 tables/frames and DMA32 stream, and matching C's
        4:2:2-sized display allocation fixed the one-frame prefix (STATUS
        `0x1100`). The full fixture now emits 10 frames/1,152,000 bytes
        byte-identical to current C (`9d339ae3…8138`), and the existing
        8/10-bit film-grain hardware regression passes without reset.
  - [x] non-identity translation/ROTZOOM/AFFINE global-motion conversion,
        primary-reference inheritance, shear quantization and invalid/scaled
        reference fallback match the C MPP integer rules
  - [ ] tile entropy decoding and a wider AV1 conformance matrix
  - [x] all 11 hardware jobs in the 2x2 tile fixture have byte-identical
        PicParams and input CDFs. Ten comparable 512-word AV1D_VPU register
        images match the C `MPP_IOC_CFG_V1` payload word-for-word outside DMA
        addresses. This comparison caught four real porting errors: the
        bit-15 field layout shifted by one, superres scaling overwrote warped
        motion, `comp_pred_mode` used the wrong source, and the stream length
        used the whole IVF packet instead of MPP's split frame
  - [x] pre-submit diagnosis no longer requires opening `/dev/mpp_service`.
        `examples/av1_dump_regs.rs` exports all 11 jobs' PicParams, global
        model, tile-info, input CDF, split stream, padded film-grain image,
        512-word register image and address-offset map;
        `tools/av1/compare_input_dumps.py` compares that directory with C
        MPP's `DUMP_AV1_DATAS`, normalising process-specific DMA descriptors.
        The host run regenerated all 11 PicParams images identical to the C
        goldens (88 files total); a simulated C/Rust match passed 11/11 and a
        one-word corruption was reported as `reg[0]`.
        `scripts/board_av1_presubmit_compare.sh` packages this into the first
        post-reboot action: it builds a separate C dump oracle, runs the
        known-good C decode, runs the CPU-only Rust exporter and refuses to
        continue unless every input image matches. The real board comparison
        remains pending until t62 returns.
  - [x] decoder orchestration and DMA lifetime: every hardware-completed frame
        imports its output CDF before the assembler parses the next frame in
        the same temporal unit, matching `av1d_parser_update()`. A regression
        proves that a later frame uses the CDF written by the previous job.
        Selecting motion-field reference `LAST2` also preserves C MPP's quirk
        of not requiring a positive order-hint distance.
  - [x] film grain: the parser and the 3240-byte PicParams image carry the
        syntax, and `update_grain=0` inherits the selected reference's complete
        parameters while replacing its seed like C MPP. Rust ports MPP's exact
        Gaussian sequence, scaling LUT interpolation, luma/chroma noise,
        autoregressive filtering and 13,056-byte `AV1FilmGrainMemory` image.
        Three 8/10/12-bit C snapshots match byte-for-byte. The HAL now writes
        that image to DMA, programs registers 7/8/28/29/94/95, and no longer
        rejects `apply_grain`.
        A libaom 320x240/10-frame 8-bit stream produces ten clean pictures
        and 1,152,000 bytes with SHA-256 `bdd74ace…f632afb`, byte-identical
        to libaom with film grain applied. A Main profile 10-bit stream also
        produces 10/10 clean pictures and 2,304,000 bytes with SHA-256
        `b75e6172…fe904d`, byte-identical to libaom's grain-applied
        `yuv420p10le` output and distinct from its skip-grain output
        (`f3b69b61…49b8`). The 10-bit failure that preceded this result was
        not a hardware status error: the display buffer, PP output mode and
        CPU unpack path still used 8-bit assumptions, so the job completed
        successfully but emitted only 1,152,000 bytes. Display allocation now
        uses MPP's packed 10-bit stride (`400` bytes for 320 pixels), PP emits
        the packed non-NV12 layout, and the output path expands the bit fields
        to unshifted little-endian 10-bit samples. The current C MPP build
        does not set `sw_apply_grain=1` in `vdpu_av1d_set_fgs()`, so C output
        instead matches libaom with film grain skipped; this is a C HAL
        defect rather than the Rust acceptance oracle.
  - [ ] wider AV1 conformance and performance matrix: more damaged-stream
        shapes still need differential fixtures. The damaged matrix now covers
        a torn container, a half-last packet and a mid-packet bit flip: Rust
        bounds the first two where C hangs after eight pictures, while the
        bit-flip case produces the same ten pictures and byte-identical planar
        output. The generated corpus covers
        640x480 long-GOP, 1280x720, 1920x1080, Main10, repeated sequence
        headers and 320<->640 dynamic resolution; all are byte-identical to C
        in the full corpus sweep.
  - The 320x240 fixture is the initial acceptance baseline: official C MPP
    emits 10/10 frames byte-identical to FFmpeg
    (`2cd9c129…291d29` for the planar YUV output). The 64x64 fixture is not
    a valid MPP hardware acceptance case on this board: official C MPP also
    emits ten identical all-zero frames (`fd9243e1…263e` per frame), so a
    mismatch there must not be attributed to the Rust port. On t62/6.1.141,
    the current C oracle self-recovers through 12 AV1 resets on this fixture;
    Rust self-recovers through 11 and produces the same ten zero frames, so the
    reset count is not a Rust-only divergence.
- [x] MPEG-2 syntax (`src/codec/m2v/parser.rs`): `00 00 01` split, sequence
      header (with optional quant matrices), sequence extension, sequence
      display extension, picture header, picture coding extension, quant matrix
      extension, slice units. Field order follows `m2vd_decode_seq_ext_header`
      and friends in the C parser, because MPEG-2 drops the MPEG-1 `one_bit`
      and every extension body starts four bits into the payload after
      `extension_start_code_identifier`. Chroma quant matrices are refused the
      way the C HAL does. `rk-mpp-rs m2v <file>` dumps the result.
      Fixtures `tests/data/m2v_320x240_12.m2v` (progressive IBBP, 15 slices per
      picture), `m2v_64x64_3.m2v` (all-intra) and `m2v_320x240_ilaced.m2v`
      (ildct/ilme, one picture coding extension per picture) are locked against
      `ffprobe -show_entries frame=pict_type`; the `f_code` test also records
      that FFmpeg emits 5 where ISO/IEC 13818-2 allows 1..4, so the four-bit
      `fcode_*` register fields must clamp.
- [x] MPEG-2 decode on vdpu2 (client 1): `src/hal/m2vd.rs` writes the 159-word
      register file (`ppReg[50]` plus sw50-57/61-64/120/122/131/134/135/136/148)
      with the values `hal_m2vd_vdpu2.c` puts there - mode 5 with a sequence
      extension and 6 for MPEG-1 syntax only, the picture coding extension
      overriding the MPEG-1 `f_code` fields, `filtering_dis` and `init_qp` held
      at the C values, and the quantiser table in raster order behind `sw61`.
      `src/codec/m2v/dpb.rs` rotates the three C reference heads, and
      `src/codec/m2v/decoder.rs` cuts picture groups the way
      `mpp_m2vd_parser_split` does, copies each group into one reused DMA buffer,
      and points the hardware at the first slice's start code with an 8-byte
      aligned base plus a bit offset, so the hardware reads slice headers itself.
      Three frame buffers cover the rotation and are recycled, never freed per
      picture. `rk-mpp-rs m2vdec <in.m2v> <out.yuv>` is the CLI path.
      On t6 `m2v_320x240_12.m2v` (IPBPB, 15 slices per picture) and
      `m2v_64x64_3.m2v` (a sequence header in front of every picture, so three
      header-only groups that submit nothing) are byte-identical to
      `mpi_dec_test -t 2` converted from NV12 to planar, and the per-picture
      `stream_start_bit`/`stream_len` pair is locked to C's own register dump.
      `m2v_320x240_ilaced.m2v` matches FFmpeg within the 2-unit IDCT rounding
      hardware MPEG-2 allows; C MPP is the outlier there, disagreeing with
      FFmpeg on whole pictures and emitting 40 frames for a 6-picture stream once
      it rewinds its own input. `scripts/board_regress_m2v.sh` locks all three,
      and the corpus sweep classifies this fixture as `KNOWN-C-BUG` instead of
      `FAIL`.
      `RFI_M2V_REG_DUMP=1` appends the register image that was submitted to
      `/tmp/our_m2v_regs.txt`, in the text `hal_m2vd_debug=2` prints. Diffed that
      way on `m2v_320x240_12.m2v` all ten jobs agree with C on every word except
      the interrupt status word and the seven that carry DMA-BUF handles (`sw61`,
      `sw63`, `sw64`, `sw131`, `sw134`, `sw135`, `sw148`).
      Field-coded pictures follow the C code - `pic_fieldmode_e`, `pic_topfield_e`
      and a bottom field landing one aligned row into the frame buffer - but no
      fixture here is true PAFF MPEG-2, so that path is still unverified.
- [x] MPEG-4 Part 2 (ASP) syntax (`src/codec/mpg4/parser.rs`): the access-unit
      rule `mpp_mpg4_parser_split` implements, the VOS/VOL/VOP chain, the
      complexity headers and the DivX user-data sniff that selects the
      macroblock-offset mode. The start codes are the ones `mpg4d_parser.c`
      compares against, which is worth stating because the log strings in that
      file mislabel them: the video object layer is `00 00 01 20` (the range runs
      `20` to `2F` and the three-bit volume id rides in the low bits), `B3` is
      the group of VOPs, and `B5` is the visual object, which the C path has no
      case for. `vop_time_increment` is read with `log2bin(resolution - 1)` bits
      as in C rather than the 16 the standard fixes for simple profile, and the
      hardware start word is measured from the end of that header. What C refuses
      we refuse: non-rectangular shape, GMC sprites, scalability, above
      1920x1088, and the divx4/old-xvid shape; an N-VOP never reaches the
      hardware.
- [x] MPEG-4 decode on vdpu2 (client 1): `src/hal/mpg4d.rs` fills the same
      159-word window MPEG-2 uses (`src/hal/vdpu2.rs` holds the shared word
      packer, surface layout and submission path), with decode mode 1, the
      `sw59`/`sw153` interpolation taps written as two's complement, deblocking
      off, `sw122.sw_sync_markers_en` on, the per-type `sw57` bits including the
      direct-motion-vector write enable that only a P picture sets, and the three
      `trb_per_trd_*` ratios of a B picture divided out of `time_bp`/`time_pp`
      exactly as `MPP_DIV` does it. The 128-byte quantiser table and the
      1080p-sized direct-MV buffer live for the life of the channel, and the MV
      buffer is never cleared between jobs, as in C. Matrices reach the hardware
      in raster order with no de-zigzag, because MPEG-4 sends them that way.
      `src/codec/mpg4/dpb.rs` is the two-head rotation of
      `mpp_mpg4_parser_update_dpb`, and `src/codec/mpg4/decoder.rs` drives it.
      `rk-mpp-rs mp4dec <in.m4v> <out.yuv>` is the CLI path.
      On t6 `mp4_320x240_12_ip.m4v` (I plus 11 P), `mp4_320x240_12_ibpb.m4v`
      (I P B B repeated, so both reference heads, the B ratios and the C output
      order are all exercised) and `mp4_150x100_9_ip.m4v` (an odd size, so the
      coded 160x112 picture is cropped back to 150x100 on readback) are byte-exact
      against `mpi_dec_test -t 4`. `RFI_MP4_REG_DUMP=1` appends the submitted
      image in the text `hal_mpg4d_debug=1` prints; diffed that way on
      `mp4_320x240_12_ip.m4v` all twelve jobs agree on every word except the eight
      that carry DMA-BUF handles and the first picture's `sw51`/`sw122`, where
      C's access unit still holds the 54 bytes of sequence headers in front of the
      VOP start code. The absolute bit position the hardware decodes from is the
      same either way.
      C releases a reference only when the next one arrives, so an I picture
      reaches the output *after* the B pictures decoded against it; the rotation
      reproduces that order rather than a corrected presentation order, which is
      what keeps the byte compare meaningful.
      Shared with C and still open: the split ends an access unit at any
      `00 00 01`, so a stream that uses resync markers (GOB headers) breaks there
      and here. Every fixture here sets `resync_marker_disable=1`.
- [x] H.263 picture syntax (`src/codec/h263/parser.rs`): the access-unit rule
      `mpp_h263_parser_split` implements and the picture header of
      `h263_parse_picture_header`, field for field. A picture starts where a
      rolling 32-bit byte window reads sixteen zeros, a one bit and a five-bit
      GOB number of zero (`00 00 80` to `00 00 83`), which is why the C masks are
      `0x00FFFF80` and `0x7C` and not the whole low byte: the two bits under the
      GOB number are the top of `MQQUANT`, so a mask that clears them loses
      pictures whose quantiser is 16 or more. Each splitter call re-enters with
      the window at all ones, which makes the end of one picture the start of the
      next. There is no width or height in the header at all - `source_format`
      picks SQCIF, QCIF, CIF, 4CIF or 16CIF - so custom source formats, the
      extended PTY and the UMV / advanced-prediction / PB-frame bits of the
      PTYPE tail are refused exactly where C refuses them, and pictures ahead of
      the first I picture are dropped by the `found_i_vop` gate rather than
      reported as an error.
- [x] H.263 decode on vdpu2 (client 1): `src/hal/h263d.rs` fills the MPEG-2
      register window - `hal_h263d_vdpu2_reg.h` differs from
      `hal_m2vd_vdpu2_reg.h` only in field names - with decode mode 2,
      `sw52.sw_apf_threshold` in the same bit `sw52` slot MPEG-4 calls
      `adv_pref_thrd`, `sw51.sw_init_qp` at the picture's own quantiser, the
      `sw59`/`sw153` taps at -1, 3, -6 and 20, and `sw136` at half-pel forward
      resolution with only `sw_prev_pic_type` varying. What the format leaves at
      zero matters as much: no quantiser table behind `sw61` and no direct
      motion-vector buffer behind `sw62`, because there are no custom matrices
      and no B pictures, so `sw134`/`sw135` stay unwritten while `sw131` and
      `sw148` both take the single forward reference. `sw120`'s macroblock
      remainders are written for every picture rather than only for a DivX build,
      and `sw122.sw_vop_time_incr` is the constant 30000 that
      `h263_fill_picture_parameters` writes rather than anything from the stream.
      `src/codec/h263/dpb.rs` is the one-head rotation of
      `mpp_h263_parser_update_dpb`: a picture is displayed as it is decoded, so
      two frame buffers alternate in the order they were decoded and the
      end-of-stream flush has nothing left to release.
      `rk-mpp-rs h263dec <in.263> <out.yuv>` is the CLI path.
      On t6 `h263_cif_12_ip.263` (I plus 11 P), `h263_qcif_6_ip.263` and
      `h263_4cif_10_ip.263` are byte-exact against `mpi_dec_test -t 3` converted
      from NV12 to planar, and `RFI_H263_REG_DUMP=1` appends the submitted image
      in the text `hal_h263d_debug=1` prints; diffed that way on the CIF stream
      all twelve jobs agree on every word but the four that carry DMA-BUF handles
      (`sw63`, `sw64`, `sw131`, `sw148`). GOB start codes with a non-zero number
      never end a picture here, in C or in Rust, so a stream that uses them for
      resynchronisation is a shared open case rather than a difference.
- [x] VP8 picture syntax (`src/codec/vp8/parser.rs`): the frame tag, the
      key-frame start code and its 14-bit size pair, segmentation, the loop filter
      type / level / sharpness and both delta tables, the six quantiser deltas, the
      refresh and copy flags, the two sign biases, the coefficient and
      motion-vector probability updates, `mb_no_coeff_skip`, and the DCT partition
      offsets of `vp8hwdSetPartitionOffsets`. Three C behaviours are copied rather
      than corrected, because the register file and the next picture both depend on
      them: the entropy-probability backup is taken at the moment
      `refresh_entropy_probs` is read, *before* the update loops, and is restored
      only after the picture reached the hardware;
      `prob_mb_skip_false`, `prob_intra`, `prob_ref_last` and `prob_ref_golden` are
      not part of that set and a key frame does not reset them, so a key frame
      carries whatever the last inter picture left there; and the header reaches
      the end-of-stream check at the same three points the C parser checks it.
      `vp8d_parser_init` fixes `decMode` to `VP8HWD_VP8` and nothing else writes
      it, so the VP7 and WebP branches - including `vp7_header_parser`, which
      returns a protocol error for every feature it is offered - are dead code in
      the decoder being matched. A VP7 stream is refused rather than decoded
      differently, and so is a key frame scale factor, which C parses into
      `scaledWidth` and never programs.
- [x] VP8 decode on vdpu2 (client 1): `src/hal/vp8d.rs` fills the same 159-word
      window MPEG-2, MPEG-4 and H.263 use, with decode mode 10, the six endian
      bits, burst 16, `sw_filtering_dis` only when the loop filter level is zero,
      and `sw_skip_mode` as the complement of `mb_no_coeff_skip`. Two buffers live
      for the life of the channel: the 64 KiB probability image, of which
      `hal_vp8hw_asic_probe_update` writes 1208 bytes and the rest reads back zero
      in C as it does here, and the 3072-byte segment map, which is cleared on a
      key frame only - an inter picture with segmentation enabled and the map not
      refreshed continues on what the hardware wrote last time.
      The reference buffers are aliases rather than copies: `src/codec/vp8/dpb.rs`
      reproduces `vp8d_ref_update` with a fifteen-slot table and a per-slot owner
      count, so `copy_buffer_to_golden == 2` and its three siblings share a surface
      the way the C `VP8Frame` objects do, and the order matters because the
      alternate buffer is updated first. A key frame points `sw131` at the picture
      it is writing, one macroblock-count past its start, and the two sign biases
      are programmed as one-byte address offsets on `sw136` and `sw137`.
      Each DCT partition past the first gets its own dma-buf offset on the same
      buffer (`sw140` to `sw144`, then `sw146` and `sw147`), and the interpolation
      taps need care: `sw_pred_bc_tap_1_1` takes `mcFilter[1][2]`, one column
      right of its field name, and the eleven words disagree about which tap comes
      first.
      `rk-mpp-rs vp8dec <in.ivf> <out.yuv>` is the CLI path and `rk-mpp-rs vp8`
      prints the frame headers without hardware.
      On t6 all six fixtures are byte-exact against `mpi_dec_test -t 9` converted
      from NV12 to planar (`scripts/board_regress_vp8.sh`), covering an I/P
      stream, a segmented stream, an alternating-quantiser stream, an odd 150x100
      size, and two auto-alt-ref streams - the 24-display one carries 26 packets,
      two of which are hidden reference frames that both decoders decode and
      neither displays. The same script dumps `hal_vp8d_debug=4` and compares every
      submitted register image: all twelve jobs agree with C on every word except
      the eight that carry a dma-buf handle. Two probability images, the key frame's
      and a later inter picture's, are committed under `tests/data/vp8/` and
      compared byte for byte offline, which is what caught the motion-vector tail
      of the image being dropped.
      Shared open cases: no fixture here uses more than one DCT partition, absolute
      segment features, or segmentation that stays enabled without refreshing the
      map, so those three register paths are ported and unit-tested from the C
      source but have no board evidence. Damaged-stream behaviour is untested as
      well. `mpi_dec_test` ends a VP8 stream by submitting an earlier picture
      again - the thirteenth job of the twelve-packet stream is the key frame with
      different buffers attached and it produces no output frame - so the register
      comparison treats a trailing C job that repeats an earlier one as C's own
      repeat, the same habit MPEG-2 and H.263 show in their own regressions.
- [ ] AVS+ and AVS2 decode

### Encoders
- [x] H.264 Baseline IDR (rkv-enc/VEPU580): SPS/PPS writer, Annex-B
      escaping, DMA input/output/reconstruction/thumb buffers, MPP register
      sections and address offsets, status polling and hardware error checks.
      On t6 the 320x240 FIXQP26 output is 861 bytes and deterministic. Its
      SPS, PPS and 825-byte IDR NAL are byte-identical to the installed C MPP
      library; FFmpeg decodes the result to the same 115,200 bytes as the
      official stream. Y/U/V PSNR against the synthetic NV12 source is
      51.96/52.75/55.85 dB. The ignored board test locks the full-stream and
      IDR hashes and runs independent FFmpeg decoding. The same test now
      covers 64x64, 320x240, 640x480, 1280x720, 322x242 and 1366x768,
      including non-16-aligned crops. Every SPS/PPS/IDR NAL is byte-identical
      to a freshly generated C MPP golden, deterministic across repeated runs,
      and independently decodes with at least 45 dB plane PSNR
- [x] H.264 (rkvenc): the implemented encode surface, including per-slice
      low-delay output, is closed. `MPP_ENC_SPLIT_OUT_LOWDELAY`, `slen_fifo` and
      poll-with-slice-info are now consumed by the encoder rather than only
      present in the service ABI. `SliceInfo` is C's one-word structure,
      `POLL_HW_IRQ` returns `count_ret`, and the VEPU580 25-bit length/bit-31
      last fields have host coverage. The rate-control loop beyond fixed QP and the three modes is ported and
      byte-exact but `roi`-weighted QP per region and the QP *map* input path are
      proved only at the table level; P pictures are byte-matched at eleven sizes
      across the FBC header threshold and at both widths above 4096 (4160x64 and
      4608x256, two pictures each), so the wide cells are no longer IDR-only. Everything else in the encoder - fixed QP and CBR/VBR/AVBR,
      the QP map and ROI tables, deblock, the unset-quality start QP, the scene
      detector, GDR, slice split, the >4096 CIME extension lines, P pictures, the
      DPB, the SEI NAL units and Baseline/Main/High - is each in its own entry
      below with its board cells recorded.
- [x] **The QP map and ROI reach the encoder through the reference's own DMA
      tables, not the `roi_qthd` rows.** The develop source settles a misreading:
      `setup_vepu580_rc_base` writes the picture-wide `qp_min`/`qp_max` into all
      eight `roi_qthd*` rows unconditionally - per-ROI QP never touches them. The
      real path is `Vepu580RoiH264BsCfg` (u64 per MB, force-intra bit 61, qp-adjust
      enable bit 62) plus `Vepu580RoiQpCfg` (u16 per MB, 7-bit signed adjustment,
      absolute-mode bit 15), both addressed from the base registers with C's
      enable-check. Regions map to blocks rounding *up* (like the 54x path's
      `set_one_roi`), stride padded to four, later regions winning, and the
      `qp_adj_en = !!qp_adj` rule. `scripts/board_h264_roi_vs_mpp.sh` runs the
      region list through the reference's own `mpp_enc_roi_utils.c` on both a
      fixed-QP and a CBR cell: byte-identical access units, 2/2.
 - [x] **Deblock fine tuning, with the beta quirk preserved.** The three
       `MppEncH264Cfg` fields reach `synt_sli2` through exactly the two writes
       `setup_vepu580_codec` performs: `dis_dblk_idc` truncated to two bits and
       `sli_alph_ofst` as a four-bit two's complement. VEPU580's HAL never writes
       `sli_beta_ofst` - the field exists in the register header and no statement
       touches it - so beta configures but stays 0 in the image, and a test locks
       that in (beta 5 with the register still zero) because byte-exactness means
       dropping beta the way the reference does. `scripts/board_h264_deblock_vs_mpp.sh`:
       default / alpha -3 / disable cells, byte-identical.
 - [x] **`cal_first_i_start_qp`: the first picture of a caller who set no
       quality.** Ported at `rc_model_v2.c:1554` with the C semantics kept, the
       unsigned ones included (the empty-picture branch returns 15 at target 1 and
       45 at target 1000 - i64 intermediates get that wrong). Wired through
       `quality_auto` end to end: CLI `qp auto`/`-1`, and the `uuid_rc_cfg` SEI
       prints the caller's `-1` sentinel, not the derived QP - `update_rc_cfg_log`
       prints what was configured. `scripts/board_h264_deblock_vs_mpp.sh` cbr and
       vbr cells run the whole QP trajectory from picture zero (the first
       picture's 23 against the library default 26 at 320x240/300k): both
       byte-identical, 5/5 cells with the deblock ones.
 - [x] **The scene detector `vepu580_h264e_tune_stat_update` is ported.** The
       score-window state machine over the STATUS histograms (five-window `madp`
       fold, three-row threshold with `mvbit` 15, 41-point sliding score, the
       majority rule over the flag matrix) drives `apply_tune`: the eleven
       current-motion writes, the P-frame chroma-KLUT row switch, and the
       ATR/CIME/RIME/intra/skip tables moving along the `ap*2+curr` row. The
       default rows cross-check against the 2024-04 capture - which also caught a
       stale note: the CIME {4,7,20,20} and RIME {4,16,64} words were *not* moved
       by section 3 after all, the newly regenerated `Vepu580WgtCommon` fields read
       back exactly the captured row. `scripts/board_h264_scene_vs_mpp.sh` cuts
       testsrc to testsrc2 mid-clip, eight fixed-QP frames: the whole stream
       byte-identical to the reference through the cut. IPC scene mode is not
       exposed on the CLI; only the default mode is wired and compared.
 - [x] **GDR / progressive intra refresh is byte-identical to the reference, four
       cells.** `apply_gdr` carries the refresh length derivation (rows per picture
       over the coded grid, capped by the GOP), the bands sliding with the two-line
       overlap, the column bands spanning the height, the `roi_*` block records the
       encoder reads back, the search-axis clamp a refresh picture takes, and the
       window's deferral past the first `igop`. Two behaviours only the board could
       settle:

       *the sweep opening.* The reference re-prefixes SPS, PPS and a
       `recovery_point` SEI (payload `06 02 24 40`) at every IGOP boundary and
       emits no IDR there - the reading that a `hdr_status` which never re-arms
       repeats nothing is what this driver did until that cell said otherwise. The
       uuid_version and uuid_rc_cfg SEIs still ride with the IDR only, once a
       session.

       *the `refresh_num == 1` corner.* `MppEncROIRegion.y` is a `RK_U16`, so the
       second band's `16 * n - 32` is stored as 0xFFF0, not as -16; the clip
       `vepu580_h264_set_one_roi` applies then leaves an empty range and the
       picture forces no intra at all. Clamping the signed value to row 0 instead -
       what this driver did - forces two extra macroblock lines and cost 836 bytes
       on that one picture (1098 against the reference's 262). Being byte-exact
       here meant reproducing a 16-bit truncation the C never intended.

       `scripts/board_h264_gdr_vs_mpp.sh` covers row-4, col-4, row-8 and row-1 a
       picture over 25 frames at a GOP of 12 - two sweeps and an IGOP boundary
       inside the file - comparing whole files: 4/4 identical, 35 NAL units each.
       The six fixed-QP GOP cells and the cbr / vbr / avbr cells were re-run after
       the change and stay byte-identical.
 - [x] **Slice split is byte-identical to the reference, four cells.**
       `apply_split` covers the supported modes with each mode's field combination
       asserted against `hal_h264e_vepu580.c`, arguments bounded by the fields that
       carry them, and `split_none` checked to move nothing beyond the recorded
       image. Three host tests, and the board cell now run:
       `scripts/board_h264_slice_vs_mpp.sh` compares the whole picture file -
       slices arrive contiguously because `split_out = 0` keeps `slen_fifo` down
       and the normal `enc_done` poll in place - for BY_CTU at 96, 8 and 150
       blocks a slice and BY_BYTE capped at 256: **4/4 byte-identical**, 28, 232,
       16 and 38 NAL units respectively. The 8-blocks cell puts 38 slices across a
       320x240 picture, past the 32-entry slice FIFO, so it also pins the
       `slc_done_en` decision - had the interrupt arming been wrong the encoder
       would not have finished the picture at all, let alone matched it.
 - [x] **Low-delay slice packets are byte-identical to the reference, five
       cells.** `split_out=1` arms `slen_fifo`, enables the slice interrupt, and
       drains `POLL_HW_IRQ` one entry at a time until VEPU580 reports `last`.
       `scripts/board_h264_lowdelay_vs_mpp.sh` compares both concatenated
       streams and per-packet offsets for BY_CTU 96/150/8, BY_BYTE 256 and a
       six-picture IDR+P stream: **5/5**. The 8-block cell crosses the 32-entry
       hardware FIFO and emits 38 data packets; the P stream has four packets in
       each picture. The first packet differs in total length only by the
       deliberately different version-SEI text; every slice boundary and every
       following packet length is equal.

       The C probe originally deadlocked above eight slices because it called
       `encode_put_frame` synchronously and only then drained packets. MPP has a
       fixed output-task pool, so the corrected probe reads packets on a worker
       thread. Rust itself drains the hardware FIFO before materializing packet
       views. The kernel may attach `last` to the final non-empty slice or emit a
       trailing zero-length EOI entry depending on timing; both representations
       end at the same byte offset and the comparator normalizes only that final
       zero.
 - [x] **Widths above 4096 encode byte-identically - the CIME extension lines
       work.** The path is `setup_hal_bufs` plus `setup_vepu580_ext_line_buf`:
       once the width aligned to 64 clears 4096 the encoder allocates
       `(aligned_w - 3072) / 64 * 480` bytes, points `ebuft_addr` (reg182) and
       `ebufb_addr` (reg183) at it, adds the buffer size to reg182's multi-offset
       entry so the second half reaches `ebufb`, and states both halves' sizes to
       the device through the two `RCB_INFO` calls. The crate used to refuse the
       geometry outright.
       `scripts/board_h264_wide_vs_mpp.sh` on 4160x64 and 4608x256: **2/2
       byte-identical**, five NAL units each, with the SPS carrying the level the
       size forces (3.0 and 3.2) on both sides.
       The first run of these two cells failed, and the reason was the harness,
       not the driver: `h264e_probe` hard-codes an intra GOP of 60 while the
       crate's CLI defaults to 1, and the script passed neither - so the rc-cfg
       SEI came out `-g:60` against `-g:1`, one character apart, while the SPS,
       the PPS and the 8099-byte IDR were already identical. Both sides now state
       the GOP explicitly. A cell that fails for the wrong reason is still worth
       recording, because the wrong reason is what the next person would hit.
 - [x] **H.265 encode started: parameter sets, and a register image derived
       rather than replayed.** `codec::h265::encoder` writes the VPS/SPS/PPS of a
       fixed-QP IDR and feeds each NAL back through the independent
       `codec::hevc::parser` (13 host tests) - that parse-back is the byte-comparable
       gate M8 needs. `hal::h265e` derives the CTL, geometry and RC-lookup windows
       word by word from `hal_h265e_v580_gen_regs`, `set_pp_regs`, `set_rc_regs` and
       `set_slice_regs` through the generated `h265e_regs` map, with ten host tests
       on the fields the C source fixes (320x240 geometry, the FIXQP quad, the
       sixteen `roi_qthd` rows, the IDR syntax block). The wgt, rdo and osd
       tuning windows and the AQ/KLUT/LAMD byte tables are left at zero and asserted
       so - **a derivation and a test bed, not a submit-capable encoder**, because
       there is no captured reference H.265 job to compare the tuning against.
       `tools/h265e_probe.c`, `scripts/board_h265_qp_vs_mpp.sh` and
       `scripts/h265_hdr_cmp.py` are the harness, `h265-enc` writes the parameter
       sets from the CLI, and `board_parameter_sets_match_the_c_encoder` sits
       `#[ignore]`d so the live comparison stays opt-in.
       The H.265 job has six windows, not the seven of an H.264 job, and the map now
       says so. The vintage's `H265eV580RegSet` is `reg_ctl`, `reg_base`,
       `reg_rc_klut`, `reg_wgt`, `reg_rdo`, `reg_osd_cfg` (plus `reg_dbg`, which is
       not part of a job): the scan/quant window at word 2176 that H.264 writes as
       `scl` (679 words) has no H.265 member, and the window sharing H.264's `param`
       offset at word 1472 is `hevc_vepu580_wgt` at 374 words. Carrying the H.264
       lengths over left **247 of the 292 named `wgt` fields** - the intra and inter
       weight tables from word 1652 upward - outside every section, and the first
       write into `rdo` past word 2170 would have indexed off its 122-word array.
       Two host tests stand between the regenerated map and a submit path:
       `windows_match_the_generated_section_table` (names, first words, lengths, plus
       a tiling check that keeps every written window below the STATUS word at 4096)
       and `the_last_word_of_every_window_is_addressable` (write and read back the
       last word of each window). Both were checked to fail rather than assumed:
       narrowing `wgt` to the old 180 words turns them red on
       `window wgt has the wrong length`.
       Source of truth, since the two vintages disagree: `h265e_regs` is generated
       from the vendored `refs/mpp-2024-04/rkenc/h265e/hal_h265e_vepu580_reg.h` and
       not from `refs/mpp` (master), because every byte comparison on the board is
       against the vintage. The vendored copy is md5
       `c3ea6482ea1801226a61eb00c2278ae6`, which is the file
       `~/rk-mpp-oracles/mpp-old/mpp/hal/rkenc/h265e/` holds on t6; the master copy
       is 1381 lines against the vintage's 3340. Both register maps now regenerate
       byte-identically, `rustfmt` included, from `tools/gen_vepu580_regs.py`, which
       resolves its headers from its own location rather than the working directory
       and no longer emits a second const when two struct views name the same
       padding - the duplicate `CTL.reserved1_3` was the symptom.
       **The header gate now passes on the board, 3/3, and the first run did not.**
       `scripts/board_h265_qp_vs_mpp.sh` on t6 compares the VPS+SPS+PPS prefix of
       `mpi_enc_test` and of `h265-enc`: 320x240, 640x480 and 1280x720 are
       byte-identical (82, 82 and 83 bytes). Two defects stood in the way, and both
       were invisible to the host tests because those tests had been derived from
       the same misreading:
       1. The NAL header's second byte carried `unit_type & 1` in its top bit, which
          is `nuh_layer_id` bit 5, not part of the NAL type. Every odd unit type
          claimed layer id 2; the SPS came out `42 81` against the oracle's `42 01`.
       2. The payload was not escaped. The oracle's VPS is `...0160 0000 0300 8000
          0003 0000 0300 78...`; deleting the three `03` bytes gives back the RBSP
          this writer produced byte for byte, so the bit patterns were already
          right and only Annex-B escaping was missing. The escape cannot come from
          `h265e_nal_escape_c`: that function leaves the insertion commented out in
          both vintages (`h265e_header_gen.c:42`). VEPU580 writes its parameter
          sets from `reg0237_synt_sps` / `reg0238_synt_pps`, and the hardware
          formatter escapes them - the C source's no-op describes the software
          fallback path only.
       A third field was wrong behind the escape: `pps_cu_qp_delta_enabled_flag`.
       `h265e_set_pps` sets `m_useDQP = (rc_mode != FIXQP)`, and the fixed-QP board
       cell writes PPS `44 01 c0 f1 80 9b 20` where the hard-wired 1 produced
       `c0 f3 c0 4d 90` and shifted every later field. `EncoderConfig::rc_mode`
       now derives the flag, with a host test covering the CBR/VBR branch. The
       `-t` value was wrong too: this vintage's `MppCodingType` makes HEVC
       `0x01000004`, which `mpi_enc_opt_t`'s `atoi` needs as `16777220`; `-t 9` is
       VP8, and that was the script's default before the first run.
       The three oracle streams are checked in as
       `tests/data/h265e_c_fixqp_qp26_{320x240,640x480,1280x720}.265` (860, 1618 and
       3585 bytes) and `parameter_sets_match_the_recorded_c_streams` compares the
       prefix on the host, so this gate no longer needs the board to stay honest.
       **The register job is captured, and the derived image matches it to one
       bit.** The same `mpi_enc_test` line under `/tmp/mpp_trace.so` writes six
       windows of 24, 94, 57, 374, 807 and 288 words - the six of
       `H265eV580RegSet`, with no SCL - which confirms the section table
       independently of the header. `tools/gen_h265e_vepu580_template.py` turns the
       trace into `src/hal/h265e_vepu580_template.rs`, and `hal::h265e` now starts
       from that template and derives on top of it, the lineage H.264 used.
       `tools/h265e_trace_diff.py` diffs the two by named field: **1026** differing
       words from the zero-filled starting point, **8** once the template lands, and
       **1** today - `enc_pic.mei_stor`, word 192 bit 3, because the C harness
       hands the encoder a motion-info buffer and this driver has none to point at.
       CTL, BASE, RC_KLUT, WGT, RDO and OSD are otherwise word-for-word equal, and
       `only_the_motion_info_bit_differs_from_the_captured_job` holds that line so a
       changed field fails with the word and both values.
       The capture corrected three H.264 habits carried across: the sixteen
       `roi_qthd` rows and the `madi`/`md_sad` threshold triples belong to the
       non-FIXQP branch of `vepu580_h265_set_rc_regs`, `synt_pps.cu_qp_dlt_en`
       follows the same `rc_mode != FIXQP` rule as the PPS writer, and the rows
       carry no picture QP in a fixed-QP job. The `madi`/`md_sad` words keep the
       traced 4/9/15 although `global_cfg_set` assigns 25 and 20/30/40; the
       assignment is recorded as unexplained rather than overwritten.
       Still missing for an IDR payload: a submit path that owns its buffers (the
       template's address words are the oracle's) and the motion-info surface if the
       image is to equal the capture outright.
       **The first submitted IDR is byte-identical at three sizes.**
       `h265-idr` (`src/hal/h265e_session.rs`) owns every surface the job points at -
       input, output, two FBC reconstructions with display and motion-vector thumbs,
       and the motion-info buffer - patches the 160..175 address words, carries the
       C's multi-offset table (161/162 chroma planes, 164/166 FBC bodies, 172 output
       size, 175 packet length), submits the six windows and polls. The hardware's
       slice NAL was already identical before this path existed; what was missing was
       the software half, and `tests/data` settled it: `uuid_version` and
       `uuid_rc_cfg` as two prefix SEIs, whose rc line prints `w*h*30/8` as the
       target rate.
       `scripts/board_h265_idr_vs_mpp.sh`: 320x240 (860 bytes), 640x480 (1618) and
       1280x720 (3585) are **byte-identical to `mpi_enc_test`**, SEIs included.
       `board_idr_matches_the_c_encoder` runs the same script as an ignored board
       test.
       Three fields had to leave the template for 640x480 to match: `src_strd0/1`
       (the 16-aligned luma stride), `set_me_regs`' CIME search range, and
       `set_me_ram`'s `cme_rama_h`/`cme_rama_max`/`cach_l2_tag`, whose C loop fills
       the line buffer until the window holds the search. The 1280x720 cell then
       needed no further change.
 - [x] **H.265 P pictures: the GOP is byte-identical.** The C's GOP configuration
       (an explicit `-g`) makes two things true that the single-IDR cell never shows:
       the SPS carries one long-term entry (`long_term_ref_pics_present = 1`,
       `num_long_term_ref_pics_sps = 1`, poc 0, unused by the current picture), and
       every P predicts from the **opening IDR** while reconstructing into the other
       slot - `synt_refm1.used_by_lt_flg0` says so, and rotating the reference to the
       previous P instead made the third picture's CABAC tail differ by three bytes.
       The traced P register delta is `dtrns_cfg.dspr_otsd = 1`,
       `enc_pic.num_pic_tot_cur = 1`, `rdo_cfg.ltm_idx0l0 = 1`, NAL type 1,
       `synt_sli0.sli_type = 1`, `num_refidx_act_ovrd = 0`, POC in `synt_sli2`, and
       `lamd_modb_qp` (52 words, `vepu5xx_common.c:183`) over the IDR's intra lambda
       table. From the third picture the reference list is stated explicitly:
       `ref_pic_lst_mdf_l0`, `lst_entry_l0` and `num_negative_pics` all 1.
       `scripts/board_h265_gop_vs_mpp.sh` compares whole streams: 320x240 at 2, 3 and
       5 pictures (883, 904, 946 bytes), 640x480 and 1280x720 at 2 (1643, 3617), and
       a horizontally shifted second frame (1032 bytes) are **byte-identical**.
       `board_gop_matches_the_c_encoder` runs the three-size cell as an ignored test.
 - [x] **H.265 rate control: the register half.** The non-FIXQP branch of
       `vepu580_h265_set_rc_regs` is ported and checked against a traced CBR job
       (`mpi_enc_test -rc 1 -bps 300000:250000:350000`, 320x240, second picture).
       It is not H.264's branch copied over: the CTU counts are 64x64
       (`rc_ctu_num = ceil(w/64)`), `aq_mode` is 1, the first difference threshold
       is `2 * negative_bits_thd` rather than 4x, and `rc_adj1.qp_adj8` stays 0.
       With the harness's quality band (10..51) and that picture's decision
       (QP 22, 580 bits) the derived image is word-for-word the traced one - only
       buffer addresses and `mei_stor` differ, both expected. Units are pinned by
       `the_rate_control_image_matches_the_traced_cbr_job`: `rc_cfg = 0x5007`,
       `rc_qp = 0x2b320000`, `ctu_ebit = 464`, and the threshold words
       `-90/-45/+45/+90` then five `0x7fffffff`.
 - [x] **H.265 CBR: the encoding path is byte-identical, with the C's decisions.**
       `h265-gop-rc` takes a per-picture `qp:bits` list, and
       `scripts/board_h265_cbr_vs_mpp.sh` feeds it the two decisions the C made on
       the same input (IDR 26 with 86955 bits, P 25 with 582 bits): the 1169-byte
       stream - CBR PPS (`cu_qp_delta` on), the `cbr` SEI line, the IDR and the P
       NAL - is **byte-identical** to `mpi_enc_test -rc 1 -qc 26:10:51:10:51`.
       Two things had to move for that: the RC prefix needs `build_prefix_line`
       (mode-dependent PPS + SEI), and the GOP's long-term register state
       (`rdo_cfg.ltm_idx0l0`, `lt_ref_pic_prsnt`, `num_lt_ref_pic`) belongs to
       *every* picture of the sequence, the opening IDR included, not only to P.
       An earlier note in this file claimed the C's first QP was 29 and did not move
       with size or rate; that was a decode error on this author's side (the field is
       not at bit 0 of `enc_pic`). With `-qc 26` the IDR is QP 26; with the default
       `-qc` the harness leaves `init_quality` at -1 and takes a derivation path the
       H.264-style `cal_first_i_start_qp` does not reproduce.
 - [x] **H.265 STATUS measurement, and the CBR model loop wired end to end.**
       `measured_from_status` reads the same STATUS words as H.264 but with H.265's
       divisors: `quality_real = qp_sum / (mb64_num << 6)`, `madi / num_b16`,
       `madp / num_ctu`, and only the hardware slice bytes for `bit_real`.
       `open_cbr` builds `rc_model::Model` (GOP 60, 30 fps, the caller's quality
       band and init QP) and `encode_frame` now runs start -> registers -> submit ->
       STATUS -> `end_with`. On the 320x240 CBR cell the first decision is the C's
       (`86956` bits / QP 26), the prefix and IDR NAL are byte-identical, and the P
       NAL is two bytes shorter. `H265_RC_DEBUG=1` prints each decision and its
       measurement.
 - [x] **The model-driven CBR loop is byte-identical on a plain GOP.** The `582` /
       QP 25 target that did not fit `bits_model_alloc` was not a model error: with
       the default `-g 60` the harness schedules the second picture as a
       virtual-intra/refresh frame, which takes the refresh branch of the allocation.
       With `-g 0:60:0` (no VI frames) the C's second register carries
       `ctu_ebit = 7456`, i.e. 9320 bits / QP 27 - exactly what `rc_model::Model`
       chooses. `h265-cbr` on that cell produces a **1163-byte stream byte-identical
       to the C's**, both QPs chosen by the model. The same cell also settled the
       reference marking: a plain GOP marks short-term reference 0
       (`num_negative_pics` + `used_by_s0_flg`), while the VI/default GOP marks the
       SPS long-term entry (`num_lt_pic` + `used_by_lt_flg0`), and the SPS grows the
       LT entry only in the latter.
 - [x] **VBR and AVBR ride the same path and are byte-identical.** `h265-rate`
       selects the controller (`RcMode`), the SEI line and the model mode together;
       the vintage's `-rc` indices were checked from the emitted SEI rather than the
       usage string (0:vbr, 1:cbr, 2:fixqp, 3:avbr - the help text's order is stale).
       On the plain-GOP 320x240 cell the model-driven CBR and VBR streams are 1163
       bytes and AVBR 1164, all byte-identical to the C encoder.
       The script now runs **thirteen cells**: the explicit-decision path at 320x240
       (1169 bytes); model-driven cbr/vbr/avbr plus a smart GOP at 320x240
       (1163/1163/1164/1163); the same four model-driven cells at 640x480
       (2475/2475/2476/2475); and at 1280x720 (5397/5466/5467/5397). All pass, so
       the model-driven rate control is byte-identical at three sizes, not just the
       captured one.
- [x] **The smart-GOP shape (`vi_len`) and the degenerate `-g 60` cell.** See the
      note above: the documented GOP forms all give the model's own 9320/QP27 second
      decision and match byte for byte, `vi_len = 1` included. Only the invalid bare
      `-g 60` produced 582, and it is not a supported harness input.
      `scripts/board_h265_cbr_vs_mpp.sh` covers the smart GOP at all three sizes.
 - [x] **Every picture of a multi-frame GOP, not just the last.** The register diff
       tool used to compare only the final job of a trace, so an earlier picture
       could drift unnoticed; it now takes `--job=N` and collapses the tracer's
       double print. `h265-regs` gained an `lt` token for the GOP-form image. On a
       six-picture fixed-QP capture (`-g 60 -rc 2 -qc 26`, note that this vintage's
       `-rc 2` is FIXQP and `-rc 4` is not) the derived images check out frame by
       frame: the IDR differs only in `mei_stor`, and each P in exactly four buffer
       addresses plus `mei_stor`.
- [ ] H.265 (rkvenc): `rc:refresh_length`-driven refresh pictures (the GDR-style
      `INTRA_RFI_FRAME` allocation), tiles/WPP/SAO, B frames, multiple references and
      per-slice output.
 - [x] **H.265 intra refresh (GDR): the register path and the sweep.** The HAL's
       `setup_intra_refresh` builds a 64-byte-per-CTU ROI base table whose band is
       `cal_refresh_area(ctu_w, ctu_h, refresh_idx, mode, refresh_num)`; a forced
       CTU gets `0x54` at byte 21, twenty `0x55` bytes from 22 and `0x05` at 42.
       `refresh_base_table` reproduces that, `h265-gop ... row:N` allocates and
       rewrites the table per picture, and the register pass sets
       `enc_pic.roi_en`, `reg0178`, and the CIME search axis the refresh mode
       overrides. A unit test pins the row bands for indices 0 and 1.
- [ ] H.265 GDR against the C oracle. With `tools/h265e_probe.c` extended for
      multiple pictures and `rc:refresh_*` (`REFRESH_EN=1 REFRESH_MODE=0
      REFRESH_NUM=1 QP_DELTA=0`; note the RC key is `rc:qp_ip`, not
      `rc:qp_delta_ip`), the two prefixes match but the slice NALs do not: the C
      writes 912 bytes over four pictures, the crate 1740. Our band is wider on the
      same indices, so the next step is a trace of the refresh job's `reg0178`,
      `roi_en` and search-axis writes rather than more table arithmetic.
      The trace answered it, but from the other side: the C never refreshed at all.
      With the trio set through `mpp_enc_cfg_set_u32` (the key type matters) and
      `h265e_proc_cfg` deriving `refresh_length = ceil(mb_rows / refresh_num)` as it
      should (`h265e_api.c:600`), every job still has `roi_en = 0` and the stream is
      the 912-byte no-refresh one. `set_frm_refresh_flag` is only reached while the
      ref-config generator walks its `st_cfg` list (`mpp_enc_refs.cpp:612`), and the
      probe sets no ref cfg, so `is_i_refresh` stays 0. Producing a C GDR oracle
      therefore needs an explicit `MPP_ENC_SET_REF_CFG` in the probe, not more rc
      keys; until then the crate's GDR path has no oracle and must not be called
      verified.
      Note on the harness cell that exposed this: `-g 60` is parsed by
      `sscanf("%d:%d:%d")` as `gop_mode = 60` with the length fields left at zero -
      an input `mpi_enc_gen_ref_cfg` rejects outright. It is the only input that
      produced the tiny 582-bit second picture; the documented forms all agree with
      the model. `-g 0:60:0` (plain GOP), `-g 0:60:1` (smart GOP with `vi_len = 1`)
      and `-g 0:0:0` (unset GOP) each give the same 7456 `ctu_ebit`, i.e. 9320 bits
      / QP 27, and our model-driven stream is byte-identical on the `vi_len = 1`
      cell too (1163 bytes). A fixed-QP 6-frame run under `-g 60` is a plain
      IDR + five P, so the refresh behaviour is RC-side only.
      The schedule itself is `mpi_enc_gen_smart_gop_ref_cfg`
      (`utils/mpi_enc_utils.c:854`): one LT entry with `lt_gap = gop_len`, then
      `st_ref[0]` as `REF_TO_PREV_INTRA` (the first P after every IDR is a
      virtual-intra frame), `vi_len - 2` plain Ps when `vi_len > 1`, then another VI.
      The RC's refresh allocation branch is gated on `rc:refresh_length`
      (`mpp_enc_impl.cpp:1302`), which is separate from the harness's `vi_len` and is
      not exercised by these cells.
      The register side already supports both markings, and the model now also
      accepts the reference's degenerate GOP forms: `igop <= 0` and `igop == 1`
      normalise to 500 (the all-intra form pins `init_ip_ratio` to 16), where it used
      to refuse them.
- [ ] H.265 (rkvenc): the VI/refresh GOP schedule (`MppEncRefCfg`'s frame modes, which
      decide when a picture takes the refresh allocation and the LT marking),
      tiles/WPP/SAO, B frames, multiple references and per-slice output.
      Note on the harness cell that exposed this: `-g 60` is parsed by
      `sscanf("%d:%d:%d")` as `gop_mode = 60` with the length fields left at zero, so
      it is not one of `mpi_enc_gen_ref_cfg`'s modes 0..3. Its second picture gets
      582 bits, and no setting of the documented `bits_model_alloc` branches with the
      real `gop_total_bits` (600000) produces a target that small; the meaningful
      comparison is a stated GOP mode (`-g 0:60:0`), where the model already matches
      byte for byte. A fixed-QP 6-frame run under the same `-g 60` is a plain
      IDR + five P (no VI slice), so the refresh behaviour is RC-side only.
      What the schedule is, from `mpi_enc_gen_smart_gop_ref_cfg`
      (`utils/mpi_enc_utils.c:854`): one LT entry with `lt_gap = gop_len`, then
      **`st_ref[0]` is `REF_TO_PREV_INTRA`** - the first P after every IDR is a
      virtual-intra frame - followed by `vi_len - 2` plain P entries when
      `vi_len > 1`, then another `REF_TO_PREV_INTRA`. The RC allocates VI frames
      through the `INTRA_RFI_FRAME` case, but that branch is gated on
      `usr_cfg.refresh_len`, which comes from `rc:refresh_length`
      (`mpp_enc_impl.cpp:1302`) and is a different setting from the harness's
      `-g` `vi_len`; the 582-bit cell is still not explained by the documented
      branches. The register side is already right for either marking: with
      `long_term` set and the RC numbers supplied, a P/VI image matches the traced
      job word for word, so only the RC frame type and its allocation remain.
      The model now also accepts the reference's degenerate GOP forms:
      `igop <= 0` and `igop == 1` both normalise to 500 (the all-intra form also
      pins `init_ip_ratio` to 16), where it used to refuse them. That is
      `bits_model_init`'s documented behaviour; it does not by itself reproduce the
      582-bit cell, which still needs the VI allocation inputs.
- [x] JPEG encoder (vepu): 8-bit NV12 and packed YUYV/UYVY input to baseline
      4:2:0 output, the eleven MPP quantization tables and quality factor
      1-100. Hardware output is byte-identical across repeated NV12 runs,
      includes the hardware EOI, and decodes independently as baseline JPEG.
      `bus.jpg` quality levels 0/5/10 produce 19,483 / 63,431 / 298,396 bytes;
      quality 10 measures 3.518 ms/frame (284 fps) with reused DMA buffers. A
      321x241 NV12 frame verifies the 328-byte stride, 256-row coded height,
      right/bottom fill, and odd chroma dimensions, with full-range plane PSNR
      Y 66.18 / U 62.20 / V 61.23 dB against FFmpeg. Packed YUYV/UYVY follows
      VEPU2's pixel-stride/no-plane-offset path and independently decodes to
      4:2:0. The ignored hardware tests lock the frame hash, DMA readback,
      packed input, and unsupported-output rejection paths
- [x] JPEG encoder output format on RK3588 VEPU2: reject 4:2:2/4:4:4/gray.
      The VEPU2 HAL has only an input-format register and no `format_out`
      register (`hal_jpege_vepu2_v2.c` programs register 74 from
      `syntax->format`). Experiments with YUYV/UYVY plus a 4:2:2 header
      produced DC errors; the same input with a 4:2:0 header decoded correctly.
      Those unsupported requests must not be represented by changing only the
      JPEG header
- **What the H.264 register file is, and what it was.** It began as a replay: the
      job captured from `mpi_enc_test` over an `MPP_CMD_SET_REG_WRITE` trace with
      eleven address words patched and only the geometry's consequences derived,
      gated on Baseline, level 3.0, GOP 1, QP 26 and everything else frozen in the
      image. That boundary is long gone - profile, entropy, transform-8x8, QP, GOP,
      slice type, the P-picture fields, the two SEI NAL units and the whole
      rate-control block are generated now, and the 24-cell matrix plus the GOP and
      CBR/VBR runs are byte-exact because of it.
       What is still a recorded-image replay: the `cime_multi` / `rime_multi` corner
       of section 3 (where the 2024-04 build and
       master disagree), and the parts of `calc_cime_parameter` whose outputs happen
       to be constant for every fixture
       used so far. Those are listed as open work, not presented as ported.
- [x] VEPU580 register field map generated from the C struct definitions
      (`tools/gen_vepu580_regs.py` to `src/hal/vepu580_regs.rs`): the absolute word,
      bit position and width of every named field in `Vepu580CtlCommon`,
      `Vepu580BaseCommon` and `Vepu580RcKlut`. The header annotates each member with
      its register number (`/* 0x00000300 reg192 */`), so a word comes from that
      annotation rather than from counting fields, and a group whose declared bits do
      not fill a word is a hard error at generation time. `RegisterFile::set` /
      `get` take these fields.
      The layout is checked against reality rather than against itself: reading the
      captured job through the generated fields yields 26 in `enc_pic.pic_qp`,
      `rc_qp.rc_min_qp` and `rc_qp.rc_max_qp`, which is exactly what
      `setup_vepu580_rc_base` writes for a fixed-QP capture.
- [x] First parameterized block of the encoder: `apply_fixed_qp` regenerates the QP
      half of `setup_vepu580_rc_base` - the picture QP, the two RC bounds and the
      sixteen ROI area thresholds - instead of replaying it. A test encodes the
      field set at QP 0, 12, 20, 33, 45 and 51 and asserts that exactly six register
      words move, so a QP write cannot bleed into a neighbour.
- [x] **Fixed-QP encoding is byte-identical to the C encoder.** The reference is
      MPP built from source at the vintage of the library installed on t6, with
      `tools/patches/mpi_enc_test-constrained-baseline.patch` applied - the stock
      harness hard-codes High profile with CABAC and transform-8x8 in that vintage
      *and* in master, and applies them after `-cfg`, so `cabac_en` in a cfg file
      does nothing. QP 10, 18, 26, 32, 42 and 51 on the reference size and sizes
      64x64, 320x240, 640x480 and 1280x720 at QP 18/26/42: **SPS, PPS and IDR
      identical in every case** (`scripts/board_h264_qp_vs_mpp.sh`, which skips the
      harness's own SEI NAL units). Two real bugs came out of getting there:
      - The PPS carried the requested QP. `h264e_pps_init` pins `pic_init_qp` at 26
        and the hardware derives `slice_qp_delta` against it, so writing qp_init
        into the PPS applied it twice: 6 bytes where the reference writes 4, at
        every QP but 26. `pps_qp_fields_ignore_qp_init` now holds the line.
      - The recorded image had been captured from a harness that requested motion
        derivation info, so `enc_pic.mei_stor` was set and `sw171` held that run's
        MD buffer fd. Submitting either as-is points the hardware at a dma-buf
        nobody owns; the builder clears the enable and the address. The job now
        completes again after having hung with `int_sta = 0`.
      `mpi_enc_test` also needs both dimensions to be multiples of 16: it seeks raw
      NV12 by width and ignores `-hstride`, so a stride-packed 322x242 frame shears
      into 9053 bytes of garbage where the packed version costs 1159. 1920x1080 is
      in that class too, because 1080 rounds up to 1088. Sizes that are not
      macroblock multiples stay under decode-and-PSNR coverage in
      `tests/h264_encoder_hw.rs`, where they pass.
- [x] **Profile is a parameter, not a property of the captured image.** Six fields
      of the H.264 window depend on it and `setup_vepu580_codec` derives every one:
      `synt_pps.etpy_mode` and `trns_8x8` mirror the PPS flags,
      `synt_sli0.cbc_init_idc` takes `cabac_init_idc` under CABAC and -1 otherwise
      - which lands as 3 in the two-bit register field, so a Baseline capture reads
      3 and a Main capture 0 and neither is a constant to copy,
      `synt_sps.drct_8x8` follows `transform8x8_mode` like
      `direct_8x8_inference_flag` in the SPS does, and `rdo_cfg.rect_size` /
      `vlc_lmt` are Baseline-and-level-at-or-below-3.0 and CAVLC respectively. In
      the SPS the constraint flags are per profile - Baseline `0xc0`, Main `0x40`,
      High `0x10` (set3, which is the one `constraints()` got wrong for High).
      Verified on t6 against a reference harness reconfigured per run by the board
      script: **24 of 24 cells - 64x64, 320x240, 640x480 and 1280x720 at Baseline,
      Main and High, QP 18 and 26 - byte-identical SPS, PPS and IDR**. The SPS and
      PPS bytes of the three profiles are also locked as unit-test fixtures, so a
      header regression shows up on the host without the board.
      To name those registers at all, `tools/gen_vepu580_regs.py` now parses
      `Vepu580BaseCfg` from the H.264 HAL header as well - reg221 onwards of the
      same BASE window, 83 more fields, with the 228 existing positions unchanged.
- [x] **Multi-frame encoding: the GOP walk, the reference buffers and the
      per-picture registers are in, and P pictures are byte-identical.** What a second picture needs, from
      `setup_vepu580_recn_refr` and `setup_vepu580_codec`: two reconstruction slots
      alternating as `curr_idx` / `refr_idx` (an IDR leaves the reference reads on
      its own slot, since there is nothing to read); `synt_sli0.sli_type` 2 for I
      and 0 for P with `frm_num`; `synt_sli1.idr_pid` -1 (all ones) for a P slice
      and `poc_lsb` from the PocType-20 recurrence, +2 per picture, reset at an
      IDR, with `idr_pic_id` alternating across IDR pictures; `synt_nal`
      nal_unit_type 5 / 1 and nal_ref_idc 3 / 2, without which the hardware stamps
      every P picture as an IDR; `me_cfg.pmv_mdst_h/_v` 5 instead of 0; and the two
      tables `setup_vepu580_rdo_pred` swaps per slice type - the chroma KLUT
      window moving from `h264e_klut_weight[0]` to `[4]` with `chrm_klut_ofst`
      0 → 3, and `aq_step`, whose I and P tables **differ in a single entry**
      (index 14: six against seven). `tools/gen_vepu580_regs.py` had to learn to
      name register *arrays* to see that last one at all; `aq_tthd`, `aq_step` and
      `rc_dthd_0_8` are now addressable, and `RKMPP_REG_DUMP` writes a Rust job in
      the same text layout `tools/mpp_trace` captures from the C encoder, so a job
      can be diffed against a captured job instead of against a recording.

      Measured on t6 against a Baseline-pinned reference encoding the same two
      frames: **the IDR is still byte-identical, and the whole register image
      agrees except for dma-buf fds and one bit.** The P NAL is 63 bytes against
      the reference's 58. The remaining bit is `enc_pic.mei_stor`: the reference
      asks the encoder to store motion derivation info for P pictures and pays for
      an MD buffer to put it in, which this driver does not allocate. Setting the
      bit without that buffer is not an option and was tested - the encoder comes
      back with `int_sta = 0x100` rather than a picture.
      The P pictures this does produce are *valid*: FFmpeg decodes the IDR+P stream
      as two pictures at PSNR y 52.05 / u 52.78 / v 55.96 dB. `verified_on_board`
      keeps the hardware path at GOP 1 until the MD buffer closes that last bit;
      `build_registers` still generates longer GOPs and
      `p_picture_changes_only_the_fields_the_reference_does` pins every field the
      reference touches, so the difference cannot silently widen.
      **Correcting the attribution above, because the follow-up measurement
      contradicted it.** `mei_stor` is *not* what separated the P payload: the
      reference sets that bit for **every** picture - the captured IDR has it set
      too, and our IDR has been byte-identical to C this whole time with the bit
      clear - so the bit does not move the coded bits, and clearing it was not why
      the P picture differed. What the bit did establish is a hard requirement
      rather than a nicety: enable set and `meiw_addr` at zero gives
      `int_sta = 0x100`, so the MD buffer had to exist before the bit could be
      raised honestly. It exists now (`mdinfo_size`: 16 bytes per 64x16 block of
      the coded size), `meiw_addr` points at it and `mei_stor` matches the
      reference - and the IDR matrix came through unchanged, the same fact from the
      other side.

      With the job dump extended to all seven register sections, the P job's
      remaining distance to the captured reference P job is **six words, all in
      section 3**: `RDO_QUANT.quant_f_bias_I` (sw1484), `ATR_THD0` and `ATR_THD1`
      (sw1488, sw1489) and the `Lvl16/17/18_ATR_WGT` weights (sw1492..1494). CTL,
      BASE, RCKLUT, RDO, SCL and OSD match word for word. Those six are exactly the
      writes of the finetune module, `mpp/hal/rkenc/h264e/
      hal_h264e_vepu580_tune.c`, which re-patches the quant bias and the ATR tables
      from statistics gathered on the preceding frame
      (`..._tune_stat_update` feeding `..._tune_reg_patch`). Nor is it a harness
      option that happens to be on: running the reference with
      `-atr_i 0 -atr_p 0 -atf 0 -atl 0 -dbe 0 -dbs 0` produced a byte-identical
      stream to the default run, so those tables move on the module's own per-frame
      state, not on any flag.
      **Resolved: the six words were two layers, not the finetune module's state.**
      `setup_vepu580_rdo_pred` writes the quantisation bias and the ATR thresholds
      *per slice type* - `quant_f_bias_I` 683 against 583, `ATR_THD0` {1,4} against
      {4,16}, `atr_thd2` 36 against 81, and a different Lvl16/8/4 weight triple -
      and `vepu580_h264e_tune_reg_patch` then overwrites the inter ATR weights from
      `atr_wgt[scene_motion_flag]`. Both layers were missing, so every P picture
      inherited the I picture's thresholds; those five bytes were the whole gap.
      Now the P job's register image differs from the captured reference job in
      **zero non-address words across all seven sections**, and six board cells -
      still and moving fixtures at 2, 4 and 6 frames, i.e. 1 IDR plus 1/3/5 P
      pictures - are byte-identical NAL for NAL, with the 24-cell IDR matrix
      unchanged. The `scene_motion_flag` those tables are indexed by is pinned at
      its initial value.

      What that leaves out, stated rather than hidden: the scene detector itself,
      `vepu580_h264e_tune_stat_update`, which reads the block statistics
      (`madi`, `madp`, `md_sad_b16num*`, `st_bnum_*`) out of the STATUS window and,
      once three consecutive pictures flag large motion, moves the same tables to
      another row. Not ported; none of the fixtures trip it. The STATUS window's
      field semantics are the prerequisite, which is also what rate control needs.

      Two smaller notes from the same work. `tools/gen_vepu580_regs.py` now covers
      the section-3 and RDO windows too (688 fields), but deliberately does *not*
      reimplement the `cime_multi` / `rime_multi` writes: that corner of section 3
      moved between the 2024-04 build this crate is verified against and current
      master, and the image already carries words that match, so inventing a layout
      there would trade a real match for a cosmetic one. And `h264-enc` had been
      sizing a frame as *file length / frame count*, which both mis-sliced any file
      whose frame count differed from the request and silently re-encoded picture
      zero; the frame size now comes from the encoder geometry, with an explicit
      error when the input is short.

- [x] **The STATUS window is ported and verified field for field**, because it is
      the shared input of the two things still missing from the encoder: the scene
      motion detector and rate control. `Statistics::from_words` names
      `bs_lgth_l32`, `qp_sum`, `madp`, `num_ctu`, `madi`, `num_b16` and the
      `md_sad_b16num` / `madi_b16num` histograms, and every offset is checked
      twice over rather than trusted: `tests/data/vepu580_status_c_fixqp_2f.txt`
      holds the C encoder's own read-back on t6 as an offline fixture, and
      `scripts/board_h264_status_vs_mpp.sh` encodes the same two pictures with
      both encoders and requires the windows to agree. They do, for the IDR and
      the P job alike, word for word - with one exception that has to stay
      different: reg4107, `st_bsb.bsbw_addr`, the bitstream address the hardware
      wrote to, which cannot match between two processes and is therefore reported
      rather than asserted.
      Two corrections the measurements forced, both of which were asserted more
      confidently than the evidence supported. First, the previous entry's claim
      that "madp and num_ctu are zero on this block" is only half right: `num_ctu`
      is 75 on the P picture, alive and non-zero; `madp` is zero because the
      fixture encodes the same picture twice, so the mean absolute inter difference
      is zero by definition. Which inputs actually gate the scene detector is
      therefore a measured question now, not an assumed one. Second, the first
      version of the STATUS fixture was shifted by one word, because the ioctl
      tracer prints `words=<count>` before the values and the extractor parsed the
      count as data; the fixture's own header now records that, since the mistake
      was invisible everywhere else it could have been caught.
- [x] **The two IDR user-data SEI NAL units belong to the access unit, not to
      the harness.** Step 17 of `mpp/codec/mpp_enc_impl.cpp` fills them at the
      default `MPP_ENC_SEI_MODE_ONE_SEQ`: a `uuid_version` NAL carrying the
      library's build string and a `uuid_rc_cfg` NAL carrying a rendering of the
      rate-control configuration (`update_rc_cfg_log`, `mpp_enc_impl.cpp:1375`).
      The C harness therefore writes them into the very files the encoder matrix
      is compared against, and skipping NAL type 6 - what the first comparison
      script did - compared a stream MPP does not produce.
      `src/codec/h264/sei.rs` frames them as `h264e_sei_to_packet` does (four-byte
      start code, disposable NAL header, payload type and size in `0xff`
      continuation form, the 16-byte UUID, the payload, `rbsp_trailing_bits`,
      escaped on the way from RBSP to NAL) and renders the rc line field for
      field. Five unit tests, of which two compare a whole NAL byte-for-byte with
      one captured from the C encoder on t6 - at 320x240 and again at 64x64, plus
      the `0xff`-continuation and emulation-prevention rules.
      One thing is deliberately not imitated: a version SEI's payload is the
      build identity of whichever encoder produced it, and MPP writes its own
      `git log` line there. Ours says `rk-mpp-rs 0.1.0 VEPU580 H.264 qp N gop N`,
      so that NAL is compared for framing - NAL header, payload type, the size in
      continuation form, the UUID, the trailing byte, and that the size accounts
      for exactly uuid + text + trailing - and not for text. Every other NAL in
      the access unit, the rc-cfg SEI included, is byte-compared.
      Both bugs this found were in the check rather than the driver. The NAL
      header was composed as a bitwise or of the *values*, so `nal_ref_idc` 5
      against `nal_unit_type` 6 produced a 7: an IDR slice header to any decoder
      reading it. And the size self-consistency check ran over our stream while
      being labelled the C one, so it could never fail on the side it claimed to
      verify.
      The bit rate numbers inside the rc-cfg SEI turned out not to be free of
      rules either. `mpi_enc_test` derives an unset `-bps` as
      `width * height / 8 * fps` and pushes it in every rate-control mode,
      fixed QP included, so the reference line reads `b:15360` at 64x64 and
      `b:288000` at 320x240; hard-coding one size's value passed at 320x240 and
      failed at 64x64. `encoder::default_bps_target` reproduces the derivation,
      and the bounds stay at `h264e_api_v2.c`'s "2Mbps +-25 %" - 1 500 000 to
      2 500 000 - which is what MPP's SEI shows on a caller that never set them,
      because the fixed-QP branch of the harness sets no bounds at all.
- [x] **The first piece of rate control is in, checked against the library rather
      than against a reading of the source.** `codec::rc::data` is `MppDataV2`
      (`rc_base.h`, `rc_base.cpp`) - the window `rc_model_v2.c` keeps every bit and
      QP history in, where `preset` records a predicted picture and `update`
      replaces that prediction in the same slot rather than appending next to it.
      `tools/rc_data_probe.c` calls the real `mpp_data_*_v2` out of
      `librockchip_mpp.so.0` over one scripted sequence at the window lengths the
      model actually allocates (2, 5, 8) plus two above the eight entries where
      `rc_base.h` says the write mode changes, printing the internal positions
      after every call. `tests/data/rc_data_c_probe.txt` is that transcript and
      `the_window_matches_the_reference_library_line_for_line` replays it: all 310
      lines agree, including the three behaviours a tidier reimplementation would
      change without noticing - `sum()` and `mean()` narrow the 64-bit accumulator
      to `int` before dividing, `reset` computes `val * size` in 32 bits and widens
      afterwards, and `sum_with_ratio` walks the array from index zero with geometric
      weights that overflow on a long window. The `swr=96343577,...` row at 64 slots
      is that overflow; it has to stay.
- [x] **The rate-control target is recorded, and it is reproducible.**
      `scripts/board_h264_rc_capture.sh` encodes the same six moving frames at
      fixed QP and at CBR 300 kbps with the ioctl tracer, and
      `tools/rc_trace_report.py` reduces the trace to the question: the named fields
      that differ between the two first jobs, then per job the fields that move plus
      what the hardware measured. `tests/data/vepu580_rc_c_move6_cbr.txt` is that
      report. Six frames at CBR produce six jobs - no re-encode - and the resulting
      stream is 6283 bytes with md5 `c34eba92a3196865d04ac23253b2f7af`, repeating
      byte for byte across runs, so it can serve as the byte oracle the way the
      fixed-QP streams do.
      **The register half of that is done and matched word for word.**
      `RegisterFile::apply_rate_control` generates the non-fixed-QP branch of
      `setup_vepu580_rc_base` - the scaled per-macroblock target and the two
      thresholds that come off it, the ladder, the ROI rows, `qpmap_mode`, the
      motion and `madi` triples - and against the captured words of both jobs
      (`the_c_encoder_s_cbr_intra_job_is_reproduced`,
      `the_c_encoder_s_cbr_p_job_is_reproduced`) our image equals the C encoder's
      CBR job in every BASE, RCKLUT and RDO word except the dma-buf addresses.
      Two things that check had to be fixed before it could mean anything. The
      fixture initially held a stray line of prose, because `--dump-job` printed
      its job count into what was meant to be machine-readable output; the strict
      hex parse caught it rather than skipping over it. And the P job only compares
      once the picture is built as the P picture the capture has - the crate's
      default intra GOP of 1 made every picture an IDR, which showed up as 30
      differences in `me_cfg`, `synt_nal`, `synt_sli0/1` and the slice-type KLUT
      tables, all of which `apply_picture` owns and rate control does not touch.
      It is not wired into `build_registers` yet, deliberately: the branch needs a
      bit target and a QP from `rc_model_v2.c`, and a half-wired mode that encodes
      at a guessed target would be worse than the fixed-QP path that is verified.
      What CBR adds to a job is 30 named fields, every one in BASE or RCKLUT, and
      all of them are what `setup_vepu580_rc_base` writes: `rc_en`, `aq_en`,
      `rc_ctu_num` = `mb_w`, `rc_qp_range`, `rc_min_qp` 10 and `rc_max_qp` 51, the
      `qp_adj` ladder (-2, -1, 0, 1, 2 and a lone 1, which is why the 5-bit fields
      read 30 and 31), the `qpmin_area` / `qpmax_area` rows over the same bounds,
      and `qpmap_mode` 1. Two of the thirty are the model's answer rather than
      configuration: `enc_pic.pic_qp` and `rc_tgt.ctu_ebit`. Across the six frames
      `ctu_ebit` walks 4637, 476, 484, 465, 449, 425 while `pic_qp` moves only once,
      23 to 22 - so `rc_model_v2.c` reduces, per picture, to a bit target and a QP.
      That also calibrates the statistics: at 320x240 the I job reports
      `ctu_ebit` 4637, which through `mb_target_bits_mul_16 = (bit_target << 4) /
      (mb_w * mb_h)` says the picture was aimed at roughly 87 kbit where the plain
      frame average is 10 kbit - the intra picture is being handed something like
      nine frames' worth of bits, which is what `calc_next_i_ratio` is for.
      One measurement correction that came out of the same table: `qp_sum` counts one
      QP per 16x16 block, four of which sit inside each block `num_ctu` counts.
      Fixed QP 26 reads back 7800 against `num_ctu` 75, and dividing by the second
      number instead of the first gives 104, which is not a QP. The divisor is now
      derived from that reading rather than assumed, and the report prints 26.00 for
      the fixed-QP run, which is the check that it is right.
- [x] **The bit-allocation half of the model is ported, and the first picture's
      target lands on the captured register word.** `codec::rc::model` is
      `bits_model_init` / `bits_frm_init` / `bits_model_preset` /
      `bits_model_update` / `bits_model_alloc` / `calc_next_i_ratio` with their
      windows, water level and the `max_i_delta_qp` cap. It is separable because
      `bit_target` comes out of `bits_model_alloc` for every mode, while
      `calc_cbr_ratio` / `calc_vbr_ratio` / `calc_avbr_ratio` only move
      `next_ratio`, which the quantiser path consumes - so this layer can be
      finished and checked before the controller exists. The check that matters is
      `the_first_intra_target_matches_the_captured_job`: 300 kbps, GOP 60 and
      `h264e_api_v2.c`'s own defaults give `600000 * 160 / (160 + 16 * 59) = 86956`
      bits for the intra picture, which scaled into sixteenths over 300 macroblocks
      is `ctu_ebit` 4637 - and the test reads 4637 out of
      `tests/data/vepu580_rc_c_move6_cbr_regs.txt` rather than being told it, so the
      allocation and the capture cannot drift apart.
      Fed the reference's own coded sizes (`bs_lgth_l32` from the STATUS window,
      times eight - `hal_h264e_vepu580.c:2389` and `:2440` settle that
      `bit_real = hw_length * 8` and `hw_length` is exactly that word, accumulated
      per segment), the model reproduces four of the six captured `ctu_ebit` values
      exactly and the whole trajectory from the third picture on: 4637, then the
      decay 465, 449, 425, which happens because every P picture codes under its
      share and `i_scale` rises as `p_sumbits` falls. Pictures 1 and 2 are
      **swapped** - the reference promised 476 then 484 where we promise 484 then
      476 - and that shape is what makes it worth chasing: a mis-scaled or
      mis-unit-ed feedback would move every later picture, so whatever it is
      touches the first P pair and nothing after it.
      `the_trajectory_matches_the_capture_except_for_one_swapped_pair` pins our
      six numbers so the discrepancy cannot be papered over.

      **It is now resolved, and it was never the port's.** The instrument turned out
      to be sitting in plain sight: `rc_model_v2.c` prints `i_sumbits`, `p_sumbits`
      and `i_scale` at every update and allocation under `rc_debug`, and
      `mpp_env_get_u32` passes its name straight to `getenv` - the variable is
      literally `rc_debug=0x40`, not `MPP_RC_DEBUG`, and the log goes to syslog
      unless `mpp_syslog_perror=1` asks for stderr too. That last detail is also
      the standing answer to this repo's old puzzlement that `mpi_enc_test` "prints
      nothing at all": the library does talk, it talks to syslog. `tools/rc_probe.c`
      drives the encoder through the API for the same reason - to have a caller that
      keeps its own stdout open while the library's state scrolls past it.

      Replaying the two runs' own coded sizes, this port reproduces the reference's
      `i_scale` at every picture of both: 160, 107, 128, 156, 200, 270 for the
      direct-API run and 160, 112, 130, 157, 196, 260 for the `mpi_enc_test` one
      (`tests/data/rc_model_v2_c_state.txt`, `the_model_matches_the_reference_model_s_own_state`).
      Run B is the capture whose traced registers read 476 then 484 for the first
      two P pictures - but the reference's own model says 112 then 130 for those
      pictures, i.e. 484 then 476, which is what this port computes. The
      transposition is in how traced register windows line up with jobs, not in the
      allocation. The direct-API run, where the pairing is unambiguous, matches our
      six `ctu_ebit` values exactly: 4637, 487, 477, 465, 447, 421.
      The same read settled the quantiser controller's inputs, which were the next
      unknown: `quality_real = qp_sum / mbs` (one QP per 16x16 block over the coded
      size, which is why fixed QP 26 at 320x240 reads back 7800), `madi` divided by
      `num_b16`, and `madp` divided by `num_ctu` - with a zero denominator taking
      the whole term to zero rather than being skipped.
- [x] **The CBR quantiser is ported and pinned to the reference's own printout.**
      `calc_cbr_ratio`'s four terms - overshoot against the previous target, the
      `tab_lnx` instantaneous-rate term, the rate-error term and the buffer
      water-level term - plus the `cur_scale_qp` bookkeeping in QP-sixteenths. The
      oracle is `tests/data/rc_model_v2_c_cbr_qp.txt`: the library printing its own
      state under `rc_debug=0xf0`, which the repository could not reach for days
      because `mpp_env_get_u32` reads the bare lowercase name and MPP logs to
      syslog. `the_cbr_quantiser_matches_the_reference_printout` replays that run
      and matches all six pictures on six numbers each - bit target, quality target,
      `next_ratio`, `ins_bps`, `last_inst_bps` and `stat_watl` - not just the QP
      that comes out.
      Two details are the easy-to-get-wrong kind and are worth naming: `ins_ratio`
      is *zeroed* unless the instantaneous rate is closing on the target from either
      side (so the logarithm table usually contributes nothing, and a port that
      always uses it will disagree on quiet scenes), and `i_quality_delta` enters
      once, as a two-QP offset on the sixteenths scale at the first intra picture,
      rather than as a per-picture QP reduction. VBR and AVBR use different term
      sets and are not ported; `cal_first_i_start_qp` is reached only when a caller
      leaves the initial quality unset, which neither `mpi_enc_test` nor a default
      `h264e_api_v2.c` configuration does.
- [x] **CBR is live and byte-identical to the reference on a six-picture run.**
      `EncoderConfig::rc_mode` selects it (`-bps` on the CLI is a request for rate
      control, as it is for `mpi_enc_test`), `Vepu580H264` owns an
      `rc::model::Model`, each picture gets a bit target and a QP from `start`, the
      job carries them into `apply_rate_control`, and the STATUS bitstream length
      feeds `end`. `scripts/board_h264_cbr_vs_mpp.sh` encodes the same six frames
      with our driver and with the reference library and compares NAL by NAL: ten
      NAL units identical, the version SEI framing-only as always. The two rc-cfg
      SEIs match byte for byte, which means the SEI's own rendering of the mode,
      bounds and I delta agreed before the payload did.
      The reference side runs through `tools/rc_probe.c` rather than
      `mpi_enc_test`, and that is a deliberate choice rather than convenience: both
      sides then set exactly CBR, the bit rate and GOP 60 at 30 fps, leaving the
      quality band at `h264e_api_v2.c`'s defaults, where `mpi_enc_test` configures
      more of its own and lands its first picture at QP 23 against the library's
      26. That harness difference is recorded as an open question, not papered
      over: the same six-frame stream from `mpi_enc_test` is a different stream,
      and matching it needs its exact configuration enumerated.
- [x] **VBR, verified the same way.** `calc_vbr_ratio` is three terms rather than
      four - no buffer water level, because VBR is allowed to spend what it saved -
      with the bit-error weight at 32 instead of 52, the logarithm table indexed by
      `bps_max`, and the instantaneous term counted only while the rate is above the
      target or falling with the target still ahead. Two rules decide the whole
      mode and both are in `vbr_budgets_from_the_max_and_steers_to_the_target`: the
      GOP budget comes from `bps_max` (60 frames of 350 kbps is 700,000 bits, so the
      first picture is aimed at 101,449 where CBR's target budget gives 86,956) while
      the controller steers towards `bps_target`. `board_h264_cbr_vs_mpp.sh` now runs
      both modes and passes both: ten NAL units identical each, six frames.
- [x] **AVBR too, which is where the motion statistics start feeding back.**
      `moving_ratio_calc` weighs the last two intra pictures' sizes by how far their
      mean QP sat from the current one (`mean_qp2scale`), compares that against the
      inter pictures' cost, and blends in `madi`/`madp` and `motion_sensitivity` into
      a 0..255 level that scales the GOP budget *before* any allocation. Five integer
      divisions and a shift, all rounding, all pinned by
      `avbr_scales_the_gop_by_a_still_moving_level` against the reference's own
      printout (`tests/data/rc_model_v2_c_avbr_state.txt`): `min_still_percent` 71,
      `percent_a` 10496, `percent_b` 29, `percent` 255, `mv_ratio` 181,
      `mad_ratio` 258, `moving_ratio` 222, `gop_bits` 607031 and a first P target of
      9276 at QP 27. Reaching it needed the STATUS feedback the other modes ignore -
      `quality_real`, `madi` and `madp`, each with the divisor the HAL applies.
      `board_h264_cbr_vs_mpp.sh` now runs all three modes and passes all three:
      ten NAL units identical each, including the rc-cfg SEI's own mode name.
- [x] Rate control beyond fixed QP - done, three modes, three kinds of evidence.
      What follows is the record of how it was measured, kept because the
      corrections in it are the useful part. `setup_vepu580_rc_base`'s VBR/CBR path -
      `rc_en`, `aq_en`, `rc_ctu_num`, `rc_qp_range`, the `rc_adj` ladder, the
      `rc_dthd` thresholds and the ROI area thresholds - is exactly the set the
      fixed-QP path leaves at zero. Those registers are generated now (above); what
      what remained after that was the loop behind them - the feedback input, since
      identified as `bs_lgth_l32 * 8`, and the quantiser controller that reads it.
      Measured so far, so the size of it is known rather than guessed. The SEI's
      own `default:` prefix names the implementation: `h264e_rc.c` registers
      `default_h264e` as a `RcImplApi` over `rc_model_v2.c`, so the loop to port is
      those 1733 lines plus the `MppDataV2` sliding-window helper
      (`rc_base.h`, `rc_data*.cpp`) and the `rc.cpp` / `rc_impl.cpp` dispatch that
      calls `start` / `end` / `hal_start` / `hal_end` around each job - about 2.9k
      lines of the 2024-04 vintage, which is not the file in `refs/mpp`: the two
      copies of `rc_model_v2.c` differ by 310 lines, so master is a guide and the
      board's `/tmp/mpp-old` is the oracle. Two rules from the harness fall inside
      this crate's boundary too: the unset-`-bps` derivation described above, and
      the per-mode bounds (`bps * 17 / 16` with `bps * 15 / 16` on CBR, the same
      top with `bps / 16` on VBR and AVBR, nothing on fixed QP).
      Two facts about the reference were measured first, because both change what
      has to be built. `-rc 1` is CBR and `-rc 2` is the fixed QP the matrix uses.
      And a claim made one commit ago here - that six CBR frames cost twelve encoder
      jobs, so every picture went through `check_re_enc` and the submit path has to
      re-run pictures - is **wrong and withdrawn**: the ioctl tracer prints every
      register window twice, once under its `before cmd=200` line and once on its
      own, and counting `write words=` lines doubles everything. Counting the
      `before` lines at offset 0 gives six jobs for six frames, at `-rc 1` with and
      without `-qc` bounds, so re-encode was never observed here. Whether
      `check_re_enc` fires for a stream that overshoots is still unmeasured; what is
      settled is that it did not fire in the runs used as the rate-control oracle.
       `check_re_enc` can still raise the QP and ask for the *same* picture again,
       which the single-shot submit path has no way to express; it did not fire in
       these runs, but nothing here says it will not fire on a stream that
       overshoots, so the re-encode loop stays on the list.
 - [x] **The SPS level is a generator input now, verified against the reference
       across the level range.** `mv_limit`, the CIME vertical search window,
       `rect_size` and the SPS's own `level_idc` are all derived from the level the
       picture needs - `h264e_sps_update` raises a request below the coded size's
       minimum before `setup_vepu580_me` and the SPS writer read it, and the HAL
       follows the raised value, which is what a request of 1.0 at 320x240 (raised
       to 1.1, mv_limit 1) or at 1280x720 (raised to 3.1, mv_limit 2, `rect_size`
       dropped) exercises. The request is bounded to 9..=52, the range the SPS can
       carry; 1-b and 5.2 included. `scripts/board_h264_level_vs_mpp.sh` compares
       both sides NAL by NAL: 1.0 through 5.2 at 320x240 over an IDR plus a P
       picture (nine cells, byte-identical including the two SEI NAL units), and
       four cells at 1280x720 over the IDR, where the raise does its work. The C
       side is `mpi_enc_test` with the level patched into its source the way the
       profile is patched elsewhere, fixed QP 26. The raised level, not the request,
       is what `effective_level_idc` exports for the HAL.
 - [x] **The P divergence beyond 640x480 was a wrong FBC body offset - found,
       fixed, and it was never about content.** `REG_RFPW_B` and `REG_RFPR_B`
      （the two offsets the hardware adds to reach the FBC *body* of a
       reconstruction surface) were a constant 8192. The surface does not start
       its body at a constant: `pixel_buf_fbc_hdr_size` is
       `align(aligned_w * aligned_h / 64, 8K)`, which stays 8192 while the coded
       area is under 524288 pixels and becomes 16384 past it - so at 1280x720 the
       driver told the hardware to read the reference from 8192 in a surface whose
       body begins at 16384, and at 1920x1088 the two numbers are 8192 and 40960,
       a displacement of 32 KB on a surface nobody had ever byte-compared (the
       harness cannot, at a height that is not a multiple of 16). An IDR reads no reference, which is why the 24-cell
       matrix - every cell a single IDR picture - stayed green at 1280x720 for as
       long as it has existed, and why the surplus looked content-independent:
       it was a constant, and it was a *displacement*.

       One function, `fbc_header_size`, now supplies both the allocation and the
       two offsets, so the two cannot disagree by construction; the unit test walks
       the threshold (8192 at 320x240, 640x480 and 800x600; 16384 at 1280x720) and
       asserts the offsets in a generated 1280x720 job.

       Measured on the board, `scripts/board_h264_p_size_probe.sh` - widened to
       eleven sizes so the FBC step has a cell on each side: 320x240, 384x288,
       448x352, 512x384, 576x416, 640x480, 640x256, 320x512, 800x600, 1024x768,
       1280x720 - all eleven byte-identical, and 1024x768, which sits past the
       step and should have failed before the fix, does agree now. The original
       failing case - six frames of testsrc at 1280x720 - is byte-identical
       picture by picture (1371, 1075, 1341, 1097, 1336 on both sides), and with
       four identical frames, and with a per-frame luma step, every delta is zero.
       The standing gates re-ran after the change: six GOP cells, the
       cbr / vbr / avbr cells, four GDR cells and the 24-cell matrix.

       The lesson is in how it hid: byte-identical IDR pictures are no evidence
       about anything on the reference path. A matrix that varies size but only
       ever encodes an IDR cannot see a reference-surface bug at all, and this one
       has been green across four sizes including 1280x720. `board_h264_qp_vs_mpp.sh`
       still keeps its P cells at 320x240; the size sweep is where P pictures are
       now proved, and it should grow into the matrix rather than sit beside it.
- [ ] rkvenc supports one live session per process here. The two ignored encoder
      tests pass serially and disagree with the goldens when cargo runs them as two
      threads, which is `setup_ext_line_bufs` programming a shared system-controller
      line-buffer configuration for both sessions. Relevant to M10's multi-instance
      work.
- [x] **JPEG encoder (vepu): restart interval, 90/270-degree rotation and RGB
      input.** The restart interval follows C's split-by-CTU derivation
      (`part_rows = ceil(arg / mcu columns)`, `DRI = mcu columns * part_rows`,
      register 107 carrying the rows over the RST0 marker the HAL seeds), with
      the DRI written between SOF and DHT exactly where `write_jpeg_header`
      puts it, and the silently-truncated 16-bit DRI field refused explicitly
      instead. Rotation maps `ROT_90`/`ROT_270` to the register-74 bits the way
      `hal_jpege_vepu2_gen_regs` does: the SOF transposes and the geometry
      registers stay on the unrotated input, which a test locks word for word;
      180 and mirroring exist in MPP only as warnings and are refused here. RGB
      input covers exactly the `vepu_rgb_cfg` rows that map to a real `VepuFmt`
      - 565/555/444, ARGB/ABGR/BGRA/RGBA8888, RGB/BGR101010 - the 24-bit packed
      pair excluded because C itself cannot read them, with the BT.601
      coefficients, component masks and byte-swap bits copied from the table.
      The byte verdict came from `scripts/board_jpege_restart_vs_mpp.sh` (twelve
      cells, `mpi_enc_test -t 8` under `split_mode=2` plus a marker audit in
      `scripts/jpege_markers.py`), `board_jpege_rotate_vs_mpp.sh` (eight cells,
      transposed SOF verified) and `board_jpege_rgb_vs_mpp.sh` (twelve cells
      through the colour path) - all byte-identical, all at sizes <= 480p
      because the reference's own default for client-19 work above 720p is a
      four-band partition the single-job path cannot byte-match by design.
 - [x] **JPEG low-delay packet output is byte-identical to C.** Rust keeps the
       already-ported single hardware job and exposes packet byte ranges ending
       after each RST marker, matching C's sequential band loop without paying
       for one submission per band. `tools/jpege_probe.c` supplies the C oracle
       because the pinned `mpi_enc_test` cannot set `base:low_delay`.
       `scripts/board_jpege_lowdelay_vs_mpp.sh` covers 320x240/5 packets and
       640x480/10 packets: **2/2 byte-identical with identical packet lengths**.
       The ten-packet cell also crosses MPP's eight-slot output-task pool.
       Rotated low-delay output and pictures above the 1280x720 area gate are
       refused because C routes those through different paths.
 - [x] **JPEG default multi-core partitioning is byte-identical to C.**
       The tracer settled the streaming batch protocol: separate `MULTI_MSG`
       ioctls, `SET_SESSION_FD` delimiters and one final `LAST_MSG` request.
       Rust submits up to four band register sets, writes later bands to scratch
       DMA buffers and concatenates their reg53 lengths. The default
       `1920x1080` qfactor-50/90 cells and uneven `1288x722` split are **3/3**
       byte-identical; stride cells `322x242` and `1366x768` are **2/2**.
       `ver_stride` now follows C's `align(height, 2)` while the DMA allocation
       retains C's 64-row tail. Rotated multi-core remains explicitly refused.

### Post-processing
- [x] RGB888 letterbox scaler (own, faster than RGA for our sizes)
- [x] **IEP2 deinterlace is ported and submits - host field evidence.**
      `src/hal/iep.rs` derives the IEP2 register set for an I5O2 field-to-frame job
      from `mpp/vproc/iep/iep2/`, and `src/service/iep_session.rs` owns the client
      28 session (announce, submit, poll). Eighteen host tests cover the register
      layout, the geometry and the address-offset list. Two `#[ignore]`d board
      assertions exist - that client 28 binds and answers `POLL_HW_FINISH`, and that
      one I5O2 job returns two frames of the input size
      (`tests/iep_deinterlace_hw.rs`) - and `scripts/board_iep_dei_vs_mpp.sh` is the
      byte comparison against the C `iep2_test`, which ships at
      `build/mpp/vproc/iep2/test/iep2_test` of the reference tree.

      The board has now run it, and it says the hardware question is answered but
      the pixel question is not. One I5O2 job at 720x480 TFF: both sides write
      1,036,800 bytes - two progressive frames of the input size, so the session,
      the register write/read pair and `POLL_HW_FINISH` all work - and
      **1,035,360 of them differ**. The shape of the difference is the finding: in
      the C's output, every row of a given frame follows one `y * 3 + n * 7`
      sequence (rows 7, 10, 13, 31, 55 - one source picture), and the first input
      frame contributes no rows at all; ours interleaves rows from the previous and
      current fields. So C's I5O2 chose same-frame pairing on this fixture - the
      motion-history decision `iep2_test` runs with its default thresholds - and
      this driver pairs fields unconditionally.
      **That reading was wrong, and the real cause is now fixed.** The 164-word
      `iep2_params` image was word-for-word identical to the C's (only the buffer
      fds differed), and the hardware's `iep2_output` came back all zeros: the job
      never ran. Our batch marked its last request
      `MPP_FLAGS_REG_OFFSET_ALONE` (`0x12`), where the C's last request is
      `MULTI_MSG | LAST_MSG` (`0x03`); with `submit_batch_plain` the IEP job runs
      and `scripts/board_iep_dei_vs_mpp.sh` reports the full **1,036,800 bytes
      identical** at 720x480 I5O2 TFF. The pairing was never ours to change - the
      engine produced the same output as soon as it was actually started.
- [ ] IEP (`mpp/vproc/iep/iep.c`, `iep2/iep2.c`, client 28): YUV scale and crop,
      other deinterlace modes and field orders, and the scale/crop register paths.
      The I5O2 deinterlace path is byte-exact against the C `iep2_test`. This is the
      M9 engine that exists on this board.
- [ ] VDPP (`mpp/vproc/vdpp/vdpp2.c`, client 29): scale, rotate, NV12 to RGB,
      sharpen, histogram. **Absent on t6** - the kernel refuses the client - so
      this cannot be ported or verified here at all.
- [ ] 10-bit ↔ 8-bit conversion, HDR tone mapping

"M9 vdpu-pp" was written against the wrong engine. Three separate things are
easily conflated in the MPP tree: the legacy per-decoder post-processor window
(registers inside the decoder's own file), which in this source only
`hal_jpegd_*.c` programs - `hal_m2vd_vdpu2.c`, `hal_m4vd_vdpu2.c`,
`hal_h263d_vdpu2.c` and `hal_vp8d_vdpu2.c` never touch it, so there is nothing to
port for those decoders and our JPEG path already has it with RGB888 and the
1/2 to 1/8 downscale; the standalone VDPP, which is the general scale / rotate /
CSC engine and is not present on this SoC; and IEP, which is present and binds.
Client 3 answers `INIT_CLIENT_TYPE` happily and no decoder HAL in this tree ever
uses it. Until something drives it, the RGB888 scaler in `src/scale.rs` remains
the RGA replacement, at 2.9 to 3.8 times RGA's speed on the sizes the inference
pipeline uses.

### Framework
- [x] synchronous submit + poll
- [x] **Asynchronous task queue with completion callbacks**
      (`src/service/task_queue.rs`), beside the synchronous path with it
      untouched. A pure `Scheduler` state machine (`admit` -> queued -> running
      -> finished -> taken, free order of completion, monotonic ids against ABA,
      capacity pinned by finished-but-untaken jobs exactly as a C output port
      nobody dequeues) plus the threaded `TaskQueue`: FIFO dispatch, callback +
      `wait` + best-effort bounded event channel delivery, cancellation of
      queued jobs and a flag-only cancel of running ones (a poll timeout never
      stops the hardware in C either), `PollTimeout` confined to the
      `MppPollType` domain (-1 / 0 / 1..8000). Multi-session is prepared for:
      each job carries its own session fd (`new(n)` = n concurrent sessions),
      and a `SessionGate` serialises rkvenc to one live session until the
      shared line-buffer question is solved - the gate keeps concurrency 1,
      which a unit test asserts. `scripts/board_framework_async_vs_sync.sh`
      decodes a photograph six times through the queue and against the sync
      path: identical md5s, six callbacks, pool reuse under cap, sync path
      unchanged (all PASS), plus an ignored hardware test locking the same
      shape.
 - [ ] dma-fence in/out, buffer import/export - surveyed, not implementable on
       this vintage: `refs/mpp` has no fence surface at all (the command table
       ends at 0x404, `MppReqV1`/kmpp/vcodec_service carry no fence fields, the
       only grep hit is a typo for "reference"). The intended shape is written
       up in `docs/framework-m10.md`: in-fence as a poll(fd) prelude inside the
       job closure, out-fence handed to the completion path, pending a kernel
       protocol that exists.
 - [ ] multi-core (CCU) distribution - a service-level `SET_SESSION_FD` question,
       parked behind the queue work that is now in.
 - [ ] secure/TA mode - only `MPP_FLAGS_SECURE_MODE` is surfaced; no TA on this
       board to verify against (decision D3).
 - [x] **Reference-counted frame buffer pool** (`src/service/buffer_pool.rs`)
       over the real dma-heap backend. `acquire` / `release` / `try_clone` model
       `mpp_buffer_get` / `unref` / `ref` with an `Empty / Cached / Live{refs}`
       state machine; the deliberate deviations from C are enforced instead of
       asserted - double release and use-after-release are errors, a partially
       released block is never handed out, over-capacity acquires fail the way
       `limit_config` returns NULL (with Block/Ms wait variants on top), best-fit
       reuse replaces exact-size buckets. Two ignored hardware tests run against
       `/dev/dma_heap/system`: reuse returns the same dma-buf across
       release/re-acquire and refuses to recycle an in-flight one; a concurrent
       32-job stream drains through the pool within its cap (`ok` on t6).

## Verification

- Tests: `cargo test --all-targets` (512 passed across all targets, twenty ignored
  board-only tests: parsers, AV1 OBU/frame-header/CDF/PicParams snapshots,
  global-motion reconstruction, tile ranges and packet assembly, POC types
  0/1/2, DPB/MMCO/RPS, registers,
  tables, VP9 probability snapshots, packed 10-bit conversion, scaler and
  NEON). The CLI test also verifies that damaged H.264
  and H.265 input returns a non-zero exit status, including the no-access-unit
  path observed on the board. Four ignored hardware tests repeat a cached VP9
  picture through `show_existing_frame`, exercise the 1280x720
  error-resilient segment-map reset, cover Profile 0/2 lossless plus a
  multi-tile error-resilient and 321x241 odd-size streams, and check AV1
  8/10-bit film-grain output against libaom; all pass on t6 when explicitly
  run.
- Board H.264 regression: 7/7 streams bit-exact (`scripts/board_regress.sh`),
  including progressive, multi-slice, MBAFF, PAFF, long-term references and a
  stream with a missing access unit.
- Board H.265 regression: 14/14 streams bit-exact (`scripts/board_regress_hevc.sh`),
  covering P, B, 1080p, Main10, default plus explicit SPS/PPS HEVC scaling
  lists, MPP auto-tiles,
  CRA/open-GOP, four slices per picture, MPP mark/use long-term references and
  dependent slice segments, plus a stream with a missing reference picture.
  The JCT-VC dependent-slice stream emits 50/50 clean pictures and matches
  MPP byte-for-byte. The Main10 stream emits 30/30
  frames with `err=0` and
  matches MPP's converted planar output. The LTR stream also emits 30/30
  frames with `err=0` and matches MPP byte-for-byte.
  All three scaling-list streams emit 10/10 frames with `err=0` and match
  MPP; the default output is 31,104,000 bytes and both explicit-matrix
  outputs are 983,040 bytes.
  The tiled stream is 1280x720 with two columns, emits 30/30 frames and matches
  MPP byte-for-byte. The
  missing-reference stream emits the same 2 clean pictures as MPP (POC 0 and
  4) and suppresses the other 7 dependent pictures (POC 1, 3, 5, 6, 7, 8 and
  9). The ordinary 720p stream outputs POC `0..9`, 13,824,000 bytes total, and
  matches MPP byte-for-byte. A 120-frame multi-IDR stream emits four complete
  `0..29` epochs and also matches MPP byte-for-byte.
- Board MPEG-2 regression: 3/3 fixtures as expected
  (`scripts/board_regress_m2v.sh`). `m2v_320x240_12.m2v` (IPBPB, 15 slices per
  picture) and `m2v_64x64_3.m2v` (a sequence header in front of every picture)
  are byte-exact against `mpi_dec_test -t 2` converted from NV12 to planar, and
  `m2v_320x240_ilaced.m2v` is within 2 units per sample of FFmpeg. That last
  fixture is the one case where C MPP is not the oracle: on interlaced MPEG-2 it
  disagrees with FFmpeg on whole pictures, and it rewinds its own input to emit
  40 frames for a 6-picture stream, so the check caps the C run at the real
  picture count and compares against FFmpeg with hardware IDCT rounding allowed.
- Board MPEG-4 regression: 3/3 fixtures byte-exact
  (`scripts/board_regress_mp4.sh`). The IP stream, the IPBB stream and the odd
  150x100 stream are all locked against `mpi_dec_test -t 4` converted from NV12
  to planar. The IPBB row is the one that earns its place: it is the only fixture
  here that exercises both reference heads, the B picture time ratios and the
  output order the C rotation produces.
- Board H.263 regression: 3/3 fixtures byte-exact
  (`scripts/board_regress_h263.sh`). QCIF, CIF and 4CIF are locked against
  `mpi_dec_test -t 3` converted from NV12 to planar. Those three are the whole
  size matrix rather than a sample of it: the short video header names no other
  resolution. The C decoder runs a picture or two again after the `-n` cap
  without writing them out, so the compare checks the output length before it
  compares bytes.
- Board VP8 regression: 6/6 fixtures byte-exact
  (`scripts/board_regress_vp8.sh`), and twelve of twelve register images identical
  outside the dma-buf handles. See the checklist entry above for what each fixture
  covers and which three register paths still have no board evidence.
- Decode throughput versus the source-built C MPP, from
  `scripts/board_bench_vs_mpp.sh`: five interleaved samples per binary on the
  same stream, medians. Rust covers submit + poll + DPB + NV12-to-planar
  conversion; C MPP covers its decode loop and writes native NV12, so the
  conversion work stays charged to the Rust number.

  | stream | pictures | C MPP | this crate | delta |
  | --- | ---: | ---: | ---: | ---: |
  | `test720.264` (H.264 720p) | 30 | 177 fps | 248 fps | +40 % |
  | `test_hard.264` (H.264 1080p) | 90 | 111 fps | 220 fps | +99 % |
  | `t720.hevc` (H.265 720p) | 10 | 109 fps | 233 fps | +115 % |
  | `hevc_multigop_120.hevc` (H.265 720p) | 120 | 243 fps | 552 fps | +127 % |
  | `hevc_main10_256.hevc` (H.265 Main10) | 30 | 157 fps | 1316 fps | +738 % |
  | `vp90_720.ivf` (VP9 720p) | 30 | 319 fps | 645 fps | +102 % |
  | `p0_320x240_30.ivf` (VP9 Profile 0) | 30 | 500 fps | 3409 fps | +582 % |
  | `av1_320x240_10.ivf` (AV1 Main 8-bit) | 10 | 526 fps | 800 fps | +52 % |
  | `m2v_320x240_12.m2v` (MPEG-2 320x240) | 10 | 476 fps | 1250 fps | +163 % |
  | `m2v_320x240_ilaced.m2v` (MPEG-2 interlaced) | 6 | 400 fps | 1154 fps | +285 % |
  | `m2v_64x64_3.m2v` (MPEG-2 64x64) | 3 | 750 fps | 3000 fps | +300 % |

  **Where the 720p time goes (performance audit, 2026-11-02).** Before optimizing
  code, profile the path: `perf stat` on `h264dec` over `test720.264` with the
  benchmark's `/dev/null` sink shows 92.9 ms wall for 30 pictures, of which 18.6 ms
  is user time, 29.8 ms sys time and ~45 ms is the blocking `POLL_HW_FINISH` wait.
  `strace -c` counts only 359 ioctls (about 12 per picture) plus the DMA syncs; the
  single-threaded CPU work is therefore under a fifth of the wall clock and the
  decoder is already at the hardware-plus-syscall floor. Micro-optimising the
  planar conversion or the parsers can move at most that 20 %, so any larger gain
  would need fewer ioctls or overlapping jobs - both framework changes, which the
  current scope freezes. This is recorded so future performance work starts from
  the split rather than from a guess.

  **Timing noise on this board is larger than most of the deltas above.** Ten
  back-to-back runs of *the same binary* on the same stream with the same
  `/dev/null` sink produced 72.9, 83.9, 84.0, 110.7, 117.6, 117.9, 118.1, 118.2,
  118.2 and 120.1 ms - a 1.65x spread. The published H.264 720p row (177 vs 248
  fps) is inside that band, so it should be read as "not slower than C", not as a
  measured 40 % win. Any future performance claim needs interleaved sampling with
  more than five samples (or a pinned frequency), and a change smaller than the
  spread is not evidence. Removing the CLI's three per-frame debug prints
  (`RKMPP_TRACE` now gates them) is a cleanliness change, not a claimed speedup.
  | `mp4_320x240_12_ip.m4v` (MPEG-4 320x240) | 12 | 522 fps | 1333 fps | +156 % |
  | `mp4_320x240_12_ibpb.m4v` (MPEG-4 IPBB) | 12 | 571 fps | 1212 fps | +112 % |
  | `mp4_150x100_9_ip.m4v` (MPEG-4 150x100) | 9 | 692 fps | 3214 fps | +364 % |
  | `h263_cif_12_ip.263` (H.263 CIF) | 12 | 400 fps | 1081 fps | +170 % |
  | `h263_qcif_6_ip.263` (H.263 QCIF) | 6 | 857 fps | 2222 fps | +159 % |
  | `h263_4cif_10_ip.263` (H.263 4CIF) | 10 | 119 fps | 373 fps | +213 % |
  | `vp8_320x240_12_ip.ivf` (VP8 320x240) | 12 | 600 fps | 1304 fps | +117 % |
  | `vp8_320x240_12_parts.ivf` (VP8 segmented) | 12 | 571 fps | 984 fps | +72 % |
  | `vp8_150x100_9_ip.ivf` (VP8 150x100) | 9 | 1000 fps | 3000 fps | +200 % |
  | `vp8_320x240_24_altref.ivf` (VP8 alt-ref) | 24 | 649 fps | 1004 fps | +55 % |

  Every decoder we ship beats the C library on its own stream, which is what the
  performance target asks for. Every row is one five-sample interleaved pass -
  the pass that landed with H.263 - so the rows are comparable with each other
  and with nothing else; re-run the script from a cold board before quoting one.
  The rows to trust are the 30, 90 and 120 picture streams. Under a dozen
  pictures the C side reports whole milliseconds, so a row like QCIF's is really
  "7 ms against 2.7 ms" and can move by tens of percentage points between passes:
  `hevc_multigop_120` has been recorded at 243, 261, 529 and 552 fps across
  passes of the same script. The four VP8 rows are their own pass
  (`BENCH_FILTER=vp8`), measured while the rest of the table's rows were not
  re-run. Non-interleaved single runs are not evidence at all:
  one lone `mpi_dec_test` pass over `test720.264` printed 1037 fps where the
  interleaved median is 166 fps.
- Encode throughput versus the source-built C encoder, from
  `scripts/board_bench_encode_vs_mpp.sh` - the encode side had never been measured
  at all, which left "never slower than the C MPP" unverified for every encoder
  feature so far. Twelve 16-aligned NV12 frames, QP 26, both sides writing to
  `/dev/null`, both timed externally in interleaved samples. The C tool's own
  `encode N frames time T ms` line would be the natural clock, but this vintage's
  `mpi_enc_test` prints nothing at all - not even its usage - so its internal
  timer is unreachable, and timing one side internally and the other externally
  would charge process startup to only one of them.

  Three passes, medians of interleaved samples; the spread across passes is given
  because it is not negligible:

  | input | C MPP | this crate | rust |
  |---|---|---|---|
  | 320x240 12p | 41-45 ms | 31-35 ms | +9% to +32% |
  | 640x480 12p | 51-54 ms | 44-48 ms | +8% to +16% |
  | 1280x720 12p | 81-91 ms | 64-73 ms | +16% to +30% |
  | 1920x1088 12p | 123-143 ms | 108-135 ms | parity, within noise |

  Those passes were all measured **before** the FBC body-offset fix, which means at
  720p and 1088 the encoder was reading its reference from a displaced address - it
  was timing a codec that coded a different picture than it thought it did. Re-measured
  after the fix (2026-11-02, `SAMPLES=5`, same board and script, from cold):
  320x240 39 to 23 ms (+70 %), 640x480 53 to 39 ms (+36 %), 1280x720 86 to 68 ms
  (+27 %), 1920x1088 134 to 135 ms (-1 %, parity again). The small-size rows came in
  above the band above, which is what twelve-picture rows do - the C side is whole
  milliseconds there and a single cold pass proves nothing - and the useful reading
  is that the 1088 row did not change character: the displacement cost bits, not
  frames.

  At 1080p-class the two distributions overlap - nine interleaved samples gave C
  {153,140,123,124,124,118,123,123,147} against our {149,108,136,135,144,109,134,110,148},
  medians 124 against 135 - so no claim of a win is made there, and the row is
  bimodal (about 110 and about 140) on both sides rather than one of them being
  erratic. Our codec loop for the same encode is ~80 ms of that, the rest being
  frame reads and roughly 18 MB of dma-buf per `open`, which the C encoder pays
  too.

  One real fix came out of measuring: the CLI read the whole input clip into
  memory before encoding, harmless while the encoder could only produce one IDR
  and a plain cost with a GOP - 12 frames of 1920x1088 is 37 MB read and held live
  to be consumed 3 MB at a time. It now streams one frame per picture, which
  moved that row's own reported work back into the timed region where it belongs.
  The six byte-exact GOP cells still pass after the change.

- Frame readback rule: the decoder output mapping is uncached, so a plane is
  read exactly once and eight bytes per load. `codec::yuv::nv12_to_planar` is
  the shared implementation behind H.264, H.265, VP9 and AV1. The per-sample
  copies it replaces read the interleaved chroma plane two or three times a byte
  at a time, which cost ~4 ms per 1080p frame and left H.264 11 % (720p) and
  32 % (1080p) *slower* than C MPP. `Picture::raw_uv` is materialised only under
  `RFI_DUMP_UV`; it was a third full chroma pass on every picture.
- Board VP9 regression: profile 0 and profile 2 10-bit 320x240 streams decode
  30/30 frames with `err=0`. Seven consecutive runs of each profile produced
  one stable SHA-256 per profile, byte-identical to FFmpeg planar output. The
  automatic-alt-ref 320x240 stream now decodes 60/60 with every hardware job
  clean and output matching both MPP and FFmpeg. Its colmv-lifetime regression
  is covered by the hidden-frame state-machine unit test. The current release
  build produced no new kernel reset or unknown-command message after the
  capability and lifetime fixes. The fixed 30-frame 1280x720
  error-resilient/temporal-segmentation regression emits 41,472,000 bytes,
  matches libvpx byte-for-byte (SHA-256
  `559a68695227f10c8243c0252fa3efc9bfae07090e5b92553ec35f341ce3ea65`),
  and decodes in 113.0 ms (265.5 fps). This is fixture-specific evidence, not
  a general VP9 throughput claim; the resize stream still needs a fixed-clock
  native-NV12 comparison.
- Board AV1 regression: the full local `cargo test --all-targets` suite passes
  311 tests with four hardware-only tests ignored by
  default. The focused CDF, parser, PicParams and register suite passes 54/54
  tests on t6. The 320x240/10-frame acceptance stream produces 1,152,000
  bytes of planar YUV byte-identical to C MPP output after converting MPP's
  native NV12. All 11 jobs have the same input CDF image as C MPP; ten
  comparable 512-word register images match outside process-specific DMA
  descriptors. A separate libaom 8-bit film-grain stream decodes 10/10
  pictures with zero hardware errors and matches libaom's grain-applied
  planar YUV byte-for-byte. Its Main10 counterpart emits 2,304,000 bytes and
  matches libaom's grain-applied `yuv420p10le` output byte-for-byte; the
  hash differs from libaom with grain skipped, proving the hardware grain
  path and the packed 10-bit unpack path are both active. The C MPP build
  emits the same stream without grain because its HAL never sets
  `sw_apply_grain=1`; the corpus sweep now labels both fixture names as
  `KNOWN-C-BUG` instead of `FAIL`, using the recorded libaom hashes. This
  remains a small fixture matrix, not full AV1 closure. The dedicated ignored
  integration test passes on t6, so a future regression in either the 8-bit
  NV12 or 10-bit packed output path fails with an explicit hash mismatch.
- Post-fix AV1 performance on t62, measured with 15 interleaved samples:
  `av1_320x240_10.ivf` is 22.0 ms in C and 19.6 ms in Rust (about 12.2 %
  higher Rust throughput). The tile fixture is now included in
  `scripts/board_bench_vs_mpp.sh`; seven interleaved samples put it at 24.0 ms
  in C and 20.0 ms in Rust (about 20 % higher Rust throughput).
  `BENCH_FILTER=av1` remains the reproducible subset command.
- The wider generated AV1 matrix is also faster after two output-path fixes:
  four 10-bit samples are unpacked from each five-byte group into preallocated
  planar output, and only the CPU-read display surface uses cached DMA32 while
  tile/reference buffers stay uncached for the hardware. Seven interleaved
  samples on t62 give Rust speed deltas of +32.6 % (320x240), +37.5 % (tiles),
  +361.9 % (Main10), +23.1 % (60-frame 640x480), +32.7 % (720p) and +7.4 %
  (1080p) relative to native-NV12 C output.
- A full t62 decoder benchmark over the regenerated corpus keeps every measured
  path at or above C once short-stream noise is removed. Representative
  five-sample medians include H.264 720p +148 %, H.265 Main10 +163 %, VP8
  IP +59 %, MPEG-4 IP +141 %, and progressive MPEG-2 +109 %. Fifteen-sample
  reruns put AV1 1080p at +13.0 %, VP8 altref at +72.8 %, and interlaced MPEG-2
  at +191.1 %. `board_bench_vs_mpp.sh` now parses both `(N output)` and
  `(N output, M packets dropped)` Rust summaries, so VP8/VP9 no longer appear
  as measurement SKIPs.
- `rk-mpp-rs m2v <file.m2v>` prints the MPEG-2 header chain,
  `rk-mpp-rs m2vdec <in.m2v> <out.yuv>`, `rk-mpp-rs mp4dec <in.m4v> <out.yuv>` and
  `rk-mpp-rs h263dec <in.263> <out.yuv>` decode MPEG-2, MPEG-4 and H.263 on vdpu2,
  and `rk-mpp-rs av1dec <in.ivf> <out.yuv>` gives AV1 the same CLI path the other
  decoders use, so `scripts/board_bench_vs_mpp.sh` can measure them.
- Real streams: `rk-mpp-rs h264 <file.264>` prints SPS/PPS/slice fields; validated
  against x264 output (720p main with B-frames, 1080p baseline).
- Bit-exactness target for every decoder: the software reference in
  `reference/` decodes the stream, and the hardware path must match it
  bit-for-bit. Both are validated against the *original* picture the test
  stream was encoded from (a synthetic source we generate), so no external
  decoder is needed.
- Performance target: never slower than the C MPP on the same stream.
