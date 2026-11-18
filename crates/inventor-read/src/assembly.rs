//! The placed parts of an assembly: which file each occurrence shows and where it stands.
//!
//! Three places of the file together give that:
//!
//! - `AmRxSegment`: record 1 lists the occurrence records (`01 00`, `02 00 00 30`, count,
//!   capacity, a `u32`, then one reference per occurrence: a record number with `8` in the
//!   highest four bits). An occurrence record holds a reference to its descriptor at +2 and its
//!   placement as a compact 4×4 matrix at +6 (at +10 after `03 02 00 00`). The descriptor has
//!   the occurrence number at +4, followed by the counted text `AmRx`.
//! - `UFRxDoc`, the referenced files: the stored path of each file as counted UTF-16, the index
//!   in that path where the part relative to the project folder starts, two counted texts (the
//!   library and the display name), two 16-byte ids, and the file number and the number of
//!   occurrences of the file.
//! - `UFRxDoc`, the occurrences: file number, occurrence number, an index, the instance
//!   number and the occurrence name as counted UTF-16 (often empty), followed by a list of
//!   properties that starts with its count twice and property `0F`.
//!
//! - `DesignViewSegment`, what is shown: record 1 lists the design views and ends with the
//!   name of the active one. A view starts with its name and lists records, among them one
//!   per occurrence it says something about: a reference to a path record and a byte, 0 for
//!   hidden. The path record holds `02 00 00 30`, the number of occurrence numbers, two more
//!   words and the occurrence numbers from the assembly inwards. (Newer files have path and
//!   byte in one record.)
//! - `UFRxDoc`, what is suppressed: after the counted text `[Primary]` and two words, a count
//!   and the occurrence numbers as counted texts of digits, each followed by seven bytes.
//!
//! Lengths are in centimetres, as in the B-rep.

use crate::bytes::Rd;
use crate::records::{self, Record};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

/// A row-major 4×4 matrix; the translation is the last column, in centimetres.
pub type Matrix = [[f64; 4]; 4];

pub const IDENTITY: Matrix = [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]];

/// A file the assembly refers to.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FileRef {
    /// The number occurrences refer to the file by.
    pub id: u32,
    /// The path as stored: absolute, on the computer the assembly was saved on.
    pub path: String,
    /// The part of `path` below the project folder, when the file is stored as such.
    pub relative: Option<String>,
    /// The name Inventor shows for the file (empty in older files).
    pub display_name: String,
    /// How many occurrences the file has according to the file list.
    pub occurrences: u32,
}

impl FileRef {
    /// The file name without its folders.
    pub fn file_name(&self) -> &str {
        crate::refs::file_name(&self.path)
    }
}

/// One placed part or subassembly.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Occurrence {
    pub id: u32,
    /// The occurrence name (`Bracket:2`); made from the file name and the instance number when
    /// the file does not store it here.
    pub name: String,
    /// Index into `Assembly::files`; `None` when the file of the occurrence was not found.
    pub file: Option<usize>,
    /// Occurrence space to assembly space.
    pub matrix: Matrix,
    /// The name of the appearance the occurrence is shown in, when it has one of its own.
    pub appearance: Option<String>,
    /// Shown in the active design view and not suppressed.
    pub visible: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Assembly {
    pub files: Vec<FileRef>,
    pub occurrences: Vec<Occurrence>,
    /// Occurrence records that could not be read.
    pub unreadable: usize,
    /// Occurrences inside subassemblies that the active design view hides: the occurrence
    /// numbers from this assembly inwards.
    pub hidden_inside: Vec<Vec<u32>>,
}

/// The compact matrix of an occurrence: two `u16` masks, then a `f64` for every entry that
/// neither mask covers, row by row. An entry in the first mask only is 1, in the second only
/// 0, in both −1.
pub(crate) fn compact_matrix(r: &mut Rd) -> Option<Matrix> {
    let (ones, zeros) = (r.u16()?, r.u16()?);
    let mut m = [[0.0; 4]; 4];
    for i in 0..16 {
        let (one, zero) = (ones >> i & 1 != 0, zeros >> i & 1 != 0);
        m[i / 4][i % 4] = match (one, zero) {
            (true, false) => 1.0,
            (false, true) => 0.0,
            (true, true) => -1.0,
            (false, false) => r.f64()?,
        };
    }
    let rigid = m[3] == [0.0, 0.0, 0.0, 1.0] && m.iter().flatten().all(|v| v.is_finite() && v.abs() < 1e9);
    rigid.then_some(m)
}

pub fn multiply(a: &Matrix, b: &Matrix) -> Matrix {
    let mut m = [[0.0; 4]; 4];
    for (i, row) in m.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (0..4).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    m
}

/// A record number from a reference with tag `8` in its highest four bits (1-based).
fn reference(body: &[u8], at: usize) -> Option<usize> {
    let v = Rd::at(body, at).u32()?;
    (v >> 28 == 8).then_some((v & 0x0FFF_FFFF) as usize).filter(|&n| n >= 1)
}

/// The occurrences of `AmRxSegment`: (occurrence number, matrix); `None` for a record that is
/// not as described.
fn placements(records: &[Record]) -> Result<Vec<Option<(u32, Matrix, Option<String>)>>, String> {
    let root = records.first().ok_or("no records")?.body;
    if root.len() < 18 || root[..2] != [1, 0] || root[2..6] != [2, 0, 0, 0x30] {
        return Err("the list of occurrences is not as expected".into());
    }
    let count = Rd::at(root, 6).u32().unwrap_or(0) as usize;
    if count > records.len() || 18 + 4 * count > root.len() {
        return Err("the list of occurrences is longer than the segment".into());
    }
    let record = |n: usize| records.get(n - 1);
    Ok((0..count)
        .map(|k| {
            let body = record(reference(root, 18 + 4 * k)?)?.body;
            let descriptor = record(reference(body, 2)?)?.body;
            let id = Rd::at(descriptor, 4).u32()?;
            if descriptor.get(8..20)? != [4, 0, 0, 0, b'A', 0, b'm', 0, b'R', 0, b'x', 0] {
                return None;
            }
            let start = if body.get(6..10)? == [3, 2, 0, 0] { 10 } else { 6 };
            Some((id, compact_matrix(&mut Rd::at(body, start))?, descriptor_appearance(descriptor)))
        })
        .collect())
}

/// The appearance of an occurrence: the first counted text of its descriptor behind the two
/// 16-byte ids that follow `AmRx` (newer files; older descriptors end before it).
fn descriptor_appearance(descriptor: &[u8]) -> Option<String> {
    (58..descriptor.len().saturating_sub(8)).find_map(|at| {
        let mut r = Rd::at(descriptor, at);
        let s = text(&mut r, 96)?;
        // a text of letters, not a number that happens to be followed by enough bytes
        (s.chars().count() >= 3
            && s.chars().filter(|c| c.is_alphabetic()).count() >= 3
            && s.chars().all(|c| c as u32 >= 0x20 && c as u32 != 0xFFFD && (c as u32) < 0x3000))
        .then_some(s)
    })
}

/// A counted UTF-16 text of at most `max` characters without control characters.
fn text(r: &mut Rd, max: usize) -> Option<String> {
    let n = r.u32()? as usize;
    if n > max {
        return None;
    }
    let units: Vec<u16> = r.bytes(2 * n)?.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let s = String::from_utf16(&units).ok()?;
    s.chars().all(|c| !c.is_control()).then_some(s)
}

/// The referenced files of `UFRxDoc`.
pub fn files(ufrx: &[u8]) -> Vec<FileRef> {
    let mut out: Vec<FileRef> = Vec::new();
    let mut i = 0;
    while i + 8 <= ufrx.len() {
        let mut r = Rd::at(ufrx, i);
        let found = text(&mut r, 1024).filter(|s| {
            let lower = s.to_ascii_lowercase();
            s.len() >= 5 && (lower.ends_with(".ipt") || lower.ends_with(".iam"))
        });
        let Some(path) = found else {
            i += 1;
            continue;
        };
        i = r.p;
        let block = (|| {
            let units = path.encode_utf16().count() as u32;
            let start = r.u32()?;
            if start != u32::MAX && start >= units {
                return None;
            }
            let _library = text(&mut r, 512)?;
            r.u16()?;
            let display_name = text(&mut r, 512)?;
            r.skip(8 + 32)?;
            let (id, occurrences) = (r.u32()?, r.u32()?);
            let relative = (start != u32::MAX).then(|| String::from_utf16_lossy(&path.encode_utf16().skip(start as usize).collect::<Vec<u16>>()));
            (id >= 1 && id < 1 << 24 && occurrences < 1 << 24).then_some(FileRef { id, path: path.clone(), relative, display_name, occurrences })
        })();
        // the list is stored once per saved state of the assembly: the first one counts
        if let Some(f) = block {
            if !out.iter().any(|o| o.id == f.id) {
                out.push(f);
            }
        }
    }
    out
}

/// The occurrences of `UFRxDoc`: occurrence number → (file number, instance number, name).
fn links(ufrx: &[u8], files: &HashSet<u32>, ids: &HashSet<u32>) -> HashMap<u32, (u32, u32, String)> {
    let mut out = HashMap::new();
    let u32_at = |p: usize| ufrx.get(p..p + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    // the property list behind the name: its count twice, then property 0F
    let properties = |p: usize| {
        (4..=40).any(|skip| {
            let q = p + skip;
            matches!((u32_at(q), u32_at(q + 4)), (Some(a), Some(b)) if a == b && (1..=64).contains(&a))
                && ufrx.get(q + 8..q + 15) == Some(&[1, 0x0F, 1, 0, 0, 0, 0x0F])
        })
    };
    let mut p = 0;
    while p + 20 <= ufrx.len() {
        let entry = (|| {
            let mut r = Rd::at(ufrx, p);
            let (file, id, index, instance) = (r.u32()?, r.u32()?, r.u32()?, r.u32()?);
            if !files.contains(&file) || !ids.contains(&id) || index == 0 || index > 1 << 24 || instance == 0 || instance > 1 << 24 {
                return None;
            }
            let name = text(&mut r, 512)?;
            properties(r.p).then_some((file, id, instance, name, r.p))
        })();
        match entry {
            Some((file, id, instance, name, end)) => {
                out.entry(id).or_insert((file, instance, name));
                p = end;
            }
            None => p += 1,
        }
    }
    out
}

/// The occurrences the active design view hides, as paths of occurrence numbers from the
/// assembly inwards, from the unpacked `DesignViewSegment`.
pub fn hidden(design_views: &[u8]) -> Vec<Vec<u32>> {
    let Some(records) = records::frame(design_views) else { return Vec::new() };
    let record = |n: usize| records.get(n - 1).map(|r| r.body);
    // a counted list: `02 00 00 30`, count, capacity, a word, the references
    let list = |body: &[u8], at: usize| -> Option<(Vec<usize>, usize)> {
        if body.get(at..at + 4)? != [2, 0, 0, 0x30] {
            return None;
        }
        let count = Rd::at(body, at + 4).u32()? as usize;
        if count > records.len() {
            return None;
        }
        let refs = (0..count).map(|k| reference(body, at + 16 + 4 * k)).collect::<Option<Vec<usize>>>()?;
        Some((refs, at + 16 + 4 * count))
    };
    let view = (|| {
        let root = records.first()?.body;
        let (views, end) = list(root, 2)?;
        let active = text(&mut Rd::at(root, end), 256)?;
        // the view of that name, else the first
        let named = views.iter().find(|&&v| record(v).and_then(|b| text(&mut Rd::at(b, 2), 256)).as_deref() == Some(active.as_str()));
        record(*named.or(views.first())?)
    })();
    let Some(view) = view else { return Vec::new() };
    let mut name = Rd::at(view, 2);
    if text(&mut name, 256).is_none() {
        return Vec::new();
    }
    let Some((entries, _)) = list(view, name.p) else { return Vec::new() };
    // the occurrence numbers of a path at `at`: `02 00 00 30`, count, two words, the numbers
    let path = |body: &[u8], at: usize| -> Option<(Vec<u32>, usize)> {
        if body.get(at..at + 4)? != [2, 0, 0, 0x30] {
            return None;
        }
        let mut r = Rd::at(body, at + 4);
        let count = r.u32()? as usize;
        r.skip(8)?;
        if count == 0 || count > 64 {
            return None;
        }
        let ids = (0..count).map(|_| r.u32()).collect::<Option<Vec<u32>>>()?;
        Some((ids, r.p))
    };
    let mut out = Vec::new();
    for entry in entries {
        let Some(body) = record(entry) else { continue };
        let found = if body.len() == 7 {
            // a reference to the path record and the byte
            reference(body, 2).and_then(record).and_then(|p| path(p, 2)).map(|(ids, _)| (ids, body[6]))
        } else {
            // the path and the byte in one record
            path(body, 2).filter(|(_, end)| *end + 1 == body.len()).map(|(ids, end)| (ids, body[end]))
        };
        if let Some((ids, 0)) = found {
            out.push(ids);
        }
    }
    out
}

/// The suppressed occurrences of `UFRxDoc`.
pub fn suppressed(ufrx: &[u8]) -> HashSet<u32> {
    let mut marker = 9u32.to_le_bytes().to_vec();
    marker.extend("[Primary]".encode_utf16().flat_map(u16::to_le_bytes));
    marker.extend([2, 0, 0, 0, 0, 0, 0, 0]);
    let mut out = HashSet::new();
    let Some(at) = ufrx.windows(marker.len()).position(|w| w == marker) else { return out };
    let mut r = Rd::at(ufrx, at + marker.len());
    let count = r.u32().unwrap_or(0);
    for _ in 0..count.min(100_000) {
        let Some(id) = text(&mut r, 10).and_then(|t| t.parse::<u32>().ok()) else { break };
        if r.bytes(7) != Some(&[1, 0, 0, 0, 0, 0, 0]) {
            break;
        }
        out.insert(id);
    }
    out
}

/// The files and occurrences of an assembly from its unpacked `AmRxSegment`, its `UFRxDoc`
/// stream and, for what is hidden, its unpacked `DesignViewSegment`.
pub fn read(amrx: &[u8], ufrx: &[u8], design_views: Option<&[u8]>) -> Result<Assembly, String> {
    let records = records::frame(amrx).ok_or("the records of the occurrence segment could not be told apart")?;
    let placements = placements(&records)?;
    let files = files(ufrx);
    let file_ids: HashSet<u32> = files.iter().map(|f| f.id).collect();
    let ids: HashSet<u32> = placements.iter().flatten().map(|p| p.0).collect();
    let links = links(ufrx, &file_ids, &ids);
    let unreadable = placements.iter().filter(|p| p.is_none()).count();
    let hidden = design_views.map(hidden).unwrap_or_default();
    let suppressed = suppressed(ufrx);
    let occurrences = placements
        .into_iter()
        .flatten()
        .map(|(id, matrix, appearance)| {
            let link = links.get(&id);
            let file = link.and_then(|l| files.iter().position(|f| f.id == l.0));
            let name = match (link, file) {
                (Some((_, _, name)), _) if !name.is_empty() => name.clone(),
                (Some((_, instance, _)), Some(f)) => format!("{}:{instance}", stem(files[f].file_name())),
                _ => format!("Occurrence {id}"),
            };
            let visible = !suppressed.contains(&id) && !hidden.iter().any(|h| h.as_slice() == [id]);
            Occurrence { id, name, file, matrix, appearance, visible }
        })
        .collect();
    let hidden_inside = hidden.into_iter().filter(|h| h.len() > 1).collect();
    Ok(Assembly { files, occurrences, unreadable, hidden_inside })
}

/// A file name without its extension.
pub fn stem(name: &str) -> &str {
    name.rsplit_once('.').map(|s| s.0).unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_matrices() {
        // identity
        let d = [0x21, 0x84, 0xDE, 0x7B];
        assert_eq!(compact_matrix(&mut Rd::new(&d)), Some(IDENTITY));
        // a translation in x: entry 3 in neither mask, so stored
        let mut d = vec![0x21, 0x84, 0xD6, 0x7B];
        d.extend(1.7f64.to_le_bytes());
        let m = compact_matrix(&mut Rd::new(&d)).unwrap();
        assert_eq!(m[0], [1.0, 0.0, 0.0, 1.7]);
        assert_eq!(m[1], [0.0, 1.0, 0.0, 0.0]);
        // an entry in both masks is −1
        let d = [0x21, 0x84, 0xDF, 0x7B];
        assert_eq!(compact_matrix(&mut Rd::new(&d)).unwrap()[0][0], -1.0);
        // truncated
        assert_eq!(compact_matrix(&mut Rd::new(&[0x21, 0x84, 0xD6, 0x7B, 1, 2])), None);
    }

    fn counted(s: &str) -> Vec<u8> {
        let mut d = (s.encode_utf16().count() as u32).to_le_bytes().to_vec();
        d.extend(s.encode_utf16().flat_map(u16::to_le_bytes));
        d
    }

    fn words(w: &[u32]) -> Vec<u8> {
        w.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    /// An occurrence segment with separators: the list, then per occurrence its record and
    /// its descriptor.
    fn segment(occurrences: &[(u32, f64)]) -> Vec<u8> {
        let n = occurrences.len() as u32;
        let mut bodies: Vec<Vec<u8>> = Vec::new();
        let mut root = vec![1, 0, 2, 0, 0, 0x30];
        root.extend(words(&[n, n, 0]));
        // records: 1 the list, 2.. the occurrences, then the descriptors
        root.extend((0..n).flat_map(|k| (0x8000_0000 | (2 + k)).to_le_bytes()));
        bodies.push(root);
        for (k, (_, x)) in occurrences.iter().enumerate() {
            let mut b = vec![6, 0];
            b.extend((0x8000_0000 | (2 + n + k as u32)).to_le_bytes());
            if k == 0 {
                b.extend([3, 2, 0, 0]);
            }
            b.extend([0x21, 0x84, 0xD6, 0x7B]);
            b.extend(x.to_le_bytes());
            b.extend([0u8; 124]);
            bodies.push(b);
        }
        for (id, _) in occurrences {
            let mut b = vec![4, 0, 0, 2];
            b.extend(id.to_le_bytes());
            b.extend(counted("AmRx"));
            b.extend([1, 0, 0, 0, 0, 0]);
            bodies.push(b);
        }
        let mut d = Vec::new();
        for (k, b) in bodies.iter().enumerate() {
            d.extend([k as u8, 1, 0, 0, 0, 0, 0, 0]);
            d.extend(b);
            d.extend((b.len() as u32 + 4).to_le_bytes());
            d.push(0);
        }
        d.extend([0xFF, 0xFF, 0xFF, 0xFF, 2, 0, 2, 0, 0, 0]);
        d
    }

    #[test]
    fn occurrences_with_their_files() {
        let amrx = segment(&[(1, 1.5), (4, -2.0), (9, 0.25)]);
        let mut ufrx = vec![7u8; 20];
        // the assembly itself: a path without a file block behind it
        ufrx.extend(counted("C:\\Work\\Frame.iam"));
        ufrx.extend([0u8; 6]);
        for (id, count, path, start, shown) in [(1u32, 2u32, "C:\\Work\\parts\\Leg.ipt", 8u32, ""), (3, 1, "C:\\Lib\\Bolt M4.ipt", u32::MAX, "Bolt M4")] {
            ufrx.extend(counted(path));
            ufrx.extend(start.to_le_bytes());
            ufrx.extend(counted(if shown.is_empty() { "" } else { "Library" }));
            ufrx.extend([1, 0]);
            ufrx.extend(counted(shown));
            ufrx.extend([0u8; 8]);
            ufrx.extend([0xAB; 32]);
            ufrx.extend(words(&[id, count, 1, 1]));
        }
        for (file, id, instance, name) in [(1u32, 1u32, 1u32, "Leg:1"), (1, 4, 2, ""), (3, 9, 1, "")] {
            ufrx.extend(words(&[file, id, id, instance]));
            ufrx.extend(counted(name));
            ufrx.extend([0, 0, 0, 0, 0, 1, 0, 0]);
            ufrx.extend(words(&[14, 14]));
            ufrx.extend([1, 0x0F, 1, 0, 0, 0, 0x0F, 0, 0, 0, 0, 0, 0]);
        }
        let a = read(&amrx, &ufrx, None).unwrap();
        assert_eq!(a.files.len(), 2);
        assert_eq!((a.files[0].id, a.files[0].file_name(), a.files[0].relative.as_deref(), a.files[0].occurrences), (1, "Leg.ipt", Some("parts\\Leg.ipt"), 2));
        assert_eq!((a.files[1].id, a.files[1].display_name.as_str(), a.files[1].relative.as_deref()), (3, "Bolt M4", None));
        let names: Vec<(&str, Option<usize>, f64)> = a.occurrences.iter().map(|o| (o.name.as_str(), o.file, o.matrix[0][3])).collect();
        assert_eq!(names, vec![("Leg:1", Some(0), 1.5), ("Leg:2", Some(0), -2.0), ("Bolt M4:1", Some(1), 0.25)]);
        assert_eq!(a.unreadable, 0);

        // without separators between the records (older files)
        let old: Vec<u8> = {
            let records = records::frame(&amrx).unwrap();
            let mut d = Vec::new();
            for r in &records {
                d.extend(r.header);
                d.extend(r.body);
                d.extend((r.body.len() as u32 + 4).to_le_bytes());
            }
            d.extend([0xFF, 0xFF, 0xFF, 0xFF, 2, 0, 2, 0, 0, 0]);
            d
        };
        assert_eq!(read(&old, &ufrx, None).unwrap(), a);

        // an occurrence the file list does not name stays, without a file
        let a = read(&segment(&[(1, 0.0), (77, 0.0)]), &ufrx, None).unwrap();
        assert_eq!(a.occurrences[1].file, None);
        assert!(read(&[1, 2, 3], &ufrx, None).is_err());
    }

    /// Records with separators and a trailer.
    fn framed(bodies: &[Vec<u8>]) -> Vec<u8> {
        let mut d = Vec::new();
        for (k, b) in bodies.iter().enumerate() {
            d.extend([k as u8, 1, 0, 0, 0, 0, 0, 0]);
            d.extend(b);
            d.extend((b.len() as u32 + 4).to_le_bytes());
            d.push(0);
        }
        d.extend([0xFF, 0xFF, 0xFF, 0xFF, 1, 0, 1, 0, 0, 0]);
        d
    }

    #[test]
    fn hidden_and_suppressed_occurrences() {
        let reference = |n: u32| (0x8000_0000 | n).to_le_bytes();
        let listed = |refs: &[u32]| {
            let mut d = vec![2, 0, 0, 0x30];
            d.extend(words(&[refs.len() as u32, refs.len() as u32, 16]));
            d.extend(refs.iter().flat_map(|&n| reference(n)));
            d
        };
        let view = |name: &str, refs: &[u32]| {
            let mut d = vec![2, 0];
            d.extend(counted(name));
            d.extend(listed(refs));
            d
        };
        let entry = |path: u32, shown: u8| {
            let mut d = vec![5, 0];
            d.extend(reference(path));
            d.push(shown);
            d
        };
        let path = |ids: &[u32]| {
            let mut d = vec![6, 0, 2, 0, 0, 0x30];
            d.extend(words(&[ids.len() as u32, 7, 8]));
            d.extend(words(ids));
            d.extend([0x21, 0x84, 0xDE, 0x7B]);
            d
        };
        // record 1: the views (2 and 3) and the active one, "Open"
        let mut root = vec![1, 0];
        root.extend(listed(&[2, 3]));
        root.extend(counted("Open"));
        let bodies = vec![
            root,
            view("Default", &[4, 5]),
            view("Open", &[4, 6, 7, 8]),
            vec![9; 30],    // 4: something else of a view
            entry(9, 0),    // 5: occurrence 1 hidden, in the view that is not active
            entry(10, 0),   // 6: occurrence 6 hidden
            entry(9, 1),    // 7: occurrence 1 shown
            entry(11, 0),   // 8: occurrence 7 inside occurrence 11 hidden
            path(&[1]),     // 9
            path(&[6]),     // 10
            path(&[11, 7]), // 11
        ];
        assert_eq!(hidden(&framed(&bodies)), vec![vec![6], vec![11, 7]]);
        // path and byte in one record
        let mut inline = vec![5, 0, 2, 0, 0, 0x30];
        inline.extend(words(&[1, 7, 8, 12]));
        inline.push(0);
        let mut root = vec![1, 0];
        root.extend(listed(&[2]));
        root.extend(counted("Default"));
        assert_eq!(hidden(&framed(&[root, view("Default", &[3]), inline])), vec![vec![12]]);
        assert!(hidden(&[1, 2, 3]).is_empty());

        let mut ufrx = vec![3u8; 9];
        ufrx.extend(counted("[Primary]"));
        ufrx.extend(words(&[2, 0, 2]));
        for id in ["4", "12"] {
            ufrx.extend(counted(id));
            ufrx.extend([1, 0, 0, 0, 0, 0, 0]);
        }
        assert_eq!(suppressed(&ufrx), HashSet::from([4, 12]));
        assert!(suppressed(&[0; 40]).is_empty());

        // both reach the occurrences
        let a = read(&segment(&[(1, 0.0), (4, 0.0), (6, 0.0)]), &ufrx, Some(&framed(&bodies))).unwrap();
        assert_eq!(a.occurrences.iter().map(|o| o.visible).collect::<Vec<_>>(), vec![true, false, false]);
        assert_eq!(a.hidden_inside, vec![vec![11, 7]]);
    }

    #[test]
    fn nested_placement() {
        let mut a = IDENTITY;
        a[0][3] = 2.0;
        let mut b = IDENTITY;
        b[1][3] = 3.0;
        let m = multiply(&a, &b);
        assert_eq!((m[0][3], m[1][3]), (2.0, 3.0));
    }
}
