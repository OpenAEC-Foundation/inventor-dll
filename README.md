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
  - parts: the B-rep tessellated, one mesh per body;
  - assemblies: the display cache Inventor stores in the file (already in assembly
    coordinates).
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
| Faces on spheres, tori and splines; `intcurve` edges; body transforms | ✗ skipped and counted |
| Assembly display cache: vertex formats, vertex and index buffers, finest level of detail (§5) | ✓ |
| Assemblies without a display cache (Inventor 2013–2015 and others, §6) | ✗ refused with a message |
| Referenced files of an assembly (`UFRxDoc`) (§6) | ~ listed, not loaded |
| Colours and materials (§7) | ✗ names only (iProperties) |

Skipped faces give a warning such as
`Inventor: 12 faces skipped (unsupported surface: spline-surface 10, torus-surface 2)`.

### How parts are tessellated

Every face is triangulated in its own parameter plane: a plane in its (u, v) frame, a cone
or cylinder unrolled to (angle, height). The loops of the face are traced through their
coedges, with the points of each edge sampled once (straight edges by their vertices,
circles and ellipses every 7.5° by default, `Options::segments_per_circle`), so that
neighbouring faces share their edge points exactly. A constrained Delaunay triangulation of
the loops gives the triangles inside the face, which are lifted back onto the surface;
cylinder normals are analytic.

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

`Document::scene` returns `Error::NoDisplayCache` for an assembly without a display cache;
`doc.references` then lists the referenced part files.

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
```

Tests: `cargo test`. Every test builds its Inventor file in the test from the specification
(`inventor_read::synth`: property sets, `B`/`M` segments with a B-rep token stream of a cube,
a pin, a frustum and a plate with a hole, compressed with zlib and Zstandard, and a display
cache); there are no sample files in the repository. A test also feeds damaged files to the
reader to check that it does not panic.

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

This first version (0.1) was written from `SPEC_v0.1.md` alone.

## Test data

Never commit `.ipt`, `.iam`, `.ipn`, `.idw` or other Inventor files, sample files of third
parties, or exports derived from them; `.gitignore` enforces the file types. Tests build their
files from the specification in the test.

## Legal

Autodesk and Inventor are registered trademarks of Autodesk, Inc. This project is not
affiliated with, endorsed or sponsored by Autodesk. Before any public release: legal review of
the clean-room protocol, the terms of the sites the analysis samples came from, and a patent
freedom-to-operate check.
