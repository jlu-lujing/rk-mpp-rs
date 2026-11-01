//! Rate control, ported from `mpp/codec/rc/`.
//!
//! Nothing here is wired into the encoder yet: `Vepu580H264` still encodes at a
//! fixed QP, and the bit rate figures in `EncoderConfig` reach only the
//! `uuid_rc_cfg` SEI. What this module holds so far is the statistic the C model
//! is written on, checked against the C library itself rather than against a
//! reading of it, because every rule above it is arithmetic on these windows.
//!
//! The order of work is deliberate. `rc_model_v2.c` - the model H.264 actually
//! uses, registered as `default_h264e` by `h264e_rc.c` - is 1733 lines of
//! accumulated heuristics whose every branch reads one of these windows. Getting
//! the container exact first means a later disagreement inside the model is
//! attributable to the model.

pub mod data;
