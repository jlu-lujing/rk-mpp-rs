//! Host-side checks for the IEP2 (client 28) register derivation.
//!
//! The deinterlace job needs RK3588 hardware, so the on-board comparison lives
//! in `scripts/board_iep_dei_vs_mpp.sh`. Everything that is pure arithmetic —
//! the `struct iep2_params` field table, the `IEP2_PARAM_TYPE_COM` derivations,
//! and the dma-buf address encoding — is checked here against the C source it
//! was read out of.

use rk_mpp_rs::hal::iep;
use rk_mpp_rs::hal::iep::{
    DeinterlaceJob, Iep2DilMode, Iep2FieldOrder, Iep2Fmt, Iep2OutMode, Iep2YuvSwap,
};

#[test]
fn register_block_is_the_size_iep2_start_sends() {
    // sizeof(struct iep2_params) == 656 bytes == 164 words; this is the exact
    // SET_REG_WRITE buffer size, so a missing or extra field would shift every
    // hardware register the kernel maps.
    assert_eq!(iep::PARAMS_WORDS, 164);
    assert_eq!(iep::PARAMS_WORDS * 4, 656);
    assert_eq!(iep::OUTPUT_WORDS, 103);
}

#[test]
fn field_offsets_match_the_c_layout() {
    // src[3] then dst[2] then mv/md pack contiguously after the strides.
    assert_eq!(iep::SRC0_Y, 9);
    assert_eq!(iep::SRC1_Y, iep::SRC0_Y + 3);
    assert_eq!(iep::SRC2_Y, iep::SRC1_Y + 3);
    assert_eq!(iep::DST0_Y, iep::SRC2_Y + 3);
    assert_eq!(iep::DST1_Y, iep::DST0_Y + 3);
    assert_eq!(iep::MV_ADDR, iep::DST1_Y + 3);
    // The mode triple sits right after mv/md addresses.
    assert_eq!(iep::DIL_MODE, iep::MD_ADDR + 1);
    assert_eq!(iep::DIL_FIELD_ORDER, iep::DIL_MODE + 2);
    // The final array (yend[8]) must be the last thing in the struct.
    assert_eq!(iep::YEND + 8, iep::PARAMS_WORDS);
}

#[test]
fn com_derivation_matches_the_c_integer_math() {
    // iep2_set_param(IEP2_PARAM_TYPE_COM): strides/4, tiles ceil-div, pec/128.
    assert_eq!(iep::tile_cols(720), 45);
    assert_eq!(iep::tile_cols(1920), 120); // the driver's IEP2_TILE_W_MAX
    assert_eq!(iep::tile_rows(480), 120);
    assert_eq!(iep::tile_rows(1088), 272);
    assert_eq!(iep::osd_pec_thr(1920), ((1920usize * 26) >> 7) as u32);
    let (y, uv, dy) = iep::strides(1920, Iep2YuvSwap::SpUv);
    assert_eq!((y, uv, dy), (480, 480, 480));
}

#[test]
fn planar_and_semiplanar_chroma_strides_differ() {
    // SWAP_P halves the horizontal count then rounds up to 16 pixels first.
    let (_, uv_p, _) = iep::strides(721, Iep2YuvSwap::P);
    let (_, uv_sp, _) = iep::strides(721, Iep2YuvSwap::SpUv);
    assert_eq!(uv_p, (((721 / 2 + 15) / 16 * 16) / 4) as u32);
    assert_eq!(uv_sp, (721 / 4) as u32);
    assert!(uv_p < uv_sp);
}

#[test]
fn frame_bytes_follows_the_test_harness() {
    assert_eq!(Iep2Fmt::Yuv420.frame_bytes(720, 480), 720 * 480 * 3 / 2);
    assert_eq!(Iep2Fmt::Yuv422.frame_bytes(720, 480), 720 * 480 * 2);
}

#[test]
fn buf_reg_is_fd_plus_offset_shifted_ten() {
    // set_addr()/ADD_OFFSET in the C test: fd + (byte_offset << 10).
    assert_eq!(iep::buf_reg(5, 0), 5);
    assert_eq!(iep::buf_reg(5, 345600), (345600u32 << 10) | 5);
}

fn job() -> DeinterlaceJob<'static> {
    DeinterlaceJob {
        width: 720,
        height: 480,
        hor_stride: 720,
        src_fmt: Iep2Fmt::Yuv420,
        src_swap: Iep2YuvSwap::SpUv,
        dst_fmt: Iep2Fmt::Yuv420,
        dst_swap: Iep2YuvSwap::SpUv,
        dil_mode: Iep2DilMode::I5O2,
        out_mode: Iep2OutMode::Line,
        field_order: Iep2FieldOrder::Tff,
        src_fds: [11, 12, 13],
        dst_fds: [14, 15],
        mv_fd: 16,
        md_fd: 17,
        md_lambda: 4,
        _marker: std::marker::PhantomData,
    }
}

#[test]
fn deinterlace_block_is_well_formed() {
    let w = iep::build_deinterlace(&job());
    assert_eq!(w.len(), iep::PARAMS_WORDS);

    // Geometry + mode from the COM/MODE derivations.
    assert_eq!(w[iep::SRC_FMT], Iep2Fmt::Yuv420 as u32);
    assert_eq!(w[iep::DIL_MODE], Iep2DilMode::I5O2 as u32);
    assert_eq!(w[iep::TILE_COLS], 45);
    assert_eq!(w[iep::TILE_ROWS], 120);

    // The five buffer bases must be distinct dma-bufs, each in its word group.
    let y_size = 720 * 480;
    assert_eq!(w[iep::SRC0_Y], 11);
    assert_eq!(w[iep::SRC1_Y], 12);
    assert_eq!(w[iep::SRC2_Y], 13);
    assert_eq!(w[iep::SRC0_Y + 1], iep::buf_reg(11, y_size));
    assert_eq!(w[iep::MV_ADDR], 16);
    assert_eq!(w[iep::MD_ADDR], 17);

    // Everything the iep2_test path leaves at zero really is zero.
    assert_eq!(w[iep::ROI_EN], 0);
    assert_eq!(w[iep::OSD_AREA_NUM], 0);
    assert_eq!(w[iep::COMB_CNT_THR], 0);
}

#[test]
fn field_order_tff_and_bff_are_distinguishable() {
    let mut tff = job();
    tff.field_order = Iep2FieldOrder::Tff;
    let mut bff = job();
    bff.field_order = Iep2FieldOrder::Bff;
    let wt = iep::build_deinterlace(&tff);
    let wb = iep::build_deinterlace(&bff);
    assert_eq!(wt[iep::DIL_FIELD_ORDER], 0);
    assert_eq!(wb[iep::DIL_FIELD_ORDER], 1);
}

#[test]
fn scratch_buffers_match_the_driver_allocations() {
    // mv = IEP2_TILE_W_MAX * IEP2_TILE_H_MAX, md = 1920 * 1088 (iep2_init).
    assert_eq!(iep::MV_BUF_BYTES, 120 * 480);
    assert_eq!(iep::MD_BUF_BYTES, 1920 * 1088);
}
