//! Property set streams after Microsoft [MS-OLEPS]: header, one or two property sets, typed
//! values and the dictionary of property names. Nothing here is specific to Inventor.

use crate::bytes::{filetime_iso, guid, latin1, utf16, Rd};

/// A property value ([MS-OLEPS] 2.15 TypedPropertyValue).
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Empty,
    Null,
    I1(i8),
    I2(i16),
    I4(i32),
    I8(i64),
    UI1(u8),
    UI2(u16),
    UI4(u32),
    UI8(u64),
    R4(f32),
    R8(f64),
    /// VT_CY: 64-bit integer in units of 1/10 000.
    Currency(i64),
    /// VT_DATE: days since 1899-12-30.
    Date(f64),
    Bool(bool),
    Error(u32),
    /// VT_BSTR, VT_LPSTR, VT_LPWSTR.
    Str(String),
    /// VT_FILETIME: 100 ns ticks since 1601-01-01 UTC.
    FileTime(u64),
    Blob(Vec<u8>),
    /// VT_CF: the bytes after the size field (format, then data).
    ClipboardData(Vec<u8>),
    Clsid(String),
    Vector(Vec<Value>),
    /// A type this reader does not decode (the VT number).
    Unsupported(u16),
}

impl Value {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            Value::Clsid(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        Some(match *self {
            Value::I1(v) => v.into(),
            Value::I2(v) => v.into(),
            Value::I4(v) => v.into(),
            Value::I8(v) => v as f64,
            Value::UI1(v) => v.into(),
            Value::UI2(v) => v.into(),
            Value::UI4(v) => v.into(),
            Value::UI8(v) => v as f64,
            Value::R4(v) => v.into(),
            Value::R8(v) => v,
            _ => return None,
        })
    }

    pub fn as_u64(&self) -> Option<u64> {
        match *self {
            Value::I1(v) => u64::try_from(v).ok(),
            Value::I2(v) => u64::try_from(v).ok(),
            Value::I4(v) => u64::try_from(v).ok(),
            Value::I8(v) => u64::try_from(v).ok(),
            Value::UI1(v) => Some(v.into()),
            Value::UI2(v) => Some(v.into()),
            Value::UI4(v) => Some(v.into()),
            Value::UI8(v) => Some(v),
            _ => None,
        }
    }

    /// The VT type name, for JSON.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Empty => "VT_EMPTY",
            Value::Null => "VT_NULL",
            Value::I1(_) => "VT_I1",
            Value::I2(_) => "VT_I2",
            Value::I4(_) => "VT_I4",
            Value::I8(_) => "VT_I8",
            Value::UI1(_) => "VT_UI1",
            Value::UI2(_) => "VT_UI2",
            Value::UI4(_) => "VT_UI4",
            Value::UI8(_) => "VT_UI8",
            Value::R4(_) => "VT_R4",
            Value::R8(_) => "VT_R8",
            Value::Currency(_) => "VT_CY",
            Value::Date(_) => "VT_DATE",
            Value::Bool(_) => "VT_BOOL",
            Value::Error(_) => "VT_ERROR",
            Value::Str(_) => "VT_LPWSTR",
            Value::FileTime(_) => "VT_FILETIME",
            Value::Blob(_) => "VT_BLOB",
            Value::ClipboardData(_) => "VT_CF",
            Value::Clsid(_) => "VT_CLSID",
            Value::Vector(_) => "VT_VECTOR",
            Value::Unsupported(_) => "unsupported",
        }
    }

    /// A short text for display (dates as ISO 8601, binary values as their size).
    pub fn display(&self) -> String {
        match self {
            Value::Empty | Value::Null => String::new(),
            Value::Str(s) | Value::Clsid(s) => s.clone(),
            Value::FileTime(t) => filetime_iso(*t),
            Value::Bool(b) => b.to_string(),
            Value::Currency(c) => format!("{}", *c as f64 / 10_000.0),
            Value::Date(d) => d.to_string(),
            Value::Error(e) => format!("error 0x{e:08X}"),
            Value::Blob(b) | Value::ClipboardData(b) => format!("{} bytes", b.len()),
            Value::Vector(v) => v.iter().map(Value::display).collect::<Vec<_>>().join("; "),
            Value::Unsupported(t) => format!("(type 0x{t:04X})"),
            other => other.as_f64().map(|v| v.to_string()).unwrap_or_default(),
        }
    }
}

/// One property: identifier, the name from the dictionary (if any) and the value.
#[derive(Debug, Clone, PartialEq)]
pub struct Property {
    pub id: u32,
    pub name: Option<String>,
    pub value: Value,
}

/// One property set of a stream.
#[derive(Debug, Clone, PartialEq)]
pub struct PropertySet {
    /// The FMTID, e.g. `32853F0F-3444-11D1-9E93-0060B03C1CA6`.
    pub fmtid: String,
    /// The code page (property 1), 1200 = UTF-16.
    pub codepage: u16,
    pub properties: Vec<Property>,
}

impl PropertySet {
    pub fn get(&self, id: u32) -> Option<&Value> {
        self.properties.iter().find(|p| p.id == id).map(|p| &p.value)
    }
}

const MAX_PROPERTIES: usize = 100_000;
const MAX_VECTOR: usize = 1_000_000;

/// Parses a property set stream: the sets it holds (one, or two for the DocumentSummary
/// with user-defined properties). `None` when the header is not [MS-OLEPS].
pub fn parse_stream(data: &[u8]) -> Option<Vec<PropertySet>> {
    let mut r = Rd::new(data);
    if r.u16()? != 0xFFFE {
        return None;
    }
    let _version = r.u16()?;
    let _system = r.u32()?;
    r.skip(16)?; // CLSID
    let count = r.u32()? as usize;
    if count == 0 || count > 16 {
        return None;
    }
    let mut sets = Vec::new();
    for _ in 0..count {
        let fmtid = guid(r.bytes(16)?);
        let offset = r.u32()? as usize;
        if let Some(set) = parse_set(data, offset, fmtid) {
            sets.push(set);
        }
    }
    Some(sets)
}

fn parse_set(data: &[u8], start: usize, fmtid: String) -> Option<PropertySet> {
    let mut r = Rd::at(data, start);
    let size = r.u32()? as usize;
    let end = start.checked_add(size)?.min(data.len());
    let set = data.get(start..end)?;
    let n = (r.u32()? as usize).min(MAX_PROPERTIES);
    let mut ids = Vec::with_capacity(n.min(1024));
    for _ in 0..n {
        let (Some(id), Some(off)) = (r.u32(), r.u32()) else { break };
        ids.push((id, off as usize));
    }
    // the code page decides how strings and dictionary names are stored
    let codepage = ids
        .iter()
        .find(|(id, _)| *id == 1)
        .and_then(|&(_, off)| read_value(set, off, 1200))
        .and_then(|v| {
            v.as_u64().or(match v {
                Value::I2(c) => Some(c as u16 as u64),
                _ => None,
            })
        })
        .unwrap_or(1252) as u16;
    let names = ids.iter().find(|(id, _)| *id == 0).and_then(|&(_, off)| dictionary(set, off, codepage)).unwrap_or_default();
    let mut properties = Vec::new();
    for &(id, off) in &ids {
        if id == 0 {
            continue;
        }
        let value = read_value(set, off, codepage).unwrap_or(Value::Unsupported(0xFFFF));
        let name = names.iter().find(|(i, _)| *i == id).map(|(_, n)| n.clone());
        properties.push(Property { id, name, value });
    }
    Some(PropertySet { fmtid, codepage, properties })
}

fn dictionary(set: &[u8], off: usize, codepage: u16) -> Option<Vec<(u32, String)>> {
    let mut r = Rd::at(set, off);
    let n = (r.u32()? as usize).min(MAX_PROPERTIES);
    let mut out = Vec::new();
    for _ in 0..n {
        let (Some(id), Some(len)) = (r.u32(), r.u32()) else { break };
        let len = len as usize;
        let name = if codepage == 1200 {
            let s = r.bytes(len.checked_mul(2)?)?;
            // Unicode entries are padded to a multiple of 4 bytes
            let pad = (4 - (2 * len) % 4) % 4;
            r.skip(pad.min(r.remaining()))?;
            utf16(s)
        } else {
            string_bytes(r.bytes(len)?, codepage)
        };
        out.push((id, name));
    }
    Some(out)
}

fn string_bytes(b: &[u8], codepage: u16) -> String {
    match codepage {
        1200 => utf16(b),
        65001 => String::from_utf8_lossy(b.split(|&c| c == 0).next().unwrap_or(&[])).into_owned(),
        _ => latin1(b.split(|&c| c == 0).next().unwrap_or(&[])),
    }
}

fn read_value(set: &[u8], off: usize, codepage: u16) -> Option<Value> {
    let mut r = Rd::at(set, off);
    let vt = r.u16()?;
    r.skip(2)?;
    if vt & 0x1000 != 0 {
        let n = r.u32()? as usize;
        if n > MAX_VECTOR {
            return None;
        }
        let mut items = Vec::with_capacity(n.min(1024));
        for _ in 0..n {
            items.push(scalar(&mut r, vt & 0x0FFF, codepage, true)?);
        }
        return Some(Value::Vector(items));
    }
    scalar(&mut r, vt, codepage, false)
}

/// Aligns the reader to 4 bytes from the start of the set (values are padded to 4 bytes).
fn align4(r: &mut Rd) {
    let pad = (4 - r.p % 4) % 4;
    r.p = (r.p + pad).min(r.d.len());
}

fn scalar(r: &mut Rd, vt: u16, codepage: u16, in_vector: bool) -> Option<Value> {
    let v = match vt {
        0x00 => Value::Empty,
        0x01 => Value::Null,
        0x02 => {
            let v = Value::I2(r.i16()?);
            if !in_vector {
                r.skip(2)?;
            }
            v
        }
        0x03 | 0x16 => Value::I4(r.i32()?),
        0x04 => Value::R4(r.f32()?),
        0x05 => Value::R8(r.f64()?),
        0x06 => Value::Currency(r.u64()? as i64),
        0x07 => Value::Date(r.f64()?),
        0x08 | 0x1E => {
            let n = r.u32()? as usize;
            let s = string_bytes(r.bytes(n)?, codepage);
            align4(r);
            Value::Str(s)
        }
        0x0A => Value::Error(r.u32()?),
        0x0B => {
            let v = Value::Bool(r.u16()? != 0);
            if !in_vector {
                r.skip(2)?;
            }
            v
        }
        0x0C => {
            // VT_VARIANT (in vectors): a nested typed value
            let t = r.u16()?;
            r.skip(2)?;
            scalar(r, t, codepage, false)?
        }
        0x10 => {
            let v = Value::I1(r.u8()? as i8);
            if !in_vector {
                r.skip(3)?;
            }
            v
        }
        0x11 => {
            let v = Value::UI1(r.u8()?);
            if !in_vector {
                r.skip(3)?;
            }
            v
        }
        0x12 => {
            let v = Value::UI2(r.u16()?);
            if !in_vector {
                r.skip(2)?;
            }
            v
        }
        0x13 | 0x17 => Value::UI4(r.u32()?),
        0x14 => Value::I8(r.u64()? as i64),
        0x15 => Value::UI8(r.u64()?),
        0x1F => {
            let n = r.u32()? as usize;
            let s = utf16(r.bytes(n.checked_mul(2)?)?);
            align4(r);
            Value::Str(s)
        }
        0x40 => Value::FileTime(r.u64()?),
        0x41 | 0x46 => {
            let n = r.u32()? as usize;
            let b = r.bytes(n)?.to_vec();
            align4(r);
            Value::Blob(b)
        }
        0x47 => {
            let n = r.u32()? as usize;
            let b = r.bytes(n.min(r.remaining()))?.to_vec();
            align4(r);
            Value::ClipboardData(b)
        }
        0x48 => Value::Clsid(guid(r.bytes(16)?)),
        other => Value::Unsupported(other),
    };
    Some(v)
}
