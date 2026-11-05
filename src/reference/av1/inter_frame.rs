//! Pixel reconstruction for the first single-reference inter slice.
//!
//! The tile sink has already decoded the reference, mode and integer MV. This
//! module applies the normative motion-vector scaling and sample copy to the
//! reference frame and maintains the eight AV1 reference-frame slots after
//! post-processing.

use crate::reference::av1::cdf_state::CdfState;
use crate::reference::av1::frame_header::FrameHeader;
use crate::reference::av1::inter::{InterBlock, MotionVector};
use crate::reference::av1::inter_sink::InterSyntaxBlock;
use crate::reference::av1::intra_frame::{IntraFrame, PlaneBuffer};
use crate::reference::av1::seq_header::SequenceHeader;

const REF_SCALE_SHIFT: u32 = 14;
const SCALE_SUBPEL_BITS: u32 = 10;
const SUBPEL_BITS: u32 = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterFrameError {
    MissingReference(i8),
    InvalidState(&'static str),
    FractionalMotionVector(MotionVector),
    MissingPlane(usize),
    UnsupportedPostFilter,
}

impl core::fmt::Display for InterFrameError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}

/// Final post-filtered frames indexed by AV1's eight reference slots.
#[derive(Debug, Clone, Default)]
pub struct ReferenceFrameStore {
    slots: [Option<ReferenceSlot>; 8],
}

#[derive(Debug, Clone)]
struct ReferenceSlot {
    frame: IntraFrame,
    cdfs: CdfState,
}

impl ReferenceFrameStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, slot: usize) -> Option<&IntraFrame> {
        self.slots
            .get(slot)
            .and_then(Option::as_ref)
            .map(|slot| &slot.frame)
    }

    pub fn load_cdfs(&self, slot: usize) -> Option<CdfState> {
        self.slots
            .get(slot)
            .and_then(Option::as_ref)
            .map(|slot| slot.cdfs.clone())
    }

    pub fn refresh(&mut self, flags: u8, frame: &IntraFrame, cdfs: &CdfState) {
        let mut saved_cdfs = cdfs.clone();
        saved_cdfs.reset_symbol_counters();
        for slot in 0..self.slots.len() {
            if flags & (1 << slot) != 0 {
                self.slots[slot] = Some(ReferenceSlot {
                    frame: frame.clone(),
                    cdfs: saved_cdfs.clone(),
                });
            }
        }
    }
}

fn round2_signed(value: i64, shift: u32) -> i64 {
    if shift == 0 {
        return value;
    }
    let offset = 1i64 << (shift - 1);
    if value >= 0 {
        (value + offset) >> shift
    } else {
        -((-value + offset) >> shift)
    }
}

fn scale_for(current: u32, reference: u32) -> i64 {
    ((i64::from(reference) << REF_SCALE_SHIFT) + i64::from(current / 2)) / i64::from(current)
}

fn source_start(
    x: usize,
    y: usize,
    mv: MotionVector,
    sub_x: u32,
    sub_y: u32,
    current_width: u32,
    current_height: u32,
    reference_width: u32,
    reference_height: u32,
) -> Result<(i32, i32), InterFrameError> {
    if !mv.is_integer() {
        return Err(InterFrameError::FractionalMotionVector(mv));
    }
    let x_scale = scale_for(current_width, reference_width);
    let y_scale = scale_for(current_height, reference_height);
    let half_sample = 1i64 << (SUBPEL_BITS - 1);
    let orig_x = (x as i64) * (1 << SUBPEL_BITS) + ((2 * i64::from(mv.col)) >> sub_x) + half_sample;
    let orig_y = (y as i64) * (1 << SUBPEL_BITS) + ((2 * i64::from(mv.row)) >> sub_y) + half_sample;
    let base_x = orig_x * x_scale - (half_sample << REF_SCALE_SHIFT);
    let base_y = orig_y * y_scale - (half_sample << REF_SCALE_SHIFT);
    let off = 1i64 << (SCALE_SUBPEL_BITS - SUBPEL_BITS - 1);
    let start_x = round2_signed(base_x, REF_SCALE_SHIFT + SUBPEL_BITS - SCALE_SUBPEL_BITS) + off;
    let start_y = round2_signed(base_y, REF_SCALE_SHIFT + SUBPEL_BITS - SCALE_SUBPEL_BITS) + off;
    Ok((
        (start_x >> SCALE_SUBPEL_BITS) as i32,
        (start_y >> SCALE_SUBPEL_BITS) as i32,
    ))
}

fn copy_integer_block(
    reference: &IntraFrame,
    current: &mut IntraFrame,
    plane: usize,
    block: InterBlock,
    block_col: usize,
    block_row: usize,
    width: usize,
    height: usize,
    sub_x: u32,
    sub_y: u32,
    current_width: u32,
    current_height: u32,
) -> Result<(), InterFrameError> {
    let reference_plane = match plane {
        0 => &reference.y,
        1 => reference
            .u
            .as_ref()
            .ok_or(InterFrameError::MissingPlane(plane))?,
        2 => reference
            .v
            .as_ref()
            .ok_or(InterFrameError::MissingPlane(plane))?,
        _ => return Err(InterFrameError::InvalidState("plane index")),
    };
    let current_plane = match plane {
        0 => &mut current.y,
        1 => current
            .u
            .as_mut()
            .ok_or(InterFrameError::MissingPlane(plane))?,
        2 => current
            .v
            .as_mut()
            .ok_or(InterFrameError::MissingPlane(plane))?,
        _ => return Err(InterFrameError::InvalidState("plane index")),
    };
    let (start_x, start_y) = source_start(
        block_col,
        block_row,
        block.mv,
        sub_x,
        sub_y,
        current_width,
        current_height,
        reference_plane.width as u32,
        reference_plane.height as u32,
    )?;
    let last_x = reference_plane.width.saturating_sub(1);
    let last_y = reference_plane.height.saturating_sub(1);
    for row in 0..height {
        let src_y = (start_y as isize + row as isize).clamp(0, last_y as isize) as usize;
        for col in 0..width {
            let src_x = (start_x as isize + col as isize).clamp(0, last_x as isize) as usize;
            let value = reference_plane.data[src_y * reference_plane.stride + src_x];
            let dst_x = block_col + col;
            let dst_y = block_row + row;
            current_plane.data[dst_y * current_plane.stride + dst_x] = value;
        }
    }
    Ok(())
}

/// Reconstruct an inter frame made entirely of single-reference blocks.
pub fn reconstruct_inter_frame(
    sequence: &SequenceHeader,
    header: &FrameHeader,
    blocks: &[InterSyntaxBlock],
    references: &ReferenceFrameStore,
) -> Result<IntraFrame, InterFrameError> {
    if header.frame_is_intra {
        return Err(InterFrameError::InvalidState(
            "inter driver received intra frame",
        ));
    }
    if header.loop_filter_level[0] != 0
        || header.loop_filter_level[1] != 0
        || (sequence.enable_cdef && !header.coded_lossless)
        || header.uses_lr
        || header.use_superres
    {
        return Err(InterFrameError::UnsupportedPostFilter);
    }
    let width = header.mi_cols * 4;
    let height = header.mi_rows * 4;
    let sub_x = u32::from(sequence.color.subsampling_x);
    let sub_y = u32::from(sequence.color.subsampling_y);
    let chroma_width = width >> sub_x;
    let chroma_height = height >> sub_y;
    let bit_depth = u32::from(sequence.color.bit_depth);
    let mut frame = IntraFrame {
        coded_width: width,
        coded_height: height,
        viewport_width: header.upscaled_width as usize,
        viewport_height: header.frame_height as usize,
        bit_depth,
        y: PlaneBuffer::new(width, height),
        u: (!sequence.color.mono_chrome).then(|| PlaneBuffer::new(chroma_width, chroma_height)),
        v: (!sequence.color.mono_chrome).then(|| PlaneBuffer::new(chroma_width, chroma_height)),
    };
    for block in blocks {
        let ref_slot = header.ref_frame_idx[(block.inter.ref_frame - 1) as usize] as usize;
        let reference = references
            .get(ref_slot)
            .ok_or(InterFrameError::MissingReference(block.inter.ref_frame))?;
        let plane_widths = [width, chroma_width, chroma_width];
        let plane_heights = [height, chroma_height, chroma_height];
        let planes = if sequence.color.mono_chrome { 1 } else { 3 };
        for plane in 0..planes {
            let plane_sub_x = if plane == 0 { 0 } else { sub_x };
            let plane_sub_y = if plane == 0 { 0 } else { sub_y };
            let block_col = (block.block.col * 4) >> plane_sub_x;
            let block_row = (block.block.row * 4) >> plane_sub_y;
            let block_width = ((block.block.size.width as usize) >> plane_sub_x)
                .min(plane_widths[plane].saturating_sub(block_col));
            let block_height = ((block.block.size.height as usize) >> plane_sub_y)
                .min(plane_heights[plane].saturating_sub(block_row));
            copy_integer_block(
                reference,
                &mut frame,
                plane,
                block.inter.clone(),
                block_col,
                block_row,
                block_width,
                block_height,
                plane_sub_x,
                plane_sub_y,
                header.frame_width,
                header.frame_height,
            )?;
        }
    }
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reference::av1::cdf_state::CdfState;
    use crate::reference::av1::frame_header::PRIMARY_REF_NONE;
    use crate::reference::av1::inter::InterMode;
    use crate::reference::av1::inter_sink::{InterTileBlockSink, InterTileConfig};
    use crate::reference::av1::intra_frame::reconstruct_intra_frame;
    use crate::reference::av1::intra_sink::{IntraTileBlockSink, IntraTileConfig};
    use crate::reference::av1::obu::parse_ivf;
    use crate::reference::av1::stream::{ReferenceStream, StreamEvent};
    use crate::reference::av1::tile_entropy::decode_tile_partitions_with_cdfs;

    #[test]
    fn signed_rounding_matches_the_spec() {
        assert_eq!(round2_signed(15, 4), 1);
        assert_eq!(round2_signed(16, 4), 1);
        assert_eq!(round2_signed(-15, 4), -1);
        assert_eq!(round2_signed(-16, 4), -1);
    }

    #[test]
    fn zero_motion_copies_the_same_sample() {
        let frame = IntraFrame {
            coded_width: 4,
            coded_height: 4,
            viewport_width: 4,
            viewport_height: 4,
            bit_depth: 8,
            y: PlaneBuffer {
                data: (0..64).collect(),
                width: 4,
                height: 4,
                stride: 8,
            },
            u: None,
            v: None,
        };
        let mut out = frame.clone();
        copy_integer_block(
            &frame,
            &mut out,
            0,
            InterBlock {
                ref_frame: 1,
                mode: crate::reference::av1::inter::InterMode::Nearest,
                mv: MotionVector::default(),
                motion_mode: crate::reference::av1::inter::MotionMode::Simple,
                interp_filter: [0, 0],
                ref_mv_idx: 0,
            },
            0,
            0,
            4,
            4,
            0,
            0,
            4,
            4,
        )
        .unwrap();
        assert_eq!(out.y.data, frame.y.data);
    }

    fn visible_hash(plane: &PlaneBuffer) -> u64 {
        plane
            .data
            .chunks_exact(plane.stride)
            .take(plane.height)
            .flat_map(|row| &row[..plane.width])
            .fold(0xcbf29ce484222325u64, |hash, sample| {
                (hash ^ u64::from(*sample)).wrapping_mul(0x100000001b3)
            })
    }

    #[test]
    fn real_integer_inter_frame_matches_libaom_and_locks_the_block_modes() {
        let data = include_bytes!("../../../tests/data/av1_inter_box_320x240.ivf");
        let ivf = parse_ivf(data).unwrap();
        let mut stream = ReferenceStream::new();
        let events = stream.parse_ivf(&ivf).unwrap();
        let sequence = stream.sequence().unwrap();
        let mut references = ReferenceFrameStore::new();
        let mut pictures = Vec::new();
        let mut inter_blocks = Vec::new();

        for event in events.iter().flatten() {
            let StreamEvent::CodedFrame(frame) = event else {
                continue;
            };
            let initial_cdfs = if frame.header.primary_ref_frame == PRIMARY_REF_NONE {
                CdfState::new()
            } else {
                let slot =
                    frame.header.ref_frame_idx[frame.header.primary_ref_frame as usize] as usize;
                references.load_cdfs(slot).unwrap()
            };
            let tile = &frame.layout.tiles[0];
            let final_cdfs;
            let picture = if frame.header.frame_is_intra {
                let config = IntraTileConfig::from_headers(sequence, &frame.header);
                let sb_size = if config.superblock_mi == 32 { 128 } else { 64 };
                let decoded = decode_tile_partitions_with_cdfs(
                    &frame.payload[tile.bytes.clone()],
                    &frame.header,
                    tile.tile,
                    sb_size,
                    false,
                    true,
                    initial_cdfs,
                    IntraTileBlockSink::new(config),
                )
                .unwrap();
                final_cdfs = decoded.cdfs;
                reconstruct_intra_frame(sequence, &frame.header, &decoded.sink.blocks).unwrap()
            } else {
                let config = InterTileConfig::from_headers(sequence, &frame.header);
                let sb_size = if config.superblock_mi == 32 { 128 } else { 64 };
                let decoded = decode_tile_partitions_with_cdfs(
                    &frame.payload[tile.bytes.clone()],
                    &frame.header,
                    tile.tile,
                    sb_size,
                    false,
                    true,
                    initial_cdfs,
                    InterTileBlockSink::new(config),
                )
                .unwrap();
                inter_blocks = decoded.sink.blocks.clone();
                final_cdfs = decoded.cdfs;
                reconstruct_inter_frame(sequence, &frame.header, &decoded.sink.blocks, &references)
                    .unwrap()
            };
            references.refresh(frame.header.refresh_frame_flags, &picture, &final_cdfs);
            pictures.push(picture);
        }

        assert_eq!(pictures.len(), 2);
        assert_eq!(
            (
                visible_hash(&pictures[0].y),
                visible_hash(pictures[0].u.as_ref().unwrap()),
                visible_hash(pictures[0].v.as_ref().unwrap())
            ),
            (0xc30180323dd09b41, 0x930bef4f94807f25, 0x930bef4f94807f25)
        );
        assert_eq!(
            (
                visible_hash(&pictures[1].y),
                visible_hash(pictures[1].u.as_ref().unwrap()),
                visible_hash(pictures[1].v.as_ref().unwrap())
            ),
            (0xdd5a7af1044fd871, 0x930bef4f94807f25, 0x930bef4f94807f25)
        );

        assert_eq!(inter_blocks.len(), 20);
        assert_eq!(
            inter_blocks
                .iter()
                .map(|block| (
                    block.block.row,
                    block.block.col,
                    block.inter.mode,
                    block.inter.mv.row,
                    block.inter.mv.col
                ))
                .collect::<Vec<_>>(),
            [
                (0, 0, InterMode::Nearest, 0, 0),
                (0, 16, InterMode::Nearest, 0, 0),
                (0, 32, InterMode::Nearest, 0, 0),
                (0, 48, InterMode::Nearest, 0, 0),
                (0, 64, InterMode::Nearest, 0, 0),
                (16, 0, InterMode::New, 0, -64),
                (16, 16, InterMode::New, 0, -64),
                (16, 32, InterMode::Nearest, 0, 0),
                (16, 48, InterMode::Nearest, 0, 0),
                (16, 64, InterMode::Nearest, 0, 0),
                (32, 0, InterMode::Nearest, 0, -64),
                (32, 16, InterMode::Nearest, 0, -64),
                (32, 32, InterMode::Nearest, 0, 0),
                (32, 48, InterMode::Nearest, 0, 0),
                (32, 64, InterMode::Nearest, 0, 0),
                (48, 0, InterMode::Nearest, 0, -64),
                (48, 16, InterMode::Nearest, 0, -64),
                (48, 32, InterMode::Nearest, 0, 0),
                (48, 48, InterMode::Nearest, 0, 0),
                (48, 64, InterMode::Nearest, 0, 0),
            ]
        );
    }
}
