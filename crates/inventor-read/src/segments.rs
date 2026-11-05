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
