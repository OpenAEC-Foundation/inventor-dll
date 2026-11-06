//! The C ABI of `inventor-read` (header: `include/inventor.h`).
//!
//! Every function catches panics and checks its pointers; strings and buffers returned belong
//! to the document and stay valid until `inventor_close`. Errors: a null or false return, with
//! the message in `inventor_last_error`.

use inventor_read::{Document, Scene};
use std::cell::RefCell;
use std::ffi::{c_char, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;

/// An opened document with the strings and meshes handed out.
pub struct InventorDocument {
    doc: Document,
    properties: Option<CString>,
    /// The warnings handed out, per state (0 open, 1 meshes read, 2 meshes failed); a string
    /// once returned stays until close.
    warnings: Vec<(u8, CString)>,
    scene: Option<Result<FfiScene, CString>>,
}

struct FfiScene {
    scene: Scene,
    names: Vec<CString>,
}

/// One mesh: millimetres, unit normals, a triangle list.
#[repr(C)]
pub struct InventorMesh {
    pub name: *const c_char,
    /// xyz per vertex (`vertex_count` × 3 floats), mm.
    pub positions: *const f32,
    /// xyz per vertex, same count as `positions`.
    pub normals: *const f32,
    pub vertex_count: usize,
    pub indices: *const u32,
    pub index_count: usize,
}

thread_local! {
    static LAST_ERROR: RefCell<CString> = RefCell::new(CString::default());
}

fn set_error(msg: impl Into<String>) {
    let c = CString::new(msg.into().replace('\0', " ")).unwrap_or_default();
    LAST_ERROR.with(|e| *e.borrow_mut() = c);
}

fn cstring(s: String) -> CString {
    CString::new(s.replace('\0', " ")).unwrap_or_default()
}

fn guard<T>(fallback: T, f: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| {
        set_error("internal error (panic) in inventor-read");
        fallback
    })
}

/// The message of the last error on this thread (empty when there was none).
#[no_mangle]
pub extern "C" fn inventor_last_error() -> *const c_char {
    LAST_ERROR.with(|e| e.borrow().as_ptr())
}

/// The library version, e.g. `0.1.0`.
#[no_mangle]
pub extern "C" fn inventor_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast()
}

fn open(doc: Result<Document, inventor_read::Error>) -> *mut InventorDocument {
    match doc {
        Ok(doc) => Box::into_raw(Box::new(InventorDocument { doc, properties: None, warnings: Vec::new(), scene: None })),
        Err(e) => {
            set_error(e.to_string());
            ptr::null_mut()
        }
    }
}

/// Opens a file (UTF-8 path). Null on error.
///
/// # Safety
/// `path` is a NUL-terminated string.
#[no_mangle]
pub unsafe extern "C" fn inventor_open(path: *const c_char) -> *mut InventorDocument {
    guard(ptr::null_mut(), || {
        if path.is_null() {
            set_error("path is null");
            return ptr::null_mut();
        }
        let Ok(path) = CStr::from_ptr(path).to_str() else {
            set_error("path is not UTF-8");
            return ptr::null_mut();
        };
        open(Document::open(path))
    })
}

/// Opens a file from memory (the bytes are copied). Null on error.
///
/// # Safety
/// `data` points to `len` readable bytes.
#[no_mangle]
pub unsafe extern "C" fn inventor_open_bytes(data: *const u8, len: usize) -> *mut InventorDocument {
    guard(ptr::null_mut(), || {
        if data.is_null() {
            set_error("data is null");
            return ptr::null_mut();
        }
        open(Document::from_bytes(std::slice::from_raw_parts(data, len).to_vec()))
    })
}

/// Closes a document and frees everything it handed out.
///
/// # Safety
/// `doc` comes from `inventor_open*` and is not used afterwards.
#[no_mangle]
pub unsafe extern "C" fn inventor_close(doc: *mut InventorDocument) {
    if !doc.is_null() {
        let _ = catch_unwind(AssertUnwindSafe(|| drop(Box::from_raw(doc))));
    }
}

/// The document as JSON: kind, version, iProperties (named and all property sets), segments,
/// references, warnings.
///
/// # Safety
/// `doc` comes from `inventor_open*`.
#[no_mangle]
pub unsafe extern "C" fn inventor_properties(doc: *mut InventorDocument) -> *const c_char {
    guard(ptr::null(), || {
        let Some(d) = doc.as_mut() else {
            set_error("document is null");
            return ptr::null();
        };
        d.properties.get_or_insert_with(|| cstring(inventor_read::json::document(&d.doc).to_string())).as_ptr()
    })
}

/// The PNG thumbnail (bytes unchanged); null when the file has none.
///
/// # Safety
/// `doc` comes from `inventor_open*`; `len` is writable or null.
#[no_mangle]
pub unsafe extern "C" fn inventor_thumbnail_png(doc: *mut InventorDocument, len: *mut usize) -> *const u8 {
    guard(ptr::null(), || {
        let Some(d) = doc.as_ref() else {
            set_error("document is null");
            return ptr::null();
        };
        match &d.doc.thumbnail {
            Some(t) => {
                if let Some(l) = len.as_mut() {
                    *l = t.png.len();
                }
                t.png.as_ptr()
            }
            None => {
                if let Some(l) = len.as_mut() {
                    *l = 0;
                }
                set_error("no thumbnail");
                ptr::null()
            }
        }
    })
}

fn scene(d: &mut InventorDocument) -> Result<&FfiScene, &CStr> {
    if d.scene.is_none() {
        let s = d
            .doc
            .scene()
            .map(|scene| FfiScene { names: scene.meshes.iter().map(|m| cstring(m.name.clone())).collect(), scene })
            .map_err(|e| cstring(e.to_string()));
        d.scene = Some(s);
    }
    d.scene.as_ref().unwrap().as_ref().map_err(|e| e.as_c_str())
}

/// The number of meshes (tessellates the part or reads the display cache on the first call);
/// −1 on error.
///
/// # Safety
/// `doc` comes from `inventor_open*`.
#[no_mangle]
pub unsafe extern "C" fn inventor_mesh_count(doc: *mut InventorDocument) -> isize {
    guard(-1, || {
        let Some(d) = doc.as_mut() else {
            set_error("document is null");
            return -1;
        };
        match scene(d) {
            Ok(s) => s.scene.meshes.len() as isize,
            Err(e) => {
                set_error(e.to_string_lossy());
                -1
            }
        }
    })
}

/// Mesh `index` into `out`; false when out of range or on error.
///
/// # Safety
/// `doc` comes from `inventor_open*`; `out` is writable.
#[no_mangle]
pub unsafe extern "C" fn inventor_mesh(doc: *mut InventorDocument, index: usize, out: *mut InventorMesh) -> bool {
    guard(false, || {
        let (Some(d), Some(out)) = (doc.as_mut(), out.as_mut()) else {
            set_error("null argument");
            return false;
        };
        let s = match scene(d) {
            Ok(s) => s,
            Err(e) => {
                set_error(e.to_string_lossy());
                return false;
            }
        };
        let (Some(m), Some(name)) = (s.scene.meshes.get(index), s.names.get(index)) else {
            set_error("mesh index out of range");
            return false;
        };
        *out = InventorMesh {
            name: name.as_ptr(),
            positions: m.positions.as_ptr(),
            normals: m.normals.as_ptr(),
            vertex_count: m.vertex_count(),
            indices: m.indices.as_ptr(),
            index_count: m.indices.len(),
        };
        true
    })
}

/// The warnings as a JSON array of strings: those of opening and, once meshes were asked
/// for, of the meshes (e.g. `Inventor: 3 faces skipped (unsupported surface: …)`).
///
/// # Safety
/// `doc` comes from `inventor_open*`.
#[no_mangle]
pub unsafe extern "C" fn inventor_warnings(doc: *mut InventorDocument) -> *const c_char {
    guard(ptr::null(), || {
        let Some(d) = doc.as_mut() else {
            set_error("document is null");
            return ptr::null();
        };
        let (state, list) = match &d.scene {
            Some(Ok(s)) => (1, s.scene.warnings.clone()),
            Some(Err(e)) => {
                let mut w = d.doc.warnings.clone();
                w.push(e.to_string_lossy().into_owned());
                (2, w)
            }
            None => (0, d.doc.warnings.clone()),
        };
        if let Some((_, s)) = d.warnings.iter().find(|(k, _)| *k == state) {
            return s.as_ptr();
        }
        d.warnings.push((state, cstring(serde_json::to_string(&list).unwrap_or_default())));
        d.warnings.last().unwrap().1.as_ptr()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use inventor_read::synth;

    #[test]
    fn round_trip_through_the_c_abi() {
        let bytes = synth::part(&synth::Solid::cube(1.0), 1, 228, synth::Ending::EndRecord);
        unsafe {
            let doc = inventor_open_bytes(bytes.as_ptr(), bytes.len());
            assert!(!doc.is_null());
            let props = CStr::from_ptr(inventor_properties(doc)).to_str().unwrap();
            assert!(props.contains("Bracket-01"), "{props}");
            let mut len = 0;
            assert!(!inventor_thumbnail_png(doc, &mut len).is_null());
            assert!(len > 8);
            assert_eq!(inventor_mesh_count(doc), 1);
            let mut m = std::mem::zeroed::<InventorMesh>();
            assert!(inventor_mesh(doc, 0, &mut m));
            assert_eq!(m.index_count, 36);
            assert!(!inventor_mesh(doc, 1, &mut m));
            let w = CStr::from_ptr(inventor_warnings(doc)).to_str().unwrap();
            assert_eq!(w, "[]");
            // a pointer handed out stays valid, and the same state gives the same string
            assert_eq!(inventor_warnings(doc), w.as_ptr().cast());
            inventor_close(doc);
            assert!(inventor_open(ptr::null()).is_null());
            let junk = [1u8, 2, 3];
            assert!(inventor_open_bytes(junk.as_ptr(), 3).is_null());
            assert!(CStr::from_ptr(inventor_last_error()).to_str().unwrap().contains("not an Inventor file"));
        }
    }
}
