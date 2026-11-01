use super::compressed::{self, CompressedHeader};
use super::prob::ProbabilityContext;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Vp9Error {
    Truncated {
        bit: usize,
        width: u8,
    },
    BadFrameMarker,
    BadCompressedHeaderMarker,
    BadSyncCode,
    BadSuperframe,
    BadIvf,
    BadHeaderSize {
        header: usize,
        partition: usize,
        packet: usize,
    },
    MissingReference(u8),
    UnsupportedProfile(u8),
    UnsupportedTxMode(u8),
}

impl fmt::Display for Vp9Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Vp9Error::Truncated { bit, width } => {
                write!(f, "bitstream truncated at bit {bit} reading {width} bits")
            }
            Vp9Error::BadFrameMarker => write!(f, "invalid VP9 frame marker"),
            Vp9Error::BadCompressedHeaderMarker => {
                write!(f, "invalid VP9 compressed-header marker")
            }
            Vp9Error::BadSyncCode => write!(f, "invalid VP9 sync code"),
            Vp9Error::BadSuperframe => write!(f, "invalid VP9 superframe index"),
            Vp9Error::BadIvf => write!(f, "invalid or truncated IVF container"),
            Vp9Error::BadHeaderSize {
                header,
                partition,
                packet,
            } => write!(
                f,
                "VP9 header {header} + first partition {partition} exceeds packet {packet}"
            ),
            Vp9Error::MissingReference(idx) => write!(f, "missing VP9 reference frame {idx}"),
            Vp9Error::UnsupportedProfile(profile) => {
                write!(f, "unsupported VP9 profile {profile}")
            }
            Vp9Error::UnsupportedTxMode(mode) => {
                write!(f, "unsupported VP9 transform mode {mode}")
            }
        }
    }
}

impl std::error::Error for Vp9Error {}

impl Vp9Error {
    /// Whether the *stream policy* may skip this packet and try the next one,
    /// the sibling of `Vp8Error::recoverable` and the same rule: every error
    /// below is decided inside one picture's own bits, the parser state after
    /// it is still the state the reference's parser would have, so the packet
    /// says nothing about its successors. `BadIvf` is the exception - the
    /// container is not a packet, and `Decoder::decode_stream` stops on a
    /// broken container without asking this function.
    pub fn recoverable(&self) -> bool {
        !matches!(self, Vp9Error::BadIvf)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    Profile0,
    Profile2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameKind {
    Key,
    Inter,
    IntraOnly,
    ShowExisting,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LoopFilter {
    pub level: u8,
    pub sharpness: u8,
    pub mode_ref_delta_enabled: bool,
    pub mode_ref_delta_update: bool,
    pub ref_deltas: [i8; 4],
    pub mode_deltas: [i8; 2],
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Quantization {
    pub base_q_idx: u8,
    pub y_dc_delta_q: i8,
    pub uv_dc_delta_q: i8,
    pub uv_ac_delta_q: i8,
    pub lossless: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segmentation {
    pub enabled: bool,
    pub update_map: bool,
    pub temporal_update: bool,
    pub abs_delta: bool,
    pub tree_probs: [u8; 7],
    pub pred_probs: [u8; 3],
    pub feature_data: [[i16; 4]; 8],
    pub feature_mask: [u8; 8],
}

impl Default for Segmentation {
    fn default() -> Self {
        Self {
            enabled: false,
            update_map: false,
            temporal_update: false,
            abs_delta: false,
            // MPP zero-initializes these fields and only assigns 255 when
            // update_map is signalled. This is observable in PicParams.
            tree_probs: [0; 7],
            pred_probs: [0; 3],
            feature_data: [[0; 4]; 8],
            feature_mask: [0; 8],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub profile: Profile,
    pub frame_kind: FrameKind,
    pub show_frame: bool,
    pub error_resilient_mode: bool,
    pub intra_only: bool,
    pub refresh_frame_flags: u8,
    pub width: u32,
    pub height: u32,
    pub render_width: u32,
    pub render_height: u32,
    pub bit_depth: u8,
    pub ref_frame_idx: [u8; 3],
    pub ref_frame_sign_bias: [bool; 3],
    pub frame_to_show_map_idx: Option<u8>,
    pub refresh_frame_context: bool,
    pub reset_frame_context: u8,
    pub allow_high_precision_mv: bool,
    pub interp_filter: u8,
    pub use_prev_in_find_mv_refs: bool,
    pub frame_context_idx: u8,
    pub frame_parallel_mode: bool,
    pub loop_filter: LoopFilter,
    pub quantization: Quantization,
    pub segmentation: Segmentation,
    pub mvscale: [[u16; 2]; 3],
    pub log2_tile_cols: u8,
    pub log2_tile_rows: u8,
    pub first_partition_size: u16,
    pub header_size: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReferenceFrame {
    width: u32,
    height: u32,
    bit_depth: u8,
}

pub struct Parser {
    references: [Option<ReferenceFrame>; 8],
    got_keyframe: bool,
    probability_contexts: [ProbabilityContext; 4],
    loop_filter: LoopFilter,
    segmentation: Segmentation,
    last_show_frame: bool,
    last_frame_size: Option<(u32, u32)>,
    ref_frame_idx: [u8; 3],
    ref_frame_sign_bias: [bool; 3],
    allow_high_precision_mv: bool,
    interp_filter: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedFrame {
    pub header: Header,
    pub compressed_header: Option<CompressedHeader>,
}

impl Parser {
    pub fn new() -> Self {
        Self {
            references: [None; 8],
            got_keyframe: false,
            probability_contexts: std::array::from_fn(|_| ProbabilityContext::default()),
            loop_filter: LoopFilter::default(),
            segmentation: Segmentation::default(),
            last_show_frame: true,
            last_frame_size: None,
            ref_frame_idx: [0; 3],
            ref_frame_sign_bias: [false; 3],
            allow_high_precision_mv: false,
            interp_filter: 0,
        }
    }

    pub fn parse_frame(&mut self, packet: &[u8]) -> Result<ParsedFrame, Vp9Error> {
        let header = self.parse(packet)?;
        if matches!(header.frame_kind, FrameKind::ShowExisting) {
            return Ok(ParsedFrame {
                header,
                compressed_header: None,
            });
        }

        let reset_all = matches!(header.frame_kind, FrameKind::Key)
            || header.error_resilient_mode
            || (header.intra_only && header.reset_frame_context == 3);
        if reset_all {
            self.probability_contexts = std::array::from_fn(|_| ProbabilityContext::default());
        } else if header.intra_only && header.reset_frame_context == 2 {
            self.probability_contexts[header.frame_context_idx as usize] =
                ProbabilityContext::default();
        }

        let start = header.header_size;
        let end = start + header.first_partition_size as usize;
        let compressed_header = compressed::parse(
            &header,
            &self.probability_contexts[header.frame_context_idx as usize],
            &packet[start..end],
        )?;
        if header.refresh_frame_context && header.frame_parallel_mode {
            self.probability_contexts[header.frame_context_idx as usize] =
                compressed_header.probabilities.clone();
        }
        Ok(ParsedFrame {
            header,
            compressed_header: Some(compressed_header),
        })
    }

    pub fn parse(&mut self, packet: &[u8]) -> Result<Header, Vp9Error> {
        let mut bits = BitReader::new(packet);
        if bits.read(2)? != 2 {
            return Err(Vp9Error::BadFrameMarker);
        }

        let mut profile = bits.read(1)? as u8;
        profile |= (bits.read(1)? as u8) << 1;
        if profile > 2 {
            profile += bits.read(1)? as u8;
        }
        let profile = match profile {
            0 => Profile::Profile0,
            2 => Profile::Profile2,
            other => return Err(Vp9Error::UnsupportedProfile(other)),
        };

        let show_existing_frame = bits.read_bit()?;
        if show_existing_frame {
            let map_idx = bits.read(3)? as u8;
            let reference = self
                .references
                .get(map_idx as usize)
                .and_then(|reference| *reference)
                .ok_or(Vp9Error::MissingReference(map_idx))?;
            return Ok(Header {
                profile,
                frame_kind: FrameKind::ShowExisting,
                show_frame: true,
                error_resilient_mode: false,
                intra_only: false,
                refresh_frame_flags: 0,
                width: reference.width,
                height: reference.height,
                render_width: reference.width,
                render_height: reference.height,
                bit_depth: reference.bit_depth,
                ref_frame_idx: [0; 3],
                ref_frame_sign_bias: [false; 3],
                frame_to_show_map_idx: Some(map_idx),
                refresh_frame_context: false,
                reset_frame_context: 0,
                allow_high_precision_mv: false,
                interp_filter: 0,
                use_prev_in_find_mv_refs: false,
                frame_context_idx: 0,
                frame_parallel_mode: false,
                loop_filter: LoopFilter::default(),
                quantization: Quantization::default(),
                segmentation: Segmentation::default(),
                mvscale: [[16, 16]; 3],
                log2_tile_cols: 0,
                log2_tile_rows: 0,
                first_partition_size: 0,
                header_size: bits.bytes_read(),
            });
        }

        let key_frame = !bits.read_bit()?;
        let show_frame = bits.read_bit()?;
        let error_resilient_mode = bits.read_bit()?;
        if key_frame {
            self.got_keyframe = true;
        }
        if !self.got_keyframe {
            return Err(Vp9Error::MissingReference(0xff));
        }

        let mut intra_only = false;
        let mut reset_frame_context = 0;
        let mut refresh_frame_flags = 0xff;
        let mut width;
        let mut height;
        let mut bit_depth = 8;
        let mut ref_frame_idx = self.ref_frame_idx;
        let mut ref_frame_sign_bias = self.ref_frame_sign_bias;
        let mut mvscale = [[16, 16]; 3];
        let mut allow_high_precision_mv = self.allow_high_precision_mv;
        let mut interp_filter = self.interp_filter;

        if key_frame {
            read_sync_code(&mut bits)?;
            bit_depth = if profile == Profile::Profile2 {
                10 + bits.read(1)? as u8 * 2
            } else {
                8
            };
            let _color_space = bits.read(3)?;
            let _color_range = bits.read_bit()?;
            width = bits.read(16)? + 1;
            height = bits.read(16)? + 1;
            let (render_width, render_height) = read_render_size(&mut bits, width, height)?;
            let header = self.finish_header(
                &mut bits,
                profile,
                FrameKind::Key,
                show_frame,
                error_resilient_mode,
                intra_only,
                refresh_frame_flags,
                width,
                height,
                render_width,
                render_height,
                bit_depth,
                ref_frame_idx,
                ref_frame_sign_bias,
                allow_high_precision_mv,
                interp_filter,
                reset_frame_context,
                mvscale,
            )?;
            for reference in &mut self.references {
                *reference = Some(ReferenceFrame {
                    width,
                    height,
                    bit_depth,
                });
            }
            return Ok(header);
        }

        if !show_frame {
            intra_only = bits.read_bit()?;
        }
        if !error_resilient_mode {
            reset_frame_context = bits.read(2)? as u8;
        }

        if intra_only {
            read_sync_code(&mut bits)?;
            if profile == Profile::Profile2 {
                bit_depth = 10 + bits.read(1)? as u8 * 2;
                let _color_space = bits.read(3)?;
                let _color_range = bits.read_bit()?;
            }
            refresh_frame_flags = bits.read(8)? as u8;
            width = bits.read(16)? + 1;
            height = bits.read(16)? + 1;
        } else {
            refresh_frame_flags = bits.read(8)? as u8;
            for i in 0..3 {
                ref_frame_idx[i] = bits.read(3)? as u8;
                ref_frame_sign_bias[i] = bits.read_bit()? && !error_resilient_mode;
            }

            width = 0;
            height = 0;
            for &reference_index in &ref_frame_idx {
                let use_reference_size = bits.read_bit()?;
                if use_reference_size {
                    let reference = self
                        .references
                        .get(reference_index as usize)
                        .and_then(|reference| *reference)
                        .ok_or(Vp9Error::MissingReference(reference_index))?;
                    width = reference.width;
                    height = reference.height;
                    bit_depth = reference.bit_depth;
                    break;
                }
            }
            if width == 0 {
                width = bits.read(16)? + 1;
                height = bits.read(16)? + 1;
                bit_depth = self
                    .references
                    .iter()
                    .flatten()
                    .next()
                    .map_or(8, |r| r.bit_depth);
            }

            for i in 0..3 {
                let reference = self
                    .references
                    .get(ref_frame_idx[i] as usize)
                    .and_then(|reference| *reference)
                    .ok_or(Vp9Error::MissingReference(ref_frame_idx[i]))?;
                if width * 2 < reference.width
                    || height * 2 < reference.height
                    || width > 16 * reference.width
                    || height > 16 * reference.height
                {
                    return Err(Vp9Error::MissingReference(ref_frame_idx[i]));
                }
                mvscale[i] = [
                    ((reference.width << 14) / width) as u16,
                    ((reference.height << 14) / height) as u16,
                ];
            }
        }

        let (render_width, render_height) = read_render_size(&mut bits, width, height)?;
        if !intra_only {
            allow_high_precision_mv = bits.read_bit()?;
            interp_filter = if bits.read_bit()? {
                4
            } else {
                bits.read(2)? as u8
            };
            self.ref_frame_idx = ref_frame_idx;
            self.ref_frame_sign_bias = ref_frame_sign_bias;
            self.allow_high_precision_mv = allow_high_precision_mv;
            self.interp_filter = interp_filter;
        }

        let header = self.finish_header(
            &mut bits,
            profile,
            if intra_only {
                FrameKind::IntraOnly
            } else {
                FrameKind::Inter
            },
            show_frame,
            error_resilient_mode,
            intra_only,
            refresh_frame_flags,
            width,
            height,
            render_width,
            render_height,
            bit_depth,
            ref_frame_idx,
            ref_frame_sign_bias,
            allow_high_precision_mv,
            interp_filter,
            reset_frame_context,
            mvscale,
        )?;

        for i in 0..8 {
            if refresh_frame_flags & (1 << i) != 0 {
                self.references[i] = Some(ReferenceFrame {
                    width,
                    height,
                    bit_depth,
                });
            }
        }
        Ok(header)
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_header(
        &mut self,
        bits: &mut BitReader<'_>,
        profile: Profile,
        frame_kind: FrameKind,
        show_frame: bool,
        error_resilient_mode: bool,
        intra_only: bool,
        refresh_frame_flags: u8,
        width: u32,
        height: u32,
        render_width: u32,
        render_height: u32,
        bit_depth: u8,
        ref_frame_idx: [u8; 3],
        ref_frame_sign_bias: [bool; 3],
        allow_high_precision_mv: bool,
        interp_filter: u8,
        reset_frame_context: u8,
        mvscale: [[u16; 2]; 3],
    ) -> Result<Header, Vp9Error> {
        let (refresh_frame_context, frame_parallel_mode) = if error_resilient_mode {
            (false, true)
        } else {
            (bits.read_bit()?, bits.read_bit()?)
        };
        let parsed_frame_context_idx = bits.read(2)? as u8;
        let frame_context_idx = if matches!(frame_kind, FrameKind::Key | FrameKind::IntraOnly)
            || error_resilient_mode
        {
            0
        } else {
            parsed_frame_context_idx
        };

        let mut loop_filter = self.loop_filter.clone();
        if matches!(frame_kind, FrameKind::Key | FrameKind::IntraOnly) || error_resilient_mode {
            loop_filter.mode_ref_delta_enabled = true;
            loop_filter.mode_ref_delta_update = true;
            loop_filter.ref_deltas = [1, 0, -1, -1];
            loop_filter.mode_deltas = [0, 0];
        }
        loop_filter.level = bits.read(6)? as u8;
        loop_filter.sharpness = bits.read(3)? as u8;
        loop_filter.mode_ref_delta_enabled = bits.read_bit()?;
        if loop_filter.mode_ref_delta_enabled {
            loop_filter.mode_ref_delta_update = bits.read_bit()?;
            if loop_filter.mode_ref_delta_update {
                for i in 0..4 {
                    if bits.read_bit()? {
                        loop_filter.ref_deltas[i] = read_signed(bits, 6)? as i8;
                    }
                }
                for i in 0..2 {
                    if bits.read_bit()? {
                        loop_filter.mode_deltas[i] = read_signed(bits, 6)? as i8;
                    }
                }
            }
        }

        let base_q_idx = bits.read(8)? as u8;
        let mut quantization = Quantization {
            base_q_idx,
            ..Default::default()
        };
        if bits.read_bit()? {
            quantization.y_dc_delta_q = read_signed(bits, 4)? as i8;
        }
        if bits.read_bit()? {
            quantization.uv_dc_delta_q = read_signed(bits, 4)? as i8;
        }
        if bits.read_bit()? {
            quantization.uv_ac_delta_q = read_signed(bits, 4)? as i8;
        }
        quantization.lossless = base_q_idx == 0
            && quantization.y_dc_delta_q == 0
            && quantization.uv_dc_delta_q == 0
            && quantization.uv_ac_delta_q == 0;

        read_segmentation(bits, &mut self.segmentation)?;
        let segmentation = self.segmentation.clone();
        let sb_cols = width.div_ceil(64);
        let mut log2_tile_cols = 0u8;
        while (sb_cols >> log2_tile_cols) > 64 {
            log2_tile_cols += 1;
        }
        let mut max_log2 = 0u8;
        while (sb_cols >> max_log2) >= 4 {
            max_log2 += 1;
        }
        max_log2 = max_log2.saturating_sub(1);
        while max_log2 > log2_tile_cols {
            if bits.read_bit()? {
                log2_tile_cols += 1;
            } else {
                break;
            }
        }
        let mut log2_tile_rows = bits.read(1)? as u8;
        if log2_tile_rows != 0 {
            log2_tile_rows += bits.read(1)? as u8;
        }

        let first_partition_size = bits.read(16)? as u16;
        bits.align();
        let header_size = bits.bytes_read();
        if header_size + first_partition_size as usize > bits.data.len() {
            return Err(Vp9Error::BadHeaderSize {
                header: header_size,
                partition: first_partition_size as usize,
                packet: bits.data.len(),
            });
        }

        let same_size = self.last_frame_size == Some((width, height));
        let use_prev_in_find_mv_refs = !error_resilient_mode
            && self.last_show_frame
            && (!matches!(frame_kind, FrameKind::Inter) || same_size);
        self.loop_filter = loop_filter.clone();
        self.last_show_frame = show_frame;
        self.last_frame_size = Some((width, height));

        Ok(Header {
            profile,
            frame_kind,
            show_frame,
            error_resilient_mode,
            intra_only,
            refresh_frame_flags,
            width,
            height,
            render_width,
            render_height,
            bit_depth,
            ref_frame_idx,
            ref_frame_sign_bias,
            frame_to_show_map_idx: None,
            refresh_frame_context,
            reset_frame_context,
            allow_high_precision_mv,
            interp_filter,
            use_prev_in_find_mv_refs,
            frame_context_idx,
            frame_parallel_mode,
            loop_filter,
            quantization,
            segmentation,
            mvscale,
            log2_tile_cols,
            log2_tile_rows,
            first_partition_size,
            header_size,
        })
    }
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

pub fn split_superframe(packet: &[u8]) -> Result<Vec<&[u8]>, Vp9Error> {
    let Some(&marker) = packet.last() else {
        return Ok(Vec::new());
    };
    if marker & 0xe0 != 0xc0 {
        return Ok(vec![packet]);
    }
    let size_bytes = 1 + ((marker >> 3) & 0x3) as usize;
    let frame_count = 1 + (marker & 0x7) as usize;
    let index_size = 2 + frame_count * size_bytes;
    if packet.len() < index_size || packet[packet.len() - index_size] != marker {
        return Ok(vec![packet]);
    }

    let sizes_start = packet.len() + 1 - index_size;
    let payload_end = packet.len() - index_size;
    let mut offset = 0usize;
    let mut frames = Vec::with_capacity(frame_count);
    for index in 0..frame_count {
        let start = sizes_start + index * size_bytes;
        let mut size = 0usize;
        for (shift, &byte) in packet[start..start + size_bytes].iter().enumerate() {
            size |= (byte as usize) << (shift * 8);
        }
        if size == 0 || size > payload_end - offset {
            return Err(Vp9Error::BadSuperframe);
        }
        frames.push(&packet[offset..offset + size]);
        offset += size;
    }
    if offset != payload_end {
        return Err(Vp9Error::BadSuperframe);
    }
    Ok(frames)
}

pub struct IvfFrame<'a> {
    pub timestamp: u64,
    pub data: &'a [u8],
}

pub struct Ivf<'a> {
    pub width: u16,
    pub height: u16,
    pub timebase_denominator: u32,
    pub timebase_numerator: u32,
    pub frames: Vec<IvfFrame<'a>>,
}

pub fn parse_ivf(data: &[u8]) -> Result<Ivf<'_>, Vp9Error> {
    let (ivf, error) = parse_ivf_tolerant(data);
    match error {
        None => Ok(ivf),
        Some(error) => Err(error),
    }
}

/// The same walk, but a container whose *tail* is torn still hands over the
/// packets that survived, matching the damaged-stream behaviour measured on
/// the board against `mpi_dec_test`. A broken header gives back nothing.
pub fn parse_ivf_tolerant(data: &[u8]) -> (Ivf<'_>, Option<Vp9Error>) {
    let empty = || {
        (
            Ivf {
                width: 0,
                height: 0,
                timebase_denominator: 0,
                timebase_numerator: 0,
                frames: Vec::new(),
            },
            Some(Vp9Error::BadIvf),
        )
    };
    if data.len() < 32 || &data[..4] != b"DKIF" {
        return empty();
    }
    let header_size = u16::from_le_bytes([data[6], data[7]]) as usize;
    if header_size < 32 || header_size > data.len() || &data[8..12] != b"VP90" {
        return empty();
    }
    let width = u16::from_le_bytes([data[12], data[13]]);
    let height = u16::from_le_bytes([data[14], data[15]]);
    let timebase_denominator = u32::from_le_bytes(data[16..20].try_into().unwrap());
    let timebase_numerator = u32::from_le_bytes(data[20..24].try_into().unwrap());
    let declared_frames = u32::from_le_bytes(data[24..28].try_into().unwrap()) as usize;
    let mut frames = Vec::with_capacity(declared_frames.min(1024));
    let mut offset = header_size;
    while offset < data.len() {
        if offset + 12 > data.len() {
            return (
                Ivf {
                    width,
                    height,
                    timebase_denominator,
                    timebase_numerator,
                    frames,
                },
                Some(Vp9Error::BadIvf),
            );
        }
        let size = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        let timestamp = u64::from_le_bytes(data[offset + 4..offset + 12].try_into().unwrap());
        offset += 12;
        if size == 0 || offset + size > data.len() {
            return (
                Ivf {
                    width,
                    height,
                    timebase_denominator,
                    timebase_numerator,
                    frames,
                },
                Some(Vp9Error::BadIvf),
            );
        }
        frames.push(IvfFrame {
            timestamp,
            data: &data[offset..offset + size],
        });
        offset += size;
    }
    if declared_frames != 0 && frames.len() != declared_frames {
        let mut hard = Ivf {
            width,
            height,
            timebase_denominator,
            timebase_numerator,
            frames,
        };
        hard.frames = Vec::new();
        return (hard, Some(Vp9Error::BadIvf));
    }
    (
        Ivf {
            width,
            height,
            timebase_denominator,
            timebase_numerator,
            frames,
        },
        None,
    )
}

fn read_sync_code(bits: &mut BitReader<'_>) -> Result<(), Vp9Error> {
    if bits.read(8)? != 0x49 || bits.read(8)? != 0x83 || bits.read(8)? != 0x42 {
        Err(Vp9Error::BadSyncCode)
    } else {
        Ok(())
    }
}

fn read_render_size(
    bits: &mut BitReader<'_>,
    width: u32,
    height: u32,
) -> Result<(u32, u32), Vp9Error> {
    if bits.read_bit()? {
        Ok((bits.read(16)? + 1, bits.read(16)? + 1))
    } else {
        Ok((width, height))
    }
}

fn read_signed(bits: &mut BitReader<'_>, width: u8) -> Result<i32, Vp9Error> {
    let magnitude = bits.read(width)? as i32;
    let negative = bits.read_bit()?;
    Ok(if negative { -magnitude } else { magnitude })
}

fn read_segmentation(
    bits: &mut BitReader<'_>,
    segmentation: &mut Segmentation,
) -> Result<(), Vp9Error> {
    segmentation.enabled = bits.read_bit()?;
    segmentation.update_map = false;
    if !segmentation.enabled {
        segmentation.feature_mask[0] = 0;
        segmentation.feature_data[0][3] = 0;
        return Ok(());
    }

    segmentation.update_map = bits.read_bit()?;
    if segmentation.update_map {
        for probability in &mut segmentation.tree_probs {
            *probability = 0xff;
            if bits.read_bit()? {
                *probability = bits.read(8)? as u8;
            }
        }
        segmentation.temporal_update = bits.read_bit()?;
        if segmentation.temporal_update {
            for probability in &mut segmentation.pred_probs {
                *probability = 0xff;
                if bits.read_bit()? {
                    *probability = bits.read(8)? as u8;
                }
            }
        } else {
            segmentation.pred_probs = [0xff; 3];
        }
    }

    if bits.read_bit()? {
        segmentation.abs_delta = bits.read_bit()?;
        for segment in 0..8 {
            let mut mask = 0u8;
            if bits.read_bit()? {
                mask |= 1;
                segmentation.feature_data[segment][0] = read_signed(bits, 8)? as i16;
            }
            if bits.read_bit()? {
                mask |= 2;
                segmentation.feature_data[segment][1] = read_signed(bits, 6)? as i16;
            }
            if bits.read_bit()? {
                mask |= 4;
                segmentation.feature_data[segment][2] = bits.read(2)? as i16;
            }
            if bits.read_bit()? {
                mask |= 8;
                segmentation.feature_data[segment][3] = 1;
            }
            segmentation.feature_mask[segment] = mask;
        }
    }
    Ok(())
}

struct BitReader<'a> {
    data: &'a [u8],
    bit: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, bit: 0 }
    }

    fn read_bit(&mut self) -> Result<bool, Vp9Error> {
        Ok(self.read(1)? != 0)
    }

    fn read(&mut self, width: u8) -> Result<u32, Vp9Error> {
        if width > 32 || self.bit + width as usize > self.data.len() * 8 {
            return Err(Vp9Error::Truncated {
                bit: self.bit,
                width,
            });
        }
        let mut value = 0u32;
        for _ in 0..width {
            let byte = self.data[self.bit / 8];
            value = (value << 1) | ((byte >> (7 - self.bit % 8)) & 1) as u32;
            self.bit += 1;
        }
        Ok(value)
    }

    fn align(&mut self) {
        self.bit = (self.bit + 7) & !7;
    }

    fn bytes_read(&self) -> usize {
        self.bit.div_ceil(8)
    }
}
