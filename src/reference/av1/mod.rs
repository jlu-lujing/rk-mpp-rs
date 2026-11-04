//! Software AV1 reference decoder (M11).
//!
//! The hardware path hands whole tile payloads to rkvdec; this module is the
//! independent decode that can judge them. It starts with the specification's
//! symbol decoder (`AV1 Bitstream & Decoding Process Specification` 8.2),
//! because every tile syntax element is read through it.

pub mod cdf_defaults;
pub mod coeff;
pub mod coeff_tables;
pub mod intra;
pub mod partition;
pub mod scan;
pub mod scan_defaults;
pub mod symbol;
pub mod tx;
