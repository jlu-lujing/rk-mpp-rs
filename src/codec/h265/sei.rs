//! The prefix SEI NAL units MPP puts ahead of an H.265 IDR.
//!
//! Two of them, both user-data-unregistered (payload type 5): the library's
//! version string and `update_rc_cfg_log`'s rate-control line. They are software
//! output - `mpp_enc_impl.cpp` adds them before the hardware's slice NAL - so the
//! stream cannot match the C encoder without them even when the IDR itself does.
//!
//! The bytes here come from the oracle's own stream (RK3588, `mpi_enc_test -t 16777220
//! -f 0 -n 1 -rc 2 -qc 26:...`), which is why `VERSION_TEXT` names the vintage's
//! commit: it is a build fact of that library, not a parameter of the picture.

use std::fmt::Write as _;

use super::encoder::annex_b;

/// Payload type `user_data_unregistered`.
const SEI_TYPE_USER_DATA_UNREGISTERED: u8 = 5;
const SEI_TYPE_RECOVERY_POINT: u8 = 6;

/// The uuid ahead of the version string, from the oracle's first SEI.
pub const VERSION_UUID: [u8; 16] = [
    0x3d, 0x07, 0x6d, 0x45, 0x73, 0x0f, 0x41, 0xa8, 0xb1, 0xc4, 0x25, 0xd7, 0x97, 0x6b, 0xf1, 0xac,
];
/// The vintage library's `uuid_version` text: commit, author, date and the change
/// that build carries.
pub const VERSION_TEXT: &str =
    "80957d266 author: Yandong Lin  2024-04-25 fix[hal_h265e_vepu580]: fix reg config err for 2pass";

/// The uuid ahead of the rate-control line.
pub const RC_UUID: [u8; 16] = [
    0xd7, 0xdc, 0x03, 0xc3, 0xc5, 0x6f, 0x40, 0xe0, 0x8e, 0xa9, 0x17, 0x1a, 0xd2, 0xef, 0x5e, 0x23,
];

/// One user-data-unregistered SEI NAL unit, start code and escaping included.
pub fn user_data_unregistered(uuid: &[u8; 16], text: &[u8]) -> Vec<u8> {
    let size = 16 + text.len();
    let mut rbsp = Vec::with_capacity(2 + size + 1);
    rbsp.push(SEI_TYPE_USER_DATA_UNREGISTERED);
    // Payload sizes above 254 need 0xff continuation bytes; the two strings here
    // are 110 and 84 bytes, so one byte is enough. Assert rather than silently
    // write a wrong length if a caller passes a long text.
    assert!(
        size < 255,
        "SEI user-data payload {size} needs ff continuation"
    );
    rbsp.push(size as u8);
    rbsp.extend_from_slice(uuid);
    rbsp.extend_from_slice(text);
    rbsp.push(0x80); // rbsp_trailing_bits
    annex_b(39, rbsp) // PREFIX_SEI_NUT
}

/// Recovery-point prefix SEI. The H.265 writer's `se(v)` implementation is not
/// the generic Exp-Golomb code: it maps the value to `2*|v|` (or `1-v*2` when
/// positive) and writes that value in `ue_size_tab` bits. Keep that quirk.
pub fn recovery_point(recovery_frame_cnt: i32) -> Vec<u8> {
    let mut bits = Vec::new();
    let coded = if recovery_frame_cnt > 0 {
        recovery_frame_cnt * 2
    } else {
        1 - recovery_frame_cnt * 2
    };
    let floor_log2 = usize::BITS - 1 - (coded as usize).leading_zeros();
    let width = (floor_log2 * 2 + 1) as usize;
    for shift in (0..width).rev() {
        bits.push(((coded >> shift) & 1) as u8);
    }
    bits.extend_from_slice(&[1, 0]);
    bits.push(1);
    while bits.len() & 7 != 0 {
        bits.push(0);
    }
    let payload: Vec<u8> = bits
        .chunks(8)
        .map(|chunk| chunk.iter().fold(0u8, |value, bit| (value << 1) | bit))
        .collect();

    let mut rbsp = Vec::with_capacity(2 + payload.len());
    rbsp.push(SEI_TYPE_RECOVERY_POINT);
    assert!(
        payload.len() < 255,
        "recovery SEI payload needs ff continuation"
    );
    rbsp.push(payload.len() as u8);
    rbsp.extend_from_slice(&payload);
    annex_b(39, rbsp)
}

/// The values `update_rc_cfg_log` prints, in the same order as the H.264 SEI.
pub struct RcCfgLine<'a> {
    pub model_name: &'a str,
    pub mode_name: &'static str,
    pub bps_target: i64,
    pub bps_min: i64,
    pub bps_max: i64,
    pub igop: i64,
    pub init_quality: i32,
    pub min_quality: i32,
    pub max_quality: i32,
    pub min_i_quality: i32,
    pub max_i_quality: i32,
    pub i_quality_delta: i32,
}

impl RcCfgLine<'static> {
    /// The fixed-QP rendering of `mpi_enc_test`'s defaults: the bit-rate fields
    /// stay at the harness values because fixed QP does not use them, and the
    /// quality band collapses onto the picture QP.
    pub fn fixed_qp(qp: u32, igop: u32, bps_target: i64) -> Self {
        Self {
            model_name: "default",
            mode_name: "fixqp",
            bps_target,
            bps_min: 1500000,
            bps_max: 2500000,
            igop: igop as i64,
            init_quality: qp as i32,
            min_quality: qp as i32,
            max_quality: qp as i32,
            min_i_quality: qp as i32,
            max_i_quality: qp as i32,
            i_quality_delta: 0,
        }
    }
}

/// `mpi_enc_test`'s default target rate: 30 fps at one byte per pixel every eight,
/// i.e. `w * h * 30 / 8`. 320x240 prints 288000, 640x480 1152000, 1280x720
/// 3456000 - all three from the oracle's own SEI lines.
pub fn default_bps_target(width: usize, height: usize, fps: u32) -> i64 {
    (width as i64) * (height as i64) * (fps as i64) / 8
}

/// `update_rc_cfg_log`'s one line, newline included.
pub fn rc_cfg_line(rc: &RcCfgLine<'_>) -> String {
    let mut out = String::with_capacity(96);
    let _ = write!(
        out,
        "{}:{}-b:{}[{}:{}]-g:{}-q:{}:[{}:{}]:[{}:{}]:{}\n",
        rc.model_name,
        rc.mode_name,
        rc.bps_target,
        rc.bps_min,
        rc.bps_max,
        rc.igop,
        rc.init_quality,
        rc.min_quality,
        rc.max_quality,
        rc.min_i_quality,
        rc.max_i_quality,
        rc.i_quality_delta,
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rate_control_line_is_the_oracle_text() {
        let line = rc_cfg_line(&RcCfgLine::fixed_qp(26, 60, 288000));
        assert_eq!(
            line,
            "default:fixqp-b:288000[1500000:2500000]-g:60-q:26:[26:26]:[26:26]:0\n"
        );
        assert_eq!(line.len(), 68);
        assert_eq!(default_bps_target(1280, 720, 30), 3_456_000);
        assert_eq!(default_bps_target(640, 480, 30), 1_152_000);
    }

    #[test]
    fn the_sei_nal_has_type_39_and_the_payload_length_included() {
        let nal = user_data_unregistered(&VERSION_UUID, VERSION_TEXT.as_bytes());
        assert_eq!(&nal[..6], &[0, 0, 0, 1, 39 << 1, 0x01]);
        assert_eq!(nal[6], SEI_TYPE_USER_DATA_UNREGISTERED);
        assert_eq!(nal[7] as usize, 16 + VERSION_TEXT.len());
        assert_eq!(&nal[8..24], &VERSION_UUID);
        assert_eq!(nal.last(), Some(&0x80));
    }

    #[test]
    fn recovery_point_matches_the_c_gdr_prefix() {
        assert_eq!(
            recovery_point(3),
            vec![0, 0, 0, 1, 0x4e, 0x01, SEI_TYPE_RECOVERY_POINT, 1, 0x35]
        );
    }
}
