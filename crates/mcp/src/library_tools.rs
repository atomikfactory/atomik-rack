//! The library tool: `library_list` lists the items of a library folder (patches, containers,
//! clips and projects) with the one line summary the file browser shows. Stateless: it reads
//! files and keeps nothing.

use crate::rpc::{Server, Tool};
use crate::tool::{Args, ToolOutput, definition};
use atomik_library::{FileKind, FileRow, ItemInfo, LibrarySection, Query, classify};
use atomik_model::{ItemKind, summarize_item};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// The library tools.
pub const TOOLS: &[Tool] = &[Tool {
    name: "library_list",
    definition: library_list_def,
    run: library_list,
}];

/// The sections that hold files, in listing order.
const FILE_SECTIONS: [LibrarySection; 4] = [
    LibrarySection::Patches,
    LibrarySection::Containers,
    LibrarySection::Clips,
    LibrarySection::Projects,
];

/// Most items one call lists.
const MAX_ITEMS: usize = 1000;
/// Deepest sub-folder level listed under a section folder.
const MAX_DEPTH: usize = 8;
/// Largest item whose text is read for its summary (the browser's limit).
const MAX_SUMMARY_BYTES: u64 = 8 * 1024 * 1024;

fn library_list_def() -> Value {
    definition(
        "library_list",
        "Lists the items of a library folder like the Library sections of the file browser: the files of `Patches/` (.atomikpatch), `Containers/` (.atomikcontainer), `Clips/` (.atomikclip) and `Projects/` (.atomik), sub-folders included, a section folder that does not exist counting as empty. Each item has its `section` (patches, containers, clips, projects), `kind` (patch, container, clip, project), `name` (without the extension), `path` (relative to `folder`, with `/`), `size` in bytes and, when the file reads, a one line `detail` (`3 devices: Poly Synth, Delay, ...`, `Container, 5 inside`, `4 bars, 32 notes`, `120 BPM, 8 tracks`). Sorted by section, then path; symbolic links are not followed; at most 1000 items (`truncated` says more exist). The built-in devices are not files: see `list_devices`.",
        &[
            (
                "folder",
                "string",
                "The library folder (the one that holds Patches, Containers, Clips and Projects); inside the allowed folders.",
            ),
            (
                "section",
                "string",
                "Only this section: patches, containers, clips or projects.",
            ),
            (
                "query",
                "string",
                "Only items whose name contains every word of this text (case-insensitive).",
            ),
        ],
        &["folder"],
    )
}

/// The section called `name` (any case), or the message that lists the choices.
fn section_named(name: &str) -> Result<LibrarySection, String> {
    FILE_SECTIONS
        .into_iter()
        .find(|s| s.title().eq_ignore_ascii_case(name))
        .ok_or_else(|| {
            format!(
                "unknown section `{name}`: use patches, containers, clips or projects (the built-in devices are listed by list_devices)"
            )
        })
}

fn model_kind(kind: FileKind) -> Option<ItemKind> {
    match kind {
        FileKind::Patch => Some(ItemKind::Patch),
        FileKind::Container => Some(ItemKind::Container),
        FileKind::Clip => Some(ItemKind::Clip),
        FileKind::Project => Some(ItemKind::Project),
        _ => None,
    }
}

fn kind_name(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Patch => "patch",
        FileKind::Container => "container",
        FileKind::Clip => "clip",
        _ => "project",
    }
}

/// One file of a section folder, before its summary is read.
struct Found {
    path: PathBuf,
    rel: String,
    kind: FileKind,
    size: u64,
}

/// Every item file under `dir` that belongs to `section`, depth first. Links are skipped, so a
/// link cannot lead out of the library folder.
fn walk(dir: &Path, prefix: &str, section: LibrarySection, depth: usize, out: &mut Vec<Found>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let rel = format!("{prefix}/{name}");
        if file_type.is_dir() {
            if depth < MAX_DEPTH {
                walk(&entry.path(), &rel, section, depth + 1, out);
            }
            continue;
        }
        let path = entry.path();
        if let Some(kind) = classify(&path, false).filter(|k| section.accepts(*k)) {
            let size = entry.metadata().map_or(0, |m| m.len());
            out.push(Found {
                path,
                rel,
                kind,
                size,
            });
        }
    }
}

fn library_list(server: &mut Server, args: &Args) -> Result<ToolOutput, String> {
    let folder = server.paths().read(args.str("folder")?)?;
    if !folder.is_dir() {
        return Err(format!("{}: not a folder", folder.display()));
    }
    let sections: Vec<LibrarySection> = match args.opt_str("section")? {
        Some(name) => vec![section_named(name)?],
        None => FILE_SECTIONS.to_vec(),
    };
    let query = Query::parse(args.opt_str("query")?.unwrap_or(""));
    let mut items = Vec::new();
    let mut truncated = false;
    'sections: for section in sections {
        let Some(name) = section.folder_name() else {
            continue;
        };
        let mut found = Vec::new();
        walk(&folder.join(name), name, section, 0, &mut found);
        found.sort_by_cached_key(|f| f.rel.to_lowercase());
        for f in found {
            let stem = f
                .path
                .file_stem()
                .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
            let row = FileRow {
                path: f.path.clone(),
                name: stem.clone(),
                kind: f.kind,
                size: f.size,
                info: None,
                item: Some(ItemInfo::default()),
                favorite: false,
                tags: Vec::new(),
            };
            if !query.matches(&row) {
                continue;
            }
            if items.len() == MAX_ITEMS {
                truncated = true;
                break 'sections;
            }
            let mut v = json!({
                "section": section.title().to_lowercase(),
                "kind": kind_name(f.kind),
                "name": stem,
                "path": f.rel,
                "size": f.size,
            });
            let detail = (f.size <= MAX_SUMMARY_BYTES)
                .then(|| std::fs::read_to_string(&f.path).ok())
                .flatten()
                .zip(model_kind(f.kind))
                .and_then(|(text, kind)| summarize_item(kind, &text));
            if let Some(d) = detail {
                v["detail"] = json!(d.detail);
            }
            items.push(v);
        }
    }
    let mut out = json!({
        "folder": folder.to_string_lossy(),
        "count": items.len(),
        "items": items,
    });
    if truncated {
        out["truncated"] = json!(true);
    }
    Ok(ToolOutput::json(&out))
}
