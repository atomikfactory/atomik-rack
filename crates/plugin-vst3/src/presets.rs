//! `.vstpreset` files: the VST3 preset container, the folders hosts keep presets in, and the
//! link to a plugin's saved state.
//!
//! A file is a 48 byte header (`VST3`, version 1, the class id as 32 ASCII hex digits, the
//! offset of the chunk list), the chunk data, and at the offset the list: `List`, a count and
//! one `(id, offset, size)` entry per chunk. All numbers are little-endian. The `Comp` chunk is
//! the component state and `Cont` the controller state, the same two sections the host's own
//! state blob packs, so loading a file is a state restore.

use atomik_plugin_host::PresetInfo;
use std::path::{Component, Path, PathBuf};

const HEADER: usize = 48;
const MAGIC: &[u8; 4] = b"VST3";
/// Most files one listing returns.
const MAX_FILES: usize = 2_000;
/// Folders deeper than this under a preset root are not read.
const MAX_DEPTH: usize = 3;
/// Largest `.vstpreset` read (the state cap is 16 MiB per plugin).
const MAX_FILE_BYTES: u64 = 17 << 20;
/// Longest preset id kept (the sandbox cuts longer ones).
const MAX_ID_BYTES: usize = 150;

/// The environment variable that replaces the standard preset folders (a `;`- or
/// `:`-separated list of roots), for tests and for unusual installs.
pub(crate) const ROOTS_ENV: &str = "ATOMIK_VST3_PRESET_DIRS";

/// The component and controller sections of a file made for `class_hex`.
pub(crate) fn parse(bytes: &[u8], class_hex: &str) -> Result<(Vec<u8>, Vec<u8>), String> {
    if bytes.len() < HEADER || &bytes[..4] != MAGIC {
        return Err("not a .vstpreset file".to_string());
    }
    let class = std::str::from_utf8(&bytes[8..40]).unwrap_or("");
    if !class.eq_ignore_ascii_case(class_hex) {
        return Err("the preset is for another plugin".to_string());
    }
    let list = i64::from_le_bytes(bytes[40..48].try_into().map_err(|_| "short header")?);
    let list = usize::try_from(list).map_err(|_| "bad chunk list offset")?;
    let end = list
        .checked_add(8)
        .filter(|e| *e <= bytes.len() && &bytes[list..list.saturating_add(4)] == b"List")
        .ok_or("the chunk list is missing")?;
    let count = i32::from_le_bytes(bytes[list + 4..end].try_into().map_err(|_| "short list")?);
    let count = usize::try_from(count).map_err(|_| "bad chunk count")?;
    let (mut comp, mut cont) = (None, Vec::new());
    for i in 0..count.min(64) {
        let at = end + i * 20;
        let entry = bytes
            .get(at..at + 20)
            .ok_or("the chunk list is cut short")?;
        let offset = i64::from_le_bytes(entry[4..12].try_into().map_err(|_| "entry")?);
        let size = i64::from_le_bytes(entry[12..20].try_into().map_err(|_| "entry")?);
        let (Ok(offset), Ok(size)) = (usize::try_from(offset), usize::try_from(size)) else {
            return Err("bad chunk bounds".to_string());
        };
        let data = offset
            .checked_add(size)
            .and_then(|e| bytes.get(offset..e))
            .ok_or("a chunk lies outside the file")?;
        match &entry[..4] {
            b"Comp" => comp = Some(data.to_vec()),
            b"Cont" => cont = data.to_vec(),
            _ => {}
        }
    }
    comp.map(|c| (c, cont))
        .ok_or_else(|| "the preset has no component state".to_string())
}

/// A `.vstpreset` file for `class_hex` holding the two sections.
pub(crate) fn write(class_hex: &str, component: &[u8], controller: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER + component.len() + controller.len() + 64);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&1_i32.to_le_bytes());
    let mut class = [b'0'; 32];
    for (d, s) in class.iter_mut().zip(class_hex.bytes()) {
        *d = s.to_ascii_uppercase();
    }
    out.extend_from_slice(&class);
    out.extend_from_slice(&[0; 8]);
    let comp_at = out.len();
    out.extend_from_slice(component);
    let cont_at = out.len();
    out.extend_from_slice(controller);
    let list = out.len();
    out[40..48].copy_from_slice(&(list as i64).to_le_bytes());
    out.extend_from_slice(b"List");
    let chunks: &[(&[u8; 4], usize, usize)] = &[
        (b"Comp", comp_at, component.len()),
        (b"Cont", cont_at, controller.len()),
    ];
    let count = if controller.is_empty() { 1 } else { 2 };
    out.extend_from_slice(&(count as i32).to_le_bytes());
    for (id, at, size) in &chunks[..count] {
        out.extend_from_slice(*id);
        out.extend_from_slice(&(*at as i64).to_le_bytes());
        out.extend_from_slice(&(*size as i64).to_le_bytes());
    }
    out
}

/// The folders to look in for one plugin: the override list of [`ROOTS_ENV`] when it is set,
/// else the user and the system preset folders of the platform, each under `vendor/name`.
pub(crate) fn roots(vendor: &str, name: &str) -> Vec<PathBuf> {
    // Justification: preset folders come from the user's environment by design.
    if let Some(list) = std::env::var_os(ROOTS_ENV) {
        return std::env::split_paths(&list).collect();
    }
    let clean = |s: &str| -> String {
        s.chars()
            .map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c })
            .collect()
    };
    let tail = Path::new(&clean(vendor)).join(clean(name));
    let var = |k: &str| std::env::var_os(k).map(PathBuf::from);
    let mut bases = Vec::new();
    if cfg!(windows) {
        if let Some(h) = var("USERPROFILE") {
            bases.push(h.join("Documents").join("VST3 Presets"));
        }
        if let Some(p) = var("PROGRAMDATA") {
            bases.push(p.join("VST3 Presets"));
        }
    } else if cfg!(target_os = "macos") {
        if let Some(h) = var("HOME") {
            bases.push(h.join("Library/Audio/Presets"));
        }
        bases.push(PathBuf::from("/Library/Audio/Presets"));
    } else {
        if let Some(h) = var("HOME") {
            bases.push(h.join(".vst3/presets"));
        }
        bases.push(PathBuf::from("/usr/share/vst3/presets"));
    }
    bases.into_iter().map(|b| b.join(&tail)).collect()
}

/// Every `.vstpreset` under `roots`, sorted: the id is the path relative to its root (with
/// `/`, preceded by the root's index when there are several roots), the group its folder.
pub(crate) fn list(roots: &[PathBuf]) -> Vec<PresetInfo> {
    let mut found = Vec::new();
    for (index, root) in roots.iter().enumerate() {
        walk(root, Path::new(""), index, 0, &mut found);
    }
    found.sort();
    found.truncate(MAX_FILES);
    found
}

fn walk(root: &Path, rel: &Path, index: usize, depth: usize, out: &mut Vec<PresetInfo>) {
    let Ok(read) = std::fs::read_dir(root.join(rel)) else {
        return;
    };
    let mut entries: Vec<_> = read.filter_map(Result::ok).collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        if out.len() >= MAX_FILES {
            return;
        }
        let path = rel.join(entry.file_name());
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if depth < MAX_DEPTH {
                walk(root, &path, index, depth + 1, out);
            }
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("vstpreset"))
        {
            let parts: Vec<_> = path
                .components()
                .filter_map(|c| c.as_os_str().to_str())
                .collect();
            let id = format!("{index}/{}", parts.join("/"));
            if id.len() > MAX_ID_BYTES {
                continue;
            }
            let name = path
                .file_stem()
                .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
            let group = parts[..parts.len() - 1].join(" / ");
            out.push(PresetInfo { id, name, group });
        }
    }
}

/// The file an id of [`list`] names, refusing anything that leaves the root.
pub(crate) fn resolve(roots: &[PathBuf], id: &str) -> Option<PathBuf> {
    let (index, rel) = id.split_once('/')?;
    let root = roots.get(index.parse::<usize>().ok()?)?;
    let rel = Path::new(rel);
    rel.components()
        .all(|c| matches!(c, Component::Normal(_)))
        .then(|| root.join(rel))
}

/// Reads a preset file (bounded).
pub(crate) fn read_file(path: &Path) -> Result<Vec<u8>, String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("cannot read the preset: {e}"))?;
    if meta.len() > MAX_FILE_BYTES {
        return Err("the preset file is too large".to_string());
    }
    std::fs::read(path).map_err(|e| format!("cannot read the preset: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLASS: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn a_file_round_trips_both_sections() {
        let bytes = write(CLASS, b"component", b"controller");
        assert_eq!(
            parse(&bytes, CLASS).unwrap(),
            (b"component".to_vec(), b"controller".to_vec())
        );
        let bytes = write(CLASS, b"only", b"");
        assert_eq!(
            parse(&bytes, CLASS).unwrap(),
            (b"only".to_vec(), Vec::new())
        );
    }

    #[test]
    fn a_file_for_another_class_or_a_damaged_one_is_refused() {
        let bytes = write(CLASS, b"c", b"d");
        assert!(parse(&bytes, "ffffffffffffffffffffffffffffffff").is_err());
        assert!(parse(&bytes[..30], CLASS).is_err());
        let mut cut = bytes.clone();
        cut.truncate(bytes.len() - 5);
        assert!(parse(&cut, CLASS).is_err());
        let mut wild = bytes;
        let at = wild.len() - 8;
        wild[at..].copy_from_slice(&i64::MAX.to_le_bytes());
        assert!(parse(&wild, CLASS).is_err());
        assert!(parse(b"nope", CLASS).is_err());
    }

    #[test]
    fn ids_never_leave_their_root() {
        let roots = vec![PathBuf::from("root")];
        assert!(resolve(&roots, "0/a/b.vstpreset").is_some());
        assert!(resolve(&roots, "0/../x.vstpreset").is_none());
        assert!(resolve(&roots, "1/a.vstpreset").is_none());
        assert!(resolve(&roots, "x/a.vstpreset").is_none());
    }
}
