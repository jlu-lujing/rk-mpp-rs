//! Software AV1 reference decoder (M11).
//!
//! The hardware path hands whole tile payloads to rkvdec; this module is the
//! independent decode that can judge them. It starts with the specification's
//! symbol decoder (`AV1 Bitstream & Decoding Process Specification` 8.2),
//! because every tile syntax element is read through it.

pub mod bitstream;
pub mod block;
pub mod cdef;
pub mod cdf_defaults;
pub mod coeff;
pub mod coeff_tables;
pub mod context;
pub mod deblock;
pub mod frame_header;
pub mod idct;
pub mod intra;
pub mod lr_frame;
pub mod lr_syntax;
pub mod obu;
pub mod partition;
pub mod predict;
pub mod quant;
pub mod quant_tables;
pub mod reconstruct;
pub mod restoration;
pub mod scan;
pub mod scan_defaults;
pub mod seq_header;
pub mod sm_weights;
pub mod subexp;
pub mod symbol;
pub mod tile;
pub mod tile_group;
pub mod tx;
