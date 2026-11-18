//! B-spline curves and surfaces as the B-rep stores them inside `intcurve-curve` and
//! `spline-surface` records, and their evaluation.
//!
//! A curve is `nubs` or `nurbs`, its degree, how it closes (0 open, 1 closed, 2 periodic), the
//! number of distinct knots, then each knot with its multiplicity and the control points
//! (x, y, z, and the weight for `nurbs`). A surface has two degrees, for `nurbs` a word saying
//! in which direction it is rational, four enumerations (how it closes and whether it is
//! singular, in u and in v), two knot counts, the knots in u and in v, and the control points
//! with u running fastest. The knot lists leave out one knot at either end: with it added the
//! number of control points is the number of knots minus the degree minus one.

use crate::brep::Tok;

type V3 = [f64; 3];

/// Bounds for damaged input.
const MAX_DEGREE: usize = 25;
const MAX_KNOTS: usize = 200_000;
const MAX_POINTS: usize = 4_000_000;

/// How a curve or one direction of a surface closes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Closure {
    Open,
    Closed,
    Periodic,
}

impl Closure {
    fn from_enum(v: i32) -> Self {
        match v {
            1 => Closure::Closed,
            2 => Closure::Periodic,
            _ => Closure::Open,
        }
    }

    /// The ends meet: a parameter outside the range continues at the other end.
    pub fn wraps(self) -> bool {
        self != Closure::Open
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Curve {
    pub degree: usize,
    pub closure: Closure,
    /// The full knot vector.
    pub knots: Vec<f64>,
    /// x, y, z, weight.
    pub points: Vec<[f64; 4]>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Surface {
    pub degree: [usize; 2],
    pub closure: [Closure; 2],
    /// The full knot vectors in u and in v.
    pub knots: [Vec<f64>; 2],
    /// Control points in u and in v.
    pub count: [usize; 2],
    /// x, y, z, weight; point (i, j) at `j * count[0] + i`.
    pub points: Vec<[f64; 4]>,
}

/// A reader over tokens.
struct Toks<'a> {
    t: &'a [Tok],
    i: usize,
}

impl Toks<'_> {
    fn int(&mut self) -> Option<i32> {
        let v = match self.t.get(self.i)? {
            Tok::Int(v) | Tok::Enum(v) => *v,
            _ => return None,
        };
        self.i += 1;
        Some(v)
    }

    fn enumeration(&mut self) -> Option<i32> {
        let Tok::Enum(v) = self.t.get(self.i)? else { return None };
        self.i += 1;
        Some(*v)
    }

    fn real(&mut self) -> Option<f64> {
        let v = match self.t.get(self.i)? {
            Tok::Real(v) => *v,
            Tok::Int(v) => f64::from(*v),
            _ => return None,
        };
        self.i += 1;
        v.is_finite().then_some(v)
    }

    /// `n` knots with their multiplicities, with the knot left out at either end added.
    fn knots(&mut self, n: usize, degree: usize) -> Option<Vec<f64>> {
        if n < 2 || n > MAX_KNOTS {
            return None;
        }
        let mut out = Vec::new();
        for _ in 0..n {
            let k = self.real()?;
            let m = usize::try_from(self.int()?).ok()?;
            if m == 0 || m > degree + 1 || out.len() + m > MAX_KNOTS || out.last().is_some_and(|&last| k < last) {
                return None;
            }
            if out.is_empty() {
                out.push(k);
            }
            out.extend(std::iter::repeat(k).take(m));
        }
        out.push(*out.last()?);
        (out.len() > 2 * (degree + 1) - 1).then_some(out)
    }

    fn points(&mut self, n: usize, rational: bool) -> Option<Vec<[f64; 4]>> {
        if n == 0 || n > MAX_POINTS {
            return None;
        }
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let p = [self.real()?, self.real()?, self.real()?, if rational { self.real()? } else { 1.0 }];
            if p[3] <= 0.0 {
                return None;
            }
            out.push(p);
        }
        Some(out)
    }
}

/// Whether the token is the start of a B-spline: `nubs` or `nurbs`.
fn rational(tok: &Tok) -> Option<bool> {
    match tok {
        Tok::Id(s) | Tok::Str(s) if s == "nubs" => Some(false),
        Tok::Id(s) | Tok::Str(s) if s == "nurbs" => Some(true),
        _ => None,
    }
}

/// A curve that starts at token `at` (`nubs` or `nurbs`).
pub fn curve_at(tokens: &[Tok], at: usize) -> Option<Curve> {
    let rational = rational(tokens.get(at)?)?;
    let mut r = Toks { t: tokens, i: at + 1 };
    let degree = usize::try_from(r.int()?).ok().filter(|d| (1..=MAX_DEGREE).contains(d))?;
    let closure = Closure::from_enum(r.enumeration()?);
    let n = usize::try_from(r.int()?).ok()?;
    let knots = r.knots(n, degree)?;
    let points = r.points(knots.len() - degree - 1, rational)?;
    Some(Curve { degree, closure, knots, points })
}

/// A surface that starts at token `at` (`nubs` or `nurbs`).
pub fn surface_at(tokens: &[Tok], at: usize) -> Option<Surface> {
    let rational = rational(tokens.get(at)?)?;
    let mut r = Toks { t: tokens, i: at + 1 };
    let du = usize::try_from(r.int()?).ok().filter(|d| (1..=MAX_DEGREE).contains(d))?;
    let dv = usize::try_from(r.int()?).ok().filter(|d| (1..=MAX_DEGREE).contains(d))?;
    if rational {
        // in which direction the surface is rational: `both`, `u`, `v`
        match r.t.get(r.i)? {
            Tok::Id(_) | Tok::Str(_) => r.i += 1,
            _ => return None,
        }
    }
    let closure = [Closure::from_enum(r.enumeration()?), Closure::from_enum(r.enumeration()?)];
    r.enumeration()?;
    r.enumeration()?;
    let (nu, nv) = (usize::try_from(r.int()?).ok()?, usize::try_from(r.int()?).ok()?);
    let ku = r.knots(nu, du)?;
    let kv = r.knots(nv, dv)?;
    let count = [ku.len() - du - 1, kv.len() - dv - 1];
    let points = r.points(count[0].checked_mul(count[1])?, rational)?;
    Some(Surface { degree: [du, dv], closure, knots: [ku, kv], count, points })
}

/// The curve of an intersection curve definition: the first B-spline directly among `tokens`
/// (not inside a nested `{ }`). The ones after it are curves in the parameter planes of the
/// surfaces, so a definition whose first one is absent (`nullbs`) has none.
pub fn find_curve(tokens: &[Tok]) -> Option<Curve> {
    let first = top_level(tokens).find(|&i| rational(&tokens[i]).is_some() || matches!(&tokens[i], Tok::Id(s) if s == "nullbs"))?;
    curve_at(tokens, first)
}

/// The first surface directly among `tokens` (not inside a nested `{ }`).
pub fn find_surface(tokens: &[Tok]) -> Option<Surface> {
    top_level(tokens).find_map(|i| surface_at(tokens, i))
}

/// The indices of the tokens outside nested `{ }`.
fn top_level(tokens: &[Tok]) -> impl Iterator<Item = usize> + '_ {
    let mut depth = 0usize;
    tokens.iter().enumerate().filter_map(move |(i, t)| {
        match t {
            Tok::Open => depth += 1,
            Tok::Close => depth = depth.saturating_sub(1),
            _ if depth == 0 => return Some(i),
            _ => {}
        }
        None
    })
}

/// The knot span of `t`: the index `k` with `knots[k] <= t < knots[k + 1]`, within the valid
/// spans of the degree.
fn span(knots: &[f64], degree: usize, t: f64) -> usize {
    let n = knots.len() - degree - 1;
    if t >= knots[n] {
        // the last span that has a length
        let mut k = n - 1;
        while k > degree && knots[k] >= knots[k + 1] {
            k -= 1;
        }
        return k;
    }
    let (mut lo, mut hi) = (degree, n);
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if t < knots[mid] {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    lo
}

/// The `degree + 1` basis functions that are not zero at `t` in span `k`.
fn basis(knots: &[f64], degree: usize, k: usize, t: f64, out: &mut [f64]) {
    let mut left = [0.0f64; MAX_DEGREE + 1];
    let mut right = [0.0f64; MAX_DEGREE + 1];
    out[0] = 1.0;
    for j in 1..=degree {
        left[j] = t - knots[k + 1 - j];
        right[j] = knots[k + j] - t;
        let mut saved = 0.0;
        for r in 0..j {
            let d = right[r + 1] + left[j - r];
            let temp = if d.abs() > 0.0 { out[r] / d } else { 0.0 };
            out[r] = saved + right[r + 1] * temp;
            saved = left[j - r] * temp;
        }
        out[j] = saved;
    }
}

/// `t` brought into the range of the knots: clamped, or continued at the other end.
fn into_range(knots: &[f64], degree: usize, closure: Closure, t: f64) -> f64 {
    let (lo, hi) = (knots[degree], knots[knots.len() - degree - 1]);
    if closure.wraps() && hi > lo {
        lo + (t - lo).rem_euclid(hi - lo)
    } else {
        t.clamp(lo, hi)
    }
}

impl Curve {
    /// The parameter range.
    pub fn range(&self) -> (f64, f64) {
        (self.knots[self.degree], self.knots[self.knots.len() - self.degree - 1])
    }

    pub fn eval(&self, t: f64) -> V3 {
        let t = into_range(&self.knots, self.degree, self.closure, t);
        let k = span(&self.knots, self.degree, t);
        let mut n = [0.0f64; MAX_DEGREE + 1];
        basis(&self.knots, self.degree, k, t, &mut n);
        let mut sum = [0.0f64; 4];
        for (j, nj) in n.iter().enumerate().take(self.degree + 1) {
            let p = self.points[k - self.degree + j];
            let w = nj * p[3];
            sum[0] += w * p[0];
            sum[1] += w * p[1];
            sum[2] += w * p[2];
            sum[3] += w;
        }
        let w = if sum[3].abs() > 1e-300 { sum[3] } else { 1.0 };
        [sum[0] / w, sum[1] / w, sum[2] / w]
    }

    /// The distinct knots inside the open range (a, b), ascending.
    pub fn knots_between(&self, a: f64, b: f64) -> Vec<f64> {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        let mut out: Vec<f64> = self.knots.iter().copied().filter(|&k| k > lo && k < hi).collect();
        out.dedup();
        out
    }
}

impl Surface {
    /// The parameter range in u (0) or v (1).
    pub fn range(&self, dir: usize) -> (f64, f64) {
        let (k, d) = (&self.knots[dir], self.degree[dir]);
        (k[d], k[k.len() - d - 1])
    }

    pub fn eval(&self, u: f64, v: f64) -> V3 {
        let u = into_range(&self.knots[0], self.degree[0], self.closure[0], u);
        let v = into_range(&self.knots[1], self.degree[1], self.closure[1], v);
        let (ku, kv) = (span(&self.knots[0], self.degree[0], u), span(&self.knots[1], self.degree[1], v));
        let mut nu = [0.0f64; MAX_DEGREE + 1];
        let mut nv = [0.0f64; MAX_DEGREE + 1];
        basis(&self.knots[0], self.degree[0], ku, u, &mut nu);
        basis(&self.knots[1], self.degree[1], kv, v, &mut nv);
        let mut sum = [0.0f64; 4];
        for (j, bv) in nv.iter().enumerate().take(self.degree[1] + 1) {
            let row = (kv - self.degree[1] + j) * self.count[0];
            for (i, bu) in nu.iter().enumerate().take(self.degree[0] + 1) {
                let p = self.points[row + ku - self.degree[0] + i];
                let w = bu * bv * p[3];
                sum[0] += w * p[0];
                sum[1] += w * p[1];
                sum[2] += w * p[2];
                sum[3] += w;
            }
        }
        let w = if sum[3].abs() > 1e-300 { sum[3] } else { 1.0 };
        [sum[0] / w, sum[1] / w, sum[2] / w]
    }

    /// The distinct knots of a direction, ascending, over its range.
    pub fn distinct_knots(&self, dir: usize) -> Vec<f64> {
        let (lo, hi) = self.range(dir);
        let mut out: Vec<f64> = self.knots[dir].iter().copied().filter(|&k| k >= lo && k <= hi).collect();
        out.dedup();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn knot(k: f64, m: i32) -> [Tok; 2] {
        [Tok::Real(k), Tok::Int(m)]
    }

    #[test]
    fn a_rational_quarter_circle() {
        // degree 2, weights 1, √½, 1: the arc from (1, 0) to (0, 1)
        let w = std::f64::consts::FRAC_1_SQRT_2;
        let mut t = vec![Tok::Id("nurbs".into()), Tok::Int(2), Tok::Enum(0), Tok::Int(2)];
        t.extend(knot(0.0, 2));
        t.extend(knot(1.0, 2));
        for p in [[1.0, 0.0, 0.0, 1.0], [1.0, 1.0, 0.0, w], [0.0, 1.0, 0.0, 1.0]] {
            t.extend(p.map(Tok::Real));
        }
        t.push(Tok::Real(0.0));
        let c = find_curve(&t).unwrap();
        assert_eq!((c.degree, c.knots.len(), c.points.len(), c.range()), (2, 6, 3, (0.0, 1.0)));
        for k in 0..=10 {
            let p = c.eval(f64::from(k) / 10.0);
            assert!((p[0].hypot(p[1]) - 1.0).abs() < 1e-12, "{p:?}");
        }
        assert_eq!(c.eval(0.0), [1.0, 0.0, 0.0]);
        assert_eq!(c.eval(1.0), [0.0, 1.0, 0.0]);
        // not inside a nested { }
        let mut nested = vec![Tok::Open];
        nested.extend(t.clone());
        nested.push(Tok::Close);
        assert!(find_curve(&nested).is_none());
    }

    #[test]
    fn a_bilinear_patch_and_a_cubic_curve() {
        let mut t = vec![Tok::Id("nubs".into()), Tok::Int(1), Tok::Int(1), Tok::Enum(0), Tok::Enum(0), Tok::Enum(0), Tok::Enum(0), Tok::Int(2), Tok::Int(2)];
        for _ in 0..2 {
            t.extend(knot(0.0, 1));
            t.extend(knot(2.0, 1));
        }
        for p in [[0.0, 0.0, 0.0], [4.0, 0.0, 0.0], [0.0, 2.0, 0.0], [4.0, 2.0, 8.0]] {
            t.extend(p.map(Tok::Real));
        }
        let s = find_surface(&t).unwrap();
        assert_eq!((s.count, s.range(0), s.range(1)), ([2, 2], (0.0, 2.0), (0.0, 2.0)));
        assert_eq!(s.eval(1.0, 1.0), [2.0, 1.0, 2.0]);
        assert_eq!(s.eval(2.0, 2.0), [4.0, 2.0, 8.0]);
        assert!(find_curve(&t).is_none());

        // a cubic with an inner knot: a straight line stays straight
        let mut t = vec![Tok::Id("nubs".into()), Tok::Int(3), Tok::Enum(0), Tok::Int(3)];
        t.extend(knot(0.0, 3));
        t.extend(knot(1.0, 1));
        t.extend(knot(3.0, 3));
        for k in 0..5 {
            t.extend([f64::from(k), 2.0 * f64::from(k), 0.0].map(Tok::Real));
        }
        let c = find_curve(&t).unwrap();
        assert_eq!((c.points.len(), c.knots_between(0.0, 3.0)), (5, vec![1.0]));
        for k in 0..=6 {
            let p = c.eval(f64::from(k) / 2.0);
            assert!((p[1] - 2.0 * p[0]).abs() < 1e-12 && p[2] == 0.0, "{p:?}");
        }
        // damaged: a knot count that does not fit the tokens
        assert!(curve_at(&t[..8], 0).is_none());
    }
}
