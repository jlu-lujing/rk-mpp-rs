//! H.263 (ITU-T H.263, the "short video header" profile MPP decodes) picture syntax.
//!
//! The access-unit rule is `mpp_h263_parser_split`: a picture starts at the
//! first `00` of a picture start code and ends at the first `00` of the next
//! one. The code itself is 16 zero bits, a one bit and a five-bit GOB number of
//! zero, which as a rolling 32-bit window is the pair of masks below; in bytes
//! it is `00 00 80` to `00 00 83`. A GOB start code with a non-zero number does
//! not match, so it stays inside the picture it belongs to.
//!
//! Only the picture header is parsed. [`picture_header`] walks exactly the
//! fields `h263_parse_picture_header` reads - and stops at the same place, which
//! is where the hardware starts - so everything after it is macroblock data the
//! block decodes itself.
//!
//! The size of a picture comes from `source_format` alone: there is no
//! width/height field in this header at all. Custom source formats and the
//! extended PTY of H.263+ have no entry in the size table, which is how the C
//! parser refuses them, and the four-bit PTYPE tail of UMV / advanced
//! prediction / PB frames has to be zero for the same reason.

use std::fmt;

/// `H263VOPType`, with the invalid entry left out. MPEG-4 numbers picture types
/// the same way, MPEG-2 starts at 1.
pub const VOP_I: u32 = 0;
pub const VOP_P: u32 = 1;

/// `H263_STARTCODE` / `H263_STARTCODE_MASK` plus `H263_GOB_ZERO_MASK`: sixteen
/// zero bits, one one bit, then a GOB number that has to read zero.
fn is_picture_start(state: u32) -> bool {
    (state & 0x00FF_FF80) == 0x0000_0080 && (state & 0x7C) == 0
}

/// `h263d_fmt_to_dimension`: source format to coded size. The two entries the C
/// table leaves blank - custom source format and the extended PTY - are the
/// `None` cases here.
pub fn source_format_size(format: u32) -> Option<(u32, u32)> {
    match format {
        1 => Some((128, 96)),    // SQCIF
        2 => Some((176, 144)),   // QCIF
        3 => Some((352, 288)),   // CIF
        4 => Some((704, 576)),   // 4CIF
        5 => Some((1408, 1152)), // 16CIF
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum H263Error {
    Truncated,
    /// A field the C parser asserts on or refuses outright.
    Unsupported(&'static str),
}

impl fmt::Display for H263Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            H263Error::Truncated => write!(f, "bitstream truncated"),
            H263Error::Unsupported(s) => write!(f, "unsupported: {s}"),
        }
    }
}

impl std::error::Error for H263Error {}

/// MSB-first bit reader. H.263 keeps `00 00` out of coded data with byte
/// stuffing rather than a separate escape, so no unescaping is needed.
pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }

    pub fn bits(&mut self, n: u32) -> Result<u32, H263Error> {
        if n == 0 {
            return Ok(0);
        }
        if n > 32 || self.data.len() * 8 - self.pos < n as usize {
            return Err(H263Error::Truncated);
        }
        let mut v = 0u32;
        for _ in 0..n {
            let byte = self.data[self.pos / 8];
            v = (v << 1) | u32::from((byte >> (7 - (self.pos % 8))) & 1);
            self.pos += 1;
        }
        Ok(v)
    }

    pub fn skip(&mut self, n: u32) -> Result<(), H263Error> {
        self.bits(n).map(|_| ())
    }

    /// Bit position from the start of the buffer, which is what the hardware's
    /// `sw_stream_start_word` and the stream-base offset are measured in.
    pub fn bit_pos(&self) -> usize {
        self.pos
    }
}

/// One picture's header, with the access unit it was cut from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Picture {
    /// Byte range of the access unit in the elementary stream, starting at the
    /// first `00` of the picture start code.
    pub au: (usize, usize),
    pub coding_type: u32,
    /// Coded size, straight out of `source_format`.
    pub width: u32,
    pub height: u32,
    /// `TR`, the eight-bit temporal reference. Nothing here is ordered by it -
    /// pictures leave in decode order - but it says which frames the encoder
    /// dropped, so it is kept for diagnostics.
    pub temporal_reference: u32,
    /// `MQQUANT`, the initial quantiser of the picture.
    pub quant: u32,
    /// Bits the header consumed, counted from the start code.
    pub header_bits: usize,
}

/// Advance a rolling byte window from `from` until a picture start code shows.
/// Returns the index of the byte that completed the code and the window state,
/// which the caller keeps for the next scan exactly as C keeps it across the two
/// loops of one split call.
fn scan_for_start(data: &[u8], from: usize, mut state: u32) -> Option<(usize, u32)> {
    for (index, &byte) in data.iter().enumerate().skip(from) {
        state = (state << 8) | u32::from(byte);
        if is_picture_start(state) {
            return Some((index, state));
        }
    }
    None
}

/// `mpp_h263_parser_split` over a whole buffer: `(start, end)` per picture.
///
/// Each splitter call in C re-enters with the window reset to all ones, so the
/// two zeros that open a start code get scanned twice and the `end` of one
/// picture is exactly the `start` of the next. The last picture runs to the end
/// of the buffer, which is what end of stream means to the C path.
pub fn access_units(data: &[u8]) -> Vec<(usize, usize)> {
    let mut units = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
        let Some((code, state)) = scan_for_start(data, pos, u32::MAX) else {
            break;
        };
        let start = code.saturating_sub(2);
        match scan_for_start(data, code + 1, state) {
            Some((next, _)) => {
                let end = next.saturating_sub(2);
                units.push((start, end));
                pos = end;
            }
            None => {
                units.push((start, data.len()));
                break;
            }
        }
    }
    units
}

/// `h263_parse_picture_header` for one access unit. The reader starts where the
/// start code starts, which `mpp_h263_parser_decode` finds by scanning for the
/// 22-bit prefix a byte at a time.
pub fn picture_header(au: &[u8]) -> Result<(usize, Picture), H263Error> {
    // The C scan keeps a 32-bit window seeded at 0xff and compares its top 22
    // bits against 0x20, so the read starts three bytes before the match.
    let mut window = 0xffu32;
    let mut start = None;
    for (index, &byte) in au.iter().enumerate() {
        window = (window << 8) | u32::from(byte);
        if window >> 10 == 0x20 {
            start = Some(index.saturating_sub(3));
            break;
        }
    }
    let start = start.ok_or(H263Error::Unsupported("no picture start code"))?;
    let mut reader = Reader::new(&au[start..]);

    if reader.bits(17)? != 1 {
        return Err(H263Error::Unsupported("picture start code"));
    }
    if reader.bits(5)? != 0 {
        // Only reachable if a GOB start code was handed in as a picture; the
        // splitter's masks keep it out, and C asserts on it.
        return Err(H263Error::Unsupported("non-zero GOB number"));
    }
    let temporal_reference = reader.bits(8)?;
    // The first five PTYPE bits (temporal-spatial prediction and the option
    // bits around it) are read into nothing at all.
    reader.skip(5)?;
    let (width, height) =
        source_format_size(reader.bits(3)?).ok_or(H263Error::Unsupported("source format"))?;
    let coding_type = reader.bits(1)?;
    // UMV, advanced prediction and PB frames all live in these four bits.
    if reader.bits(4)? != 0 {
        return Err(H263Error::Unsupported("PTYPE mode"));
    }
    let quant = reader.bits(5)?;
    reader.skip(1)?;
    // ESBMC, ALSC, end-of-block command and the rest of the optional extension
    // set: one continuation bit each, followed by eight bits of parameters.
    while reader.bits(1)? == 1 {
        reader.skip(8)?;
    }

    Ok((
        start,
        Picture {
            au: (0, au.len()),
            coding_type,
            width,
            height,
            temporal_reference,
            quant,
            header_bits: reader.bit_pos(),
        },
    ))
}

/// The pictures of a stream, in decode order.
///
/// Two rules from `h263_parse_picture_header` decide what never reaches the
/// hardware, and both are reproduced by dropping the picture rather than by
/// failing: anything before the first I picture (the `found_i_vop` gate) and
/// anything whose header the C parser would return an error for. The C decoder
/// logs the failure and moves on to the next access unit, so a stream with one
/// damaged picture still decodes the rest.
pub fn parse(data: &[u8]) -> Vec<Picture> {
    let mut pictures = Vec::new();
    let mut found_i_vop = false;
    for (start, end) in access_units(data) {
        let au = &data[start..end];
        let Ok((_, mut picture)) = picture_header(au) else {
            continue;
        };
        if !found_i_vop {
            found_i_vop = picture.coding_type == VOP_I;
            if !found_i_vop {
                continue;
            }
        }
        picture.au = (start, end);
        pictures.push(picture);
    }
    pictures
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_code_matches_the_c_masks() {
        // `00 00 80` .. `00 00 83` are pictures; GOB 1 to 15 are not, and
        // neither is a code whose one bit sits anywhere else.
        for code in 0x80u32..=0x83 {
            assert!(is_picture_start(code), "{code:02x} is a picture");
        }
        for code in 0x84u32..=0xff {
            assert!(
                !is_picture_start(code),
                "{code:02x} must not read as a picture"
            );
        }
        assert!(!is_picture_start(0x00), "no start bit at all");
        assert!(
            !is_picture_start(0x00_01_00_80),
            "the zeros must be adjacent"
        );
        assert!(
            is_picture_start(0x00_00_00_80),
            "anything above the code is outside the mask"
        );
    }

    #[test]
    fn source_formats_are_the_ones_the_c_table_lists() {
        assert_eq!(source_format_size(1), Some((128, 96)));
        assert_eq!(source_format_size(3), Some((352, 288)));
        assert_eq!(source_format_size(5), Some((1408, 1152)));
        assert_eq!(source_format_size(0), None);
        assert_eq!(source_format_size(6), None, "custom source format");
        assert_eq!(source_format_size(7), None, "extended PTY");
    }

    /// The header values of the CIF fixture, against the `hal_h263d_debug=1`
    /// register dumps of the same stream on t6: `sw51` carries the stream
    /// length and the quantiser of every picture.
    #[test]
    fn cif_fixture_headers_match_the_c_hal() {
        let data = std::fs::read("tests/data/h263_cif_12_ip.263").expect("test fixture");
        let pictures = parse(&data);
        assert_eq!(pictures.len(), 12);
        assert_eq!(pictures[0].coding_type, VOP_I);
        assert!(
            pictures[1..].iter().all(|p| p.coding_type == VOP_P),
            "one I picture then eleven P pictures"
        );
        assert!(
            pictures
                .iter()
                .all(|p| (p.width, p.height, p.header_bits) == (352, 288, 50)),
            "every picture of the stream is CIF and its header is 50 bits"
        );
        let lengths: Vec<u32> = pictures.iter().map(|p| (p.au.1 - p.au.0) as u32).collect();
        assert_eq!(
            lengths,
            [15877, 11081, 8659, 10471, 9192, 11512, 8201, 7305, 5558, 7490, 5588, 4486],
            "sw51.sw_stream_len of the matching C jobs"
        );
        let quants: Vec<u32> = pictures.iter().map(|p| p.quant).collect();
        assert_eq!(
            quants,
            [4, 2, 2, 2, 2, 2, 2, 3, 3, 3, 4, 4],
            "sw51.sw_init_qp of the matching C jobs"
        );
        // The rate controller dropped two pictures, which is why TR does not run
        // 0, 1, 2 ...; the C decoder sees the same gaps.
        let tr: Vec<u32> = pictures.iter().map(|p| p.temporal_reference).collect();
        assert_eq!(tr, [0, 1, 2, 3, 4, 5, 7, 8, 9, 10, 11, 13]);
    }

    #[test]
    fn qcif_and_4cif_sizes_come_from_the_source_format() {
        let fixtures = [
            ("tests/data/h263_qcif_6_ip.263", 176u32, 144u32, 6usize),
            ("tests/data/h263_4cif_10_ip.263", 704, 576, 10),
        ];
        for (path, width, height, count) in fixtures {
            let data = std::fs::read(path).expect("test fixture");
            let pictures = parse(&data);
            assert_eq!(pictures.len(), count, "{path}");
            assert!(
                pictures
                    .iter()
                    .all(|p| (p.width, p.height) == (width, height)),
                "{path} is {width}x{height}"
            );
        }
    }

    /// Access units tile the stream from the first start code on, and the last
    /// one runs to the end of the buffer like the C splitter's EOS branch.
    #[test]
    fn access_units_tile_the_stream() {
        let data = std::fs::read("tests/data/h263_qcif_6_ip.263").expect("test fixture");
        let units = access_units(&data);
        assert_eq!(units[0].0, 0);
        assert_eq!(*units.last().unwrap(), (27015, data.len()));
        for pair in units.windows(2) {
            assert_eq!(pair[0].1, pair[1].0, "units are contiguous");
            assert!(pair[0].0 < pair[0].1, "units are not empty");
        }
    }

    /// Anything ahead of the first I picture is dropped, not an error, and a
    /// header the C parser refuses costs one picture and nothing more.
    #[test]
    fn pictures_ahead_of_the_first_intra_are_dropped() {
        let data = std::fs::read("tests/data/h263_cif_12_ip.263").expect("test fixture");
        let units = access_units(&data);
        let mut stream = Vec::new();
        // A P picture with the PTYPE tail C rejects, then the real stream.
        stream.extend_from_slice(&[0x00, 0x00, 0x80, 0b0001_1000, 0x00, 0x04]);
        stream.extend_from_slice(&data[..units[1].1]);

        let pictures = parse(&stream);
        assert_eq!(
            pictures.len(),
            2,
            "the opening P picture and the malformed one are both dropped"
        );
        assert_eq!(pictures[0].coding_type, VOP_I);
        // The six filler bytes move every access unit of the real stream along.
        assert_eq!(pictures[0].au, (6, 6 + units[0].1));
        assert_eq!(pictures[1].coding_type, VOP_P);
    }
}
