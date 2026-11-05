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
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::f64::consts::TAU;

type V3 = [f64; 3];

fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn mul(a: V3, s: f64) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn len(a: V3) -> f64 {
    dot(a, a).sqrt()
}
fn unit(a: V3) -> Option<V3> {
    let l = len(a);
    (l > 1e-300 && l.is_finite()).then(|| mul(a, 1.0 / l))
}
fn dist(a: V3, b: V3) -> f64 {
    len(sub(a, b))
}

/// Options for tessellation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Options {
    /// Segments of a full circle (edges and cylinders); 48 = 7.5°.
    pub segments_per_circle: u32,
}

impl Default for Options {
    fn default() -> Self {
        Self { segments_per_circle: 48 }
    }
}

/// A triangulated face, in model units.
#[derive(Debug, Clone, Default)]
pub struct FaceMesh {
    pub positions: Vec<V3>,
    pub normals: Vec<V3>,
    pub triangles: Vec<[u32; 3]>,
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
    /// Faces whose records or loops could not be followed or triangulated.
    pub failed: usize,
    /// Cone faces turned to match their planar neighbours.
    pub reoriented: usize,
    /// Bodies with a transform record (not specified, ignored).
    pub transforms_ignored: usize,
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
        self.failed += o.failed;
        self.reoriented += o.reoriented;
        self.transforms_ignored += o.transforms_ignored;
    }

    pub fn skipped(&self) -> usize {
        self.unsupported_surface.values().sum::<usize>() + self.unsupported_curve.values().sum::<usize>() + self.failed
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
        if self.failed > 0 {
            out.push(format!("Inventor: {} faces skipped (could not be tessellated)", self.failed));
        }
        if self.transforms_ignored > 0 {
            out.push(format!("Inventor: {} bodies have a transform, which is not read yet (shown untransformed)", self.transforms_ignored));
        }
        out
    }
}

enum Skip {
    Surface(String),
    Curve(String),
    Failed,
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

/// Tessellates one body: its faces with their triangles, and what was skipped.
pub fn body(b: &Brep, body: i32, opt: &Options) -> (Vec<FaceMesh>, Stats) {
    let mut stats = Stats::default();
    if b.body(body).is_some_and(|bd| bd.transform >= 0) {
        stats.transforms_ignored += 1;
    }
    let mut t = Tessellator { b, opt, edges: HashMap::new() };
    let mut meshes = Vec::new();
    for face in body_faces(b, body) {
        stats.faces += 1;
        // damaged input must not take the whole part down
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| t.face(face))).unwrap_or(Err(Skip::Failed));
        match result {
            Ok(m) if !m.triangles.is_empty() => {
                stats.tessellated += 1;
                meshes.push(m);
            }
            Ok(_) | Err(Skip::Failed) => stats.failed += 1,
            Err(Skip::Surface(s)) => *stats.unsupported_surface.entry(s).or_default() += 1,
            Err(Skip::Curve(c)) => *stats.unsupported_curve.entry(c).or_default() += 1,
        }
    }
    stats.reoriented = orient(&mut meshes);
    (meshes, stats)
}

struct Tessellator<'a> {
    b: &'a Brep,
    opt: &'a Options,
    /// Sampled points per edge, from its start vertex to its end vertex.
    edges: HashMap<i32, Result<Vec<V3>, String>>,
}

impl Tessellator<'_> {
    fn face(&mut self, i: i32) -> Result<FaceMesh, Skip> {
        let b = self.b;
        let face = b.face(i).ok_or(Skip::Failed)?;
        let surface = b.surface(face.surface).ok_or(Skip::Failed)?;
        if let Surface::Other(name) = &surface {
            return Err(Skip::Surface(name.clone()));
        }
        let mut loops = Vec::new();
        for l in chain(face.loop_, |i| b.loop_(i).map(|l| l.next)) {
            let lp = b.loop_(l).ok_or(Skip::Failed)?;
            let mut pts: Vec<V3> = Vec::new();
            let coedges = chain(lp.coedge, |i| b.coedge(i).map(|c| c.next));
            for c in coedges {
                let co = b.coedge(c).ok_or(Skip::Failed)?;
                let samples = self.edge(co.edge).map_err(Skip::Curve)?;
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
        if loops.is_empty() {
            return Err(Skip::Failed);
        }
        match surface {
            Surface::Plane { p, n, u } => plane(p, n, u, face.forward, &loops),
            Surface::Cone { c, a, m, ratio, sin, cos } => cone(c, a, m, ratio, sin, cos, face.forward, &loops),
            Surface::Other(_) => unreachable!(),
        }
        .ok_or(Skip::Failed)
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
        Some(Curve::Other(name)) => Err(name),
    }
}

/// Constrained Delaunay triangulation of rings in a plane; triangles inside an odd number of
/// rings are kept. Returns triangles as indices into the concatenated ring points.
fn triangulate(rings: &[Vec<[f64; 2]>]) -> Option<Vec<[usize; 3]>> {
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
            let h = cdt.insert(Point2::new(p[0], p[1])).ok()?;
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
    let tris = triangulate(&rings)?;
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
    Some(compact(FaceMesh { normals: vec![out; positions.len()], positions, triangles, known: true }))
}

/// Drops the vertices no triangle uses.
fn compact(m: FaceMesh) -> FaceMesh {
    let mut map = vec![u32::MAX; m.positions.len()];
    let mut out = FaceMesh { known: m.known, ..Default::default() };
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
    let mut around: Vec<Vec<(f64, f64, Option<V3>)>> = Vec::new();
    let mut closed: Vec<Vec<(f64, f64, Option<V3>)>> = Vec::new();
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
        let ring: Vec<(f64, f64, Option<V3>)> = th.iter().zip(&loc).zip(l).map(|((&t, s), &q)| (t, s.1, Some(q))).collect();
        let winding = match (th.first(), th.last()) {
            (Some(&first), Some(&last)) if th.len() > 1 => ((last - first + wrap(first - last)) / TAU).round() as i64,
            _ => 0,
        };
        if winding != 0 {
            around.push(ring);
        } else if ring.len() >= 3 {
            closed.push(ring);
        }
    }
    let mut polys = Vec::new();
    match around.len() {
        0 => polys = closed,
        1 | 2 => {
            let side_a = band_side(&around[0]);
            let side_b = if around.len() == 2 {
                band_side(&around[1])
            } else if k != 0.0 {
                // a cone closed by its apex
                let apex = -r0 / k;
                let p = add(c, mul(a, apex));
                vec![(0.0, apex, Some(p)), (TAU, apex, Some(p))]
            } else {
                return None;
            };
            let mut band = side_a;
            band.extend(side_b.into_iter().rev());
            polys.push(band);
            for mut ring in closed {
                let mean = ring.iter().map(|p| p.0).sum::<f64>() / ring.len() as f64;
                let shift = (mean / TAU).floor() * TAU;
                for p in &mut ring {
                    p.0 -= shift;
                }
                polys.push(ring);
            }
        }
        _ => return None,
    }
    let rings2d: Vec<Vec<[f64; 2]>> = polys.iter().map(|r| r.iter().map(|&(t, v, _)| [t * mean_r, v]).collect()).collect();
    let tris = triangulate(&rings2d)?;
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
    Some(compact(FaceMesh { positions, normals, triangles, known: false }))
}

/// A loop around the axis as one side of the band 0 ≤ θ ≤ 2π: its points by θ, with the
/// value at the seam (θ = 0 and 2π) interpolated across it.
fn band_side(ring: &[(f64, f64, Option<V3>)]) -> Vec<(f64, f64, Option<V3>)> {
    let mut pts: Vec<(f64, f64, Option<V3>)> = ring.iter().map(|&(t, v, q)| (t.rem_euclid(TAU), v, q)).collect();
    pts.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (first, last) = (pts[0], pts[pts.len() - 1]);
    let span = first.0 + TAU - last.0;
    let seam = if span > 1e-12 { last.1 + (first.1 - last.1) * (TAU - last.0) / span } else { first.1 };
    let mut out = Vec::with_capacity(pts.len() + 2);
    if first.0 > 1e-12 {
        out.push((0.0, seam, None));
    }
    out.extend(pts.iter().copied());
    out.push(if first.0 > 1e-12 { (TAU, seam, None) } else { (TAU, first.1, first.2) });
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
    let mut state: Vec<Option<bool>> = meshes.iter().map(|m| m.known.then_some(false)).collect();
    let mut queue: VecDeque<usize> = (0..meshes.len()).filter(|&f| meshes[f].known).collect();
    while let Some(f) = queue.pop_front() {
        let flipped = state[f].unwrap_or(false);
        for &(g, v) in neighbours.get(&f).map(Vec::as_slice).unwrap_or(&[]) {
            if state[g].is_none() {
                state[g] = Some(flipped ^ (v < 0));
                queue.push_back(g);
            }
        }
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
