//! Open Bitstream Units and the IVF container used by the AV1 fixtures.
//!
//! Implements `open_bitstream_unit(sz)`, `obu_header()`,
//! `obu_extension_header()` and `trailing_bits()` from specification §5.2,
//! plus a small IVF demuxer so the real streams in `tests/data` can be parsed
//! without any external library.
//!
//! OBUs are returned as borrowed slices; no payload is copied. Systems that
//! need a different OBU framing (Annex B) can pass `sz` to [`parse_obu`] for
//! the `obu_has_size_field == 0` case.

use crate::reference::av1::bitstream::{BitReader, BitstreamError};

/// `obu_type` values from the §5.2 table. `9..14` are reserved and `15` is
/// padding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObuType {
    Reserved,
    SequenceHeader,
    TemporalDelimiter,
    FrameHeader,
    TileGroup,
    Metadata,
    Frame,
    RedundantFrameHeader,
    TileList,
    Padding,
    /// Reserved values 9..=14.
    Unknown(u8),
}

impl ObuType {
    pub const fn from_raw(value: u8) -> Self {
        match value {
            0 => Self::Reserved,
            1 => Self::SequenceHeader,
            2 => Self::TemporalDelimiter,
            3 => Self::FrameHeader,
            4 => Self::TileGroup,
            5 => Self::Metadata,
            6 => Self::Frame,
            7 => Self::RedundantFrameHeader,
            8 => Self::TileList,
            15 => Self::Padding,
            other => Self::Unknown(other),
        }
    }

    pub const fn raw(self) -> u8 {
        match self {
            Self::Reserved => 0,
            Self::SequenceHeader => 1,
            Self::TemporalDelimiter => 2,
            Self::FrameHeader => 3,
            Self::TileGroup => 4,
            Self::Metadata => 5,
            Self::Frame => 6,
            Self::RedundantFrameHeader => 7,
            Self::TileList => 8,
            Self::Padding => 15,
            Self::Unknown(v) => v,
        }
    }

    /// "Layer-specific" column of the §5.2 table. Only layer-specific OBUs
    /// can be dropped by the temporal/spatial layer test.
    pub const fn is_layer_specific(self) -> bool {
        matches!(
            self,
            Self::FrameHeader
                | Self::TileGroup
                | Self::Frame
                | Self::RedundantFrameHeader
                | Self::Metadata
        )
    }

    /// `open_bitstream_unit()` only appends `trailing_bits()` for OBUs that
    /// are not tile groups, tile lists or frames.
    pub const fn has_trailing_bits(self) -> bool {
        !matches!(self, Self::TileGroup | Self::TileList | Self::Frame)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObuHeader {
    pub obu_type: ObuType,
    pub obu_type_raw: u8,
    pub forbidden_bit: u8,
    pub extension_flag: bool,
    pub has_size_field: bool,
    pub reserved_1bit: u8,
    /// Only meaningful when `extension_flag` is set.
    pub temporal_id: u8,
    pub spatial_id: u8,
    pub extension_reserved_3bits: u8,
    /// 1 or 2, depending on `obu_extension_flag`.
    pub header_bytes: usize,
    /// Number of bytes taken by `leb128()` when `has_size_field` is set.
    pub size_field_bytes: usize,
}

impl ObuHeader {
    pub const fn total_header_bytes(&self) -> usize {
        self.header_bytes + self.size_field_bytes
    }
}

/// One parsed OBU, with its payload borrowed from the input slice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Obu<'a> {
    pub header: ObuHeader,
    /// `obu_size`: payload bytes, including any trailing bits.
    pub size: usize,
    pub payload: &'a [u8],
    /// Byte offset of this OBU inside the containing frame/bitstream.
    pub offset: usize,
    /// Total bytes consumed (header + extension + size field + payload).
    pub consumed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ObuError {
    EmptyInput,
    ForbiddenBitSet(u8),
    TruncatedHeader {
        available: usize,
    },
    NoSizeField,
    SizeField(BitstreamError),
    /// `obu_has_size_field == 0` needs the enclosing `sz` from the caller.
    MissingSizeArgument,
    SizeSmallerThanHeader {
        sz: usize,
        header_bytes: usize,
    },
    TruncatedPayload {
        expected: usize,
        available: usize,
    },
    /// [`Obu::trailing_bits_needed`] found an impossible payload bit count.
    PayloadBitsOverflow {
        consumed_bits: usize,
        payload_bits: usize,
    },
    /// [`parse_obus`] reports which OBU in the sequence failed.
    At {
        index: usize,
        source: Box<ObuError>,
    },
}

impl core::fmt::Display for ObuError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}

/// Parse a single `open_bitstream_unit(sz)` at the start of `data`.
///
/// `sz` is the total OBU byte length known from the container (Annex B framing).
/// It is required only when `obu_has_size_field == 0`; when the size field is
/// present it is ignored. Returns the OBU and the number of bytes consumed.
pub fn parse_obu<'a>(data: &'a [u8], sz: Option<usize>) -> Result<(Obu<'a>, usize), ObuError> {
    if data.is_empty() {
        return Err(ObuError::EmptyInput);
    }
    let first = data[0];
    let forbidden_bit = first >> 7;
    if forbidden_bit != 0 {
        return Err(ObuError::ForbiddenBitSet(first));
    }
    let obu_type_raw = (first >> 3) & 0x0f;
    let extension_flag = (first >> 2) & 1 != 0;
    let has_size_field = (first >> 1) & 1 != 0;
    let reserved_1bit = first & 1;

    let mut header_bytes = 1;
    let (temporal_id, spatial_id, extension_reserved_3bits) = if extension_flag {
        if data.len() < 2 {
            return Err(ObuError::TruncatedHeader { available: 1 });
        }
        let ext = data[1];
        header_bytes = 2;
        ((ext >> 5) & 7, (ext >> 3) & 3, ext & 7)
    } else {
        (0, 0, 0)
    };

    let (obu_size, size_field_bytes) = if has_size_field {
        let mut reader = BitReader::new(&data[header_bytes..]);
        let (value, bytes) = reader.read_leb128().map_err(ObuError::SizeField)?;
        (value as usize, bytes)
    } else {
        let sz = sz.ok_or(ObuError::MissingSizeArgument)?;
        let payload_size = sz
            .checked_sub(header_bytes)
            .ok_or(ObuError::SizeSmallerThanHeader { sz, header_bytes })?;
        (payload_size, 0)
    };

    let header = ObuHeader {
        obu_type: ObuType::from_raw(obu_type_raw),
        obu_type_raw,
        forbidden_bit,
        extension_flag,
        has_size_field,
        reserved_1bit,
        temporal_id,
        spatial_id,
        extension_reserved_3bits,
        header_bytes,
        size_field_bytes,
    };

    let payload_start = header.total_header_bytes();
    let payload_end = payload_start
        .checked_add(obu_size)
        .ok_or(ObuError::TruncatedPayload {
            expected: usize::MAX,
            available: data.len(),
        })?;
    if payload_end > data.len() {
        return Err(ObuError::TruncatedPayload {
            expected: payload_end,
            available: data.len(),
        });
    }

    Ok((
        Obu {
            header,
            size: obu_size,
            payload: &data[payload_start..payload_end],
            offset: 0,
            consumed: payload_end,
        },
        payload_end,
    ))
}

/// Parse a whole low-overhead bitstream (one IVF frame payload, or a `.obu`
/// file) into a sequence of OBUs.
pub fn parse_obus(data: &[u8]) -> Result<Vec<Obu<'_>>, ObuError> {
    let mut out = Vec::new();
    let mut offset = 0;
    while offset < data.len() {
        let (mut obu, consumed) =
            parse_obu(&data[offset..], None).map_err(|source| ObuError::At {
                index: out.len(),
                source: Box::new(source),
            })?;
        obu.offset += offset;
        offset += consumed;
        out.push(obu);
    }
    Ok(out)
}

impl Obu<'_> {
    /// `open_bitstream_unit()` computes `payloadBits = currentPosition -
    /// startPosition` then calls `trailing_bits(obu_size * 8 - payloadBits)`.
    /// Consumers pass their `BitReader::position()` after parsing the payload
    /// and feed the result to `BitReader::read_trailing_bits`.
    pub fn trailing_bits_needed(&self, consumed_bits: usize) -> Result<usize, ObuError> {
        if self.size == 0 || !self.header.obu_type.has_trailing_bits() {
            return Ok(0);
        }
        let payload_bits = self.size * 8;
        if consumed_bits > payload_bits {
            return Err(ObuError::PayloadBitsOverflow {
                consumed_bits,
                payload_bits,
            });
        }
        Ok(payload_bits - consumed_bits)
    }

    /// The temporal/spatial layer drop test in §5.2. The specification gates
    /// it on `obu_type != OBU_SEQUENCE_HEADER && != OBU_TEMPORAL_DELIMITER`
    /// (not on the "layer-specific" table column) plus
    /// `OperatingPointIdc != 0 && obu_extension_flag == 1`.
    pub fn should_drop(&self, operating_point_idc: u16) -> bool {
        let h = &self.header;
        if matches!(
            h.obu_type,
            ObuType::SequenceHeader | ObuType::TemporalDelimiter
        ) || operating_point_idc == 0
            || !h.extension_flag
        {
            return false;
        }
        let in_temporal_layer = (operating_point_idc >> h.temporal_id) & 1 != 0;
        let in_spatial_layer = (operating_point_idc >> (h.spatial_id + 8)) & 1 != 0;
        !in_temporal_layer || !in_spatial_layer
    }
}

/// IVF file header (`DKIF`, 32 bytes in all fixtures here).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IvfHeader {
    pub version: u16,
    pub header_len: u16,
    pub fourcc: [u8; 4],
    pub width: u16,
    pub height: u16,
    pub rate: u32,
    pub scale: u32,
    pub frame_count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IvfFrame<'a> {
    pub timestamp: u64,
    /// The OBU bitstream for this frame.
    pub payload: &'a [u8],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ivf<'a> {
    pub header: IvfHeader,
    pub frames: Vec<IvfFrame<'a>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IvfError {
    TooShort {
        available: usize,
    },
    BadMagic([u8; 4]),
    UnsupportedVersion(u16),
    HeaderLengthTooSmall(u16),
    BadFourcc([u8; 4]),
    TruncatedFrame {
        index: usize,
        size: usize,
        available: usize,
    },
}

impl core::fmt::Display for IvfError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}

fn read_u16(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([data[at], data[at + 1]])
}

fn read_u32(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]])
}

fn read_u64(data: &[u8], at: usize) -> u64 {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&data[at..at + 8]);
    u64::from_le_bytes(bytes)
}

/// Demux an IVF container: 32-byte `DKIF` header followed by
/// `[frame_size u32][timestamp u64][payload]` records.
pub fn parse_ivf(data: &[u8]) -> Result<Ivf<'_>, IvfError> {
    if data.len() < 32 {
        return Err(IvfError::TooShort {
            available: data.len(),
        });
    }
    if &data[0..4] != b"DKIF" {
        let mut magic = [0u8; 4];
        magic.copy_from_slice(&data[0..4]);
        return Err(IvfError::BadMagic(magic));
    }
    let version = read_u16(data, 4);
    if version != 0 {
        return Err(IvfError::UnsupportedVersion(version));
    }
    let header_len = read_u16(data, 6);
    if (header_len as usize) < 32 || header_len as usize > data.len() {
        return Err(IvfError::HeaderLengthTooSmall(header_len));
    }
    let mut fourcc = [0u8; 4];
    fourcc.copy_from_slice(&data[8..12]);
    if &fourcc != b"AV01" {
        return Err(IvfError::BadFourcc(fourcc));
    }
    let header = IvfHeader {
        version,
        header_len,
        fourcc,
        width: read_u16(data, 12),
        height: read_u16(data, 14),
        rate: read_u32(data, 16),
        scale: read_u32(data, 20),
        frame_count: read_u32(data, 24),
    };

    let mut frames = Vec::new();
    let mut offset = header_len as usize;
    while offset + 12 <= data.len() {
        let size = read_u32(data, offset) as usize;
        let timestamp = read_u64(data, offset + 4);
        let payload_start = offset + 12;
        let payload_end = match payload_start.checked_add(size) {
            Some(end) if end <= data.len() => end,
            _ => {
                return Err(IvfError::TruncatedFrame {
                    index: frames.len(),
                    size,
                    available: data.len() - payload_start,
                })
            }
        };
        frames.push(IvfFrame {
            timestamp,
            payload: &data[payload_start..payload_end],
        });
        offset = payload_end;
    }
    Ok(Ivf { header, frames })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TD: u8 = 0x12; // type=2, ext=0, has_size=1
    const SEQ: u8 = 0x0a; // type=1, ext=0, has_size=1

    #[test]
    fn obu_type_table_matches_the_specification() {
        for raw in 0..=15u8 {
            assert_eq!(ObuType::from_raw(raw).raw(), raw);
        }
        assert_eq!(ObuType::from_raw(1), ObuType::SequenceHeader);
        assert_eq!(ObuType::from_raw(2), ObuType::TemporalDelimiter);
        assert_eq!(ObuType::from_raw(6), ObuType::Frame);
        assert_eq!(ObuType::from_raw(15), ObuType::Padding);
        assert_eq!(ObuType::from_raw(9), ObuType::Unknown(9));
        assert!(ObuType::TileGroup.is_layer_specific());
        assert!(!ObuType::SequenceHeader.is_layer_specific());
        assert!(!ObuType::Frame.has_trailing_bits());
        assert!(ObuType::SequenceHeader.has_trailing_bits());
    }

    #[test]
    fn parses_empty_temporal_delimiter_and_sequence_header() {
        // TD (size 0) + SEQ (size 3) + FRAME_HEADER (size 2), all with size
        // fields. This is exactly the framing aomenc emits for a key frame.
        let data = [
            TD, 0x00, SEQ, 0x03, 0x80, 0x00, 0x00, 0x1a, 0x02, 0xaa, 0xbb,
        ];
        let obus = parse_obus(&data).unwrap();
        assert_eq!(obus.len(), 3);
        assert_eq!(obus[0].header.obu_type, ObuType::TemporalDelimiter);
        assert_eq!(obus[0].size, 0);
        assert!(obus[0].payload.is_empty());
        assert_eq!(obus[0].consumed, 2);
        assert_eq!(obus[1].header.obu_type, ObuType::SequenceHeader);
        assert_eq!(obus[1].offset, 2);
        assert_eq!(obus[1].payload, &[0x80, 0x00, 0x00]);
        assert_eq!(obus[2].header.obu_type, ObuType::FrameHeader);
        assert_eq!(obus[2].offset, 7);
        assert_eq!(obus[2].payload, &[0xaa, 0xbb]);
    }

    #[test]
    fn multi_byte_leb128_and_extension_header() {
        // Extension flag set, type=FRAME_HEADER, size 300 encoded as ac 02.
        let data = [0x1e, 0xa0, 0xac, 0x02];
        let payload = vec![0x5a; 300];
        let mut full = data.to_vec();
        full.extend_from_slice(&payload);
        let (obu, consumed) = parse_obu(&full, None).unwrap();
        assert_eq!(obu.header.obu_type, ObuType::FrameHeader);
        assert!(obu.header.extension_flag);
        assert_eq!(obu.header.temporal_id, 5);
        assert_eq!(obu.header.spatial_id, 0);
        assert_eq!(obu.header.extension_reserved_3bits, 0);
        assert_eq!(obu.header.size_field_bytes, 2);
        assert_eq!(obu.size, 300);
        assert_eq!(consumed, 304);
    }

    #[test]
    fn no_size_field_uses_the_callers_sz() {
        // type=TD, ext=0, has_size=0; sz covers the header plus 2 payload bytes.
        let data = [0x10, 0xaa, 0xbb, 0xcc];
        let (obu, consumed) = parse_obu(&data, Some(4)).unwrap();
        assert_eq!(obu.header.obu_type, ObuType::TemporalDelimiter);
        assert!(!obu.header.has_size_field);
        assert_eq!(obu.header.header_bytes, 1);
        assert_eq!(obu.size, 3);
        assert_eq!(obu.payload, &[0xaa, 0xbb, 0xcc]);
        assert_eq!(consumed, 4);
        assert_eq!(parse_obu(&data, None), Err(ObuError::MissingSizeArgument));
        assert_eq!(
            parse_obu(&data, Some(0)),
            Err(ObuError::SizeSmallerThanHeader {
                sz: 0,
                header_bytes: 1
            })
        );
    }

    #[test]
    fn malformed_obus_are_rejected_with_context() {
        assert_eq!(parse_obus(&[]), Ok(vec![]));
        assert_eq!(
            parse_obus(&[0x92]),
            Err(ObuError::At {
                index: 0,
                source: Box::new(ObuError::ForbiddenBitSet(0x92))
            })
        );
        assert_eq!(
            parse_obus(&[SEQ, 0x05, 0x00]),
            Err(ObuError::At {
                index: 0,
                source: Box::new(ObuError::TruncatedPayload {
                    expected: 7,
                    available: 3
                })
            })
        );
        assert_eq!(
            parse_obus(&[SEQ, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80]),
            Err(ObuError::At {
                index: 0,
                source: Box::new(ObuError::SizeField(BitstreamError::Leb128TooLong))
            })
        );
    }

    #[test]
    fn trailing_bits_accounting_matches_open_bitstream_unit() {
        let obu = Obu {
            header: ObuHeader {
                obu_type: ObuType::SequenceHeader,
                obu_type_raw: 1,
                forbidden_bit: 0,
                extension_flag: false,
                has_size_field: true,
                reserved_1bit: 0,
                temporal_id: 0,
                spatial_id: 0,
                extension_reserved_3bits: 0,
                header_bytes: 1,
                size_field_bytes: 1,
            },
            size: 3,
            payload: &[0x00, 0x00, 0x00],
            offset: 0,
            consumed: 4,
        };
        // 20 payload bits consumed leaves 4 trailing bits: one + three zeros.
        assert_eq!(obu.trailing_bits_needed(20), Ok(4));
        assert_eq!(obu.trailing_bits_needed(24), Ok(0));
        assert!(matches!(
            obu.trailing_bits_needed(25),
            Err(ObuError::PayloadBitsOverflow { .. })
        ));
        // Frames and tile groups never carry trailing_bits().
        let mut tile = obu;
        tile.header.obu_type = ObuType::Frame;
        assert_eq!(tile.trailing_bits_needed(0), Ok(0));
    }

    #[test]
    fn layer_drop_test_follows_operating_point_idc() {
        let ext_frame = [0x36, 0xa0, 0x00]; // FRAME, ext flag, temporal=5, spatial=0
        let (obu, _) = parse_obu(&ext_frame, None).unwrap();
        assert_eq!(obu.header.temporal_id, 5);
        assert_eq!(obu.header.spatial_id, 0);
        assert!(!obu.should_drop(0), "idc 0 disables the test");
        assert!(obu.should_drop(1 << 8), "temporal layer 5 missing");
        assert!(!obu.should_drop((1 << 5) | (1 << 8)));
        // Sequence headers are never dropped.
        let (seq, _) = parse_obu(&[0x0e, 0xa0, 0x00], None).unwrap();
        assert_eq!(seq.header.obu_type, ObuType::SequenceHeader);
        assert!(!seq.should_drop(1));
    }

    #[test]
    fn ivf_demux_reads_header_and_frame_records() {
        let mut data = Vec::new();
        data.extend_from_slice(b"DKIF");
        data.extend_from_slice(&0u16.to_le_bytes()); // version
        data.extend_from_slice(&32u16.to_le_bytes()); // header len
        data.extend_from_slice(b"AV01");
        data.extend_from_slice(&64u16.to_le_bytes()); // width
        data.extend_from_slice(&48u16.to_le_bytes()); // height
        data.extend_from_slice(&30u32.to_le_bytes()); // rate
        data.extend_from_slice(&1u32.to_le_bytes()); // scale
        data.extend_from_slice(&2u32.to_le_bytes()); // frame count
        data.extend_from_slice(&0u32.to_le_bytes()); // unused
        for (ts, payload) in [(7u64, &[TD, 0x00][..]), (9, &[SEQ, 0x01, 0x80][..])] {
            data.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            data.extend_from_slice(&ts.to_le_bytes());
            data.extend_from_slice(payload);
        }
        let ivf = parse_ivf(&data).unwrap();
        assert_eq!(ivf.header.width, 64);
        assert_eq!(ivf.header.height, 48);
        assert_eq!(ivf.header.frame_count, 2);
        assert_eq!(ivf.frames.len(), 2);
        assert_eq!(ivf.frames[0].timestamp, 7);
        assert_eq!(ivf.frames[1].payload, &[SEQ, 0x01, 0x80]);
        assert_eq!(parse_obus(ivf.frames[1].payload).unwrap().len(), 1);
    }

    fn valid_ivf_header() -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(b"DKIF");
        d.extend_from_slice(&0u16.to_le_bytes());
        d.extend_from_slice(&32u16.to_le_bytes());
        d.extend_from_slice(b"AV01");
        d.extend_from_slice(&64u16.to_le_bytes());
        d.extend_from_slice(&64u16.to_le_bytes());
        d.extend_from_slice(&30u32.to_le_bytes());
        d.extend_from_slice(&1u32.to_le_bytes());
        d.extend_from_slice(&1u32.to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes());
        d
    }

    #[test]
    fn ivf_rejects_bad_containers() {
        assert_eq!(
            parse_ivf(&[0u8; 8]),
            Err(IvfError::TooShort { available: 8 })
        );
        let good = valid_ivf_header();
        assert!(parse_ivf(&good).is_ok());

        let mut bad_magic = good.clone();
        bad_magic[..4].copy_from_slice(b"NOPE");
        assert_eq!(parse_ivf(&bad_magic), Err(IvfError::BadMagic(*b"NOPE")));

        let mut bad_version = good.clone();
        bad_version[4..6].copy_from_slice(&1u16.to_le_bytes());
        assert_eq!(
            parse_ivf(&bad_version),
            Err(IvfError::UnsupportedVersion(1))
        );

        let mut bad_header_len = good.clone();
        bad_header_len[6..8].copy_from_slice(&16u16.to_le_bytes());
        assert_eq!(
            parse_ivf(&bad_header_len),
            Err(IvfError::HeaderLengthTooSmall(16))
        );

        let mut bad_fourcc = good.clone();
        bad_fourcc[8..12].copy_from_slice(b"VP90");
        assert_eq!(parse_ivf(&bad_fourcc), Err(IvfError::BadFourcc(*b"VP90")));

        let mut truncated = good.clone();
        truncated.extend_from_slice(&5u32.to_le_bytes());
        truncated.extend_from_slice(&0u64.to_le_bytes());
        truncated.extend_from_slice(&[0x00]);
        assert_eq!(
            parse_ivf(&truncated),
            Err(IvfError::TruncatedFrame {
                index: 0,
                size: 5,
                available: 1
            })
        );
    }
}
