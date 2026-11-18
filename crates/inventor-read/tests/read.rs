//! End-to-end tests on files built here from the specification (SPEC §9).

use inventor_read::synth::{self, Cache, Ending, Solid, SurfaceSpec};
use inventor_read::{Codec, Document, DocumentKind, Error, Mesh, MeshSource};
use std::f64::consts::PI;

fn open(bytes: Vec<u8>) -> Document {
    Document::from_bytes(bytes).expect("synthetic file opens")
}

/// Signed volume of a closed triangle mesh (positive when the triangles face outward).
fn volume(m: &Mesh) -> f64 {
    let p = |i: u32| {
        let i = i as usize * 3;
        [m.positions[i] as f64, m.positions[i + 1] as f64, m.positions[i + 2] as f64]
    };
    m.indices
        .chunks_exact(3)
        .map(|t| {
            let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
            a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0]) + a[2] * (b[0] * c[1] - b[1] * c[0])
        })
        .sum::<f64>()
        / 6.0
}

fn bounds(m: &Mesh) -> ([f32; 3], [f32; 3]) {
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for p in m.positions.chunks_exact(3) {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    (lo, hi)
}

/// Every triangle's winding agrees with its vertex normals.
fn winding_matches_normals(m: &Mesh) -> bool {
    let v = |i: u32, a: &[f32]| {
        let i = i as usize * 3;
        [a[i] as f64, a[i + 1] as f64, a[i + 2] as f64]
    };
    m.indices.chunks_exact(3).all(|t| {
        let (a, b, c) = (v(t[0], &m.positions), v(t[1], &m.positions), v(t[2], &m.positions));
        let (u, w) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
        let g = [u[1] * w[2] - u[2] * w[1], u[2] * w[0] - u[0] * w[2], u[0] * w[1] - u[1] * w[0]];
        let n = v(t[0], &m.normals);
        g[0] * n[0] + g[1] * n[1] + g[2] * n[2] > 0.0
    })
}

#[test]
fn container_version_and_iproperties() {
    let doc = open(synth::part(&Solid::cube(1.0), 1, 228, Ending::EndRecord));
    assert_eq!(doc.kind, DocumentKind::Part);
    assert!(!doc.sheet_metal);
    let v = doc.version();
    assert_eq!(v.display.as_deref(), Some("2024"));
    assert_eq!(v.release, Some(2024));
    assert_eq!(v.build, Some(280_158_000));
    assert_eq!(v.codec, Some(Codec::Zlib));
    assert_eq!(doc.kernel_version(), Some(228));
    let p = &doc.iproperties;
    assert_eq!(p.part_number.as_deref(), Some("Bracket-01"));
    assert_eq!(p.title.as_deref(), Some("Synthetic bracket"));
    assert_eq!(p.author.as_deref(), Some("OpenAEC test"));
    assert_eq!(p.company.as_deref(), Some("OpenAEC Foundation"));
    assert_eq!(p.material.as_deref(), Some("Aluminium 6061"));
    assert_eq!(p.appearance.as_deref(), Some("Aluminium (Polished)"));
    assert_eq!(p.color_style.as_deref(), Some("Blue - Wall Paint - Glossy"));
    assert_eq!(p.document_subtype.as_deref(), Some("Modeling"));
    assert_eq!(p.document_subtype_id.as_deref(), Some("4D29B490-49B2-11D0-93C3-7E0706000000"));
    assert_eq!(p.created.as_deref(), Some("2000-01-01T00:00:00Z"));
    assert_eq!(p.density, Some(2.7));
    // user-defined properties carry their names from the dictionary
    let user = doc.property_sets.iter().find(|s| s.name == "Inventor User Defined Properties").unwrap();
    let project = user.set.properties.iter().find(|p| p.id == 2).unwrap();
    assert_eq!(project.name.as_deref(), Some("Project"));
    assert_eq!(project.value.as_str(), Some("Project X"));
    assert_eq!(user.stream.len(), 26);
    // thumbnail: the PNG unchanged
    let t = doc.thumbnail.as_ref().unwrap();
    assert_eq!((t.width, t.height), (4, 3));
    assert!(t.png.starts_with(b"\x89PNG"));
    assert_eq!(t.png, synth::png(4, 3));
    // segments with their classes
    let classes: Vec<&str> = doc.segments.iter().map(|s| s.class.as_str()).collect();
    for c in ["PmAppSegment", "PmBRepSegment", "PmGraphicsSegment"] {
        assert!(classes.contains(&c), "{classes:?}");
    }
    let app = doc.segments.iter().find(|s| s.class == "PmAppSegment").unwrap();
    assert_eq!(doc.segment_data(app).unwrap(), b"app");
    assert!(doc.warnings.is_empty(), "{:?}", doc.warnings);
}

#[test]
fn json_export() {
    let doc = open(synth::part(&Solid::cube(1.0), 2, 230, Ending::EndRecord));
    let j = inventor_read::json::document(&doc);
    assert_eq!(j["kind"], "part");
    assert_eq!(j["version"]["codec"], "zstd");
    assert_eq!(j["iproperties"]["part_number"], "Bracket-01");
    assert_eq!(j["thumbnail"]["width"], 4);
    let sets = j["property_sets"].as_array().unwrap();
    assert!(sets.iter().any(|s| s["name"] == "Design Tracking Properties"));
    let s = inventor_read::json::scene(&doc.scene().unwrap());
    assert_eq!(s["faces"]["total"], 6);
    assert_eq!(s["meshes"][0]["triangles"], 12);
}

fn check_cube(codec: u8, ending: Ending) {
    let doc = open(synth::part(&Solid::cube(1.0), codec, 228, ending));
    let scene = doc.scene().unwrap();
    assert_eq!(scene.source, MeshSource::Brep);
    assert!(scene.warnings.is_empty(), "{:?}", scene.warnings);
    assert_eq!(scene.meshes.len(), 1);
    let m = &scene.meshes[0];
    assert_eq!(m.name, "Bracket-01");
    assert_eq!(m.triangle_count(), 12);
    // 1 cm → 10 mm
    assert_eq!(bounds(m), ([0.0; 3], [10.0; 3]));
    assert!((volume(m) - 1000.0).abs() < 1e-3, "{}", volume(m));
    assert!(winding_matches_normals(m));
    let stats = scene.brep.unwrap();
    assert_eq!((stats.faces, stats.tessellated, stats.skipped()), (6, 6, 0));
}

#[test]
fn cube_zlib() {
    check_cube(1, Ending::EndRecord);
}

#[test]
fn cube_zstd() {
    let doc = open(synth::part(&Solid::cube(1.0), 2, 232, Ending::EndRecord));
    assert_eq!(doc.version().codec, Some(Codec::Zstd));
    check_cube(2, Ending::EndRecord);
}

#[test]
fn cube_without_end_record() {
    check_cube(1, Ending::History);
}

#[test]
fn pin_cylinder() {
    for forward in [true, false] {
        let doc = open(synth::part(&Solid::pin(0.5, 2.0, forward), 1, 228, Ending::EndRecord));
        let scene = doc.scene().unwrap();
        assert!(scene.warnings.is_empty(), "{:?}", scene.warnings);
        let m = &scene.meshes[0];
        let exact = PI * 25.0 * 20.0;
        assert!((volume(m) - exact).abs() / exact < 0.01, "volume {} vs {exact}", volume(m));
        assert!(winding_matches_normals(m));
        let (lo, hi) = bounds(m);
        assert!((hi[2] - 20.0).abs() < 1e-4 && lo[2].abs() < 1e-4 && (hi[0] - 5.0).abs() < 1e-3);
        // side normals point away from the axis, whatever the face sense said
        for (p, n) in m.positions.chunks_exact(3).zip(m.normals.chunks_exact(3)) {
            if n[2].abs() < 0.5 {
                assert!(p[0] * n[0] + p[1] * n[1] > 0.0);
            }
        }
        // a reversed sense on the cylinder is corrected from the caps
        assert_eq!(scene.brep.unwrap().reoriented, usize::from(!forward));
    }
}

#[test]
fn frustum_cone() {
    let doc = open(synth::part(&Solid::frustum(1.0, 0.5, 1.5), 1, 228, Ending::EndRecord));
    let scene = doc.scene().unwrap();
    assert!(scene.warnings.is_empty(), "{:?}", scene.warnings);
    let m = &scene.meshes[0];
    let (r1, r2, h) = (10.0, 5.0, 15.0);
    let exact = PI * h / 3.0 * (r1 * r1 + r1 * r2 + r2 * r2);
    assert!((volume(m) - exact).abs() / exact < 0.01, "volume {} vs {exact}", volume(m));
    assert!(winding_matches_normals(m));
    // the side normals lean upwards (the cone narrows towards +z)
    let side: Vec<&[f32]> = m.normals.chunks_exact(3).filter(|n| n[2].abs() < 0.9).collect();
    assert!(!side.is_empty() && side.iter().all(|n| n[2] > 0.1));
}

#[test]
fn plate_with_hole() {
    let doc = open(synth::part(&Solid::plate_with_hole(4.0, 0.5, 1.0), 1, 228, Ending::EndRecord));
    let scene = doc.scene().unwrap();
    assert!(scene.warnings.is_empty(), "{:?}", scene.warnings);
    let m = &scene.meshes[0];
    let exact = 40.0 * 40.0 * 5.0 - PI * 100.0 * 5.0;
    assert!((volume(m) - exact).abs() / exact < 0.01, "volume {} vs {exact}", volume(m));
    assert!(winding_matches_normals(m));
    // the hole's normals point to its axis (x = y = 20 mm)
    for (p, n) in m.positions.chunks_exact(3).zip(m.normals.chunks_exact(3)) {
        let (dx, dy) = (p[0] - 20.0, p[1] - 20.0);
        if n[2].abs() < 0.5 && dx.hypot(dy) < 10.5 {
            assert!(dx * n[0] + dy * n[1] < 0.0);
        }
    }
    // no triangle of the top face covers the hole
    for t in m.indices.chunks_exact(3) {
        let c: Vec<f32> = (0..3).map(|k| t.iter().map(|&i| m.positions[i as usize * 3 + k]).sum::<f32>() / 3.0).collect();
        if (c[2] - 5.0).abs() < 1e-4 {
            assert!((c[0] - 20.0).hypot(c[1] - 20.0) > 9.0);
        }
    }
}

#[test]
fn unsupported_faces_are_skipped_and_counted() {
    let mut solid = Solid::cube(1.0);
    solid.set_surface(0, SurfaceSpec::Other("offset-surface"));
    solid.set_surface(1, SurfaceSpec::Other("spline-surface"));
    solid.set_surface(2, SurfaceSpec::Other("spline-surface"));
    let doc = open(synth::part(&solid, 1, 228, Ending::EndRecord));
    let scene = doc.scene().unwrap();
    assert_eq!(scene.meshes[0].triangle_count(), 6);
    assert_eq!(scene.warnings, vec!["Inventor: 3 faces skipped (unsupported surface: offset-surface 1, spline-surface 2)".to_string()]);
}

#[test]
fn unsupported_edge_curve_is_skipped_and_counted() {
    // a triangular prism whose one slanted edge is an intcurve
    let mut s = Solid::default();
    let v: Vec<usize> = [[0., 0., 0.], [1., 0., 0.], [0., 1., 0.], [0., 0., 1.]].iter().map(|&p| s.vertex(p)).collect();
    let e01 = s.line(v[0], v[1]);
    let e12 = s.other_edge(v[1], v[2], "intcurve-curve");
    let e20 = s.line(v[2], v[0]);
    let e03 = s.line(v[0], v[3]);
    let e13 = s.line(v[1], v[3]);
    let e23 = s.line(v[2], v[3]);
    let plane = |n: [f64; 3]| SurfaceSpec::Plane { p: [0.0; 3], n, u: [1.0, 0.0, 0.0] };
    s.face(plane([0., 0., -1.]), true, vec![vec![(e01, true), (e12, true), (e20, true)]]);
    s.face(plane([0., -1., 0.]), true, vec![vec![(e01, false), (e03, true), (e13, false)]]);
    s.face(SurfaceSpec::Plane { p: [0.0; 3], n: [-1., 0., 0.], u: [0.0, 1.0, 0.0] }, true, vec![vec![(e20, false), (e23, false), (e03, true)]]);
    let k = 1.0 / 3f64.sqrt();
    s.face(SurfaceSpec::Plane { p: [1.0, 0.0, 0.0], n: [k, k, k], u: [0.0, 1.0, 0.0] }, true, vec![vec![(e12, false), (e13, true), (e23, false)]]);
    let doc = open(synth::part(&s, 1, 228, Ending::EndRecord));
    let scene = doc.scene().unwrap();
    assert_eq!(scene.meshes[0].triangle_count(), 2);
    assert_eq!(scene.warnings, vec!["Inventor: 2 faces skipped (unsupported edge curve: intcurve-curve 2)".to_string()]);
}

#[test]
fn assembly_from_the_display_cache() {
    let mut cache = Cache::default();
    cache.boxed([0.0, 0.0, 0.0], [1.0, 2.0, 3.0]);
    cache.lines(&[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]], &[0, 1]);
    cache.boxed([5.0, 0.0, 0.0], [6.0, 1.0, 1.0]);
    let doc = open(synth::assembly(Some(&cache), &["Pin.ipt", "Plate.ipt"]));
    assert_eq!(doc.kind, DocumentKind::Assembly);
    assert!(doc.has_display_cache);
    assert_eq!(doc.references.len(), 2);
    let scene = doc.scene().unwrap();
    assert_eq!(scene.source, MeshSource::DisplayCache);
    assert!(scene.warnings.is_empty(), "{:?}", scene.warnings);
    assert_eq!(scene.meshes.len(), 2);
    // the finest level of detail, in mm
    assert_eq!(scene.meshes[0].triangle_count(), 12);
    assert_eq!(bounds(&scene.meshes[0]), ([0.0; 3], [10.0, 20.0, 30.0]));
    assert_eq!(bounds(&scene.meshes[1]), ([50.0, 0.0, 0.0], [60.0, 10.0, 10.0]));
    assert!(winding_matches_normals(&scene.meshes[0]));
    let stats = scene.cache.unwrap();
    assert_eq!((stats.vertex_buffers, stats.line_buffers, stats.lods_dropped, stats.invalid), (3, 1, 2, 0));
}

#[test]
fn assembly_without_display_cache_is_refused() {
    let doc = open(synth::assembly(None, &["Pin.ipt", "Sub.iam"]));
    assert_eq!(doc.references, vec!["C:\\Users\\someone\\Design\\Pin.ipt".to_string(), "C:\\Users\\someone\\Design\\Sub.iam".to_string()]);
    let err = doc.scene().unwrap_err();
    assert_eq!(err, Error::NoDisplayCache { references: 2 });
    assert!(err.to_string().contains("no display cache"));
}

#[test]
fn not_inventor() {
    assert!(matches!(Document::from_bytes(b"solid cube\nendsolid".to_vec()), Err(Error::NotInventor(_))));
    let empty = synth::File::new().stream("/Other", vec![1, 2, 3]).to_bytes();
    assert!(matches!(Document::from_bytes(empty), Err(Error::NotInventor(_))));
}

#[test]
fn unknown_codec_is_reported() {
    let bytes = synth::part_properties(synth::File::new(), "X", "Modeling", "2027 (Build 310000000, 1)", 310_000_000)
        .segment("PmBRepSegment", 9, &Solid::cube(1.0).segment(232, Ending::EndRecord))
        .to_bytes();
    let doc = open(bytes);
    assert_eq!(doc.segments[0].codec, Some(Codec::Unknown(9)));
    assert!(doc.warnings.iter().any(|w| w.contains("unknown codec 9")));
    assert!(matches!(doc.scene(), Err(Error::Format(_))));
}

/// A small deterministic generator for damaged input.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
}

#[test]
fn damaged_input_does_not_panic() {
    let mut rng = Lcg(7);
    let solid = Solid::plate_with_hole(4.0, 0.5, 1.0);
    let segment = solid.segment(228, Ending::EndRecord);
    for round in 0..300 {
        // damage the B-rep inside an intact container
        let mut seg = segment.clone();
        for _ in 0..1 + round % 8 {
            let i = rng.next() as usize % seg.len();
            seg[i] = rng.next() as u8;
        }
        if round % 5 == 0 {
            seg.truncate(rng.next() as usize % seg.len());
        }
        let bytes = synth::part_properties(synth::File::new(), "X", "Modeling", "2023", 270_000_000).segment("PmBRepSegment", 1, &seg).to_bytes();
        let _ = open(bytes).scene();
    }
    // damaged containers
    let whole = synth::part(&solid, 1, 228, Ending::EndRecord);
    for _ in 0..200 {
        let mut b = whole.clone();
        for _ in 0..4 {
            let i = rng.next() as usize % b.len();
            b[i] = rng.next() as u8;
        }
        if let Ok(doc) = Document::from_bytes(b) {
            let _ = doc.scene();
        }
    }
    // damaged display caches
    let mut cache = Cache::default();
    cache.boxed([0.0; 3], [1.0; 3]);
    let world_bytes = synth::assembly(Some(&cache), &[]);
    for _ in 0..100 {
        let mut b = world_bytes.clone();
        let i = rng.next() as usize % b.len();
        b[i] ^= 0x5A;
        if let Ok(doc) = Document::from_bytes(b) {
            let _ = doc.scene();
        }
    }
}
