//! Segments in `RSeStorage` (SPEC §3): `B`/`M` stream pairs, the class name from the `M`
//! stream and the compressed data of the `B` stream (zlib or Zstandard).

use crate::bytes::{latin1, Rd};
use serde::Serialize;
use std::io::Read;

/// The constant first 16 bytes of every `B` stream (SPEC §3.4).
pub const B_MAGIC: [u8; 16] = [0x9E, 0xC2, 0x2B, 0xA4, 0xD4, 0x11, 0xE8, 0x01, 0x60, 0x00, 0x2D, 0xB3, 0xEE, 0x29, 0xFB, 0xB0];

/// Upper bound of an unpacked segment (a decompression bomb stops here).
pub const MAX_UNPACKED: u64 = 1 << 30;

/// How a segment is compressed (SPEC §3.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Codec {
    /// 1: a zlib stream (RFC 1950), Inventor 2011–2025.
    Zlib,
    /// 2: a Zstandard frame (RFC 8878), Inventor 2026 and later.
    Zstd,
    /// Another codec byte: the segment is skipped and reported.
    Unknown(u8),
}

impl Codec {
    pub fn from_byte(b: u8) -> Self {
        match b {
            1 => Codec::Zlib,
            2 => Codec::Zstd,
            other => Codec::Unknown(other),
        }
    }
}

/// One segment: the 26-character key, the class from its `M` stream, and its `B` stream.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Segment {
    pub key: String,
    /// E.g. `PmBRepSegment`, `AmBREPSegment` (empty when the `M` stream could not be read).
    pub class: String,
    /// Path of the `B` stream inside the compound file.
    #[serde(skip)]
    pub data_path: String,
    /// Size of the `B` stream in bytes.
    pub stored_size: u64,
    /// The codec byte of the `B` stream; `None` when its header is not the expected one.
    pub codec: Option<Codec>,
}

impl Segment {
    /// True for the B-rep segment (`PmBRepSegment`, `AmBREPSegment`; SPEC §3.3).
    pub fn is_brep(&self) -> bool {
        self.class.to_ascii_lowercase().contains("brep")
    }
}

/// The class name in an `M` stream (SPEC §3.2): `u32 n`, `n` ASCII bytes, 18 bytes, `u32 k`,
/// `2k` bytes UTF-16.
pub fn meta_class(m: &[u8]) -> Option<String> {
    let mut r = Rd::new(m);
    let n = r.u32()? as usize;
    if n > 256 {
        return None;
    }
    let tag = latin1(r.bytes(n)?);
    if !tag.starts_with("RSe Meta Stream") {
        return None;
    }
    r.skip(18)?;
    let k = r.u32()? as usize;
    if k == 0 || k > 512 {
        return None;
    }
    r.utf16(k)
}

/// The unpacked content of an `M` stream: after the class name come a 16-byte id, twelve
/// bytes, two counted ASCII texts (dates), the codec byte and the compressed data.
pub fn meta_unpack(m: &[u8]) -> Option<Vec<u8>> {
    let mut r = Rd::new(m);
    let n = r.u32()? as usize;
    if n > 256 || !latin1(r.bytes(n)?).starts_with("RSe Meta Stream") {
        return None;
    }
    r.skip(18)?;
    let k = r.u32()? as usize;
    if k == 0 || k > 512 {
        return None;
    }
    r.skip(2 * k + 28)?;
    for _ in 0..2 {
        let len = r.u32()? as usize;
        if len > 64 {
            return None;
        }
        r.skip(len)?;
    }
    let codec = Codec::from_byte(r.u8()?);
    inflate(codec, m.get(r.p..)?).ok()
}

/// The class ids of an unpacked `M` stream, by class index: the number of records and their
/// sizes, a second table of ten-byte entries, then one 28-byte entry per class with its
/// 16-byte id at +4. A record's class index is the first word of its header minus 256.
pub fn classes(meta: &[u8]) -> Option<Vec<[u8; 16]>> {
    let count = Rd::at(meta, 14).u32()? as usize;
    let p = 18usize.checked_add(count.checked_mul(4)?)?;
    let second = Rd::at(meta, p.checked_add(4)?).u32()? as usize;
    let start = p.checked_add(8)?.checked_add(second.checked_mul(10)?)?.checked_add(12)?;
    let entries = meta.get(start..)?;
    Some(entries.chunks_exact(28).map(|e| e[4..20].try_into().unwrap_or([0; 16])).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn class_table_of_an_m_stream() {
        // the unpacked table: two records (sizes 5 and 9), one ten-byte entry, two classes
        let mut table = vec![0u8; 14];
        table.extend(2u32.to_le_bytes());
        table.extend([5u32, 9].iter().flat_map(|v| v.to_le_bytes()));
        table.extend([0u32, 1].iter().flat_map(|v| v.to_le_bytes()));
        table.extend([7u8; 10]);
        table.extend([14u32, 0, 4].iter().flat_map(|v| v.to_le_bytes()));
        for class in [0xA1u8, 0xB2] {
            table.extend([0u8; 4]);
            table.extend([class; 16]);
            table.extend([0u8; 8]);
        }
        assert_eq!(classes(&table), Some(vec![[0xA1; 16], [0xB2; 16]]));
        assert_eq!(classes(&table[..16]), None);

        // the stream: name, class name, id, twelve bytes, two dates, the codec, zlib
        let mut m = Vec::new();
        let tag = b"RSe Meta Stream Version 8";
        m.extend((tag.len() as u32).to_le_bytes());
        m.extend(tag);
        m.extend([0u8; 18]);
        let name: Vec<u16> = "PmGraphicsSegment".encode_utf16().collect();
        m.extend((name.len() as u32).to_le_bytes());
        m.extend(name.iter().flat_map(|u| u.to_le_bytes()));
        m.extend([9u8; 28]);
        for date in ["04/04/2021 14:14:10", "04/10/2022 01:43:53"] {
            m.extend((date.len() as u32).to_le_bytes());
            m.extend(date.as_bytes());
        }
        m.push(1);
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(&table).unwrap();
        m.extend(z.finish().unwrap());
        assert_eq!(meta_class(&m).as_deref(), Some("PmGraphicsSegment"));
        assert_eq!(meta_unpack(&m), Some(table));
        assert_eq!(meta_unpack(&m[..60]), None);
    }
}

/// The codec and the compressed payload of a `B` stream (SPEC §3.4).
pub fn data_header(b: &[u8]) -> Option<(Codec, &[u8])> {
    if b.len() < 18 || b[..16] != B_MAGIC || b[16] != 4 {
        return None;
    }
    Some((Codec::from_byte(b[17]), &b[18..]))
}

/// The unpacked content of a `B` stream.
pub fn unpack(b: &[u8]) -> Result<Vec<u8>, String> {
    let (codec, payload) = data_header(b).ok_or("segment data does not start with the expected header")?;
    inflate(codec, payload)
}

fn inflate(codec: Codec, payload: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    match codec {
        Codec::Zlib => {
            flate2::read::ZlibDecoder::new(payload).take(MAX_UNPACKED).read_to_end(&mut out).map_err(|e| format!("zlib: {e}"))?;
        }
        Codec::Zstd => {
            let mut input = payload;
            let decoder = ruzstd::decoding::StreamingDecoder::new(&mut input).map_err(|e| format!("zstd: {e}"))?;
            decoder.take(MAX_UNPACKED).read_to_end(&mut out).map_err(|e| format!("zstd: {e}"))?;
        }
        Codec::Unknown(c) => return Err(format!("unknown segment codec {c}")),
    }
    Ok(out)
}
