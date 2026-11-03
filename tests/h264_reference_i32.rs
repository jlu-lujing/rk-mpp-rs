use rk_mpp_rs::codec::h264::parser::{parse_pps, parse_slice_header, parse_sps, split_annex_b};
use rk_mpp_rs::reference::h264_cabac::decode_cabac_i_picture_slices;
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
            8,
            slice.deblocking_alpha * 2,
            slice.deblocking_beta * 2,
            [pps.chroma_qp_offset, pps.second_chroma_qp_offset],
        )
        .map_err(str::to_owned)?;
    }

    let mut actual = Vec::with_capacity(expected.len());
    actual.extend(y.iter().map(|&sample| sample as u8));
    if compare_chroma {
        actual.extend(u.iter().map(|&sample| sample as u8));
        actual.extend(v.iter().map(|&sample| sample as u8));
    }
    let planes: &[usize] = if compare_chroma {
        &[y.len(), u.len(), v.len()]
    } else {
        &[y.len()]
    };
    compare_samples(&actual, expected, planes)
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
