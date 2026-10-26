//! The library tools: `library_list` lists the items of a library folder (patches, containers,
//! clips and projects) with the one line summary the file browser shows (stateless: it reads
//! files and keeps nothing), and `session_library` does what Save to Library and a double click
//! on an item do in the open session.

use crate::rpc::{Server, Tool};
use crate::session::Session;
use crate::tool::{Args, ToolOutput, definition};
use atomik_app::AppCore;
use atomik_app::app_core::LoadTarget;
use atomik_app::session::{LiveSession, SystemBackend};
use atomik_library::{
    FileKind, FileRow, ItemInfo, LibrarySection, Query, classify, device_type_of,
};
use atomik_model::{ClipId, DeviceKey, ItemKind, RackSlot, summarize_item};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The library tools.
pub const TOOLS: &[Tool] = &[
    Tool {
        name: "library_list",
        definition: library_list_def,
        run: library_list,
    },
    Tool {
        name: "session_library",
        definition: session_library_def,
        run: session_library,
    },
];

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

// ------------------------------------------------------------------------ session_library

fn session_library_def() -> Value {
    definition(
        "session_library",
        "The library of the open session (session_open with `library` gives it one). `save_selection` saves the selected rack devices (or the ones named in `devices`) as a patch, or as a container when exactly one container is selected, into `Patches/` or `Containers/` of the library, with their audio in a sibling folder: it is not an edit, the document and the undo history stay as they are. `save_clip` saves the selected note clip (or clip number `clip`) into `Clips/`. `load` puts an item back: `path` is an item file (relative to the library folder or to the working directory, inside the allowed folders) or a built-in device row `atomik-device:<type id>`. Devices (a patch, a container or a device row) go after the selected device, or into rack slot `slot` (`column:index`, a column past the last makes a new one; `container` names the container the slot is in); a clip goes on note lane `lane` of track `track` at `tick`, or on the selected note track at the snapped playhead. One undo step; the item's audio is copied into the session and loaded, and what the item lacks is listed as missing. A project is not loaded here: open it with session_open. Returns where the item was saved (relative to the library folder), or what was loaded, with the status line.",
        &[
            ("action", "string", "save_selection, save_clip or load."),
            ("path", "string", "load: the item file or device row."),
            (
                "devices",
                "strings",
                "save_selection: select these device keys first (path keys reach inside containers).",
            ),
            (
                "clip",
                "number",
                "save_clip: select this clip number first (see session_status).",
            ),
            (
                "slot",
                "string",
                "load of devices: the rack slot `column:index`, like `1:0`.",
            ),
            (
                "container",
                "string",
                "load of devices with slot: the path key of the container the slot is in.",
            ),
            (
                "track",
                "string",
                "load of a clip: the instrument device whose track takes it.",
            ),
            (
                "lane",
                "number",
                "load of a clip with track: the note lane (default 0).",
            ),
            (
                "tick",
                "number",
                "load of a clip with track: the start in ticks (15360 per beat; default the snapped playhead).",
            ),
        ],
        &["action"],
    )
}

fn session_library(server: &mut Server, args: &Args) -> Result<ToolOutput, String> {
    let action = args.str("action")?;
    if !["save_selection", "save_clip", "load"].contains(&action) {
        return Err(format!(
            "unknown action `{action}`: use save_selection, save_clip or load"
        ));
    }
    let session = server.require_session()?;
    let root = session.shell().core().library_root().ok_or_else(|| {
        "the session has no library folder: open it with session_open `library`".to_string()
    })?;
    let load = if action == "load" {
        Some(load_request(server, &root, args)?)
    } else {
        for name in ["path", "slot", "container", "track"] {
            if args.opt_str(name)?.is_some() {
                return Err(format!("`{name}` goes with action load"));
            }
        }
        for name in ["lane", "tick"] {
            if args.opt_f64(name)?.is_some() {
                return Err(format!("`{name}` goes with action load"));
            }
        }
        None
    };
    let session = server.require_session()?;
    match (action, load) {
        ("save_selection", _) => save_selection(session, &root, args),
        ("save_clip", _) => save_clip(session, &root, args),
        (_, Some(request)) => load_item(session, &root, request),
        _ => Err("nothing to do".to_string()),
    }
}

/// What a `load` call asks for, checked before the session is touched.
struct LoadRequest {
    path: PathBuf,
    kind: Option<FileKind>,
    target: Target,
}

enum Target {
    Default,
    Slot(Option<DeviceKey>, RackSlot),
    Track(DeviceKey, usize, Option<i64>),
}

fn load_request(server: &Server, root: &Path, args: &Args) -> Result<LoadRequest, String> {
    let name = args.opt_str("path")?.ok_or("load needs `path`")?;
    let (path, kind) = if device_type_of(Path::new(name)).is_some() {
        (PathBuf::from(name), None)
    } else {
        // A name inside the library folder wins over the same name from the working directory.
        let in_library = root.join(name);
        let path = if Path::new(name).is_relative() && in_library.exists() {
            server.paths().read(&in_library.to_string_lossy())?
        } else {
            server.paths().read(name)?
        };
        let kind = classify(&path, false);
        match kind {
            Some(FileKind::Project) => {
                return Err(format!(
                    "{name}: a project is opened with session_open, not loaded into the open one"
                ));
            }
            Some(FileKind::Patch | FileKind::Container | FileKind::Clip) => {}
            _ => return Err(format!("{name}: not a library item")),
        }
        (path, kind)
    };
    let clip = kind == Some(FileKind::Clip);
    let slot = args.opt_str("slot")?;
    let track = args.opt_str("track")?;
    let container = args.opt_str("container")?;
    let target = match (slot, track) {
        (Some(_), Some(_)) => return Err("give `slot` or `track`, not both".to_string()),
        (Some(_), None) if clip => {
            return Err("`slot` is for devices; a clip goes on a `track`".to_string());
        }
        (None, Some(_)) if !clip => {
            return Err("`track` is for a clip; devices go to a `slot`".to_string());
        }
        (Some(text), None) => Target::Slot(
            container.map(|c| DeviceKey(c.to_string())),
            parse_slot(text)?,
        ),
        (None, Some(device)) => {
            let lane = args.opt_u64("lane")?.unwrap_or(0);
            let lane = usize::try_from(lane).map_err(|_| "`lane` is too large".to_string())?;
            let tick = match args.opt_u64("tick")? {
                Some(t) => Some(i64::try_from(t).map_err(|_| "`tick` is too large".to_string())?),
                None => None,
            };
            Target::Track(DeviceKey(device.to_string()), lane, tick)
        }
        (None, None) => {
            if container.is_some() {
                return Err("`container` goes with `slot`".to_string());
            }
            Target::Default
        }
    };
    Ok(LoadRequest { path, kind, target })
}

/// `column:index` as a rack slot.
fn parse_slot(text: &str) -> Result<RackSlot, String> {
    let bad = || format!("`slot` must be `column:index`, like `1:0`, not `{text}`");
    let (column, index) = text.split_once(':').ok_or_else(bad)?;
    Ok(RackSlot {
        column: column.trim().parse().map_err(|_| bad())?,
        index: index.trim().parse().map_err(|_| bad())?,
    })
}

fn save_selection(session: &mut Session, root: &Path, args: &Args) -> Result<ToolOutput, String> {
    let keys = args.opt_strings("devices")?;
    if !keys.is_empty() {
        let core = session.shell_mut().core_mut();
        let keys: Vec<DeviceKey> = keys.into_iter().map(DeviceKey).collect();
        if let Some(missing) = keys.iter().find(|k| core.doc().device(k).is_none()) {
            return Err(format!("unknown device `{}`", missing.0));
        }
        core.after_view_change();
        core.view_mut().set_selection(&keys, None);
    }
    save_with(session, root, AppCore::save_selection_to_library)
}

fn save_clip(session: &mut Session, root: &Path, args: &Args) -> Result<ToolOutput, String> {
    let clip = args.opt_u64("clip")?;
    let core = session.shell_mut().core_mut();
    if let Some(number) = clip {
        let id = ClipId(number);
        if core.doc().song.clip(id).is_none() {
            return Err(format!("no clip {number} in the document"));
        }
        core.arrangement_mut().set_selected_clips(&[id]);
    }
    save_with(session, root, AppCore::save_clip_to_library)
}

type Core = AppCore<LiveSession<SystemBackend>>;

/// Runs a save, waits for the audio copy and reports the item written (or why nothing was).
fn save_with(
    session: &mut Session,
    root: &Path,
    save: fn(&mut Core),
) -> Result<ToolOutput, String> {
    let before = session
        .shell()
        .core()
        .last_saved_item()
        .map(Path::to_path_buf);
    save(session.shell_mut().core_mut());
    settle(session);
    let core = session.shell().core();
    let saved = core.last_saved_item().map(Path::to_path_buf);
    let Some(saved) = saved.filter(|p| Some(p) != before.as_ref()) else {
        return Err(core
            .last_rejection()
            .unwrap_or("nothing was saved")
            .to_string());
    };
    let relative = saved.strip_prefix(root).unwrap_or(&saved);
    let rel = relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    let section = rel.split('/').next().unwrap_or("").to_lowercase();
    let mut out = reply(session);
    out["saved"] = json!(rel);
    out["section"] = json!(section);
    Ok(ToolOutput::json(&out))
}

fn load_item(
    session: &mut Session,
    root: &Path,
    request: LoadRequest,
) -> Result<ToolOutput, String> {
    let path = request.path;
    let core = session.shell_mut().core_mut();
    let before = core.doc().clone();
    let target = match request.target {
        Target::Default => LoadTarget::Default,
        Target::Slot(container, slot) => LoadTarget::Rack { container, slot },
        Target::Track(device, lane, tick) => {
            let tick = tick.unwrap_or_else(|| core.snapped_playhead());
            LoadTarget::Track { device, lane, tick }
        }
    };
    core.load_library_item(&path, target);
    settle(session);
    let core = session.shell().core();
    if core.doc() == &before {
        return Err(core
            .last_rejection()
            .unwrap_or("nothing was loaded")
            .to_string());
    }
    let shown = path.strip_prefix(root).unwrap_or(&path);
    let mut out = reply(session);
    out["loaded"] = json!(shown.to_string_lossy().replace('\\', "/"));
    out["kind"] = json!(match request.kind {
        Some(FileKind::Patch) => "patch",
        Some(FileKind::Container) => "container",
        Some(FileKind::Clip) => "clip",
        _ => "device",
    });
    Ok(ToolOutput::json(&out))
}

/// Runs frames until the audio copies and loads of the library are done.
fn settle(session: &mut Session) {
    for _ in 0..5_000 {
        session.shell_mut().frame();
        let core = session.shell().core();
        if !core.browser_busy() && !core.is_busy() {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    session.shell_mut().core_mut().wait_background();
    session.shell_mut().frame();
}

/// The state a library call reports: the status line, unsaved changes, undo, missing audio.
fn reply(session: &Session) -> Value {
    let core = session.shell().core();
    json!({
        "message": core.last_rejection(),
        "status_text": session.shell().status_text(),
        "dirty": core.is_dirty(),
        "can_undo": core.document().can_undo(),
        "missing_audio": core
            .missing_assets()
            .iter()
            .map(|m| m.name.clone())
            .collect::<Vec<_>>(),
    })
}
