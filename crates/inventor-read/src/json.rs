//! JSON export of what a document describes besides its geometry.

use crate::oleps::Value;
use crate::{Document, Scene};
use serde_json::{json, Map, Value as Json};

fn value(v: &Value) -> Json {
    match v {
        Value::Empty | Value::Null => Json::Null,
        Value::Str(s) | Value::Clsid(s) => json!(s),
        Value::Bool(b) => json!(b),
        Value::FileTime(_) | Value::Error(_) | Value::Unsupported(_) => json!(v.display()),
        Value::Blob(b) | Value::ClipboardData(b) => json!({ "bytes": b.len() }),
        Value::Vector(items) => Json::Array(items.iter().map(value).collect()),
        Value::Currency(c) => json!(*c as f64 / 10_000.0),
        Value::Date(d) => json!(d),
        other => other.as_f64().map(|f| json!(f)).unwrap_or(Json::Null),
    }
}

/// The document: type, version, the named iProperties, every property set, segments,
/// references and warnings. The thumbnail is described by its size only.
pub fn document(doc: &Document) -> Json {
    let sets: Vec<Json> = doc
        .property_sets
        .iter()
        .map(|s| {
            json!({
                "name": s.name,
                "fmtid": s.set.fmtid,
                "stream": s.stream,
                "properties": s.set.properties.iter().map(|p| {
                    let mut o = Map::new();
                    o.insert("id".into(), json!(p.id));
                    if let Some(n) = &p.name {
                        o.insert("name".into(), json!(n));
                    }
                    o.insert("type".into(), json!(p.value.type_name()));
                    o.insert("value".into(), value(&p.value));
                    Json::Object(o)
                }).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "kind": doc.kind,
        "sheet_metal": doc.sheet_metal,
        "version": doc.version(),
        "iproperties": doc.iproperties,
        "property_sets": sets,
        "thumbnail": doc.thumbnail.as_ref().map(|t| json!({ "width": t.width, "height": t.height, "png_bytes": t.png.len() })),
        "segments": doc.segments,
        "display_cache": doc.has_display_cache,
        "references": doc.references,
        "warnings": doc.warnings,
    })
}

/// A summary of a scene: meshes with their sizes, and the warnings.
pub fn scene(scene: &Scene) -> Json {
    json!({
        "source": scene.source,
        "meshes": scene.meshes.iter().map(|m| json!({ "name": m.name, "vertices": m.vertex_count(), "triangles": m.triangle_count() })).collect::<Vec<_>>(),
        "warnings": scene.warnings,
        "faces": scene.brep.as_ref().map(|s| json!({
            "total": s.faces, "tessellated": s.tessellated, "skipped": s.skipped(),
            "unsupported_surface": s.unsupported_surface, "unsupported_curve": s.unsupported_curve,
            "failed": s.failed, "reoriented": s.reoriented,
        })),
        "cache": scene.cache.as_ref().map(|c| json!({
            "vertex_buffers": c.vertex_buffers, "line_buffers": c.line_buffers, "invalid": c.invalid, "lods_dropped": c.lods_dropped,
        })),
    })
}
