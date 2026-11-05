//! iProperties: the property set streams at the top of the file (SPEC §2), the named fields of
//! SPEC §2.2 and the thumbnail (§2.3).

use crate::oleps::{PropertySet, Value};
use serde::Serialize;

pub const DESIGN_TRACKING_PROPERTIES: &str = "32853F0F-3444-11D1-9E93-0060B03C1CA6";
pub const DESIGN_TRACKING_CONTROL: &str = "D861FB30-3136-11D1-9E92-0060B03C1CA6";
pub const INVENTOR_SUMMARY: &str = "3D38DE39-0588-4C14-BB37-18F4D5DD31C7";
pub const INVENTOR_DOC_SUMMARY: &str = "8CF58000-DA66-4AE6-8FF0-7B58406FB049";
pub const INVENTOR_USER_DEFINED: &str = "9929ADB8-6407-413E-B3DC-CB9AD2F564B7";
pub const PRIVATE_MODEL_INFORMATION: &str = "BB586990-AF3E-11D3-95A9-00A0C9B6E37A";
/// The standard sets of [MS-OLEPS], used when the Inventor ones are missing.
pub const SUMMARY_INFORMATION: &str = "F29F85E0-4FF9-1068-AB91-08002B27B3D9";
pub const DOC_SUMMARY_INFORMATION: &str = "D5CDD502-2E9C-101B-9397-08002B2CF9AE";
pub const USER_DEFINED_PROPERTIES: &str = "D5CDD505-2E9C-101B-9397-08002B2CF9AE";

/// The name of a set: from its FMTID, else from property 255 in the set, else the FMTID.
pub fn set_name(set: &PropertySet) -> String {
    let known = match set.fmtid.as_str() {
        DESIGN_TRACKING_PROPERTIES => "Design Tracking Properties",
        DESIGN_TRACKING_CONTROL => "Design Tracking Control",
        INVENTOR_SUMMARY => "Inventor Summary Information",
        INVENTOR_DOC_SUMMARY => "Inventor Document Summary Information",
        INVENTOR_USER_DEFINED => "Inventor User Defined Properties",
        PRIVATE_MODEL_INFORMATION => "_Private Model Information",
        SUMMARY_INFORMATION => "SummaryInformation",
        DOC_SUMMARY_INFORMATION => "DocumentSummaryInformation",
        USER_DEFINED_PROPERTIES => "User Defined Properties",
        _ => "",
    };
    if !known.is_empty() {
        return known.to_string();
    }
    set.get(255).and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| set.fmtid.clone())
}

/// A property set read from the file, with its stream name and set name.
#[derive(Debug, Clone, PartialEq)]
pub struct NamedSet {
    /// The stream name without the leading U+0005.
    pub stream: String,
    pub name: String,
    pub set: PropertySet,
}

/// The iProperties with a known meaning (SPEC §2.2). Empty strings are left out.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct IProperties {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comments: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub company: Option<String>,
    /// Creation date, ISO 8601 (UTC).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub part_number: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub designer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub material: Option<String>,
    /// Path and key in the material library.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub material_library: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub appearance: Option<String>,
    /// The active colour style (from `_Private Model Information`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_style: Option<String>,
    /// Density in g/cm³.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub density: Option<f64>,
    /// Document subtype: `Modeling`, `Sheet Metal`, `Assembly`, `Drawing Layout`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_subtype: Option<String>,
    /// The subtype as a GUID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub document_subtype_id: Option<String>,
    /// Version text, e.g. `2023 (Build 270158000, 158)`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version_text: Option<String>,
    /// Build number, e.g. 270158000.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build: Option<u64>,
    /// Standard parts: standard, revision, organisation, size designation (pid 37, 47, 49, 52).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub standard: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub standard_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub standards_organization: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_designation: Option<String>,
}

impl IProperties {
    pub fn from_sets(sets: &[NamedSet]) -> Self {
        let find = |fmtid: &str| sets.iter().find(|s| s.set.fmtid == fmtid).map(|s| &s.set);
        let text = |fmtid: &str, id: u32| -> Option<String> {
            find(fmtid)?.get(id).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
        };
        // the Inventor summary sets follow the numbering of the standard ones
        let summary = |id| text(INVENTOR_SUMMARY, id).or_else(|| text(SUMMARY_INFORMATION, id));
        let doc_summary = |id| text(INVENTOR_DOC_SUMMARY, id).or_else(|| text(DOC_SUMMARY_INFORMATION, id));
        let dtp = |id| text(DESIGN_TRACKING_PROPERTIES, id);
        let dtp_value = |id| find(DESIGN_TRACKING_PROPERTIES).and_then(|s| s.get(id));
        Self {
            title: summary(2),
            author: summary(4),
            comments: summary(6),
            revision: summary(9),
            company: doc_summary(15),
            created: dtp_value(4).and_then(|v| match v {
                Value::FileTime(t) if *t > 0 => Some(crate::bytes::filetime_iso(*t)),
                _ => None,
            }),
            part_number: dtp(5),
            description: dtp(29),
            designer: dtp(41),
            material: dtp(20),
            material_library: dtp(71),
            appearance: dtp(72),
            color_style: text(PRIVATE_MODEL_INFORMATION, 16).and_then(|xml| style_name(&xml)),
            density: dtp_value(61).and_then(Value::as_f64),
            document_subtype: dtp(32),
            document_subtype_id: dtp(31),
            version_text: dtp(67),
            build: find(DESIGN_TRACKING_CONTROL).and_then(|s| s.get(22)).and_then(Value::as_u64),
            standard: dtp(37),
            standard_revision: dtp(47),
            standards_organization: dtp(49),
            size_designation: dtp(52),
        }
    }

    /// The named fields as (label, value) rows, for display.
    pub fn rows(&self) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        let mut push = |k: &'static str, v: &Option<String>| {
            if let Some(v) = v {
                out.push((k, v.clone()));
            }
        };
        push("Part number", &self.part_number);
        push("Title", &self.title);
        push("Description", &self.description);
        push("Author", &self.author);
        push("Designer", &self.designer);
        push("Company", &self.company);
        push("Revision", &self.revision);
        push("Comments", &self.comments);
        push("Created", &self.created);
        push("Material", &self.material);
        push("Appearance", &self.appearance);
        push("Colour style", &self.color_style);
        push("Document subtype", &self.document_subtype);
        push("Standard", &self.standard);
        push("Standard revision", &self.standard_revision);
        push("Standards organization", &self.standards_organization);
        push("Size designation", &self.size_designation);
        if let Some(d) = self.density {
            out.push(("Density", format!("{d} g/cm³")));
        }
        out
    }
}

/// The `<StyleName>` of the colour style XML.
fn style_name(xml: &str) -> Option<String> {
    let start = xml.find("<StyleName>")? + "<StyleName>".len();
    let end = xml[start..].find('<')? + start;
    Some(xml[start..end].trim().to_string()).filter(|s| !s.is_empty())
}

/// The PNG thumbnail in Summary pid 17 (VT_CF, SPEC §2.3).
#[derive(Debug, Clone, PartialEq)]
pub struct Thumbnail {
    pub width: u32,
    pub height: u32,
    /// The PNG file, unchanged.
    pub png: Vec<u8>,
}

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

pub fn thumbnail(sets: &[NamedSet]) -> Option<Thumbnail> {
    let cf = [INVENTOR_SUMMARY, SUMMARY_INFORMATION].iter().find_map(|f| {
        sets.iter().filter(|s| s.set.fmtid == *f).find_map(|s| match s.set.get(17) {
            Some(Value::ClipboardData(b)) => Some(b.as_slice()),
            _ => None,
        })
    })?;
    // i32 -1 (clipboard format) | u32 3 | u16 8 | u16 width | u16 height | u16 0 | PNG
    let mut r = crate::bytes::Rd::new(cf);
    let (format, _kind, _eight, w, h) = (r.i32()?, r.u32()?, r.u16()?, r.u16()?, r.u16()?);
    let png_at = if cf.get(14..22) == Some(&PNG_SIGNATURE[..]) {
        14
    } else {
        // tolerate other header lengths: the PNG signature itself
        cf.windows(8).position(|w| w == PNG_SIGNATURE)?
    };
    let png = cf[png_at..].to_vec();
    // the PNG header knows the size too (IHDR after the signature)
    let ihdr = |o: usize| png.get(o..o + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
    let (width, height) = match (ihdr(16), ihdr(20)) {
        (Some(pw), Some(ph)) if png.get(12..16) == Some(b"IHDR") => (pw, ph),
        _ if format == -1 => (w.into(), h.into()),
        _ => (0, 0),
    };
    Some(Thumbnail { width, height, png })
}
