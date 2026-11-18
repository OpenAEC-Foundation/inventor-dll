//! Tessellation of the B-rep (SPEC §4.8): faces on `plane-surface` and `cone-surface`
//! (cylinders and cones), bounded by `straight-curve` and `ellipse-curve` edges. Faces on other
//! surfaces, or with other edge curves, are skipped and counted.
//!
//! Each face is triangulated in its own parameter plane (a constrained Delaunay
//! triangulation of its loops), then lifted back onto the surface. Edge points are sampled once
//! per edge, so neighbouring faces share them exactly; that is used to give cone faces the
//! orientation of their planar neighbours (the outward side of a cone face is not in the
//! specification: see the README).

use crate::brep::{Brep, Curve, Surface};
use crate::param;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

/// Points of one face's loops, and of all faces of a body: beyond these the face (or the rest
/// of the body) is skipped as damaged.
const MAX_FACE_POINTS: usize = 1_000_000;
const MAX_BODY_POINTS: usize = 20_000_000;
use std::f64::consts::TAU;

pub(crate) type V3 = [f64; 3];

pub(crate) fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub(crate) fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
pub(crate) fn mul(a: V3, s: f64) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
pub(crate) fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub(crate) fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
pub(crate) fn len(a: V3) -> f64 {
    dot(a, a).sqrt()
}
pub(crate) fn unit(a: V3) -> Option<V3> {
    let l = len(a);
    (l > 1e-300 && l.is_finite()).then(|| mul(a, 1.0 / l))
}
pub(crate) fn dist(a: V3, b: V3) -> f64 {
    len(sub(a, b))
}

/// Options for tessellation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Options {
    /// Segments of a full circle (edges and cylinders); 48 = 7.5°.
    pub segments_per_circle: u32,
    /// An assembly is put together from its part files when all of them are given, also when
    /// it has a display cache; the cache is taken when part files are missing. Off: the cache
    /// first, the part files only for an assembly without one.
    pub parts_first: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self { segments_per_circle: 48, parts_first: true }
    }
}

/// A triangulated face, in model units.
#[derive(Debug, Clone, Default)]
pub struct FaceMesh {
    pub positions: Vec<V3>,
    pub normals: Vec<V3>,
    pub triangles: Vec<[u32; 3]>,
    /// The colour the face has of its own.
    pub color: Option<[f32; 3]>,
    /// The orientation follows from the specification (planes); cone faces may be turned to
    /// match their neighbours.
    pub known: bool,
}

/// What became of the faces of a body.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stats {
    pub faces: usize,
    pub tessellated: usize,
    /// Faces skipped per surface type that is not specified (`spline-surface`: 3).
    pub unsupported_surface: BTreeMap<String, usize>,
    /// Faces skipped per edge curve type that is not specified (`intcurve-curve`: 2).
    pub unsupported_curve: BTreeMap<String, usize>,
    /// Faces whose records or loops could not be followed or triangulated, per reason.
    pub failures: BTreeMap<String, usize>,
    /// Cone faces turned to match their planar neighbours.
    pub reoriented: usize,
    /// Bodies with a transform record that could not be read (shown untransformed).
    pub transforms_ignored: usize,
    /// Bodies left out because Inventor does not show them as part of the part.
    pub bodies_not_shown: usize,
}

impl Stats {
    pub fn merge(&mut self, o: &Stats) {
        self.faces += o.faces;
        self.tessellated += o.tessellated;
        for (k, v) in &o.unsupported_surface {
            *self.unsupported_surface.entry(k.clone()).or_default() += v;
        }
        for (k, v) in &o.unsupported_curve {
            *self.unsupported_curve.entry(k.clone()).or_default() += v;
        }
        for (k, v) in &o.failures {
            *self.failures.entry(k.clone()).or_default() += v;
        }
        self.reoriented += o.reoriented;
        self.transforms_ignored += o.transforms_ignored;
        self.bodies_not_shown += o.bodies_not_shown;
    }

    pub fn skipped(&self) -> usize {
        self.unsupported_surface.values().sum::<usize>() + self.unsupported_curve.values().sum::<usize>() + self.failed()
    }

    /// Faces whose records or loops could not be followed or triangulated.
    pub fn failed(&self) -> usize {
        self.failures.values().sum()
    }

    /// Warnings for the skipped faces, e.g. `Inventor: 12 faces skipped (unsupported surface:
    /// spline-surface 10, torus-surface 2)`.
    pub fn warnings(&self) -> Vec<String> {
        let list = |m: &BTreeMap<String, usize>| m.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", ");
        let mut out = Vec::new();
        let n: usize = self.unsupported_surface.values().sum();
        if n > 0 {
            out.push(format!("Inventor: {n} faces skipped (unsupported surface: {})", list(&self.unsupported_surface)));
        }
        let n: usize = self.unsupported_curve.values().sum();
        if n > 0 {
            out.push(format!("Inventor: {n} faces skipped (unsupported edge curve: {})", list(&self.unsupported_curve)));
        }
        if self.failed() > 0 {
            out.push(format!("Inventor: {} faces skipped (could not be tessellated: {})", self.failed(), list(&self.failures)));
        }
        if self.transforms_ignored > 0 {
            out.push(format!("Inventor: {} bodies have a transform that could not be read (shown untransformed)", self.transforms_ignored));
        }
        out
    }
}

enum Skip {
    Surface(String),
    Curve(String),
    /// With the reason.
    Failed(&'static str),
}

/// Cap on the length of a `next` chain (a cycle in damaged data stops here).
const MAX_CHAIN: usize = 1_000_000;

/// Follows a `next` chain from `first`, stopping at −1, a repeat, or a record of another type.
fn chain(first: i32, next: impl Fn(i32) -> Option<i32>) -> Vec<i32> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut i = first;
    while i >= 0 && seen.insert(i) && out.len() < MAX_CHAIN {
        out.push(i);
        match next(i) {
            Some(n) => i = n,
            None => break,
        }
    }
    out
}

/// The faces of a body (body → lumps → shells → faces).
pub fn body_faces(b: &Brep, body: i32) -> Vec<i32> {
    let Some(bd) = b.body(body) else { return Vec::new() };
    let mut faces = Vec::new();
    for lump in chain(bd.lump, |i| b.lump(i).map(|l| l.next)) {
        let Some(l) = b.lump(lump) else { continue };
        for shell in chain(l.shell, |i| b.shell(i).map(|s| s.next)) {
            let Some(s) = b.shell(shell) else { continue };
            faces.extend(chain(s.face, |i| b.face(i).map(|f| f.next)).into_iter().filter(|&f| b.face(f).is_some()));
        }
    }
    faces
}

/// Tessellates one body: its faces with their triangles, and what was skipped. `used` holds the
/// face, loop and coedge records taken by bodies before (a record shared by several owners in
/// damaged data is tessellated once).
pub fn body(b: &Brep, body: i32, opt: &Options, used: &mut HashSet<i32>) -> (Vec<FaceMesh>, Stats) {
    let mut stats = Stats::default();
    let mut t = Tessellator { b, opt, edges: HashMap::new(), used, points: 0 };
    let colours = b.face_colours();
    let mut meshes = Vec::new();
    for face in body_faces(b, body) {
        stats.faces += 1;
        if !t.used.insert(face) || t.points > MAX_BODY_POINTS {
            *stats.failures.entry("damaged topology".into()).or_default() += 1;
            continue;
        }
        // damaged input must not take the whole part down
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| t.face(face))).unwrap_or(Err(Skip::Failed("damaged geometry")));
        match result {
            Ok(mut m) if !m.triangles.is_empty() => {
                stats.tessellated += 1;
                m.color = colours.get(&face).copied();
                meshes.push(m);
            }
            Ok(_) => *stats.failures.entry("no triangles".into()).or_default() += 1,
            Err(Skip::Failed(why)) => *stats.failures.entry(why.into()).or_default() += 1,
            Err(Skip::Surface(s)) => *stats.unsupported_surface.entry(s).or_default() += 1,
            Err(Skip::Curve(c)) => *stats.unsupported_curve.entry(c).or_default() += 1,
        }
    }
    stats.reoriented = orient(&mut meshes);
    // the body stands where its transform puts it
    if let Some([x, y, z, t]) = b.body(body).and_then(|bd| b.transform(bd.transform)) {
        let turn = |v: V3| add(add(mul(x, v[0]), mul(y, v[1])), mul(z, v[2]));
        let mirrored = dot(cross(x, y), z) < 0.0;
        for m in &mut meshes {
            for p in &mut m.positions {
                *p = add(turn(*p), t);
            }
            for n in &mut m.normals {
                *n = unit(turn(*n)).unwrap_or(*n);
            }
            if mirrored {
                for t in &mut m.triangles {
                    t.swap(1, 2);
                }
            }
        }
    } else if b.body(body).is_some_and(|bd| bd.transform >= 0) {
        stats.transforms_ignored += 1;
    }
    (meshes, stats)
}

struct Tessellator<'a> {
    b: &'a Brep,
    opt: &'a Options,
    /// Sampled points per edge, from its start vertex to its end vertex.
    edges: HashMap<i32, Result<Vec<V3>, String>>,
    used: &'a mut HashSet<i32>,
    /// Loop points of the faces so far.
    points: usize,
}

impl Tessellator<'_> {
    fn face(&mut self, i: i32) -> Result<FaceMesh, Skip> {
        let b = self.b;
        let face = b.face(i).ok_or(Skip::Failed("damaged topology"))?;
        let surface = b.surface(face.surface).ok_or(Skip::Failed("surface record not as expected"))?;
        if let Surface::Other(name) = &surface {
            return Err(Skip::Surface(name.clone()));
        }
        let mut loops = Vec::new();
        let mut count = 0usize;
        for l in chain(face.loop_, |i| b.loop_(i).map(|l| l.next)) {
            let lp = b.loop_(l).ok_or(Skip::Failed("damaged topology"))?;
            if !self.used.insert(l) {
                return Err(Skip::Failed("damaged topology"));
            }
            let mut pts: Vec<V3> = Vec::new();
            let coedges = chain(lp.coedge, |i| b.coedge(i).map(|c| c.next));
            for c in coedges {
                let co = b.coedge(c).ok_or(Skip::Failed("damaged topology"))?;
                if !self.used.insert(c) {
                    return Err(Skip::Failed("damaged topology"));
                }
                let samples = self.edge(co.edge).map_err(Skip::Curve)?;
                count += samples.len();
                if count > MAX_FACE_POINTS {
                    return Err(Skip::Failed("too many points"));
                }
                let mut s = samples.clone();
                if !co.forward {
                    s.reverse();
                }
                // the last point of one edge is the first of the next
                let skip = usize::from(pts.last().is_some_and(|p| s.first() == Some(p)));
                pts.extend(s.into_iter().skip(skip));
            }
            while pts.len() > 1 && pts.first() == pts.last() {
                pts.pop();
            }
            pts.dedup();
            if !pts.is_empty() {
                loops.push(pts);
            }
        }
        self.points += count;
        let segments = self.opt.segments_per_circle;
        let kind = match &surface {
            Surface::Plane { .. } => "plane",
            Surface::Cone { .. } => "cone or cylinder",
            Surface::Sphere { .. } => "sphere",
            Surface::Torus { .. } => "torus",
            _ => "spline surface",
        };
        match surface {
            Surface::Plane { .. } | Surface::Cone { .. } if loops.is_empty() => None,
            Surface::Plane { p, n, u } => plane(p, n, u, face.forward, &loops),
            Surface::Cone { c, a, m, ratio, sin, cos } => cone(c, a, m, ratio, sin, cos, face.forward, &loops),
            // a negative radius turns the surface normal inwards
            Surface::Sphere { c, r, x, pole } => {
                param::Sphere::new(c, r, x, pole, &loops, segments).and_then(|s| param::face(&s, &loops, face.forward == (r > 0.0)))
            }
            Surface::Torus { c, a, major, minor, x } => {
                param::Torus::new(c, a, major, minor, x, segments).and_then(|s| param::face(&s, &loops, face.forward == (minor > 0.0)))
            }
            Surface::Spline { surface, forward } => param::Spline::new(&surface).and_then(|s| param::face(&s, &loops, face.forward == forward)),
            Surface::Other(_) => unreachable!(),
        }
        .ok_or(Skip::Failed(kind))
    }

    /// The points of an edge from its start to its end vertex (model units).
    fn edge(&mut self, e: i32) -> Result<&Vec<V3>, String> {
        if !self.edges.contains_key(&e) {
            let s = sample_edge(self.b, e, self.opt);
            self.edges.insert(e, s);
        }
        self.edges[&e].as_ref().map_err(Clone::clone)
    }
}

fn sample_edge(b: &Brep, e: i32, opt: &Options) -> Result<Vec<V3>, String> {
    let fail = || "edge".to_string();
    let edge = b.edge(e).ok_or_else(fail)?;
    let s = b.vertex_point(edge.start).ok_or_else(fail)?;
    let t = b.vertex_point(edge.end).ok_or_else(fail)?;
    let curve = if edge.curve < 0 { None } else { Some(b.curve(edge.curve).ok_or_else(fail)?) };
    match curve {
        None | Some(Curve::Line { .. }) => Ok(if s == t { vec![s] } else { vec![s, t] }),
        Some(Curve::Ellipse { c, n, m, ratio }) => {
            let n = unit(n).ok_or_else(fail)?;
            let r = len(m);
            let ratio = if ratio.abs() > 1e-12 && ratio.is_finite() { ratio } else { 1.0 };
            // point = C + cos θ·M + sin θ·(r·(N×M)) (SPEC §4.7); the turning direction and
            // the edge sense are one choice: θ or −θ, whichever meets the vertices
            let y = mul(cross(n, m), ratio);
            let at = |th: f64| add(c, add(mul(m, th.cos()), mul(y, th.sin())));
            let err = |sg: f64| dist(at(sg * edge.t0), s) + dist(at(sg * edge.t1), t);
            let (ea, eb) = (err(1.0), err(-1.0));
            let tol = 1e-6 * r.max(1e-9) + 1e-9;
            let sign = if ea <= tol && eb <= tol {
                if edge.forward {
                    1.0
                } else {
                    -1.0
                }
            } else if ea <= eb {
                1.0
            } else {
                -1.0
            };
            let span = edge.t1 - edge.t0;
            if !span.is_finite() || span.abs() > 4.0 * TAU {
                return Err(fail());
            }
            let step = TAU / f64::from(opt.segments_per_circle.max(3));
            let n = ((span.abs() / step).ceil() as usize).clamp(1, 4096);
            let mut pts: Vec<V3> = (0..=n).map(|k| at(sign * (edge.t0 + span * k as f64 / n as f64))).collect();
            // the ends are the vertices themselves, so neighbouring edges meet exactly
            pts[0] = s;
            pts[n] = t;
            if s == t && n == 1 {
                pts.pop();
            }
            Ok(pts)
        }
        Some(Curve::Spline(c)) => {
            // the edge runs along the curve or against it, with its parameters negated then:
            // whichever meets the vertices
            let size = c.points.iter().fold(0.0f64, |m, p| m.max(dist([p[0], p[1], p[2]], s))).max(dist(s, t));
            let err = |sg: f64| dist(c.eval(sg * edge.t0), s) + dist(c.eval(sg * edge.t1), t);
            let (ea, eb) = (err(1.0), err(-1.0));
            let sign = if ea <= eb { 1.0 } else { -1.0 };
            // the curve may be an approximation of the edge: it passes near the vertices
            if !(ea.min(eb) <= 0.02 * size + 1e-7) || !edge.t0.is_finite() || !edge.t1.is_finite() {
                return Err("intcurve-curve (its ends are not on its curve)".into());
            }
            let (a, z) = (sign * edge.t0, sign * edge.t1);
            let (lo, hi) = c.range();
            // through the knots when the edge lies within the range of the curve, else evenly
            let inside = a.min(z) >= lo - 1e-9 && a.max(z) <= hi + 1e-9;
            let mut stops = vec![a];
            if inside {
                let mut knots = c.knots_between(a, z);
                if a > z {
                    knots.reverse();
                }
                stops.extend(knots);
            } else {
                let turns = ((z - a).abs() / (hi - lo).max(1e-300)).min(4.0);
                let n = (turns * (c.points.len() as f64)).ceil().clamp(1.0, 512.0) as usize;
                stops.extend((1..n).map(|k| a + (z - a) * k as f64 / n as f64));
            }
            stops.push(z);
            let parts = if c.degree == 1 { 1 } else { (2048 / stops.len()).clamp(1, 8) };
            let mut pts: Vec<V3> = vec![s];
            for w in stops.windows(2) {
                for k in 1..=parts {
                    pts.push(c.eval(w[0] + (w[1] - w[0]) * k as f64 / parts as f64));
                }
            }
            let last = pts.len() - 1;
            pts[last] = t;
            let mut pts = simplify(&pts);
            if s == t && pts.len() == 2 {
                pts.pop();
            }
            Ok(pts)
        }
        Some(Curve::Other(name)) => Err(name),
    }
}

/// A polyline with the points left out that lie within a small distance (1/2000 of its
/// length) of the line through the points kept; the ends stay.
fn simplify(pts: &[V3]) -> Vec<V3> {
    if pts.len() < 3 {
        return pts.to_vec();
    }
    let length: f64 = pts.windows(2).map(|w| dist(w[0], w[1])).sum();
    let tol = 5e-4 * length;
    let mut keep = vec![false; pts.len()];
    keep[0] = true;
    keep[pts.len() - 1] = true;
    // a closed curve needs a point across from its start to have a line to measure from
    let mut stack = if pts[0] == pts[pts.len() - 1] {
        let mid = pts.len() / 2;
        keep[mid] = true;
        vec![(0, mid), (mid, pts.len() - 1)]
    } else {
        vec![(0, pts.len() - 1)]
    };
    while let Some((a, z)) = stack.pop() {
        if z <= a + 1 {
            continue;
        }
        let d = sub(pts[z], pts[a]);
        let l2 = dot(d, d);
        let off = |p: V3| {
            let v = sub(p, pts[a]);
            if l2 < 1e-300 {
                len(v)
            } else {
                len(sub(v, mul(d, (dot(v, d) / l2).clamp(0.0, 1.0))))
            }
        };
        let (k, far) = (a + 1..z).map(|k| (k, off(pts[k]))).fold((a, 0.0), |m, c| if c.1 > m.1 { c } else { m });
        if far > tol {
            keep[k] = true;
            stack.push((a, k));
            stack.push((k, z));
        }
    }
    pts.iter().zip(&keep).filter(|p| *p.1).map(|p| *p.0).collect()
}

/// A coordinate for the triangulation, which refuses numbers nearer to zero than about 1e-43
/// (rounding leaves such numbers where zero was meant).
fn tidy(v: f64) -> f64 {
    if v.abs() < 1e-40 {
        0.0
    } else {
        v
    }
}

/// Constrained Delaunay triangulation of rings in a plane, together with loose points (`more`);
/// triangles inside an odd number of rings are kept. Returns triangles as indices into the
/// concatenated ring points followed by `more`.
pub(crate) fn triangulate(rings: &[Vec<[f64; 2]>], more: &[[f64; 2]]) -> Option<Vec<[usize; 3]>> {
    use spade::handles::FixedVertexHandle;
    use spade::{ConstrainedDelaunayTriangulation, Point2, Triangulation};
    let mut cdt = ConstrainedDelaunayTriangulation::<Point2<f64>>::new();
    // spade vertex index → first ring point at that position
    let mut owner: HashMap<usize, usize> = HashMap::new();
    let mut handles: Vec<Vec<FixedVertexHandle>> = Vec::new();
    let mut flat = 0usize;
    for ring in rings {
        let mut hs = Vec::with_capacity(ring.len());
        for p in ring {
            if !p[0].is_finite() || !p[1].is_finite() {
                return None;
            }
            let h = cdt.insert(Point2::new(tidy(p[0]), tidy(p[1]))).ok()?;
            owner.entry(h.index()).or_insert(flat);
            hs.push(h);
            flat += 1;
        }
        handles.push(hs);
    }
    for hs in &handles {
        if hs.len() < 2 {
            continue;
        }
        for k in 0..hs.len() {
            let (a, b) = (hs[k], hs[(k + 1) % hs.len()]);
            if a != b && cdt.can_add_constraint(a, b) {
                cdt.add_constraint(a, b);
            }
        }
    }
    for p in more {
        if !p[0].is_finite() || !p[1].is_finite() {
            return None;
        }
        let h = cdt.insert(Point2::new(tidy(p[0]), tidy(p[1]))).ok()?;
        owner.entry(h.index()).or_insert(flat);
        flat += 1;
    }
    // inside/outside by crossing constraint edges from the outer face
    let mut parity: HashMap<usize, bool> = HashMap::new();
    let mut queue = VecDeque::new();
    for face in cdt.inner_faces() {
        for e in face.adjacent_edges() {
            if e.rev().face().is_outer() {
                let inside = cdt.is_constraint_edge(e.as_undirected().fix());
                if parity.insert(face.fix().index(), inside).is_none() {
                    queue.push_back(face.fix());
                }
                break;
            }
        }
    }
    while let Some(f) = queue.pop_front() {
        let inside = parity[&f.index()];
        let face = cdt.face(f);
        for e in face.adjacent_edges() {
            let Some(other) = e.rev().face().as_inner() else { continue };
            if parity.contains_key(&other.fix().index()) {
                continue;
            }
            let cross = cdt.is_constraint_edge(e.as_undirected().fix());
            parity.insert(other.fix().index(), inside ^ cross);
            queue.push_back(other.fix());
        }
    }
    let mut out = Vec::new();
    for face in cdt.inner_faces() {
        if parity.get(&face.fix().index()) != Some(&true) {
            continue;
        }
        let [a, b, c] = face.vertices().map(|v| owner.get(&v.fix().index()).copied());
        if let (Some(a), Some(b), Some(c)) = (a, b, c) {
            out.push([a, b, c]);
        }
    }
    Some(out)
}

fn plane(p: V3, n: V3, u: V3, forward: bool, loops: &[Vec<V3>]) -> Option<FaceMesh> {
    let n = unit(n)?;
    let u = unit(sub(u, mul(n, dot(u, n)))).or_else(|| unit(cross(n, if n[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] })))?;
    let v = cross(n, u);
    let rings: Vec<Vec<[f64; 2]>> = loops
        .iter()
        .map(|l| {
            l.iter()
                .map(|&q| {
                    let d = sub(q, p);
                    [dot(d, u), dot(d, v)]
                })
                .collect()
        })
        .collect();
    let tris = triangulate(&rings, &[])?;
    let flat: Vec<[f64; 2]> = rings.concat();
    let positions: Vec<V3> = loops.concat();
    // (u, v, n) is right-handed: counter-clockwise in (u, v) faces +n
    let out = if forward { n } else { mul(n, -1.0) };
    let triangles = tris
        .into_iter()
        .filter_map(|[a, b, c]| {
            let area = (flat[b][0] - flat[a][0]) * (flat[c][1] - flat[a][1]) - (flat[c][0] - flat[a][0]) * (flat[b][1] - flat[a][1]);
            if area.abs() < 1e-300 {
                None
            } else if (area > 0.0) == forward {
                Some([a as u32, b as u32, c as u32])
            } else {
                Some([a as u32, c as u32, b as u32])
            }
        })
        .collect();
    Some(compact(FaceMesh { normals: vec![out; positions.len()], positions, triangles, known: true, color: None }))
}

/// Drops the vertices no triangle uses.
pub(crate) fn compact(m: FaceMesh) -> FaceMesh {
    let mut map = vec![u32::MAX; m.positions.len()];
    let mut out = FaceMesh { known: m.known, color: m.color, ..Default::default() };
    for t in &m.triangles {
        let mut nt = [0u32; 3];
        for (k, &i) in t.iter().enumerate() {
            let i = i as usize;
            if map[i] == u32::MAX {
                map[i] = out.positions.len() as u32;
                out.positions.push(m.positions[i]);
                out.normals.push(m.normals[i]);
            }
            nt[k] = map[i];
        }
        out.triangles.push(nt);
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn cone(c: V3, a: V3, m: V3, ratio: f64, sin: f64, cos: f64, forward: bool, loops: &[Vec<V3>]) -> Option<FaceMesh> {
    let a = unit(a)?;
    let m = sub(m, mul(a, dot(m, a)));
    let r0 = len(m);
    let x = unit(m)?;
    let y = cross(a, x);
    let ratio = if ratio.abs() > 1e-12 && ratio.is_finite() { ratio.abs() } else { 1.0 };
    if cos.abs() < 1e-9 || !sin.is_finite() || !cos.is_finite() {
        return None;
    }
    let tan = sin / cos;
    // (θ, v, ρ) of a point: v along the axis, θ around it, ρ the (elliptic) distance
    let local = |q: V3| {
        let d = sub(q, c);
        let (px, py) = (dot(d, x), dot(d, y) / ratio);
        (py.atan2(px), dot(d, a), px.hypot(py))
    };
    let samples: Vec<(f64, f64, f64)> = loops.iter().flatten().map(|&q| local(q)).collect();
    // ρ(v) = |M| + k·v; the sign of the slope from the boundary itself
    let k = if tan.abs() < 1e-12 {
        0.0
    } else {
        let fit = |k: f64| samples.iter().map(|&(_, v, rho)| (rho - (r0 + k * v)).abs()).sum::<f64>();
        if fit(tan) <= fit(-tan) {
            tan
        } else {
            -tan
        }
    };
    let rho = |v: f64| r0 + k * v;
    let lift = |th: f64, v: f64| add(c, add(mul(a, v), mul(add(mul(x, th.cos()), mul(y, ratio * th.sin())), rho(v))));
    let normal = |th: f64| unit(add(add(mul(x, th.cos()), mul(y, th.sin() / ratio)), mul(a, -k))).unwrap_or(x);
    let mean_r = (samples.iter().map(|s| s.2).sum::<f64>() / samples.len().max(1) as f64).max(r0 * 1e-3).max(1e-12);
    let tiny = 1e-9 * mean_r.max(r0);

    // unwrap θ along each loop; the winding tells loops around the axis from closed ones
    let mut around: Vec<Vec<Pt>> = Vec::new();
    let mut closed: Vec<Vec<Pt>> = Vec::new();
    for l in loops {
        let loc: Vec<(f64, f64, f64)> = l.iter().map(|&q| local(q)).collect();
        let mut th: Vec<f64> = Vec::with_capacity(loc.len());
        for &(t, _, r) in &loc {
            let t = match th.last() {
                // on the apex θ is undefined: keep the previous one
                Some(&p) if r <= tiny => p,
                Some(&p) => p + wrap(t - p),
                None if r <= tiny => loc.iter().find(|s| s.2 > tiny).map(|s| s.0).unwrap_or(t),
                None => t,
            };
            th.push(t);
        }
        let mut ring: Vec<Pt> = th.iter().zip(&loc).zip(l).map(|((&t, s), &q)| (t, s.1, Some(q))).collect();
        let winding = match (th.first(), th.last()) {
            (Some(&first), Some(&last)) if th.len() > 1 => ((last - first + wrap(first - last)) / TAU).round() as i64,
            _ => 0,
        };
        match winding {
            0 if ring.len() >= 3 => closed.push(ring),
            0 => {}
            1 => around.push(ring),
            -1 => {
                // run around the axis in increasing θ
                ring.reverse();
                around.push(ring);
            }
            _ => return None,
        }
    }
    let mut polys: Vec<Vec<Pt>> = Vec::new();
    match around.len() {
        0 => {
            // all loops on one 2π branch, around the widest one
            let widest = (0..closed.len()).max_by(|&i, &j| extent(&closed[i]).total_cmp(&extent(&closed[j])))?;
            let reference = mean_theta(&closed[widest]);
            for mut ring in closed {
                shift_into(&mut ring, reference - std::f64::consts::PI);
                polys.push(ring);
            }
        }
        1 | 2 => {
            // the band 0 ≤ θ − start ≤ 2π, with its seam where no closed loop is
            let start = seam_angle(&around[0], &closed);
            let side_a = band_side(&around[0], start);
            let s0 = side_a[0].0;
            let side_b = if around.len() == 2 {
                band_side(&around[1], s0)
            } else if k != 0.0 {
                // a cone closed by its apex
                let apex = -r0 / k;
                let p = add(c, mul(a, apex));
                vec![(s0, apex, Some(p)), (s0 + TAU, apex, Some(p))]
            } else {
                return None;
            };
            let mut band = side_a;
            band.extend(side_b.into_iter().rev());
            polys.push(band);
            for mut ring in closed {
                shift_into(&mut ring, s0);
                polys.push(ring);
            }
        }
        _ => return None,
    }
    let rings2d: Vec<Vec<[f64; 2]>> = polys.iter().map(|r| r.iter().map(|&(t, v, _)| [t * mean_r, v]).collect()).collect();
    let tris = triangulate(&rings2d, &[])?;
    let flat: Vec<(f64, f64, Option<V3>)> = polys.concat();
    let sign = if forward { 1.0 } else { -1.0 };
    let positions: Vec<V3> = flat.iter().map(|&(t, v, q)| q.unwrap_or_else(|| lift(t, v))).collect();
    let normals: Vec<V3> = flat.iter().map(|&(t, _, _)| mul(normal(t), sign)).collect();
    let triangles = tris
        .into_iter()
        .filter_map(|[i, j, l]| {
            let (pa, pb, pc) = (positions[i], positions[j], positions[l]);
            let g = cross(sub(pb, pa), sub(pc, pa));
            if len(g) < 1e-300 {
                return None;
            }
            let n = add(add(normals[i], normals[j]), normals[l]);
            Some(if dot(g, n) >= 0.0 { [i as u32, j as u32, l as u32] } else { [i as u32, l as u32, j as u32] })
        })
        .collect();
    Some(compact(FaceMesh { positions, normals, triangles, known: false, color: None }))
}

/// A point of a cone face in its parameter plane: θ, v, and the edge point it came from.
type Pt = (f64, f64, Option<V3>);

fn mean_theta(ring: &[Pt]) -> f64 {
    ring.iter().map(|p| p.0).sum::<f64>() / ring.len().max(1) as f64
}

fn extent(ring: &[Pt]) -> f64 {
    let (lo, hi) = ring.iter().fold((f64::MAX, f64::MIN), |(lo, hi), p| (lo.min(p.0), hi.max(p.0)));
    hi - lo
}

/// Moves a closed ring by whole turns so that its mean θ lies in [lo, lo + 2π).
fn shift_into(ring: &mut [Pt], lo: f64) {
    let turns = ((mean_theta(ring) - lo) / TAU).floor() * TAU;
    for p in ring {
        p.0 -= turns;
    }
}

/// A θ for the seam of the band: one of the points of the loop `ring`, outside the θ range of
/// every closed loop (holes) if there is such a point.
fn seam_angle(ring: &[Pt], closed: &[Vec<Pt>]) -> f64 {
    let ranges: Vec<(f64, f64)> = closed.iter().map(|r| r.iter().fold((f64::MAX, f64::MIN), |(lo, hi), p| (lo.min(p.0), hi.max(p.0)))).collect();
    let covered = |t: f64| ranges.iter().any(|&(lo, hi)| (t - lo).rem_euclid(TAU) <= hi - lo);
    ring.iter().map(|p| p.0).find(|&t| !covered(t)).unwrap_or(ring[0].0)
}

/// A loop around the axis (θ increasing) as one side of the band: in loop order from the point
/// nearest to θ = `start` (mod 2π), once around, ending on that point again 2π further.
fn band_side(ring: &[Pt], start: f64) -> Vec<Pt> {
    let n = ring.len();
    let gap = |t: f64| {
        let d = (t - start).rem_euclid(TAU);
        d.min(TAU - d)
    };
    let j = (0..n).min_by(|&a, &b| gap(ring[a].0).total_cmp(&gap(ring[b].0))).unwrap_or(0);
    let mut out: Vec<Pt> = Vec::with_capacity(n + 1);
    out.extend(ring[j..].iter().copied());
    out.extend(ring[..j].iter().map(|&(t, v, q)| (t + TAU, v, q)));
    out.push((ring[j].0 + TAU, ring[j].1, ring[j].2));
    // the start next to `start`
    let turns = ((out[0].0 - start) / TAU).round() * TAU;
    for p in &mut out {
        p.0 -= turns;
    }
    out
}

fn wrap(d: f64) -> f64 {
    let mut d = d % TAU;
    if d > std::f64::consts::PI {
        d -= TAU;
    } else if d < -std::f64::consts::PI {
        d += TAU;
    }
    d
}

/// Gives cone faces the orientation of their neighbours: along a shared edge, two consistently
/// oriented faces run in opposite directions. Returns the number of faces turned.
fn orient(meshes: &mut [FaceMesh]) -> usize {
    let key = |p: V3| p.map(f64::to_bits);
    // boundary edges of every face: undirected key → (face, runs from the smaller key)
    type Key = ([u64; 3], [u64; 3]);
    let mut edges: HashMap<Key, Vec<(usize, bool)>> = HashMap::new();
    for (f, m) in meshes.iter().enumerate() {
        let mut count: HashMap<(u32, u32), i32> = HashMap::new();
        for t in &m.triangles {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                *count.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        for t in &m.triangles {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                if count[&(a.min(b), a.max(b))] != 1 {
                    continue;
                }
                let (ka, kb) = (key(m.positions[a as usize]), key(m.positions[b as usize]));
                if ka == kb {
                    continue;
                }
                let e = if ka < kb { (ka, kb) } else { (kb, ka) };
                edges.entry(e).or_default().push((f, ka < kb));
            }
        }
    }
    // votes between face pairs: + consistent, − inconsistent
    let mut votes: HashMap<(usize, usize), i64> = HashMap::new();
    for list in edges.values() {
        if list.len() != 2 || list[0].0 == list[1].0 {
            continue;
        }
        let ((f, df), (g, dg)) = (list[0], list[1]);
        let v = if df != dg { 1 } else { -1 };
        *votes.entry((f.min(g), f.max(g))).or_default() += v;
    }
    let mut neighbours: HashMap<usize, Vec<(usize, i64)>> = HashMap::new();
    for (&(f, g), &v) in &votes {
        if v != 0 {
            neighbours.entry(f).or_default().push((g, v));
            neighbours.entry(g).or_default().push((f, v));
        }
    }
    // layer by layer outwards from the planar faces: each face takes the summed votes of its
    // neighbours decided before it
    let none: &[(usize, i64)] = &[];
    let mut state: Vec<Option<bool>> = meshes.iter().map(|m| m.known.then_some(false)).collect();
    let mut frontier: Vec<usize> = (0..meshes.len()).filter(|&f| meshes[f].known).collect();
    while !frontier.is_empty() {
        let mut next: Vec<usize> =
            frontier.iter().flat_map(|f| neighbours.get(f).map(Vec::as_slice).unwrap_or(none)).map(|&(g, _)| g).filter(|&g| state[g].is_none()).collect();
        next.sort_unstable();
        next.dedup();
        let decided: Vec<(usize, bool)> = next
            .iter()
            .map(|&g| {
                let sum: i64 = neighbours[&g].iter().filter_map(|&(f, v)| state[f].map(|flipped| if flipped { -v } else { v })).sum();
                (g, sum < 0)
            })
            .collect();
        for &(g, flip) in &decided {
            state[g] = Some(flip);
        }
        frontier = next;
    }
    let mut turned = 0;
    for (m, s) in meshes.iter_mut().zip(state) {
        if s == Some(true) {
            turned += 1;
            for t in &mut m.triangles {
                t.swap(1, 2);
            }
            for n in &mut m.normals {
                *n = mul(*n, -1.0);
            }
        }
    }
    turned
}
