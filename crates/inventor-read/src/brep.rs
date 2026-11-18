//! The B-rep of a part (SPEC §4): the token stream after `ASM BinaryFile4` / `ACIS BinaryFile`,
//! split into records, and typed access to the topology and geometry records of §4.6–§4.7.

use crate::bytes::{latin1, Rd};

/// The signatures that start the token stream (SPEC §4.1).
pub const SIGNATURES: [&[u8]; 2] = [b"ASM BinaryFile4", b"ACIS BinaryFile"];

/// Bounds of the token stream (a token takes 32 bytes in memory, so these cap the memory a
/// damaged or hostile segment can ask for at about 0.5 GB).
const MAX_RECORDS: usize = 4_000_000;
const MAX_TOKENS: usize = 16_000_000;
const MAX_RECORD_TOKENS: usize = 1_000_000;

/// One token (SPEC §4.2).
#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Int(i32),
    Real(f64),
    Str(String),
    /// `0x0A`: off / reversed / bounded.
    False,
    /// `0x0B`: on / forward / unbounded.
    True,
    Ptr(i32),
    /// `0x0D`: last part of a record type name.
    Id(String),
    /// `0x0E`: prefix part of a record type name.
    Prefix(String),
    Open,
    Close,
    Pos([f64; 3]),
    Vec([f64; 3]),
    Enum(i32),
    /// `0x17`: a 64-bit integer (seen in attributes of imported parts).
    Long(i64),
}

/// A record: its type name (`plane-surface`, `End-of-ASM-data`) and the tokens after it,
/// up to but without the `0x11` that ends it.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub name: String,
    pub fields: Vec<Tok>,
}

impl Record {
    /// The last part of the type name (`surface` for `plane-surface`, `vertex` for
    /// `tvertex-vertex`).
    pub fn base(&self) -> &str {
        self.name.rsplit('-').next().unwrap_or("")
    }

    /// Attribute records (type names ending in `attrib`, SPEC §4.5).
    pub fn is_attribute(&self) -> bool {
        self.name.ends_with("attrib")
    }
}

/// The parsed B-rep: the kernel header and the record list (record 0 is the header record).
#[derive(Debug, Clone, PartialEq)]
pub struct Brep {
    /// `versie×100` of the binary header, e.g. 22800.
    pub version: i32,
    /// The header strings: product, kernel version (`ASM 228.3.1.65535 NT`), date.
    pub header_strings: Vec<String>,
    /// Millimetres per model unit (the first header real; 10 = centimetres).
    pub mm_per_unit: f64,
    pub records: Vec<Record>,
    /// How the record list ended.
    pub end: End,
    /// Where each `{ }` definition of the stream starts: the record and the token index of
    /// its `{`. A `{ ref n }` stands for definition `n` of this list.
    pub definitions: Vec<(u32, u32)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    /// An `End-of-ASM-data` (or `End-of-ACIS-data`) record.
    EndRecord,
    /// The history records (`Begin-of-ASM-History-Data`) start.
    History,
    /// A byte that is not a token tag.
    UnknownTag,
    /// The data ended inside a record.
    Truncated,
}

/// Finds the token stream in the unpacked B-rep segment and parses it.
pub fn parse(segment: &[u8]) -> Result<Brep, String> {
    let (at, sig) =
        SIGNATURES.iter().filter_map(|s| find(segment, s).map(|p| (p, *s))).min_by_key(|(p, _)| *p).ok_or("no ASM/ACIS B-rep in the B-rep segment")?;
    let mut r = Rd::at(segment, at + sig.len());
    let version = r.i32().ok_or("B-rep header truncated")?;
    r.skip(12).ok_or("B-rep header truncated")?;
    let (records, end) = records(&mut r);
    if records.is_empty() {
        return Err("empty B-rep token stream".into());
    }
    let header_strings = records[0].fields.iter().filter_map(|t| if let Tok::Str(s) = t { Some(s.clone()) } else { None }).take(3).collect();
    let mm_per_unit =
        records[0].fields.iter().find_map(|t| if let Tok::Real(v) = t { Some(*v) } else { None }).filter(|v| v.is_finite() && *v > 0.0).unwrap_or(10.0);
    let mut definitions = Vec::new();
    for (r, record) in records.iter().enumerate() {
        for (t, pair) in record.fields.windows(2).enumerate() {
            if pair[0] == Tok::Open && !matches!(&pair[1], Tok::Id(s) if s == "ref") {
                definitions.push((r as u32, t as u32));
            }
        }
    }
    Ok(Brep { version, header_strings, mm_per_unit, records, end, definitions })
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// The next token; `Ok(None)` at the end of a record (`0x11`), `Err(End)` when the stream ends.
fn token(r: &mut Rd) -> Result<Option<Tok>, End> {
    let Some(tag) = r.u8() else { return Err(End::Truncated) };
    let str8 = |r: &mut Rd| -> Option<String> {
        let n = r.u8()? as usize;
        r.bytes(n).map(latin1)
    };
    let v3 = |r: &mut Rd| -> Option<[f64; 3]> { Some([r.f64()?, r.f64()?, r.f64()?]) };
    let t = match tag {
        0x04 => r.i32().map(Tok::Int),
        0x06 => r.f64().map(Tok::Real),
        0x07 | 0x12 => str8(r).map(Tok::Str),
        0x08 => r.u16().and_then(|n| r.bytes(n as usize)).map(|b| Tok::Str(latin1(b))),
        0x0A => Some(Tok::False),
        0x0B => Some(Tok::True),
        0x0C => r.i32().map(Tok::Ptr),
        0x0D => str8(r).map(Tok::Id),
        0x0E => str8(r).map(Tok::Prefix),
        0x0F => Some(Tok::Open),
        0x10 => Some(Tok::Close),
        0x11 => return Ok(None),
        0x13 => v3(r).map(Tok::Pos),
        0x14 => v3(r).map(Tok::Vec),
        0x15 => r.i32().map(Tok::Enum),
        0x17 => r.u64().map(|v| Tok::Long(v as i64)),
        _ => return Err(End::UnknownTag),
    };
    t.map(Some).ok_or(End::Truncated)
}

fn records(r: &mut Rd) -> (Vec<Record>, End) {
    let mut out = Vec::new();
    let mut total = 0usize;
    loop {
        if out.len() >= MAX_RECORDS {
            return (out, End::Truncated);
        }
        let mut toks = Vec::new();
        loop {
            match token(r) {
                Ok(Some(t)) => {
                    total += 1;
                    if total > MAX_TOKENS || toks.len() >= MAX_RECORD_TOKENS {
                        return (out, End::Truncated);
                    }
                    toks.push(t);
                }
                Ok(None) => break,
                Err(end) => return (out, end),
            }
        }
        // the type name: the leading 0x0E parts and the 0x0D part
        let mut parts = Vec::new();
        let mut n: usize = 0;
        for t in &toks {
            match t {
                Tok::Prefix(s) => parts.push(s.clone()),
                Tok::Id(s) => {
                    parts.push(s.clone());
                    n += 1;
                    break;
                }
                _ => break,
            }
            n += 1;
        }
        let named = matches!(toks.get(n.saturating_sub(1)), Some(Tok::Id(_)));
        let (name, fields) = if named && !out.is_empty() {
            (parts.join("-"), toks.split_off(n))
        } else if out.is_empty() {
            ("header".to_string(), toks)
        } else {
            (String::new(), toks)
        };
        let lower = name.to_ascii_lowercase();
        if lower.starts_with("end-of-") && lower.ends_with("-data") {
            return (out, End::EndRecord);
        }
        if lower.starts_with("begin-of-") && lower.contains("history") {
            return (out, End::History);
        }
        out.push(Record { name, fields });
    }
}

/// A typed cursor over the fields of a record after the common start (SPEC §4.5: attribute
/// pointer, id, pointer).
pub struct Fields<'a> {
    toks: &'a [Tok],
    i: usize,
}

impl<'a> Fields<'a> {
    pub fn new(rec: &'a Record) -> Self {
        Self { toks: &rec.fields, i: 3.min(rec.fields.len()) }
    }

    fn next(&mut self) -> Option<&'a Tok> {
        let t = self.toks.get(self.i)?;
        self.i += 1;
        Some(t)
    }

    pub fn ptr(&mut self) -> Option<i32> {
        match self.next()? {
            Tok::Ptr(p) => Some(*p),
            _ => None,
        }
    }

    pub fn int(&mut self) -> Option<i32> {
        match self.next()? {
            Tok::Int(v) | Tok::Enum(v) | Tok::Ptr(v) => Some(*v),
            _ => None,
        }
    }

    pub fn real(&mut self) -> Option<f64> {
        match self.next()? {
            Tok::Real(v) => Some(*v),
            Tok::Int(v) => Some(*v as f64),
            _ => None,
        }
    }

    /// A position or a vector token.
    pub fn v3(&mut self) -> Option<[f64; 3]> {
        match self.next()? {
            Tok::Pos(v) | Tok::Vec(v) => Some(*v),
            _ => None,
        }
    }

    /// A logical: `0x0B` true (forward), `0x0A` false (reversed).
    pub fn logical(&mut self) -> Option<bool> {
        match self.next()? {
            Tok::True => Some(true),
            Tok::False => Some(false),
            _ => None,
        }
    }

    /// One end of a range: `0x0B` unbounded, or `0x0A` and a real bound (SPEC §4.7).
    pub fn bound(&mut self) -> Option<Option<f64>> {
        match self.next()? {
            Tok::True => Some(None),
            Tok::False => match self.toks.get(self.i) {
                Some(Tok::Real(v)) => {
                    self.i += 1;
                    Some(Some(*v))
                }
                _ => Some(None),
            },
            _ => None,
        }
    }
}

/// Topology (SPEC §4.6), with pointers as record indices (−1 = none).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Body {
    pub lump: i32,
    pub wire: i32,
    pub transform: i32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lump {
    pub next: i32,
    pub shell: i32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shell {
    pub next: i32,
    pub face: i32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Face {
    pub next: i32,
    pub loop_: i32,
    pub surface: i32,
    /// The outward normal equals the surface normal.
    pub forward: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Loop {
    pub next: i32,
    pub coedge: i32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Coedge {
    pub next: i32,
    pub partner: i32,
    pub edge: i32,
    /// Same direction as the edge.
    pub forward: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Edge {
    pub start: i32,
    pub t0: f64,
    pub end: i32,
    pub t1: f64,
    pub curve: i32,
    pub forward: bool,
}

/// Geometry (SPEC §4.7), in model units.
#[derive(Debug, Clone, PartialEq)]
pub enum Surface {
    Plane {
        p: [f64; 3],
        n: [f64; 3],
        u: [f64; 3],
    },
    /// Cone or cylinder (sin α = 0): axis through `c` along `a`; `m` to the rim in the base
    /// plane (|m| = radius), `ratio` minor/major axis.
    Cone {
        c: [f64; 3],
        a: [f64; 3],
        m: [f64; 3],
        ratio: f64,
        sin: f64,
        cos: f64,
    },
    /// Centre `c`, radius `r` (negative: the surface normal points inwards), `x` the direction
    /// of longitude 0 on the equator, `pole` the north pole.
    Sphere {
        c: [f64; 3],
        r: f64,
        x: [f64; 3],
        pole: [f64; 3],
    },
    /// Centre `c`, axis `a`, the radius of the ring `major` and of the tube `minor` (negative:
    /// the surface normal points into the tube), `x` the direction of angle 0 around the axis.
    Torus {
        c: [f64; 3],
        a: [f64; 3],
        major: f64,
        minor: f64,
        x: [f64; 3],
    },
    /// A B-spline surface: the one the record stores for its surface, exact or an approximation
    /// of a procedural one. `forward` false: the normal is turned.
    Spline {
        surface: Box<crate::nurbs::Surface>,
        forward: bool,
    },
    /// A surface type that is not read; for a spline surface the name of its kind is added
    /// (`spline-surface helix_spl_circ`).
    Other(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Curve {
    Line {
        p: [f64; 3],
        d: [f64; 3],
    },
    /// Ellipse or circle around `c` in the plane normal to `n`; `m` the major axis.
    Ellipse {
        c: [f64; 3],
        n: [f64; 3],
        m: [f64; 3],
        ratio: f64,
    },
    /// A B-spline curve: the one the record stores for an intersection curve.
    Spline(Box<crate::nurbs::Curve>),
    /// A curve type that is not read, with the name of its kind (`intcurve-curve helix_int_cur`).
    Other(String),
}

impl Brep {
    pub fn record(&self, i: i32) -> Option<&Record> {
        usize::try_from(i).ok().and_then(|i| self.records.get(i))
    }

    fn typed(&self, i: i32, base: &str) -> Option<Fields<'_>> {
        let r = self.record(i)?;
        (r.base() == base).then(|| Fields::new(r))
    }

    /// Indices of every `body` record.
    pub fn bodies(&self) -> Vec<i32> {
        (0..self.records.len()).filter(|&i| self.records[i].name == "body").map(|i| i as i32).collect()
    }

    /// The number of a body: the integer after the attribute pointer of its record.
    pub fn body_number(&self, i: i32) -> Option<i32> {
        let r = self.record(i).filter(|r| r.name == "body")?;
        match r.fields.get(1)? {
            Tok::Int(n) => Some(*n),
            _ => None,
        }
    }

    pub fn body(&self, i: i32) -> Option<Body> {
        let mut f = self.typed(i, "body")?;
        Some(Body { lump: f.ptr()?, wire: f.ptr().unwrap_or(-1), transform: f.ptr().unwrap_or(-1) })
    }

    pub fn lump(&self, i: i32) -> Option<Lump> {
        let mut f = self.typed(i, "lump")?;
        Some(Lump { next: f.ptr()?, shell: f.ptr()? })
    }

    pub fn shell(&self, i: i32) -> Option<Shell> {
        let mut f = self.typed(i, "shell")?;
        let next = f.ptr()?;
        let _subshell = f.ptr()?;
        Some(Shell { next, face: f.ptr()? })
    }

    pub fn face(&self, i: i32) -> Option<Face> {
        let mut f = self.typed(i, "face")?;
        let next = f.ptr()?;
        let loop_ = f.ptr()?;
        let _shell = f.ptr()?;
        let _subshell = f.ptr()?;
        let surface = f.ptr()?;
        Some(Face { next, loop_, surface, forward: f.logical().unwrap_or(true) })
    }

    pub fn loop_(&self, i: i32) -> Option<Loop> {
        let mut f = self.typed(i, "loop")?;
        Some(Loop { next: f.ptr()?, coedge: f.ptr()? })
    }

    pub fn coedge(&self, i: i32) -> Option<Coedge> {
        let mut f = self.typed(i, "coedge")?;
        let next = f.ptr()?;
        let _prev = f.ptr()?;
        let partner = f.ptr()?;
        let edge = f.ptr()?;
        Some(Coedge { next, partner, edge, forward: f.logical().unwrap_or(true) })
    }

    pub fn edge(&self, i: i32) -> Option<Edge> {
        let mut f = self.typed(i, "edge")?;
        let start = f.ptr()?;
        let t0 = f.real()?;
        let end = f.ptr()?;
        let t1 = f.real()?;
        let _coedge = f.ptr()?;
        let curve = f.ptr()?;
        Some(Edge { start, t0, end, t1, curve, forward: f.logical().unwrap_or(true) })
    }

    /// The point of a vertex, in model units.
    pub fn vertex_point(&self, i: i32) -> Option<[f64; 3]> {
        let mut f = self.typed(i, "vertex")?;
        let _edge = f.ptr()?;
        let _n = f.int()?;
        let point = f.ptr()?;
        let r = self.record(point)?;
        if r.name != "point" {
            return None;
        }
        // the first position of the point record
        let p = r.fields.iter().skip(3).find_map(|t| if let Tok::Pos(p) | Tok::Vec(p) = t { Some(*p) } else { None })?;
        p.iter().all(|v| v.is_finite()).then_some(p)
    }

    pub fn surface(&self, i: i32) -> Option<Surface> {
        let r = self.record(i)?;
        if r.base() != "surface" {
            return None;
        }
        let mut f = Fields::new(r);
        Some(match r.name.as_str() {
            "plane-surface" => Surface::Plane { p: f.v3()?, n: f.v3()?, u: f.v3()? },
            "cone-surface" => {
                let (c, a, m, ratio) = (f.v3()?, f.v3()?, f.v3()?, f.real()?);
                f.bound()?;
                f.bound()?;
                Surface::Cone { c, a, m, ratio, sin: f.real()?, cos: f.real()? }
            }
            "sphere-surface" => Surface::Sphere { c: f.v3()?, r: f.real()?, x: f.v3()?, pole: f.v3()? },
            "torus-surface" => Surface::Torus { c: f.v3()?, a: f.v3()?, major: f.real()?, minor: f.real()?, x: f.v3()? },
            "spline-surface" => {
                let forward = f.logical().unwrap_or(true);
                let (kind, tokens) = self.definition(i).unwrap_or(("", &[]));
                match crate::nurbs::find_surface(tokens) {
                    Some(surface) => Surface::Spline { surface: Box::new(surface), forward },
                    None => Surface::Other(format!("spline-surface {kind}").trim_end().to_string()),
                }
            }
            other => Surface::Other(other.to_string()),
        })
    }

    /// The `{ }` definition of a record: the name of its kind and the tokens after it up to
    /// its `}`. A `{ ref n }` gives definition `n` of the stream.
    pub fn definition(&self, i: i32) -> Option<(&str, &[Tok])> {
        let fields = &self.record(i)?.fields;
        let open = fields.iter().position(|t| *t == Tok::Open)?;
        let (fields, open) = match (fields.get(open + 1)?, fields.get(open + 2)) {
            (Tok::Id(s), Some(Tok::Int(n))) if s == "ref" => {
                let &(r, t) = self.definitions.get(usize::try_from(*n).ok()?)?;
                (&self.records.get(r as usize)?.fields, t as usize)
            }
            _ => (fields, open),
        };
        let Tok::Id(kind) = fields.get(open + 1)? else { return None };
        // up to the `}` that closes this `{`
        let mut depth = 0usize;
        let mut end = fields.len();
        for (k, t) in fields.iter().enumerate().skip(open) {
            match t {
                Tok::Open => depth += 1,
                Tok::Close => {
                    depth -= 1;
                    if depth == 0 {
                        end = k;
                        break;
                    }
                }
                _ => {}
            }
        }
        Some((kind, &fields[open + 2..end]))
    }

    /// The faces with a colour of their own: an attribute whose type name starts with
    /// `NMx_FFColor_Entity` has its face as its owner and the colour as the text `r,g,b`.
    pub fn face_colours(&self) -> std::collections::HashMap<i32, [f32; 3]> {
        let mut out = std::collections::HashMap::new();
        for r in self.records.iter().filter(|r| r.name.starts_with("NMx_FFColor_Entity")) {
            let Some(Tok::Ptr(owner)) = r.fields.get(4) else { continue };
            let colour = r.fields.iter().find_map(|t| if let Tok::Str(s) = t { crate::appearance::from_triple(s) } else { None });
            if let Some(c) = colour {
                out.insert(*owner, c);
            }
        }
        out
    }

    /// A body transform: the images of the x, y and z axes, the translation and the scale.
    pub fn transform(&self, i: i32) -> Option<[[f64; 3]; 4]> {
        let r = self.record(i)?;
        if r.name != "transform" {
            return None;
        }
        let v: Vec<[f64; 3]> = r.fields.iter().filter_map(|t| if let Tok::Vec(v) | Tok::Pos(v) = t { Some(*v) } else { None }).take(4).collect();
        let scale = r.fields.iter().find_map(|t| if let Tok::Real(s) = t { Some(*s) } else { None }).unwrap_or(1.0);
        if v.len() != 4 || !v.iter().flatten().all(|x| x.is_finite()) || !scale.is_finite() || scale == 0.0 {
            return None;
        }
        Some([v[0].map(|x| x * scale), v[1].map(|x| x * scale), v[2].map(|x| x * scale), v[3]])
    }

    pub fn curve(&self, i: i32) -> Option<Curve> {
        let r = self.record(i)?;
        if r.base() != "curve" {
            return None;
        }
        let mut f = Fields::new(r);
        Some(match r.name.as_str() {
            "straight-curve" => Curve::Line { p: f.v3()?, d: f.v3()? },
            "ellipse-curve" => Curve::Ellipse { c: f.v3()?, n: f.v3()?, m: f.v3()?, ratio: f.real()? },
            "intcurve-curve" => {
                let (kind, tokens) = self.definition(i).unwrap_or(("", &[]));
                match crate::nurbs::find_curve(tokens) {
                    Some(curve) => Curve::Spline(Box::new(curve)),
                    None => Curve::Other(format!("intcurve-curve {kind}").trim_end().to_string()),
                }
            }
            other => Curve::Other(other.to_string()),
        })
    }

    /// The kernel version number (`versie×100` / 100), e.g. 228.
    pub fn kernel_major(&self) -> i32 {
        self.version / 100
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_from_prefixes() {
        let mut d = b"xxACIS BinaryFile".to_vec();
        for v in [21800i32, 0, 2, 0] {
            d.extend(v.to_le_bytes());
        }
        // header record
        d.extend([0x07, 1, b'p', 0x06]);
        d.extend(10f64.to_le_bytes());
        d.push(0x11);
        // 0E "plane" 0D "surface" 0C -1 0x11
        d.extend([0x0E, 5]);
        d.extend(b"plane");
        d.extend([0x0D, 7]);
        d.extend(b"surface");
        d.push(0x0C);
        d.extend((-1i32).to_le_bytes());
        d.push(0x11);
        // 0E End 0E of 0E ASM 0D data
        for (t, s) in [(0x0E, "End"), (0x0E, "of"), (0x0E, "ASM"), (0x0D, "data")] {
            d.extend([t, s.len() as u8]);
            d.extend(s.as_bytes());
        }
        d.push(0x11);
        let b = parse(&d).unwrap();
        assert_eq!(b.version, 21800);
        assert_eq!(b.end, End::EndRecord);
        assert_eq!(b.records.len(), 2);
        assert_eq!(b.records[1].name, "plane-surface");
        assert_eq!(b.records[1].fields, vec![Tok::Ptr(-1)]);
        assert_eq!(b.mm_per_unit, 10.0);
    }

    /// A token stream from tokens: the header record, then one record per (type name, fields).
    fn stream(records: &[(&str, Vec<Tok>)]) -> Vec<u8> {
        let mut d = b"ASM BinaryFile4".to_vec();
        for v in [22600i32, 0, 2, 0] {
            d.extend(v.to_le_bytes());
        }
        let name = |d: &mut Vec<u8>, tag: u8, s: &str| {
            d.extend([tag, s.len() as u8]);
            d.extend(s.as_bytes());
        };
        d.extend([0x07, 1, b'p', 0x06]);
        d.extend(10f64.to_le_bytes());
        d.push(0x11);
        for (kind, fields) in records {
            let parts: Vec<&str> = kind.split('-').collect();
            for (k, part) in parts.iter().enumerate() {
                name(&mut d, if k + 1 == parts.len() { 0x0D } else { 0x0E }, part);
            }
            for t in fields {
                match t {
                    Tok::Int(v) => {
                        d.push(0x04);
                        d.extend(v.to_le_bytes());
                    }
                    Tok::Real(v) => {
                        d.push(0x06);
                        d.extend(v.to_le_bytes());
                    }
                    Tok::Str(s) => name(&mut d, 0x07, s),
                    Tok::False => d.push(0x0A),
                    Tok::True => d.push(0x0B),
                    Tok::Ptr(v) => {
                        d.push(0x0C);
                        d.extend(v.to_le_bytes());
                    }
                    Tok::Id(s) => name(&mut d, 0x0D, s),
                    Tok::Prefix(s) => name(&mut d, 0x0E, s),
                    Tok::Open => d.push(0x0F),
                    Tok::Close => d.push(0x10),
                    Tok::Pos(v) | Tok::Vec(v) => {
                        d.push(if matches!(t, Tok::Pos(_)) { 0x13 } else { 0x14 });
                        d.extend(v.iter().flat_map(|x| x.to_le_bytes()));
                    }
                    Tok::Enum(v) => {
                        d.push(0x15);
                        d.extend(v.to_le_bytes());
                    }
                    Tok::Long(v) => {
                        d.push(0x17);
                        d.extend(v.to_le_bytes());
                    }
                }
            }
            d.push(0x11);
        }
        d
    }

    fn id(s: &str) -> Tok {
        Tok::Id(s.into())
    }

    #[test]
    fn definitions_references_colours_and_transforms() {
        let common = || vec![Tok::Ptr(-1), Tok::Int(-1), Tok::Ptr(-1)];
        // a straight B-spline from the origin to (2, 0, 0): degree 1, two knots
        let line = || {
            let mut t = vec![id("nubs"), Tok::Int(1), Tok::Enum(0), Tok::Int(2), Tok::Real(0.0), Tok::Int(1), Tok::Real(1.0), Tok::Int(1)];
            t.extend([0.0, 0.0, 0.0, 2.0, 0.0, 0.0].map(Tok::Real));
            t
        };
        let with = |mut fields: Vec<Tok>, more: Vec<Tok>| {
            fields.extend(more);
            fields
        };
        // 1: a pcurve whose definition holds another one, 2: the curve, 3: a reference to it,
        // 4: a curve without a B-spline, 5: a face colour, 6: a transform, 7: a 64-bit integer
        let mut inner = vec![Tok::True, Tok::Open, id("exp_par_cur"), Tok::Open, id("exact_spl_sur"), Tok::Int(22600), Tok::Close, Tok::Close];
        inner.insert(0, Tok::Int(0));
        let mut curve = vec![Tok::True, Tok::Open, id("exact_int_cur"), Tok::Int(22600), Tok::Enum(0)];
        curve.extend(line());
        curve.extend([Tok::Real(0.0), Tok::Close]);
        let records = vec![
            ("pcurve", with(common(), inner)),
            ("intcurve-curve", with(common(), curve)),
            ("intcurve-curve", with(common(), vec![Tok::True, Tok::Open, id("ref"), Tok::Int(2), Tok::Close])),
            ("intcurve-curve", with(common(), vec![Tok::True, Tok::Open, id("helix_int_cur"), Tok::Int(22600), id("nullbs"), Tok::Close])),
            (
                "NMx_FFColor_Entity-NamingMatching-attrib",
                vec![Tok::Ptr(-1), Tok::Int(-1), Tok::Ptr(-1), Tok::Ptr(-1), Tok::Ptr(42), Tok::Int(43), Tok::Str("255,0,51".into()), Tok::Int(0)],
            ),
            (
                "transform",
                vec![
                    Tok::Ptr(-1),
                    Tok::Int(-1),
                    Tok::Vec([0.0, 1.0, 0.0]),
                    Tok::Vec([-1.0, 0.0, 0.0]),
                    Tok::Vec([0.0, 0.0, 1.0]),
                    Tok::Vec([5.0, 6.0, 7.0]),
                    Tok::Real(2.0),
                    Tok::True,
                ],
            ),
            ("some-attrib", vec![Tok::Long(1_508_913_994), Tok::Int(3)]),
        ];
        let b = parse(&stream(&records)).unwrap();
        assert_eq!(b.records.len(), 8);
        // the definitions in the order of their `{`: the pcurve, the surface inside it, the curve
        assert_eq!(b.definitions, vec![(1, 5), (1, 7), (2, 4), (4, 4)]);
        let Some(Curve::Spline(c)) = b.curve(2) else { panic!("{:?}", b.curve(2)) };
        assert_eq!((c.degree, c.eval(0.5)), (1, [1.0, 0.0, 0.0]));
        assert_eq!(b.curve(3), b.curve(2));
        assert_eq!(b.curve(4), Some(Curve::Other("intcurve-curve helix_int_cur".into())));
        assert_eq!(b.definition(3).map(|d| d.0), Some("exact_int_cur"));
        assert_eq!(b.face_colours().get(&42), Some(&[1.0, 0.0, 0.2]));
        assert_eq!(b.transform(6), Some([[0.0, 2.0, 0.0], [-2.0, 0.0, 0.0], [0.0, 0.0, 2.0], [5.0, 6.0, 7.0]]));
        assert_eq!(b.records[7].fields, vec![Tok::Long(1_508_913_994), Tok::Int(3)]);
    }
}
