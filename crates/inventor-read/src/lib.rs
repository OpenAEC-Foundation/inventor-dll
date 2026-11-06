//! Reader for Autodesk Inventor parts (`.ipt`) and assemblies (`.iam`), written from the
//! clean-room specification `SPEC_v0.1.md` only (see the README).
//!
//! ```no_run
//! let doc = inventor_read::Document::open("bracket.ipt")?;
//! println!("{:?} {:?}", doc.kind, doc.version().display);
//! for (label, value) in doc.iproperties.rows() {
//!     println!("{label}: {value}");
//! }
//! let scene = doc.scene()?; // triangle meshes in millimetres
//! for w in &scene.warnings {
//!     eprintln!("{w}");
//! }
//! # Ok::<(), inventor_read::Error>(())
//! ```

pub mod brep;
mod bytes;
pub mod json;
pub mod ogs;
pub mod oleps;
pub mod props;
pub mod refs;
pub mod segments;
#[cfg(feature = "synth")]
pub mod synth;
pub mod tess;

pub use props::{IProperties, NamedSet, Thumbnail};
pub use segments::{Codec, Segment};
pub use tess::Options;

use serde::Serialize;
use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::Mutex;

/// The compound file signature ([MS-CFB]).
pub const CFB_SIGNATURE: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    Io(String),
    /// Not a compound file, or not one with Inventor's streams.
    NotInventor(String),
    /// The file is damaged or not as specified.
    Format(String),
    /// An assembly without the display cache (SPEC §5.1); `references` = the referenced
    /// part and assembly files.
    NoDisplayCache {
        references: usize,
    },
    /// A document type or feature this version does not read.
    Unsupported(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "{e}"),
            Error::NotInventor(e) => write!(f, "not an Inventor file: {e}"),
            Error::Format(e) => write!(f, "damaged or unsupported Inventor file: {e}"),
            Error::NoDisplayCache { references } => write!(
                f,
                "this Inventor assembly has no display cache (CacheGraphics); showing assemblies from their part files is not supported yet ({references} referenced files)"
            ),
            Error::Unsupported(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// The document type (SPEC §1, §3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DocumentKind {
    /// `.ipt`
    Part,
    /// `.iam`
    Assembly,
    /// `.idw`
    Drawing,
    /// `.ipn`
    Presentation,
    Unknown,
}

impl DocumentKind {
    pub fn name(self) -> &'static str {
        match self {
            DocumentKind::Part => "Part",
            DocumentKind::Assembly => "Assembly",
            DocumentKind::Drawing => "Drawing",
            DocumentKind::Presentation => "Presentation",
            DocumentKind::Unknown => "Unknown",
        }
    }
}

/// Version information (SPEC §8).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Version {
    /// E.g. `2025.2`: the version text before ` (`.
    pub display: Option<String>,
    /// E.g. 292293000.
    pub build: Option<u64>,
    /// The release year from the build: 1996 + ⌊build / 10 000 000⌋.
    pub release: Option<u64>,
    /// The codec of the segments: zlib up to 2025, Zstandard from 2026.
    pub codec: Option<Codec>,
}

/// An opened Inventor file. The streams that describe the document are read on open; segments
/// are unpacked when asked for.
pub struct Document {
    file: Mutex<cfb::CompoundFile<Cursor<Vec<u8>>>>,
    pub kind: DocumentKind,
    /// A sheet metal part (`Sheet Metal` document subtype).
    pub sheet_metal: bool,
    /// Every property set (SPEC §2.1).
    pub property_sets: Vec<NamedSet>,
    /// The fields of SPEC §2.2.
    pub iproperties: IProperties,
    pub thumbnail: Option<Thumbnail>,
    pub segments: Vec<Segment>,
    /// `CacheGraphics/OGSCache/OGS/DefaultScene/streams` is present.
    pub has_display_cache: bool,
    /// Referenced `.ipt` / `.iam` paths as stored (SPEC §6).
    pub references: Vec<String>,
    /// What could not be read while opening.
    pub warnings: Vec<String>,
}

impl std::fmt::Debug for Document {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Document").field("kind", &self.kind).field("segments", &self.segments.len()).finish_non_exhaustive()
    }
}

/// One triangle mesh, in millimetres.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mesh {
    pub name: String,
    /// xyz per vertex, mm.
    pub positions: Vec<f32>,
    /// xyz per vertex, unit length.
    pub normals: Vec<f32>,
    /// Triangle list.
    pub indices: Vec<u32>,
}

impl Mesh {
    pub fn vertex_count(&self) -> usize {
        self.positions.len() / 3
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

/// Where the meshes of a scene come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MeshSource {
    /// The part's B-rep, tessellated (SPEC §4).
    Brep,
    /// The assembly's display cache (SPEC §5).
    DisplayCache,
}

/// The meshes of a document and what was left out.
#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    pub source: MeshSource,
    pub meshes: Vec<Mesh>,
    pub warnings: Vec<String>,
    /// Faces of the B-rep (parts).
    pub brep: Option<tess::Stats>,
    /// Buffers of the display cache (assemblies).
    pub cache: Option<ogs::CacheStats>,
}

impl Document {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let data = std::fs::read(path).map_err(|e| Error::Io(format!("cannot read {}: {e}", path.display())))?;
        Self::from_bytes(data)
    }

    pub fn from_bytes(data: Vec<u8>) -> Result<Self> {
        if !data.starts_with(&CFB_SIGNATURE) {
            return Err(Error::NotInventor("no compound file signature".into()));
        }
        let mut file = cfb::CompoundFile::open(Cursor::new(data)).map_err(|e| Error::Format(format!("compound file: {e}")))?;
        let mut warnings = Vec::new();

        // property sets: top-level streams whose name starts with U+0005
        let names: Vec<String> = file.read_root_storage().filter(|e| e.is_stream() && e.name().starts_with('\u{5}')).map(|e| e.name().to_string()).collect();
        let mut property_sets = Vec::new();
        for name in names {
            match read_stream(&mut file, &format!("/{name}")).ok().and_then(|d| oleps::parse_stream(&d)) {
                Some(sets) => property_sets.extend(sets.into_iter().map(|set| NamedSet { stream: name[1..].to_string(), name: props::set_name(&set), set })),
                None => warnings.push(format!("property set stream {:?} could not be read", &name[1..])),
            }
        }
        let iproperties = IProperties::from_sets(&property_sets);
        let thumbnail = props::thumbnail(&property_sets);

        let segments = if file.is_storage("/RSeStorage") { read_segments(&mut file, &mut warnings) } else { Vec::new() };
        let has_display_cache = file.is_stream(ogs::STREAM);
        let references =
            if file.is_stream("/UFRxDoc") { read_stream(&mut file, "/UFRxDoc").map(|d| refs::references(&d)).unwrap_or_default() } else { Vec::new() };

        if property_sets.is_empty() && segments.is_empty() {
            return Err(Error::NotInventor("a compound file without iProperties or RSeStorage segments".into()));
        }
        let subtype = iproperties.document_subtype.as_deref().unwrap_or("");
        let kind = kind_from_subtype(subtype).unwrap_or_else(|| kind_from_segments(&segments));
        Ok(Self {
            file: Mutex::new(file),
            kind,
            sheet_metal: subtype.eq_ignore_ascii_case("Sheet Metal"),
            property_sets,
            iproperties,
            thumbnail,
            segments,
            has_display_cache,
            references,
            warnings,
        })
    }

    pub fn version(&self) -> Version {
        let display = self.iproperties.version_text.as_deref().map(|t| t.split(" (").next().unwrap_or(t).trim().to_string()).filter(|s| !s.is_empty());
        let build = self.iproperties.build;
        Version {
            display,
            build,
            release: build.filter(|b| *b >= 10_000_000).map(|b| 1996 + b / 10_000_000),
            codec: self.segments.iter().find_map(|s| s.codec),
        }
    }

    /// The bytes of a stream, e.g. `/UFRxDoc`.
    pub fn stream(&self, path: &str) -> Result<Vec<u8>> {
        let mut file = self.file.lock().map_err(|_| Error::Io("file lock poisoned".into()))?;
        read_stream(&mut file, path).map_err(|e| Error::Format(format!("{path}: {e}")))
    }

    /// The unpacked content of a segment.
    pub fn segment_data(&self, segment: &Segment) -> Result<Vec<u8>> {
        let b = self.stream(&segment.data_path)?;
        segments::unpack(&b).map_err(|e| Error::Format(format!("segment {}: {e}", segment.class)))
    }

    /// The B-rep segment of a part (`PmBRepSegment`), parsed.
    pub fn brep(&self) -> Result<brep::Brep> {
        let seg = self.segments.iter().find(|s| s.is_brep()).ok_or_else(|| Error::Format("no B-rep segment".into()))?;
        let data = self.segment_data(seg)?;
        brep::parse(&data).map_err(Error::Format)
    }

    /// The kernel version of the B-rep (e.g. 228), from its header (SPEC §4.1, §8).
    pub fn kernel_version(&self) -> Option<i32> {
        self.brep().ok().map(|b| b.kernel_major())
    }

    /// The meshes with default options.
    pub fn scene(&self) -> Result<Scene> {
        self.scene_with(&Options::default())
    }

    /// Parts: the tessellated B-rep, one mesh per body. Assemblies: the display cache.
    pub fn scene_with(&self, opt: &Options) -> Result<Scene> {
        match self.kind {
            DocumentKind::Part => self.part_scene(opt),
            DocumentKind::Assembly if self.has_display_cache => self.cache_scene(),
            DocumentKind::Assembly => Err(Error::NoDisplayCache { references: self.references.len() }),
            DocumentKind::Drawing => Err(Error::Unsupported("Inventor drawings (.idw) are not supported yet".into())),
            DocumentKind::Presentation => Err(Error::Unsupported("Inventor presentations (.ipn) are not supported yet".into())),
            DocumentKind::Unknown if self.has_display_cache => self.cache_scene(),
            DocumentKind::Unknown => self.part_scene(opt),
        }
    }

    fn part_scene(&self, opt: &Options) -> Result<Scene> {
        let brep = self.brep()?;
        let scale = brep.mm_per_unit;
        let bodies = brep.bodies();
        let base = self.iproperties.part_number.clone().unwrap_or_else(|| "Body".into());
        let mut stats = tess::Stats::default();
        let mut meshes = Vec::new();
        let mut used = std::collections::HashSet::new();
        for (k, &body) in bodies.iter().enumerate() {
            let (faces, s) = tess::body(&brep, body, opt, &mut used);
            stats.merge(&s);
            let mut m = Mesh { name: if bodies.len() > 1 { format!("{base} ({})", k + 1) } else { base.clone() }, ..Default::default() };
            for f in faces {
                let first = m.vertex_count() as u32;
                for (p, n) in f.positions.iter().zip(&f.normals) {
                    m.positions.extend(p.map(|v| (v * scale) as f32));
                    m.normals.extend(n.map(|v| v as f32));
                }
                m.indices.extend(f.triangles.iter().flatten().map(|i| i + first));
            }
            if !m.indices.is_empty() {
                meshes.push(m);
            }
        }
        let mut warnings = self.warnings.clone();
        if bodies.is_empty() {
            warnings.push("Inventor: the B-rep has no bodies".into());
        }
        if brep.end == brep::End::Truncated {
            warnings.push("Inventor: the B-rep data ends inside a record".into());
        }
        warnings.extend(stats.warnings());
        Ok(Scene { source: MeshSource::Brep, meshes, warnings, brep: Some(stats), cache: None })
    }

    fn cache_scene(&self) -> Result<Scene> {
        let packed = self.stream(ogs::STREAM)?;
        let data = ogs::unpack(&packed).map_err(|e| Error::Format(format!("display cache: {e}")))?;
        let blocks = ogs::blocks(&data);
        let mesh = blocks.iter().find(|(n, _)| n == "stream_mesh_000").map(|b| b.1).unwrap_or(&[]);
        let world = blocks.iter().find(|(n, _)| n == "world").map(|b| b.1).ok_or_else(|| Error::Format("display cache without a world block".into()))?;
        let w = ogs::world(world, mesh.len());
        let (cache, stats) = ogs::meshes(&w, mesh);
        let mut warnings = self.warnings.clone();
        if stats.invalid > 0 {
            warnings.push(format!("Inventor: {} display cache buffers skipped (format unknown or indices out of range)", stats.invalid));
        }
        if stats.over_budget > 0 {
            warnings.push(format!("Inventor: {} display cache buffers skipped (more data than the cache holds)", stats.over_budget));
        }
        if w.orphan_index_buffers > 0 {
            warnings.push(format!("Inventor: {} display cache index buffers without a vertex buffer", w.orphan_index_buffers));
        }
        let meshes = cache
            .into_iter()
            .enumerate()
            .map(|(k, c)| Mesh {
                name: format!("Mesh {}", k + 1),
                // centimetres → millimetres
                positions: c.positions.iter().map(|v| v * 10.0).collect(),
                normals: c.normals,
                indices: c.indices,
            })
            .collect();
        Ok(Scene { source: MeshSource::DisplayCache, meshes, warnings, brep: None, cache: Some(stats) })
    }
}

fn read_stream(file: &mut cfb::CompoundFile<Cursor<Vec<u8>>>, path: &str) -> std::io::Result<Vec<u8>> {
    let mut s = file.open_stream(path)?;
    let mut out = Vec::new();
    s.read_to_end(&mut out)?;
    Ok(out)
}

fn read_segments(file: &mut cfb::CompoundFile<Cursor<Vec<u8>>>, warnings: &mut Vec<String>) -> Vec<Segment> {
    let Ok(entries) = file.read_storage("/RSeStorage") else { return Vec::new() };
    let mut data: Vec<(String, u64)> = Vec::new();
    let mut meta: Vec<String> = Vec::new();
    for e in entries {
        let name = e.name();
        if !e.is_stream() || name.chars().count() != 27 {
            continue;
        }
        let mut chars = name.chars();
        let first = chars.next();
        let key = chars.as_str().to_string();
        match first {
            Some('B') => data.push((key, e.len())),
            Some('M') => meta.push(key),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for (key, size) in data {
        if !meta.contains(&key) {
            continue;
        }
        let class = read_stream(file, &format!("/RSeStorage/M{key}")).ok().and_then(|m| segments::meta_class(&m)).unwrap_or_default();
        let data_path = format!("/RSeStorage/B{key}");
        let codec = file.open_stream(&data_path).ok().and_then(|mut s| {
            let mut head = [0u8; 18];
            s.read_exact(&mut head).ok()?;
            segments::data_header(&head).map(|(c, _)| c)
        });
        if class.is_empty() {
            warnings.push(format!("segment {key}: no class name"));
        }
        if let Some(Codec::Unknown(c)) = codec {
            warnings.push(format!("segment {class}: unknown codec {c}, skipped"));
        }
        out.push(Segment { key, class, data_path, stored_size: size, codec });
    }
    out
}

fn kind_from_subtype(subtype: &str) -> Option<DocumentKind> {
    match subtype.to_ascii_lowercase().as_str() {
        "modeling" | "sheet metal" => Some(DocumentKind::Part),
        "assembly" => Some(DocumentKind::Assembly),
        "drawing layout" => Some(DocumentKind::Drawing),
        _ => None,
    }
}

fn kind_from_segments(segments: &[Segment]) -> DocumentKind {
    let has = |p: &str| segments.iter().any(|s| s.class.to_ascii_lowercase().starts_with(p));
    if has("pm") {
        DocumentKind::Part
    } else if has("am") {
        DocumentKind::Assembly
    } else if has("dl") {
        DocumentKind::Drawing
    } else if has("dx") {
        DocumentKind::Presentation
    } else {
        DocumentKind::Unknown
    }
}
