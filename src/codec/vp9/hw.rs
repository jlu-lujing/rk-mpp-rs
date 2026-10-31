//! VP9 buffers consumed by the vdpu34x hardware.
//!
//! `DXVA_PicParams_VP9` stores probabilities in a convenient fixed layout, but
//! the hardware does not consume that layout directly. `hal_vp9d_com.c`
//! repacks it into three different bit streams: the probe/context data,
//! the default probability image, and the flag/delta update image. The bit
//! writer is LSB-first, and the unusual 27-byte alignment follows the C code
//! byte for byte.

use crate::codec::h264::hw::BitWriter;

use super::compressed::CompressedHeader;
use super::parser::{FrameKind, Header};
use super::prob::{
    CoefficientProbabilities, HeaderProbabilities, ProbabilityContext, DEFAULT_COEF_PROBS,
    DEFAULT_HEADER_PROBS,
};
use super::tables::{KEYFRAME_PARTITION_PROBS, KEYFRAME_UV_MODE_PROBS, KEYFRAME_Y_MODE_PROBS};

pub const PROB_SIZE: usize = 4864;
pub const COUNT_SIZE: usize = 13208;
pub const COUNT_OFFSET: usize = 8192;
pub const PROBE_BUFFER_SIZE: usize = COUNT_OFFSET + align_up(COUNT_SIZE, 4096);
pub const PROBE_WORDS: usize = 304;
pub const PROBE_SIZE: usize = PROBE_WORDS * 8;
pub const MAX_SEGMAP_SIZE: usize = 73728 * 4;
pub const MAX_SEGMAP_SIZE_ALIGNED: usize = align_up(MAX_SEGMAP_SIZE, 4096);
pub const VP9_CTU_SIZE: u32 = 64;

pub type ProbeBuffer = [u8; PROBE_SIZE];
pub type ProbabilityBuffer = [u8; PROB_SIZE];

/// Default probability image loaded into `reg162` for a key/intra-only frame.
pub fn build_prob_default(header: &Header, compressed: &CompressedHeader) -> ProbabilityBuffer {
    let probabilities = effective_default_context(header, compressed);
    let header_probs = to_hardware_header(&probabilities.header);
    let mut writer = BitWriter::new();

    put_partition(&mut writer, header_probs.partition.as_flattened(), 8);
    put_zeros(&mut writer, 3 + 7);
    put_bytes(&mut writer, &header_probs.skip);
    put_tx_probabilities(&mut writer, &header_probs);
    put_bytes(&mut writer, &header_probs.intra);
    writer.align(128);

    put_inter_header_probabilities(&mut writer, &header_probs);
    writer.align(128);

    put_coefficient_type(&mut writer, &probabilities.coefficients, 0);
    put_coefficient_type(&mut writer, &probabilities.coefficients, 1);

    put_uv_probabilities(&mut writer, &header_probs.uv_mode);
    writer.align(128);
    writer.put(0, 8);
    writer.align(128);

    put_motion_vector_probabilities(&mut writer, &header_probs);
    writer.align(128);
    writer.put(0, 8);
    writer.align(128);

    fixed_array(writer)
}

/// Flag/delta image loaded through `reg160` for the in-hardware probability
/// update engine.
pub fn build_prob_flag_delta(header: &Header, compressed: &CompressedHeader) -> ProbabilityBuffer {
    let updates = &compressed.updates;
    let flag = to_hardware_header(&updates.flags.header);
    let delta = to_hardware_header(&updates.deltas.header);
    let mut writer = BitWriter::new();

    if is_intra(header) {
        put_zero_bits(&mut writer, 48 + 3 + 7);
        put_bytes_bits(&mut writer, &flag.skip);
        put_tx_flag_probabilities(&mut writer, &flag);
        put_bytes_bits(&mut writer, &flag.intra);
        writer.put(0, 3);

        put_coefficient_flags(&mut writer, &updates.flags.coefficients, 0);
        put_coefficient_flags(&mut writer, &updates.flags.coefficients, 1);
        writer.align(128);

        put_partition(&mut writer, &KEYFRAME_PARTITION_PROBS, 8);
        put_bytes(&mut writer, &header.segmentation.pred_probs);
        put_bytes(&mut writer, &header.segmentation.tree_probs);
        put_bytes(&mut writer, &delta.skip);
        put_tx_probabilities(&mut writer, &delta);
        put_bytes(&mut writer, &delta.intra);
        writer.align(128);

        put_coefficient_type(&mut writer, &updates.deltas.coefficients, 0);
        put_keyframe_mode_probabilities(&mut writer);
        put_coefficient_type(&mut writer, &updates.deltas.coefficients, 1);
    } else {
        put_partition(&mut writer, flag.partition.as_flattened(), 1);
        put_zero_bits(&mut writer, 3 + 7);
        put_bytes_bits(&mut writer, &flag.skip);
        put_tx_flag_probabilities(&mut writer, &flag);
        put_bytes_bits(&mut writer, &flag.intra);
        writer.put(0, 3);

        put_inter_header_flags(&mut writer, &flag);
        writer.put(0, 11);
        put_coefficient_flags(&mut writer, &updates.flags.coefficients, 0);
        put_coefficient_flags(&mut writer, &updates.flags.coefficients, 1);
        put_uv_flags(&mut writer, &flag.uv_mode);
        put_motion_vector_flags(&mut writer, &flag);
        writer.put(0, 11);
        for _ in 0..8 {
            writer.put(0, 16);
        }

        put_partition(&mut writer, delta.partition.as_flattened(), 8);
        put_bytes(&mut writer, &header.segmentation.pred_probs);
        put_bytes(&mut writer, &header.segmentation.tree_probs);
        put_bytes(&mut writer, &delta.skip);
        put_tx_probabilities(&mut writer, &delta);
        put_bytes(&mut writer, &delta.intra);
        writer.put(0, 24);

        put_inter_header_probabilities(&mut writer, &delta);
        put_zeros(&mut writer, 11);
        put_inter_coefficient_delta(&mut writer, &updates.deltas.coefficients, 0);
        put_inter_coefficient_delta(&mut writer, &updates.deltas.coefficients, 1);
        put_uv_delta(&mut writer, &delta.uv_mode);
        put_motion_vector_probabilities(&mut writer, &delta);
        put_zeros(&mut writer, 27);
    }
    writer.align(128);
    fixed_array(writer)
}

/// First 2432 bytes of the shared probe buffer (`reg160`).
pub fn build_probe(header: &Header, compressed: &CompressedHeader) -> ProbeBuffer {
    let probabilities = effective_default_context(header, compressed);
    let hardware = to_hardware_header(&probabilities.header);
    let mut writer = BitWriter::new();

    if is_intra(header) {
        put_partition(&mut writer, &KEYFRAME_PARTITION_PROBS, 8);
    } else {
        put_partition(&mut writer, hardware.partition.as_flattened(), 8);
    }
    put_bytes(&mut writer, &header.segmentation.pred_probs);
    put_bytes(&mut writer, &header.segmentation.tree_probs);
    put_bytes(&mut writer, &hardware.skip);
    put_tx_probabilities(&mut writer, &hardware);
    put_bytes(&mut writer, &hardware.intra);
    writer.align(128);

    if is_intra(header) {
        put_coefficient_type(&mut writer, &probabilities.coefficients, 0);
        put_keyframe_mode_probabilities(&mut writer);
        for _ in 0..2 {
            writer.put(0, 8);
            writer.align(128);
        }
    } else {
        put_inter_header_probabilities(&mut writer, &hardware);
        writer.align(128);
        put_coefficient_type(&mut writer, &probabilities.coefficients, 0);
        put_coefficient_type(&mut writer, &probabilities.coefficients, 1);
        put_uv_probabilities(&mut writer, &hardware.uv_mode);
        writer.align(128);
        writer.put(0, 8);
        writer.align(128);
        put_motion_vector_probabilities(&mut writer, &hardware);
        writer.align(128);
    }

    let bytes = writer.bytes();
    assert!(
        bytes.len() <= PROBE_SIZE,
        "VP9 probe generator produced {} bytes",
        bytes.len()
    );
    let mut output = [0u8; PROBE_SIZE];
    output[..bytes.len()].copy_from_slice(&bytes);
    output
}

fn effective_default_context(header: &Header, compressed: &CompressedHeader) -> ProbabilityContext {
    let mut probabilities = compressed.probabilities.clone();
    if is_intra(header) {
        probabilities.header.tx32p = DEFAULT_HEADER_PROBS.tx32p;
        probabilities.header.tx16p = DEFAULT_HEADER_PROBS.tx16p;
        probabilities.header.tx8p = DEFAULT_HEADER_PROBS.tx8p;
        probabilities.header.skip = DEFAULT_HEADER_PROBS.skip;
        probabilities.coefficients = DEFAULT_COEF_PROBS;
    }
    probabilities
}

fn is_intra(header: &Header) -> bool {
    matches!(header.frame_kind, FrameKind::Key | FrameKind::IntraOnly)
}

fn to_hardware_header(header: &HeaderProbabilities) -> HeaderProbabilities {
    let mut hardware = *header;
    hardware.uv_mode = [
        header.uv_mode[2],
        header.uv_mode[0],
        header.uv_mode[1],
        header.uv_mode[3],
        header.uv_mode[4],
        header.uv_mode[5],
        header.uv_mode[6],
        header.uv_mode[8],
        header.uv_mode[7],
        header.uv_mode[9],
    ];
    hardware.partition.reverse();
    hardware
}

fn put_tx_probabilities(writer: &mut BitWriter, header: &HeaderProbabilities) {
    for row in &header.tx32p {
        put_bytes(writer, row);
    }
    for row in &header.tx16p {
        put_bytes(writer, row);
    }
    put_bytes(writer, &header.tx8p);
}

fn put_tx_flag_probabilities(writer: &mut BitWriter, header: &HeaderProbabilities) {
    for row in &header.tx32p {
        put_bytes_bits(writer, row);
    }
    for row in &header.tx16p {
        put_bytes_bits(writer, row);
    }
    put_bytes_bits(writer, &header.tx8p);
}

fn put_inter_header_probabilities(writer: &mut BitWriter, header: &HeaderProbabilities) {
    for row in &header.y_mode {
        put_bytes(writer, row);
    }
    put_bytes(writer, &header.comp);
    put_bytes(writer, &header.comp_ref);
    for row in &header.single_ref {
        put_bytes(writer, row);
    }
    for row in &header.mv_mode {
        put_bytes(writer, row);
    }
    for row in &header.filter {
        put_bytes(writer, row);
    }
}

fn put_inter_header_flags(writer: &mut BitWriter, header: &HeaderProbabilities) {
    for row in &header.y_mode {
        put_bytes_bits(writer, row);
    }
    put_bytes_bits(writer, &header.comp);
    put_bytes_bits(writer, &header.comp_ref);
    for row in &header.single_ref {
        put_bytes_bits(writer, row);
    }
    for row in &header.mv_mode {
        put_bytes_bits(writer, row);
    }
    for row in &header.filter {
        put_bytes_bits(writer, row);
    }
}

fn put_partition(writer: &mut BitWriter, partition: &[[u8; 3]], bits: u32) {
    for probabilities in partition {
        for &probability in probabilities {
            writer.put(probability as u32, bits);
        }
    }
}

fn put_coefficient_type(
    writer: &mut BitWriter,
    coefficients: &CoefficientProbabilities,
    block_type: usize,
) {
    for transform in coefficients {
        for plane in transform {
            let mut count = 0;
            for band in &plane[block_type] {
                for context in band {
                    for &probability in context {
                        writer.put(probability as u32, 8);
                        count += 1;
                        if count == 27 {
                            writer.align(128);
                            count = 0;
                        }
                    }
                }
            }
            writer.align(128);
        }
    }
}

fn put_inter_coefficient_delta(
    writer: &mut BitWriter,
    coefficients: &CoefficientProbabilities,
    block_type: usize,
) {
    for transform in coefficients {
        for plane in transform {
            let mut count = 0;
            for band in &plane[block_type] {
                for context in band {
                    for &probability in context {
                        writer.put(probability as u32, 8);
                        count += 1;
                        if count == 27 {
                            writer.put(0, 40);
                            count = 0;
                        }
                    }
                }
            }
        }
    }
}

fn put_coefficient_flags(
    writer: &mut BitWriter,
    coefficients: &CoefficientProbabilities,
    block_type: usize,
) {
    for transform in 0..4 {
        for plane in 0..2 {
            let mut count = 0;
            for band in &coefficients[transform][plane][block_type] {
                for context in band {
                    for &flag in context {
                        writer.put(flag as u32, 1);
                        count += 1;
                        if count == 27 {
                            writer.put(0, 5);
                            count = 0;
                        }
                    }
                }
            }
        }
    }
}

fn put_uv_probabilities(writer: &mut BitWriter, uv_mode: &[[u8; 9]; 10]) {
    for group in uv_mode.chunks(3) {
        for row in group {
            put_bytes(writer, row);
        }
        writer.align(128);
    }
}

fn put_uv_flags(writer: &mut BitWriter, uv_mode: &[[u8; 9]; 10]) {
    for group in uv_mode[..9].chunks(3) {
        for row in group {
            put_bytes_bits(writer, row);
        }
        writer.put(0, 5);
    }
    let last = uv_mode[9];
    put_bytes_bits(writer, &last);
    writer.put(0, 23);
}

fn put_uv_delta(writer: &mut BitWriter, uv_mode: &[[u8; 9]; 10]) {
    for group in uv_mode[..9].chunks(3) {
        for row in group {
            put_bytes(writer, row);
        }
        writer.put(0, 40);
    }
    put_bytes(writer, &uv_mode[9]);
    for _ in 0..23 {
        writer.put(0xff, 8);
    }
}

fn put_motion_vector_probabilities(writer: &mut BitWriter, header: &HeaderProbabilities) {
    put_bytes(writer, &header.mv_joint);
    for component in &header.mv_comp {
        writer.put(component.sign as u32, 8);
    }
    for component in &header.mv_comp {
        put_bytes(writer, &component.classes);
    }
    for component in &header.mv_comp {
        writer.put(component.class0 as u32, 8);
    }
    for component in &header.mv_comp {
        put_bytes(writer, &component.bits);
    }
    for component in &header.mv_comp {
        for row in &component.class0_fp {
            put_bytes(writer, row);
        }
    }
    for component in &header.mv_comp {
        put_bytes(writer, &component.fp);
    }
    for component in &header.mv_comp {
        writer.put(component.class0_hp as u32, 8);
    }
    for component in &header.mv_comp {
        writer.put(component.hp as u32, 8);
    }
}

fn put_motion_vector_flags(writer: &mut BitWriter, header: &HeaderProbabilities) {
    put_bytes_bits(writer, &header.mv_joint);
    for component in &header.mv_comp {
        writer.put(component.sign as u32, 1);
    }
    for component in &header.mv_comp {
        put_bytes_bits(writer, &component.classes);
    }
    for component in &header.mv_comp {
        writer.put(component.class0 as u32, 1);
    }
    for component in &header.mv_comp {
        put_bytes_bits(writer, &component.bits);
    }
    for component in &header.mv_comp {
        for row in &component.class0_fp {
            put_bytes_bits(writer, row);
        }
    }
    for component in &header.mv_comp {
        put_bytes_bits(writer, &component.fp);
    }
    for component in &header.mv_comp {
        writer.put(component.class0_hp as u32, 1);
    }
    for component in &header.mv_comp {
        writer.put(component.hp as u32, 1);
    }
}

fn put_keyframe_mode_probabilities(writer: &mut BitWriter) {
    for (above, rows) in KEYFRAME_Y_MODE_PROBS.iter().enumerate() {
        let mut count = 0;
        for row in rows {
            for &probability in row {
                writer.put(probability as u32, 8);
                count += 1;
                if count == 27 {
                    writer.align(128);
                    count = 0;
                }
            }
        }
        if above < 4 {
            let limit = if above < 3 { 23 } else { 21 };
            for value in KEYFRAME_UV_MODE_PROBS
                .as_flattened()
                .iter()
                .skip(above * 23)
                .take(limit)
            {
                writer.put(*value as u32, 8);
            }
            for _ in limit..23 {
                writer.put(0, 8);
            }
        } else {
            put_zeros(writer, 23);
        }
        writer.align(128);
    }
}

fn put_bytes(writer: &mut BitWriter, values: &[u8]) {
    for &value in values {
        writer.put(value as u32, 8);
    }
}

fn put_bytes_bits(writer: &mut BitWriter, values: &[u8]) {
    for &value in values {
        writer.put(value as u32, 1);
    }
}

fn put_zeros(writer: &mut BitWriter, count: usize) {
    for _ in 0..count {
        writer.put(0, 8);
    }
}

fn put_zero_bits(writer: &mut BitWriter, count: usize) {
    for _ in 0..count {
        writer.put(0, 1);
    }
}

fn fixed_array<const N: usize>(mut writer: BitWriter) -> [u8; N] {
    let bytes = writer.bytes();
    assert!(
        bytes.len() <= N,
        "VP9 hardware table produced {} bytes, limit {N}",
        bytes.len()
    );
    let mut output = [0u8; N];
    output[..bytes.len()].copy_from_slice(&bytes);
    output
}

const fn align_up(value: usize, alignment: usize) -> usize {
    (value + alignment - 1) & !(alignment - 1)
}
