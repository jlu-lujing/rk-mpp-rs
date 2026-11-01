//! SEI NAL units the encoder puts in front of an IDR.
//!
//! `mpp/codec/enc/h264/h264e_api_v2.c` calls `h264e_add_sei` on every IDR when
//! the SEI mode is at least `MPP_ENC_SEI_MODE_ONE_SEQ`, which is what
//! `mpp_enc_v2.cpp` initialises it to - so an MPP-produced stream carries two
//! unregistered-user-data SEI NALs ahead of each IDR: one with the library's
//! build string under `uuid_version`, one with a rendering of the rate-control
//! configuration under `uuid_rc_cfg`. A replacement encoder that omits them is
//! not producing the same access unit, which is why this is here rather than
//! being filtered out of the comparisons.
//!
//! The framing is `h264e_sei_to_packet`: four-byte start code, a NAL header with
//! `nal_ref_idc` disposable and type 6, then payload type and size in 0xff
//! continuation form, the 16-byte UUID, then the payload, then
//! `rbsp_trailing_bits`. Bytes are escaped as the RBSP becomes a NAL, so a
//! `00 00` followed by something at or below 3 grows a `03` - which a UUID
//! containing zero runs will hit, and a payload of free text will not.

use std::fmt::Write as _;

/// `uuid_version` of `mpp_enc_impl.cpp`.
pub const UUID_VERSION: [u8; 16] = [
    0x3d, 0x07, 0x6d, 0x45, 0x73, 0x0f, 0x41, 0xa8, 0xb1, 0xc4, 0x25, 0xd7, 0x97, 0x6b, 0xf1, 0xac,
];

/// `uuid_rc_cfg` of `mpp_enc_impl.cpp`.
pub const UUID_RC_CFG: [u8; 16] = [
    0xd7, 0xdc, 0x03, 0xc3, 0xc5, 0x6f, 0x40, 0xe0, 0x8e, 0xa9, 0x17, 0x1a, 0xd2, 0xef, 0x5e, 0x23,
];

/// `uuid_refresh_cfg`: the ASCII `recovery_point` with two zero bytes, which is
/// why the escaping rule below is not hypothetical.
pub const UUID_REFRESH_CFG: [u8; 16] = [
    0x72, 0x65, 0x63, 0x6f, 0x76, 0x65, 0x72, 0x79, 0x5f, 0x70, 0x6f, 0x69, 0x6e, 0x74, 0x00, 0x00,
];

/// `H264_SEI_USER_DATA_UNREGISTERED`.
pub const SEI_USER_DATA_UNREGISTERED: u32 = 5;
/// `H264_NALU_TYPE_SEI`.
const NAL_UNIT_TYPE_SEI: u8 = 6;
/// `H264_NALU_PRIORITY_DISPOSABLE`.
const NAL_PRIORITY_DISPOSABLE: u8 = 0;

/// Append `byte`, holding back the two-zero run that decides whether the next
/// byte needs an emulation prevention `03` in front of it.
fn put_escaped(nal: &mut Vec<u8>, zero_run: &mut usize, byte: u8) {
    if *zero_run >= 2 && byte <= 3 {
        nal.push(3);
        *zero_run = 0;
    }
    nal.push(byte);
    *zero_run = if byte == 0 { *zero_run + 1 } else { 0 };
}

/// One unregistered-user-data SEI as a complete Annex-B NAL.
///
/// `payload` sits after the UUID, matching `h264e_sei_to_packet(pkt, len,
/// H264_SEI_USER_DATA_UNREGISTERED, uuid, data, size)`.
pub fn user_data(uuid: &[u8; 16], payload: &[u8]) -> Vec<u8> {
    let rbsp_len = 1 + 1 + 1 + 16 + payload.len();
    let mut rbsp = Vec::with_capacity(rbsp_len);
    // forbidden_zero_bit zero, nal_ref_idc `disposable` (zero), nal_unit_type SEI.
    // Composing these as a bitwise or of the *values* - which once produced a 7
    // here - silently turns the SEI into an IDR slice header to any decoder.
    let forbidden_zero_bit: u8 = 0;
    let nal_ref_idc: u8 = NAL_PRIORITY_DISPOSABLE;
    rbsp.push(forbidden_zero_bit << 7 | nal_ref_idc << 5 | NAL_UNIT_TYPE_SEI);
    let mut types: Vec<u8> = Vec::new();
    let mut remaining = SEI_USER_DATA_UNREGISTERED;
    while remaining >= 255 {
        types.push(0xff);
        remaining -= 255;
    }
    types.push(remaining as u8);
    rbsp.extend_from_slice(&types);
    let payload_size = payload.len() + 16;
    let mut sizes: Vec<u8> = Vec::new();
    let mut remaining = payload_size;
    while remaining >= 255 {
        sizes.push(0xff);
        remaining -= 255;
    }
    sizes.push(remaining as u8);
    rbsp.extend_from_slice(&sizes);
    rbsp.extend_from_slice(uuid);
    rbsp.extend_from_slice(payload);
    rbsp.push(0x80); // rbsp_trailing_bits

    let mut nal = Vec::with_capacity(rbsp.len() + 8);
    nal.extend_from_slice(&[0, 0, 0, 1]);
    let mut zero_run = 0usize;
    for byte in rbsp {
        put_escaped(&mut nal, &mut zero_run, byte);
    }
    nal
}

/// The rate-control configuration line `update_rc_cfg_log` renders into the
/// `uuid_rc_cfg` SEI: the RC model's name, then the mode, bit rate target and
/// bounds, the intra GOP, and the QP bounds. Under fixed QP the model collapses
/// every bound onto `init_quality`, which is what `mpi_enc_test -rc 2` shows.
pub fn rc_cfg_line(rc: &RcCfgLine) -> String {
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

/// The values `update_rc_cfg_log` prints. They are plain configuration rather
/// than anything the hardware computes, so a caller that configures the same
/// numbers gets the same SEI byte for byte.
pub struct RcCfgLine {
    /// `brief->name` from the RC model, `default` for the built-in one.
    pub model_name: &'static str,
    /// `name_of_rc_mode[mode]`; `fixqp` for fixed QP.
    pub mode_name: &'static str,
    pub bps_target: i64,
    pub bps_min: i64,
    pub bps_max: i64,
    /// `usr_cfg.igop`, the intra GOP.
    pub igop: i64,
    pub init_quality: i32,
    pub min_quality: i32,
    pub max_quality: i32,
    pub min_i_quality: i32,
    pub max_i_quality: i32,
    pub i_quality_delta: i32,
}

impl RcCfgLine {
    /// The fixed-QP rendering: every quality bound collapses onto `init_quality`
    /// and the I/QP delta is zero, as the reference's own output shows for
    /// `-rc 2 -qc 26:...`.
    pub fn fixqp(qp: i32, bps_target: i64, bps_min: i64, bps_max: i64, igop: i64) -> Self {
        Self {
            model_name: "default",
            mode_name: "fixqp",
            bps_target,
            bps_min,
            bps_max,
            igop,
            init_quality: qp,
            min_quality: qp,
            max_quality: qp,
            min_i_quality: qp,
            max_i_quality: qp,
            i_quality_delta: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(text: &str) -> Vec<u8> {
        (0..text.len() / 2)
            .map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap())
            .collect()
    }

    /// The `uuid_rc_cfg` SEI of a `mpi_enc_test -rc 2 -qc 26:...` run on t6,
    /// captured with `tools/mpp_ioctl_trace.c` and read back out of the C
    /// encoder's own output file.
    const C_RC_CFG_SEI: &str = "00000001060554d7dc03c3c56f40e08ea9171ad2ef5e2364656661756c743a\
        66697871702d623a3238383030305b313530303030303a323530303030305d2d673a36302d713a32363a5b32\
        363a32365d3a5b32363a32365d3a300a80";

    /// The same capture at 64x64, where the harness' derived target is 15360
    /// rather than 288000: `64 * 64 / 8 * 30`. Same bounds, because fixed QP never
    /// sets them and `h264e_api_v2.c`'s defaults stand.
    const C_RC_CFG_SEI_64: &str = "00000001060553d7dc03c3c56f40e08ea9171ad2ef5e23\
        64656661756c743a66697871702d623a31353336305b313530303030303a323530303030305d\
        2d673a36302d713a32363a5b32363a32365d3a5b32363a32365d3a300a80";

    /// The same run's `uuid_version` SEI. Its payload is that build's git log
    /// line, so it is used here as a framing witness with the string supplied
    /// explicitly - the point is the framing, not a claim that a Rust build can
    /// carry MPP's commit id.
    const C_VERSION_SEI: &str = "0000000106056e3d076d45730f41a8b1c425d7976bf1ac\
        38303935376432363620617574686f723a2059616e646f6e67204c696e2020323032342d30342d3235206669\
        785b68616c5f68323635655f766570753538305d3a206669782072656720636f6e6669672065727220666f72\
        20327061737380";

    #[test]
    fn rc_cfg_sei_matches_the_c_encoder_byte_for_byte() {
        let line = rc_cfg_line(&RcCfgLine::fixqp(26, 288_000, 1_500_000, 2_500_000, 60));
        assert_eq!(
            line,
            "default:fixqp-b:288000[1500000:2500000]-g:60-q:26:[26:26]:[26:26]:0\n"
        );
        assert_eq!(
            user_data(&UUID_RC_CFG, line.as_bytes()),
            unhex(C_RC_CFG_SEI),
            "the rc cfg SEI must be the NAL MPP writes"
        );
    }

    /// The derived target is per picture size, so it is the part of the SEI that a
    /// single hard-coded fixture would have frozen wrongly. Both numbers are what
    /// the C encoder wrote on t6, not what the formula is guessed to say.
    #[test]
    fn the_derived_bit_rate_target_reaches_the_sei() {
        use super::super::encoder::default_bps_target;

        assert_eq!(default_bps_target(320, 240, 30), 288_000);
        assert_eq!(default_bps_target(64, 64, 30), 15_360);
        assert_eq!(
            user_data(
                &UUID_RC_CFG,
                rc_cfg_line(&RcCfgLine::fixqp(
                    26,
                    default_bps_target(64, 64, 30),
                    1_500_000,
                    2_500_000,
                    60
                ))
                .as_bytes()
            ),
            unhex(C_RC_CFG_SEI_64),
            "the 64x64 rc cfg SEI as the C encoder wrote it"
        );
    }

    #[test]
    fn sei_framing_matches_the_c_encoder() {
        let payload = "80957d266 author: Yandong Lin  2024-04-25 fix[hal_h265e_vepu580]: \
                       fix reg config err for 2pass"
            .as_bytes();
        assert_eq!(payload.len(), 94, "the captured payload length");
        assert_eq!(
            user_data(&UUID_VERSION, payload),
            unhex(C_VERSION_SEI),
            "start code, NAL header, 0xff-continuation type and size, uuid, payload, trailing"
        );
    }

    #[test]
    fn long_payloads_extend_type_and_size_in_ff_bytes() {
        // 300 bytes of payload plus the 16-byte uuid is 316, one 0xff and a 0x3c.
        let payload = vec![b'x'; 300];
        let nal = user_data(&UUID_VERSION, &payload);
        assert_eq!(nal[4], 6, "nal_unit_type SEI");
        assert_eq!(nal[5], 5, "payload type");
        assert_eq!(
            &nal[6..8],
            [0xff, 61],
            "payload size 316 is one 0xff then 316 - 255"
        );
        assert_eq!(nal[8 + 16 + 300], 0x80, "rbsp trailing after the payload");
    }

    #[test]
    fn zero_runs_are_escaped_as_the_rbsp_becomes_a_nal() {
        // A payload carrying `00 00 01` would otherwise hand the decoder what
        // looks like a start code, so a 03 has to appear between the zeros and
        // the 01 once the RBSP becomes a NAL.
        let mut payload = b"ab".to_vec();
        payload.extend_from_slice(&[0, 0, 1]);
        let nal = user_data(&UUID_REFRESH_CFG, &payload);
        assert!(
            nal.windows(4).any(|w| w == [0, 0, 3, 1]),
            "no emulation prevention byte before 01: {nal:02x?}"
        );
        assert_eq!(
            nal.iter().filter(|&&b| b == 3).count(),
            1,
            "exactly the inserted prevention byte - this uuid carries no 03 of its own"
        );
    }

    #[test]
    fn trailing_zeros_of_the_uuid_do_not_need_escaping() {
        // `recovery_point` ends in two zero bytes and the byte after them is the
        // 0x80 trailing bit, above the threshold - so no 03 goes in, and the NAL
        // is exactly header plus uuid plus payload plus trailing.
        let nal = user_data(&UUID_REFRESH_CFG, b"xy");
        assert!(
            !nal.windows(3).any(|w| w == [0, 0, 3]),
            "unexpected escape: {nal:02x?}"
        );
        assert_eq!(nal.len(), 4 + 1 + 1 + 1 + 16 + 2 + 1);
    }
}
