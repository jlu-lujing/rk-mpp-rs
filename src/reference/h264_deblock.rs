//! H.264 deblocking primitives.
//!
//! Edge traversal and alpha/beta/tc table selection are separate from the
//! arithmetic filters. Lines use `p3,p2,p1,p0,q0,q1,q2,q3` order and `u16`
//! samples, so the same kernels support 8- and 10-bit planes.

use super::h264::chroma_qp;
use super::h264_recon::PlaneMut;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeblockMap {
    pub mb_width: usize,
    pub mb_height: usize,
    pub decoded: Vec<bool>,
    pub qp: Vec<u8>,
    pub luma_non_zero: Vec<[u8; 16]>,
    /// Cb/Cr each store 4 or 8 block counts per macroblock.
    pub chroma_non_zero: [Vec<Vec<u8>>; 2],
    /// Per-4x4 quarter-pixel MVs for P slices.
    pub motion_vectors: Option<Vec<[[i32; 2]; 16]>>,
}

#[derive(Clone, Copy)]
enum SliceDeblockKind {
    I,
    P,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeblockParameters {
    pub alpha: i32,
    pub beta: i32,
    pub tc0: [i32; 4],
}

pub fn parameters(
    qp: u8,
    alpha_offset_div2: i32,
    beta_offset_div2: i32,
    bit_depth: u8,
) -> Result<DeblockParameters, &'static str> {
    if !(8..=10).contains(&bit_depth) {
        return Err("H.264 deblocking supports 8..=10 bit");
    }
    let depth_offset = 6 * i32::from(bit_depth - 8);
    let alpha_index = (i32::from(qp) + alpha_offset_div2 * 2 - depth_offset).clamp(0, 51) as usize;
    let beta_index = (i32::from(qp) + beta_offset_div2 * 2 - depth_offset).clamp(0, 51) as usize;
    let shift = bit_depth - 8;
    Ok(DeblockParameters {
        alpha: i32::from(ALPHA[alpha_index]) << shift,
        beta: i32::from(BETA[beta_index]) << shift,
        tc0: TC0[alpha_index].map(|value| {
            if value < 0 {
                i32::from(value)
            } else {
                i32::from(value) << shift
            }
        }),
    })
}

/// I-slice boundary strength before motion-vector rules are needed.
pub fn i_boundary_strength(
    macroblock_edge: bool,
    p_has_coefficients: bool,
    q_has_coefficients: bool,
) -> u8 {
    if macroblock_edge {
        4
    } else if p_has_coefficients || q_has_coefficients {
        3
    } else {
        0
    }
}

pub fn p_boundary_strength(
    p_has_coefficients: bool,
    q_has_coefficients: bool,
    p_motion: [i32; 2],
    q_motion: [i32; 2],
) -> u8 {
    if p_has_coefficients || q_has_coefficients {
        3
    } else if (p_motion[0] - q_motion[0]).abs() >= 4 || (p_motion[1] - q_motion[1]).abs() >= 4 {
        2
    } else if p_motion != q_motion {
        1
    } else {
        0
    }
}

/// Apply the I-slice deblocking pass to tightly related picture planes.
pub fn deblock_i_picture(
    luma: &mut PlaneMut<'_>,
    chroma_cb: Option<&mut PlaneMut<'_>>,
    chroma_cr: Option<&mut PlaneMut<'_>>,
    map: &DeblockMap,
    bit_depth: u8,
    alpha_offset_div2: i32,
    beta_offset_div2: i32,
    chroma_qp_offset: [i32; 2],
) -> Result<(), &'static str> {
    deblock_picture(
        luma,
        chroma_cb,
        chroma_cr,
        map,
        bit_depth,
        alpha_offset_div2,
        beta_offset_div2,
        chroma_qp_offset,
        SliceDeblockKind::I,
    )
}

/// Apply the P-slice deblocking pass using coded-block and 4x4 MV strength.
pub fn deblock_p_picture(
    luma: &mut PlaneMut<'_>,
    chroma_cb: Option<&mut PlaneMut<'_>>,
    chroma_cr: Option<&mut PlaneMut<'_>>,
    map: &DeblockMap,
    bit_depth: u8,
    alpha_offset_div2: i32,
    beta_offset_div2: i32,
    chroma_qp_offset: [i32; 2],
) -> Result<(), &'static str> {
    if map.motion_vectors.is_none() {
        return Err("P deblocking requires motion vectors");
    }
    deblock_picture(
        luma,
        chroma_cb,
        chroma_cr,
        map,
        bit_depth,
        alpha_offset_div2,
        beta_offset_div2,
        chroma_qp_offset,
        SliceDeblockKind::P,
    )
}

#[allow(clippy::too_many_arguments)]
fn deblock_picture(
    luma: &mut PlaneMut<'_>,
    chroma_cb: Option<&mut PlaneMut<'_>>,
    chroma_cr: Option<&mut PlaneMut<'_>>,
    map: &DeblockMap,
    bit_depth: u8,
    alpha_offset_div2: i32,
    beta_offset_div2: i32,
    chroma_qp_offset: [i32; 2],
    kind: SliceDeblockKind,
) -> Result<(), &'static str> {
    let mut chroma_cb = chroma_cb;
    let mut chroma_cr = chroma_cr;
    for mb_y in 0..map.mb_height {
        for mb_x in 0..map.mb_width {
            let index = mb_y * map.mb_width + mb_x;
            if !map.decoded[index] {
                continue;
            }
            let x = mb_x * 16;
            let y = mb_y * 16;
            deblock_luma_macroblock(
                luma,
                map,
                index,
                x,
                y,
                bit_depth,
                alpha_offset_div2,
                beta_offset_div2,
                kind,
            );
            if let Some(chroma) = chroma_cb.as_deref_mut() {
                deblock_chroma_macroblock(
                    chroma,
                    map,
                    0,
                    index,
                    x / 2,
                    y / 2,
                    bit_depth,
                    alpha_offset_div2,
                    beta_offset_div2,
                    chroma_qp_offset[0],
                    kind,
                )?;
            }
            if let Some(chroma) = chroma_cr.as_deref_mut() {
                deblock_chroma_macroblock(
                    chroma,
                    map,
                    1,
                    index,
                    x / 2,
                    y / 2,
                    bit_depth,
                    alpha_offset_div2,
                    beta_offset_div2,
                    chroma_qp_offset[1],
                    kind,
                )?;
            }
        }
    }
    Ok(())
}

/// Ordinary luma edge filter, including the optional p1/q1 updates.
pub fn filter_luma(line: &mut [u16; 8], alpha: i32, beta: i32, tc0: i32, bit_depth: u8) -> bool {
    let p2 = i32::from(line[1]);
    let p1 = i32::from(line[2]);
    let p0 = i32::from(line[3]);
    let q0 = i32::from(line[4]);
    let q1 = i32::from(line[5]);
    let q2 = i32::from(line[6]);
    if (p0 - q0).abs() >= alpha || (p1 - p0).abs() >= beta || (q1 - q0).abs() >= beta {
        return false;
    }

    let mut tc = tc0;
    if (p2 - p0).abs() < beta {
        if tc0 != 0 {
            let value = ((p2 + ((p0 + q0 + 1) >> 1)) >> 1) - p1;
            line[2] = clip(p1 + value.clamp(-tc0, tc0), bit_depth);
        }
        tc += 1;
    }
    if (q2 - q0).abs() < beta {
        if tc0 != 0 {
            let value = ((q2 + ((p0 + q0 + 1) >> 1)) >> 1) - q1;
            line[5] = clip(q1 + value.clamp(-tc0, tc0), bit_depth);
        }
        tc += 1;
    }
    let delta = ((4 * (q0 - p0) + (p1 - q1) + 4) >> 3).clamp(-tc, tc);
    line[3] = clip(p0 + delta, bit_depth);
    line[4] = clip(q0 - delta, bit_depth);
    true
}

/// Strong intra luma edge filter.
pub fn filter_luma_intra(line: &mut [u16; 8], alpha: i32, beta: i32, bit_depth: u8) -> bool {
    let p3 = i32::from(line[0]);
    let p2 = i32::from(line[1]);
    let p1 = i32::from(line[2]);
    let p0 = i32::from(line[3]);
    let q0 = i32::from(line[4]);
    let q1 = i32::from(line[5]);
    let q2 = i32::from(line[6]);
    let q3 = i32::from(line[7]);
    if (p0 - q0).abs() >= alpha || (p1 - p0).abs() >= beta || (q1 - q0).abs() >= beta {
        return false;
    }

    if (p0 - q0).abs() < (alpha >> 2) + 2 {
        if (p2 - p0).abs() < beta {
            line[3] = clip((p2 + 2 * p1 + 2 * p0 + 2 * q0 + q1 + 4) >> 3, bit_depth);
            line[2] = clip((p2 + p1 + p0 + q0 + 2) >> 2, bit_depth);
            line[1] = clip((2 * p3 + 3 * p2 + p1 + p0 + q0 + 4) >> 3, bit_depth);
        } else {
            line[3] = clip((2 * p1 + p0 + q1 + 2) >> 2, bit_depth);
        }
        if (q2 - q0).abs() < beta {
            line[4] = clip((p1 + 2 * p0 + 2 * q0 + 2 * q1 + q2 + 4) >> 3, bit_depth);
            line[5] = clip((p0 + q0 + q1 + q2 + 2) >> 2, bit_depth);
            line[6] = clip((2 * q3 + 3 * q2 + q1 + q0 + p0 + 4) >> 3, bit_depth);
        } else {
            line[4] = clip((2 * q1 + q0 + p1 + 2) >> 2, bit_depth);
        }
    } else {
        line[3] = clip((2 * p1 + p0 + q1 + 2) >> 2, bit_depth);
        line[4] = clip((2 * q1 + q0 + p1 + 2) >> 2, bit_depth);
    }
    true
}

/// Ordinary chroma edge filter. The line is `p1,p0,q0,q1`.
pub fn filter_chroma(line: &mut [u16; 4], alpha: i32, beta: i32, tc: i32, bit_depth: u8) -> bool {
    let p1 = i32::from(line[0]);
    let p0 = i32::from(line[1]);
    let q0 = i32::from(line[2]);
    let q1 = i32::from(line[3]);
    if (p0 - q0).abs() >= alpha || (p1 - p0).abs() >= beta || (q1 - q0).abs() >= beta {
        return false;
    }
    let delta = ((4 * (q0 - p0) + (p1 - q1) + 4) >> 3).clamp(-tc, tc);
    line[1] = clip(p0 + delta, bit_depth);
    line[2] = clip(q0 - delta, bit_depth);
    true
}

/// Strong intra chroma edge filter.
pub fn filter_chroma_intra(line: &mut [u16; 4], alpha: i32, beta: i32, bit_depth: u8) -> bool {
    let p1 = i32::from(line[0]);
    let p0 = i32::from(line[1]);
    let q0 = i32::from(line[2]);
    let q1 = i32::from(line[3]);
    if (p0 - q0).abs() >= alpha || (p1 - p0).abs() >= beta || (q1 - q0).abs() >= beta {
        return false;
    }
    line[1] = clip((2 * p1 + p0 + q1 + 2) >> 2, bit_depth);
    line[2] = clip((2 * q1 + q0 + p1 + 2) >> 2, bit_depth);
    true
}

#[allow(clippy::too_many_arguments)]
fn deblock_luma_macroblock(
    plane: &mut PlaneMut<'_>,
    map: &DeblockMap,
    index: usize,
    x: usize,
    y: usize,
    bit_depth: u8,
    alpha_offset: i32,
    beta_offset: i32,
    kind: SliceDeblockKind,
) {
    for edge in [0usize, 4, 8, 12] {
        let (p_index, q_index, p_col, q_col, mb_edge) = if edge == 0 {
            if x == 0 {
                continue;
            }
            let left = index - 1;
            if !map.decoded[left] {
                continue;
            }
            (left, index, 3, 0, true)
        } else {
            (index, index, edge / 4 - 1, edge / 4, false)
        };
        for block_row in 0..4 {
            let p_has = map.luma_non_zero[p_index][block_row * 4 + p_col] != 0;
            let q_has = map.luma_non_zero[q_index][block_row * 4 + q_col] != 0;
            let bs = match kind {
                SliceDeblockKind::I => i_boundary_strength(mb_edge, p_has, q_has),
                SliceDeblockKind::P => {
                    let motion = map
                        .motion_vectors
                        .as_ref()
                        .expect("P deblock map validated");
                    p_boundary_strength(
                        p_has,
                        q_has,
                        motion[p_index][block_row * 4 + p_col],
                        motion[q_index][block_row * 4 + q_col],
                    )
                }
            };
            if bs == 0 {
                continue;
            }
            let qp = qp_average(map.qp[p_index], map.qp[q_index]);
            let params = match parameters(qp, alpha_offset, beta_offset, bit_depth) {
                Ok(params) => params,
                Err(_) => continue,
            };
            for row in 0..4 {
                filter_luma_vertical(
                    plane,
                    x + edge,
                    y + block_row * 4 + row,
                    params,
                    bs,
                    bit_depth,
                );
            }
        }
    }

    for edge in [0usize, 4, 8, 12] {
        let (p_index, q_index, p_row, q_row, mb_edge) = if edge == 0 {
            if y == 0 {
                continue;
            }
            let top = index - map.mb_width;
            if !map.decoded[top] {
                continue;
            }
            (top, index, 3, 0, true)
        } else {
            (index, index, edge / 4 - 1, edge / 4, false)
        };
        for block_column in 0..4 {
            let p_has = map.luma_non_zero[p_index][p_row * 4 + block_column] != 0;
            let q_has = map.luma_non_zero[q_index][q_row * 4 + block_column] != 0;
            let bs = match kind {
                SliceDeblockKind::I => i_boundary_strength(mb_edge, p_has, q_has),
                SliceDeblockKind::P => {
                    let motion = map
                        .motion_vectors
                        .as_ref()
                        .expect("P deblock map validated");
                    p_boundary_strength(
                        p_has,
                        q_has,
                        motion[p_index][p_row * 4 + block_column],
                        motion[q_index][q_row * 4 + block_column],
                    )
                }
            };
            if bs == 0 {
                continue;
            }
            let qp = qp_average(map.qp[p_index], map.qp[q_index]);
            let params = match parameters(qp, alpha_offset, beta_offset, bit_depth) {
                Ok(params) => params,
                Err(_) => continue,
            };
            for column in 0..4 {
                filter_luma_horizontal(
                    plane,
                    x + block_column * 4 + column,
                    y + edge,
                    params,
                    bs,
                    bit_depth,
                );
            }
        }
    }
}

fn filter_luma_vertical(
    plane: &mut PlaneMut<'_>,
    x: usize,
    y: usize,
    params: DeblockParameters,
    bs: u8,
    bit_depth: u8,
) {
    let mut line: [u16; 8] =
        std::array::from_fn(|index| plane.data[y * plane.stride + x - 4 + index]);
    let changed = if bs == 4 {
        filter_luma_intra(&mut line, params.alpha, params.beta, bit_depth)
    } else {
        filter_luma(
            &mut line,
            params.alpha,
            params.beta,
            params.tc0[bs as usize],
            bit_depth,
        )
    };
    if changed {
        for (index, value) in line.into_iter().enumerate() {
            plane.data[y * plane.stride + x - 4 + index] = value;
        }
    }
}

fn filter_luma_horizontal(
    plane: &mut PlaneMut<'_>,
    x: usize,
    y: usize,
    params: DeblockParameters,
    bs: u8,
    bit_depth: u8,
) {
    let mut line: [u16; 8] =
        std::array::from_fn(|index| plane.data[(y - 4 + index) * plane.stride + x]);
    let changed = if bs == 4 {
        filter_luma_intra(&mut line, params.alpha, params.beta, bit_depth)
    } else {
        filter_luma(
            &mut line,
            params.alpha,
            params.beta,
            params.tc0[bs as usize],
            bit_depth,
        )
    };
    if changed {
        for (index, value) in line.into_iter().enumerate() {
            plane.data[(y - 4 + index) * plane.stride + x] = value;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn deblock_chroma_macroblock(
    plane: &mut PlaneMut<'_>,
    map: &DeblockMap,
    plane_index: usize,
    index: usize,
    x: usize,
    y: usize,
    bit_depth: u8,
    alpha_offset: i32,
    beta_offset: i32,
    qp_offset: i32,
    kind: SliceDeblockKind,
) -> Result<(), &'static str> {
    let block_count = map.chroma_non_zero[plane_index][index].len();
    let width = 2usize;
    let height = block_count / width;
    if x > 0 && map.decoded[index - 1] {
        let left = index - 1;
        for block_row in 0..height {
            let p_has = map.chroma_non_zero[plane_index][left][block_row * width + 1] != 0;
            let q_has = map.chroma_non_zero[plane_index][index][block_row * width] != 0;
            let bs = match kind {
                SliceDeblockKind::I => i_boundary_strength(true, p_has, q_has),
                SliceDeblockKind::P => {
                    let motion = map
                        .motion_vectors
                        .as_ref()
                        .expect("P deblock map validated");
                    p_boundary_strength(
                        p_has,
                        q_has,
                        motion[left][block_row * 8 + 3],
                        motion[index][block_row * 8],
                    )
                }
            };
            if bs != 0 {
                let qp = chroma_qp_average(map.qp[left], map.qp[index], qp_offset, bit_depth)?;
                let params = parameters(qp, alpha_offset, beta_offset, bit_depth)?;
                for row in 0..4 {
                    filter_chroma_vertical(
                        plane,
                        x,
                        y + block_row * 4 + row,
                        params,
                        bs,
                        bit_depth,
                    );
                }
            }
        }
    }
    for block_row in 0..height {
        let p_has = map.chroma_non_zero[plane_index][index][block_row * width] != 0;
        let q_has = map.chroma_non_zero[plane_index][index][block_row * width + 1] != 0;
        let bs = match kind {
            SliceDeblockKind::I => i_boundary_strength(false, p_has, q_has),
            SliceDeblockKind::P => {
                let motion = map
                    .motion_vectors
                    .as_ref()
                    .expect("P deblock map validated");
                p_boundary_strength(
                    p_has,
                    q_has,
                    motion[index][block_row * 8 + 1],
                    motion[index][block_row * 8 + 2],
                )
            }
        };
        if bs != 0 {
            let params = parameters(
                chroma_qp_average(map.qp[index], map.qp[index], qp_offset, bit_depth)?,
                alpha_offset,
                beta_offset,
                bit_depth,
            )?;
            for row in 0..4 {
                filter_chroma_vertical(
                    plane,
                    x + 4,
                    y + block_row * 4 + row,
                    params,
                    bs,
                    bit_depth,
                );
            }
        }
    }

    for edge in 0..height {
        let edge_y = edge * 4;
        for block_column in 0..width {
            let (p_index, p_row, mb_edge) = if edge == 0 {
                if y == 0 {
                    continue;
                }
                let top = index - map.mb_width;
                if !map.decoded[top] {
                    continue;
                }
                (top, height - 1, true)
            } else {
                (index, edge - 1, false)
            };
            let q_row = edge;
            let p_has =
                map.chroma_non_zero[plane_index][p_index][p_row * width + block_column] != 0;
            let q_has = map.chroma_non_zero[plane_index][index][q_row * width + block_column] != 0;
            let bs = match kind {
                SliceDeblockKind::I => i_boundary_strength(mb_edge, p_has, q_has),
                SliceDeblockKind::P => {
                    let motion = map
                        .motion_vectors
                        .as_ref()
                        .expect("P deblock map validated");
                    let p_block_row = if mb_edge { 3 } else { edge * 2 - 1 };
                    let q_block_row = if mb_edge { 0 } else { edge * 2 };
                    let block_column = block_column * 2;
                    p_boundary_strength(
                        p_has,
                        q_has,
                        motion[p_index][p_block_row * 4 + block_column],
                        motion[index][q_block_row * 4 + block_column],
                    )
                }
            };
            if bs == 0 {
                continue;
            }
            let qp = chroma_qp_average(map.qp[p_index], map.qp[index], qp_offset, bit_depth)?;
            let params = parameters(qp, alpha_offset, beta_offset, bit_depth)?;
            for column in 0..4 {
                filter_chroma_horizontal(
                    plane,
                    x + block_column * 4 + column,
                    y + edge_y,
                    params,
                    bs,
                    bit_depth,
                );
            }
        }
    }
    Ok(())
}

fn filter_chroma_vertical(
    plane: &mut PlaneMut<'_>,
    x: usize,
    y: usize,
    params: DeblockParameters,
    bs: u8,
    bit_depth: u8,
) {
    let mut line: [u16; 4] =
        std::array::from_fn(|index| plane.data[y * plane.stride + x - 2 + index]);
    let changed = if bs == 4 {
        filter_chroma_intra(&mut line, params.alpha, params.beta, bit_depth)
    } else {
        filter_chroma(
            &mut line,
            params.alpha,
            params.beta,
            params.tc0[bs as usize].max(0) + 1,
            bit_depth,
        )
    };
    if changed {
        for (index, value) in line.into_iter().enumerate() {
            plane.data[y * plane.stride + x - 2 + index] = value;
        }
    }
}

fn filter_chroma_horizontal(
    plane: &mut PlaneMut<'_>,
    x: usize,
    y: usize,
    params: DeblockParameters,
    bs: u8,
    bit_depth: u8,
) {
    let mut line: [u16; 4] =
        std::array::from_fn(|index| plane.data[(y - 2 + index) * plane.stride + x]);
    let changed = if bs == 4 {
        filter_chroma_intra(&mut line, params.alpha, params.beta, bit_depth)
    } else {
        filter_chroma(
            &mut line,
            params.alpha,
            params.beta,
            params.tc0[bs as usize].max(0) + 1,
            bit_depth,
        )
    };
    if changed {
        for (index, value) in line.into_iter().enumerate() {
            plane.data[(y - 2 + index) * plane.stride + x] = value;
        }
    }
}

fn qp_average(a: u8, b: u8) -> u8 {
    ((u16::from(a) + u16::from(b) + 1) >> 1) as u8
}

fn chroma_qp_average(a: u8, b: u8, offset: i32, bit_depth: u8) -> Result<u8, &'static str> {
    let a = chroma_qp(i32::from(a), offset, bit_depth)?;
    let b = chroma_qp(i32::from(b), offset, bit_depth)?;
    Ok(qp_average(a, b))
}

fn clip(value: i32, bit_depth: u8) -> u16 {
    value.clamp(0, (1 << bit_depth) - 1) as u16
}

const ALPHA: [u8; 52] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4, 4, 5, 6, 7, 8, 9, 10, 12, 13, 15, 17, 20,
    22, 25, 28, 32, 36, 40, 45, 50, 56, 63, 71, 80, 90, 101, 113, 127, 144, 162, 182, 203, 226,
    255, 255,
];

const BETA: [u8; 52] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 6, 6, 7, 7, 8, 8,
    9, 9, 10, 10, 11, 11, 12, 12, 13, 13, 14, 14, 15, 15, 16, 16, 17, 17, 18, 18,
];

const TC0: [[i8; 4]; 52] = [
    [-1, 0, 0, 0],
    [-1, 0, 0, 0],
    [-1, 0, 0, 0],
    [-1, 0, 0, 0],
    [-1, 0, 0, 0],
    [-1, 0, 0, 0],
    [-1, 0, 0, 0],
    [-1, 0, 0, 0],
    [-1, 0, 0, 0],
    [-1, 0, 0, 0],
    [-1, 0, 0, 0],
    [-1, 0, 0, 0],
    [-1, 0, 0, 0],
    [-1, 0, 0, 0],
    [-1, 0, 0, 0],
    [-1, 0, 0, 0],
    [-1, 0, 0, 0],
    [-1, 0, 0, 1],
    [-1, 0, 0, 1],
    [-1, 0, 0, 1],
    [-1, 0, 0, 1],
    [-1, 0, 1, 1],
    [-1, 0, 1, 1],
    [-1, 1, 1, 1],
    [-1, 1, 1, 1],
    [-1, 1, 1, 1],
    [-1, 1, 1, 1],
    [-1, 1, 1, 2],
    [-1, 1, 1, 2],
    [-1, 1, 1, 2],
    [-1, 1, 1, 2],
    [-1, 1, 2, 3],
    [-1, 1, 2, 3],
    [-1, 2, 2, 3],
    [-1, 2, 2, 4],
    [-1, 2, 3, 4],
    [-1, 2, 3, 4],
    [-1, 3, 3, 5],
    [-1, 3, 4, 6],
    [-1, 3, 4, 6],
    [-1, 4, 5, 7],
    [-1, 4, 5, 8],
    [-1, 4, 6, 9],
    [-1, 5, 7, 10],
    [-1, 6, 8, 11],
    [-1, 6, 8, 13],
    [-1, 7, 10, 14],
    [-1, 8, 11, 16],
    [-1, 9, 12, 18],
    [-1, 10, 13, 20],
    [-1, 11, 15, 23],
    [-1, 13, 17, 25],
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_luma_filter_updates_p0_and_q0_with_the_rounded_delta() {
        let mut line = [100, 110, 120, 130, 140, 150, 160, 170];
        assert!(filter_luma(&mut line, 255, 18, 4, 8));
        assert_eq!(line, [100, 110, 120, 131, 139, 150, 160, 170]);
    }

    #[test]
    fn strong_luma_filter_handles_p1_and_q1() {
        let mut line = [80, 110, 120, 130, 145, 160, 175, 190];
        assert!(filter_luma_intra(&mut line, 255, 40, 8));
        assert_eq!(line[1..7], [111, 126, 133, 146, 153, 168]);
    }

    #[test]
    fn chroma_filters_use_the_same_rounding_pattern() {
        let mut line = [120, 130, 140, 150];
        assert!(filter_chroma(&mut line, 255, 18, 2, 8));
        assert_eq!(line, [120, 131, 139, 150]);

        let mut line = [120, 130, 140, 150];
        assert!(filter_chroma_intra(&mut line, 255, 22, 8));
        assert_eq!(line, [120, 130, 140, 150]);
    }

    #[test]
    fn boundary_strength_distinguishes_macroblock_and_internal_edges() {
        assert_eq!(i_boundary_strength(true, false, false), 4);
        assert_eq!(i_boundary_strength(false, true, false), 3);
        assert_eq!(i_boundary_strength(false, false, true), 3);
        assert_eq!(i_boundary_strength(false, false, false), 0);
    }

    #[test]
    fn p_boundary_strength_uses_coefficients_and_motion_delta() {
        assert_eq!(p_boundary_strength(true, false, [0, 0], [0, 0]), 3);
        assert_eq!(p_boundary_strength(false, false, [0, 0], [4, 0]), 2);
        assert_eq!(p_boundary_strength(false, false, [0, 0], [1, -1]), 1);
        assert_eq!(p_boundary_strength(false, false, [2, -3], [2, -3]), 0);
    }

    #[test]
    fn parameter_tables_respect_qp_offsets_and_bit_depth_scaling() {
        let p = parameters(26, 0, 0, 8).unwrap();
        assert_eq!(p.alpha, 15);
        assert_eq!(p.beta, 6);
        assert_eq!(p.tc0, [-1, 1, 1, 1]);

        let low = parameters(20, -2, -2, 8).unwrap();
        assert_eq!(low.alpha, 4);
        assert_eq!(low.beta, 2);

        let high = parameters(32, 0, 0, 10).unwrap();
        assert_eq!(high.alpha, 28);
        assert_eq!(high.beta, 12);
    }

    #[test]
    fn high_bit_depth_samples_are_clipped_to_the_configured_depth() {
        let mut line = [1000, 1010, 1020, 1022, 1023, 1023, 1023, 1023];
        filter_luma(&mut line, 255, 18, 8, 10);
        assert!(line.iter().all(|&sample| sample <= 1023));
    }

    #[test]
    fn picture_pass_applies_an_internal_coded_edge_in_place() {
        let mut data = vec![130u16; 16 * 16];
        for row in 0..16 {
            data[row * 16 + 8..row * 16 + 16].fill(135);
        }
        let mut nonzero = [0u8; 16];
        nonzero[1] = 1;
        nonzero[2] = 1;
        let map = DeblockMap {
            mb_width: 1,
            mb_height: 1,
            decoded: vec![true],
            qp: vec![26],
            luma_non_zero: vec![nonzero],
            chroma_non_zero: [Vec::new(), Vec::new()],
            motion_vectors: None,
        };
        let mut plane = PlaneMut {
            data: &mut data,
            width: 16,
            height: 16,
            stride: 16,
        };
        deblock_i_picture(&mut plane, None, None, &map, 8, 0, 0, [0, 0]).unwrap();
        assert_eq!(plane.data[7], 132);
        assert_eq!(plane.data[8], 133);
    }
}
