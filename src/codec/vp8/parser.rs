//! VP8 picture syntax (`mpp/codec/dec/vp8/vp8d_parser.c`).
//!
//! The hardware decodes the macroblock layer itself, so this layer only has to
//! reproduce what the C parser hands the HAL: the frame tag, the loop filter and
//! quantiser state, the coefficient probabilities as they stand at the end of
//! the header, the position the first partition resumes at, and the offsets of
//! the DCT partitions.
//!
//! Only the VP8 bitstream format is reachable here. `vp8d_parser_init` fixes
//! `decMode` to `VP8HWD_VP8` and nothing else ever writes it, so the VP7 and
//! WebP branches of the C decoder - including `vp7_header_parser`, which returns
//! a protocol error for every feature it is offered - are dead code in the
//! reference we are matching. A VP7 stream is rejected instead of decoded
//! differently.
//!
//! Three C behaviours are load bearing and are copied rather than corrected:
//!
//! * The entropy-probability backup is taken at the moment
//!   `refresh_entropy_probs` is read, before the update loops, and restored
//!   after the picture reaches the hardware. The decoder therefore decodes the
//!   current picture with the *updated* probabilities and the next one starts
//!   from the *backup*.
//! * `prob_mb_skip_false`, `prob_intra`, `prob_ref_last` and `prob_ref_golden`
//!   are not part of the entropy set and are not reset by a key frame; a key
//!   frame keeps whatever the previous inter frame left there.
//! * The probability tables are read with the ordinary tree coder while the rest
//!   of the header uses the 50/50 coder, so the two refill paths (`do { } while`
//!   against a single refill) both have to be exact.

use std::fmt;

use super::tables::{
    COEFF_UPDATE_PROBS, DEFAULT_COEFF_PROBS, MV_UPDATE_PROBS, VP8_DEFAULT_MV_PROBS,
};

/// `VP8_KEY_FRAME_START_CODE`
pub const KEY_FRAME_START_CODE: u32 = 0x9d012a;
/// `VP8_MV_PROBS_PER_COMPONENT`
pub const MV_PROBS_PER_COMPONENT: usize = 19;
/// `MAX_NBR_OF_SEGMENTS`
pub const MAX_SEGMENTS: usize = 4;
/// `MAX_NBR_OF_DCT_PARTITIONS`
pub const MAX_DCT_PARTITIONS: usize = 8;

/// `vp8EntropyProbs_t`. Copyable because the C decoder memcpys the whole set
/// twice per picture, once into the backup and once back out of it.
pub type CoefficientProbs = [[[[u8; 11]; 3]; 8]; 4];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntropyProbs {
    /// `probLuma16x16PredMode`
    pub luma16x16_pred: [u8; 4],
    /// `probChromaPredMode`
    pub chroma_pred: [u8; 3],
    /// `probMvContext`
    pub mv: [[u8; MV_PROBS_PER_COMPONENT]; 2],
    /// `probCoeffs`
    pub coeffs: CoefficientProbs,
}

#[derive(Debug)]
pub enum Vp8Error {
    /// A fixed-width field or a table index ran past the packet.
    Truncated,
    /// The frame tag or the key-frame start code does not parse.
    Unsupported(&'static str),
}

impl fmt::Display for Vp8Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Vp8Error::Truncated => write!(f, "bitstream truncated"),
            Vp8Error::Unsupported(what) => write!(f, "unsupported: {what}"),
        }
    }
}

impl Vp8Error {
    /// Whether the *stream* policy may skip this packet and try the next one.
    ///
    /// Every syntax error the VP8 front end raises is decided while reading one
    /// picture's own packet: the reference parses each task from its frame tag,
    /// and its parser state survives a failed task the same way ours does
    /// (`found_key_frame` only ever becomes true, the probability state is
    /// re-seeded by the next key frame). A packet we could not parse therefore
    /// says nothing about the following one, so both error kinds are
    /// per-packet failures, and only a container error or a broken hardware
    /// job stops the stream. That is the whole policy, documented here because
    /// `Decoder::decode_stream` implements exactly it and nothing else may
    /// invent a different one.
    pub fn recoverable(&self) -> bool {
        true
    }
}

impl std::error::Error for Vp8Error {}

/// `vpBoolCoder_t`, including both refill paths.
#[derive(Debug)]
pub struct BoolDecoder<'a> {
    buffer: &'a [u8],
    /// `pos` counts bytes consumed past the start of `buffer`.
    pub pos: usize,
    /// `count`: bits of the byte last read that are still unconsumed.
    pub count: u32,
    pub range: u32,
    pub value: u32,
    /// `streamEndPos`: the byte count this coder may read.
    pub stream_end_pos: usize,
    pub stream_error: bool,
}

impl<'a> BoolDecoder<'a> {
    /// `vp8hwdBoolStart`: range 255 and four bytes prefetched into the top of a
    /// 32-bit window, which is why `pos` starts at 4 rather than 1.
    pub fn start(buffer: &'a [u8]) -> Self {
        let byte = |index: usize| u32::from(buffer.get(index).copied().unwrap_or(0));
        Self {
            buffer,
            pos: 4,
            count: 8,
            range: 255,
            value: (byte(0) << 24) | (byte(1) << 16) | (byte(2) << 8) | byte(3),
            stream_end_pos: buffer.len(),
            stream_error: 4 > buffer.len(),
        }
    }

    /// `vp8hwdDecodeBool`: the tree coder, with the refill in a `do { } while`
    /// so one call can consume several bytes when the range keeps shrinking.
    pub fn decode_bool(&mut self, probability: u32) -> u32 {
        let mut bit = 0u32;
        let mut count = self.count;
        let mut range = self.range;
        let mut value = self.value;

        let split = 1 + (((range - 1) * probability) >> 8);
        let bigsplit = split << 24;
        range = split;

        if value >= bigsplit {
            range = self.range - split;
            value = value.wrapping_sub(bigsplit);
            bit = 1;
        }

        if range >= 0x80 {
            self.value = value;
            self.range = range;
            return bit;
        }

        loop {
            range = range.wrapping_add(range);
            value = value.wrapping_add(value);
            count = count.wrapping_sub(1);
            if count == 0 {
                if self.pos >= self.stream_end_pos {
                    self.stream_error = true;
                    break;
                }
                count = 8;
                value |= u32::from(self.buffer[self.pos]);
                self.pos += 1;
            }
            if range >= 0x80 {
                break;
            }
        }

        self.count = count;
        self.value = value;
        self.range = range;
        bit
    }

    /// `vp8hwdDecodeBool128`: the 50/50 coder, which refills at most once.
    pub fn decode_bool128(&mut self) -> u32 {
        let mut bit = 0u32;
        let mut count = self.count;
        let mut range = self.range;
        let mut value = self.value;

        let split = (range + 1) >> 1;
        let bigsplit = split << 24;
        range = split;

        if value >= bigsplit {
            range = self.range - split;
            value = value.wrapping_sub(bigsplit);
            bit = 1;
        }

        if range >= 0x80 {
            self.value = value;
            self.range = range;
            return bit;
        }

        range <<= 1;
        value <<= 1;
        count = count.wrapping_sub(1);
        if count == 0 {
            if self.pos >= self.stream_end_pos {
                self.stream_error = true;
                // The reference returns zero here without storing anything, so
                // the coder stays where it was.
                return 0;
            }
            count = 8;
            value |= u32::from(self.buffer[self.pos]);
            self.pos += 1;
        }

        self.count = count;
        self.value = value;
        self.range = range;
        bit
    }

    /// `vp8hwdReadBits`: most significant bit first, one 50/50 call per bit.
    pub fn read_bits(&mut self, bits: u32) -> u32 {
        let mut value = 0u32;
        for shift in (0..bits).rev() {
            value |= self.decode_bool128() << shift;
        }
        value
    }

    /// `DecodeQuantizerDelta`
    fn read_delta(&mut self) -> i8 {
        if self.decode_bool128() == 0 {
            return 0;
        }
        let value = self.read_bits(4);
        if self.decode_bool128() == 1 {
            -i8::try_from(value).unwrap_or(i8::MAX)
        } else {
            i8::try_from(value).unwrap_or(i8::MAX)
        }
    }

    /// `vp8d_convert_to_syntx`: the bit position the macroblock layer starts at.
    /// The base moves to the 8-byte aligned byte at or before it, so `key_frame`
    /// has to be accounted for - the seven key-frame header bytes are inside the
    /// packet but in front of this coder.
    pub fn stream_position(&self, key_frame: bool) -> (u32, u32) {
        let mut bit = self.pos * 8 + (8 - self.count as usize);
        if key_frame {
            bit += 7 * 8;
        }
        let bit = u32::try_from(bit).unwrap_or(u32::MAX);
        // The reference writes the same expression twice, once rounded down and
        // once masked back to the bit inside that byte, which is what makes the
        // pair land in the six-bit register fields.
        let mut byte = bit / 8;
        let mut start_bit = (byte & 0x07) * 8;
        byte &= !0x07;
        start_bit += bit & 0x7;
        (byte, start_bit)
    }
}

/// One coded picture: everything `DXVA_PicParams_VP8` carries, plus the buffer
/// bookkeeping the reference derives from the frame tag.
#[derive(Clone, Debug)]
pub struct Picture {
    pub key_frame: bool,
    /// `version`: 0 selects the eight-tap filter bank, 3 turns off the extra
    /// motion-vector range bit.
    pub version: u8,
    pub show_frame: bool,
    pub width: u32,
    pub height: u32,

    pub segmentation_enabled: bool,
    pub segmentation_map_update: bool,
    /// `update_mb_segmentation_data`: absolute rather than delta.
    pub segment_feature_mode: bool,
    pub segment_qp: [i8; MAX_SEGMENTS],
    pub segment_loopfilter: [i8; MAX_SEGMENTS],
    pub segment_tree_probs: [u8; 3],

    pub loop_filter_type: u8,
    pub loop_filter_level: u8,
    pub sharpness: u8,
    pub mode_ref_lf_delta_enabled: bool,
    pub ref_lf_deltas: [i8; 4],
    pub mode_lf_deltas: [i8; 4],

    pub base_qindex: u8,
    pub y1dc_delta_q: i8,
    pub y1ac_delta_q: i8,
    pub y2dc_delta_q: i8,
    pub y2ac_delta_q: i8,
    pub uvdc_delta_q: i8,
    pub uvac_delta_q: i8,

    pub mb_no_coeff_skip: bool,
    pub prob_skip_false: u8,
    pub prob_intra: u8,
    pub prob_last: u8,
    pub prob_golden: u8,

    pub ref_frame_sign_bias_golden: u8,
    pub ref_frame_sign_bias_altref: u8,

    pub refresh_golden: bool,
    pub refresh_alternate: bool,
    pub refresh_last: bool,
    /// `copy_buffer_to_golden` / `..._alternate`: 1 takes the last frame, 2 the
    /// other one, 0 leaves the buffer alone.
    pub copy_buffer_to_golden: u8,
    pub copy_buffer_to_alternate: u8,

    /// The probabilities this picture is decoded with, before any rollback.
    pub entropy: EntropyProbs,

    pub bool_value: u8,
    pub bool_range: u8,
    /// Byte offset added to the stream buffer, 8-byte aligned.
    pub stream_start_offset: u32,
    /// Bit position inside that aligned base.
    pub stream_start_bit: u32,
    /// `streamEndPos`: bytes left after the frame tag and the key-frame header.
    pub stream_end_pos: u32,
    pub frame_tag_size: u32,
    pub offset_to_dct_parts: u32,
    pub log2_dct_partitions: u8,
    pub dct_partition_offsets: [u32; MAX_DCT_PARTITIONS],
}

/// The reference's `VP8DParserContext_t` state that survives a picture.
#[derive(Clone, Debug)]
pub struct Parser {
    entropy: EntropyProbs,
    /// `entropyLast`
    entropy_backup: EntropyProbs,
    width: u32,
    height: u32,
    segmentation_enabled: bool,
    segmentation_map_update: bool,
    segment_feature_mode: bool,
    segment_qp: [i8; MAX_SEGMENTS],
    segment_loopfilter: [i8; MAX_SEGMENTS],
    segment_tree_probs: [u8; 3],
    loop_filter_type: u8,
    loop_filter_level: u8,
    loop_filter_sharpness: u8,
    mode_ref_lf_enabled: bool,
    ref_lf_deltas: [i8; MAX_SEGMENTS],
    mode_lf_deltas: [i8; MAX_SEGMENTS],
    qp_y_ac: u8,
    qp_y_dc: i8,
    qp_y2_dc: i8,
    qp_y2_ac: i8,
    qp_ch_dc: i8,
    qp_ch_ac: i8,
    /// Not in the entropy set and not reset by a key frame; see the module note.
    prob_mb_skip_false: u8,
    prob_intra: u8,
    prob_ref_last: u8,
    prob_ref_golden: u8,
    /// `needKeyFrame`: pictures ahead of the first key frame are dropped.
    found_key_frame: bool,
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

impl Parser {
    pub fn new() -> Self {
        Self {
            entropy: EntropyProbs {
                luma16x16_pred: [0; 4],
                chroma_pred: [0; 3],
                mv: [[0; MV_PROBS_PER_COMPONENT]; 2],
                coeffs: [[[[0; 11]; 3]; 8]; 4],
            },
            entropy_backup: EntropyProbs {
                luma16x16_pred: [0; 4],
                chroma_pred: [0; 3],
                mv: [[0; MV_PROBS_PER_COMPONENT]; 2],
                coeffs: [[[[0; 11]; 3]; 8]; 4],
            },
            width: 0,
            height: 0,
            segmentation_enabled: false,
            segmentation_map_update: false,
            segment_feature_mode: false,
            segment_qp: [0; MAX_SEGMENTS],
            segment_loopfilter: [0; MAX_SEGMENTS],
            segment_tree_probs: [0; 3],
            loop_filter_type: 0,
            loop_filter_level: 0,
            loop_filter_sharpness: 0,
            mode_ref_lf_enabled: false,
            ref_lf_deltas: [0; MAX_SEGMENTS],
            mode_lf_deltas: [0; MAX_SEGMENTS],
            qp_y_ac: 0,
            qp_y_dc: 0,
            qp_y2_dc: 0,
            qp_y2_ac: 0,
            qp_ch_dc: 0,
            qp_ch_ac: 0,
            prob_mb_skip_false: 0,
            prob_intra: 0,
            prob_ref_last: 0,
            prob_ref_golden: 0,
            found_key_frame: false,
        }
    }

    /// `vp8hwdResetProbs`: only the entropy set and the loop filter deltas.
    fn reset_probs(&mut self) {
        self.entropy.luma16x16_pred = [112, 86, 140, 37];
        self.entropy.chroma_pred = [162, 101, 204];
        self.ref_lf_deltas = [0; MAX_SEGMENTS];
        self.mode_lf_deltas = [0; MAX_SEGMENTS];
        self.entropy.mv = VP8_DEFAULT_MV_PROBS;
        self.entropy.coeffs = DEFAULT_COEFF_PROBS;
    }

    /// `vp8hwdDecodeCoeffUpdate`.
    ///
    /// Indexed rather than iterator-nested: the table is four dimensions deep -
    /// plane, token tree, context, token - and all four indices appear on both
    /// sides of the assignment.
    #[allow(clippy::needless_range_loop)]
    fn decode_coeff_update(&mut self, coder: &mut BoolDecoder<'_>) {
        for plane in 0..4 {
            for class in 0..8 {
                for context in 0..3 {
                    for token in 0..11 {
                        let probability =
                            u32::from(COEFF_UPDATE_PROBS[plane][class][context][token]);
                        if coder.decode_bool(probability) == 1 {
                            self.entropy.coeffs[plane][class][context][token] =
                                coder.read_bits(8) as u8;
                        }
                    }
                }
            }
        }
    }

    /// `decoder_frame_header` plus `vp8_header_parser` plus
    /// `vp8hwdSetPartitionOffsets`, and the syntax hand-off of
    /// `vp8d_convert_to_syntx`.
    ///
    /// `Ok(None)` is the reference's "no found key frame" path: the packet is
    /// well formed, it just cannot be decoded yet.
    pub fn parse_picture(&mut self, data: &[u8]) -> Result<Option<Picture>, Vp8Error> {
        if data.len() < 3 {
            return Err(Vp8Error::Truncated);
        }
        let tag = [data[0], data[1], data[2]];
        let key_frame = tag[0] & 1 == 0;
        // `version` 0 picks the eight-tap filter bank and 3 turns off the extra
        // motion-vector range bit; both are register-side decisions, so every
        // VP8 profile is decodeable here.
        let version = (tag[0] >> 1) & 7;
        let show_frame = (tag[0] >> 4) & 1 == 1;
        let offset_to_dct_parts =
            u32::from(tag[0] >> 5) | (u32::from(tag[1]) << 3) | (u32::from(tag[2]) << 11);
        let frame_tag_size = 3u32;

        if !key_frame && !self.found_key_frame {
            return Ok(None);
        }
        self.found_key_frame = true;
        if key_frame {
            self.reset_probs();
        }

        let mut payload = &data[frame_tag_size as usize..];
        let mut width = self.width;
        let mut height = self.height;
        if key_frame {
            if payload.len() < 7 {
                return Err(Vp8Error::Truncated);
            }
            let start_code = (u32::from(payload[0]) << 16)
                | (u32::from(payload[1]) << 8)
                | u32::from(payload[2]);
            if start_code != KEY_FRAME_START_CODE {
                return Err(Vp8Error::Unsupported("missing key-frame start code"));
            }
            let coded_width = u32::from(payload[3]) | (u32::from(payload[4]) << 8);
            let coded_height = u32::from(payload[5]) | (u32::from(payload[6]) << 8);
            width = coded_width & 0x3fff;
            height = coded_height & 0x3fff;
            // `scaledWidth` and `scaledHeight` are the only place the reference
            // records a key-frame scale factor, and no register ever reads them:
            // a scaled stream would be decoded at full size. Refuse instead.
            if coded_width >> 14 != 0 || coded_height >> 14 != 0 {
                return Err(Vp8Error::Unsupported("key-frame scale factors"));
            }
            payload = &payload[7..];
        }

        let mut coder = BoolDecoder::start(payload);
        let header = self.decode_header(&mut coder, key_frame)?;
        let stream_end_pos = u32::try_from(coder.stream_end_pos).unwrap_or(u32::MAX);
        let bool_value = ((coder.value >> 24) & 0xff) as u8;
        let bool_range = (coder.range & 0xff) as u8;
        let (stream_start_offset, stream_start_bit) = coder.stream_position(key_frame);

        let log2_dct_partitions = header.log2_dct_partitions;
        let mut dct_partition_offsets = [0u32; MAX_DCT_PARTITIONS];
        {
            let mut base_offset = frame_tag_size + offset_to_dct_parts;
            let size_fields = (1u32 << log2_dct_partitions) - 1;
            base_offset += 3 * size_fields;
            // A damaged packet can claim a DCT-partition table past its own
            // end. With more than one partition C dereferences three bytes
            // outside the copy of the packet before checking anything; the
            // deterministic version is a stream error at the same point. With
            // a single partition there is no table to read and C only applies
            // its final length check, which is reproduced below.
            let partition_area_start = frame_tag_size as usize
                + offset_to_dct_parts as usize
                + if key_frame { 7 } else { 0 };
            let mut stream = data.get(partition_area_start..);
            if size_fields > 0 && stream.is_none() {
                return Err(Vp8Error::Truncated);
            }
            let mut offset = 0u32;
            for slot in dct_partition_offsets.iter_mut().take(size_fields as usize) {
                *slot = base_offset + offset;
                let Some(rest) = stream else {
                    return Err(Vp8Error::Truncated);
                };
                let Some(part) = rest.get(..3) else {
                    return Err(Vp8Error::Truncated);
                };
                offset +=
                    u32::from(part[0]) | (u32::from(part[1]) << 8) | (u32::from(part[2]) << 16);
                stream = rest.get(3..);
            }
            let last = size_fields as usize;
            dct_partition_offsets[last] = base_offset + offset;
            // `vp8hwdSetPartitionOffsets` returns MPP_ERR_STREAM unless the
            // last partition still starts inside the packet.
            if dct_partition_offsets[last] >= data.len() as u32 {
                return Err(Vp8Error::Truncated);
            }
        }

        self.width = width;
        self.height = height;

        let picture = Picture {
            key_frame,
            version,
            show_frame,
            width,
            height,
            segmentation_enabled: self.segmentation_enabled,
            segmentation_map_update: self.segmentation_map_update,
            segment_feature_mode: self.segment_feature_mode,
            segment_qp: self.segment_qp,
            segment_loopfilter: self.segment_loopfilter,
            segment_tree_probs: self.segment_tree_probs,
            loop_filter_type: self.loop_filter_type,
            loop_filter_level: self.loop_filter_level,
            sharpness: self.loop_filter_sharpness,
            mode_ref_lf_delta_enabled: self.mode_ref_lf_enabled,
            ref_lf_deltas: self.ref_lf_deltas,
            mode_lf_deltas: self.mode_lf_deltas,
            base_qindex: self.qp_y_ac,
            y1dc_delta_q: self.qp_y_dc,
            y1ac_delta_q: self.qp_y_ac as i8,
            y2dc_delta_q: self.qp_y2_dc,
            y2ac_delta_q: self.qp_y2_ac,
            uvdc_delta_q: self.qp_ch_dc,
            uvac_delta_q: self.qp_ch_ac,
            mb_no_coeff_skip: header.mb_no_coeff_skip,
            prob_skip_false: self.prob_mb_skip_false,
            prob_intra: self.prob_intra,
            prob_last: self.prob_ref_last,
            prob_golden: self.prob_ref_golden,
            ref_frame_sign_bias_golden: header.ref_frame_sign_bias[0],
            ref_frame_sign_bias_altref: header.ref_frame_sign_bias[1],
            refresh_golden: header.refresh_golden,
            refresh_alternate: header.refresh_alternate,
            refresh_last: header.refresh_last,
            copy_buffer_to_golden: header.copy_buffer_to_golden,
            copy_buffer_to_alternate: header.copy_buffer_to_alternate,
            entropy: self.entropy,
            bool_value,
            bool_range,
            stream_start_offset,
            stream_start_bit,
            stream_end_pos,
            frame_tag_size,
            offset_to_dct_parts,
            log2_dct_partitions,
            dct_partition_offsets,
        };

        // The reference rolls the backup back only after the picture reached the
        // hardware, which is why `picture.entropy` is the updated set.
        if !header.refresh_entropy_probs {
            self.entropy = self.entropy_backup;
        }
        Ok(Some(picture))
    }

    /// `vp8_header_parser`, on a borrowed coder.
    fn decode_header(
        &mut self,
        coder: &mut BoolDecoder<'_>,
        key_frame: bool,
    ) -> Result<Header, Vp8Error> {
        let mut header = Header {
            log2_dct_partitions: 0,
            mb_no_coeff_skip: false,
            refresh_entropy_probs: false,
            refresh_golden: false,
            refresh_alternate: false,
            refresh_last: false,
            copy_buffer_to_golden: 0,
            copy_buffer_to_alternate: 0,
            ref_frame_sign_bias: [0; 2],
        };
        if key_frame {
            // `colorSpace` and `clamping` are read and dropped, as in C.
            coder.decode_bool128();
            coder.decode_bool128();
        }
        self.segmentation_enabled = coder.decode_bool128() == 1;
        self.segmentation_map_update = false;
        if self.segmentation_enabled {
            self.segmentation_map_update = coder.decode_bool128() == 1;
            if coder.decode_bool128() == 1 {
                self.segment_feature_mode = coder.decode_bool128() == 1;
                self.segment_qp = [0; MAX_SEGMENTS];
                self.segment_loopfilter = [0; MAX_SEGMENTS];
                for segment in 0..MAX_SEGMENTS {
                    if coder.decode_bool128() == 1 {
                        let value = coder.read_bits(7);
                        self.segment_qp[segment] = if coder.decode_bool128() == 1 {
                            -i8::try_from(value).unwrap_or(i8::MAX)
                        } else {
                            i8::try_from(value).unwrap_or(i8::MAX)
                        };
                    }
                }
                for segment in 0..MAX_SEGMENTS {
                    if coder.decode_bool128() == 1 {
                        let value = coder.read_bits(6);
                        self.segment_loopfilter[segment] = if coder.decode_bool128() == 1 {
                            -i8::try_from(value).unwrap_or(i8::MAX)
                        } else {
                            i8::try_from(value).unwrap_or(i8::MAX)
                        };
                    }
                }
            }
            if self.segmentation_map_update {
                self.segment_tree_probs = [255; 3];
                for probability in 0..3 {
                    if coder.decode_bool128() == 1 {
                        self.segment_tree_probs[probability] = coder.read_bits(8) as u8;
                    }
                }
            }
            if coder.stream_error {
                return Err(Vp8Error::Truncated);
            }
        }

        self.loop_filter_type = coder.decode_bool128() as u8;
        self.loop_filter_level = coder.read_bits(6) as u8;
        self.loop_filter_sharpness = coder.read_bits(3) as u8;
        self.mode_ref_lf_enabled = coder.decode_bool128() == 1;
        if self.mode_ref_lf_enabled && coder.decode_bool128() == 1 {
            for delta in 0..MAX_SEGMENTS {
                if coder.decode_bool128() == 1 {
                    let value = coder.read_bits(6);
                    self.ref_lf_deltas[delta] = if coder.decode_bool128() == 1 {
                        -i8::try_from(value).unwrap_or(i8::MAX)
                    } else {
                        i8::try_from(value).unwrap_or(i8::MAX)
                    };
                }
            }
            for delta in 0..MAX_SEGMENTS {
                if coder.decode_bool128() == 1 {
                    let value = coder.read_bits(6);
                    self.mode_lf_deltas[delta] = if coder.decode_bool128() == 1 {
                        -i8::try_from(value).unwrap_or(i8::MAX)
                    } else {
                        i8::try_from(value).unwrap_or(i8::MAX)
                    };
                }
            }
        }
        if coder.stream_error {
            return Err(Vp8Error::Truncated);
        }

        header.log2_dct_partitions = coder.read_bits(2) as u8;
        self.qp_y_ac = coder.read_bits(7) as u8;
        self.qp_y_dc = coder.read_delta();
        self.qp_y2_dc = coder.read_delta();
        self.qp_y2_ac = coder.read_delta();
        self.qp_ch_dc = coder.read_delta();
        self.qp_ch_ac = coder.read_delta();

        if key_frame {
            header.refresh_golden = true;
            header.refresh_alternate = true;
            header.copy_buffer_to_golden = 0;
            header.copy_buffer_to_alternate = 0;
            header.refresh_entropy_probs = coder.decode_bool128() == 1;
            header.ref_frame_sign_bias = [0; 2];
            header.refresh_last = true;
        } else {
            header.refresh_golden = coder.decode_bool128() == 1;
            header.refresh_alternate = coder.decode_bool128() == 1;
            header.copy_buffer_to_golden = if header.refresh_golden {
                0
            } else {
                coder.read_bits(2) as u8
            };
            header.copy_buffer_to_alternate = if header.refresh_alternate {
                0
            } else {
                coder.read_bits(2) as u8
            };
            header.ref_frame_sign_bias[0] = coder.decode_bool128() as u8;
            header.ref_frame_sign_bias[1] = coder.decode_bool128() as u8;
            header.refresh_entropy_probs = coder.decode_bool128() == 1;
            header.refresh_last = coder.decode_bool128() == 1;
        }

        // The backup point, exactly where the reference takes it.
        if !header.refresh_entropy_probs {
            self.entropy_backup = self.entropy;
        }

        self.decode_coeff_update(coder);
        header.mb_no_coeff_skip = coder.decode_bool128() == 1;
        if header.mb_no_coeff_skip {
            self.prob_mb_skip_false = coder.read_bits(8) as u8;
        }
        if !key_frame {
            self.prob_intra = coder.read_bits(8) as u8;
            self.prob_ref_last = coder.read_bits(8) as u8;
            self.prob_ref_golden = coder.read_bits(8) as u8;
            if coder.decode_bool128() == 1 {
                for mode in 0..4 {
                    self.entropy.luma16x16_pred[mode] = coder.read_bits(8) as u8;
                }
            }
            if coder.decode_bool128() == 1 {
                for mode in 0..3 {
                    self.entropy.chroma_pred[mode] = coder.read_bits(8) as u8;
                }
            }
            for (update_probs, current_probs) in MV_UPDATE_PROBS.iter().zip(&mut self.entropy.mv) {
                for (probability, current) in update_probs.iter().zip(current_probs) {
                    if coder.decode_bool(u32::from(*probability)) == 1 {
                        let value = coder.read_bits(7);
                        *current = if value == 0 { 1 } else { (value << 1) as u8 };
                    }
                }
            }
        }
        if coder.stream_error {
            return Err(Vp8Error::Truncated);
        }
        Ok(header)
    }
}

/// The fields `vp8_header_parser` derives that are not persistent parser state.
struct Header {
    log2_dct_partitions: u8,
    mb_no_coeff_skip: bool,
    refresh_entropy_probs: bool,
    refresh_golden: bool,
    refresh_alternate: bool,
    refresh_last: bool,
    copy_buffer_to_golden: u8,
    copy_buffer_to_alternate: u8,
    ref_frame_sign_bias: [u8; 2],
}

/// IVF (`DKIF`) container: the fixtures and the C test binary both use it.
pub struct IvfFrame<'a> {
    pub timestamp: u64,
    pub data: &'a [u8],
}

pub fn parse_ivf(data: &[u8]) -> Result<Vec<IvfFrame<'_>>, Vp8Error> {
    if data.len() < 32 || &data[..4] != b"DKIF" {
        return Err(Vp8Error::Unsupported("not an IVF file"));
    }
    let header_size = u16::from_le_bytes([data[6], data[7]]) as usize;
    if header_size < 32 || header_size > data.len() || &data[8..12] != b"VP80" {
        return Err(Vp8Error::Unsupported("IVF header is not VP80"));
    }
    let declared = u32::from_le_bytes(data[24..28].try_into().unwrap()) as usize;
    let mut frames = Vec::new();
    let mut offset = header_size;
    while offset < data.len() {
        if offset + 12 > data.len() {
            return Err(Vp8Error::Truncated);
        }
        let size = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        let timestamp = u64::from_le_bytes(data[offset + 4..offset + 12].try_into().unwrap());
        offset += 12;
        if size == 0 || offset + size > data.len() {
            return Err(Vp8Error::Truncated);
        }
        frames.push(IvfFrame {
            timestamp,
            data: &data[offset..offset + size],
        });
        offset += size;
    }
    if declared != 0 && frames.len() != declared {
        return Err(Vp8Error::Unsupported("IVF frame count does not match"));
    }
    Ok(frames)
}

#[cfg(test)]
mod tests {
    use super::{parse_ivf, Parser};

    /// Every fixture, with the *packet* count its IVF header declares. They were
    /// made by the same `gen_streams.sh` family the other codecs use: an I and P
    /// pair, a segmentation stream, an alternating-quant stream, an odd size, and
    /// two auto-alt-ref streams.
    ///
    /// The alt-ref pair is named after its *display* count, not its packet count:
    /// `vp8_320x240_24_altref` carries 26 packets, of which two are hidden
    /// reference frames, and `vp8_320x240_12_altref` turned out to need no hidden
    /// frame at all.
    const FIXTURES: [(&str, usize); 5] = [
        ("tests/data/vp8_320x240_12_ip.ivf", 12),
        ("tests/data/vp8_320x240_12_parts.ivf", 12),
        ("tests/data/vp8_320x240_12_aq.ivf", 12),
        ("tests/data/vp8_150x100_9_ip.ivf", 9),
        ("tests/data/vp8_320x240_24_altref.ivf", 26),
    ];

    fn parse_stream(path: &str) -> Vec<super::Picture> {
        let data = std::fs::read(path).expect("test fixture");
        let mut parser = Parser::new();
        parse_ivf(&data)
            .expect("parse container")
            .iter()
            .map(|frame| parser.parse_picture(frame.data))
            .map(|result| result.expect("parse picture").expect("key frame present"))
            .collect()
    }

    #[test]
    fn every_fixture_starts_at_a_key_frame() {
        for (path, declared) in FIXTURES {
            let pictures = parse_stream(path);
            assert_eq!(pictures.len(), declared, "{path} packet count");
            assert!(pictures[0].key_frame, "{path} opens with a key frame");
        }
    }

    /// The stream re-closes its group half way through, which is the only case
    /// that resets the reference buffers mid-stream.
    #[test]
    fn a_stream_can_carry_several_key_frames() {
        let pictures = parse_stream("tests/data/vp8_320x240_24_altref.ivf");
        let keys: Vec<usize> = pictures
            .iter()
            .enumerate()
            .filter(|(_, picture)| picture.key_frame)
            .map(|(index, _)| index + 1)
            .collect();
        assert_eq!(keys, vec![1, 14], "two 13-packet groups");
    }

    /// The key-frame header carries the visible size; the coded size that follows
    /// is that rounded up to macroblocks, and 150x100 is the fixture that makes
    /// the two differ.
    #[test]
    fn inter_pictures_keep_the_key_frame_size() {
        let pictures = parse_stream("tests/data/vp8_150x100_9_ip.ivf");
        assert_eq!(pictures[0].width, 150);
        assert_eq!(pictures[0].height, 100);
        assert!(
            pictures.iter().all(|p| (p.width, p.height) == (150, 100)),
            "no picture changes the size"
        );
        assert_eq!(((150 + 15) & !15, (100 + 15) & !15), (160, 112));
    }

    /// `sw51` and `sw124` of the key frame in `vp8_320x240_12_ip.ivf`, read out of
    /// the C HAL's own register dump on t6: the primary stream is 7161 bytes from
    /// the aligned base and the first partition 465, with no extra partitions.
    /// Those two lengths are made of `stream_end_pos`, `offset_to_dct_parts`, the
    /// frame tag, the seven key-frame header bytes and the aligned base, so they
    /// pin every one of them at once.
    #[test]
    fn the_first_partition_position_matches_c() {
        let pictures = parse_stream("tests/data/vp8_320x240_12_ip.ivf");
        let key = &pictures[0];
        assert_eq!(key.frame_tag_size, 3);
        assert_eq!(key.offset_to_dct_parts, 894);
        assert_eq!(key.stream_end_pos, 8055);
        assert_eq!(key.log2_dct_partitions, 0);
        assert_eq!(key.stream_start_offset, 440);
        assert_eq!(key.stream_start_bit, 63);
        // One partition, so its offset is the base the partition lengths count from.
        assert_eq!(key.dct_partition_offsets[0], 897);

        let second = &pictures[1];
        assert_eq!(second.offset_to_dct_parts, 226);
        assert_eq!(second.stream_end_pos, 904);
        assert_eq!(second.stream_start_offset, 64);
        assert_eq!(second.stream_start_bit, 19);
    }

    /// The `parts` fixture runs the segment feature on every picture, always as a
    /// delta of the picture level, which is the case the register file builds by
    /// clamping four loop filter levels and four quantisers into range.
    ///
    /// Two of the three cases the C HAL distinguishes have no fixture: absolute
    /// segment features (`update_mb_segmentation_data`), and a map that stays
    /// enabled without being refreshed. Neither does any fixture carry more than
    /// one DCT partition. All three are recorded as open in `ROADMAP.md` rather
    /// than claimed.
    #[test]
    fn segmentation_covers_the_delta_form() {
        let pictures = parse_stream("tests/data/vp8_320x240_12_parts.ivf");
        assert!(
            pictures.iter().all(|picture| picture.segmentation_enabled),
            "the whole stream keeps the feature on"
        );
        assert!(
            pictures
                .iter()
                .all(|picture| picture.segmentation_map_update),
            "and refreshes the map every picture"
        );
        assert!(
            pictures.iter().all(|picture| !picture.segment_feature_mode),
            "always as a delta, never as an absolute value"
        );
        // libvpx spends the feature on the quantiser here and leaves the loop
        // filter at the picture level, so `sw151` and `sw130` see the delta while
        // `sw129` keeps the same value in every segment.
        assert!(
            pictures
                .iter()
                .any(|picture| picture.segment_qp.iter().any(|value| *value != 0)),
            "the segment features move the quantiser"
        );
        assert!(
            pictures
                .iter()
                .all(|picture| picture.segment_loopfilter == [0; 4]),
            "and leave the loop filter at the picture level"
        );

        let unsegmented = parse_stream("tests/data/vp8_320x240_12_ip.ivf");
        assert!(
            unsegmented
                .iter()
                .all(|picture| !picture.segmentation_enabled),
            "the other streams exercise the picture-level path alone"
        );
        assert!(
            unsegmented
                .iter()
                .all(|picture| picture.log2_dct_partitions == 0),
            "one DCT partition, so `sw_coeffs_part_am` is zero throughout"
        );
    }

    /// Auto-alt-ref writes a reference frame that is never displayed. The C
    /// decoder decodes it and never enqueues it, which is what makes the two
    /// alt-ref fixtures differ in picture count from their packet count.
    #[test]
    fn alt_ref_streams_hide_a_reference_frame() {
        let pictures = parse_stream("tests/data/vp8_320x240_24_altref.ivf");
        let hidden: Vec<usize> = pictures
            .iter()
            .enumerate()
            .filter(|(_, picture)| !picture.show_frame)
            .map(|(index, _)| index + 1)
            .collect();
        assert_eq!(hidden, vec![2, 15], "one hidden reference frame per group");
        assert_eq!(pictures[1].width, 320, "and it still carries a size");

        let shown = pictures.iter().filter(|picture| picture.show_frame).count();
        assert_eq!(shown, 24, "26 packets, 24 display pictures");
    }

    /// A picture ahead of the first key frame cannot be decoded: the reference
    /// drops it with "no found key frame" rather than failing the stream.
    #[test]
    fn pictures_ahead_of_the_first_key_frame_are_dropped() {
        let data = std::fs::read("tests/data/vp8_320x240_12_ip.ivf").expect("test fixture");
        let frames = parse_ivf(&data).expect("parse container");
        let mut parser = Parser::new();
        assert!(
            parser
                .parse_picture(frames[1].data)
                .expect("no error")
                .is_none(),
            "a P picture before any key frame is dropped"
        );
        assert!(
            parser
                .parse_picture(frames[0].data)
                .expect("no error")
                .is_some(),
            "and the key frame that follows is decoded"
        );
    }

    /// The frame tag's three version bits are carried through untouched for all
    /// eight values. C reads them with the same `(pbase[0] >> 1) & 7` and never
    /// branches on them in `vp8_header_parser` - the version-conditional code
    /// lives in the dead `vp7_header_parser` and in the register file - so a
    /// version value is syntax-opaque here and decides registers only.
    #[test]
    fn every_version_value_survives_the_parser_untouched() {
        let data = std::fs::read("tests/data/vp8_320x240_12_ip.ivf").expect("test fixture");
        let frames = parse_ivf(&data).expect("parse container");
        for version in 0..8u8 {
            let mut packet = frames[0].data.to_vec();
            packet[0] = (packet[0] & 0xf1) | (version << 1);
            let picture = Parser::new()
                .parse_picture(&packet)
                .expect("patching the version cannot break the syntax")
                .expect("a key frame is a key frame");
            assert_eq!(picture.version, version, "version {version} round-trip");
        }
    }

    /// Profile 1 and profile 3 streams parsed here produce the same syntax as
    /// profile 0 - byte-identical pictures except the version field. The two
    /// fixtures are `vp8_320x240_12_ip.ivf` with every frame tag's version bits
    /// rewritten; nothing else changes, because VP8's profile is *only* the
    /// version field: chroma sampling stays 4:2:0 and the header syntax does not
    /// branch on it, in C or here. Whether the hardware reproduces the profile
    /// 1/3 IDCT precision is a board question the profile scripts answer; these
    /// fixtures lock the front end.
    #[test]
    fn profile_one_and_three_parse_exactly_like_profile_zero() {
        let baseline = parse_stream("tests/data/vp8_320x240_12_ip.ivf");
        for (path, profile) in [
            ("tests/data/vp8/vp8_p1_320x240_12_ip.ivf", 1u8),
            ("tests/data/vp8/vp8_p3_320x240_12_ip.ivf", 3u8),
        ] {
            let pictures = parse_stream(path);
            assert_eq!(pictures.len(), baseline.len(), "{path} packet count");
            for (index, (patched, original)) in pictures.iter().zip(&baseline).enumerate() {
                assert_eq!(patched.version, profile, "{path} packet {index} version");
                let mut equal = patched.clone();
                equal.version = original.version;
                assert_eq!(
                    format!("{equal:?}"),
                    format!("{original:?}"),
                    "{path} packet {index} must be identical to profile 0 apart from the version"
                );
            }
        }
    }

    /// A truncated packet reports the stream error at the same place the C parser
    /// checks it, instead of reading past the buffer.
    #[test]
    fn a_truncated_header_reports_a_stream_error() {
        let data = std::fs::read("tests/data/vp8_320x240_12_ip.ivf").expect("test fixture");
        let frame = parse_ivf(&data).expect("parse container").remove(0);
        // Frame tag plus key-frame header and a handful of probability bytes.
        let cut = &frame.data[..20];
        let error = Parser::new()
            .parse_picture(cut)
            .expect_err("a quarter of a key frame cannot parse");
        assert!(
            matches!(error, super::Vp8Error::Truncated),
            "unexpected error {error:?}"
        );
    }
}
