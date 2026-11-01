# Reimplementing the full MPP surface in Rust

The official Rockchip MPP is ~296k lines of C/C++ (decoders 43k, encoders 10k,
HAL 143k, framework 29k, osal/driver 18k). This file tracks every feature
point, what it costs in the original, and where we stand.

Scope is the whole library, not only what YOLO needs: all decoders, all
encoders, post-processing, and the framework features (async tasks, fences,
multi-core, secure mode).

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
| M7 | H.264 encode (rkvenc): RC (CBR/VBR), intra/inter, deblock, ROI, GDR | 15k | **IDR *and* P pictures byte-identical to the C encoder - 24 cells of size x profile x QP plus 6 GOP cells (1 IDR + up to 5 P, still and moving), SEI NAL units included, with zero non-address register differences in all seven sections; rate control, scene detection and ROI remain** |
| M8 | H.265 encode (rkvenc) + JPEG encode (vepu) | 12k | **JPEG 4:2:0 baseline done; H.264 encode generates its QP, profile, GOP, slice-type and P-picture fields rather than replaying them, and is still a recorded-image replay for rate control, level and the CIME/RIME corner of section 3 (see the encoder notes below); H.265 encode pending** |
| M9 | Post-processor: deinterlace, scale, format conversion | 8k | **not available as written - the engine MPP uses for this is VDPP (client 29), absent on this board; IEP (client 28) binds and is the M9 engine here. See the note below** |
| M10 | Framework: task queue, async completion, dma fences, multi-core CCU, secure mode | 29k | todo |
| M11 | Software reference decoders (our own ground truth, no ffmpeg) | — | **in progress** |

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
  - [ ] wider AV1 conformance and performance matrix: larger resolutions,
        long GOPs, multiple sequence headers, dynamic resolution, tiles,
        10-bit where supported and damaged streams still need C/FFmpeg
        differential fixtures. The current acceptance evidence is one
        320x240 stream and one 2x2 tile stream, not a general AV1 claim.
  - The 320x240 fixture is the initial acceptance baseline: official C MPP
    emits 10/10 frames byte-identical to FFmpeg
    (`2cd9c129…291d29` for the planar YUV output). The 64x64 fixture is not
    a valid MPP hardware acceptance case on this board: official C MPP also
    emits ten identical all-zero frames (`fd9243e1…263e` per frame), so a
    mismatch there must not be attributed to the Rust port
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
      it rewinds its own input. `scripts/board_regress_m2v.sh` locks all three.
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
- [ ] H.264 (rkvenc): rate control beyond fixed QP, QP map, ROI, GDR, deblock
      fine tuning, slice split and widths above 4096. P pictures, the DPB, the SEI
      NAL units and the Baseline/Main/High profiles are done, each in its own entry
      below; the scene detector and the >4096-pixel CIME lines are the open items
- [ ] H.265 (rkvenc): same plus tiles/WPP, SAO
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
- **What the H.264 IDR path actually is.** `src/hal/h264e.rs` does not generate
      the VEPU580 register file; it replays one captured from `mpi_enc_test` over
      an `MPP_CMD_SET_REG_WRITE` trace, patches the eleven address words, and
      derives only what a different picture size needs - the macroblock counts,
      the line-buffer layout, the source plane offsets and the search ranges.
      `validate_reference_config` is the boundary and it is deliberate: Baseline
      only, level 3.0, GOP 1, QP 26, CAVLC, no transform-8x8, and nothing past
      4096 pixels, everything else still coming from the recorded image. Byte-
      exactness against C across six sizes therefore shows the plumbing and the
      address arithmetic are right; it says nothing about `setup_vepu580_codec`,
      `setup_vepu580_rc_base`, `setup_vepu580_me` or `calc_cime_parameter` in
      `mpp/hal/rkenc/h264e/hal_h264e_vepu580.c`, none of which is ported. M7 is
      gated on replacing the template with that generator, not on adding features
      on top of a fixture.
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
- [ ] Rate control beyond fixed QP. `setup_vepu580_rc_base`'s VBR/CBR path -
      `rc_en`, `aq_en`, `rc_ctu_num`, `rc_qp_range`, the `rc_adj` ladder, the
      `rc_dthd` thresholds and the ROI area thresholds - is exactly the set the
      fixed-QP path leaves at zero, so it is the next thing to generate. It needs
      MPP's rate-control loop (`mpp/codec/rc/`) behind it, not just the registers.
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
      The model is not the last word in the register file either: `check_re_enc`
      can raise the QP and ask for the *same* picture again, which the current
      single-shot submit path has no way to express, so rate control also pulls in
      the re-encode loop.
- [ ] rkvenc supports one live session per process here. The two ignored encoder
      tests pass serially and disagree with the goldens when cargo runs them as two
      threads, which is `setup_ext_line_bufs` programming a shared system-controller
      line-buffer configuration for both sessions. Relevant to M10's multi-instance
      work.
- [ ] JPEG encoder (vepu): RGB input, 90/270-degree rotation, restart markers,
      low-delay slices and multi-core partitioning

### Post-processing
- [x] RGB888 letterbox scaler (own, faster than RGA for our sizes)
- [ ] IEP (`mpp/vproc/iep/iep.c`, `iep2/iep2.c`, client 28): deinterlace, YUV
      scale, crop. This is the M9 engine that exists on this board.
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
- [ ] async task queue with completion callbacks
- [ ] dma-fence in/out, buffer import/export
- [ ] multi-core (CCU) distribution
- [ ] secure/TA mode
- [ ] frame buffer pool with reference counting (DPB)

## Verification

- Tests: `cargo test --all-targets` (337 unit + 88 integration, eleven ignored
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
  `sw_apply_grain=1`. This remains a small fixture matrix, not full AV1
  closure. The dedicated ignored integration test passes on t6, so a future
  regression in either the 8-bit NV12 or 10-bit packed output path fails with
  an explicit hash mismatch.
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
