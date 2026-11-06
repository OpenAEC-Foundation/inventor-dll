//! The display cache of assemblies (SPEC §5): the bzip2 stream
//! `CacheGraphics/OGSCache/OGS/DefaultScene/streams`, its blocks, and the `VertexFormat`,
//! `VertexBuffer` and `AIndexBuffer` objects of the `world` block that point into
//! `stream_mesh_000`. Coordinates are in assembly space, centimetres.

use crate::bytes::{utf16, Rd};
use std::collections::{HashMap, HashSet};
use std::io::Read;

pub const STREAM: &str = "/CacheGraphics/OGSCache/OGS/DefaultScene/streams";

/// Upper bound of the unpacked cache.
const MAX_UNPACKED: u64 = 2 << 30;

/// The unpacked blocks: name and bytes (SPEC §5.2).
pub fn blocks(data: &[u8]) -> Vec<(String, &[u8])> {
    let mut r = Rd::new(data);
    let mut out = Vec::new();
    while r.remaining() >= 12 {
        let Some(b) = r.u32() else { break };
        if b == 0 || b > 4096 || b % 2 != 0 {
            break;
        }
        let Some(name) = r.bytes(b as usize).map(utf16) else { break };
        let Some(l) = r.u64() else { break };
        let l = usize::try_from(l).unwrap_or(usize::MAX).min(r.remaining());
        let Some(body) = r.bytes(l) else { break };
        out.push((name, body));
    }
    out
}

pub fn unpack(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    bzip2::read::MultiBzDecoder::new(data).take(MAX_UNPACKED).read_to_end(&mut out).map_err(|e| format!("bzip2: {e}"))?;
    Ok(out)
}

/// One element of a vertex format: components, semantic, offset in floats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Element {
    pub components: u32,
    /// 0 position, 1 normal, 2 texture coordinate (set in the high byte), 3/4 extra vectors.
    pub semantic: u32,
    pub offset: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VertexFormat {
    pub elements: Vec<Element>,
}

impl VertexFormat {
    /// Bytes per vertex.
    pub fn stride(&self) -> usize {
        4 * self.elements.iter().map(|e| (e.offset + e.components) as usize).max().unwrap_or(0)
    }

    fn find(&self, semantic: u32, components: u32) -> Option<Element> {
        self.elements.iter().copied().find(|e| e.semantic & 0xFF == semantic && e.components >= components)
    }
}

/// A vertex buffer: its format and its bytes in `stream_mesh_000`, and the index buffers that
/// follow it.
#[derive(Debug, Clone, PartialEq)]
pub struct VertexBuffer {
    pub format: Option<VertexFormat>,
    pub offset: usize,
    pub length: usize,
    /// (offset, byte length) of each index buffer (u16 indices).
    pub index_buffers: Vec<(usize, usize)>,
}

/// What the objects of `world` describe.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct World {
    pub vertex_buffers: Vec<VertexBuffer>,
    /// Index buffers before any vertex buffer.
    pub orphan_index_buffers: usize,
}

const CLASSES: [&str; 3] = ["VertexFormat", "VertexBuffer", "AIndexBuffer"];

fn class_pattern(name: &str) -> Vec<u8> {
    let mut p = (name.len() as u32).to_le_bytes().to_vec();
    for u in name.encode_utf16() {
        p.extend(u.to_le_bytes());
    }
    p
}

/// An object header at `at`: class, object id and the position after it (SPEC §5.3).
fn header(d: &[u8], at: usize) -> Option<(usize, String, usize)> {
    let mut r = Rd::at(d, at);
    let n = r.u32()? as usize;
    if n == 0 || n > 64 {
        return None;
    }
    let name = r.utf16(n)?;
    let class = CLASSES.iter().position(|c| *c == name)?;
    let m = r.u32()? as usize;
    if m == 0 || m > 64 {
        return None;
    }
    let id = r.utf16(m)?;
    if !id.chars().all(|c| c.is_ascii_alphanumeric()) || id.chars().count() != m {
        return None;
    }
    Some((class, id, r.p))
}

/// `u32 c`, then `c` × 6 u32 `[components − 1, semantic, offset, 0, 0, 0]`.
fn format_definition(d: &[u8], at: usize) -> Option<(VertexFormat, usize)> {
    let mut r = Rd::at(d, at);
    let c = r.u32()?;
    if c == 0 || c > 16 {
        return None;
    }
    let mut elements = Vec::new();
    for _ in 0..c {
        let (n, semantic, offset) = (r.u32()?, r.u32()?, r.u32()?);
        r.skip(12)?;
        if n > 15 || offset > 256 {
            return None;
        }
        elements.push(Element { components: n + 1, semantic, offset });
    }
    Some((VertexFormat { elements }, r.p))
}

/// The words `1, 0, offset, length, 1` at `at` or (when `search`) within the next bytes.
fn buffer_range(d: &[u8], at: usize, mesh_len: usize, search: bool) -> Option<(usize, usize, usize)> {
    let limit = if search { 4096 } else { 0 };
    for p in at..=at.saturating_add(limit).min(d.len()) {
        let mut r = Rd::at(d, p);
        let (Some(1), Some(0), Some(off), Some(len), Some(1)) = (r.u32(), r.u32(), r.u32(), r.u32(), r.u32()) else { continue };
        let (off, len) = (off as usize, len as usize);
        if len > 0 && off.checked_add(len).is_some_and(|e| e <= mesh_len) {
            return Some((off, len, r.p));
        }
    }
    None
}

/// Reads the vertex and index buffers of `world`, in file order.
pub fn world(d: &[u8], mesh_len: usize) -> World {
    let patterns: Vec<Vec<u8>> = CLASSES.iter().map(|c| class_pattern(c)).collect();
    let mut defined: HashSet<String> = HashSet::new();
    let mut formats: HashMap<String, VertexFormat> = HashMap::new();
    let mut out = World::default();
    let mut i = 0;
    while i + 4 <= d.len() {
        // a class name with its length in front ("VertexFormat" etc. are 12 characters)
        if d[i] != 12 || !patterns.iter().any(|p| d[i..].starts_with(p)) {
            i += 1;
            continue;
        }
        let Some((class, id, end)) = header(d, i) else {
            i += 1;
            continue;
        };
        let first = defined.insert(id.clone());
        i = end;
        if !first {
            continue; // a reference to an object read before
        }
        match class {
            0 => {
                if let Some((f, e)) = format_definition(d, end) {
                    formats.insert(id, f);
                    i = e;
                }
            }
            1 => {
                // u8 0, u8 1, a VertexFormat header (definition or reference), then the range
                let mut r = Rd::at(d, end);
                if r.u8() != Some(0) || r.u8() != Some(1) {
                    continue;
                }
                let Some((fc, fid, fend)) = header(d, r.p) else { continue };
                if fc != 0 {
                    continue;
                }
                let mut after = fend;
                let mut search = false;
                if defined.insert(fid.clone()) {
                    if let Some((f, e)) = format_definition(d, fend) {
                        formats.insert(fid.clone(), f);
                        after = e;
                    }
                    // unknown bytes follow a first definition (SPEC §5.3 [H])
                    search = true;
                }
                let Some((offset, length, e)) = buffer_range(d, after, mesh_len, search) else { continue };
                out.vertex_buffers.push(VertexBuffer { format: formats.get(&fid).cloned(), offset, length, index_buffers: Vec::new() });
                i = e;
            }
            _ => {
                // u8 0 | u32 0 | u32 offset | u32 byte length | u32 1
                let mut r = Rd::at(d, end);
                let (Some(0), Some(0), Some(off), Some(len), Some(1)) = (r.u8(), r.u32(), r.u32(), r.u32(), r.u32()) else { continue };
                let (off, len) = (off as usize, len as usize);
                if len == 0 || off.checked_add(len).is_none_or(|e| e > mesh_len) {
                    continue;
                }
                match out.vertex_buffers.last_mut() {
                    // an index buffer belongs to the last vertex buffer read (SPEC §5.4 [H])
                    Some(vb) => vb.index_buffers.push((off, len)),
                    None => out.orphan_index_buffers += 1,
                }
                i = r.p;
            }
        }
    }
    out
}

/// A triangle mesh from the cache, in centimetres (assembly space).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CacheMesh {
    pub positions: Vec<f32>,
    pub normals: Vec<f32>,
    pub indices: Vec<u32>,
}

/// What [`meshes`] left out.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CacheStats {
    pub vertex_buffers: usize,
    /// Buffers with positions only (edges, SPEC §5.4): left out of the shaded view.
    pub line_buffers: usize,
    /// Buffers without a known format, or whose index buffers point past their vertices.
    pub invalid: usize,
    /// Smaller levels of detail that were left out.
    pub lods_dropped: usize,
    /// Buffers left out because the buffers before them already read more than the cache
    /// holds (ranges shared many times over in damaged data).
    pub over_budget: usize,
}

fn f32_at(d: &[u8], at: usize) -> f32 {
    d.get(at..at + 4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).unwrap_or(f32::NAN)
}

/// The triangle meshes of the cache: per vertex buffer with normals, its largest valid index
/// buffer (the finest level of detail).
pub fn meshes(world: &World, mesh: &[u8]) -> (Vec<CacheMesh>, CacheStats) {
    let mut stats = CacheStats { vertex_buffers: world.vertex_buffers.len(), ..Default::default() };
    let mut out = Vec::new();
    // what the buffers may read in all: twice the mesh stream, plus some slack
    let mut budget = mesh.len().saturating_mul(2) + (16 << 20);
    for vb in &world.vertex_buffers {
        let cost = vb.length + vb.index_buffers.iter().map(|ib| ib.1).sum::<usize>();
        if cost > budget {
            stats.over_budget += 1;
            continue;
        }
        budget -= cost;
        let Some(format) = &vb.format else {
            stats.invalid += 1;
            continue;
        };
        let stride = format.stride();
        let Some(pos) = format.find(0, 3) else {
            stats.invalid += 1;
            continue;
        };
        let Some(nrm) = format.find(1, 3) else {
            stats.line_buffers += 1;
            continue;
        };
        if stride == 0 {
            stats.invalid += 1;
            continue;
        }
        let count = vb.length / stride;
        let indices = |&(off, len): &(usize, usize)| -> Option<Vec<u32>> {
            let raw = mesh.get(off..off + len)?;
            let idx: Vec<u32> = raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]]) as u32).collect();
            idx.iter().all(|&i| (i as usize) < count).then_some(idx)
        };
        let valid: Vec<Vec<u32>> = vb.index_buffers.iter().filter_map(indices).filter(|i| i.len() >= 3).collect();
        let Some(best) = valid.iter().max_by_key(|i| i.len()) else {
            stats.invalid += 1;
            continue;
        };
        stats.lods_dropped += valid.len() - 1;
        let mut m = CacheMesh { positions: Vec::with_capacity(count * 3), normals: Vec::with_capacity(count * 3), indices: Vec::new() };
        for v in 0..count {
            let base = vb.offset + v * stride;
            for k in 0..3 {
                m.positions.push(f32_at(mesh, base + 4 * (pos.offset as usize + k)));
                m.normals.push(f32_at(mesh, base + 4 * (nrm.offset as usize + k)));
            }
        }
        if m.positions.iter().any(|v| !v.is_finite()) {
            stats.invalid += 1;
            continue;
        }
        m.indices = best[..best.len() / 3 * 3].to_vec();
        out.push(m);
    }
    (out, stats)
}
