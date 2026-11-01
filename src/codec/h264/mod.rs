//! H.264 / AVC bitstream syntax.
//!
//! Split out of the official MPP's `mpp/codec/dec/h264` (H264Slice, H264DPB,
//! H264Parser). This module owns the syntax layer only: Annex-B splitting,
//! RBSP extraction, SPS/PPS/slice-header parsing. The register HAL that feeds
//! `rkvenc`/`rkvdec` lives under `hal/`.

pub mod cabac_table;
pub mod decoder;
pub mod dpb;
pub mod encoder;
pub mod hw;
pub mod parser;
pub mod regs;
pub mod sei;

pub use decoder::{sort_pictures_by_poc, Decoder, Picture};
pub use dpb::{Dpb, FrameStore};
pub use parser::{
    parse_pps, parse_slice_header, parse_sps, split_annex_b, NalUnit, Pps, SliceHeader, Sps,
    SpsError,
};
