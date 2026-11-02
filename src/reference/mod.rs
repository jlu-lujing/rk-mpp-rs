//! Software reference decoders.
//!
//! These exist to be the ground truth for the hardware paths: they are written
//! from the specifications, not wrapped around another library. Validation is
//! against the *original* picture the test streams were encoded from (a
//! synthetic source we generate ourselves), so nothing external is needed.

pub mod cabac;
pub mod h264;
pub mod transform;
