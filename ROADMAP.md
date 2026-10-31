# Reimplementing the full MPP surface in Rust

The official Rockchip MPP is ~296k lines of C/C++ (decoders 43k, encoders 10k,
HAL 143k, framework 29k, osal/driver 18k). This file tracks every feature
point, what it costs in the original, and where we stand.

Scope is the whole library, not only what YOLO needs: all decoders, all
encoders, post-processing, and the framework features (async tasks, fences,
multi-core, secure mode).

## Board capability map (NanoPC-T6-LTS, mask `0x1009321a`)

| block | present | drives |
|---|---|---|
| vdpu2 | yes | H.264, MPEG2, MPEG4, H.263, VP8 |
| vdpu2-pp | yes | deinterlace, scale, format conversion |
| rkvdec | yes | H.264, H.265, VP9, AVS2/AVS+ |
| av1d | yes | AV1 |
| jpegd | yes | baseline/progressive-free JPEG decode |
| rkv-enc | yes | H.264, H.265 encode |
| vepu-jpeg | yes | JPEG encode |
| vepu1/2/22 | no | — |

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
| M6 | MPEG2 / MPEG4 / H.263 / VP8 / AVS+ / AVS2 decode | 14k | **MPEG-2 decode done (syntax, vdpu2 register file, three-buffer DPB, `rk-mpp-rs m2vdec`): byte-exact vs C MPP on the progressive fixtures. Field-coded PAFF unverified. MPEG-4/H.263/VP8/AVS todo** |
| M7 | H.264 encode (rkvenc): RC (CBR/VBR), intra/inter, deblock, ROI, GDR | 15k | todo |
| M8 | H.265 encode (rkvenc) + JPEG encode (vepu) | 12k | **JPEG 4:2:0 baseline and parameterized H.264 Baseline IDR done; H.264 inter/SEI/RC and H.265 pending** |
| M9 | Post-processor: vdpu-pp deinterlace/scale/format, RGA interop | 8k | todo |
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
- [ ] MPEG4 ASP, H.263, VP8, AVS+, AVS2

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
- [ ] H.264 (rkvenc): P frames/DPB, SEI, CBR/VBR, QP map, ROI, GDR, deblock,
      slice split, Main/High profiles and widths above 4096
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
- [ ] JPEG encoder (vepu): RGB input, 90/270-degree rotation, restart markers,
      low-delay slices and multi-core partitioning

### Post-processing
- [x] RGB888 letterbox scaler (own, faster than RGA for our sizes)
- [ ] vdpu-pp: deinterlace, YUV scale, NV12↔RGB, crop
- [ ] 10-bit ↔ 8-bit conversion, HDR tone mapping

### Framework
- [x] synchronous submit + poll
- [ ] async task queue with completion callbacks
- [ ] dma-fence in/out, buffer import/export
- [ ] multi-core (CCU) distribution
- [ ] secure/TA mode
- [ ] frame buffer pool with reference counting (DPB)

## Verification

- Tests: `cargo test --all-targets` (272 unit + 88 integration, eleven ignored
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
- Decode throughput versus the source-built C MPP, from
  `scripts/board_bench_vs_mpp.sh`: five interleaved samples per binary on the
  same stream, medians. Rust covers submit + poll + DPB + NV12-to-planar
  conversion; C MPP covers its decode loop and writes native NV12, so the
  conversion work stays charged to the Rust number.

  | stream | pictures | C MPP | this crate | delta |
  | --- | ---: | ---: | ---: | ---: |
  | `test720.264` (H.264 720p) | 30 | 166 fps | 257 fps | +55 % |
  | `test_hard.264` (H.264 1080p) | 90 | 107 fps | 220 fps | +107 % |
  | `t720.hevc` (H.265 720p) | 10 | 109 fps | 242 fps | +123 % |
  | `hevc_multigop_120.hevc` (H.265 720p) | 120 | 244 fps | 542 fps | +122 % |
  | `hevc_main10_256.hevc` (H.265 Main10) | 30 | 157 fps | 1322 fps | +741 % |
  | `vp90_720.ivf` (VP9 720p) | 30 | 275 fps | 674 fps | +145 % |
  | `p0_320x240_30.ivf` (VP9 Profile 0) | 30 | 509 fps | 3409 fps | +571 % |
  | `av1_320x240_10.ivf` (AV1 Main 8-bit) | 10 | 526 fps | 800 fps | +52 % |
  | `m2v_320x240_12.m2v` (MPEG-2 320x240) | 10 | 500 fps | 1449 fps | +190 % |
  | `m2v_320x240_ilaced.m2v` (MPEG-2 interlaced) | 6 | 381 fps | 1177 fps | +312 % |
  | `m2v_64x64_3.m2v` (MPEG-2 64x64) | 3 | 750 fps | 2727 fps | +264 % |

  Every decoder we ship now beats the C library on its own stream, which is what
  the performance target asks for. The rows to quote are the 30, 90 and 120
  picture streams: the two small-picture rows (320x256 Main10, 320x240 VP9) swing
  by a factor of two between runs because they are dominated by fixed per-run
  overhead, and `hevc_multigop_120` measured 405 fps on a five-sample pass
  against 542 fps on a seven-sample pass. Non-interleaved single runs are not
  evidence at all: one lone `mpi_dec_test` pass over `test720.264` printed
  1037 fps where the interleaved median is 166 fps.

  The MPEG-2 rows are from a later pass than the rows above them, and that pass
  re-measured the older streams too: `hevc_main10_256` came in at 748 fps where
  this table records 1322 fps, and `vp90_720.ivf` at 1027 fps where it records
  674 fps, which is the swing the caveat above describes. All three MPEG-2
  fixtures are 3 to 10 pictures, so their margins say more about per-run overhead
  than about steady-state throughput on a stream long enough to matter.
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
  `rk-mpp-rs m2vdec <in.m2v> <out.yuv>` decodes it on vdpu2, and
  `rk-mpp-rs av1dec <in.ivf> <out.yuv>` gives AV1 the same CLI path the other
  decoders use, so `scripts/board_bench_vs_mpp.sh` can measure them.
- Real streams: `rk-mpp-rs h264 <file.264>` prints SPS/PPS/slice fields; validated
  against x264 output (720p main with B-frames, 1080p baseline).
- Bit-exactness target for every decoder: the software reference in
  `reference/` decodes the stream, and the hardware path must match it
  bit-for-bit. Both are validated against the *original* picture the test
  stream was encoded from (a synthetic source we generate), so no external
  decoder is needed.
- Performance target: never slower than the C MPP on the same stream.
