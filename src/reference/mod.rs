//! Software reference decoders.
//!
//! These exist to be the ground truth for the hardware paths: they are written
//! from the specifications, not wrapped around another library. Validation is
//! against the *original* picture the test streams were encoded from (a
//! synthetic source we generate ourselves), so nothing external is needed.

pub mod av1;
pub mod cabac;
pub mod h264;
pub mod h264_cabac;
pub(crate) mod h264_cabac_pb;
pub mod h264_cavlc;
pub mod h264_deblock;
pub mod h264_mb;
pub mod h264_pred;
pub mod h264_recon;
pub mod h264_residual;
pub mod h264_slice;
pub mod transform;
