//! MPEG-4 Part 2 (ISO/IEC 14496-2) visual object layer syntax.
//!
//! The access-unit rule is the one `mpp_mpg4_parser_split` implements: a unit
//! begins at a `00 00 01 B6` video object plane start code and ends at the next
//! `00 00 01`, three bytes of which are handed to the following unit. Sequence
//! level headers (VOS, VOL, GOP, user data) sit outside every unit and only
//! update parser state, which is why [`parse`] walks the whole stream and hands
//! back a video object layer snapshot with each picture.
//!
//! Field order follows `mpg4d_parser.c` bit for bit, including the places where
//! it deviates from the standard: the video object layer identifier is read into
//! a local that never reaches `vol.ver_id`, so every later decision takes the
//! version-1 branch, and the user data header is sniffed for a DivX build string
//! because that changes how macroblock sizes are programmed.

use std::fmt;

/// `MPG4_VOL_STARTCODE` .. `MPG4_VOL_STOPCODE`: the video object layer, whose
/// three-bit identifier rides in the low bits of the code. The names in
/// `mpg4d_parser.c` invite a double take - the video object *sequence* is 0xB0,
/// the *group of VOPs* is 0xB3, and the video object plane is 0xB6.
pub const CODE_VOL: u8 = 0x20;
pub const CODE_VOL_LAST: u8 = 0x2F;
/// `MPG4_VOS_STARTCODE` .. `MPG4_VOS_STOPCODE`: the visual object sequence,
/// which carries only the profile and level we do not need.
pub const CODE_VOS: u8 = 0xB0;
pub const CODE_VOS_LAST: u8 = 0xB1;
pub const CODE_USER_DATA: u8 = 0xB2;
pub const CODE_GOP: u8 = 0xB3;
/// `MPG4_VISUAL_OBJ_STARTCODE`. The C path has no case for it, so neither do we.
pub const CODE_VISUAL_OBJ: u8 = 0xB5;
pub const CODE_VOP: u8 = 0xB6;

/// `MPEG4VOPType`. MPEG-2 numbers picture types from 1, MPEG-4 from 0.
pub const VOP_I: u32 = 0;
pub const VOP_P: u32 = 1;
pub const VOP_B: u32 = 2;
pub const VOP_S: u32 = 3;
/// `vop_coded == 0`: the picture is skipped and never reaches the hardware.
pub const VOP_N: u32 = 4;

/// `MPEG4_VIDOBJLAY_SHAPE_RECTANGULAR`
const SHAPE_RECTANGULAR: u32 = 0;
/// `MPEG4_VIDOBJLAY_SHAPE_BINARY_ONLY`
const SHAPE_BINARY_ONLY: u32 = 2;
/// `MPEG4_VIDOBJLAY_AR_EXTPAR`
const ASPECT_EXT_PAR: u32 = 15;

/// Hardware ceiling of the vdpu2 MPEG-4 path (`mppg4d_parse_vol_header`).
pub const MAX_WIDTH: u32 = 1920;
pub const MAX_HEIGHT: u32 = 1088;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mpg4Error {
    Truncated,
    Unsupported(&'static str),
}

impl fmt::Display for Mpg4Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Mpg4Error::Truncated => write!(f, "bitstream truncated"),
            Mpg4Error::Unsupported(s) => write!(f, "unsupported: {s}"),
        }
    }
}

impl std::error::Error for Mpg4Error {}

/// MSB-first bit reader. MPEG-4 keeps `00 00 01` out of coded data with escape
/// codes rather than emulation-prevention bytes, so no unescaping is needed.
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }

    fn bit(&mut self) -> Result<u32, Mpg4Error> {
        if self.pos >= self.data.len() * 8 {
            return Err(Mpg4Error::Truncated);
        }
        let b = (self.data[self.pos / 8] >> (7 - (self.pos % 8))) & 1;
        self.pos += 1;
        Ok(u32::from(b))
    }

    pub fn bits(&mut self, n: u32) -> Result<u32, Mpg4Error> {
        if n == 0 {
            return Ok(0);
        }
        if n > 32 || self.data.len() * 8 - self.pos < n as usize {
            return Err(Mpg4Error::Truncated);
        }
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | self.bit()?;
        }
        Ok(v)
    }

    pub fn flag(&mut self) -> Result<bool, Mpg4Error> {
        Ok(self.bits(1)? == 1)
    }

    pub fn skip(&mut self, n: u32) -> Result<(), Mpg4Error> {
        if self.data.len() * 8 - self.pos < n as usize {
            return Err(Mpg4Error::Truncated);
        }
        self.pos += n as usize;
        Ok(())
    }

    /// Bit position from the start of the buffer, which is what the hardware
    /// `stream_start_word` is measured in.
    pub fn bit_pos(&self) -> usize {
        self.pos
    }
}

/// `log2bin` of the C parser: the number of bits needed to write `value`.
fn bits_for(value: u32) -> u32 {
    let mut n = 0;
    let mut v = value;
    while v != 0 {
        v >>= 1;
        n += 1;
    }
    n
}

/// The complexity estimation header, kept because it sits in front of every VOP
/// header field the hardware needs when it is not disabled.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Estimation {
    pub method: u32,
    opaque: bool,
    transparent: bool,
    intra_cae: bool,
    inter_cae: bool,
    no_update: bool,
    upsampling: bool,
    intra_blocks: bool,
    inter_blocks: bool,
    inter4v_blocks: bool,
    not_coded_blocks: bool,
    dct_coefs: bool,
    dct_lines: bool,
    vlc_symbols: bool,
    vlc_bits: bool,
    apm: bool,
    npm: bool,
    interpolate_mc_q: bool,
    forw_back_mc_q: bool,
    halfpel2: bool,
    halfpel4: bool,
    sadct: bool,
    quarterpel: bool,
}

impl Estimation {
    /// `read_vol_complexity_estimation_header`.
    fn parse(r: &mut Reader<'_>) -> Result<Self, Mpg4Error> {
        let mut e = Estimation {
            method: r.bits(2)?,
            ..Estimation::default()
        };
        if e.method == 0 || e.method == 1 {
            if !r.flag()? {
                // shape_complexity_estimation_disable
                e.opaque = r.flag()?;
                e.transparent = r.flag()?;
                e.intra_cae = r.flag()?;
                e.inter_cae = r.flag()?;
                e.no_update = r.flag()?;
                e.upsampling = r.flag()?;
            }
            if !r.flag()? {
                // texture_complexity_estimation_set_1_disable
                e.intra_blocks = r.flag()?;
                e.inter_blocks = r.flag()?;
                e.inter4v_blocks = r.flag()?;
                e.not_coded_blocks = r.flag()?;
            }
        }
        r.skip(1)?;
        if !r.flag()? {
            // texture_complexity_estimation_set_2_disable
            e.dct_coefs = r.flag()?;
            e.dct_lines = r.flag()?;
            e.vlc_symbols = r.flag()?;
            e.vlc_bits = r.flag()?;
        }
        if !r.flag()? {
            // motion_compensation_complexity_disable
            e.apm = r.flag()?;
            e.npm = r.flag()?;
            e.interpolate_mc_q = r.flag()?;
            e.forw_back_mc_q = r.flag()?;
            e.halfpel2 = r.flag()?;
            e.halfpel4 = r.flag()?;
        }
        r.skip(1)?;
        if e.method == 1 && !r.flag()? {
            // version2_complexity_estimation_disable
            e.sadct = r.flag()?;
            e.quarterpel = r.flag()?;
        }
        Ok(e)
    }

    /// `read_vop_complexity_estimation_header`: eight bits per enabled counter.
    fn skip_vop_header(&self, r: &mut Reader<'_>, coding_type: u32) -> Result<(), Mpg4Error> {
        if self.method != 0 && self.method != 1 {
            return Ok(());
        }
        let mut counters = 0u32;
        match coding_type {
            VOP_I => {
                for on in [
                    self.opaque,
                    self.transparent,
                    self.intra_cae,
                    self.inter_cae,
                    self.no_update,
                    self.upsampling,
                    self.intra_blocks,
                    self.not_coded_blocks,
                    self.dct_coefs,
                    self.dct_lines,
                    self.vlc_symbols,
                    self.vlc_bits,
                    self.sadct,
                ] {
                    if on {
                        counters += 1;
                    }
                }
            }
            VOP_P => {
                for on in [
                    self.opaque,
                    self.transparent,
                    self.intra_cae,
                    self.inter_cae,
                    self.no_update,
                    self.upsampling,
                    self.intra_blocks,
                    self.not_coded_blocks,
                    self.dct_coefs,
                    self.dct_lines,
                    self.vlc_symbols,
                    self.vlc_bits,
                    self.inter_blocks,
                    self.inter4v_blocks,
                    self.apm,
                    self.npm,
                    self.forw_back_mc_q,
                    self.halfpel2,
                    self.halfpel4,
                    self.sadct,
                    self.quarterpel,
                ] {
                    if on {
                        counters += 1;
                    }
                }
            }
            VOP_B => {
                for on in [
                    self.opaque,
                    self.transparent,
                    self.intra_cae,
                    self.inter_cae,
                    self.no_update,
                    self.upsampling,
                    self.intra_blocks,
                    self.not_coded_blocks,
                    self.dct_coefs,
                    self.dct_lines,
                    self.vlc_symbols,
                    self.vlc_bits,
                    self.inter_blocks,
                    self.inter4v_blocks,
                    self.apm,
                    self.npm,
                    self.forw_back_mc_q,
                    self.halfpel2,
                    self.halfpel4,
                    self.interpolate_mc_q,
                    self.sadct,
                    self.quarterpel,
                ] {
                    if on {
                        counters += 1;
                    }
                }
            }
            _ => {}
        }
        r.skip(counters * 8)
    }
}

/// One video object layer header, plus the sequence state the VOP headers and
/// the register file are built from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vol {
    /// `video_object_layer_verid` as the C parser keeps it: the header's own
    /// verid goes into a local only, so this stays at its initial 1.
    pub ver_id: u32,
    pub shape: u32,
    pub width: u32,
    pub height: u32,
    pub time_inc_resolution: u32,
    pub time_inc_bits: u32,
    pub interlaced: bool,
    pub low_delay: bool,
    pub sprite_enable: u32,
    pub quant_bits: u32,
    pub quant_type: bool,
    pub quarter_sample: bool,
    pub complexity_estimation_disable: bool,
    pub resync_marker_disable: bool,
    pub data_partitioned: bool,
    pub newpred_enable: bool,
    pub reduced_resolution_enable: bool,
    pub load_intra_matrix: bool,
    pub load_inter_matrix: bool,
    pub intra_matrix: [u8; 64],
    pub inter_matrix: [u8; 64],
}

impl Default for Vol {
    fn default() -> Self {
        Vol {
            ver_id: 1,
            shape: SHAPE_RECTANGULAR,
            width: 0,
            height: 0,
            time_inc_resolution: 0,
            time_inc_bits: 1,
            interlaced: false,
            low_delay: false,
            sprite_enable: 0,
            quant_bits: 5,
            quant_type: false,
            quarter_sample: false,
            complexity_estimation_disable: true,
            resync_marker_disable: true,
            data_partitioned: false,
            newpred_enable: false,
            reduced_resolution_enable: false,
            load_intra_matrix: false,
            load_inter_matrix: false,
            intra_matrix: [0; 64],
            inter_matrix: [0; 64],
        }
    }
}

impl Vol {
    /// `mpg4d_parse_vol_header`.
    fn parse(
        r: &mut Reader<'_>,
        state: &Vol,
        estimation: &mut Estimation,
    ) -> Result<Self, Mpg4Error> {
        let mut vol = state.clone();
        r.skip(1)?; // random_accessible_vol
        let vo_type = r.bits(8)?; // video_object_type_indication

        let ver_id = if r.flag()? {
            // video_object_layer_identifier
            let verid = r.bits(4)?;
            r.skip(3)?; // video_object_layer_priority
            verid
        } else {
            state.ver_id
        };

        if r.bits(4)? == ASPECT_EXT_PAR {
            r.skip(8)?; // par_width
            r.skip(8)?; // par_height
        }

        let vol_control_parameters = r.flag()?;
        if vol_control_parameters {
            r.skip(2)?; // chroma_format, always 4:2:0 for this hardware
            vol.low_delay = r.flag()?;
            if r.flag()? {
                // vbv_parameters
                r.skip(15)?;
                r.skip(1)?;
                r.skip(15)?;
                r.skip(1)?;
                r.skip(15)?;
                r.skip(1)?;
                r.skip(3)?;
                r.skip(11)?;
                r.skip(1)?;
                r.skip(15)?;
                r.skip(1)?;
            }
        } else {
            vol.low_delay = false;
        }

        // The C parser refuses streams that look like divx4 or old xvid, which it
        // spots from an empty object type and no control parameters.
        if vo_type == 0 && !vol_control_parameters {
            return Err(Mpg4Error::Unsupported("divx4 or old xvid stream"));
        }

        vol.shape = r.bits(2)?;
        if vol.shape != SHAPE_RECTANGULAR {
            return Err(Mpg4Error::Unsupported("non-rectangular shape"));
        }

        r.skip(1)?; // marker
        vol.time_inc_resolution = r.bits(16)?;
        vol.time_inc_bits = if vol.time_inc_resolution > 0 {
            bits_for(vol.time_inc_resolution - 1).max(1)
        } else {
            1
        };

        r.skip(1)?; // marker
        if r.flag()? {
            r.skip(vol.time_inc_bits)?; // fixed_vop_time_increment
        }

        if vol.shape != SHAPE_BINARY_ONLY {
            if vol.shape == SHAPE_RECTANGULAR {
                r.skip(1)?; // marker
                vol.width = r.bits(13)?;
                r.skip(1)?; // marker
                vol.height = r.bits(13)?;
                r.skip(1)?; // marker
                if vol.width > MAX_WIDTH || vol.height > MAX_HEIGHT {
                    return Err(Mpg4Error::Unsupported("larger than 1920x1088"));
                }
            }

            vol.interlaced = r.flag()?;
            r.skip(1)?; // obmc_disable

            vol.sprite_enable = r.bits(if ver_id == 1 { 1 } else { 2 })?;
            if vol.sprite_enable != 0 {
                return Err(Mpg4Error::Unsupported("GMC sprite"));
            }

            if ver_id != 1 && vol.shape != SHAPE_RECTANGULAR {
                r.skip(1)?; // sadct_disable
            }

            if r.flag()? {
                // not_8_bit
                vol.quant_bits = r.bits(4)?;
                r.skip(4)?; // bits_per_pixel
            } else {
                vol.quant_bits = 5;
            }

            vol.quant_type = r.flag()?;
            if vol.quant_type {
                vol.load_intra_matrix = r.flag()?;
                if vol.load_intra_matrix {
                    vol.intra_matrix = parse_matrix(r)?;
                }
                vol.load_inter_matrix = r.flag()?;
                if vol.load_inter_matrix {
                    vol.inter_matrix = parse_matrix(r)?;
                }
            } else {
                vol.load_intra_matrix = false;
                vol.load_inter_matrix = false;
            }

            vol.quarter_sample = ver_id != 1 && r.flag()?;
            vol.complexity_estimation_disable = r.flag()?;
            if !vol.complexity_estimation_disable {
                *estimation = Estimation::parse(r)?;
            }

            vol.resync_marker_disable = r.flag()?;
            if r.flag()? {
                // data_partitioned; the reversible_vlc flag follows it and the
                // hardware path has no partitioned-packet support.
                r.skip(1)?;
                vol.data_partitioned = true;
            }

            if ver_id != 1 {
                vol.newpred_enable = r.flag()?;
                if vol.newpred_enable {
                    r.skip(2)?; // requested_upstream_message_type
                    r.skip(1)?; // newpred_segment_type
                }
                vol.reduced_resolution_enable = r.flag()?;
            } else {
                vol.newpred_enable = false;
                vol.reduced_resolution_enable = false;
            }

            if r.flag()? {
                return Err(Mpg4Error::Unsupported("layered video (scalability)"));
            }
            vol.ver_id = state.ver_id;
        }

        Ok(vol)
    }

    /// Macroblock count of the coded size, which is what the hardware decodes.
    pub fn mb_width(&self) -> u32 {
        self.width.div_ceil(16)
    }

    pub fn mb_height(&self) -> u32 {
        self.height.div_ceil(16)
    }
}

/// `mpg4d_parse_matrix`: bytes until a zero, then that value to the end.
fn parse_matrix(r: &mut Reader<'_>) -> Result<[u8; 64], Mpg4Error> {
    let mut matrix = [0u8; 64];
    let mut index = 0;
    let mut last;
    loop {
        last = r.bits(8)?;
        if index < 64 {
            matrix[index] = last as u8;
        }
        index += 1;
        if last == 0 {
            break;
        }
        if index >= 64 {
            return Err(Mpg4Error::Truncated);
        }
    }
    for slot in matrix.iter_mut().skip(index.saturating_sub(1)) {
        *slot = last as u8;
    }
    Ok(matrix)
}

/// One video object plane header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vop {
    pub coding_type: u32,
    pub quant: u32,
    pub fcode_forward: u32,
    pub fcode_backward: u32,
    pub rounding: bool,
    pub intra_dc_vlc_thr: u32,
    pub top_field_first: bool,
    pub alternate_vertical_scan: bool,
    /// Bits from the start of the access unit to the end of this header, which
    /// is the position the hardware starts decoding at.
    pub header_bits: usize,
    /// B picture interpolation ratio numerator, `time_bp`.
    pub time_bp: u32,
    /// Distance between the two references, `time_pp`.
    pub time_pp: u32,
}

/// One picture and everything the register file needs with it.
#[derive(Debug, Clone)]
pub struct Picture {
    pub vop: Vop,
    /// Video object layer in force for this picture.
    pub vol: Vol,
    /// Access-unit bytes: the `00 00 01 B6` prefix through the byte before the
    /// next start code.
    pub au: (usize, usize),
    /// DivX build detected in the user data header, 0 when there was none.
    pub custom_version: u32,
}

#[derive(Debug, Clone)]
pub enum Item {
    Vol(Vol),
    Picture(Picture),
}

/// Parser state that outlives one header: the time counters and the DivX sniff.
struct State {
    time_base: i64,
    last_time_base: i64,
    time: i64,
    last_non_b_time: i64,
    time_pp: i64,
    custom_version: u32,
}

impl State {
    fn new() -> Self {
        State {
            time_base: 0,
            last_time_base: 0,
            time: 0,
            last_non_b_time: 0,
            time_pp: 0,
            custom_version: 0,
        }
    }
}

/// Positions of every `00 00 01` start code and the code byte that follows it.
pub fn start_codes(data: &[u8]) -> Vec<(usize, u8)> {
    let mut found = Vec::new();
    let mut i = 0;
    while i + 3 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            found.push((i, data[i + 3]));
            i += 4;
        } else {
            i += 1;
        }
    }
    found
}

/// Walk an elementary stream into headers and pictures.
pub fn parse(data: &[u8]) -> Result<Vec<Item>, Mpg4Error> {
    let mut state = StreamState::new();
    parse_with_state(data, &mut state)
}

/// State that survives packet boundaries for the incremental MPP object path.
pub struct StreamState {
    vol: Vol,
    estimation: Estimation,
    state: State,
}

impl StreamState {
    pub fn new() -> Self {
        Self {
            vol: Vol::default(),
            estimation: Estimation::default(),
            state: State::new(),
        }
    }
}

/// Parse a new chunk while carrying VOL, timing and custom-version state from
/// earlier chunks.
pub fn parse_with_state(data: &[u8], stream: &mut StreamState) -> Result<Vec<Item>, Mpg4Error> {
    let codes = start_codes(data);
    let StreamState {
        vol,
        estimation,
        state,
    } = stream;
    let mut items = Vec::new();

    for (index, (start, code)) in codes.iter().enumerate() {
        let start = *start;
        let next = codes
            .get(index + 1)
            .map(|(next, _)| *next)
            .unwrap_or(data.len());
        match *code {
            CODE_VOL..=CODE_VOL_LAST => {
                let mut reader = Reader::new(&data[start + 4..next]);
                *vol = Vol::parse(&mut reader, vol, &mut *estimation)?;
                items.push(Item::Vol(vol.clone()));
            }
            CODE_USER_DATA => {
                state.custom_version = parse_user_data(
                    &mut Reader::new(&data[start + 4..next]),
                    state.custom_version,
                );
            }
            CODE_VOS..=CODE_VOS_LAST | CODE_VISUAL_OBJ | CODE_GOP => {
                // time_code, closed_gov, broken_link: no register value needs them.
            }
            CODE_VOP => {
                // The access unit runs to the next start code and the header is
                // counted from the `00 00 01`, exactly as the C split presents it.
                let mut reader = Reader::new(&data[start..next]);
                let Some(vop) = parse_vop(&mut reader, vol, &mut *estimation, &mut *state)? else {
                    continue;
                };
                items.push(Item::Picture(Picture {
                    vop,
                    vol: vol.clone(),
                    au: (start, next),
                    custom_version: state.custom_version,
                }));
            }
            _ => {}
        }
    }

    Ok(items)
}

/// `mpg4d_parse_user_data`, reduced to what changes hardware behaviour: the DivX
/// build string sets the macroblock offset mode and packed mode.
fn parse_user_data(r: &mut Reader<'_>, mut custom_version: u32) -> u32 {
    let mut text = [0u8; 256];
    let mut len = 0;
    while len < 255 {
        match r.bits(8) {
            Ok(byte) if byte != 0 => {
                text[len] = byte as u8;
                len += 1;
            }
            _ => break,
        }
    }
    if !text.starts_with(b"DivX") {
        return custom_version;
    }
    let mut j = 4;
    custom_version = if text.get(j).copied().unwrap_or(0) <= b'4' {
        4
    } else {
        5
    };
    while text.get(j).copied().map_or(false, |c| c.is_ascii_digit()) {
        j += 1;
    }
    custom_version
}

/// `mpeg4_parse_vop_header`. `None` is an `N-VOP`, which is not coded and never
/// reaches the hardware.
fn parse_vop(
    r: &mut Reader<'_>,
    vol: &Vol,
    estimation: &mut Estimation,
    state: &mut State,
) -> Result<Option<Vop>, Mpg4Error> {
    if vol.width == 0 || vol.height == 0 {
        return Err(Mpg4Error::Unsupported("picture before any VOL header"));
    }
    r.skip(4 * 8)?; // 00 00 01 B6
    let coding_type = r.bits(2)?;

    let mut time_incr = 0i64;
    while r.flag()? {
        time_incr += 1; // time_base, unary
    }
    r.skip(1)?; // marker
    let time_increment = i64::from(r.bits(vol.time_inc_bits)?);

    let (time, time_bp, time_pp) = if coding_type != VOP_B {
        state.last_time_base = state.time_base;
        state.time_base += time_incr;
        state.time = state.time_base * i64::from(vol.time_inc_resolution) + time_increment;
        state.time_pp = state.time - state.last_non_b_time;
        state.last_non_b_time = state.time;
        (state.time, 0, state.time_pp)
    } else {
        state.time = (state.last_time_base + time_incr) * i64::from(vol.time_inc_resolution)
            + time_increment;
        let time_bp = state.time_pp - (state.last_non_b_time - state.time);
        (state.time, time_bp, state.time_pp)
    };
    let _ = time;

    r.skip(1)?; // marker
    if !r.flag()? {
        // vop_coded == 0, an N-VOP.
        return Ok(None);
    }

    if vol.newpred_enable {
        r.skip((vol.time_inc_bits + 3).min(15))?; // vop_id
        if r.flag()? {
            r.skip((vol.time_inc_bits + 3).min(15))?; // vop_id_for_prediction
        }
        r.skip(1)?; // marker
    }

    let mut rounding = false;
    if coding_type == VOP_P {
        rounding = r.flag()?; // rounding_type
    }

    if vol.reduced_resolution_enable && (coding_type == VOP_P || coding_type == VOP_I) {
        r.skip(1)?; // reduced_resolution_vop_flag
    }

    if !vol.complexity_estimation_disable {
        estimation.skip_vop_header(r, coding_type)?;
    }

    let intra_dc_vlc_thr = r.bits(3)?;
    let mut top_field_first = false;
    let mut alternate_vertical_scan = false;
    if vol.interlaced {
        top_field_first = r.flag()?;
        alternate_vertical_scan = r.flag()?;
    }

    let mut quant = r.bits(vol.quant_bits)?;
    if quant < 1 {
        quant = 1;
    }

    let mut fcode_forward = 0;
    let mut fcode_backward = 0;
    if coding_type != VOP_I {
        fcode_forward = r.bits(3)?;
    }
    if coding_type == VOP_B {
        fcode_backward = r.bits(3)?;
    }

    Ok(Some(Vop {
        coding_type,
        quant,
        fcode_forward,
        fcode_backward,
        rounding,
        intra_dc_vlc_thr,
        top_field_first,
        alternate_vertical_scan,
        header_bits: r.bit_pos(),
        time_bp: time_bp as u32,
        time_pp: time_pp as u32,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_state_carries_vol_and_timing_across_vop_packets() {
        let data = std::fs::read("tests/data/mp4_320x240_12_ibpb.m4v").expect("test fixture");
        let split = start_codes(&data)
            .into_iter()
            .filter(|(_, code)| *code == CODE_VOP)
            .nth(5)
            .expect("sixth VOP")
            .0;
        let full = parse(&data).expect("full parse");
        let mut state = StreamState::new();
        let mut items = parse_with_state(&data[..split], &mut state).expect("first packet");
        items.extend(parse_with_state(&data[split..], &mut state).expect("second packet"));

        let full_pictures: Vec<_> = full
            .into_iter()
            .filter_map(|item| match item {
                Item::Picture(picture) => Some(picture),
                Item::Vol(_) => None,
            })
            .collect();
        let packet_pictures: Vec<_> = items
            .into_iter()
            .filter_map(|item| match item {
                Item::Picture(picture) => Some(picture),
                Item::Vol(_) => None,
            })
            .collect();
        assert_eq!(packet_pictures.len(), full_pictures.len());
        for (packet, full) in packet_pictures.iter().zip(&full_pictures) {
            assert_eq!(packet.vop, full.vop);
            assert_eq!(packet.vol, full.vol);
        }
    }
}
