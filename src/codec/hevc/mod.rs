//! H.265 / HEVC bitstream syntax.
//!
//! Mirrors the official MPP's `mpp/codec/dec/h265` layer. This module covers
//! the syntax (NAL split, VPS/SPS/PPS, slice header); the register HAL for
//! rkvdec lands under `hal/` once the tables are ready.

pub mod dpb;
pub mod hw;
pub mod parser;

pub use dpb::{Dpb, FrameStore};
pub use hw::{build_slice_rps, build_sps_rps, LongTermRef};
pub use parser::{
    parse_pps, parse_slice_header, parse_sps, parse_vps, split_annex_b, Pps, ShortTermRps,
    SliceHeader, Sps, Vps,
};
