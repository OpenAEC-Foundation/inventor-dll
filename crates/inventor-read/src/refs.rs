//! Referenced files of an assembly (SPEC §6): the UTF-16 strings with a `u32` length prefix
//! (in characters) in `UFRxDoc` that end in `.ipt` or `.iam`.

use crate::bytes::Rd;

pub fn references(ufrx: &[u8]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i + 4 <= ufrx.len() {
        let n = u32::from_le_bytes([ufrx[i], ufrx[i + 1], ufrx[i + 2], ufrx[i + 3]]) as usize;
        if (5..=1024).contains(&n) {
            if let Some(s) = Rd::at(ufrx, i + 4).bytes(2 * n).and_then(text) {
                let lower = s.to_ascii_lowercase();
                if (lower.ends_with(".ipt") || lower.ends_with(".iam")) && !out.contains(&s) {
                    out.push(s);
                    i += 4 + 2 * n;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

/// Printable UTF-16 text (a trailing NUL allowed), else `None`.
fn text(b: &[u8]) -> Option<String> {
    let mut units: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    if units.last() == Some(&0) {
        units.pop();
    }
    let s = String::from_utf16(&units).ok()?;
    s.chars().all(|c| !c.is_control()).then_some(s)
}

/// The file name of a stored path (absolute paths point at the author's computer): the part
/// after the last `\` or `/`.
pub fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_paths() {
        let mut d = vec![1, 2, 3];
        for s in ["C:\\Work\\Pin.ipt", "Pin:1", "D:\\x\\Sub.IAM"] {
            d.extend((s.encode_utf16().count() as u32).to_le_bytes());
            d.extend(s.encode_utf16().flat_map(u16::to_le_bytes));
        }
        assert_eq!(references(&d), vec!["C:\\Work\\Pin.ipt".to_string(), "D:\\x\\Sub.IAM".to_string()]);
        assert_eq!(file_name("C:\\Work\\Pin.ipt"), "Pin.ipt");
    }
}
