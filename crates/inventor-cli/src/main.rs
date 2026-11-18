//! `inventor-cli`: look into Inventor files from the command line.
//!
//! ```text
//! inventor-cli info  <file>              kind, version, iProperties, segments, mesh summary
//! inventor-cli json  <file>              everything as JSON
//! inventor-cli obj   <file> -o out.obj   the meshes as Wavefront OBJ (millimetres)
//! inventor-cli thumbnail <file> -o t.png the PNG thumbnail
//! inventor-cli dump  <file> -o <folder> the unpacked segments and the other streams, one file each
//! inventor-cli records <file> [name]    the records of the B-rep, or those whose type name contains `name`
//! inventor-cli corpus <folder>           every .ipt and .iam below a folder: one line each, then totals
//! ```

use inventor_read::Document;
use std::io::Write;
use std::process::ExitCode;

fn usage() -> ExitCode {
    eprintln!(
        "usage: inventor-cli (info | json | obj | thumbnail) <file> [-o <out>]
       inventor-cli corpus <folder>"
    );
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(cmd), Some(file)) = (args.first(), args.get(1)) else { return usage() };
    let out = args.iter().position(|a| a == "-o").and_then(|i| args.get(i + 1));
    if cmd == "corpus" {
        return corpus(std::path::Path::new(file));
    }
    let doc = match Document::open(file) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{file}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let result = match cmd.as_str() {
        "info" => info(&doc, file),
        "json" => {
            let mut j = inventor_read::json::document(&doc);
            if let Ok(s) = scene(&doc, file) {
                j["scene"] = inventor_read::json::scene(&s);
            }
            println!("{}", serde_json::to_string_pretty(&j).unwrap_or_default());
            Ok(())
        }
        "obj" => obj(&doc, file, out),
        "dump" => dump(&doc, out),
        "records" => records(&doc, args.get(2).map(String::as_str)),
        "thumbnail" => match (&doc.thumbnail, out) {
            (Some(t), Some(o)) => std::fs::write(o, &t.png).map_err(|e| e.to_string()),
            (None, _) => Err("no thumbnail".into()),
            (_, None) => Err("-o <file.png> needed".into()),
        },
        _ => return usage(),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{file}: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The meshes of a document; the part files of an assembly are looked for next to it.
fn scene(doc: &Document, path: &str) -> inventor_read::Result<inventor_read::Scene> {
    let mut locator = inventor_read::locate::Locator::new(std::path::Path::new(path));
    doc.scene_with_parts(&inventor_read::Options::default(), &mut |file| locator.read(file))
}

fn info(doc: &Document, path: &str) -> Result<(), String> {
    let v = doc.version();
    println!("Kind:      {}{}", doc.kind.name(), if doc.sheet_metal { " (sheet metal)" } else { "" });
    println!("Version:   {} (build {})", v.display.as_deref().unwrap_or("?"), v.build.map(|b| b.to_string()).unwrap_or("?".into()));
    if let Some(k) = doc.kernel_version() {
        println!("Kernel:    ASM {k}");
    }
    for (k, val) in doc.iproperties.rows() {
        println!("{k}: {val}");
    }
    if let Some(t) = &doc.thumbnail {
        println!("Thumbnail: {}×{} PNG, {} bytes", t.width, t.height, t.png.len());
    }
    println!("Segments:");
    for s in &doc.segments {
        println!("  {:<24} {:>10} bytes  {:?}", s.class, s.stored_size, s.codec);
    }
    for r in &doc.references {
        println!("Reference: {r}");
    }
    if doc.kind == inventor_read::DocumentKind::Assembly {
        match doc.assembly() {
            Ok(a) => {
                for f in &a.files {
                    println!("File {}: {} ({} occurrences)", f.id, f.path, f.occurrences);
                }
                for o in &a.occurrences {
                    let file = o.file.map(|f| a.files[f].file_name()).unwrap_or("?");
                    println!("Occurrence {} {:?}: {file} at ({:.3}, {:.3}, {:.3}) cm", o.id, o.name, o.matrix[0][3], o.matrix[1][3], o.matrix[2][3]);
                }
            }
            Err(e) => println!("Occurrences: {e}"),
        }
    }
    match scene(doc, path) {
        Ok(s) => {
            for m in &s.meshes {
                println!("Mesh {:?}: {} vertices, {} triangles", m.name, m.vertex_count(), m.triangle_count());
            }
            for w in &s.warnings {
                println!("Warning: {w}");
            }
        }
        Err(e) => println!("Meshes:    {e}"),
    }
    Ok(())
}

fn obj(doc: &Document, path: &str, out: Option<&String>) -> Result<(), String> {
    let out = out.ok_or("-o <file.obj> needed")?;
    let scene = scene(doc, path).map_err(|e| e.to_string())?;
    let mut f = std::io::BufWriter::new(std::fs::File::create(out).map_err(|e| e.to_string())?);
    let mut base = 1;
    for m in &scene.meshes {
        writeln!(f, "o {}", m.name).map_err(|e| e.to_string())?;
        for p in m.positions.chunks_exact(3) {
            writeln!(f, "v {} {} {}", p[0], p[1], p[2]).map_err(|e| e.to_string())?;
        }
        for n in m.normals.chunks_exact(3) {
            writeln!(f, "vn {} {} {}", n[0], n[1], n[2]).map_err(|e| e.to_string())?;
        }
        for t in m.indices.chunks_exact(3) {
            let (a, b, c) = (t[0] as usize + base, t[1] as usize + base, t[2] as usize + base);
            writeln!(f, "f {a}//{a} {b}//{b} {c}//{c}").map_err(|e| e.to_string())?;
        }
        base += m.vertex_count();
    }
    for w in &scene.warnings {
        eprintln!("{w}");
    }
    Ok(())
}

fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n != ".git" && n != "OldVersions") {
                walk(&p, out);
            }
        } else if p.extension().and_then(|x| x.to_str()).is_some_and(|x| x.eq_ignore_ascii_case("ipt") || x.eq_ignore_ascii_case("iam")) {
            out.push(p);
        }
    }
}

/// Reads every part and assembly below a folder and prints what came of each.
fn corpus(root: &std::path::Path) -> ExitCode {
    let mut files = Vec::new();
    walk(root, &mut files);
    files.sort();
    let mut totals: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let mut skipped: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    let (mut faces, mut shown) = (0usize, 0usize);
    for path in &files {
        let rel = path.strip_prefix(root).unwrap_or(path).display().to_string();
        let line = std::panic::catch_unwind(|| one(path)).unwrap_or_else(|_| Line { status: "PANIC".into(), ..Default::default() });
        *totals.entry(format!("{} {}", line.kind, line.status)).or_default() += 1;
        faces += line.faces;
        shown += line.shown;
        for (k, v) in &line.skipped {
            *skipped.entry(k.clone()).or_default() += v;
        }
        println!(
            "{}\t{}\t{}\t{}\t{} tri\t{}/{} faces\t{}\t{}\t{}",
            line.status, line.kind, line.release, line.source, line.triangles, line.shown, line.faces, line.parts, rel, line.note
        );
    }
    println!();
    for (k, v) in &totals {
        println!("{v:>6}  {k}");
    }
    println!("{shown} of {faces} B-rep faces shown");
    for (k, v) in &skipped {
        println!("{v:>6}  skipped: {k}");
    }
    ExitCode::SUCCESS
}

#[derive(Default)]
struct Line {
    status: String,
    kind: String,
    release: String,
    source: String,
    triangles: usize,
    faces: usize,
    shown: usize,
    skipped: Vec<(String, usize)>,
    note: String,
    parts: String,
}

fn one(path: &std::path::Path) -> Line {
    let mut line = Line::default();
    let doc = match Document::open(path) {
        Ok(d) => d,
        Err(e) => {
            line.status = "UNREADABLE".into();
            line.note = e.to_string();
            return line;
        }
    };
    line.kind = doc.kind.name().to_string();
    line.release = doc.version().release.map(|r| r.to_string()).unwrap_or_default();
    match scene(&doc, &path.to_string_lossy()) {
        Ok(s) => {
            if let Some(p) = &s.parts {
                line.parts = format!("{}/{} parts", p.shown, p.occurrences);
            }
            line.source = format!("{:?}", s.source);
            line.triangles = s.meshes.iter().map(|m| m.triangle_count()).sum();
            if let Some(b) = &s.brep {
                line.faces = b.faces;
                line.shown = b.tessellated;
                line.skipped.extend(b.unsupported_surface.iter().map(|(k, v)| (k.clone(), *v)));
                line.skipped.extend(b.unsupported_curve.iter().map(|(k, v)| (k.clone(), *v)));
                if b.failed > 0 {
                    line.skipped.push(("failed".into(), b.failed));
                }
            }
            let parts_left_out = s.parts.as_ref().is_some_and(|p| p.shown < p.occurrences);
            line.status = if line.triangles == 0 {
                "EMPTY"
            } else if line.shown < line.faces || parts_left_out {
                "PARTIAL"
            } else {
                "OK"
            }
            .into();
            line.note = s.warnings.join("; ");
        }
        Err(e) => {
            line.status = "NOSCENE".into();
            line.note = e.to_string();
        }
    }
    line
}

/// Prints the B-rep records: index, type name and fields.
fn records(doc: &Document, filter: Option<&str>) -> Result<(), String> {
    let brep = doc.brep().map_err(|e| e.to_string())?;
    println!("version {} units {} mm, {} records, end {:?}", brep.version, brep.mm_per_unit, brep.records.len(), brep.end);
    for (i, r) in brep.records.iter().enumerate() {
        if filter.is_none_or(|f| r.name.contains(f)) {
            println!("{i} {} {:?}", r.name, r.fields);
        }
    }
    Ok(())
}

/// Writes every segment unpacked (`<class>.B.bin`, `<class>.M.bin`) and every other stream.
fn dump(doc: &Document, out: Option<&String>) -> Result<(), String> {
    let dir = std::path::PathBuf::from(out.ok_or("-o <folder> needed")?);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let clean = |s: &str| s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect::<String>();
    for seg in &doc.segments {
        match doc.segment_data(seg) {
            Ok(d) => std::fs::write(dir.join(format!("{}.B.bin", clean(&seg.class))), d).map_err(|e| e.to_string())?,
            Err(e) => eprintln!("{}: {e}", seg.class),
        }
        if let Ok(m) = doc.stream(&seg.data_path.replace("/RSeStorage/B", "/RSeStorage/M")) {
            std::fs::write(dir.join(format!("{}.M.bin", clean(&seg.class))), m).map_err(|e| e.to_string())?;
        }
    }
    for path in doc.stream_paths() {
        if path.starts_with("/RSeStorage/B") || path.starts_with("/RSeStorage/M") {
            continue;
        }
        if let Ok(d) = doc.stream(&path) {
            std::fs::write(dir.join(format!("stream{}.bin", clean(&path))), d).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}
