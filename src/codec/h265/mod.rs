//! H.265 (HEVC) encoder front-ends.
//!
//! The syntax this crate emits today is the parameter sets in [`encoder`]. The
//! decoder side of the same format lives beside it under `codec::hevc`.

pub mod encoder;
