//! Faces on spheres, tori and B-spline surfaces: surfaces with two parameters that may each
//! run around (the angle around a sphere or torus, a closed spline direction).
//!
//! The points of the loops are taken to the parameter plane, each loop followed point by point
//! so that it stays in one piece where the parameter starts again. A loop that comes back at
//! the same parameters bounds a region like on a plane. A loop that comes back one period
//! further runs around the surface: the face is then the band between two such loops, or
//! between one and the end of the surface (the pole of a sphere). Which side of a loop the face
//! is on follows from the direction of the loop: seen from outside, the face is on its left.
//! The region is triangulated in the parameter plane together with a grid of points inside it,
//! and lifted back onto the surface.

use crate::nurbs;
use crate::tess::{add, compact, cross, dist, dot, len, mul, sub, triangulate, unit, FaceMesh, V3};
use std::f64::consts::{FRAC_PI_2, TAU};

/// Points of the grid inside one face at most.
const MAX_GRID: usize = 40_000;

/// A surface with two parameters.
pub(crate) trait Patch {
    /// The period of u and of v; `None` for a direction that does not run around.
    fn period(&self) -> [Option<f64>; 2];
    /// The range of each parameter (one period for a direction that runs around).
    fn range(&self) -> [(f64, f64); 2];
    /// The parameters of a point on the surface; where they are not determined (a pole) those
    /// of `near` are kept.
    fn invert(&self, q: V3, near: Option<[f64; 2]>) -> Option<[f64; 2]>;
    fn eval(&self, uv: [f64; 2]) -> V3;
    /// The unit normal on the side of ∂/∂u × ∂/∂v.
    fn normal(&self, uv: [f64; 2]) -> Option<V3>;
    /// Roughly the length on the surface of one unit of u and of v.
    fn metric(&self) -> [f64; 2];
    /// The distance between grid points in u and in v.
    fn step(&self) -> [f64; 2];
    /// The surface is a single point at the low (0) or high (1) end of v.
    fn pole(&self, end: usize) -> bool;
}

/// A point in the parameter plane, and the edge point it came from.
#[derive(Debug, Clone, Copy)]
struct P {
    uv: [f64; 2],
    q: Option<V3>,
}

fn wrap(d: f64, period: f64) -> f64 {
    let d = d % period;
    if d > period / 2.0 {
        d - period
    } else if d < -period / 2.0 {
        d + period
    } else {
        d
    }
}

fn mean(ring: &[P], dir: usize) -> f64 {
    ring.iter().map(|p| p.uv[dir]).sum::<f64>() / ring.len().max(1) as f64
}

fn extent(ring: &[P], dir: usize) -> (f64, f64) {
    ring.iter().fold((f64::MAX, f64::MIN), |(lo, hi), p| (lo.min(p.uv[dir]), hi.max(p.uv[dir])))
}

/// Moves a ring by whole periods so that its mean lies in [lo, lo + period).
fn shift_into(ring: &mut [P], dir: usize, lo: f64, period: f64) {
    let turns = ((mean(ring, dir) - lo) / period).floor() * period;
    for p in ring {
        p.uv[dir] -= turns;
    }
}

/// A loop in the parameter plane and how often it runs around u and v.
struct Mapped {
    ring: Vec<P>,
    winding: [i64; 2],
}

fn map_loop(s: &dyn Patch, l: &[V3]) -> Option<Mapped> {
    let period = s.period();
    let mut ring: Vec<P> = Vec::with_capacity(l.len());
    for &q in l {
        let near = ring.last().map(|p: &P| p.uv);
        let mut uv = s.invert(q, near)?;
        if let Some(prev) = near {
            for d in 0..2 {
                if let Some(p) = period[d] {
                    uv[d] = prev[d] + wrap(uv[d] - prev[d], p);
                }
            }
        }
        ring.push(P { uv, q: Some(q) });
    }
    // a loop that starts on a pole: its first point takes the u it is left with
    if ring.len() > 1 {
        if let Some(first) = s.invert(l[0], Some(ring[1].uv)) {
            if let Some(p) = period[0] {
                ring[0].uv[0] = ring[1].uv[0] + wrap(first[0] - ring[1].uv[0], p);
            }
        }
    }
    let mut winding = [0i64; 2];
    if let (Some(a), Some(z)) = (ring.first(), ring.last()) {
        for d in 0..2 {
            if let Some(p) = period[d] {
                let back = z.uv[d] + wrap(a.uv[d] - z.uv[d], p);
                winding[d] = ((back - a.uv[d]) / p).round() as i64;
            }
        }
    }
    Some(Mapped { ring, winding })
}

/// A loop that runs around direction `d` once, as one side of a band: from its point nearest
/// to `start`, once around in increasing `d`, ending on that point again one period further.
fn band_side(ring: &[P], d: usize, start: f64, period: f64) -> Vec<P> {
    let n = ring.len();
    let gap = |t: f64| {
        let g = (t - start).rem_euclid(period);
        g.min(period - g)
    };
    let j = (0..n).min_by(|&a, &b| gap(ring[a].uv[d]).total_cmp(&gap(ring[b].uv[d]))).unwrap_or(0);
    let mut out: Vec<P> = Vec::with_capacity(n + 1);
    out.extend(ring[j..].iter().copied());
    let further = |p: &P| {
        let mut p = *p;
        p.uv[d] += period;
        p
    };
    out.extend(ring[..j].iter().map(further));
    out.push(further(&ring[j]));
    let turns = ((out[0].uv[d] - start) / period).round() * period;
    for p in &mut out {
        p.uv[d] -= turns;
    }
    out
}

/// Tessellates a face on `s`. `outward`: the outward side of the face is the side of the
/// surface normal (∂/∂u × ∂/∂v).
pub(crate) fn face(s: &dyn Patch, loops: &[Vec<V3>], outward: bool) -> Option<FaceMesh> {
    let period = s.period();
    let range = s.range();
    let mut mapped: Vec<Mapped> = Vec::new();
    for l in loops {
        if l.len() < 2 {
            continue;
        }
        mapped.push(map_loop(s, l)?);
    }
    if mapped.iter().any(|m| m.winding[0] != 0 && m.winding[1] != 0 || m.winding.iter().any(|w| w.abs() > 1)) {
        return None;
    }
    let around = [mapped.iter().filter(|m| m.winding[0] != 0).count(), mapped.iter().filter(|m| m.winding[1] != 0).count()];
    let mut polys: Vec<Vec<P>> = Vec::new();
    let step = s.step();
    match around {
        [0, 0] if mapped.is_empty() => {
            // the whole surface: every direction runs around or ends in a pole
            if period[0].is_none() || (period[1].is_none() && !(s.pole(0) && s.pole(1))) {
                return None;
            }
            let (u0, u1) = range[0];
            let (v0, v1) = range[1];
            polys.push(rectangle([u0, v0], [u1, v1], step));
        }
        [0, 0] => {
            // all loops near the widest one
            let widest = (0..mapped.len()).max_by(|&i, &j| {
                let w = |k: usize| (0..2).map(|d| extent(&mapped[k].ring, d)).map(|(lo, hi)| hi - lo).sum::<f64>();
                w(i).total_cmp(&w(j))
            })?;
            let centre = [mean(&mapped[widest].ring, 0), mean(&mapped[widest].ring, 1)];
            for m in mapped {
                let mut ring = m.ring;
                if ring.len() < 3 {
                    continue;
                }
                for d in 0..2 {
                    if let Some(p) = period[d] {
                        shift_into(&mut ring, d, centre[d] - p / 2.0, p);
                    }
                }
                polys.push(ring);
            }
        }
        [n, 0] | [0, n] => {
            let d = usize::from(around[1] != 0);
            let e = 1 - d;
            let p = period[d]?;
            if n > 2 {
                return None;
            }
            // seen from outside the face is on the left of its loops; in the parameter plane
            // that is the left when (u, v, outward) is right-handed
            let handed = if outward { 1.0 } else { -1.0 };
            let mut sides: Vec<(Vec<P>, f64)> = Vec::new();
            let mut closed: Vec<Vec<P>> = Vec::new();
            for m in mapped {
                let w = m.winding[d];
                if w == 0 {
                    if m.ring.len() >= 3 {
                        closed.push(m.ring);
                    }
                    continue;
                }
                let mut ring = m.ring;
                if w < 0 {
                    ring.reverse();
                }
                // running in +u the left is +v; running in +v the left is −u
                let left = if d == 0 { 1.0 } else { -1.0 };
                sides.push((ring, left * w as f64 * handed));
            }
            // the seam of the band: at a point of the first loop that no closed loop spans
            let spans: Vec<(f64, f64)> = closed.iter().map(|r| extent(r, d)).collect();
            let covered = |t: f64| spans.iter().any(|&(lo, hi)| (t - lo).rem_euclid(p) <= hi - lo);
            let start = sides[0].0.iter().map(|q| q.uv[d]).find(|&t| !covered(t)).unwrap_or(sides[0].0[0].uv[d]);
            let low_first = |a: &[P], b: &[P]| mean(a, e) <= mean(b, e);
            let (low, high): (Vec<P>, Vec<P>) = if sides.len() == 2 {
                let (b, a) = (sides.pop()?, sides.pop()?);
                let (mut low, mut high) = match period[e] {
                    // around the other direction too: from the loop with the face above it up
                    // to the other one
                    Some(_) if a.1 != b.1 => {
                        if a.1 > 0.0 {
                            (a.0, b.0)
                        } else {
                            (b.0, a.0)
                        }
                    }
                    // loops that do not say: the narrower of the two bands between them
                    Some(pe) if (mean(&b.0, e) - mean(&a.0, e)).rem_euclid(pe) <= pe / 2.0 => (a.0, b.0),
                    Some(_) => (b.0, a.0),
                    None if low_first(&a.0, &b.0) => (a.0, b.0),
                    None => (b.0, a.0),
                };
                low = band_side(&low, d, start, p);
                high = band_side(&high, d, low[0].uv[d], p);
                if let Some(pe) = period[e] {
                    let lift = ((mean(&low, e) - mean(&high, e)) / pe).ceil() * pe;
                    for q in &mut high {
                        q.uv[e] += lift;
                    }
                }
                (low, high)
            } else {
                // one loop: the face runs from it to an end of the surface
                if period[e].is_some() {
                    return None;
                }
                let (ring, side) = sides.pop()?;
                let ring = band_side(&ring, d, start, p);
                let mut up = side > 0.0;
                if !s.pole(usize::from(up)) && s.pole(usize::from(!up)) {
                    up = !up;
                }
                let end = if up { range[e].1 } else { range[e].0 };
                let from = ring[0].uv[d];
                let n = ((p / step[d]).ceil() as usize).clamp(2, 512);
                let edge: Vec<P> = (0..=n)
                    .map(|k| {
                        let mut uv = [0.0; 2];
                        uv[d] = from + p * k as f64 / n as f64;
                        uv[e] = end;
                        P { uv, q: None }
                    })
                    .collect();
                if up {
                    (ring, edge)
                } else {
                    (edge, ring)
                }
            };
            let lo = low[0].uv[d];
            let centre_e = (mean(&low, e) + mean(&high, e)) / 2.0;
            // the seam on either side of the band, with points along it: the same points on
            // the surface from both sides
            let between = |from: &P, to: &P| -> Vec<P> {
                let n = (((to.uv[e] - from.uv[e]).abs() / step[e]).ceil() as usize).clamp(1, 512);
                (1..n)
                    .map(|k| k as f64 / n as f64)
                    .map(|t| P { uv: [from.uv[0] + (to.uv[0] - from.uv[0]) * t, from.uv[1] + (to.uv[1] - from.uv[1]) * t], q: None })
                    .collect()
            };
            let up = between(low.last()?, high.last()?);
            let mut down = between(low.first()?, high.first()?);
            down.reverse();
            let mut band = low;
            band.extend(up);
            band.extend(high.into_iter().rev());
            band.extend(down);
            polys.push(band);
            for mut ring in closed {
                shift_into(&mut ring, d, lo, p);
                if let Some(pe) = period[e] {
                    shift_into(&mut ring, e, centre_e - pe / 2.0, pe);
                }
                polys.push(ring);
            }
        }
        _ => return None,
    }
    if polys.is_empty() {
        return None;
    }

    let metric = s.metric();
    let to2d = |p: &P| [p.uv[0] * metric[0], p.uv[1] * metric[1]];
    let rings: Vec<Vec<[f64; 2]>> = polys.iter().map(|r| r.iter().map(to2d).collect()).collect();
    let mut flat: Vec<P> = polys.concat();
    let grid = grid(&polys, &rings, step, metric);
    let grid2d: Vec<[f64; 2]> = grid.iter().map(to2d).collect();
    let tris = triangulate(&rings, &grid2d)?;
    flat.extend(grid);

    let sign = if outward { 1.0 } else { -1.0 };
    let positions: Vec<V3> = flat.iter().map(|p| p.q.unwrap_or_else(|| s.eval(p.uv))).collect();
    let mut normals: Vec<Option<V3>> = flat.iter().map(|p| s.normal(p.uv).map(|n| mul(n, sign))).collect();
    let triangles: Vec<[u32; 3]> = tris
        .into_iter()
        .filter_map(|[i, j, l]| {
            let g = cross(sub(positions[j], positions[i]), sub(positions[l], positions[i]));
            let size = dist(positions[i], positions[j]).max(dist(positions[i], positions[l]));
            if !(len(g) > 1e-12 * size * size) {
                return None;
            }
            let n = [i, j, l].iter().filter_map(|&k| normals[k]).fold([0.0; 3], add);
            Some(if dot(g, n) >= 0.0 { [i as u32, j as u32, l as u32] } else { [i as u32, l as u32, j as u32] })
        })
        .collect();
    // where the surface has no normal of its own (a pole of a spline): that of its triangles
    if normals.iter().any(Option::is_none) {
        let mut sums = vec![[0.0f64; 3]; positions.len()];
        for t in &triangles {
            let g = cross(sub(positions[t[1] as usize], positions[t[0] as usize]), sub(positions[t[2] as usize], positions[t[0] as usize]));
            for &k in t {
                sums[k as usize] = add(sums[k as usize], g);
            }
        }
        for (n, sum) in normals.iter_mut().zip(sums) {
            if n.is_none() {
                *n = unit(sum);
            }
        }
    }
    let normals: Vec<V3> = normals.into_iter().map(|n| n.unwrap_or([0.0, 0.0, 1.0])).collect();
    Some(compact(FaceMesh { positions, normals, triangles, known: false, color: None }))
}

/// The rectangle from `a` to `b` in the parameter plane with points along its sides.
fn rectangle(a: [f64; 2], b: [f64; 2], step: [f64; 2]) -> Vec<P> {
    let n = [0, 1].map(|d| (((b[d] - a[d]) / step[d]).ceil() as usize).clamp(1, 512));
    let at = |i: usize, j: usize| P { uv: [a[0] + (b[0] - a[0]) * i as f64 / n[0] as f64, a[1] + (b[1] - a[1]) * j as f64 / n[1] as f64], q: None };
    let mut out = Vec::new();
    out.extend((0..n[0]).map(|i| at(i, 0)));
    out.extend((0..n[1]).map(|j| at(n[0], j)));
    out.extend((0..n[0]).map(|i| at(n[0] - i, n[1])));
    out.extend((0..n[1]).map(|j| at(0, n[1] - j)));
    out
}

/// Grid points inside the rings (an odd number of them) and away from their edges.
fn grid(polys: &[Vec<P>], rings: &[Vec<[f64; 2]>], step: [f64; 2], metric: [f64; 2]) -> Vec<P> {
    let mut lo = [f64::MAX; 2];
    let mut hi = [f64::MIN; 2];
    for p in polys.iter().flatten() {
        for d in 0..2 {
            lo[d] = lo[d].min(p.uv[d]);
            hi[d] = hi[d].max(p.uv[d]);
        }
    }
    let mut step = step;
    let mut n = [0usize; 2];
    loop {
        for d in 0..2 {
            if !(step[d] > 0.0) || !(hi[d] - lo[d]).is_finite() {
                return Vec::new();
            }
            n[d] = ((hi[d] - lo[d]) / step[d]).ceil().max(1.0) as usize;
        }
        if n[0].saturating_mul(n[1]) <= MAX_GRID {
            break;
        }
        step = [step[0] * 1.5, step[1] * 1.5];
    }
    // a grid point this near to an edge would give a sliver
    let clear = 0.3 * (step[0] * metric[0]).min(step[1] * metric[1]);
    let segments: Vec<([f64; 2], [f64; 2])> = rings.iter().flat_map(|r| (0..r.len()).map(move |k| (r[k], r[(k + 1) % r.len()]))).collect();
    let mut out = Vec::new();
    for i in 1..n[0] {
        for j in 1..n[1] {
            let uv = [lo[0] + (hi[0] - lo[0]) * i as f64 / n[0] as f64, lo[1] + (hi[1] - lo[1]) * j as f64 / n[1] as f64];
            let p = [uv[0] * metric[0], uv[1] * metric[1]];
            let mut inside = false;
            let mut near = false;
            for &(a, b) in &segments {
                if (a[1] > p[1]) != (b[1] > p[1]) && p[0] < a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]) {
                    inside = !inside;
                }
                let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                let l2 = dx * dx + dy * dy;
                let t = if l2 > 0.0 { (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
                if (p[0] - a[0] - t * dx).hypot(p[1] - a[1] - t * dy) < clear {
                    near = true;
                    break;
                }
            }
            if inside && !near {
                out.push(P { uv, q: None });
            }
        }
    }
    out
}

/// A sphere with its poles on an axis of our choosing.
pub(crate) struct Sphere {
    c: V3,
    r: f64,
    x: V3,
    y: V3,
    a: V3,
    step: f64,
}

impl Sphere {
    /// `pole` and `x` as the surface has them; when a loop comes near a pole another axis is
    /// taken, as far from the loops as can be found.
    pub fn new(c: V3, r: f64, x: V3, pole: V3, loops: &[Vec<V3>], segments: u32) -> Option<Self> {
        let r = r.abs();
        let a0 = unit(pole)?;
        let x0 = unit(sub(x, mul(a0, dot(x, a0)))).or_else(|| unit(cross(a0, if a0[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] })))?;
        let y0 = cross(a0, x0);
        if !(r > 0.0) || !r.is_finite() {
            return None;
        }
        // how far (as a sine) the loops stay from the poles of an axis
        let clear = |a: V3| loops.iter().flatten().map(|&q| unit(sub(q, c)).map(|d| len(cross(d, a))).unwrap_or(1.0)).fold(1.0f64, f64::min);
        let mut candidates = vec![a0, x0, y0];
        for sx in [1.0, -1.0] {
            for sy in [1.0, -1.0] {
                candidates.extend(unit(add(add(mul(x0, sx), mul(y0, sy)), a0)));
            }
        }
        let mut best = (a0, clear(a0));
        if best.1 < 0.2 {
            for &a in &candidates[1..] {
                let k = clear(a);
                if k > best.1 {
                    best = (a, k);
                }
            }
        }
        let a = best.0;
        let x = unit(cross(if a[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] }, a))?;
        let (x, y) = if a == a0 { (x0, y0) } else { (x, cross(a, x)) };
        Some(Self { c, r, x, y, a, step: TAU / f64::from(segments.max(8)) })
    }
}

impl Patch for Sphere {
    fn period(&self) -> [Option<f64>; 2] {
        [Some(TAU), None]
    }
    fn range(&self) -> [(f64, f64); 2] {
        [(0.0, TAU), (-FRAC_PI_2, FRAC_PI_2)]
    }
    fn invert(&self, q: V3, near: Option<[f64; 2]>) -> Option<[f64; 2]> {
        let d = unit(sub(q, self.c))?;
        let (px, py, pz) = (dot(d, self.x), dot(d, self.y), dot(d, self.a));
        let v = pz.clamp(-1.0, 1.0).asin();
        let u = if px.hypot(py) < 1e-9 { near.map(|n| n[0]).unwrap_or(0.0) } else { py.atan2(px) };
        Some([u, v])
    }
    fn eval(&self, uv: [f64; 2]) -> V3 {
        add(self.c, mul(self.normal(uv).unwrap_or(self.a), self.r))
    }
    fn normal(&self, uv: [f64; 2]) -> Option<V3> {
        let (u, v) = (uv[0], uv[1]);
        Some(add(mul(add(mul(self.x, u.cos()), mul(self.y, u.sin())), v.cos()), mul(self.a, v.sin())))
    }
    fn metric(&self) -> [f64; 2] {
        [self.r, self.r]
    }
    fn step(&self) -> [f64; 2] {
        [self.step, self.step]
    }
    fn pole(&self, _end: usize) -> bool {
        true
    }
}

pub(crate) struct Torus {
    c: V3,
    a: V3,
    x: V3,
    y: V3,
    major: f64,
    minor: f64,
    step: f64,
}

impl Torus {
    pub fn new(c: V3, a: V3, major: f64, minor: f64, x: V3, segments: u32) -> Option<Self> {
        let a = unit(a)?;
        let x = unit(sub(x, mul(a, dot(x, a)))).or_else(|| unit(cross(a, if a[0].abs() < 0.9 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] })))?;
        let (major, minor) = (major.abs(), minor.abs());
        // also a tube wider than its ring (the surface then crosses itself on the axis): a face
        // lies on the part outside or the part inside, and its points say which
        if !(minor > 0.0) || !major.is_finite() || !minor.is_finite() {
            return None;
        }
        Some(Self { c, a, x, y: cross(a, x), major, minor, step: TAU / f64::from(segments.max(8)) })
    }

    fn radial(&self, u: f64) -> V3 {
        add(mul(self.x, u.cos()), mul(self.y, u.sin()))
    }
}

impl Patch for Torus {
    fn period(&self) -> [Option<f64>; 2] {
        [Some(TAU), Some(TAU)]
    }
    fn range(&self) -> [(f64, f64); 2] {
        [(0.0, TAU), (0.0, TAU)]
    }
    fn invert(&self, q: V3, _near: Option<[f64; 2]>) -> Option<[f64; 2]> {
        let d = sub(q, self.c);
        let (px, py, pz) = (dot(d, self.x), dot(d, self.y), dot(d, self.a));
        Some([py.atan2(px), pz.atan2(px.hypot(py) - self.major)])
    }
    fn eval(&self, uv: [f64; 2]) -> V3 {
        add(add(self.c, mul(self.radial(uv[0]), self.major)), mul(self.normal(uv).unwrap_or(self.a), self.minor))
    }
    fn normal(&self, uv: [f64; 2]) -> Option<V3> {
        Some(add(mul(self.radial(uv[0]), uv[1].cos()), mul(self.a, uv[1].sin())))
    }
    fn metric(&self) -> [f64; 2] {
        [self.major.max(self.minor), self.minor]
    }
    fn step(&self) -> [f64; 2] {
        [self.step, self.step]
    }
    fn pole(&self, _end: usize) -> bool {
        false
    }
}

/// A B-spline surface with a grid of its points to start looking for a point from.
pub(crate) struct Spline<'a> {
    s: &'a nurbs::Surface,
    range: [(f64, f64); 2],
    /// Samples per direction and the points, u running fastest.
    samples: [usize; 2],
    grid: Vec<V3>,
    metric: [f64; 2],
    size: f64,
    poles: [bool; 2],
}

impl<'a> Spline<'a> {
    pub fn new(s: &'a nurbs::Surface) -> Option<Self> {
        let range = [s.range(0), s.range(1)];
        if range.iter().any(|r| !(r.1 > r.0) || !r.0.is_finite() || !r.1.is_finite()) {
            return None;
        }
        let samples = [0, 1].map(|d| (3 * (s.distinct_knots(d).len().max(2) - 1) + 1).clamp(9, 96));
        let at = |i: usize, j: usize| {
            [
                range[0].0 + (range[0].1 - range[0].0) * i as f64 / (samples[0] - 1) as f64,
                range[1].0 + (range[1].1 - range[1].0) * j as f64 / (samples[1] - 1) as f64,
            ]
        };
        let mut grid = Vec::with_capacity(samples[0] * samples[1]);
        for j in 0..samples[1] {
            for i in 0..samples[0] {
                grid.push(s.eval(at(i, j)[0], at(i, j)[1]));
            }
        }
        let g = |i: usize, j: usize| grid[j * samples[0] + i];
        // the mean length of a line of the grid in u and in v
        let mut lengths = [0.0f64; 2];
        for j in 0..samples[1] {
            lengths[0] += (1..samples[0]).map(|i| dist(g(i - 1, j), g(i, j))).sum::<f64>();
        }
        for i in 0..samples[0] {
            lengths[1] += (1..samples[1]).map(|j| dist(g(i, j - 1), g(i, j))).sum::<f64>();
        }
        lengths[0] /= samples[1] as f64;
        lengths[1] /= samples[0] as f64;
        let size = lengths[0].max(lengths[1]);
        if !(size > 0.0) || !size.is_finite() {
            return None;
        }
        let metric = [0, 1].map(|d| (lengths[d] / (range[d].1 - range[d].0)).max(1e-9 * size / (range[d].1 - range[d].0)));
        let row = |j: usize| (1..samples[0]).map(|i| dist(g(0, j), g(i, j))).fold(0.0f64, f64::max);
        let poles = [row(0) < 1e-7 * size, row(samples[1] - 1) < 1e-7 * size];
        Some(Self { s, range, samples, grid, metric, size, poles })
    }

    /// Brings (u, v) to the surface point nearest to `q`, starting from `uv`; the distance left.
    fn refine(&self, q: V3, mut uv: [f64; 2]) -> ([f64; 2], f64) {
        let period = self.period();
        let h = [0, 1].map(|d| 1e-6 * (self.range[d].1 - self.range[d].0));
        let hold = |uv: &mut [f64; 2]| {
            for d in 0..2 {
                if period[d].is_none() {
                    uv[d] = uv[d].clamp(self.range[d].0, self.range[d].1);
                }
            }
        };
        hold(&mut uv);
        let mut best = (uv, dist(self.eval(uv), q));
        for _ in 0..24 {
            let p = self.eval(uv);
            let r = sub(p, q);
            let du = mul(sub(self.eval([uv[0] + h[0], uv[1]]), self.eval([uv[0] - h[0], uv[1]])), 0.5 / h[0]);
            let dv = mul(sub(self.eval([uv[0], uv[1] + h[1]]), self.eval([uv[0], uv[1] - h[1]])), 0.5 / h[1]);
            let (a, b, c) = (dot(du, du), dot(du, dv), dot(dv, dv));
            let (e, f) = (-dot(du, r), -dot(dv, r));
            let det = a * c - b * b;
            let delta = if det.abs() > 1e-14 * a.max(c).powi(2).max(1e-300) {
                [(e * c - f * b) / det, (a * f - b * e) / det]
            } else if a > c && a > 0.0 {
                [e / a, 0.0]
            } else if c > 0.0 {
                [0.0, f / c]
            } else {
                break;
            };
            // no further than a quarter of the range in one step
            let k = (0..2).map(|d| delta[d].abs() / (0.25 * (self.range[d].1 - self.range[d].0))).fold(1.0f64, f64::max);
            uv = [uv[0] + delta[0] / k, uv[1] + delta[1] / k];
            hold(&mut uv);
            let left = dist(self.eval(uv), q);
            if left < best.1 {
                best = (uv, left);
            }
            if delta[0].abs() < 1e-10 * (self.range[0].1 - self.range[0].0) && delta[1].abs() < 1e-10 * (self.range[1].1 - self.range[1].0) {
                break;
            }
        }
        best
    }
}

impl Patch for Spline<'_> {
    fn period(&self) -> [Option<f64>; 2] {
        [0, 1].map(|d| self.s.closure[d].wraps().then_some(self.range[d].1 - self.range[d].0))
    }
    fn range(&self) -> [(f64, f64); 2] {
        self.range
    }
    fn invert(&self, q: V3, near: Option<[f64; 2]>) -> Option<[f64; 2]> {
        let good = 1e-5 * self.size;
        if let Some(n) = near {
            let (uv, left) = self.refine(q, n);
            if left <= good {
                return Some(uv);
            }
        }
        // from the nearest points of the grid
        let mut order: Vec<(usize, f64)> = self.grid.iter().enumerate().map(|(k, &p)| (k, dist(p, q))).collect();
        order.sort_by(|a, b| a.1.total_cmp(&b.1));
        let mut best: Option<([f64; 2], f64)> = None;
        for &(k, _) in order.iter().take(4) {
            let (i, j) = (k % self.samples[0], k / self.samples[0]);
            let start = [
                self.range[0].0 + (self.range[0].1 - self.range[0].0) * i as f64 / (self.samples[0] - 1) as f64,
                self.range[1].0 + (self.range[1].1 - self.range[1].0) * j as f64 / (self.samples[1] - 1) as f64,
            ];
            let found = self.refine(q, start);
            if best.is_none_or(|b| found.1 < b.1) {
                best = Some(found);
            }
            if found.1 <= good {
                break;
            }
        }
        // an edge point further off than this is not on this surface
        best.filter(|b| b.1 <= 0.02 * self.size).map(|b| b.0)
    }
    fn eval(&self, uv: [f64; 2]) -> V3 {
        self.s.eval(uv[0], uv[1])
    }
    fn normal(&self, uv: [f64; 2]) -> Option<V3> {
        let h = [0, 1].map(|d| 1e-5 * (self.range[d].1 - self.range[d].0));
        // one-sided at the ends of a direction that does not run around
        let period = self.period();
        let ends = |d: usize| {
            let (mut lo, mut hi) = (uv[d] - h[d], uv[d] + h[d]);
            if period[d].is_none() {
                lo = lo.max(self.range[d].0);
                hi = hi.min(self.range[d].1);
            }
            (lo, hi)
        };
        let (u0, u1) = ends(0);
        let (v0, v1) = ends(1);
        let du = sub(self.eval([u1, uv[1]]), self.eval([u0, uv[1]]));
        let dv = sub(self.eval([uv[0], v1]), self.eval([uv[0], v0]));
        let n = cross(du, dv);
        (len(n) > 1e-9 * len(du) * len(dv) && len(n) > 0.0).then(|| unit(n)).flatten()
    }
    fn metric(&self) -> [f64; 2] {
        self.metric
    }
    fn step(&self) -> [f64; 2] {
        // a few points per stretch between knots, where the surface is curved
        [0, 1].map(|d| {
            let spans = (self.s.distinct_knots(d).len().max(2) - 1) as f64;
            let per = if self.s.degree[d] == 1 { 1.0 } else { 4.0 };
            (self.range[d].1 - self.range[d].0) / (spans * per).clamp(per, 96.0)
        })
    }
    fn pole(&self, end: usize) -> bool {
        self.poles[end]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn circle(c: V3, x: V3, y: V3, r: f64, n: usize) -> Vec<V3> {
        (0..n).map(|k| TAU * k as f64 / n as f64).map(|t| add(c, add(mul(x, r * t.cos()), mul(y, r * t.sin())))).collect()
    }

    fn area(m: &FaceMesh) -> f64 {
        m.triangles
            .iter()
            .map(|t| 0.5 * len(cross(sub(m.positions[t[1] as usize], m.positions[t[0] as usize]), sub(m.positions[t[2] as usize], m.positions[t[0] as usize]))))
            .sum()
    }

    /// Triangles face the side of their normals, and the normals point away from `c`.
    fn faces_away_from(m: &FaceMesh, c: impl Fn(V3) -> V3) -> bool {
        m.triangles.iter().all(|t| {
            let [p, q, r] = t.map(|k| m.positions[k as usize]);
            let g = cross(sub(q, p), sub(r, p));
            let centre = mul(add(add(p, q), r), 1.0 / 3.0);
            dot(g, sub(centre, c(centre))) > 0.0 && t.iter().all(|&k| dot(m.normals[k as usize], g) > 0.0)
        })
    }

    const X: V3 = [1.0, 0.0, 0.0];
    const Y: V3 = [0.0, 1.0, 0.0];
    const Z: V3 = [0.0, 0.0, 1.0];

    #[test]
    fn whole_sphere_and_caps() {
        let s = Sphere::new([1.0, 2.0, 3.0], 2.0, X, Z, &[], 48).unwrap();
        let m = face(&s, &[], true).unwrap();
        let exact = 4.0 * std::f64::consts::PI * 4.0;
        assert!((area(&m) / exact - 1.0).abs() < 0.01, "{}", area(&m));
        assert!(faces_away_from(&m, |_| [1.0, 2.0, 3.0]));

        // the cap above z = 1 of a unit sphere: its loop runs counter-clockwise seen from above
        let h = 0.5f64;
        let ring = circle([0.0, 0.0, h], X, Y, (1.0 - h * h).sqrt(), 48);
        let s = Sphere::new([0.0; 3], 1.0, X, Z, std::slice::from_ref(&ring), 48).unwrap();
        let cap = face(&s, std::slice::from_ref(&ring), true).unwrap();
        assert!((area(&cap) / (TAU * (1.0 - h)) - 1.0).abs() < 0.01, "{}", area(&cap));
        assert!(cap.positions.iter().all(|p| p[2] > h - 1e-9));
        // the same loop the other way round bounds the rest of the sphere
        let rev: Vec<V3> = ring.iter().rev().copied().collect();
        let rest = face(&s, std::slice::from_ref(&rev), true).unwrap();
        assert!((area(&rest) / (TAU * (1.0 + h)) - 1.0).abs() < 0.01, "{}", area(&rest));
        assert!(faces_away_from(&rest, |_| [0.0; 3]));

        // a loop through the pole of the surface: another axis is taken
        let ring = circle([h, 0.0, 0.0], Y, Z, (1.0 - h * h).sqrt(), 48);
        let s = Sphere::new([0.0; 3], 1.0, X, Z, std::slice::from_ref(&ring), 48).unwrap();
        let cap = face(&s, std::slice::from_ref(&ring), true).unwrap();
        assert!((area(&cap) / (TAU * (1.0 - h)) - 1.0).abs() < 0.01, "{}", area(&cap));
        assert!(cap.positions.iter().all(|p| p[0] > h - 1e-9));
    }

    #[test]
    fn torus_whole_band_and_patch() {
        let (big, small) = (3.0, 0.5);
        let t = Torus::new([0.0; 3], Z, big, small, X, 48).unwrap();
        let whole = face(&t, &[], true).unwrap();
        let exact = TAU * big * TAU * small;
        assert!((area(&whole) / exact - 1.0).abs() < 0.01, "{}", area(&whole));
        let tube = |p: V3| mul(unit([p[0], p[1], 0.0]).unwrap(), big);
        assert!(faces_away_from(&whole, tube));

        // the outer quarter above the equator: between the circle on top (radius 3, z = 0.5)
        // and the outer equator (radius 3.5, z = 0); seen from outside the face is on the
        // left of both loops
        let top: Vec<V3> = circle([0.0, 0.0, small], X, Y, big, 48).into_iter().rev().collect();
        let outer = circle([0.0; 3], X, Y, big + small, 48);
        let band = face(&t, &[top.clone(), outer.clone()], true).unwrap();
        // the area of a torus between tube angles 0 and 90°: 2π r (R φ + r sin φ)
        let exact = TAU * small * (big * FRAC_PI_2 + small);
        assert!((area(&band) / exact - 1.0).abs() < 0.01, "{} {exact}", area(&band));
        assert!(band.positions.iter().all(|p| p[2] > -1e-9 && p[0].hypot(p[1]) > big - 1e-9));
        assert!(faces_away_from(&band, tube));
        // the loops the other way round: the other three quarters of the tube
        let rest = face(&t, &[top.into_iter().rev().collect(), outer.into_iter().rev().collect()], true).unwrap();
        assert!((area(&rest) / (TAU * big * TAU * small - exact) - 1.0).abs() < 0.01, "{}", area(&rest));

        // a patch: a quarter of the way around the axis, the outer upper quarter of the tube
        let at = |u: f64, v: f64| t.eval([u, v]);
        let mut ring: Vec<V3> = Vec::new();
        let n = 12;
        ring.extend((0..n).map(|k| at(FRAC_PI_2 * k as f64 / n as f64, 0.0)));
        ring.extend((0..n).map(|k| at(FRAC_PI_2, FRAC_PI_2 * k as f64 / n as f64)));
        ring.extend((0..n).map(|k| at(FRAC_PI_2 * (n - k) as f64 / n as f64, FRAC_PI_2)));
        ring.extend((0..n).map(|k| at(0.0, FRAC_PI_2 * (n - k) as f64 / n as f64)));
        let patch = face(&t, &[ring], true).unwrap();
        assert!((area(&patch) / (exact / 4.0) - 1.0).abs() < 0.01, "{}", area(&patch));
        assert!(faces_away_from(&patch, tube));
    }

    #[test]
    fn spline_patch_with_a_hole() {
        use crate::brep::Tok;
        // a bilinear sheet over [0, 4] × [0, 2], raised to z = 1 along one side
        let mut t = vec![Tok::Id("nubs".into()), Tok::Int(1), Tok::Int(1), Tok::Enum(0), Tok::Enum(0), Tok::Enum(0), Tok::Enum(0), Tok::Int(2), Tok::Int(2)];
        for _ in 0..2 {
            t.extend([Tok::Real(0.0), Tok::Int(1), Tok::Real(1.0), Tok::Int(1)]);
        }
        for p in [[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [0.0, 2.0, 1.0], [4.0, 2.0, 1.0]] {
            t.extend(p.map(Tok::Real));
        }
        let surface = nurbs::find_surface(&t).unwrap();
        let s = Spline::new(&surface).unwrap();
        assert_eq!(s.period(), [None, None]);
        let on = |x: f64, y: f64| [x, y, y / 2.0];
        let uv = s.invert(on(1.0, 1.5), None).unwrap();
        assert!((uv[0] - 0.25).abs() < 1e-6 && (uv[1] - 0.75).abs() < 1e-6, "{uv:?}");
        let outer = vec![on(0.0, 0.0), on(4.0, 0.0), on(4.0, 2.0), on(0.0, 2.0)];
        let hole = vec![on(1.0, 0.5), on(1.0, 1.5), on(3.0, 1.5), on(3.0, 0.5)];
        let m = face(&s, &[outer, hole], true).unwrap();
        let slope = (1.0f64 + 0.25).sqrt();
        assert!((area(&m) - (8.0 - 2.0) * slope).abs() < 1e-9, "{}", area(&m));
        // the normal of ∂/∂u × ∂/∂v: up
        assert!(m.normals.iter().all(|n| n[2] > 0.5));
        let turned = face(&s, &[vec![on(0.0, 0.0), on(4.0, 0.0), on(4.0, 2.0), on(0.0, 2.0)]], false).unwrap();
        assert!(turned.normals.iter().all(|n| n[2] < -0.5));
    }
}
