# inventor-dll

Native library (DLL) that reads Autodesk Inventor parts (`.ipt`) and assemblies (`.iam`)
without Inventor, for [Open 3D Viewer Studio](https://github.com/OpenAEC-Foundation/open-3d-viewer),
[OpenCAD Studio](https://github.com/OpenAEC-Foundation/OpenCADStudio) and other tools.

**Status:** private, pre-alpha. Do not make this repository public without a decision by the
maintainers (see "Legal" below).

## Scope

- Input: Inventor parts and assemblies, Inventor 2011 to 2027 (as far as the specification
  covers them). Drawings (`.idw`) and presentations (`.ipn`) are recognised and their
  iProperties read, but they have no geometry here.
- Output: the document type and version, the iProperties (all property sets, and the known
  fields by name), the PNG thumbnail, the segments of the model, the files an assembly
  references, and triangle meshes in **millimetres**:
  - parts: the B-rep tessellated, one mesh per body (and per colour, where faces have
    colours of their own);
  - assemblies: the display cache Inventor stores in the file (already in assembly
    coordinates), or, when the file has none, its part files, each tessellated once and placed
    where the assembly places it; subassemblies are opened up the same way.
- No writing of Inventor files.

| Crate | What it is |
|---|---|
| `inventor-read` | Compound file ([MS-CFB]), property sets ([MS-OLEPS]), thumbnail, segments (zlib and Zstandard), the B-rep token stream and its tessellation, the display cache (bzip2), JSON. Feature `synth`: writers of synthetic Inventor files for tests. Builds for `wasm32-unknown-unknown`. |
| `inventor-ffi` | The C ABI: `inventor.dll` / `libinventor.so` / `libinventor.dylib`, header [`include/inventor.h`](include/inventor.h). |
| `inventor-cli` | The `inventor-cli` tool: `info`, `json`, `obj`, `thumbnail`. |

All dependencies are generic crates with free licences and no Inventor knowledge: `cfb`,
`flate2`, `ruzstd`, `bzip2` (pure-Rust backend), `spade` (constrained Delaunay
triangulation), `serde`/`serde_json`.

## What is read

From the clean-room specification `SPEC_v0.1.md` (section numbers below). ✓ read and tested
on synthetic files, ~ partly, ✗ not in the specification yet.

| | |
|---|---|
| Container, document type (part, sheet metal part, assembly, drawing, presentation), version, build, segment codec (§1, §3.3, §8) | ✓ |
| Property sets: every set and value, user-defined property names, the named fields of §2.2 | ✓ |
| Thumbnail (PNG, passed on unchanged) (§2.3) | ✓ |
| Segments: `B`/`M` pairs, class names, zlib (2011–2025) and Zstandard (2026 and later) (§3) | ✓ |
| B-rep: token stream, records, `body → lump → shell → face → loop → coedge → edge → vertex` (§4) | ✓ |
| Faces on planes, cylinders and cones; edges on lines, circles and ellipses (§4.7) | ✓ |
| Faces on spheres and tori | ✓ checked on real files |
| Faces on spline surfaces and edges on intersection curves: the B-spline the record stores (exact, or the approximation of a procedural surface or curve), also through `{ ref n }` | ✓ checked on real files |
| Helical surfaces and curves (`helix_spl_circ`, `helix_spl_line`, `helix_int_cur`: threads, springs), which store no B-spline | ✗ skipped and counted |
| Body transforms | ✓ |
| Face colours (`NMx_FFColor_Entity` attributes, as imported models have them) | ✓ |
| Assembly display cache: vertex formats, vertex and index buffers, finest level of detail (§5) | ✓ |
| Assemblies without a display cache, or with an empty one: the placed parts (`AmRxSegment`) with the file of each (`UFRxDoc`), put together from the part files the caller gives | ✓ checked on real files (2014 to 2023) |
| Suppressed and invisible occurrences, positional representations, level of detail | ✗ every occurrence is shown |
| Colours and materials (§7) | ~ face colours; a colour that the name of an appearance tells (`Opaque(67,72,77)`, a colour word); the appearance library of the file is not read |

Skipped faces give a warning such as
`Inventor: 12 faces skipped (unsupported surface: spline-surface helix_spl_circ 12)` or
`Inventor: 3 faces skipped (could not be tessellated: cone or cylinder 3)`; part files of an
assembly that were not given such as `Inventor: 2 referenced files not found: Pin.ipt, Nut.ipt`.

### How parts are tessellated

Every face is triangulated in its own parameter plane: a plane in its (u, v) frame, a cone
or cylinder unrolled to (angle, height). The loops of the face are traced through their
coedges, with the points of each edge sampled once (straight edges by their vertices,
circles and ellipses every 7.5° by default, `Options::segments_per_circle`), so that
neighbouring faces share their edge points exactly. A constrained Delaunay triangulation of
the loops gives the triangles inside the face, which are lifted back onto the surface;
cylinder normals are analytic.

Faces on spheres, tori and B-spline surfaces are triangulated in the parameter plane of the
surface in the same way, together with a grid of points inside the face so that the triangles
follow the curvature. A loop that runs once around the surface (the rim of a dome, a fillet
around a hole) bounds a band: between two such loops, or between one and the pole of a sphere.
On which side of a loop the face lies follows from the direction of the loop (seen from
outside, the face is on its left). The parameters of an edge point on a B-spline surface are
found by Newton's method from the nearest point of a coarse grid. Edges on intersection curves
are sampled along the stored B-spline through its knots and thinned to a chord tolerance.

The specification gives the outward side of planar faces (normal and face sense). For cones
and cylinders it does not say which side the surface normal is on (question Q-I1), so a cone
face starts with "away from the axis" times its face sense, and is then turned, where needed,
to match its planar neighbours: two consistently oriented faces run along a shared edge in
opposite directions. Likewise the slope sign of a cone (Q-I2) and the direction of an
ellipse parameter (Q-I3) are taken from the vertices, not assumed.

## Use

Rust (pin a revision, like the viewer does):

```toml
inventor-read = { git = "https://github.com/OpenAEC-Foundation/inventor-dll", rev = "<commit>" }
```

```rust
let doc = inventor_read::Document::open("bracket.ipt")?;   // or Document::from_bytes(bytes)
println!("{} {:?}", doc.kind.name(), doc.version().display);
for (label, value) in doc.iproperties.rows() {             // Part number, Material, ...
    println!("{label}: {value}");
}
let png = doc.thumbnail.as_ref().map(|t| &t.png);
let scene = doc.scene()?;                                   // meshes in millimetres
for m in &scene.meshes {
    // m.positions, m.normals (xyz f32), m.indices (u32 triangle list)
}
for w in &scene.warnings { eprintln!("{w}"); }
```

`Document::scene` returns `Error::NoDisplayCache` for an assembly without a display cache.
`Document::scene_with_parts` puts such an assembly together from its part files:

```rust
let doc = inventor_read::Document::open("frame.iam")?;
let mut locator = inventor_read::locate::Locator::new("frame.iam".as_ref());
let scene = doc.scene_with_parts(&inventor_read::Options::default(), &mut |file| locator.read(file))?;
for m in &scene.meshes {
    // m.name: the occurrence ("Leg:2", "Drawer:1/Handle:1"), m.part: the file, m.instance: the
    // number of the placed part, m.color, m.properties: the iProperties of the part
}
```

The closure gets a `FileRef` (the stored path, its part below the project folder, the display
name) and returns the bytes of that file or `None`; `Locator` looks for the file by the end of
its stored path around the assembly. `doc.assembly()` gives the files and the occurrences with
their placement matrices without reading any part.

C (`cargo build --release -p inventor-ffi` gives `target/release/inventor.dll`,
`inventor.dll.lib` and `inventor.lib`; see [`include/inventor.h`](include/inventor.h)):

```c
InventorDocument *doc = inventor_open("bracket.ipt");
const char *json = inventor_properties(doc);          /* iProperties and more, JSON */
size_t png_len;
const uint8_t *png = inventor_thumbnail_png(doc, &png_len);
ptrdiff_t n = inventor_mesh_count(doc);               /* -1: see inventor_last_error() */
for (ptrdiff_t i = 0; i < n; i++) {
    InventorMesh m;
    inventor_mesh(doc, (size_t)i, &m);                /* mm, normals, u32 indices */
}
const char *warnings = inventor_warnings(doc);        /* JSON array */
inventor_close(doc);
```

Command line (`cargo build --release -p inventor-cli`):

```
inventor-cli info bracket.ipt
inventor-cli json frame.iam
inventor-cli obj bracket.ipt -o bracket.obj
inventor-cli thumbnail bracket.ipt -o bracket.png
inventor-cli corpus samples/          # every .ipt and .iam below a folder: one line each, totals
inventor-cli dump frame.iam -o out/   # the unpacked segments and the other streams
inventor-cli records bracket.ipt cone # the B-rep records whose type name contains "cone"
```

Tests: `cargo test`. Every test builds its Inventor file in the test from the specification
(`inventor_read::synth`: property sets, `B`/`M` segments with a B-rep token stream of a cube,
a pin, a frustum and a plate with a hole, compressed with zlib and Zstandard, and a display
cache, and single cylinder faces with holes and notches across the angle seam); there are no
sample files in the repository. Other tests feed damaged and hostile files to the reader
(random damage, token floods, nested and shared property values, buffers read many times
over) to check that it neither panics nor runs out of bounds in memory or time.

## Open 3D Viewer Studio

The viewer's `src/loaders/inventor.rs` is a thin adapter: `Document::from_bytes` (or `open`) →
`Document::scene` → one layer per part body or one layer for the assembly cache, with the
iProperties as element properties and project information, and the warnings as information
rows. The dependency is a git dependency pinned to a commit of this repository.

## Clean-room rules

The Inventor format work runs with strict role separation (protocol in the analysis
workspace, following Directive 2009/24/EC art. 5(3) and 6 and CJEU C-406/10 SAS/WPL):

- The **analysis team** studies freely published sample files as black boxes (container
  structure, hex dumps, decompression, statistics, comparison with STEP/STL exports published
  next to them) and writes the specification (`SPEC_v*.md`).
- The **implementation team** (this repository) writes code only from that specification and
  from public specifications of standard formats: Microsoft [MS-CFB] and [MS-OLEPS],
  RFC 1950/1951 (zlib), RFC 8878 (Zstandard), bzip2, PNG. Questions go to the analysis team in
  writing (`QUESTIONS.md`, Q-I…) and are answered only by a specification update.
- Never: installing, running, disassembling, decompiling or debugging Inventor, Inventor
  View, Design Review, ODA or other software that reads Inventor files; reading the source,
  documentation or test files of other Inventor readers (FreeCAD InventorLoader and its forks,
  ODA, commercial converters, online viewers); the Inventor API/SDK or Apprentice; licensed
  ShapeManager or ACIS documentation; copying Autodesk content (Content Center, material and
  appearance libraries, templates).
- Contributors who have seen the sample files, the analysis material or any of the above do
  not write reader code.

The first version (0.1) was written from `SPEC_v0.1.md` alone. What came after it (assemblies
from their part files, spheres, tori, B-spline curves and surfaces, body transforms, face
colours) was written outside this role separation: with public sample files at hand as black
boxes (see "Checked on real files") and with the notes of the OpenAEC `inventor-io` project on
the record framing, the compact occurrence matrix and the file list of `UFRxDoc`. No Autodesk
software, no third-party Inventor reader and no licensed kernel documentation was used. That
difference matters for the legal review named under "Legal".

## Checked on real files

`inventor-cli corpus` over public repositories with Inventor files (not in this repository;
never commit them), Inventor 2014 to 2023:

| Source | Licence | Parts | Assemblies |
|---|---|---|---|
| github.com/bad-alloc-heavy-industries/MXKeyboard (`case/`) | BSD-3-Clause, hardware CERN-OHL-S v2 | 9 | 1 |
| github.com/RespiraWorks/Ventilator (`manufacturing/`) | Apache-2.0 | 134 | 44 |
| github.com/robotics-erlangen/hardware (`pasta/mechanics/`) | none stated | 803 | 294 |
| github.com/starliliko/3d-printed-smart-car-cad (`inventor/`) | none stated | 49 | 119 |

The counts of what opens and what is left out are in the pull request that brought these
changes; run the command again to get them for the current code.

## Test data

Never commit `.ipt`, `.iam`, `.ipn`, `.idw` or other Inventor files, sample files of third
parties, or exports derived from them; `.gitignore` enforces the file types. Tests build their
files from the specification in the test.

## Legal

Autodesk and Inventor are registered trademarks of Autodesk, Inc. This project is not
affiliated with, endorsed or sponsored by Autodesk. Before any public release: legal review of
the clean-room protocol, the terms of the sites the analysis samples came from, and a patent
freedom-to-operate check.
