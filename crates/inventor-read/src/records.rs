//! The records of an unpacked segment. A segment is a run of records, each an 8-byte header
//! (a class index and flags), a body and a `u32` length (body + 4), and ends with a trailer
//! that starts with `FF FF FF FF`. From Inventor 2015 or so on a zero byte separates the records
//! and precedes the trailer; older files have none. The lengths stand behind the bodies, so
//! the records are found from the end backwards.

/// Bounds for damaged input.
const MAX_RECORDS: usize = 4_000_000;

/// One record: its header and its body.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Record<'a> {
    pub header: [u8; 8],
    pub body: &'a [u8],
}

impl Record<'_> {
    /// The class index of the record (an index into the class table of the `M` stream).
    pub fn class(&self) -> u16 {
        u16::from_le_bytes([self.header[0], self.header[1]])
    }
}

/// The records of a segment, in file order (record numbers in references are 1-based).
pub fn frame(data: &[u8]) -> Option<Vec<Record<'_>>> {
    let trailer = data.windows(4).rposition(|w| w == [0xFF; 4])?;
    // with a separator first: a file without one fails on the separator check
    for separator in [true, false] {
        let end = if separator {
            if trailer == 0 || data[trailer - 1] != 0 {
                continue;
            }
            trailer - 1
        } else {
            trailer
        };
        if let Some(records) = frame_from(data, end, separator) {
            return Some(records);
        }
    }
    None
}

fn frame_from(data: &[u8], mut end: usize, separator: bool) -> Option<Vec<Record<'_>>> {
    let mut out = Vec::new();
    while end > 0 {
        if end < 12 || out.len() >= MAX_RECORDS {
            return None;
        }
        let size = u32::from_le_bytes(data[end - 4..end].try_into().ok()?) as usize;
        let header = end.checked_sub(size.checked_add(8)?)?;
        if size < 4 {
            return None;
        }
        let body = &data[header + 8..end - 4];
        out.push(Record { header: data[header..header + 8].try_into().ok()?, body });
        end = if header == 0 {
            0
        } else if separator {
            if data[header - 1] != 0 {
                return None;
            }
            header - 1
        } else {
            header
        };
    }
    out.reverse();
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(class: u16, body: &[u8]) -> Vec<u8> {
        let mut d = class.to_le_bytes().to_vec();
        d.extend([0u8; 6]);
        d.extend(body);
        d.extend((body.len() as u32 + 4).to_le_bytes());
        d
    }

    #[test]
    fn with_and_without_separators() {
        let (a, b) = (record(0x100, &[1, 2, 3, 4, 5]), record(0x102, &[9; 12]));
        let trailer = [0xFF, 0xFF, 0xFF, 0xFF, 2, 0, 2, 0, 0, 0];
        let old: Vec<u8> = [a.clone(), b.clone(), trailer.to_vec()].concat();
        let new: Vec<u8> = [a, vec![0], b, vec![0], trailer.to_vec()].concat();
        for d in [old, new] {
            let r = frame(&d).unwrap();
            assert_eq!(r.len(), 2);
            assert_eq!((r[0].class(), r[0].body), (0x100, &[1u8, 2, 3, 4, 5][..]));
            assert_eq!((r[1].class(), r[1].body.len()), (0x102, 12));
        }
        assert!(frame(&[1, 2, 3]).is_none());
    }
}
