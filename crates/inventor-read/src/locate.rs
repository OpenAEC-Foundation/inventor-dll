//! Finding the files an assembly refers to on this computer. The assembly stores each path as
//! it was on the computer it was saved on, so the file is looked for by the end of that path:
//! first with as many of its folders as possible, below the folder of the assembly and the
//! folders above it, then by its name alone anywhere below the folder of the assembly.

use crate::assembly::FileRef;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Folders above the assembly that are tried.
const LEVELS_UP: usize = 8;
/// Folders read while looking for a file by name.
const MAX_FOLDERS: usize = 5_000;

/// Finds referenced files next to an assembly.
pub struct Locator {
    folder: PathBuf,
    /// Lower-case file name → path, of the Inventor files below `folder`; made when first needed.
    by_name: Option<HashMap<String, PathBuf>>,
}

impl Locator {
    /// For the assembly at `assembly`.
    pub fn new(assembly: &Path) -> Self {
        let folder = assembly.parent().map(Path::to_path_buf).unwrap_or_default();
        Self { folder, by_name: None }
    }

    pub fn find(&mut self, file: &FileRef) -> Option<PathBuf> {
        let parts: Vec<&str> = file.path.split(['\\', '/']).filter(|p| !p.is_empty() && !p.ends_with(':') && *p != "." && *p != "..").collect();
        let name = *parts.last()?;
        // with its folders, the most folders first
        for keep in (1..=parts.len()).rev() {
            let tail: PathBuf = parts[parts.len() - keep..].iter().collect();
            let mut base = Some(self.folder.as_path());
            for _ in 0..=LEVELS_UP {
                let Some(b) = base else { break };
                let candidate = b.join(&tail);
                if candidate.is_file() {
                    return Some(candidate);
                }
                base = b.parent();
            }
        }
        let folder = self.folder.clone();
        self.by_name.get_or_insert_with(|| index(&folder)).get(&name.to_lowercase()).cloned()
    }

    /// The bytes of a referenced file, when it is found and can be read.
    pub fn read(&mut self, file: &FileRef) -> Option<Vec<u8>> {
        std::fs::read(self.find(file)?).ok()
    }
}

/// The Inventor files below a folder by lower-case name; of two with one name the one nearest
/// to the folder.
fn index(folder: &Path) -> HashMap<String, PathBuf> {
    let mut out = HashMap::new();
    let mut level = vec![folder.to_path_buf()];
    let mut seen = 0;
    while !level.is_empty() && seen < MAX_FOLDERS {
        let mut next = Vec::new();
        for dir in level {
            seen += 1;
            if seen > MAX_FOLDERS {
                break;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            let mut entries: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
            entries.sort();
            for path in entries {
                let Some(name) = path.file_name().map(|n| n.to_string_lossy().to_lowercase()) else { continue };
                if path.is_dir() {
                    // Inventor keeps earlier saves of a file in OldVersions
                    if name != "oldversions" && !name.starts_with('.') {
                        next.push(path);
                    }
                } else if name.ends_with(".ipt") || name.ends_with(".iam") {
                    out.entry(name).or_insert(path);
                }
            }
        }
        level = next;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str) -> FileRef {
        FileRef { id: 1, path: path.into(), relative: None, display_name: String::new(), occurrences: 1 }
    }

    #[test]
    fn by_folders_then_by_name() {
        let root = std::env::temp_dir().join(format!("inventor-locate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["project/frame", "project/parts/bolts", "project/frame/deep/er"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        for f in ["project/parts/bolts/M4.ipt", "project/frame/Leg.ipt", "project/frame/deep/er/Odd.ipt"] {
            std::fs::write(root.join(f), b"x").unwrap();
        }
        let mut l = Locator::new(&root.join("project/frame/Frame.iam"));
        // next to the assembly
        assert_eq!(l.find(&file("C:\\Work\\project\\frame\\Leg.ipt")), Some(root.join("project/frame").join("Leg.ipt")));
        // through the folders of the stored path, from a folder above the assembly
        assert_eq!(
            l.find(&file("C:\\Work\\project\\parts\\bolts\\M4.ipt")).map(|p| p.canonicalize().unwrap()),
            Some(root.join("project/parts/bolts/M4.ipt").canonicalize().unwrap())
        );
        // by name below the assembly
        assert_eq!(
            l.find(&file("D:\\Elsewhere\\Odd.ipt")).map(|p| p.canonicalize().unwrap()),
            Some(root.join("project/frame/deep/er/Odd.ipt").canonicalize().unwrap())
        );
        assert_eq!(l.find(&file("D:\\Elsewhere\\Gone.ipt")), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
