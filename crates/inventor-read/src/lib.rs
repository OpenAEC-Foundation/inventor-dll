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

pub mod appearance;
pub mod assembly;
pub mod brep;
mod bytes;
pub mod json;
#[cfg(not(target_arch = "wasm32"))]
pub mod locate;
pub mod nurbs;
pub mod ogs;
pub mod oleps;
mod param;
pub mod props;
pub mod records;
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
                "this Inventor assembly has no display cache (CacheGraphics); it is shown from its part files, which were not given here ({references} referenced files)"
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
    /// The colour of the mesh (red, green, blue, 0 to 1), when the file gives one.
    pub color: Option<[f32; 3]>,
    /// The number of the body in its part (from 0); a body with faces in several colours has a
    /// mesh for each.
    pub body: u32,
    /// In an assembly shown from its part files: the file name of the part.
    pub part: Option<String>,
    /// In an assembly shown from its part files: the number of the placed part (from 1); the
    /// bodies of one placed part share it.
    pub instance: Option<u32>,
    /// In an assembly shown from its part files: the iProperties of the part.
    pub properties: Vec<(&'static str, String)>,
    /// In an assembly shown from its part files: the occurrence numbers from the assembly
    /// inwards to the placed part.
    pub occurrence: Vec<u32>,
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
    /// The part files of the assembly, each tessellated and placed.
    Parts,
}

/// What became of the placed parts of an assembly shown from its part files.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct PartsStats {
    /// Placed parts, those inside subassemblies included.
    pub occurrences: usize,
    /// Of those, shown.
    pub shown: usize,
    /// Occurrences whose file the assembly does not name in a way this reader follows.
    pub unlinked: usize,
    /// Occurrence records that could not be read.
    pub unreadable: usize,
    /// Placed parts and subassemblies left out because the assembly hides or suppresses them.
    pub hidden: usize,
    /// File names that were asked for and not given.
    pub missing: Vec<String>,
    /// File names that were given and could not be read, with the reason.
    pub failed: Vec<String>,
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
    /// Placed parts (assemblies shown from their part files).
    pub parts: Option<PartsStats>,
}

/// The meshes of one referenced file in its own coordinates; `Err` when there are none.
type Fetched = std::rc::Rc<std::result::Result<Vec<Mesh>, ()>>;

/// Gives the bytes of a referenced file, or `None` when it is not there.
pub type Fetch<'a> = dyn FnMut(&assembly::FileRef) -> Option<Vec<u8>> + 'a;

/// The state of one walk through an assembly and its subassemblies.
struct Walk<'a, 'b> {
    opt: &'a Options,
    fetch: &'a mut Fetch<'b>,
    /// Lower-case file name → its meshes.
    files: std::collections::HashMap<String, Fetched>,
    /// The assemblies being opened, outermost first (lower-case file names).
    stack: Vec<String>,
    stats: PartsStats,
    brep: tess::Stats,
    instances: u32,
    warnings: Vec<String>,
}

/// Subassemblies nested deeper than this are left out.
const MAX_DEPTH: usize = 24;
/// More triangles than this in one assembly are left out.
const MAX_ASSEMBLY_TRIANGLES: usize = 40_000_000;

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

    /// The paths of every stream in the compound file.
    pub fn stream_paths(&self) -> Vec<String> {
        let Ok(file) = self.file.lock() else { return Vec::new() };
        file.walk().filter(|e| e.is_stream()).map(|e| e.path().to_string_lossy().replace('\\', "/")).collect()
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

    /// What the graphics segment of a part says about each body: the number of the body (as in
    /// its B-rep record) and two bytes of its display record, at +19 and at +32.
    pub fn body_display(&self) -> Vec<(i32, u8, u8)> {
        /// The class of the display record of a body.
        const BODY: [u8; 16] = [0x50, 0x6a, 0x67, 0x9a, 0xd4, 0x11, 0xa6, 0x5d, 0x60, 0x00, 0xe3, 0xb8, 0x12, 0x69, 0xf1, 0xb0];
        let Some(segment) = self.segments.iter().find(|s| s.class.eq_ignore_ascii_case("PmGraphicsSegment")) else { return Vec::new() };
        let found = (|| {
            let meta = segments::meta_unpack(&self.stream(&segment.data_path.replace("/RSeStorage/B", "/RSeStorage/M")).ok()?)?;
            let class = segments::classes(&meta)?.iter().position(|c| *c == BODY)? as u16 + 0x100;
            let data = self.segment_data(segment).ok()?;
            let records = records::frame(&data)?;
            Some(
                records
                    .iter()
                    .filter(|r| r.class() == class && r.body.len() >= 48)
                    .map(|r| {
                        let at = r.body.len() - 12;
                        (i32::from_le_bytes([r.body[at], r.body[at + 1], r.body[at + 2], r.body[at + 3]]), r.body[19], r.body[32])
                    })
                    .collect::<Vec<_>>(),
            )
        })();
        found.unwrap_or_default()
    }

    /// The kernel version of the B-rep (e.g. 228), from its header (SPEC §4.1, §8).
    pub fn kernel_version(&self) -> Option<i32> {
        self.brep().ok().map(|b| b.kernel_major())
    }

    /// The meshes with default options.
    pub fn scene(&self) -> Result<Scene> {
        self.scene_with(&Options::default())
    }

    /// The referenced files and the placed parts of an assembly.
    pub fn assembly(&self) -> Result<assembly::Assembly> {
        let segment = self
            .segments
            .iter()
            .find(|s| s.class.eq_ignore_ascii_case("AmRxSegment"))
            .ok_or_else(|| Error::Format("no occurrence segment (AmRxSegment)".into()))?;
        let amrx = self.segment_data(segment)?;
        let ufrx = self.stream("/UFRxDoc")?;
        let views = self.segments.iter().find(|s| s.class.eq_ignore_ascii_case("DesignViewSegment")).and_then(|s| self.segment_data(s).ok());
        assembly::read(&amrx, &ufrx, views.as_deref()).map_err(|e| Error::Format(format!("assembly: {e}")))
    }

    /// As `scene_with`, and an assembly is put together from its part files: `fetch` gives the
    /// bytes of a referenced file, or `None` when it is not there. Each part is tessellated once
    /// and placed where its occurrences stand; subassemblies are opened up the same way. The
    /// display cache of the assembly is taken instead when it has one and part files are
    /// missing (or first of all, with `Options::parts_first` off): the cache shows every part,
    /// but as loose meshes without their names.
    pub fn scene_with_parts(&self, opt: &Options, fetch: &mut Fetch) -> Result<Scene> {
        if self.kind != DocumentKind::Assembly {
            return self.scene_with(opt);
        }
        let cached = self.filled_cache();
        if !opt.parts_first {
            if let Some(scene) = cached {
                return Ok(scene);
            }
            return self.parts_scene(opt, fetch);
        }
        match (self.parts_scene(opt, fetch), cached) {
            (Ok(parts), Some(mut cache)) => {
                let stats = parts.parts.as_ref();
                let complete = stats.is_some_and(|p| p.missing.is_empty() && p.failed.is_empty() && p.unlinked + p.unreadable == 0) && !parts.meshes.is_empty();
                if complete {
                    return Ok(parts);
                }
                if let Some(p) = stats.filter(|p| !p.missing.is_empty()) {
                    cache
                        .warnings
                        .push(format!("Inventor: shown from the display cache of the assembly, as {} of its part files were not found", p.missing.len()));
                }
                Ok(cache)
            }
            (Ok(parts), None) => Ok(parts),
            (Err(_), Some(cache)) => Ok(cache),
            (Err(e), None) => Err(e),
        }
    }

    /// The assembly from its part files.
    fn parts_scene(&self, opt: &Options, fetch: &mut Fetch) -> Result<Scene> {
        let mut walk = Walk {
            opt,
            fetch,
            files: std::collections::HashMap::new(),
            stack: Vec::new(),
            stats: PartsStats::default(),
            brep: tess::Stats::default(),
            instances: 0,
            warnings: self.warnings.clone(),
        };
        let meshes = self.placed(&mut walk)?;
        let Walk { mut stats, brep, mut warnings, .. } = walk;
        stats.missing.sort();
        stats.missing.dedup();
        stats.failed.sort();
        stats.failed.dedup();
        let list = |names: &[String]| {
            let shown: Vec<&str> = names.iter().take(12).map(String::as_str).collect();
            format!("{}{}", shown.join(", "), if names.len() > shown.len() { format!(" and {} more", names.len() - shown.len()) } else { String::new() })
        };
        if !stats.missing.is_empty() {
            warnings.push(format!("Inventor: {} referenced files not found: {}", stats.missing.len(), list(&stats.missing)));
        }
        if !stats.failed.is_empty() {
            warnings.push(format!("Inventor: {} referenced files could not be read: {}", stats.failed.len(), list(&stats.failed)));
        }
        if stats.unlinked + stats.unreadable > 0 {
            warnings.push(format!("Inventor: {} placed parts left out (their file or placement could not be read)", stats.unlinked + stats.unreadable));
        }
        warnings.extend(brep.warnings());
        Ok(Scene { source: MeshSource::Parts, meshes, warnings, brep: Some(brep), cache: None, parts: Some(stats) })
    }

    /// The display cache as a scene, when the file has one with triangles in it.
    fn filled_cache(&self) -> Option<Scene> {
        if !self.has_display_cache {
            return None;
        }
        self.cache_scene().ok().filter(|s| s.meshes.iter().any(|m| !m.indices.is_empty()))
    }

    /// The placed parts of this assembly as meshes in its own coordinates.
    fn placed(&self, walk: &mut Walk) -> Result<Vec<Mesh>> {
        let asm = self.assembly()?;
        walk.stats.unreadable += asm.unreadable;
        let mut out: Vec<Mesh> = Vec::new();
        let mut triangles = 0usize;
        for occ in &asm.occurrences {
            if !occ.visible {
                walk.stats.hidden += 1;
                continue;
            }
            let Some(file) = occ.file.map(|f| &asm.files[f]) else {
                walk.stats.occurrences += 1;
                walk.stats.unlinked += 1;
                continue;
            };
            let fetched = self.fetched(file, walk);
            let Ok(meshes) = &*fetched else {
                walk.stats.occurrences += 1;
                continue;
            };
            let is_assembly = file.path.to_ascii_lowercase().ends_with(".iam");
            if is_assembly {
                // counted per placement: a subassembly placed twice shows its parts twice
                let inner: std::collections::HashSet<u32> = meshes.iter().filter_map(|m| m.instance).collect();
                walk.stats.occurrences += inner.len().max(1);
                walk.stats.shown += inner.len();
            } else {
                walk.stats.occurrences += 1;
                walk.stats.shown += usize::from(!meshes.is_empty());
                walk.instances += 1;
            }
            // the placed parts inside a subassembly get numbers of their own per placement
            let mut renumbered: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
            // what this assembly hides inside the subassembly: paths below this occurrence
            let hidden: Vec<&[u32]> = asm.hidden_inside.iter().filter(|h| h[0] == occ.id).map(|h| &h[1..]).collect();
            for m in meshes.iter() {
                if hidden.iter().any(|h| m.occurrence.starts_with(h)) {
                    continue;
                }
                triangles += m.triangle_count();
                if triangles > MAX_ASSEMBLY_TRIANGLES {
                    walk.warnings.push("Inventor: the assembly has more triangles than this reader shows; the rest is left out".into());
                    return Ok(out);
                }
                let mut placed = place(m, &occ.matrix);
                placed.occurrence.insert(0, occ.id);
                if let Some(colour) = occ.appearance.as_deref().and_then(appearance::from_name) {
                    placed.color = Some(colour);
                }
                if is_assembly {
                    placed.name = format!("{}/{}", occ.name, m.name);
                    let next = &mut walk.instances;
                    placed.instance = Some(*renumbered.entry(m.instance.unwrap_or(0)).or_insert_with(|| {
                        *next += 1;
                        *next
                    }));
                } else {
                    placed.name = occ.name.clone();
                    placed.instance = Some(walk.instances);
                }
                out.push(placed);
            }
        }
        Ok(out)
    }

    /// The meshes of a referenced file, read once.
    fn fetched(&self, file: &assembly::FileRef, walk: &mut Walk) -> Fetched {
        let key = file.file_name().to_lowercase();
        if let Some(done) = walk.files.get(&key) {
            return done.clone();
        }
        let result = Self::fetch_meshes(file, &key, walk);
        let result: Fetched = std::rc::Rc::new(result);
        walk.files.insert(key, result.clone());
        result
    }

    fn fetch_meshes(file: &assembly::FileRef, key: &str, walk: &mut Walk) -> std::result::Result<Vec<Mesh>, ()> {
        let name = file.file_name().to_string();
        if walk.stack.iter().any(|s| s == key) || walk.stack.len() >= MAX_DEPTH {
            walk.stats.failed.push(format!("{name} (nested in itself or too deep)"));
            return Err(());
        }
        let Some(bytes) = (walk.fetch)(file) else {
            walk.stats.missing.push(name);
            return Err(());
        };
        let doc = match Document::from_bytes(bytes) {
            Ok(d) => d,
            Err(e) => {
                walk.stats.failed.push(format!("{name} ({e})"));
                return Err(());
            }
        };
        let properties = doc.iproperties.rows();
        let result = if doc.kind == DocumentKind::Assembly {
            // the saved picture of the subassembly: its parts are not told apart
            let cached = |doc: &Document| -> Option<Vec<Mesh>> {
                let scene = doc.filled_cache()?;
                Some(scene.meshes.into_iter().map(|m| Mesh { part: Some(name.clone()), instance: Some(0), properties: properties.clone(), ..m }).collect())
            };
            if !walk.opt.parts_first {
                if let Some(meshes) = cached(&doc) {
                    return Ok(meshes);
                }
            }
            let (missing, failed, unlinked) = (walk.stats.missing.len(), walk.stats.failed.len(), walk.stats.unlinked + walk.stats.unreadable);
            walk.stack.push(key.to_string());
            let inner = doc.placed(walk);
            walk.stack.pop();
            let complete = inner.as_ref().is_ok_and(|m| !m.is_empty())
                && walk.stats.missing.len() == missing
                && walk.stats.failed.len() == failed
                && walk.stats.unlinked + walk.stats.unreadable == unlinked;
            if !complete && walk.opt.parts_first {
                if let Some(meshes) = cached(&doc) {
                    // what its own picture shows is not missing
                    walk.stats.missing.truncate(missing);
                    walk.stats.failed.truncate(failed);
                    return Ok(meshes);
                }
            }
            inner
        } else {
            doc.part_scene(walk.opt).map(|scene| {
                if let Some(b) = &scene.brep {
                    walk.brep.merge(b);
                }
                scene.meshes.into_iter().map(|m| Mesh { part: Some(name.clone()), properties: properties.clone(), ..m }).collect()
            })
        };
        result.map_err(|e| walk.stats.failed.push(format!("{name} ({e})")))
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
        // the colour of the part as far as the name of its appearance tells it
        let own = [&self.iproperties.appearance, &self.iproperties.color_style].into_iter().flatten().find_map(|n| appearance::from_name(n));
        let mut stats = tess::Stats::default();
        let mut meshes = Vec::new();
        let mut used = std::collections::HashSet::new();
        for (k, &body) in bodies.iter().enumerate() {
            let (faces, s) = tess::body(&brep, body, opt, &mut used);
            stats.merge(&s);
            // one mesh per colour of the body: faces with a colour of their own, and the rest
            let mut by_colour: Vec<Mesh> = Vec::new();
            for f in faces {
                let colour = f.color.or(own);
                let at = match by_colour.iter().position(|m| m.color == colour) {
                    Some(at) => at,
                    None => {
                        by_colour.push(Mesh {
                            name: if bodies.len() > 1 { format!("{base} ({})", k + 1) } else { base.clone() },
                            color: colour,
                            body: k as u32,
                            ..Default::default()
                        });
                        by_colour.len() - 1
                    }
                };
                let m = &mut by_colour[at];
                let first = m.vertex_count() as u32;
                for (p, n) in f.positions.iter().zip(&f.normals) {
                    m.positions.extend(p.map(|v| (v * scale) as f32));
                    m.normals.extend(n.map(|v| v as f32));
                }
                m.indices.extend(f.triangles.iter().flatten().map(|i| i + first));
            }
            meshes.extend(by_colour.into_iter().filter(|m| !m.indices.is_empty()));
        }
        let mut warnings = self.warnings.clone();
        if bodies.is_empty() {
            warnings.push("Inventor: the B-rep has no bodies".into());
        }
        if brep.end == brep::End::Truncated {
            warnings.push("Inventor: the B-rep data ends inside a record".into());
        }
        warnings.extend(stats.warnings());
        Ok(Scene { source: MeshSource::Brep, meshes, warnings, brep: Some(stats), cache: None, parts: None })
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
                ..Default::default()
            })
            .collect();
        Ok(Scene { source: MeshSource::DisplayCache, meshes, warnings, brep: None, cache: Some(stats), parts: None })
    }
}

/// A mesh (millimetres) moved by a placement (centimetres); a mirroring placement turns the
/// triangles so that they keep facing outwards.
fn place(mesh: &Mesh, m: &assembly::Matrix) -> Mesh {
    let mut out = mesh.clone();
    for p in out.positions.chunks_exact_mut(3) {
        let v = [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])];
        for i in 0..3 {
            p[i] = (m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2] + m[i][3] * 10.0) as f32;
        }
    }
    for n in out.normals.chunks_exact_mut(3) {
        let v = [f64::from(n[0]), f64::from(n[1]), f64::from(n[2])];
        let mut w = [0.0f64; 3];
        for i in 0..3 {
            w[i] = m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2];
        }
        let l = (w[0] * w[0] + w[1] * w[1] + w[2] * w[2]).sqrt();
        if l > 1e-12 {
            for i in 0..3 {
                n[i] = (w[i] / l) as f32;
            }
        }
    }
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det < 0.0 {
        for t in out.indices.chunks_exact_mut(3) {
            t.swap(1, 2);
        }
    }
    out
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
