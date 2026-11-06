//! Single cylinder faces with holes and notches, where the loops cross the θ = ±π branch or the
//! seam of the band (synthetic, from SPEC §4.7).

use inventor_read::synth::{self, Ending, Solid, SurfaceSpec};
use inventor_read::{Document, Mesh};

const R: f64 = 1.0; // cm

fn deg(d: f64) -> f64 {
    d.to_radians()
}

fn at(th: f64, z: f64) -> [f64; 3] {
    [R * th.cos(), R * th.sin(), z]
}

/// A circular arc on the cylinder at height `z` from angle `t0` to `t1` (radians).
fn arc(s: &mut Solid, a: usize, b: usize, z: f64, t0: f64, t1: f64) -> usize {
    s.arc(a, b, [0.0, 0.0, z], [0.0, 0.0, 1.0], [R, 0.0, 0.0], t0, t1)
}

fn cylinder() -> SurfaceSpec {
    SurfaceSpec::Cylinder { c: [0.0; 3], axis: [0.0, 0.0, 1.0], major: [R, 0.0, 0.0] }
}

/// A rectangle in (θ, z) as a loop: arc along the bottom, line up, arc back along the top, line
/// down; the loop starts at the corner `first` (0..4).
fn rectangle(s: &mut Solid, t0: f64, t1: f64, z0: f64, z1: f64, first: usize) -> Vec<(usize, bool)> {
    let v = [s.vertex(at(t0, z0)), s.vertex(at(t1, z0)), s.vertex(at(t1, z1)), s.vertex(at(t0, z1))];
    let bottom = arc(s, v[0], v[1], z0, t0, t1);
    let right = s.line(v[1], v[2]);
    let top = arc(s, v[3], v[2], z1, t0, t1);
    let left = s.line(v[0], v[3]);
    let mut lp = vec![(bottom, true), (right, true), (top, false), (left, false)];
    lp.rotate_left(first);
    lp
}

fn mesh(s: &Solid) -> Mesh {
    let doc = Document::from_bytes(synth::part(s, 1, 228, Ending::EndRecord)).unwrap();
    let scene = doc.scene().unwrap();
    assert!(scene.warnings.is_empty(), "{:?}", scene.warnings);
    scene.meshes.into_iter().next().unwrap()
}

fn area(m: &Mesh) -> f64 {
    let p = |i: u32| {
        let i = i as usize * 3;
        [m.positions[i] as f64, m.positions[i + 1] as f64, m.positions[i + 2] as f64]
    };
    m.indices
        .chunks_exact(3)
        .map(|t| {
            let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
            let (u, w) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
            let g = [u[1] * w[2] - u[2] * w[1], u[2] * w[0] - u[0] * w[2], u[0] * w[1] - u[1] * w[0]];
            (g[0] * g[0] + g[1] * g[1] + g[2] * g[2]).sqrt() / 2.0
        })
        .sum()
}

/// (θ in degrees, z in mm) of every triangle centroid, projected back onto the cylinder.
fn centroids(m: &Mesh) -> Vec<(f64, f64)> {
    m.indices
        .chunks_exact(3)
        .map(|t| {
            let c: Vec<f64> = (0..3).map(|k| t.iter().map(|&i| m.positions[i as usize * 3 + k] as f64).sum::<f64>() / 3.0).collect();
            (c[1].atan2(c[0]).to_degrees(), c[2])
        })
        .collect()
}

fn close(a: f64, b: f64, rel: f64) -> bool {
    (a - b).abs() <= rel * b.abs()
}

#[test]
fn partial_cylinder_with_a_hole_across_the_branch_cut() {
    for first in 0..4 {
        let mut s = Solid::default();
        let outer = rectangle(&mut s, deg(170.0), deg(190.0), 0.0, 2.0, 0);
        // the hole loop starts at 185°, on the other side of atan2's ±180°
        let hole = rectangle(&mut s, deg(175.0), deg(185.0), 0.5, 1.5, (first + 1) % 4);
        s.face(cylinder(), true, vec![outer, hole]);
        let m = mesh(&s);
        // mm²: R·Δθ·h of the strip minus that of the hole
        let exact = 100.0 * (R * deg(20.0) * 2.0 - R * deg(10.0) * 1.0);
        assert!(close(area(&m), exact, 0.01), "area {} vs {exact}", area(&m));
        for (th, z) in centroids(&m) {
            let th = th.rem_euclid(360.0);
            assert!(!((175.5..184.5).contains(&th) && (5.5..14.5).contains(&z)), "triangle in the hole at {th}° {z} mm");
        }
    }
}

#[test]
fn band_with_a_keyway_notch() {
    // every start edge, and the notched loop run both ways round
    for first in 0..4 {
        for reversed in [false, true] {
            let mut s = Solid::default();
            let (t1, t2) = (deg(80.0), deg(100.0));
            let b0 = s.vertex(at(0.0, 0.0));
            let bottom = arc(&mut s, b0, b0, 0.0, 0.0, std::f64::consts::TAU);
            // the top rim the long way round, down the keyway wall, along its floor, up the other wall
            let v = [s.vertex(at(t2, 2.0)), s.vertex(at(t1, 2.0)), s.vertex(at(t1, 1.5)), s.vertex(at(t2, 1.5))];
            let rim = arc(&mut s, v[0], v[1], 2.0, t2, t1 + std::f64::consts::TAU);
            let wall1 = s.line(v[1], v[2]);
            let floor = arc(&mut s, v[2], v[3], 1.5, t1, t2);
            let wall2 = s.line(v[3], v[0]);
            let mut notched = vec![(rim, true), (wall1, true), (floor, true), (wall2, true)];
            if reversed {
                notched = notched.into_iter().rev().map(|(e, f)| (e, !f)).collect();
            }
            notched.rotate_left(first);
            s.face(cylinder(), true, vec![vec![(bottom, true)], notched]);
            let m = mesh(&s);
            let exact = 100.0 * (std::f64::consts::TAU * R * 2.0 - R * (t2 - t1) * 0.5);
            assert!(close(area(&m), exact, 0.01), "area {} vs {exact} (start {first}, reversed {reversed})", area(&m));
            for (th, z) in centroids(&m) {
                assert!(!((80.5..99.5).contains(&th) && z > 15.5), "triangle over the keyway at {th}° {z} mm");
            }
        }
    }
}

#[test]
fn tube_with_a_window_across_the_seam() {
    let mut s = Solid::default();
    let tau = std::f64::consts::TAU;
    let b = s.vertex(at(0.0, 0.0));
    let t = s.vertex(at(0.0, 2.0));
    let bottom = arc(&mut s, b, b, 0.0, 0.0, tau);
    let top = arc(&mut s, t, t, 2.0, 0.0, tau);
    // a window from −10° to 10°: across θ = 0, the direction of M
    let window = rectangle(&mut s, deg(-10.0), deg(10.0), 0.5, 1.5, 2);
    s.face(cylinder(), true, vec![vec![(bottom, true)], vec![(top, false)], window]);
    let m = mesh(&s);
    let exact = 100.0 * (tau * R * 2.0 - R * deg(20.0) * 1.0);
    assert!(close(area(&m), exact, 0.01), "area {} vs {exact}", area(&m));
    for (th, z) in centroids(&m) {
        assert!(!((-9.5..9.5).contains(&th) && (5.5..14.5).contains(&z)), "triangle in the window at {th}° {z} mm");
    }
}
