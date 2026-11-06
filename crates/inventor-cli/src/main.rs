//! `inventor-cli`: look into Inventor files from the command line.
//!
//! ```text
//! inventor-cli info  <file>              kind, version, iProperties, segments, mesh summary
//! inventor-cli json  <file>              everything as JSON
//! inventor-cli obj   <file> -o out.obj   the meshes as Wavefront OBJ (millimetres)
//! inventor-cli thumbnail <file> -o t.png the PNG thumbnail
//! ```

use inventor_read::Document;
use std::io::Write;
use std::process::ExitCode;

fn usage() -> ExitCode {
    eprintln!("usage: inventor-cli (info | json | obj | thumbnail) <file> [-o <out>]");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(cmd), Some(file)) = (args.first(), args.get(1)) else { return usage() };
    let out = args.iter().position(|a| a == "-o").and_then(|i| args.get(i + 1));
    let doc = match Document::open(file) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{file}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let result = match cmd.as_str() {
        "info" => info(&doc),
        "json" => {
            let mut j = inventor_read::json::document(&doc);
            if let Ok(s) = doc.scene() {
                j["scene"] = inventor_read::json::scene(&s);
            }
            println!("{}", serde_json::to_string_pretty(&j).unwrap_or_default());
            Ok(())
        }
        "obj" => obj(&doc, out),
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

fn info(doc: &Document) -> Result<(), String> {
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
    match doc.scene() {
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

fn obj(doc: &Document, out: Option<&String>) -> Result<(), String> {
    let out = out.ok_or("-o <file.obj> needed")?;
    let scene = doc.scene().map_err(|e| e.to_string())?;
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
