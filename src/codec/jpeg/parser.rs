//! JPEG marker parser, laid out for the Rockchip jpegd hardware.
//!
//! Only what the decoder needs: frame dimensions, component sampling, the
//! quantisation tables, the Huffman tables, the restart interval and the range
//! of entropy-coded data. Table values are kept in **file order (zigzag)**,
//! which is what `jpegd_vpu7xx_write_qtbl`/`write_htbl` expect.
//!
//! Reference: rockchip-linux/mpp `mpp/codec/dec/jpeg/jpegd_parser.c`.

use std::fmt;

pub const SOF0: u8 = 0xc0;
pub const SOF1: u8 = 0xc1;
pub const SOF2: u8 = 0xc2;
pub const DHT: u8 = 0xc4;
pub const SOI: u8 = 0xd8;
pub const EOI: u8 = 0xd9;
pub const SOS: u8 = 0xda;
pub const DQT: u8 = 0xdb;
pub const DRI: u8 = 0xdd;

#[derive(Debug)]
pub enum Error {
    Truncated,
    NotJpeg,
    Unsupported(&'static str),
    BadTable(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Truncated => write!(f, "truncated JPEG"),
            Error::NotJpeg => write!(f, "missing SOI"),
            Error::Unsupported(s) => write!(f, "unsupported: {s}"),
            Error::BadTable(s) => write!(f, "bad table: {s}"),
        }
    }
}

#[derive(Clone, Debug)]
pub struct QuantTable {
    /// 64 values in file (zigzag) order, already widened to 16 bit.
    pub values: [u16; 64],
}

#[derive(Clone, Debug)]
pub struct HuffTable {
    /// `bits[i]` = number of codes of length i (index 1..=16).
    pub bits: [u8; 17],
    /// Symbol values, in code order.
    pub values: Vec<u8>,
}

impl HuffTable {
    pub fn symbol_count(&self) -> usize {
        self.bits[1..].iter().map(|&b| b as usize).sum()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Component {
    pub id: u8,
    pub h: u8,
    pub v: u8,
    pub tq: u8,
    pub td: u8,
    pub ta: u8,
}

#[derive(Clone, Debug)]
pub struct JpegInfo {
    pub width: usize,
    pub height: usize,
    pub components: Vec<Component>,
    pub quant: [Option<QuantTable>; 4],
    pub dc: [Option<HuffTable>; 4],
    pub ac: [Option<HuffTable>; 4],
    pub restart_interval: u16,
    pub progressive: bool,
    pub entropy_start: usize,
    pub entropy_end: usize,
}

impl JpegInfo {
    /// Sampling factors of the luma component, e.g. (2, 2) for 4:2:0.
    pub fn luma_sampling(&self) -> (u8, u8) {
        self.components.first().map(|c| (c.h, c.v)).unwrap_or((1, 1))
    }

    pub fn is_420(&self) -> bool {
        let (h, v) = self.luma_sampling();
        h == 2 && v == 2
    }

    pub fn is_444(&self) -> bool {
        self.components.iter().all(|c| c.h == 1 && c.v == 1)
    }

    pub fn is_422(&self) -> bool {
        let (h, v) = self.luma_sampling();
        h == 2 && v == 1
    }
}

fn be16(d: &[u8], i: usize) -> Result<u16, Error> {
    if i + 1 >= d.len() {
        return Err(Error::Truncated);
    }
    Ok(((d[i] as u16) << 8) | d[i + 1] as u16)
}

/// Parse the headers of a JPEG up to (and including) the start of scan.
pub fn parse(data: &[u8]) -> Result<JpegInfo, Error> {
    if data.len() < 2 || data[0] != 0xff || data[1] != SOI {
        return Err(Error::NotJpeg);
    }

    let mut info = JpegInfo {
        width: 0,
        height: 0,
        components: Vec::new(),
        quant: [None, None, None, None],
        dc: [None, None, None, None],
        ac: [None, None, None, None],
        restart_interval: 0,
        progressive: false,
        entropy_start: 0,
        entropy_end: data.len(),
    };

    let mut i = 2usize;
    loop {
        // markers may be preceded by fill bytes
        while i < data.len() && data[i] != 0xff {
            i += 1;
        }
        while i < data.len() && data[i] == 0xff {
            i += 1;
        }
        if i >= data.len() {
            return Err(Error::Truncated);
        }
        let marker = data[i];
        i += 1;

        match marker {
            EOI => return Err(Error::Unsupported("EOI before SOS")),
            0xd0..=0xd7 | 0x01 => continue, // RSTn / TEM carry no length
            SOS => {
                let len = be16(data, i)? as usize;
                let ncomp = *data.get(i + 2).ok_or(Error::Truncated)? as usize;
                if ncomp == 0 || ncomp > 4 {
                    return Err(Error::BadTable("scan component count"));
                }
                let need = i + 2 + 1 + ncomp * 2 + 3;
                if need > data.len() {
                    return Err(Error::Truncated);
                }
                for k in 0..ncomp {
                    let cs = data[i + 3 + k * 2];
                    let t = data[i + 4 + k * 2];
                    if let Some(c) = info.components.iter_mut().find(|c| c.id == cs) {
                        c.td = t >> 4;
                        c.ta = t & 0x0f;
                    } else {
                        return Err(Error::BadTable("scan references unknown component"));
                    }
                }
                info.entropy_start = i + len; // i is at the length field, len includes it
                if info.entropy_start > data.len() {
                    return Err(Error::Truncated);
                }
                break;
            }
            SOF0 | SOF1 => {
                let len = be16(data, i)? as usize;
                if len < 8 {
                    return Err(Error::Truncated);
                }
                info.height = be16(data, i + 3)? as usize;
                info.width = be16(data, i + 5)? as usize;
                let ncomp = data[i + 7] as usize;
                if ncomp == 0 || ncomp > 4 {
                    return Err(Error::Unsupported("component count"));
                }
                if i + 8 + ncomp * 3 > data.len() {
                    return Err(Error::Truncated);
                }
                info.components.clear();
                for k in 0..ncomp {
                    let o = i + 8 + k * 3;
                    let hv = data[o + 1];
                    info.components.push(Component {
                        id: data[o],
                        h: hv >> 4,
                        v: hv & 0x0f,
                        tq: data[o + 2],
                        td: 0,
                        ta: 0,
                    });
                }
                i += len;
            }
            SOF2 => {
                info.progressive = true;
                let len = be16(data, i)? as usize;
                info.height = be16(data, i + 3)? as usize;
                info.width = be16(data, i + 5)? as usize;
                let ncomp = data[i + 7] as usize;
                if ncomp == 0 || ncomp > 4 || i + 8 + ncomp * 3 > data.len() {
                    return Err(Error::Unsupported("progressive component count"));
                }
                info.components.clear();
                for k in 0..ncomp {
                    let o = i + 8 + k * 3;
                    let hv = data[o + 1];
                    info.components.push(Component {
                        id: data[o],
                        h: hv >> 4,
                        v: hv & 0x0f,
                        tq: data[o + 2],
                        td: 0,
                        ta: 0,
                    });
                }
                i += len;
            }
            DQT => {
                let len = be16(data, i)? as usize;
                let end = i + len;
                if end > data.len() {
                    return Err(Error::Truncated);
                }
                let mut p = i + 2;
                while p < end {
                    let pq_tq = data[p];
                    let pq = (pq_tq >> 4) as usize;
                    let tq = (pq_tq & 0x0f) as usize;
                    p += 1;
                    if tq > 3 {
                        return Err(Error::BadTable("quant table index"));
                    }
                    let bytes = if pq == 0 { 1 } else { 2 };
                    if p + 64 * bytes > end {
                        return Err(Error::Truncated);
                    }
                    let mut t = QuantTable { values: [0; 64] };
                    for k in 0..64 {
                        t.values[k] = if bytes == 1 {
                            data[p + k] as u16
                        } else {
                            ((data[p + k * 2] as u16) << 8) | data[p + k * 2 + 1] as u16
                        };
                    }
                    info.quant[tq] = Some(t);
                    p += 64 * bytes;
                }
                i = end;
            }
            DHT => {
                let len = be16(data, i)? as usize;
                let end = i + len;
                if end > data.len() {
                    return Err(Error::Truncated);
                }
                let mut p = i + 2;
                while p < end {
                    let tc_th = data[p];
                    let tc = (tc_th >> 4) as usize;
                    let th = (tc_th & 0x0f) as usize;
                    p += 1;
                    if tc > 1 || th > 3 || p + 16 > end {
                        return Err(Error::BadTable("huffman table header"));
                    }
                    let mut bits = [0u8; 17];
                    let mut total = 0usize;
                    for k in 1..=16 {
                        bits[k] = data[p + k - 1];
                        total += bits[k] as usize;
                    }
                    p += 16;
                    if p + total > end {
                        return Err(Error::Truncated);
                    }
                    let h = HuffTable {
                        bits,
                        values: data[p..p + total].to_vec(),
                    };
                    p += total;
                    if tc == 0 {
                        info.dc[th] = Some(h);
                    } else {
                        info.ac[th] = Some(h);
                    }
                }
                i = end;
            }
            DRI => {
                info.restart_interval = be16(data, i + 2)?;
                i += be16(data, i)? as usize;
            }
            _ => {
                let len = be16(data, i)? as usize;
                if len < 2 {
                    return Err(Error::BadTable("segment length"));
                }
                i += len;
            }
        }
        if i > data.len() {
            return Err(Error::Truncated);
        }
    }

    if info.width == 0 || info.height == 0 || info.components.is_empty() {
        return Err(Error::Unsupported("missing frame header"));
    }

    // Find EOI to bound the entropy data (tolerate a missing one).
    let mut j = info.entropy_start;
    while j + 1 < data.len() {
        if data[j] == 0xff {
            let m = data[j + 1];
            if m != 0x00 && !(0xd0..=0xd7).contains(&m) {
                if m == EOI {
                    info.entropy_end = j;
                    break;
                }
                // another segment after the scan (progressive): stop here
                info.entropy_end = j;
                break;
            }
        }
        j += 1;
    }
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUS: &str = "/home/user/projects/rknn-projects-test/baseline/images/bus.jpg";
    const COCO: &str = "/home/user/projects/rknn-projects-test/baseline/images/coco/000000288685.jpg";

    #[test]
    fn rejects_non_jpeg() {
        assert!(matches!(parse(b"not a jpeg"), Err(Error::NotJpeg)));
        assert!(matches!(parse(&[0xff, 0xd8]), Err(Error::Truncated)));
    }

    #[test]
    fn parses_bus_4x2x0() {
        let Ok(data) = std::fs::read(BUS) else { return };
        let info = parse(&data).unwrap();
        assert_eq!((info.width, info.height), (640, 640));
        assert_eq!(info.components.len(), 3);
        assert!(info.is_420(), "bus.jpg should be 4:2:0, sampling {:?}", info.luma_sampling());
        assert!(info.quant[0].is_some());
        assert!(info.ac[0].as_ref().unwrap().symbol_count() > 0);
        assert!(info.entropy_start < info.entropy_end);
        assert!(!info.progressive);
    }

    #[test]
    fn parses_coco_444() {
        let Ok(data) = std::fs::read(COCO) else { return };
        let info = parse(&data).unwrap();
        assert_eq!((info.width, info.height), (640, 436));
        assert!(info.is_444(), "sampling {:?}", info.luma_sampling());
        assert_eq!(info.entropy_end, data.len().saturating_sub(2).min(info.entropy_end));
    }
}
