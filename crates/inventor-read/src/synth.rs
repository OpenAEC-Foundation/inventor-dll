//! Writers of synthetic Inventor files, built from the specification for tests (SPEC §9): a
//! compound file with property sets ([MS-OLEPS]), `RSeStorage` segments with a B-rep token
//! stream, and a display cache. No bytes of third-party files are used.

use crate::bytes::guid_bytes;
use crate::segments::B_MAGIC;
use std::collections::HashMap;
use std::io::{Cursor, Write};

type V3 = [f64; 3];

// ------------------------------------------------------------------------------------------
// Property sets

/// A value to write into a property set.
#[derive(Debug, Clone, PartialEq)]
pub enum Prop {
    Str(String),
    I2(i16),
    I4(i32),
    UI4(u32),
    R8(f64),
    Bool(bool),
    FileTime(u64),
    Clsid(String),
    /// VT_CF: format and data (after the size field).
    Cf(Vec<u8>),
}

pub fn s(v: &str) -> Prop {
    Prop::Str(v.to_string())
}

fn pad4(b: &mut Vec<u8>) {
    while b.len() % 4 != 0 {
        b.push(0);
    }
}

fn typed(p: &Prop) -> Vec<u8> {
    let mut b = Vec::new();
    let vt: u16 = match p {
        Prop::Str(_) => 0x1F,
        Prop::I2(_) => 0x02,
        Prop::I4(_) => 0x03,
        Prop::UI4(_) => 0x13,
        Prop::R8(_) => 0x05,
        Prop::Bool(_) => 0x0B,
        Prop::FileTime(_) => 0x40,
        Prop::Clsid(_) => 0x48,
        Prop::Cf(_) => 0x47,
    };
    b.extend(vt.to_le_bytes());
    b.extend([0, 0]);
    match p {
        Prop::Str(s) => {
            let units: Vec<u16> = s.encode_utf16().chain([0]).collect();
            b.extend((units.len() as u32).to_le_bytes());
            b.extend(units.iter().flat_map(|u| u.to_le_bytes()));
        }
        Prop::I2(v) => b.extend(v.to_le_bytes()),
        Prop::I4(v) => b.extend(v.to_le_bytes()),
        Prop::UI4(v) => b.extend(v.to_le_bytes()),
        Prop::R8(v) => b.extend(v.to_le_bytes()),
        Prop::Bool(v) => b.extend(if *v { [0xFF, 0xFF] } else { [0, 0] }),
        Prop::FileTime(v) => b.extend(v.to_le_bytes()),
        Prop::Clsid(g) => b.extend(guid_bytes(g).expect("GUID")),
        Prop::Cf(d) => {
            b.extend((d.len() as u32).to_le_bytes());
            b.extend(d);
        }
    }
    pad4(&mut b);
    b
}

/// A property set stream ([MS-OLEPS]) with one set: code page 1200 is added; `names` go into
/// the dictionary (property 0).
pub fn property_set_stream(fmtid: &str, props: &[(u32, Prop)], names: &[(u32, &str)]) -> Vec<u8> {
    let mut entries: Vec<(u32, Vec<u8>)> = vec![(1, typed(&Prop::I2(1200)))];
    if !names.is_empty() {
        let mut d = (names.len() as u32).to_le_bytes().to_vec();
        for (id, name) in names {
            let units: Vec<u16> = name.encode_utf16().chain([0]).collect();
            d.extend(id.to_le_bytes());
            d.extend((units.len() as u32).to_le_bytes());
            d.extend(units.iter().flat_map(|u| u.to_le_bytes()));
            pad4(&mut d);
        }
        entries.push((0, d));
    }
    entries.extend(props.iter().map(|(id, p)| (*id, typed(p))));
    let header_len = 8 + 8 * entries.len();
    let mut set = Vec::new();
    let mut offsets = Vec::new();
    let mut body: Vec<u8> = Vec::new();
    for (id, data) in &entries {
        offsets.push((*id, (header_len + body.len()) as u32));
        body.extend(data);
    }
    set.extend(((header_len + body.len()) as u32).to_le_bytes());
    set.extend((entries.len() as u32).to_le_bytes());
    for (id, off) in offsets {
        set.extend(id.to_le_bytes());
        set.extend(off.to_le_bytes());
    }
    set.extend(body);

    let mut out = Vec::new();
    out.extend(0xFFFEu16.to_le_bytes());
    out.extend(0u16.to_le_bytes());
    out.extend(0x0002_0006u32.to_le_bytes());
    out.extend([0u8; 16]);
    out.extend(1u32.to_le_bytes());
    out.extend(guid_bytes(fmtid).expect("FMTID"));
    out.extend(48u32.to_le_bytes());
    out.extend(set);
    out
}

/// The stream name of a property set: U+0005 and the 26 characters of [MS-OLEPS] (5 bits per
/// character of the stored FMTID, from the lowest bit up).
pub fn stream_name(fmtid: &str) -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz012345";
    let b = guid_bytes(fmtid).expect("FMTID");
    let bits = u128::from_le_bytes(b);
    let mut name = String::from('\u{5}');
    for i in 0..26 {
        name.push(ALPHABET[((bits >> (5 * i)) & 0x1F) as usize] as char);
    }
    name
}

/// The VT_CF thumbnail payload of SPEC §2.3.
pub fn thumbnail_cf(width: u16, height: u16, png: &[u8]) -> Vec<u8> {
    let mut b = (-1i32).to_le_bytes().to_vec();
    b.extend(3u32.to_le_bytes());
    b.extend(8u16.to_le_bytes());
    b.extend(width.to_le_bytes());
    b.extend(height.to_le_bytes());
    b.extend(0u16.to_le_bytes());
    b.extend(png);
    b
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &byte in data {
        c ^= byte as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}

/// A grey PNG of `w` × `h` pixels.
pub fn png(w: u32, h: u32) -> Vec<u8> {
    let mut raw = Vec::new();
    for _ in 0..h {
        raw.push(0);
        raw.extend(std::iter::repeat_n(0x80u8, w as usize));
    }
    let chunk = |out: &mut Vec<u8>, kind: &[u8], data: &[u8]| {
        out.extend((data.len() as u32).to_be_bytes());
        let mut c = kind.to_vec();
        c.extend(data);
        out.extend(&c);
        out.extend(crc32(&c).to_be_bytes());
    };
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = w.to_be_bytes().to_vec();
    ihdr.extend(h.to_be_bytes());
    ihdr.extend([8, 0, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &zlib(&raw));
    chunk(&mut out, b"IEND", &[]);
    out
}

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

// ------------------------------------------------------------------------------------------
// Segments

/// An `M` stream with a class name (SPEC §3.2).
pub fn meta_stream(class: &str) -> Vec<u8> {
    let tag = b"RSe Meta Stream Version 8";
    let mut b = (tag.len() as u32).to_le_bytes().to_vec();
    b.extend(tag);
    b.extend([0u8; 18]);
    let units: Vec<u16> = class.encode_utf16().collect();
    b.extend((units.len() as u32).to_le_bytes());
    b.extend(units.iter().flat_map(|u| u.to_le_bytes()));
    b.extend([0x11u8; 16]);
    for date in ["11/06/2026 10:00:00", "11/06/2026 10:00:01"] {
        b.extend((date.len() as u32).to_le_bytes());
        b.extend(date.as_bytes());
    }
    b.extend(zlib(b"meta"));
    b
}

/// A `B` stream (SPEC §3.4): codec 1 = zlib, 2 = Zstandard, other values are written as zlib
/// data with that codec byte.
pub fn data_stream(codec: u8, content: &[u8]) -> Vec<u8> {
    let mut b = B_MAGIC.to_vec();
    b.push(4);
    b.push(codec);
    if codec == 2 {
        b.extend(ruzstd::encoding::compress_to_vec(content, ruzstd::encoding::CompressionLevel::Fastest));
    } else {
        b.extend(zlib(content));
    }
    b
}

// ------------------------------------------------------------------------------------------
// B-rep token stream

/// Writes tokens of SPEC §4.2.
#[derive(Default)]
pub struct Tokens(pub Vec<u8>);

impl Tokens {
    pub fn int(&mut self, v: i32) -> &mut Self {
        self.0.push(0x04);
        self.0.extend(v.to_le_bytes());
        self
    }
    pub fn real(&mut self, v: f64) -> &mut Self {
        self.0.push(0x06);
        self.0.extend(v.to_le_bytes());
        self
    }
    pub fn str(&mut self, v: &str) -> &mut Self {
        self.0.extend([0x07, v.len() as u8]);
        self.0.extend(v.as_bytes());
        self
    }
    pub fn logical(&mut self, v: bool) -> &mut Self {
        self.0.push(if v { 0x0B } else { 0x0A });
        self
    }
    pub fn ptr(&mut self, v: i32) -> &mut Self {
        self.0.push(0x0C);
        self.0.extend(v.to_le_bytes());
        self
    }
    /// The type name: `plane-surface` → `0E "plane" 0D "surface"`.
    pub fn name(&mut self, name: &str) -> &mut Self {
        let parts: Vec<&str> = name.split('-').collect();
        for (i, p) in parts.iter().enumerate() {
            self.0.extend([if i + 1 == parts.len() { 0x0D } else { 0x0E }, p.len() as u8]);
            self.0.extend(p.as_bytes());
        }
        self
    }
    /// The common start of an entity record (SPEC §4.5): attribute, id, pointer.
    pub fn common(&mut self, attrib: i32) -> &mut Self {
        self.ptr(attrib).int(-1).ptr(-1)
    }
    pub fn pos(&mut self, v: V3) -> &mut Self {
        self.0.push(0x13);
        v.iter().for_each(|c| self.0.extend(c.to_le_bytes()));
        self
    }
    pub fn vec(&mut self, v: V3) -> &mut Self {
        self.0.push(0x14);
        v.iter().for_each(|c| self.0.extend(c.to_le_bytes()));
        self
    }
    pub fn end(&mut self) -> &mut Self {
        self.0.push(0x11);
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum CurveSpec {
    Line,
    /// A circle around `center` in the plane normal to `normal`; `major` from the centre to
    /// θ = 0 (its length is the radius).
    Circle {
        center: V3,
        normal: V3,
        major: V3,
    },
    /// A curve type without specified fields, e.g. `intcurve-curve`.
    Other(&'static str),
}

#[derive(Debug, Clone, PartialEq)]
pub enum SurfaceSpec {
    Plane { p: V3, n: V3, u: V3 },
    Cylinder { c: V3, axis: V3, major: V3 },
    Cone { c: V3, axis: V3, major: V3, half_angle: f64 },
    Other(&'static str),
}

#[derive(Debug, Clone)]
struct EdgeSpec {
    start: usize,
    end: usize,
    t0: f64,
    t1: f64,
    curve: CurveSpec,
}

#[derive(Debug, Clone)]
struct FaceSpec {
    surface: SurfaceSpec,
    forward: bool,
    loops: Vec<Vec<(usize, bool)>>,
}

/// How the synthetic token stream ends (SPEC §4.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    EndRecord,
    /// History records and then a byte that is not a tag.
    History,
}

/// A solid (one body, lump and shell) to write as a B-rep token stream. Points in cm.
#[derive(Debug, Clone, Default)]
pub struct Solid {
    points: Vec<V3>,
    edges: Vec<EdgeSpec>,
    faces: Vec<FaceSpec>,
}

fn d3(a: V3, b: V3) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

impl Solid {
    pub fn vertex(&mut self, p: V3) -> usize {
        self.points.push(p);
        self.points.len() - 1
    }

    /// A straight edge from vertex `a` to `b` (parameters 0 … length).
    pub fn line(&mut self, a: usize, b: usize) -> usize {
        let l = d3(self.points[a], self.points[b]);
        self.edges.push(EdgeSpec { start: a, end: b, t0: 0.0, t1: l, curve: CurveSpec::Line });
        self.edges.len() - 1
    }

    /// A circular edge from `a` to `b`, parameters `t0` … `t1` (radians).
    #[allow(clippy::too_many_arguments)]
    pub fn arc(&mut self, a: usize, b: usize, center: V3, normal: V3, major: V3, t0: f64, t1: f64) -> usize {
        self.edges.push(EdgeSpec { start: a, end: b, t0, t1, curve: CurveSpec::Circle { center, normal, major } });
        self.edges.len() - 1
    }

    /// An edge on a curve type the reader does not know.
    pub fn other_edge(&mut self, a: usize, b: usize, name: &'static str) -> usize {
        self.edges.push(EdgeSpec { start: a, end: b, t0: 0.0, t1: 1.0, curve: CurveSpec::Other(name) });
        self.edges.len() - 1
    }

    /// A face: its surface, sense, and loops of (edge, coedge runs along the edge).
    pub fn face(&mut self, surface: SurfaceSpec, forward: bool, loops: Vec<Vec<(usize, bool)>>) {
        self.faces.push(FaceSpec { surface, forward, loops });
    }

    /// Planar faces given as vertex lists, counter-clockwise seen from outside; each face may
    /// have holes (clockwise seen from outside).
    pub fn polyhedron(points: &[V3], faces: &[Vec<Vec<usize>>]) -> Self {
        let mut s = Solid::default();
        for &p in points {
            s.vertex(p);
        }
        let mut by_pair: HashMap<(usize, usize), usize> = HashMap::new();
        for face in faces {
            let ring = &face[0];
            let (a, b, c) = (points[ring[0]], points[ring[1]], points[ring[2]]);
            let n = unit(cross(sub(b, a), sub(c, a)));
            let u = unit(sub(b, a));
            let mut loops = Vec::new();
            for ring in face {
                let mut lp = Vec::new();
                for k in 0..ring.len() {
                    let (va, vb) = (ring[k], ring[(k + 1) % ring.len()]);
                    let key = (va.min(vb), va.max(vb));
                    let e = *by_pair.entry(key).or_insert_with(|| s.line(key.0, key.1));
                    lp.push((e, s.edges[e].start == va));
                }
                loops.push(lp);
            }
            s.face(SurfaceSpec::Plane { p: a, n, u }, true, loops);
        }
        s
    }

    /// An axis-aligned cube from the origin with side `size` (cm): 6 planes, 12 lines, 8 points.
    pub fn cube(size: f64) -> Self {
        let p = |x: f64, y: f64, z: f64| [x * size, y * size, z * size];
        let pts = [p(0., 0., 0.), p(1., 0., 0.), p(1., 1., 0.), p(0., 1., 0.), p(0., 0., 1.), p(1., 0., 1.), p(1., 1., 1.), p(0., 1., 1.)];
        let faces = [[0, 3, 2, 1], [4, 5, 6, 7], [0, 1, 5, 4], [1, 2, 6, 5], [2, 3, 7, 6], [3, 0, 4, 7]];
        Self::polyhedron(&pts, &faces.map(|f| vec![f.to_vec()]))
    }

    /// A cylinder (pin) along +z from the origin: two planar caps and one cylinder face bounded
    /// by two full circles. `cylinder_forward` is the sense of the cylinder face.
    pub fn pin(radius: f64, height: f64, cylinder_forward: bool) -> Self {
        let mut s = Solid::default();
        let pi = std::f64::consts::PI;
        let v0 = s.vertex([-radius, 0.0, 0.0]);
        let v1 = s.vertex([-radius, 0.0, height]);
        let major = [radius, 0.0, 0.0];
        let bottom = s.arc(v0, v0, [0.0; 3], [0.0, 0.0, 1.0], major, -pi, pi);
        let top = s.arc(v1, v1, [0.0, 0.0, height], [0.0, 0.0, 1.0], major, -pi, pi);
        s.face(SurfaceSpec::Plane { p: [0.0; 3], n: [0.0, 0.0, -1.0], u: [1.0, 0.0, 0.0] }, true, vec![vec![(bottom, false)]]);
        s.face(SurfaceSpec::Plane { p: [0.0, 0.0, height], n: [0.0, 0.0, 1.0], u: [1.0, 0.0, 0.0] }, true, vec![vec![(top, true)]]);
        s.face(SurfaceSpec::Cylinder { c: [0.0; 3], axis: [0.0, 0.0, 1.0], major }, cylinder_forward, vec![vec![(bottom, true)], vec![(top, false)]]);
        s
    }

    /// A truncated cone along +z: radius `r1` at z = 0, `r2` at z = `height` (cm).
    pub fn frustum(r1: f64, r2: f64, height: f64) -> Self {
        let mut s = Solid::default();
        let pi = std::f64::consts::PI;
        let v0 = s.vertex([-r1, 0.0, 0.0]);
        let v1 = s.vertex([-r2, 0.0, height]);
        let bottom = s.arc(v0, v0, [0.0; 3], [0.0, 0.0, 1.0], [r1, 0.0, 0.0], -pi, pi);
        let top = s.arc(v1, v1, [0.0, 0.0, height], [0.0, 0.0, 1.0], [r2, 0.0, 0.0], -pi, pi);
        s.face(SurfaceSpec::Plane { p: [0.0; 3], n: [0.0, 0.0, -1.0], u: [1.0, 0.0, 0.0] }, true, vec![vec![(bottom, false)]]);
        s.face(SurfaceSpec::Plane { p: [0.0, 0.0, height], n: [0.0, 0.0, 1.0], u: [1.0, 0.0, 0.0] }, true, vec![vec![(top, true)]]);
        let half_angle = ((r1 - r2).abs() / height).atan();
        s.face(
            SurfaceSpec::Cone { c: [0.0; 3], axis: [0.0, 0.0, 1.0], major: [r1, 0.0, 0.0], half_angle },
            true,
            vec![vec![(bottom, true)], vec![(top, false)]],
        );
        s
    }

    /// A square plate (`size` × `size` × `thickness`, cm) with a round hole of `radius` through
    /// its centre: 6 planes (top and bottom with a hole loop), 1 cylinder.
    pub fn plate_with_hole(size: f64, thickness: f64, radius: f64) -> Self {
        let pts = [
            [0., 0., 0.],
            [size, 0., 0.],
            [size, size, 0.],
            [0., size, 0.],
            [0., 0., thickness],
            [size, 0., thickness],
            [size, size, thickness],
            [0., size, thickness],
        ];
        // the outer faces first, the hole loops added below
        let faces = [[0, 3, 2, 1], [4, 5, 6, 7], [0, 1, 5, 4], [1, 2, 6, 5], [2, 3, 7, 6], [3, 0, 4, 7]];
        let mut s = Self::polyhedron(&pts, &faces.map(|f| vec![f.to_vec()]));
        let pi = std::f64::consts::PI;
        let c = size / 2.0;
        let major = [radius, 0.0, 0.0];
        let vb = s.vertex([c - radius, c, 0.0]);
        let vt = s.vertex([c - radius, c, thickness]);
        let hb = s.arc(vb, vb, [c, c, 0.0], [0.0, 0.0, 1.0], major, -pi, pi);
        let ht = s.arc(vt, vt, [c, c, thickness], [0.0, 0.0, 1.0], major, -pi, pi);
        s.faces[0].loops.push(vec![(hb, true)]);
        s.faces[1].loops.push(vec![(ht, false)]);
        // the hole: material outside the cylinder, so the outward side faces the axis
        s.face(SurfaceSpec::Cylinder { c: [c, c, 0.0], axis: [0.0, 0.0, 1.0], major }, false, vec![vec![(hb, false)], vec![(ht, true)]]);
        s
    }

    /// Replaces the surface of face `i` (to test unsupported surfaces).
    pub fn set_surface(&mut self, i: usize, surface: SurfaceSpec) {
        self.faces[i].surface = surface;
    }

    /// The number of faces.
    pub fn face_count(&self) -> usize {
        self.faces.len()
    }

    /// The unpacked B-rep segment: some leading bytes, `ASM BinaryFile4`, the header and the
    /// token stream. `kernel` e.g. 228.
    pub fn segment(&self, kernel: i32, ending: Ending) -> Vec<u8> {
        let mut out = b"segment data before the B-rep\x00\x01\x02".to_vec();
        out.extend(b"ASM BinaryFile4");
        for v in [kernel * 100, 0, 2, 7] {
            out.extend(v.to_le_bytes());
        }
        out.extend(self.tokens(kernel, ending));
        out.extend(b"\x00\x00trailing segment data");
        out
    }

    fn tokens(&self, kernel: i32, ending: Ending) -> Vec<u8> {
        let nf = self.faces.len();
        let nl: usize = self.faces.iter().map(|f| f.loops.len()).sum();
        let nc: usize = self.faces.iter().flat_map(|f| &f.loops).map(Vec::len).sum();
        let (ne, nv) = (self.edges.len(), self.points.len());
        let base_f = 4;
        let base_l = base_f + nf;
        let base_c = base_l + nl;
        let base_e = base_c + nc;
        let base_v = base_e + ne;
        let base_p = base_v + nv;
        let base_s = base_p + nv;
        let base_k = base_s + nf;
        let attrib = (base_k + ne) as i32;
        let i = |v: usize| v as i32;

        // coedge numbering, and per edge the coedges that use it
        let mut loop_of_coedge = Vec::new();
        let mut loop_first = Vec::new();
        let mut uses: Vec<Vec<usize>> = vec![Vec::new(); ne];
        let mut c = 0;
        for f in &self.faces {
            for lp in &f.loops {
                loop_first.push(c);
                for &(e, _) in lp {
                    uses[e].push(c);
                    loop_of_coedge.push(loop_first.len() - 1);
                    c += 1;
                }
            }
        }
        let mut t = Tokens::default();
        let ver = format!("{kernel}.3.1.65535");
        t.str("OpenAEC synthetic part").str(&format!("ASM {ver} NT")).str("Fri Nov 06 10:00:00 2026");
        t.real(10.0).real(1e-6).real(1e-10);
        t.name("asmheader").common(-1).str(&ver).end();
        t.name("body").common(-1).ptr(2).ptr(-1).ptr(-1).end();
        t.name("lump").common(-1).ptr(-1).ptr(3).ptr(1).end();
        t.name("shell").common(-1).ptr(-1).ptr(-1).ptr(i(base_f)).ptr(-1).ptr(2).end();
        let mut l = 0;
        for (k, f) in self.faces.iter().enumerate() {
            let next = if k + 1 < nf { i(base_f + k + 1) } else { -1 };
            // the first face carries an attribute, which the reader skips
            t.name("face")
                .common(if k == 0 { attrib } else { -1 })
                .ptr(next)
                .ptr(i(base_l + l))
                .ptr(3)
                .ptr(-1)
                .ptr(i(base_s + k))
                .logical(f.forward)
                .logical(false)
                .end();
            l += f.loops.len();
        }
        let mut l = 0;
        let mut c = 0;
        for (k, f) in self.faces.iter().enumerate() {
            for (j, lp) in f.loops.iter().enumerate() {
                let next = if j + 1 < f.loops.len() { i(base_l + l + 1) } else { -1 };
                t.name("loop").common(-1).ptr(next).ptr(i(base_c + c)).ptr(i(base_f + k)).end();
                l += 1;
                c += lp.len();
            }
        }
        let mut c = 0;
        for f in &self.faces {
            for lp in &f.loops {
                let first = c;
                for (j, &(e, fwd)) in lp.iter().enumerate() {
                    let next = base_c + first + (j + 1) % lp.len();
                    let prev = base_c + first + (j + lp.len() - 1) % lp.len();
                    let partner = uses[e].iter().copied().find(|&o| o != c).map(|o| i(base_c + o)).unwrap_or(-1);
                    t.name("coedge")
                        .common(-1)
                        .ptr(i(next))
                        .ptr(i(prev))
                        .ptr(partner)
                        .ptr(i(base_e + e))
                        .logical(fwd)
                        .ptr(i(base_l + loop_of_coedge[c]))
                        .int(0)
                        .ptr(-1)
                        .end();
                    c += 1;
                }
            }
        }
        for (k, e) in self.edges.iter().enumerate() {
            let co = uses[k].first().map(|&c| i(base_c + c)).unwrap_or(-1);
            t.name("edge")
                .common(-1)
                .ptr(i(base_v + e.start))
                .real(e.t0)
                .ptr(i(base_v + e.end))
                .real(e.t1)
                .ptr(co)
                .ptr(i(base_k + k))
                .logical(true)
                .str("convex")
                .end();
        }
        for k in 0..nv {
            let edge = self.edges.iter().position(|e| e.start == k || e.end == k).map(|e| i(base_e + e)).unwrap_or(-1);
            t.name("vertex").common(-1).ptr(edge).int(0).ptr(i(base_p + k)).end();
        }
        for p in &self.points {
            t.name("point").common(-1).pos(*p).end();
        }
        for f in &self.faces {
            match &f.surface {
                SurfaceSpec::Plane { p, n, u } => {
                    t.name("plane-surface").common(-1).pos(*p).vec(*n).vec(*u);
                    (0..5).for_each(|_| {
                        t.logical(false);
                    });
                }
                SurfaceSpec::Cylinder { c, axis, major } => {
                    t.name("cone-surface")
                        .common(-1)
                        .pos(*c)
                        .vec(*axis)
                        .vec(*major)
                        .real(1.0)
                        .logical(true)
                        .logical(true)
                        .real(0.0)
                        .real(1.0)
                        .real(d3(*major, [0.0; 3]));
                    (0..5).for_each(|_| {
                        t.logical(false);
                    });
                }
                SurfaceSpec::Cone { c, axis, major, half_angle } => {
                    t.name("cone-surface")
                        .common(-1)
                        .pos(*c)
                        .vec(*axis)
                        .vec(*major)
                        .real(1.0)
                        .logical(true)
                        .logical(true)
                        .real(half_angle.sin())
                        .real(half_angle.cos())
                        .real(d3(*major, [0.0; 3]));
                    (0..5).for_each(|_| {
                        t.logical(false);
                    });
                }
                SurfaceSpec::Other(name) => {
                    t.name(name).common(-1).real(1.0);
                }
            }
            t.end();
        }
        for e in &self.edges {
            match &e.curve {
                CurveSpec::Line => {
                    let (a, b) = (self.points[e.start], self.points[e.end]);
                    t.name("straight-curve").common(-1).pos(a).vec(unit(sub(b, a))).logical(true).logical(true);
                }
                CurveSpec::Circle { center, normal, major } => {
                    t.name("ellipse-curve").common(-1).pos(*center).vec(*normal).vec(*major).real(1.0).logical(true).logical(true);
                }
                CurveSpec::Other(name) => {
                    t.name(name).common(-1).real(0.0);
                }
            }
            t.end();
        }
        t.name("colour-tag-attrib").common(-1).ptr(i(base_f)).int(3).end();
        match ending {
            Ending::EndRecord => {
                t.name("End-of-ASM-data").end();
            }
            Ending::History => {
                t.name("Begin-of-ASM-History-Data").end();
                t.name("delta_state").common(-1).int(1).end();
                t.0.push(0xF3);
            }
        }
        t.0
    }
}

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn unit(a: V3) -> V3 {
    let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    [a[0] / l, a[1] / l, a[2] / l]
}

// ------------------------------------------------------------------------------------------
// Display cache

/// (offset, length) of a vertex buffer and those of its index buffers.
type LastBuffer = (u32, u32, Vec<(u32, u32)>);

/// A display cache (SPEC §5): `stream_mesh_000` with the buffers and `world` with the objects.
#[derive(Default)]
pub struct Cache {
    mesh: Vec<u8>,
    world: Vec<u8>,
    next_id: u64,
    triangle_format: Option<String>,
    line_format: Option<String>,
    /// The ranges of the last vertex buffer and its index buffers.
    last: Option<LastBuffer>,
}

impl Cache {
    fn id(&mut self) -> String {
        self.next_id += 1;
        format!("{:016X}", 0xA5A5_0000_0000_0000u64 + self.next_id)
    }

    fn header(&mut self, class: &str, id: &str) {
        let w = &mut self.world;
        w.extend((class.len() as u32).to_le_bytes());
        w.extend(class.encode_utf16().flat_map(u16::to_le_bytes));
        w.extend((id.len() as u32).to_le_bytes());
        w.extend(id.encode_utf16().flat_map(u16::to_le_bytes));
    }

    /// Some object the reader does not need, between the buffers.
    fn filler(&mut self) {
        let id = self.id();
        self.header("AdvancedMaterialEffectInstance", &id);
        self.world.extend([7, 0, 0, 0, 0x42, 0x42, 0x42, 1, 0, 0, 0]);
    }

    fn vertex_buffer(&mut self, lines: bool, data: &[f32]) {
        self.filler();
        let id = self.id();
        self.header("VertexBuffer", &id);
        self.world.extend([0, 1]);
        let existing = if lines { self.line_format.clone() } else { self.triangle_format.clone() };
        match existing {
            Some(fid) => self.header("VertexFormat", &fid),
            None => {
                let fid = self.id();
                self.header("VertexFormat", &fid);
                // [components − 1, semantic, offset in floats, 0, 0, 0]
                let elements: &[[u32; 3]] = if lines { &[[2, 0, 0]] } else { &[[2, 0, 0], [2, 1, 3], [1, 0x102, 6]] };
                self.world.extend((elements.len() as u32).to_le_bytes());
                for e in elements {
                    for v in [e[0], e[1], e[2], 0, 0, 0] {
                        self.world.extend(v.to_le_bytes());
                    }
                }
                // unknown bytes after a first definition (SPEC §5.3)
                self.world.extend([9, 0, 0, 0, 0xEE, 0xEE, 0xEE]);
                if lines {
                    self.line_format = Some(fid);
                } else {
                    self.triangle_format = Some(fid);
                }
            }
        }
        let off = self.mesh.len() as u32;
        self.mesh.extend(data.iter().flat_map(|v| v.to_le_bytes()));
        for v in [1, 0, off, (data.len() * 4) as u32, 1] {
            self.world.extend(v.to_le_bytes());
        }
        self.last = Some((off, (data.len() * 4) as u32, Vec::new()));
    }

    fn index_buffer(&mut self, indices: &[u16]) {
        let id = self.id();
        self.header("AIndexBuffer", &id);
        let off = self.mesh.len() as u32;
        self.mesh.extend(indices.iter().flat_map(|v| v.to_le_bytes()));
        self.world.push(0);
        for v in [0, off, (indices.len() * 2) as u32, 1] {
            self.world.extend(v.to_le_bytes());
        }
        if let Some(last) = &mut self.last {
            last.2.push((off, (indices.len() * 2) as u32));
        }
    }

    /// `n` more triangle vertex buffers (with their index buffers) on the ranges of the last
    /// one: damaged or hostile data that reads the same bytes many times.
    pub fn repeat_last(&mut self, n: usize) -> &mut Self {
        let Some((off, len, ibs)) = self.last.clone() else { return self };
        let fid = self.triangle_format.clone().expect("a triangle mesh first");
        for _ in 0..n {
            let id = self.id();
            self.header("VertexBuffer", &id);
            self.world.extend([0, 1]);
            self.header("VertexFormat", &fid);
            for v in [1, 0, off, len, 1] {
                self.world.extend(v.to_le_bytes());
            }
            for &(ioff, ilen) in &ibs {
                let id = self.id();
                self.header("AIndexBuffer", &id);
                self.world.push(0);
                for v in [0, ioff, ilen, 1] {
                    self.world.extend(v.to_le_bytes());
                }
            }
        }
        self
    }

    /// A triangle mesh in cm (positions and normals per vertex, uv written as 0) with one or
    /// more levels of detail.
    pub fn mesh(&mut self, positions: &[[f32; 3]], normals: &[[f32; 3]], lods: &[&[u16]]) -> &mut Self {
        let data: Vec<f32> = positions.iter().zip(normals).flat_map(|(p, n)| [p[0], p[1], p[2], n[0], n[1], n[2], 0.0, 0.0]).collect();
        self.vertex_buffer(false, &data);
        for lod in lods {
            self.index_buffer(lod);
        }
        self
    }

    /// Edges: positions only, two indices per segment.
    pub fn lines(&mut self, positions: &[[f32; 3]], indices: &[u16]) -> &mut Self {
        let data: Vec<f32> = positions.iter().flatten().copied().collect();
        self.vertex_buffer(true, &data);
        self.index_buffer(indices);
        self
    }

    /// A box from `min` to `max` (cm) as 12 triangles with flat normals.
    pub fn boxed(&mut self, min: [f32; 3], max: [f32; 3]) -> &mut Self {
        let (mut p, mut n, mut idx) = (Vec::new(), Vec::new(), Vec::new());
        let corner = |x: usize, y: usize, z: usize| [[min[0], max[0]][x], [min[1], max[1]][y], [min[2], max[2]][z]];
        for axis in 0..3 {
            for side in 0..2 {
                let mut normal = [0.0f32; 3];
                normal[axis] = if side == 1 { 1.0 } else { -1.0 };
                let base = p.len() as u16;
                let (u, v) = ((axis + 1) % 3, (axis + 2) % 3);
                for (a, b) in [(0, 0), (1, 0), (1, 1), (0, 1)] {
                    let mut c = [0usize; 3];
                    c[axis] = side;
                    c[u] = a;
                    c[v] = b;
                    p.push(corner(c[0], c[1], c[2]));
                    n.push(normal);
                }
                let quad = if side == 1 { [0, 1, 2, 0, 2, 3] } else { [0, 2, 1, 0, 3, 2] };
                idx.extend(quad.map(|q| base + q));
            }
        }
        // a coarser level of detail first: one triangle
        let coarse = [idx[0], idx[1], idx[2]];
        self.mesh(&p, &n, &[&coarse, &idx])
    }

    /// The bzip2 stream of `CacheGraphics/OGSCache/OGS/DefaultScene/streams`.
    pub fn stream(&self) -> Vec<u8> {
        let mut data = Vec::new();
        for (name, body) in [("stream_mesh_000", &self.mesh), ("world", &self.world)] {
            let units: Vec<u8> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
            data.extend((units.len() as u32).to_le_bytes());
            data.extend(units);
            data.extend((body.len() as u64).to_le_bytes());
            data.extend(body.iter());
        }
        let mut e = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::best());
        e.write_all(&data).unwrap();
        e.finish().unwrap()
    }
}

// ------------------------------------------------------------------------------------------
// The compound file

/// A synthetic Inventor file: streams by path, written as an [MS-CFB] version 3 file.
#[derive(Default)]
pub struct File {
    streams: Vec<(String, Vec<u8>)>,
    segments: usize,
}

impl File {
    pub fn new() -> Self {
        Self::default()
    }

    /// Any stream, e.g. `/UFRxDoc`.
    pub fn stream(mut self, path: &str, data: Vec<u8>) -> Self {
        self.streams.push((path.to_string(), data));
        self
    }

    /// A property set stream at the top level.
    pub fn property_set(self, fmtid: &str, props: &[(u32, Prop)], names: &[(u32, &str)]) -> Self {
        let name = format!("/{}", stream_name(fmtid));
        self.stream(&name, property_set_stream(fmtid, props, names))
    }

    /// A segment: `B`/`M` streams in `RSeStorage` (codec 1 zlib, 2 zstd).
    pub fn segment(mut self, class: &str, codec: u8, content: &[u8]) -> Self {
        const KEY: &[u8] = b"abcdefghijklmnopqrstuvwxyz012345";
        let key: String = (0..26).map(|k| KEY[(k * 7 + self.segments * 11) % KEY.len()] as char).collect();
        self.segments += 1;
        let m = meta_stream(class);
        let b = data_stream(codec, content);
        self.stream(&format!("/RSeStorage/M{key}"), m).stream(&format!("/RSeStorage/B{key}"), b)
    }

    /// The display cache stream.
    pub fn display_cache(self, cache: &Cache) -> Self {
        self.stream(crate::ogs::STREAM, cache.stream())
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut cf = cfb::CompoundFile::create_with_version(cfb::Version::V3, Cursor::new(Vec::new())).expect("compound file");
        for (path, data) in &self.streams {
            // the storages on the way
            let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
            let mut dir = String::new();
            for p in &parts[..parts.len() - 1] {
                dir.push('/');
                dir.push_str(p);
                if !cf.exists(&dir) {
                    cf.create_storage(&dir).expect("storage");
                }
            }
            let mut s = cf.create_stream(path).expect("stream");
            s.write_all(data).expect("write");
        }
        cf.flush().expect("flush");
        cf.into_inner().into_inner()
    }
}

/// The iProperties of a part: Inventor Summary (title, author, thumbnail), Design Tracking
/// Properties and Control (SPEC §2.2).
pub fn part_properties(file: File, part_number: &str, subtype: &str, version: &str, build: u32) -> File {
    let png = png(4, 3);
    file.property_set(
        crate::props::INVENTOR_SUMMARY,
        &[(2, s("Synthetic bracket")), (4, s("OpenAEC test")), (17, Prop::Cf(thumbnail_cf(4, 3, &png))), (255, s("Inventor Summary Information"))],
        &[],
    )
    .property_set(crate::props::INVENTOR_DOC_SUMMARY, &[(15, s("OpenAEC Foundation")), (255, s("Inventor Document Summary Information"))], &[])
    .property_set(
        crate::props::DESIGN_TRACKING_PROPERTIES,
        &[
            (4, Prop::FileTime(125_911_584_000_000_000)),
            (5, s(part_number)),
            (20, s("Aluminium 6061")),
            (29, s("Test part")),
            (31, Prop::Clsid("4D29B490-49B2-11D0-93C3-7E0706000000".into())),
            (32, s(subtype)),
            (41, s("Designer")),
            (61, Prop::R8(2.7)),
            (67, s(version)),
            (72, s("Aluminium (Polished)")),
            (255, s("Design Tracking Properties")),
        ],
        &[],
    )
    .property_set(crate::props::DESIGN_TRACKING_CONTROL, &[(22, Prop::UI4(build)), (255, s("Design Tracking Control"))], &[])
    .property_set(
        crate::props::INVENTOR_USER_DEFINED,
        &[(2, s("Project X")), (3, Prop::R8(12.5)), (255, s("Inventor User Defined Properties"))],
        &[(2, "Project"), (3, "Weight class")],
    )
    .property_set(
        crate::props::PRIVATE_MODEL_INFORMATION,
        &[(16, s("<PartActiveColorStyle><StyleName>Blue - Wall Paint - Glossy</StyleName></PartActiveColorStyle>"))],
        &[],
    )
}

/// A whole part file with the solid's B-rep in a `PmBRepSegment` (codec 1 zlib or 2 zstd),
/// next to other segments.
pub fn part(solid: &Solid, codec: u8, kernel: i32, ending: Ending) -> Vec<u8> {
    let release = 1996 + (kernel as u32 - 200);
    let build = (release - 1996) * 10_000_000 + 158_000;
    part_properties(File::new(), "Bracket-01", "Modeling", &format!("{release} (Build {build}, 158)"), build)
        .segment("PmAppSegment", codec, b"app")
        .segment("PmBRepSegment", codec, &solid.segment(kernel, ending))
        .segment("PmGraphicsSegment", codec, b"graphics")
        .stream("/UFRxDoc", Vec::new())
        .to_bytes()
}

/// A whole assembly file; with `cache` its display cache, and `UFRxDoc` naming `parts`.
pub fn assembly(cache: Option<&Cache>, parts: &[&str]) -> Vec<u8> {
    let mut ufrx = vec![0u8; 8];
    for p in parts {
        let path = format!("C:\\Users\\someone\\Design\\{p}");
        ufrx.extend((path.encode_utf16().count() as u32).to_le_bytes());
        ufrx.extend(path.encode_utf16().flat_map(u16::to_le_bytes));
        ufrx.extend([0, 0, 0, 0]);
    }
    let mut f = part_properties(File::new(), "Frame", "Assembly", "2023 (Build 270158000, 158)", 270_158_000)
        .segment("AmAppSegment", 1, b"app")
        .segment("AmBREPSegment", 1, b"no B-rep in an assembly")
        .stream("/UFRxDoc", ufrx);
    if let Some(c) = cache {
        f = f.display_cache(c);
    }
    f.to_bytes()
}
