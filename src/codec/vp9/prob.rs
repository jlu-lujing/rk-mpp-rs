//! VP9 probability contexts.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MotionVectorProbabilities {
    pub sign: u8,
    pub classes: [u8; 10],
    pub class0: u8,
    pub bits: [u8; 10],
    pub class0_fp: [[u8; 3]; 2],
    pub fp: [u8; 3],
    pub class0_hp: u8,
    pub hp: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HeaderProbabilities {
    pub y_mode: [[u8; 9]; 4],
    pub uv_mode: [[u8; 9]; 10],
    pub filter: [[u8; 2]; 4],
    pub mv_mode: [[u8; 3]; 7],
    pub intra: [u8; 4],
    pub comp: [u8; 5],
    pub single_ref: [[u8; 2]; 5],
    pub comp_ref: [u8; 5],
    pub tx32p: [[u8; 3]; 2],
    pub tx16p: [[u8; 2]; 2],
    pub tx8p: [u8; 2],
    pub skip: [u8; 3],
    pub mv_joint: [u8; 3],
    pub mv_comp: [MotionVectorProbabilities; 2],
    pub partition: [[[u8; 3]; 4]; 4],
}

pub type CoefficientProbabilities = [[[[[[u8; 3]; 6]; 6]; 2]; 2]; 4];

/// Per-symbol update metadata consumed by the vdpu34x probability loader.
///
/// `flags` has the same shape as the probability context. `deltas` stores the
/// raw VP9 probability delta code, except for motion-vector probabilities,
/// where MPP stores the new probability itself.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProbabilityUpdateData {
    pub header: HeaderProbabilities,
    pub coefficients: CoefficientProbabilities,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProbabilityUpdates {
    pub flags: ProbabilityUpdateData,
    pub deltas: ProbabilityUpdateData,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbabilityContext {
    pub header: HeaderProbabilities,
    pub coefficients: CoefficientProbabilities,
}

impl ProbabilityContext {
    pub fn snapshot_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(311 + 4 * 2 * 2 * 6 * 6 * 3);
        append_header_probabilities(&self.header, &mut bytes);
        debug_assert_eq!(bytes.len(), 311);
        append_coefficient_probabilities(&self.coefficients, &mut bytes);
        bytes
    }

    /// `vp9d_fill_picparams` reverses the partition levels and maps the
    /// syntax uv_mode order `v,h,dc,...` to `dc,v,h,...` for the hardware.
    pub fn hardware_snapshot_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(311 + 4 * 2 * 2 * 6 * 6 * 3);
        append_hardware_header_probabilities(&self.header, &mut bytes);
        append_coefficient_probabilities(&self.coefficients, &mut bytes);
        bytes
    }
}

impl ProbabilityUpdateData {
    pub fn snapshot_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(311 + 4 * 2 * 2 * 6 * 6 * 3);
        append_header_probabilities(&self.header, &mut bytes);
        debug_assert_eq!(bytes.len(), 311);
        append_coefficient_probabilities(&self.coefficients, &mut bytes);
        bytes
    }

    pub fn hardware_snapshot_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(311 + 4 * 2 * 2 * 6 * 6 * 3);
        append_hardware_header_probabilities(&self.header, &mut bytes);
        append_coefficient_probabilities(&self.coefficients, &mut bytes);
        bytes
    }
}

impl ProbabilityUpdates {
    /// Exact `DXVA_PicParams_VP9::prob_flag_delta` byte order.
    pub fn hardware_snapshot_bytes(&self) -> Vec<u8> {
        let flags = self.flags.hardware_snapshot_bytes();
        let deltas = self.deltas.hardware_snapshot_bytes();
        let mut bytes = Vec::with_capacity(flags.len() + deltas.len());
        bytes.extend_from_slice(&flags[..311]);
        bytes.extend_from_slice(&deltas[..311]);
        bytes.extend_from_slice(&flags[311..]);
        bytes.extend_from_slice(&deltas[311..]);
        bytes
    }
}

fn append_header_probabilities(header: &HeaderProbabilities, bytes: &mut Vec<u8>) {
    for row in &header.y_mode {
        bytes.extend_from_slice(row);
    }
    for row in &header.uv_mode {
        bytes.extend_from_slice(row);
    }
    for row in &header.filter {
        bytes.extend_from_slice(row);
    }
    for row in &header.mv_mode {
        bytes.extend_from_slice(row);
    }
    bytes.extend_from_slice(&header.intra);
    bytes.extend_from_slice(&header.comp);
    for row in &header.single_ref {
        bytes.extend_from_slice(row);
    }
    bytes.extend_from_slice(&header.comp_ref);
    for row in &header.tx32p {
        bytes.extend_from_slice(row);
    }
    for row in &header.tx16p {
        bytes.extend_from_slice(row);
    }
    bytes.extend_from_slice(&header.tx8p);
    bytes.extend_from_slice(&header.skip);
    bytes.extend_from_slice(&header.mv_joint);
    for component in &header.mv_comp {
        bytes.push(component.sign);
        bytes.extend_from_slice(&component.classes);
        bytes.push(component.class0);
        bytes.extend_from_slice(&component.bits);
        for row in &component.class0_fp {
            bytes.extend_from_slice(row);
        }
        bytes.extend_from_slice(&component.fp);
        bytes.push(component.class0_hp);
        bytes.push(component.hp);
    }
    for row in &header.partition {
        for probability in row {
            bytes.extend_from_slice(probability);
        }
    }
}

fn append_hardware_header_probabilities(header: &HeaderProbabilities, bytes: &mut Vec<u8>) {
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
    append_header_probabilities(&hardware, bytes);
}

fn append_coefficient_probabilities(coefficients: &CoefficientProbabilities, bytes: &mut Vec<u8>) {
    for transform in coefficients {
        for block_type in transform {
            for intra in block_type {
                for band in intra {
                    for position in band {
                        bytes.extend_from_slice(position);
                    }
                }
            }
        }
    }
}

impl Default for ProbabilityContext {
    fn default() -> Self {
        Self {
            header: DEFAULT_HEADER_PROBS,
            coefficients: DEFAULT_COEF_PROBS,
        }
    }
}

pub use super::tables::{DEFAULT_COEF_PROBS, DEFAULT_HEADER_PROBS};
