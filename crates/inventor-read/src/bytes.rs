//! Bounded little-endian reads from untrusted bytes: every read returns `None` past the end.

#[derive(Clone)]
pub(crate) struct Rd<'a> {
    pub d: &'a [u8],
    pub p: usize,
}

impl<'a> Rd<'a> {
    pub fn new(d: &'a [u8]) -> Self {
        Self { d, p: 0 }
    }

    pub fn at(d: &'a [u8], p: usize) -> Self {
        Self { d, p }
    }

    pub fn remaining(&self) -> usize {
        self.d.len().saturating_sub(self.p)
    }

    pub fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.p.checked_add(n)?;
        let s = self.d.get(self.p..end)?;
        self.p = end;
        Some(s)
    }

    pub fn skip(&mut self, n: usize) -> Option<()> {
        self.bytes(n).map(|_| ())
    }

    pub fn u8(&mut self) -> Option<u8> {
        self.bytes(1).map(|b| b[0])
    }

    pub fn u16(&mut self) -> Option<u16> {
        self.bytes(2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn i16(&mut self) -> Option<i16> {
        self.u16().map(|v| v as i16)
    }

    pub fn u32(&mut self) -> Option<u32> {
        self.bytes(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn i32(&mut self) -> Option<i32> {
        self.u32().map(|v| v as i32)
    }

    pub fn u64(&mut self) -> Option<u64> {
        self.bytes(8).map(|b| u64::from_le_bytes(b.try_into().unwrap()))
    }

    pub fn f32(&mut self) -> Option<f32> {
        self.u32().map(f32::from_bits)
    }

    pub fn f64(&mut self) -> Option<f64> {
        self.u64().map(f64::from_bits)
    }

    /// `n` UTF-16 code units.
    pub fn utf16(&mut self, n: usize) -> Option<String> {
        self.bytes(n.checked_mul(2)?).map(utf16)
    }
}

/// UTF-16LE bytes to a string, up to the first NUL.
pub(crate) fn utf16(b: &[u8]) -> String {
    let units: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
    String::from_utf16_lossy(&units)
}

/// Latin-1 bytes to a string.
pub(crate) fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

/// A GUID as stored ([MS-DTYP]: u32, u16, u16 little-endian, then 8 bytes) in the usual
/// `XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX` form.
pub(crate) fn guid(b: &[u8]) -> String {
    if b.len() < 16 {
        return String::new();
    }
    let d1 = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let d2 = u16::from_le_bytes([b[4], b[5]]);
    let d3 = u16::from_le_bytes([b[6], b[7]]);
    format!("{d1:08X}-{d2:04X}-{d3:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}", b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15])
}

/// The 16 stored bytes of a GUID written as `XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX`.
#[cfg_attr(not(feature = "synth"), allow(dead_code))]
pub(crate) fn guid_bytes(s: &str) -> Option<[u8; 16]> {
    let hex: String = s.chars().filter(|c| *c != '-' && *c != '{' && *c != '}').collect();
    if hex.len() != 32 {
        return None;
    }
    let mut raw = [0u8; 16];
    for (i, byte) in raw.iter_mut().enumerate() {
        *byte = u8::from_str_radix(hex.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    let mut out = raw;
    // Data1..Data3 are little-endian in storage
    out[0..4].copy_from_slice(&[raw[3], raw[2], raw[1], raw[0]]);
    out[4..6].copy_from_slice(&[raw[5], raw[4]]);
    out[6..8].copy_from_slice(&[raw[7], raw[6]]);
    Some(out)
}

/// A FILETIME (100 ns ticks since 1601-01-01 UTC) as ISO 8601, e.g. `2023-04-01T12:30:00Z`.
pub(crate) fn filetime_iso(ticks: u64) -> String {
    let secs = (ticks / 10_000_000) as i64 - 11_644_473_600;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // civil from days (H. Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem / 60 % 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guid_round_trip() {
        let s = "32853F0F-3444-11D1-9E93-0060B03C1CA6";
        assert_eq!(guid(&guid_bytes(s).unwrap()), s);
    }

    #[test]
    fn filetime() {
        // 2000-01-01T00:00:00Z
        assert_eq!(filetime_iso(125_911_584_000_000_000), "2000-01-01T00:00:00Z");
    }

    #[test]
    fn bounded() {
        let mut r = Rd::new(&[1, 0, 0]);
        assert_eq!(r.u16(), Some(1));
        assert_eq!(r.u32(), None);
        assert_eq!(r.u8(), Some(0));
    }
}
