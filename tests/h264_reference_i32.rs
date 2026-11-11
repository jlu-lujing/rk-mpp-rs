use rk_mpp_rs::codec::h264::parser::{parse_pps, parse_slice_header, parse_sps, split_annex_b};
use rk_mpp_rs::reference::h264_cabac::{
    decode_cabac_i_picture_slices, decode_cabac_p16_slice, decode_cabac_p_skip_slice,
};
use rk_mpp_rs::reference::h264_deblock::deblock_i_picture;
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
    let p = decode_cabac_p16_slice(
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
    if p.motion_vectors.iter().any(|motion| *motion != [0, 0]) {
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

fn compare_cabac_p16_motion_fixture(stream: &[u8], expected: &[u8]) -> Result<(), String> {
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
    if headers[0].deblocking_disable_idc != 1 || headers[1].deblocking_disable_idc != 1 {
        return Err("the integer-MV fixture must disable deblocking".into());
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
    let p = decode_cabac_p16_slice(
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
    if p.motion_vectors.iter().all(|motion| *motion == [0, 0]) {
        return Err("integer-motion fixture produced only zero vectors".into());
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
