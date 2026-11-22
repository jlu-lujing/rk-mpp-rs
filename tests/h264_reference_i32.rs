use rk_mpp_rs::codec::h264::parser::{parse_pps, parse_slice_header, parse_sps, split_annex_b};
use rk_mpp_rs::reference::h264_cabac::{
    decode_cabac_b16_slice, decode_cabac_i_picture_slices, decode_cabac_p_multi_ref_slice,
    decode_cabac_p_skip_slice, decode_cabac_p_slice, CabacReferenceFrame,
    CabacReferenceMotionFrame, DecodedMotionPartition,
};
use rk_mpp_rs::reference::h264_deblock::{deblock_b_picture, deblock_i_picture, deblock_p_picture};
use rk_mpp_rs::reference::h264_recon::PlaneMut;
use rk_mpp_rs::reference::h264_slice::decode_i_slice;

#[test]
fn black_idr_matches_ffmpeg_yuv420p() {
    compare_fixture(
        include_bytes!("../tests/data/h264_i32_black_idr.264"),
        include_bytes!("../tests/data/h264_i32_black_idr.yuv"),
    )
    .unwrap();
}

#[test]
fn textured_idr_matches_ffmpeg_yuv420p() {
    compare_fixture(
        include_bytes!("../tests/data/h264_i32_cavlc.264"),
        include_bytes!("../tests/data/h264_i32_cavlc.yuv"),
    )
    .unwrap();
}

#[test]
fn cabac_midpoint_idr_matches_ffmpeg_yuv420p() {
    compare_fixture(
        include_bytes!("../tests/data/h264_i16_cabac.264"),
        include_bytes!("../tests/data/h264_i16_cabac.yuv"),
    )
    .unwrap();
}

#[test]
fn cabac_luma_dc_residual_matches_ffmpeg_yuv420p() {
    compare_fixture(
        include_bytes!("../tests/data/h264_i16_cabac_dc.264"),
        include_bytes!("../tests/data/h264_i16_cabac_dc.yuv"),
    )
    .unwrap();
}

#[test]
fn cabac_textured_i16_matches_ffmpeg_yuv420p() {
    compare_fixture(
        include_bytes!("../tests/data/h264_i16_cabac_texture.264"),
        include_bytes!("../tests/data/h264_i16_cabac_texture.yuv"),
    )
    .unwrap();
}

#[test]
fn cabac_two_macroblock_i16_matches_ffmpeg_yuv420p() {
    compare_fixture(
        include_bytes!("../tests/data/h264_i16_cabac_multi.264"),
        include_bytes!("../tests/data/h264_i16_cabac_multi.yuv"),
    )
    .unwrap();
}

#[test]
fn cabac_two_by_two_i16_matches_ffmpeg_yuv420p() {
    compare_fixture(
        include_bytes!("../tests/data/h264_i16_cabac_2x2.264"),
        include_bytes!("../tests/data/h264_i16_cabac_2x2.yuv"),
    )
    .unwrap();
}

#[test]
fn cabac_two_by_two_i16_without_deblock_matches_ffmpeg_yuv420p() {
    compare_fixture(
        include_bytes!("../tests/data/h264_i16_cabac_2x2_nodeblock.264"),
        include_bytes!("../tests/data/h264_i16_cabac_2x2_nodeblock.yuv"),
    )
    .unwrap();
}

#[test]
fn cabac_textured_i4x4_matches_ffmpeg_yuv420p() {
    compare_fixture(
        include_bytes!("../tests/data/h264_i4x4_cabac.264"),
        include_bytes!("../tests/data/h264_i4x4_cabac.yuv"),
    )
    .unwrap();
}

#[test]
fn cabac_textured_i8x8_matches_ffmpeg_yuv420p() {
    compare_fixture(
        include_bytes!("../tests/data/h264_i8x8_cabac.264"),
        include_bytes!("../tests/data/h264_i8x8_cabac.yuv"),
    )
    .unwrap();
}

#[test]
fn cabac_i_pcm_matches_ffmpeg_yuv420p() {
    compare_fixture(
        include_bytes!("../tests/data/h264_i_pcm_cabac.264"),
        include_bytes!("../tests/data/h264_i_pcm_cabac.yuv"),
    )
    .unwrap();
}

#[test]
fn cabac_ten_bit_i_pcm_matches_ffmpeg_yuv420p10le() {
    compare_fixture(
        include_bytes!("../tests/data/h264_i_pcm_cabac_10bit.264"),
        include_bytes!("../tests/data/h264_i_pcm_cabac_10bit.yuv"),
    )
    .unwrap();
}

#[test]
fn cabac_mixed_i8x8_and_i4x4_across_rows_matches_ffmpeg_yuv420p() {
    compare_fixture(
        include_bytes!("../tests/data/h264_cabac_mixed_2x2.264"),
        include_bytes!("../tests/data/h264_cabac_mixed_2x2.yuv"),
    )
    .unwrap();
}

#[test]
fn cabac_mixed_i8x8_and_i4x4_without_deblock_matches_ffmpeg_yuv420p() {
    compare_fixture(
        include_bytes!("../tests/data/h264_cabac_mixed_2x2_nodeblock.264"),
        include_bytes!("../tests/data/h264_cabac_mixed_2x2_nodeblock.yuv"),
    )
    .unwrap();
}

#[test]
fn cabac_two_slices_with_nonzero_first_mb_match_ffmpeg_yuv420p() {
    compare_multislice_fixture(
        include_bytes!("../tests/data/h264_cabac_two_slices.264"),
        include_bytes!("../tests/data/h264_cabac_two_slices.yuv"),
    )
    .unwrap();
}

#[test]
fn cabac_two_slices_without_deblock_match_ffmpeg_yuv420p() {
    compare_multislice_fixture(
        include_bytes!("../tests/data/h264_cabac_two_slices_nodeblock.264"),
        include_bytes!("../tests/data/h264_cabac_two_slices_nodeblock.yuv"),
    )
    .unwrap();
}

#[test]
fn cabac_p_skip_copies_reference_macroblocks_matches_ffmpeg_yuv420p() {
    compare_cabac_p_skip_fixture(
        include_bytes!("../tests/data/h264_cabac_pskip_48x32.264"),
        include_bytes!("../tests/data/h264_cabac_pskip_48x32.yuv"),
    )
    .unwrap();
}

#[test]
fn cabac_p16_zero_mv_residuals_match_ffmpeg_yuv420p() {
    compare_cabac_p16_zero_mv_fixture(
        include_bytes!("../tests/data/h264_cabac_p16_48x32.264"),
        include_bytes!("../tests/data/h264_cabac_p16_48x32.yuv"),
    )
    .unwrap();
}

#[test]
fn cabac_p16_integer_motion_matches_ffmpeg_yuv420p() {
    compare_cabac_p16_motion_fixture(
        include_bytes!("../tests/data/h264_cabac_p16_mv_48x32.264"),
        include_bytes!("../tests/data/h264_cabac_p16_mv_48x32.yuv"),
        false,
        PartitionLayout::Uniform(16, 16),
        None,
        false,
    )
    .unwrap();
}

#[test]
fn cabac_p16_fractional_motion_matches_ffmpeg_yuv420p() {
    compare_cabac_p16_motion_fixture(
        include_bytes!("../tests/data/h264_cabac_p16_fractional_mv_48x32.264"),
        include_bytes!("../tests/data/h264_cabac_p16_fractional_mv_48x32.yuv"),
        true,
        PartitionLayout::Uniform(16, 16),
        None,
        false,
    )
    .unwrap();
}

#[test]
fn cabac_p16x8_two_motion_vectors_match_ffmpeg_yuv420p() {
    compare_cabac_p16_motion_fixture(
        include_bytes!("../tests/data/h264_cabac_p16x8_48x32.264"),
        include_bytes!("../tests/data/h264_cabac_p16x8_48x32.yuv"),
        false,
        PartitionLayout::Uniform(16, 8),
        None,
        false,
    )
    .unwrap();
}

#[test]
fn cabac_p8x16_two_motion_vectors_match_ffmpeg_yuv420p() {
    compare_cabac_p16_motion_fixture(
        include_bytes!("../tests/data/h264_cabac_p8x16_48x32.264"),
        include_bytes!("../tests/data/h264_cabac_p8x16_48x32.yuv"),
        false,
        PartitionLayout::Uniform(8, 16),
        None,
        false,
    )
    .unwrap();
}

#[test]
fn cabac_p8x8_mixed_subpartitions_match_ffmpeg_yuv420p() {
    compare_cabac_p16_motion_fixture(
        include_bytes!("../tests/data/h264_cabac_p8x8_48x32.264"),
        include_bytes!("../tests/data/h264_cabac_p8x8_48x32.yuv"),
        false,
        PartitionLayout::P8x8,
        None,
        false,
    )
    .unwrap();
}

#[test]
fn cabac_p16_explicit_weighted_prediction_matches_ffmpeg_yuv420p() {
    compare_cabac_p16_motion_fixture(
        include_bytes!("../tests/data/h264_cabac_p16_weighted_48x32.264"),
        include_bytes!("../tests/data/h264_cabac_p16_weighted_48x32.yuv"),
        false,
        PartitionLayout::Uniform(16, 16),
        Some((true, true)),
        false,
    )
    .unwrap();
}

#[test]
fn cabac_p16_deblocking_matches_ffmpeg_yuv420p() {
    compare_cabac_p16_motion_fixture(
        include_bytes!("../tests/data/h264_cabac_p16_deblock_48x32.264"),
        include_bytes!("../tests/data/h264_cabac_p16_deblock_48x32.yuv"),
        false,
        PartitionLayout::Uniform(16, 16),
        None,
        true,
    )
    .unwrap();
}

#[test]
fn cabac_p16_multiple_references_match_ffmpeg_yuv420p() {
    compare_cabac_p16_multiref_fixture(
        include_bytes!("../tests/data/h264_cabac_p16_multiref_48x32.264"),
        include_bytes!("../tests/data/h264_cabac_p16_multiref_48x32.yuv"),
    )
    .unwrap();
}

#[test]
fn cabac_b_l0_16x16_matches_ffmpeg_yuv420p() {
    compare_cabac_b16_fixture(
        include_bytes!("../tests/data/h264_cabac_b_l0_16x16_48x32.264"),
        include_bytes!("../tests/data/h264_cabac_b_l0_16x16_48x32.yuv"),
        Some(0),
    )
    .unwrap();
}

#[test]
fn cabac_b_l1_16x16_matches_ffmpeg_yuv420p() {
    compare_cabac_b16_fixture(
        include_bytes!("../tests/data/h264_cabac_b_l1_16x16_48x32.264"),
        include_bytes!("../tests/data/h264_cabac_b_l1_16x16_48x32.yuv"),
        Some(1),
    )
    .unwrap();
}

#[test]
fn cabac_b_bi_16x16_matches_ffmpeg_yuv420p() {
    compare_cabac_b16_fixture(
        include_bytes!("../tests/data/h264_cabac_b_bi_16x16_48x32.264"),
        include_bytes!("../tests/data/h264_cabac_b_bi_16x16_48x32.yuv"),
        Some(2),
    )
    .unwrap();
}

#[test]
fn cabac_b_direct_and_skip_match_ffmpeg_yuv420p() {
    compare_cabac_b16_fixture(
        include_bytes!("../tests/data/h264_cabac_b_direct_skip_48x32.264"),
        include_bytes!("../tests/data/h264_cabac_b_direct_skip_48x32.yuv"),
        None,
    )
    .unwrap();
}

#[test]
fn cabac_b_temporal_direct_and_skip_match_ffmpeg_yuv420p() {
    compare_cabac_b16_fixture(
        include_bytes!("../tests/data/h264_cabac_b_temporal_direct_skip_48x32.264"),
        include_bytes!("../tests/data/h264_cabac_b_temporal_direct_skip_48x32.yuv"),
        None,
    )
    .unwrap();
}

#[test]
fn cabac_b_16x8_l0_matches_ffmpeg_yuv420p() {
    compare_cabac_b16_fixture(
        include_bytes!("../tests/data/h264_cabac_b_16x8_l0_48x32.264"),
        include_bytes!("../tests/data/h264_cabac_b_16x8_l0_48x32.yuv"),
        Some(0),
    )
    .unwrap();
}

#[test]
fn cabac_b_8x16_l0_matches_ffmpeg_yuv420p() {
    compare_cabac_b16_fixture(
        include_bytes!("../tests/data/h264_cabac_b_8x16_l0_48x32.264"),
        include_bytes!("../tests/data/h264_cabac_b_8x16_l0_48x32.yuv"),
        Some(0),
    )
    .unwrap();
}

#[test]
fn cabac_b_8x8_l0_matches_ffmpeg_yuv420p() {
    compare_cabac_b16_fixture(
        include_bytes!("../tests/data/h264_cabac_b_8x8_l0_48x32.264"),
        include_bytes!("../tests/data/h264_cabac_b_8x8_l0_48x32.yuv"),
        Some(0),
    )
    .unwrap();
}

#[test]
fn cabac_b_8x8_l1_matches_ffmpeg_yuv420p() {
    compare_cabac_b16_fixture(
        include_bytes!("../tests/data/h264_cabac_b_8x8_l1_48x32.264"),
        include_bytes!("../tests/data/h264_cabac_b_8x8_l1_48x32.yuv"),
        Some(1),
    )
    .unwrap();
}

#[test]
fn cabac_b_8x8_bi_matches_ffmpeg_yuv420p() {
    compare_cabac_b16_fixture(
        include_bytes!("../tests/data/h264_cabac_b_8x8_bi_48x32.264"),
        include_bytes!("../tests/data/h264_cabac_b_8x8_bi_48x32.yuv"),
        Some(2),
    )
    .unwrap();
}

#[test]
fn cabac_b_8x8_mixed_l0_matches_ffmpeg_yuv420p() {
    compare_cabac_b16_fixture(
        include_bytes!("../tests/data/h264_cabac_b_8x8_mixed_l0_48x32.264"),
        include_bytes!("../tests/data/h264_cabac_b_8x8_mixed_l0_48x32.yuv"),
        Some(0),
    )
    .unwrap();
}

#[test]
fn cabac_b_deblocking_matches_ffmpeg_yuv420p() {
    compare_cabac_b16_fixture_with_deblock(
        include_bytes!("../tests/data/h264_cabac_b_deblock_48x32.264"),
        include_bytes!("../tests/data/h264_cabac_b_deblock_48x32.yuv"),
        None,
    )
    .unwrap();
}

#[test]
fn sixteen_macroblock_luma_matches_ffmpeg_yuv420p() {
    compare_fixture_planes(
        include_bytes!("../tests/data/h264_i64_cavlc.264"),
        include_bytes!("../tests/data/h264_i64_cavlc.yuv"),
        false,
    )
    .unwrap();
}

#[test]
fn sixteen_macroblock_yuv_matches_ffmpeg_yuv420p() {
    compare_fixture(
        include_bytes!("../tests/data/h264_i64_cavlc.264"),
        include_bytes!("../tests/data/h264_i64_cavlc.yuv"),
    )
    .unwrap();
}

fn compare_multislice_fixture(stream: &[u8], expected: &[u8]) -> Result<(), String> {
    let nals = split_annex_b(stream);
    let sps = parse_sps(
        &nals
            .iter()
            .find(|nal| nal.unit_type == 7)
            .ok_or("missing SPS")?
            .rbsp,
    )
    .map_err(|error| error.to_string())?;
    let pps = parse_pps(
        &nals
            .iter()
            .find(|nal| nal.unit_type == 8)
            .ok_or("missing PPS")?
            .rbsp,
    )
    .map_err(|error| error.to_string())?;
    let slice_nals: Vec<_> = nals.iter().filter(|nal| nal.is_vcl()).collect();
    let headers: Vec<_> = slice_nals
        .iter()
        .map(|nal| {
            parse_slice_header(&nal.rbsp, &sps, &pps, nal.unit_type, nal.ref_idc)
                .map_err(|error| error.to_string())
        })
        .collect::<Result<_, _>>()?;
    if headers.len() != 2 || headers[0].first_mb != 0 || headers[1].first_mb != 2 {
        return Err(format!(
            "unexpected slice layout: {:?}",
            headers
                .iter()
                .map(|slice| slice.first_mb)
                .collect::<Vec<_>>()
        ));
    }
    let slices: Vec<_> = slice_nals
        .iter()
        .zip(&headers)
        .map(|(nal, header)| (nal.rbsp.as_ref(), header))
        .collect();

    let width = sps.coded_width as usize;
    let height = sps.coded_height as usize;
    let cw = width / 2;
    let ch = height / 2;
    let mut y = vec![0u16; width * height];
    let mut u = vec![0u16; cw * ch];
    let mut v = vec![0u16; cw * ch];
    let decoded =
        decode_cabac_i_picture_slices(&slices, &sps, &pps, &mut y, Some(&mut u), Some(&mut v))
            .map_err(|error| error.to_string())?;
    if decoded.macroblocks != 4 {
        return Err(format!("decoded {} macroblocks", decoded.macroblocks));
    }

    if pps.deblocking_filter_control && headers[0].deblocking_disable_idc != 1 {
        let mut yp = PlaneMut {
            data: &mut y,
            width,
            height,
            stride: width,
        };
        let mut up = PlaneMut {
            data: &mut u,
            width: cw,
            height: ch,
            stride: cw,
        };
        let mut vp = PlaneMut {
            data: &mut v,
            width: cw,
            height: ch,
            stride: cw,
        };
        deblock_i_picture(
            &mut yp,
            Some(&mut up),
            Some(&mut vp),
            &decoded.deblock,
            8,
            headers[0].deblocking_alpha * 2,
            headers[0].deblocking_beta * 2,
            [pps.chroma_qp_offset, pps.second_chroma_qp_offset],
        )
        .map_err(str::to_owned)?;
    }

    let mut actual = Vec::with_capacity(expected.len());
    actual.extend(y.iter().map(|&sample| sample as u8));
    actual.extend(u.iter().map(|&sample| sample as u8));
    actual.extend(v.iter().map(|&sample| sample as u8));
    compare_samples(&actual, expected, &[y.len(), u.len(), v.len()])
}

fn compare_cabac_p_skip_fixture(stream: &[u8], expected: &[u8]) -> Result<(), String> {
    let nals = split_annex_b(stream);
    let sps = parse_sps(
        &nals
            .iter()
            .find(|nal| nal.unit_type == 7)
            .ok_or("missing SPS")?
            .rbsp,
    )
    .map_err(|error| error.to_string())?;
    let pps = parse_pps(
        &nals
            .iter()
            .find(|nal| nal.unit_type == 8)
            .ok_or("missing PPS")?
            .rbsp,
    )
    .map_err(|error| error.to_string())?;
    let slice_nals: Vec<_> = nals.iter().filter(|nal| nal.is_vcl()).collect();
    if slice_nals.len() != 2 {
        return Err(format!("expected IDR+P, got {} slices", slice_nals.len()));
    }
    let headers: Vec<_> = slice_nals
        .iter()
        .map(|nal| {
            parse_slice_header(&nal.rbsp, &sps, &pps, nal.unit_type, nal.ref_idc)
                .map_err(|error| error.to_string())
        })
        .collect::<Result<_, _>>()?;
    if !headers[0].is_i() || !headers[1].is_p() {
        return Err(format!(
            "expected IDR+P, got slice types {}+{}",
            headers[0].slice_type, headers[1].slice_type
        ));
    }
    if headers[0].deblocking_disable_idc != 1 || headers[1].deblocking_disable_idc != 1 {
        return Err("the P_Skip fixture must disable deblocking".into());
    }

    let width = sps.coded_width as usize;
    let height = sps.coded_height as usize;
    let cw = width / 2;
    let ch = height / 2;
    let mut reference_y = vec![0u16; width * height];
    let mut reference_u = vec![0u16; cw * ch];
    let mut reference_v = vec![0u16; cw * ch];
    let idr = decode_i_slice(
        &slice_nals[0].rbsp,
        &headers[0],
        &sps,
        &pps,
        &mut reference_y,
        Some(&mut reference_u),
        Some(&mut reference_v),
    )
    .map_err(|error| error.to_string())?;
    if idr.macroblocks != (width / 16) * (height / 16) {
        return Err(format!("IDR decoded {} macroblocks", idr.macroblocks));
    }

    let ref_y = reference_y.clone();
    let ref_u = reference_u.clone();
    let ref_v = reference_v.clone();
    let mut p_y = vec![0u16; width * height];
    let mut p_u = vec![0u16; cw * ch];
    let mut p_v = vec![0u16; cw * ch];
    let p = decode_cabac_p_skip_slice(
        &slice_nals[1].rbsp,
        &headers[1],
        &sps,
        &pps,
        &ref_y,
        &ref_u,
        &ref_v,
        &mut p_y,
        &mut p_u,
        &mut p_v,
    )
    .map_err(|error| error.to_string())?;
    let macroblocks = (width / 16) * (height / 16);
    if p.macroblocks != macroblocks {
        return Err(format!("P slice decoded {} macroblocks", p.macroblocks));
    }
    if p_y != reference_y || p_u != reference_u || p_v != reference_v {
        return Err("P_Skip output does not reproduce the reference picture".into());
    }

    let frame_len = reference_y.len() + reference_u.len() + reference_v.len();
    if expected.len() != frame_len * 2 {
        return Err(format!(
            "expected {frame_len} bytes per frame, got {} total",
            expected.len()
        ));
    }
    let mut actual = Vec::with_capacity(expected.len());
    for plane in [&reference_y, &reference_u, &reference_v, &p_y, &p_u, &p_v] {
        actual.extend(plane.iter().map(|&sample| sample as u8));
    }
    compare_samples(
        &actual,
        expected,
        &[
            reference_y.len(),
            reference_u.len(),
            reference_v.len(),
            p_y.len(),
            p_u.len(),
            p_v.len(),
        ],
    )
}

fn compare_cabac_p16_zero_mv_fixture(stream: &[u8], expected: &[u8]) -> Result<(), String> {
    let nals = split_annex_b(stream);
    let sps = parse_sps(
        &nals
            .iter()
            .find(|nal| nal.unit_type == 7)
            .ok_or("missing SPS")?
            .rbsp,
    )
    .map_err(|error| error.to_string())?;
    let pps = parse_pps(
        &nals
            .iter()
            .find(|nal| nal.unit_type == 8)
            .ok_or("missing PPS")?
            .rbsp,
    )
    .map_err(|error| error.to_string())?;
    let slice_nals: Vec<_> = nals.iter().filter(|nal| nal.is_vcl()).collect();
    if slice_nals.len() != 2 {
        return Err(format!("expected IDR+P, got {} slices", slice_nals.len()));
    }
    let headers: Vec<_> = slice_nals
        .iter()
        .map(|nal| {
            parse_slice_header(&nal.rbsp, &sps, &pps, nal.unit_type, nal.ref_idc)
                .map_err(|error| error.to_string())
        })
        .collect::<Result<_, _>>()?;
    if !headers[0].is_i() || !headers[1].is_p() {
        return Err(format!(
            "expected IDR+P, got slice types {}+{}",
            headers[0].slice_type, headers[1].slice_type
        ));
    }
    if headers[0].deblocking_disable_idc != 1 || headers[1].deblocking_disable_idc != 1 {
        return Err("the P16 fixture must disable deblocking".into());
    }
    if pps.pic_init_qp + headers[1].qp_delta != 20 || headers[1].cabac_init_idc != Some(0) {
        return Err(format!(
            "unexpected P CABAC setup: init_qp={}, slice_delta={}, cabac_init_idc={:?}",
            pps.pic_init_qp, headers[1].qp_delta, headers[1].cabac_init_idc
        ));
    }
    if headers[1].data_offset_bits != 24 {
        return Err(format!(
            "unexpected P CABAC data offset: {} bits",
            headers[1].data_offset_bits
        ));
    }

    let width = sps.coded_width as usize;
    let height = sps.coded_height as usize;
    let cw = width / 2;
    let ch = height / 2;
    let mut reference_y = vec![0u16; width * height];
    let mut reference_u = vec![0u16; cw * ch];
    let mut reference_v = vec![0u16; cw * ch];
    let idr = decode_i_slice(
        &slice_nals[0].rbsp,
        &headers[0],
        &sps,
        &pps,
        &mut reference_y,
        Some(&mut reference_u),
        Some(&mut reference_v),
    )
    .map_err(|error| error.to_string())?;
    let macroblocks = (width / 16) * (height / 16);
    if idr.macroblocks != macroblocks {
        return Err(format!("IDR decoded {} macroblocks", idr.macroblocks));
    }

    let ref_y = reference_y.clone();
    let ref_u = reference_u.clone();
    let ref_v = reference_v.clone();
    let mut p_y = vec![0u16; width * height];
    let mut p_u = vec![0u16; cw * ch];
    let mut p_v = vec![0u16; cw * ch];
    let p = decode_cabac_p_slice(
        &slice_nals[1].rbsp,
        &headers[1],
        &sps,
        &pps,
        &ref_y,
        &ref_u,
        &ref_v,
        &mut p_y,
        &mut p_u,
        &mut p_v,
    )
    .map_err(|error| error.to_string())?;
    if p.macroblocks != macroblocks {
        return Err(format!("P slice decoded {} macroblocks", p.macroblocks));
    }
    if p.motion_partitions.len() != macroblocks {
        return Err(format!(
            "zero-motion P16 slice decoded {} partitions",
            p.motion_partitions.len()
        ));
    }
    if p.motion_partitions
        .iter()
        .any(|partition| partition.motion_vector != [0, 0])
    {
        return Err("zero-motion P16 fixture produced a non-zero motion vector".into());
    }
    if p_y == reference_y && p_u == reference_u && p_v == reference_v {
        return Err("P16 fixture did not exercise an inter residual".into());
    }

    let frame_len = reference_y.len() + reference_u.len() + reference_v.len();
    if expected.len() != frame_len * 2 {
        return Err(format!(
            "expected {frame_len} bytes per frame, got {} total",
            expected.len()
        ));
    }
    let mut actual = Vec::with_capacity(expected.len());
    for plane in [&reference_y, &reference_u, &reference_v, &p_y, &p_u, &p_v] {
        actual.extend(plane.iter().map(|&sample| sample as u8));
    }
    compare_samples(
        &actual,
        expected,
        &[
            reference_y.len(),
            reference_u.len(),
            reference_v.len(),
            p_y.len(),
            p_u.len(),
            p_v.len(),
        ],
    )
}

#[derive(Clone, Copy)]
enum PartitionLayout {
    Uniform(usize, usize),
    P8x8,
}

fn local_partition_layout(layout: PartitionLayout) -> Option<Vec<(usize, usize, usize, usize)>> {
    match layout {
        PartitionLayout::Uniform(width, height) => Some(
            (0..16 / height)
                .flat_map(|row| {
                    (0..16 / width).map(move |column| (column * width, row * height, width, height))
                })
                .collect(),
        ),
        PartitionLayout::P8x8 => None,
    }
}

fn validate_mixed_p_partitions(
    partitions: &[DecodedMotionPartition],
    macroblocks: usize,
    mb_width: usize,
) -> Result<(), String> {
    let mut index = 0;
    let mut types_seen = [false; 4];
    for macroblock in 0..macroblocks {
        let mut coverage = [false; 16];
        while index < partitions.len() && partitions[index].macroblock == macroblock {
            let partition = partitions[index];
            let local_x = partition.x - (macroblock % mb_width) * 16;
            let local_y = partition.y - (macroblock / mb_width) * 16;
            let type_index = match (partition.width, partition.height) {
                (16, 16) | (16, 8) | (8, 16) => None,
                (8, 8) => Some(0),
                (8, 4) => Some(1),
                (4, 8) => Some(2),
                (4, 4) => Some(3),
                _ => {
                    return Err(format!(
                        "unexpected P partition {}x{}",
                        partition.width, partition.height
                    ));
                }
            };
            if let Some(type_index) = type_index {
                types_seen[type_index] = true;
            }
            for block_y in local_y / 4..(local_y + partition.height) / 4 {
                for block_x in local_x / 4..(local_x + partition.width) / 4 {
                    let block = block_y * 4 + block_x;
                    if coverage[block] {
                        return Err(format!("P macroblock {macroblock} overlaps block {block}"));
                    }
                    coverage[block] = true;
                }
            }
            index += 1;
        }
        if coverage.iter().any(|covered| !covered) {
            return Err(format!(
                "P macroblock {macroblock} does not cover 16 blocks"
            ));
        }
    }
    if index != partitions.len() {
        return Err("P partitions out of macroblock order".into());
    }
    if types_seen.iter().any(|seen| !seen) {
        return Err(format!(
            "P_8x8 fixture covered only sub-types {types_seen:?}"
        ));
    }
    Ok(())
}

fn compare_cabac_p16_motion_fixture(
    stream: &[u8],
    expected: &[u8],
    requires_fractional_motion: bool,
    layout: PartitionLayout,
    expected_weights: Option<(bool, bool)>,
    deblock_enabled: bool,
) -> Result<(), String> {
    let nals = split_annex_b(stream);
    let sps = parse_sps(
        &nals
            .iter()
            .find(|nal| nal.unit_type == 7)
            .ok_or("missing SPS")?
            .rbsp,
    )
    .map_err(|error| error.to_string())?;
    let pps = parse_pps(
        &nals
            .iter()
            .find(|nal| nal.unit_type == 8)
            .ok_or("missing PPS")?
            .rbsp,
    )
    .map_err(|error| error.to_string())?;
    let slice_nals: Vec<_> = nals.iter().filter(|nal| nal.is_vcl()).collect();
    if slice_nals.len() != 2 {
        return Err(format!("expected IDR+P, got {} slices", slice_nals.len()));
    }
    let headers: Vec<_> = slice_nals
        .iter()
        .map(|nal| {
            parse_slice_header(&nal.rbsp, &sps, &pps, nal.unit_type, nal.ref_idc)
                .map_err(|error| error.to_string())
        })
        .collect::<Result<_, _>>()?;
    if !headers[0].is_i() || !headers[1].is_p() {
        return Err("the integer-MV fixture must contain IDR+P".into());
    }
    let expected_deblock_idc = if deblock_enabled { 0 } else { 1 };
    if headers[0].deblocking_disable_idc != expected_deblock_idc
        || headers[1].deblocking_disable_idc != expected_deblock_idc
    {
        return Err(format!(
            "unexpected deblocking idc {}, {}",
            headers[0].deblocking_disable_idc, headers[1].deblocking_disable_idc
        ));
    }
    if let Some((expected_luma, expected_chroma)) = expected_weights {
        let weights = headers[1]
            .weights
            .as_ref()
            .ok_or("weighted P fixture has no pred_weight_table")?;
        let entry = weights
            .l0
            .first()
            .ok_or("weighted P fixture has no L0 entry")?;
        if headers[1].data_offset_bits != 72 {
            return Err(format!(
                "weighted P CABAC starts at {} bits, expected 72",
                headers[1].data_offset_bits
            ));
        }
        if entry.luma_flag != expected_luma || entry.chroma_flag != expected_chroma {
            return Err(format!(
                "unexpected weighted flags luma={} chroma={}",
                entry.luma_flag, entry.chroma_flag
            ));
        }
    }

    let width = sps.coded_width as usize;
    let height = sps.coded_height as usize;
    let cw = width / 2;
    let ch = height / 2;
    let y_len = width * height;
    let chroma_len = cw * ch;
    let frame_len = y_len + 2 * chroma_len;
    if expected.len() != frame_len * 2 {
        return Err(format!(
            "expected {frame_len} bytes per frame, got {} total",
            expected.len()
        ));
    }
    let reference_y: Vec<u16> = expected[..y_len]
        .iter()
        .map(|&sample| u16::from(sample))
        .collect();
    let reference_u: Vec<u16> = expected[y_len..y_len + chroma_len]
        .iter()
        .map(|&sample| u16::from(sample))
        .collect();
    let reference_v: Vec<u16> = expected[y_len + chroma_len..frame_len]
        .iter()
        .map(|&sample| u16::from(sample))
        .collect();
    let mut p_y = vec![0u16; y_len];
    let mut p_u = vec![0u16; chroma_len];
    let mut p_v = vec![0u16; chroma_len];
    let p = decode_cabac_p_slice(
        &slice_nals[1].rbsp,
        &headers[1],
        &sps,
        &pps,
        &reference_y,
        &reference_u,
        &reference_v,
        &mut p_y,
        &mut p_u,
        &mut p_v,
    )
    .map_err(|error| error.to_string())?;
    if p.macroblocks != (width / 16) * (height / 16) {
        return Err(format!("P slice decoded {} macroblocks", p.macroblocks));
    }
    let mb_width = width / 16;
    if let Some(local_partitions) = local_partition_layout(layout) {
        let partitions_per_macroblock = local_partitions.len();
        let expected_partition_count = p.macroblocks * partitions_per_macroblock;
        if p.motion_partitions.len() != expected_partition_count {
            return Err(format!(
                "P slice decoded {} partitions for {} macroblocks, expected {expected_partition_count}",
                p.motion_partitions.len(),
                p.macroblocks
            ));
        }
        for (index, partition) in p.motion_partitions.iter().enumerate() {
            let macroblock = index / partitions_per_macroblock;
            let within = index % partitions_per_macroblock;
            let (local_x, local_y, partition_width, partition_height) = local_partitions[within];
            let expected_x = (macroblock % mb_width) * 16 + local_x;
            let expected_y = (macroblock / mb_width) * 16 + local_y;
            if partition.macroblock != macroblock
                || partition.x != expected_x
                || partition.y != expected_y
                || partition.width != partition_width
                || partition.height != partition_height
            {
                return Err(format!(
                    "unexpected partition {index}: mb={} x={} y={} {}x{}",
                    partition.macroblock,
                    partition.x,
                    partition.y,
                    partition.width,
                    partition.height
                ));
            }
        }
    } else {
        validate_mixed_p_partitions(&p.motion_partitions, p.macroblocks, mb_width)
            .map_err(|error| format!("{error}; partitions={:?}", p.motion_partitions))?;
    }
    if p.motion_partitions
        .iter()
        .all(|partition| partition.motion_vector == [0, 0])
    {
        return Err("integer-motion fixture produced only zero vectors".into());
    }
    if requires_fractional_motion
        && p.motion_partitions.iter().all(|partition| {
            partition.motion_vector[0] % 4 == 0 && partition.motion_vector[1] % 4 == 0
        })
    {
        return Err("fractional-motion fixture produced only integer luma vectors".into());
    }
    if deblock_enabled {
        let mut luma_plane = PlaneMut {
            data: &mut p_y,
            width,
            height,
            stride: width,
        };
        let mut cb_plane = PlaneMut {
            data: &mut p_u,
            width: cw,
            height: ch,
            stride: cw,
        };
        let mut cr_plane = PlaneMut {
            data: &mut p_v,
            width: cw,
            height: ch,
            stride: cw,
        };
        deblock_p_picture(
            &mut luma_plane,
            Some(&mut cb_plane),
            Some(&mut cr_plane),
            &p.deblock,
            sps.bit_depth_luma as u8,
            headers[1].deblocking_alpha * 2,
            headers[1].deblocking_beta * 2,
            [pps.chroma_qp_offset, pps.second_chroma_qp_offset],
        )
        .map_err(|error| error.to_string())?;
    }

    let mut actual = Vec::with_capacity(frame_len);
    for plane in [&p_y, &p_u, &p_v] {
        actual.extend(plane.iter().map(|&sample| sample as u8));
    }
    compare_samples(
        &actual,
        &expected[frame_len..],
        &[y_len, chroma_len, chroma_len],
    )
    .map_err(|error| {
        let expected_y = &expected[frame_len..frame_len + y_len];
        let mut regions = [[0usize; 6]; 4];
        for row in 0..height {
            for column in 0..width {
                if p_y[row * width + column] as u8 != expected_y[row * width + column] {
                    regions[row / 8][column / 8] += 1;
                }
            }
        }
        format!(
            "{error}; 8x8_differences={regions:?}; partitions={:?}",
            p.motion_partitions
        )
    })
}

fn compare_cabac_p16_multiref_fixture(stream: &[u8], expected: &[u8]) -> Result<(), String> {
    let nals = split_annex_b(stream);
    let sps = parse_sps(
        &nals
            .iter()
            .find(|nal| nal.unit_type == 7)
            .ok_or("missing SPS")?
            .rbsp,
    )
    .map_err(|error| error.to_string())?;
    let pps = parse_pps(
        &nals
            .iter()
            .find(|nal| nal.unit_type == 8)
            .ok_or("missing PPS")?
            .rbsp,
    )
    .map_err(|error| error.to_string())?;
    let slice_nals: Vec<_> = nals.iter().filter(|nal| nal.is_vcl()).collect();
    if slice_nals.len() != 3 {
        return Err(format!("expected IDR+P+P, got {} slices", slice_nals.len()));
    }
    let headers: Vec<_> = slice_nals
        .iter()
        .map(|nal| {
            parse_slice_header(&nal.rbsp, &sps, &pps, nal.unit_type, nal.ref_idc)
                .map_err(|error| error.to_string())
        })
        .collect::<Result<_, _>>()?;
    if !headers[0].is_i() || !headers[1].is_p() || !headers[2].is_p() {
        return Err("expected IDR+P+P slice types".into());
    }
    if headers[2].ref_idx_l0_active != 2 {
        return Err(format!(
            "expected two active references, got {}",
            headers[2].ref_idx_l0_active
        ));
    }

    let width = sps.coded_width as usize;
    let height = sps.coded_height as usize;
    let cw = width / 2;
    let ch = height / 2;
    let y_len = width * height;
    let chroma_len = cw * ch;
    let frame_len = y_len + 2 * chroma_len;
    if expected.len() != frame_len * 3 {
        return Err(format!(
            "expected {frame_len} bytes per frame, got {} total",
            expected.len()
        ));
    }
    let plane_u16 = |frame: &[u8], start: usize, len: usize| -> Vec<u16> {
        frame[start..start + len]
            .iter()
            .map(|&sample| u16::from(sample))
            .collect()
    };
    let frame0 = &expected[..frame_len];
    let frame1 = &expected[frame_len..2 * frame_len];
    let frame2 = &expected[2 * frame_len..];
    let reference0_y = plane_u16(frame1, 0, y_len);
    let reference0_u = plane_u16(frame1, y_len, chroma_len);
    let reference0_v = plane_u16(frame1, y_len + chroma_len, chroma_len);
    let reference1_y = plane_u16(frame0, 0, y_len);
    let reference1_u = plane_u16(frame0, y_len, chroma_len);
    let reference1_v = plane_u16(frame0, y_len + chroma_len, chroma_len);
    let references = [
        CabacReferenceFrame {
            luma: &reference0_y,
            chroma_cb: &reference0_u,
            chroma_cr: &reference0_v,
            picture_order_count: 0,
            motion: None,
        },
        CabacReferenceFrame {
            luma: &reference1_y,
            chroma_cb: &reference1_u,
            chroma_cr: &reference1_v,
            picture_order_count: 1,
            motion: None,
        },
    ];
    let mut p_y = vec![0u16; y_len];
    let mut p_u = vec![0u16; chroma_len];
    let mut p_v = vec![0u16; chroma_len];
    let decoded = decode_cabac_p_multi_ref_slice(
        &slice_nals[2].rbsp,
        &headers[2],
        &sps,
        &pps,
        &references,
        &mut p_y,
        &mut p_u,
        &mut p_v,
    )
    .map_err(|error| error.to_string())?;
    if decoded.macroblocks != (width / 16) * (height / 16) {
        return Err(format!(
            "P slice decoded {} macroblocks",
            decoded.macroblocks
        ));
    }
    let reference_pattern: Vec<u8> = decoded
        .motion_partitions
        .iter()
        .map(|partition| partition.reference_index)
        .collect();
    if reference_pattern != [1, 0, 0, 0, 1, 0] {
        return Err(format!(
            "unexpected P reference pattern {reference_pattern:?}"
        ));
    }

    let mut actual = Vec::with_capacity(frame_len);
    for plane in [&p_y, &p_u, &p_v] {
        actual.extend(plane.iter().map(|&sample| sample as u8));
    }
    compare_samples(&actual, frame2, &[y_len, chroma_len, chroma_len])
}

fn compare_cabac_b16_fixture(
    stream: &[u8],
    expected: &[u8],
    expected_prediction_list: Option<u8>,
) -> Result<(), String> {
    compare_cabac_b16_fixture_inner(stream, expected, expected_prediction_list, false)
}

fn compare_cabac_b16_fixture_with_deblock(
    stream: &[u8],
    expected: &[u8],
    expected_prediction_list: Option<u8>,
) -> Result<(), String> {
    compare_cabac_b16_fixture_inner(stream, expected, expected_prediction_list, true)
}

fn compare_cabac_b16_fixture_inner(
    stream: &[u8],
    expected: &[u8],
    expected_prediction_list: Option<u8>,
    apply_deblock: bool,
) -> Result<(), String> {
    let nals = split_annex_b(stream);
    let sps = parse_sps(
        &nals
            .iter()
            .find(|nal| nal.unit_type == 7)
            .ok_or("missing SPS")?
            .rbsp,
    )
    .map_err(|error| error.to_string())?;
    let pps = parse_pps(
        &nals
            .iter()
            .find(|nal| nal.unit_type == 8)
            .ok_or("missing PPS")?
            .rbsp,
    )
    .map_err(|error| error.to_string())?;
    let slice_nals: Vec<_> = nals.iter().filter(|nal| nal.is_vcl()).collect();
    if slice_nals.len() != 3 {
        return Err(format!("expected IDR+P+B, got {} slices", slice_nals.len()));
    }
    let headers: Vec<_> = slice_nals
        .iter()
        .map(|nal| {
            parse_slice_header(&nal.rbsp, &sps, &pps, nal.unit_type, nal.ref_idc)
                .map_err(|error| error.to_string())
        })
        .collect::<Result<_, _>>()?;
    let b_index = headers
        .iter()
        .position(|header| header.is_b())
        .ok_or("missing B slice")?;
    let p_index = headers
        .iter()
        .position(|header| header.is_p())
        .ok_or("missing P slice")?;
    if headers.iter().filter(|header| header.is_b()).count() != 1 {
        return Err("expected exactly one B slice".into());
    }

    let width = sps.coded_width as usize;
    let height = sps.coded_height as usize;
    let cw = width / 2;
    let ch = height / 2;
    let y_len = width * height;
    let chroma_len = cw * ch;
    let frame_len = y_len + 2 * chroma_len;
    if expected.len() != frame_len * 3 {
        return Err(format!(
            "expected {frame_len} bytes per frame, got {} total",
            expected.len()
        ));
    }
    let plane_u16 = |frame: &[u8], start: usize, len: usize| -> Vec<u16> {
        frame[start..start + len]
            .iter()
            .map(|&sample| u16::from(sample))
            .collect()
    };
    let frame0 = &expected[..frame_len];
    let frame1 = &expected[frame_len..2 * frame_len];
    let frame2 = &expected[2 * frame_len..];
    let l0_y = plane_u16(frame0, 0, y_len);
    let l0_u = plane_u16(frame0, y_len, chroma_len);
    let l0_v = plane_u16(frame0, y_len + chroma_len, chroma_len);
    let l1_y = plane_u16(frame2, 0, y_len);
    let l1_u = plane_u16(frame2, y_len, chroma_len);
    let l1_v = plane_u16(frame2, y_len + chroma_len, chroma_len);
    let references_l0 = [CabacReferenceFrame {
        luma: &l0_y,
        chroma_cb: &l0_u,
        chroma_cr: &l0_v,
        picture_order_count: headers[0].poc_lsb.unwrap_or(0) as i32,
        motion: None,
    }];
    let mut p_y = vec![0u16; y_len];
    let mut p_u = vec![0u16; chroma_len];
    let mut p_v = vec![0u16; chroma_len];
    let p_decoded = decode_cabac_p_multi_ref_slice(
        &slice_nals[p_index].rbsp,
        &headers[p_index],
        &sps,
        &pps,
        &references_l0,
        &mut p_y,
        &mut p_u,
        &mut p_v,
    )
    .map_err(|error| error.to_string())?;
    let l1_motion = CabacReferenceMotionFrame { slice: &p_decoded };
    let references_l1 = [CabacReferenceFrame {
        luma: &l1_y,
        chroma_cb: &l1_u,
        chroma_cr: &l1_v,
        picture_order_count: headers[p_index].poc_lsb.unwrap_or(0) as i32,
        motion: Some(l1_motion),
    }];
    let mut b_y = vec![0u16; y_len];
    let mut b_u = vec![0u16; chroma_len];
    let mut b_v = vec![0u16; chroma_len];
    let decoded = decode_cabac_b16_slice(
        &slice_nals[b_index].rbsp,
        &headers[b_index],
        &sps,
        &pps,
        headers[b_index].poc_lsb.unwrap_or(0) as i32,
        &references_l0,
        &references_l1,
        &mut b_y,
        &mut b_u,
        &mut b_v,
    )
    .map_err(|error| error.to_string())?;
    if decoded.macroblocks != (width / 16) * (height / 16) {
        return Err(format!(
            "B slice decoded {} macroblocks",
            decoded.macroblocks
        ));
    }
    if let Some(expected_prediction_list) = expected_prediction_list {
        if decoded.motion_partitions.iter().any(|partition| {
            partition.prediction_list != expected_prediction_list || partition.reference_index != 0
        }) {
            return Err(format!(
                "B fixture did not decode as prediction list {expected_prediction_list}"
            ));
        }
    }
    if apply_deblock {
        if !pps.deblocking_filter_control || headers[b_index].deblocking_disable_idc == 1 {
            return Err("B deblock fixture does not enable deblocking".into());
        }
        let mut yp = PlaneMut {
            data: &mut b_y,
            width,
            height,
            stride: width,
        };
        let mut up = PlaneMut {
            data: &mut b_u,
            width: cw,
            height: ch,
            stride: cw,
        };
        let mut vp = PlaneMut {
            data: &mut b_v,
            width: cw,
            height: ch,
            stride: cw,
        };
        deblock_b_picture(
            &mut yp,
            Some(&mut up),
            Some(&mut vp),
            &decoded.deblock,
            sps.bit_depth_luma as u8,
            headers[b_index].deblocking_alpha * 2,
            headers[b_index].deblocking_beta * 2,
            [pps.chroma_qp_offset, pps.second_chroma_qp_offset],
        )
        .map_err(str::to_owned)?;
    }
    let mut actual = Vec::with_capacity(frame_len);
    for plane in [&b_y, &b_u, &b_v] {
        actual.extend(plane.iter().map(|&sample| sample as u8));
    }
    compare_samples(&actual, frame1, &[y_len, chroma_len, chroma_len])
}

fn compare_fixture(stream: &[u8], expected: &[u8]) -> Result<(), String> {
    compare_fixture_planes(stream, expected, true)
}

fn compare_fixture_planes(
    stream: &[u8],
    expected: &[u8],
    compare_chroma: bool,
) -> Result<(), String> {
    let nals = split_annex_b(stream);
    let sps = parse_sps(
        &nals
            .iter()
            .find(|nal| nal.unit_type == 7)
            .ok_or("missing SPS")?
            .rbsp,
    )
    .map_err(|error| error.to_string())?;
    let pps = parse_pps(
        &nals
            .iter()
            .find(|nal| nal.unit_type == 8)
            .ok_or("missing PPS")?
            .rbsp,
    )
    .map_err(|error| error.to_string())?;
    let slice_nal = nals
        .iter()
        .find(|nal| nal.is_vcl())
        .ok_or("missing slice")?;
    let slice = parse_slice_header(
        &slice_nal.rbsp,
        &sps,
        &pps,
        slice_nal.unit_type,
        slice_nal.ref_idc,
    )
    .map_err(|error| error.to_string())?;

    let width = sps.coded_width as usize;
    let height = sps.coded_height as usize;
    let cw = width / 2;
    let ch = height / 2;
    let mut y = vec![0u16; width * height];
    let mut u = vec![0u16; cw * ch];
    let mut v = vec![0u16; cw * ch];
    let decoded = decode_i_slice(
        &slice_nal.rbsp,
        &slice,
        &sps,
        &pps,
        &mut y,
        Some(&mut u),
        Some(&mut v),
    )
    .map_err(|error| error.to_string())?;
    let sample_count = if compare_chroma {
        y.len() + u.len() + v.len()
    } else {
        y.len()
    };
    let sample_bytes = if compare_chroma {
        let bytes = expected.len() / sample_count;
        if !matches!(bytes, 1 | 2) || expected.len() != sample_count * bytes {
            return Err(format!(
                "unexpected expected size {} for {sample_count} samples",
                expected.len()
            ));
        }
        bytes
    } else if expected.len() == y.len() * 2 {
        2
    } else {
        1
    };

    if pps.deblocking_filter_control && slice.deblocking_disable_idc != 1 {
        let mut yp = PlaneMut {
            data: &mut y,
            width,
            height,
            stride: width,
        };
        let mut up = PlaneMut {
            data: &mut u,
            width: cw,
            height: ch,
            stride: cw,
        };
        let mut vp = PlaneMut {
            data: &mut v,
            width: cw,
            height: ch,
            stride: cw,
        };
        deblock_i_picture(
            &mut yp,
            Some(&mut up),
            Some(&mut vp),
            &decoded.deblock,
            sps.bit_depth_luma as u8,
            slice.deblocking_alpha * 2,
            slice.deblocking_beta * 2,
            [pps.chroma_qp_offset, pps.second_chroma_qp_offset],
        )
        .map_err(str::to_owned)?;
    }

    let mut actual = Vec::with_capacity(expected.len());
    extend_samples(&mut actual, &y, sample_bytes);
    if compare_chroma {
        extend_samples(&mut actual, &u, sample_bytes);
        extend_samples(&mut actual, &v, sample_bytes);
    }
    let planes: &[usize] = if compare_chroma {
        &[
            y.len() * sample_bytes,
            u.len() * sample_bytes,
            v.len() * sample_bytes,
        ]
    } else {
        &[y.len() * sample_bytes]
    };
    compare_samples(&actual, expected, planes)
}

fn extend_samples(output: &mut Vec<u8>, samples: &[u16], bytes_per_sample: usize) {
    for &sample in samples {
        if bytes_per_sample == 2 {
            output.extend_from_slice(&sample.to_le_bytes());
        } else {
            output.push(sample as u8);
        }
    }
}

fn compare_samples(actual: &[u8], expected: &[u8], planes: &[usize]) -> Result<(), String> {
    let differences: Vec<_> = actual
        .iter()
        .zip(expected)
        .enumerate()
        .filter(|(_, (actual, expected))| actual != expected)
        .collect();
    if differences.is_empty() {
        return Ok(());
    }
    let plane_differences: Vec<_> = planes
        .iter()
        .scan(0usize, |offset, &length| {
            let start = *offset;
            *offset += length;
            Some(
                actual[start..*offset]
                    .iter()
                    .zip(&expected[start..*offset])
                    .enumerate()
                    .filter(|(_, (actual, expected))| actual != expected)
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    Err(format!(
        "{} differing samples; planes: {:?}; first: {:?}",
        differences.len(),
        plane_differences
            .iter()
            .map(|differences| (differences.len(), differences.first()))
            .collect::<Vec<_>>(),
        differences.first()
    ))
}
