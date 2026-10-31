//! The stateful tools: one headless [`Shell`] (the real app shell with audio off and no window)
//! that the agent opens, drives with menu commands, pointer and key input, looks at (the texts
//! the UI draws, a PNG screenshot) and bounces.

use crate::analysis;
use crate::rpc::{Server, Tool};
use crate::tier1::{self, bounce_json};
use crate::tool::{Args, ToolOutput, definition};
use atomik_app::AppCore;
use atomik_app::app_core::MidiExport;
use atomik_app::app_core::Pane;
use atomik_app::commands::AppCommand;
use atomik_app::session::{LiveSession, SystemBackend};
use atomik_app::settings::AudioSettings;
use atomik_app::shell::{PendingAction, Shell, ShellOptions, ShellRequest};
use atomik_app::windows::WindowKind;
use atomik_render::wav::{SampleFormat, read_wav_file, write_wav_file};
use atomik_ui::cpu_canvas::CpuCanvas;
use atomik_ui::display_list::Op;
use atomik_ui::event::NumpadKey;
use atomik_ui::keymap::Os;
use atomik_ui::kurbo::{Affine, Point, Size, Vec2};
use atomik_ui::{
    DisplayList, Key, Modifiers, NotesMode, NotesValues, PointerButton, TextEngine, UiEvent,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// Default logical window size.
pub const DEFAULT_SIZE: (f64, f64) = (1280.0, 860.0);

/// The stateful tools.
pub const TOOLS: &[Tool] = &[
    Tool {
        name: "session_open",
        definition: open_def,
        run: open,
    },
    Tool {
        name: "session_status",
        definition: status_def,
        run: status,
    },
    Tool {
        name: "session_commands",
        definition: commands_def,
        run: commands,
    },
    Tool {
        name: "session_command",
        definition: command_def,
        run: command,
    },
    Tool {
        name: "session_pointer",
        definition: pointer_def,
        run: pointer,
    },
    Tool {
        name: "session_key",
        definition: key_def,
        run: key,
    },
    Tool {
        name: "session_texts",
        definition: texts_def,
        run: texts,
    },
    Tool {
        name: "session_screenshot",
        definition: screenshot_def,
        run: screenshot,
    },
    Tool {
        name: "session_save",
        definition: save_def,
        run: save,
    },
    Tool {
        name: "session_relink",
        definition: relink_def,
        run: relink,
    },
    Tool {
        name: "session_midi",
        definition: midi_def,
        run: midi,
    },
    Tool {
        name: "session_audio_notes",
        definition: audio_notes_def,
        run: audio_notes,
    },
    Tool {
        name: "session_set_param",
        definition: set_param_def,
        run: set_param,
    },
    Tool {
        name: "session_meters",
        definition: meters_def,
        run: meters,
    },
    Tool {
        name: "session_bounce",
        definition: bounce_def,
        run: bounce,
    },
    Tool {
        name: "session_export",
        definition: export_def,
        run: export,
    },
    Tool {
        name: "session_close",
        definition: close_def,
        run: close,
    },
    Tool {
        name: "session_window",
        definition: window_def,
        run: window_tool,
    },
];

/// The open headless app.
pub struct Session {
    shell: Shell<SystemBackend>,
    text: TextEngine,
    scale: f64,
    logical: Size,
    os: Os,
    /// The clock and the surface port of the MIDI tools ([`crate::midi_tools`]).
    pub(crate) midi: crate::midi_tools::MidiTools,
    /// The frame clock of the tooltips (seconds; only `session_pointer` `wait` moves it).
    clock: f64,
}

impl Session {
    /// A session on the starter project, or on the file at `file` (already validated), with
    /// `library` as its library folder (`None`: it has none, and Save to Library refuses).
    pub fn open(
        file: Option<&Path>,
        library: Option<PathBuf>,
        logical: Size,
        scale: f64,
        os: Os,
    ) -> Self {
        let backend = LiveSession::new(SystemBackend, AudioSettings::default(), false);
        let core = AppCore::with_doc(backend, atomik_model::new_project_doc(), None);
        let mut shell = Shell::new(
            core,
            ShellOptions {
                in_window_menu: true,
                os: Some(os),
                library_root: library,
                ..ShellOptions::default()
            },
        );
        shell.handle(&UiEvent::ScaleChanged(scale));
        shell.handle(&UiEvent::Resize(logical));
        if let Some(file) = file {
            shell.open_file(file);
        }
        shell.frame();
        Self {
            shell,
            text: TextEngine::new(),
            scale,
            logical,
            os,
            midi: crate::midi_tools::MidiTools::default(),
            clock: 0.0,
        }
    }

    /// The shell.
    pub fn shell(&self) -> &Shell<SystemBackend> {
        &self.shell
    }

    /// The shell, to drive it.
    pub fn shell_mut(&mut self) -> &mut Shell<SystemBackend> {
        &mut self.shell
    }

    /// The state of the session as JSON.
    pub fn status_json(&mut self, include_params: bool) -> Value {
        let state = self.shell.menu_state();
        let (pw, ph) = self.shell.physical_size();
        let windows = self.windows_json();
        json!({
            "title": self.shell.title(),
            "dirty": self.shell.core().is_dirty(),
            "status_text": self.shell.status_text(),
            "overload": overload_json(self.shell.overload()),
            "focus": format!("{:?}", self.shell.focus()),
            "lower_pane": format!("{:?}", self.shell.lower_pane()),
            "library": self.shell.library_root().map(|p| p.to_string_lossy().into_owned()),
            "recent_files": self
                .shell
                .core()
                .recent()
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            "recording": self.shell.core().is_recording(),
            "armed_lanes": self
                .shell
                .core()
                .armed_automation_lanes()
                .iter()
                .map(|(device, param)| json!({"device": device.as_str(), "param": param}))
                .collect::<Vec<_>>(),
            "can_undo": state.can_undo,
            "can_redo": state.can_redo,
            "undo_label": state.undo_label,
            "redo_label": state.redo_label,
            "settings_dialog_open": self.shell.dialog_open(),
            "context_menu_open": self.shell.context_menu().is_some(),
            "window": {
                "logical": [self.logical.width, self.logical.height],
                "scale": self.scale,
                "physical": [pw, ph],
                "os": os_name(self.os),
            },
            "windows": windows,
            "midi": crate::midi_tools::status_json(self),
            "document": tier1::doc_json(self.shell.core().doc(), include_params),
        })
    }

    /// The open windows: the main one, then each detached pane's.
    fn windows_json(&self) -> Vec<Value> {
        let focused = self.shell.focused_window();
        self.shell
            .windows_wanted()
            .into_iter()
            .filter_map(|kind| {
                let size = self.shell.window_size(kind)?;
                let (pw, ph) = self.shell.physical_size_in(kind)?;
                Some(json!({
                    "kind": window_name(kind),
                    "detached": self.shell.detached(kind),
                    "logical": {"width": size.width, "height": size.height},
                    "scale": self.shell.window_scale(kind),
                    "physical": {"width": pw, "height": ph},
                    "focused": focused == kind,
                }))
            })
            .collect()
    }

    /// Whether `kind` is open; the error names the window and the command that opens it.
    fn require_window(&self, kind: WindowKind) -> Result<(), String> {
        if self.shell.window_size(kind).is_some() {
            return Ok(());
        }
        let name = window_name(kind);
        Err(format!(
            "the {name} window is not open: run view.detach_{name}"
        ))
    }

    /// Paints a window into a display list (after a frame, so the layout is current).
    pub fn display_list(&mut self, window: WindowKind) -> DisplayList {
        self.shell.frame();
        let mut list = DisplayList::new();
        self.shell.paint_in(window, &mut list);
        list
    }

    /// Lets `seconds` pass on the clock of the hover delays (a tooltip shows after the pointer
    /// has rested for the delay): one frame at the time it is, then one `seconds` later.
    pub fn rest(&mut self, seconds: f64) {
        self.shell.frame_at(self.clock);
        self.clock += seconds.max(0.0);
        self.shell.frame_at(self.clock);
    }

    /// Delivers `events` to `window` in order, then runs a frame; returns the requests the shell
    /// raised.
    pub fn feed(&mut self, window: WindowKind, events: &[UiEvent]) -> Vec<ShellRequest> {
        let mut requests = Vec::new();
        for e in events {
            requests.extend(self.shell.handle_in(window, e));
        }
        self.shell.frame();
        requests
    }
}

fn window_name(kind: WindowKind) -> &'static str {
    match kind {
        WindowKind::Main => "main",
        WindowKind::Rack => "rack",
        WindowKind::Console => "console",
    }
}

/// The `window` argument (`main` when it is left out), checked against the open windows.
fn window_arg(session: &Session, args: &Args) -> Result<WindowKind, String> {
    let kind = parse_window(args.opt_str("window")?)?;
    session.require_window(kind)?;
    Ok(kind)
}

fn parse_window(name: Option<&str>) -> Result<WindowKind, String> {
    match name {
        None | Some("main") => Ok(WindowKind::Main),
        Some("rack") => Ok(WindowKind::Rack),
        Some("console") => Ok(WindowKind::Console),
        Some(other) => Err(format!(
            "window must be main, rack or console, not `{other}`"
        )),
    }
}

/// The `window` argument of the input and picture tools.
const WINDOW_ARG: (&str, &str, &str) = (
    "window",
    "string",
    "main (default), rack or console: the window to act on. The rack and the console are windows while detached (view.detach_rack, view.detach_console).",
);

fn os_name(os: Os) -> &'static str {
    match os {
        Os::Windows => "windows",
        Os::Mac => "mac",
        Os::Linux => "linux",
    }
}

fn request_json(r: ShellRequest) -> Value {
    let (name, note) = match r {
        ShellRequest::ConfirmSave {
            then: PendingAction::Command(_),
        } => (
            "confirm_save",
            "the document has unsaved changes (the app asks Save, Don't Save or Cancel): save with session_save and repeat the command, or repeat it with confirm true to discard them",
        ),
        ShellRequest::ConfirmSave {
            then: PendingAction::OpenFile(_),
        } => (
            "confirm_save_open",
            "the document has unsaved changes: save with session_save first, or open the project with session_open instead",
        ),
        ShellRequest::PickSaveFileThen { .. } => (
            "pick_save_file",
            "a file dialog: use session_save with a path",
        ),
        ShellRequest::Quit => ("quit", "the app asked to quit; the session stays open"),
        ShellRequest::PickOpenFile => (
            "pick_open_file",
            "a file dialog: use session_open with a path",
        ),
        ShellRequest::PickRelinkFile | ShellRequest::PickRelinkFolder => (
            "pick_relink_path",
            "a file or folder dialog: use session_relink with a file or a folder",
        ),
        ShellRequest::PickSaveFile | ShellRequest::PickExportFile => (
            "pick_save_file",
            "a file dialog: use session_save with a path",
        ),
        ShellRequest::PickExportMidiPreset | ShellRequest::PickImportMidiPreset => (
            "pick_midi_preset",
            "a file dialog: use session_midi_learn with action export or import and a file",
        ),
        ShellRequest::PickExportAudio(_) => (
            "pick_export_audio",
            "a file dialog: use session_export with a file (it writes the same file the dialog would)",
        ),
        ShellRequest::PickImportMidi { .. } | ShellRequest::PickExportMidi(_) => (
            "pick_midi_file",
            "a file dialog: a headless session cannot show it (the MIDI file import and export are covered by the app tests)",
        ),
        _ => (
            "dialog",
            "a native dialog, which a headless session cannot show",
        ),
    };
    json!({"request": name, "detail": format!("{r:?}"), "note": note})
}

// ----------------------------------------------------------------- session_open

fn open_def() -> Value {
    definition(
        "session_open",
        "Starts the headless app (replacing any open session): the real shell, audio off, no \
window. Opens `path` (a project, bundle folder or patch) or the starter project. Returns the \
session status.",
        &[
            (
                "path",
                "string",
                "Project or patch to open; omit for the starter project.",
            ),
            ("width", "number", "Logical window width (default 1280)."),
            ("height", "number", "Logical window height (default 860)."),
            (
                "scale",
                "number",
                "Display scale factor (default 1; 2 gives a sharper screenshot).",
            ),
            (
                "os",
                "string",
                "windows (default), mac or linux: the keymap and shortcut texts.",
            ),
            (
                "library",
                "string",
                "The library folder of the session (Save to Library writes there, session_library loads from it); inside the allowed folders, made when missing. Without it the session has no library.",
            ),
        ],
        &[],
    )
}

fn open(server: &mut Server, args: &Args) -> Result<ToolOutput, String> {
    let width = args.opt_f64("width")?.unwrap_or(DEFAULT_SIZE.0);
    let height = args.opt_f64("height")?.unwrap_or(DEFAULT_SIZE.1);
    let scale = args.opt_f64("scale")?.unwrap_or(1.0);
    if !(200.0..=8192.0).contains(&width) || !(200.0..=8192.0).contains(&height) {
        return Err("width and height must be 200..=8192".to_string());
    }
    if !(0.5..=4.0).contains(&scale) {
        return Err("scale must be 0.5..=4".to_string());
    }
    let os = match args.opt_str("os")? {
        None | Some("windows") => Os::Windows,
        Some("mac") => Os::Mac,
        Some("linux") => Os::Linux,
        Some(other) => return Err(format!("os must be windows, mac or linux, not `{other}`")),
    };
    let file = match args.opt_str("path")? {
        None => None,
        Some(name) => {
            let (file, text) = tier1::read_project_text(server, name)?;
            // The shell reports a bad file only in its status line: load it here first so the
            // error is the precise one (a JSON path and what is wrong).
            atomik_model::load_project(&text, &atomik_render::default_registry())
                .map_err(|e| format!("{}: {e}", file.display()))?;
            Some(file)
        }
    };
    let library = match args.opt_str("library")? {
        None => None,
        Some(name) => Some(library_folder(server, name)?),
    };
    let mut session = Session::open(
        file.as_deref(),
        library,
        Size::new(width, height),
        scale,
        os,
    );
    let status = session.status_json(false);
    server.set_session(Some(session));
    Ok(ToolOutput::json(&status))
}

/// The library folder `name` of a session: inside the allowed folders, made when missing.
fn library_folder(server: &Server, name: &str) -> Result<PathBuf, String> {
    if name.trim().is_empty() {
        return Err("`library` is empty".to_string());
    }
    // `write` checks a file and makes its folder: a name inside the library gives the folder.
    let marker = server.paths().write(&format!("{name}/library"))?;
    marker
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("{name}: not a folder"))
}

/// The `overload` block of `session_status`: the counters the status bar shows when they are
/// not zero, all of them here (a headless session has no audio stream, so they read 0).
fn overload_json(o: atomik_app::status::OverloadStats) -> Value {
    json!({
        "dropped_events": o.dropped_events,
        "input_overruns": o.input_overruns,
        "tap_overflow_frames": o.tap_overflow_frames,
        "plugin_drops": o.plugin_drops,
        "control_stalled": o.control_stalled,
        "late_sandbox_batches": o.late_sandbox_batches,
    })
}

// --------------------------------------------------------------- session_status

fn status_def() -> Value {
    definition(
        "session_status",
        "The open session: title, unsaved changes, the recent songs, status line, focused pane, undo/redo \
availability, whether a take records, the automation lanes armed for recording, the overload counters (dropped events, input overruns, record overflow, plugin drops, control stalls, late sandbox batches), window size, and the document (devices, cables, song; automation lanes with their write mode).",
        &[(
            "include_params",
            "boolean",
            "List every device's saved parameter values.",
        )],
        &[],
    )
}

fn status(server: &mut Server, args: &Args) -> Result<ToolOutput, String> {
    let include = args.opt_bool("include_params")?.unwrap_or(false);
    Ok(ToolOutput::json(
        &server.require_session()?.status_json(include),
    ))
}

// ------------------------------------------------------------ session_commands

fn commands_def() -> Value {
    definition(
        "session_commands",
        "The menu commands of the open session: menu, label, id (what session_command takes), \
shortcut, checked state and whether it can be chosen now.",
        &[
            (
                "menu",
                "string",
                "Only this menu (File, Edit, View, Transport, Mixer, Create, Track, Audio ...).",
            ),
            (
                "query",
                "string",
                "Only commands whose label or id contains this text.",
            ),
            (
                "enabled_only",
                "boolean",
                "Only commands that can be chosen now.",
            ),
        ],
        &[],
    )
}

fn command_text(c: &AppCommand) -> Option<String> {
    c.command_id().map(|(id, arg)| match arg {
        Some(a) => format!("{id}:{a}"),
        None => id.to_string(),
    })
}

fn commands(server: &mut Server, args: &Args) -> Result<ToolOutput, String> {
    let menu_filter = args.opt_str("menu")?.map(str::to_lowercase);
    let query = args.opt_str("query")?.map(str::to_lowercase);
    let enabled_only = args.opt_bool("enabled_only")?.unwrap_or(false);
    let session = server.require_session()?;
    let mut out = Vec::new();
    for menu in session.shell().menu_model() {
        if menu_filter
            .as_deref()
            .is_some_and(|m| m != menu.title.to_lowercase())
        {
            continue;
        }
        for item in menu.items() {
            let Some(id) = command_text(&item.command) else {
                continue;
            };
            if enabled_only && !item.enabled {
                continue;
            }
            if let Some(q) = &query
                && !id.to_lowercase().contains(q)
                && !item.label.to_lowercase().contains(q)
            {
                continue;
            }
            out.push(json!({
                "menu": menu.title, "label": item.label, "id": id,
                "shortcut": item.shortcut, "checked": item.checked, "enabled": item.enabled,
            }));
        }
    }
    Ok(ToolOutput::json(
        &json!({"count": out.len(), "commands": out}),
    ))
}

// ------------------------------------------------------------- session_command

fn command_def() -> Value {
    definition(
        "session_command",
        "Runs a menu command by id (see session_commands), as the menu or its shortcut would. \
A command that needs a native dialog or a confirmation is reported in `requests`, not run \
silently; `confirm` true answers the unsaved-changes question (Save, Don't Save, Cancel) with Don't Save. A freeze or bounce (sequencer.freeze_track, sequencer.bounce_in_place ...) renders on a worker: the session has no frame loop, so the call waits for the render to finish (and the track to be frozen) unless `wait` is false; `message` then carries what it said (for example `Froze Lead`).",
        &[
            (
                "id",
                "string",
                "Command id, e.g. edit.undo, view.toggle_browser, file.new.",
            ),
            (
                "confirm",
                "boolean",
                "Answer the 'unsaved changes' question with Don't Save (discard them and carry on).",
            ),
            (
                "wait",
                "boolean",
                "Wait for a freeze or bounce render to finish (default true).",
            ),
            (
                "arg",
                "string",
                "The command's argument, for the ones that take one: the id of a VCA group for mixer.add_to_vca and mixer.delete_vca (ids are in session_status under console.vcas; without it they act on the last group, and on the groups of the selected channels). For the snapshot commands (mixer.recall_snapshot, mixer.update_snapshot, mixer.compare_snapshot, mixer.delete_snapshot) the id or the name of a snapshot (ids and names are in session_status under console.snapshots; without it they act on the current snapshot); mixer.recall_snapshot also takes `<snapshot>:<scope>` with a scope of all, levels, mutes, sends or inserts; mixer.store_snapshot takes the name of the new snapshot (without it the next of A, B, 1, 2, ...).",
            ),
        ],
        &["id"],
    )
}

fn command(server: &mut Server, args: &Args) -> Result<ToolOutput, String> {
    let id = args.str("id")?;
    let arg = args.opt_str("arg")?;
    let confirm = args.opt_bool("confirm")?.unwrap_or(false);
    let wait = args.opt_bool("wait")?.unwrap_or(true);
    let session = server.require_session()?;
    let found = session
        .shell()
        .menu_model()
        .iter()
        .flat_map(|m| m.items().cloned().collect::<Vec<_>>())
        .find(|item| command_text(&item.command).as_deref() == Some(id));
    // A command whose argument names a runtime thing (a snapshot) may be run without its own
    // menu item: the argument is what the menu cannot spell.
    let snapshot_arg = arg.is_some() && id.ends_with("_snapshot");
    let item = match found {
        Some(item) => Some(item),
        None if snapshot_arg => None,
        None => return Err(format!("no menu command `{id}` (see session_commands)")),
    };
    if let Some(item) = &item
        && !item.enabled
    {
        return Err(format!("`{id}` ({}) is disabled right now", item.label));
    }
    // The snapshot commands that take a name or a scope are run on the core.
    let special = snapshot_special(session.shell().core().doc(), id, arg)?;
    let command = match (&special, arg, &item) {
        (Some(_), _, _) => None,
        (None, None, Some(item)) => Some(item.command),
        // A command that takes an argument the menu cannot spell (a group id) is built from it.
        (None, Some(a), _) => {
            let a = snapshot_number(session.shell().core().doc(), id, a)?;
            Some(
                AppCommand::from_command(id, Some(&a))
                    .ok_or_else(|| format!("`{id}` does not take the argument `{a}`"))?,
            )
        }
        (None, None, None) => return Err(format!("no menu command `{id}` (see session_commands)")),
    };
    let shell = session.shell_mut();
    let mut raised = match (special, command) {
        (Some(special), _) => {
            if !special.run(shell) {
                return Err(shell
                    .core()
                    .last_rejection()
                    .unwrap_or("the command was refused")
                    .to_owned());
            }
            Vec::new()
        }
        (None, Some(command)) => shell.execute(command),
        (None, None) => Vec::new(),
    };
    let mut done = Vec::new();
    while let Some(r) = raised.pop() {
        match r {
            ShellRequest::ConfirmSave {
                then: then @ PendingAction::Command(_),
            } if confirm => raised.extend(shell.discard_then(then)),
            other => done.push(request_json(other)),
        }
    }
    // A render started by this command finishes (and is applied) before the answer.
    let waited = wait && shell.core().freeze_busy();
    if waited {
        shell.core_mut().wait_background();
    }
    shell.frame();
    // The status line carries the progress of a render that was not waited for.
    shell.poll_status();
    let mut out = json!({
        "command": id,
        "requests": done,
        "status_text": session.shell().status_text(),
        "dirty": session.shell().core().is_dirty(),
        "title": session.shell().title(),
    });
    if waited {
        out["waited"] = json!(true);
        out["message"] = json!(session.shell().core().last_rejection());
    }
    Ok(ToolOutput::json(&out))
}

/// A snapshot command that the menu cannot spell: storing under a name, or recalling only some
/// categories.
enum SnapshotSpecial {
    Store(String),
    Recall(u32, atomik_model::SnapshotScope),
}

impl SnapshotSpecial {
    fn run(self, shell: &mut Shell<SystemBackend>) -> bool {
        let core = shell.core_mut();
        match self {
            SnapshotSpecial::Store(name) => core.store_snapshot(Some(name)),
            SnapshotSpecial::Recall(id, scope) => core.recall_scoped(id, scope),
        }
    }
}

/// The id of the snapshot an argument names: a number is the id (it must exist when it is looked
/// up by the command), anything else is a name.
fn snapshot_id(doc: &atomik_model::RackDoc, text: &str) -> Result<u32, String> {
    let text = text.trim();
    if let Ok(id) = text.parse::<u32>() {
        return Ok(id);
    }
    doc.console
        .snapshot_by_name(text)
        .map(|s| s.id)
        .ok_or_else(|| {
            let names: Vec<&str> = doc
                .console
                .snapshots
                .iter()
                .map(|s| s.name.as_str())
                .collect();
            format!(
                "no snapshot called `{text}` (snapshots: {})",
                names.join(", ")
            )
        })
}

/// A snapshot argument as the number the registry command takes (a name becomes its id); other
/// commands' arguments pass through.
fn snapshot_number(doc: &atomik_model::RackDoc, id: &str, arg: &str) -> Result<String, String> {
    if matches!(
        id,
        "mixer.recall_snapshot"
            | "mixer.update_snapshot"
            | "mixer.compare_snapshot"
            | "mixer.delete_snapshot"
    ) {
        return Ok(snapshot_id(doc, arg)?.to_string());
    }
    Ok(arg.to_owned())
}

/// `mixer.store_snapshot` with a name, and `mixer.recall_snapshot` with a scope
/// (`<snapshot>:<scope>`).
fn snapshot_special(
    doc: &atomik_model::RackDoc,
    id: &str,
    arg: Option<&str>,
) -> Result<Option<SnapshotSpecial>, String> {
    let (Some(arg), true) = (arg, id.ends_with("_snapshot")) else {
        return Ok(None);
    };
    match id {
        "mixer.store_snapshot" => Ok(Some(SnapshotSpecial::Store(arg.to_owned()))),
        "mixer.recall_snapshot" => {
            let Some((which, scope)) = arg.rsplit_once(':') else {
                return Ok(None);
            };
            let scope = atomik_model::SnapshotScope::from_name(scope).ok_or_else(|| {
                format!("`{scope}` is not a scope (all, levels, mutes, sends, inserts)")
            })?;
            Ok(Some(SnapshotSpecial::Recall(
                snapshot_id(doc, which)?,
                scope,
            )))
        }
        _ => Ok(None),
    }
}

// ------------------------------------------------------------ session_pointer

fn pointer_def() -> Value {
    definition(
        "session_pointer",
        "Pointer input in logical pixels (the screenshot divided by its scale): move, down, up, \
click, double_click, drag (from x,y to to_x,to_y), wheel (dx, dy in notches, positive y up), \
leave. Find where to aim with session_texts. With `window`, the position is in that window.",
        &[
            WINDOW_ARG,
            (
                "action",
                "string",
                "move, down, up, click, double_click, drag, wheel or leave.",
            ),
            ("x", "number", "Pointer x."),
            ("y", "number", "Pointer y."),
            ("to_x", "number", "drag: end x."),
            ("to_y", "number", "drag: end y."),
            ("steps", "integer", "drag: moves on the way (default 8)."),
            ("dx", "number", "wheel: horizontal notches."),
            (
                "dy",
                "number",
                "wheel: vertical notches (positive scrolls up / increases).",
            ),
            (
                "button",
                "string",
                "primary (default), secondary or middle.",
            ),
            (
                "modifiers",
                "strings",
                "Held keys: shift, ctrl, alt, meta (cmd is meta on mac, ctrl elsewhere).",
            ),
            (
                "wait",
                "number",
                "Seconds the pointer then rests where it is. A tooltip shows after 0.6 s on a button: read it in `tooltip`, in session_texts or in a screenshot.",
            ),
        ],
        &["action"],
    )
}

fn modifiers(args: &Args, os: Os) -> Result<Modifiers, String> {
    let mut m = Modifiers::NONE;
    for name in args.opt_strings("modifiers")? {
        match name.to_lowercase().as_str() {
            "shift" => m.shift = true,
            "ctrl" | "control" => m.ctrl = true,
            "alt" | "option" => m.alt = true,
            "meta" | "super" | "win" => m.meta = true,
            "cmd" | "command" => {
                if os == Os::Mac {
                    m.meta = true;
                } else {
                    m.ctrl = true;
                }
            }
            other => return Err(format!("unknown modifier `{other}`")),
        }
    }
    Ok(m)
}

fn button(args: &Args) -> Result<PointerButton, String> {
    match args.opt_str("button")? {
        None | Some("primary" | "left") => Ok(PointerButton::Primary),
        Some("secondary" | "right") => Ok(PointerButton::Secondary),
        Some("middle") => Ok(PointerButton::Middle),
        Some(other) => Err(format!(
            "button must be primary, secondary or middle, not `{other}`"
        )),
    }
}

fn pointer(server: &mut Server, args: &Args) -> Result<ToolOutput, String> {
    let action = args.str("action")?;
    let session = server.require_session()?;
    let window = window_arg(session, args)?;
    let mods = modifiers(args, session.os)?;
    let button = button(args)?;
    let pos = |xn: &str, yn: &str| -> Result<Point, String> {
        Ok(Point::new(args.f64(xn)?, args.f64(yn)?))
    };
    let down = |pos, clicks| UiEvent::PointerDown {
        pos,
        button,
        modifiers: mods,
        clicks,
    };
    let up = |pos| UiEvent::PointerUp {
        pos,
        button,
        modifiers: mods,
    };
    let mv = |pos| UiEvent::PointerMove {
        pos,
        modifiers: mods,
    };
    let mut events = Vec::new();
    if mods != Modifiers::NONE {
        events.push(UiEvent::ModifiersChanged { modifiers: mods });
    }
    let mut last = None;
    match action {
        "move" => {
            let p = pos("x", "y")?;
            events.push(mv(p));
            last = Some(p);
        }
        "down" => {
            let p = pos("x", "y")?;
            events.extend([mv(p), down(p, 1)]);
            last = Some(p);
        }
        "up" => {
            let p = pos("x", "y")?;
            events.extend([mv(p), up(p)]);
            last = Some(p);
        }
        "click" => {
            let p = pos("x", "y")?;
            events.extend([mv(p), down(p, 1), up(p)]);
            last = Some(p);
        }
        "double_click" => {
            let p = pos("x", "y")?;
            events.extend([mv(p), down(p, 1), up(p), down(p, 2), up(p)]);
            last = Some(p);
        }
        "drag" => {
            let (a, b) = (pos("x", "y")?, pos("to_x", "to_y")?);
            let steps = args.opt_u64("steps")?.unwrap_or(8).clamp(1, 200);
            events.extend([mv(a), down(a, 1)]);
            for i in 1..=steps {
                let t = i as f64 / steps as f64;
                events.push(mv(a.lerp(b, t)));
            }
            events.push(up(b));
            last = Some(b);
        }
        "wheel" => {
            let p = pos("x", "y")?;
            events.push(UiEvent::Wheel {
                delta: Vec2::new(
                    args.opt_f64("dx")?.unwrap_or(0.0),
                    args.opt_f64("dy")?.unwrap_or(0.0),
                ),
                pos: p,
                modifiers: mods,
            });
            last = Some(p);
        }
        "leave" => events.push(UiEvent::PointerLeave),
        other => return Err(format!("unknown pointer action `{other}`")),
    }
    if mods != Modifiers::NONE {
        events.push(UiEvent::ModifiersChanged {
            modifiers: Modifiers::NONE,
        });
    }
    let requests = session.feed(window, &events);
    if let Some(wait) = args.opt_f64("wait")?.filter(|w| *w > 0.0) {
        session.rest(wait);
    }
    let pane = last.and_then(|p| match window {
        WindowKind::Main => session.shell().hit_pane(p),
        _ => {
            let pane = if window == WindowKind::Rack {
                Pane::Rack
            } else {
                Pane::Console
            };
            let (_, rect) = session.shell().pane_rect(pane)?;
            rect.contains(p).then_some(pane)
        }
    });
    Ok(ToolOutput::json(&json!({
        "action": action,
        "window": window_name(window),
        "pane_under_pointer": pane.map(|p| format!("{p:?}")),
        "requests": requests.into_iter().map(request_json).collect::<Vec<_>>(),
        "tooltip": session.shell().tooltip().map(|t| t.text),
        "status_text": session.shell().status_text(),
        "dirty": session.shell().core().is_dirty(),
    })))
}

// ----------------------------------------------------------------- session_key

fn key_def() -> Value {
    definition(
        "session_key",
        "Keyboard input: press and release `key` with `modifiers`, or type `text` character by \
character. Keys: a single character, Escape, Enter, Space, Backspace, Delete, Tab, Up, Down, \
Left, Right, PageUp, PageDown, Home, End, Insert, F1..F24, Digit0..Digit9 (physical, for Alt \
chords), Numpad0..Numpad9. The keymap decides what a chord does (session_commands shows the \
shortcuts). With `window`, the key comes from that window (one that does not have the focus takes it first).",
        &[
            WINDOW_ARG,
            ("key", "string", "The key to press."),
            ("text", "string", "Characters to type instead of a key."),
            (
                "modifiers",
                "strings",
                "shift, ctrl, alt, meta (cmd is meta on mac, ctrl elsewhere).",
            ),
        ],
        &[],
    )
}

/// Parses a key name.
///
/// # Errors
/// A message that lists the accepted names.
pub fn parse_key(name: &str) -> Result<Key, String> {
    let lower = name.to_lowercase();
    let simple = match lower.as_str() {
        "escape" | "esc" => Some(Key::Escape),
        "enter" | "return" => Some(Key::Enter),
        "space" => Some(Key::Space),
        "backspace" => Some(Key::Backspace),
        "delete" | "del" => Some(Key::Delete),
        "tab" => Some(Key::Tab),
        "up" => Some(Key::Up),
        "down" => Some(Key::Down),
        "left" => Some(Key::Left),
        "right" => Some(Key::Right),
        "pageup" => Some(Key::PageUp),
        "pagedown" => Some(Key::PageDown),
        "home" => Some(Key::Home),
        "end" => Some(Key::End),
        "insert" => Some(Key::Insert),
        _ => None,
    };
    if let Some(k) = simple {
        return Ok(k);
    }
    if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u8>().ok())
        && (1..=24).contains(&n)
    {
        return Ok(Key::F(n));
    }
    let digit = |prefix: &str| {
        lower
            .strip_prefix(prefix)
            .and_then(|n| n.parse::<u8>().ok())
            .filter(|n| *n <= 9)
    };
    if let Some(n) = digit("digit") {
        return Ok(Key::Digit(n));
    }
    if let Some(n) = digit("numpad") {
        return Ok(Key::Numpad(NumpadKey::Digit(n)));
    }
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Ok(Key::Char(c)),
        _ => Err(format!("unknown key `{name}`")),
    }
}

fn key(server: &mut Server, args: &Args) -> Result<ToolOutput, String> {
    let session = server.require_session()?;
    let window = window_arg(session, args)?;
    let mods = modifiers(args, session.os)?;
    let mut events = Vec::new();
    let press = |key: Key, modifiers: Modifiers| {
        [
            UiEvent::Key {
                key: key.clone(),
                modifiers,
                pressed: true,
            },
            UiEvent::Key {
                key,
                modifiers,
                pressed: false,
            },
        ]
    };
    match (args.opt_str("key")?, args.opt_str("text")?) {
        (Some(_), Some(_)) => return Err("give key or text, not both".to_string()),
        (None, None) => return Err("give key or text".to_string()),
        (Some(name), None) => {
            if mods != Modifiers::NONE {
                events.push(UiEvent::ModifiersChanged { modifiers: mods });
            }
            events.extend(press(parse_key(name)?, mods));
            if mods != Modifiers::NONE {
                events.push(UiEvent::ModifiersChanged {
                    modifiers: Modifiers::NONE,
                });
            }
        }
        (None, Some(text)) => {
            for c in text.chars() {
                events.extend(press(Key::Char(c), Modifiers::NONE));
            }
        }
    }
    let requests = session.feed(window, &events);
    Ok(ToolOutput::json(&json!({
        "window": window_name(window),
        "requests": requests.into_iter().map(request_json).collect::<Vec<_>>(),
        "status_text": session.shell().status_text(),
        "dirty": session.shell().core().is_dirty(),
    })))
}

// --------------------------------------------------------------- session_texts

fn texts_def() -> Value {
    definition(
        "session_texts",
        "Every text the UI draws right now, with its position in logical pixels (the baseline \
anchor) and size: the cheap way to read the screen and to find where to click. `window` reads a detached window instead of the main one.",
        &[
            WINDOW_ARG,
            (
                "contains",
                "string",
                "Only texts containing this (case-insensitive).",
            ),
            ("limit", "integer", "At most this many (default 300)."),
        ],
        &[],
    )
}

/// The texts of a display list with absolute logical positions.
pub fn list_texts(list: &DisplayList, scale: f64) -> Vec<(String, f64, f64, f32)> {
    let mut stack = vec![Affine::IDENTITY];
    let mut out = Vec::new();
    for op in list.ops() {
        match op {
            Op::PushTransform(t) => {
                let top = *stack.last().unwrap_or(&Affine::IDENTITY);
                stack.push(top * *t);
            }
            Op::PopTransform => {
                if stack.len() > 1 {
                    stack.pop();
                }
            }
            Op::Text(run) => {
                let p = *stack.last().unwrap_or(&Affine::IDENTITY) * run.pos;
                // Physical pixels back to logical ones; the size is already logical or scaled by
                // the same transform, so report it as drawn.
                out.push((run.text.clone(), p.x / scale, p.y / scale, run.size));
            }
            _ => {}
        }
    }
    out
}

fn texts(server: &mut Server, args: &Args) -> Result<ToolOutput, String> {
    let needle = args.opt_str("contains")?.map(str::to_lowercase);
    let limit = args.opt_u64("limit")?.unwrap_or(300) as usize;
    let session = server.require_session()?;
    let window = window_arg(session, args)?;
    let scale = session
        .shell()
        .window_scale(window)
        .unwrap_or(session.scale);
    let list = session.display_list(window);
    let all = list_texts(&list, scale);
    let matching: Vec<_> = all
        .iter()
        .filter(|(t, ..)| {
            needle
                .as_deref()
                .is_none_or(|n| t.to_lowercase().contains(n))
        })
        .collect();
    let shown: Vec<Value> = matching
        .iter()
        .take(limit)
        .map(|(t, x, y, s)| {
            json!({"text": t, "x": (x * 10.0).round() / 10.0, "y": (y * 10.0).round() / 10.0, "size": s})
        })
        .collect();
    Ok(ToolOutput::json(&json!({
        "window": window_name(window),
        "total": matching.len(),
        "shown": shown.len(),
        "texts": shown,
    })))
}

// ----------------------------------------------------------- session_screenshot

fn screenshot_def() -> Value {
    definition(
        "session_screenshot",
        "A PNG of the window as the app paints it (CPU rasteriser, so it matches the app closely \
but is not the GPU output). Returned as an image; `save_to` also writes the file. `window` shoots a detached window at its own size.",
        &[
            WINDOW_ARG,
            ("save_to", "string", "Also write the PNG to this path."),
        ],
        &[],
    )
}

fn screenshot(server: &mut Server, args: &Args) -> Result<ToolOutput, String> {
    let save_to = args
        .opt_str("save_to")?
        .map(|p| server.paths().write(p))
        .transpose()?;
    let session = server.require_session()?;
    let window = window_arg(session, args)?;
    session.shell_mut().frame();
    let (w, h) = session
        .shell()
        .physical_size_in(window)
        .ok_or("the window is not open")?;
    let (w16, h16) = (
        u16::try_from(w).map_err(|_| "window too wide to rasterise")?,
        u16::try_from(h).map_err(|_| "window too tall to rasterise")?,
    );
    let mut canvas = CpuCanvas::new(w16, h16, &mut session.text);
    session.shell.paint_in(window, &mut canvas);
    let png = canvas
        .finish()
        .into_png()
        .map_err(|e| format!("cannot encode the PNG: {e}"))?;
    if let Some(path) = &save_to {
        std::fs::write(path, &png).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(ToolOutput::json(&json!({
        "window": window_name(window),
        "width": w, "height": h,
        "scale": session.shell().window_scale(window),
        "saved_to": save_to.map(|p| p.to_string_lossy().into_owned()),
    }))
    .with_png(png))
}

// -------------------------------------------------------------- session_window

fn window_def() -> Value {
    definition(
        "session_window",
        "Plays the operating system for the windows of the session: `focus` gives a window the keyboard focus (the keys then go to its pane), `close` closes a detached window, which attaches its pane to the main window again (the same as view.detach_rack or view.detach_console), `resize` resizes a window to `width` x `height` logical pixels. The main window cannot be closed here: session_close ends the session. Detach with session_command view.detach_rack or view.detach_console. Returns the open windows, as session_status lists them.",
        &[
            ("action", "string", "focus, close or resize."),
            (
                "window",
                "string",
                "main, rack or console: the window to act on.",
            ),
            ("width", "number", "resize: the logical width (200..8192)."),
            (
                "height",
                "number",
                "resize: the logical height (200..8192).",
            ),
        ],
        &["action", "window"],
    )
}

fn window_tool(server: &mut Server, args: &Args) -> Result<ToolOutput, String> {
    let action = args.str("action")?;
    if !matches!(action, "focus" | "close" | "resize") {
        return Err(format!(
            "action must be focus, close or resize, not `{action}`"
        ));
    }
    let kind = parse_window(Some(args.str("window")?))?;
    let size = if action == "resize" {
        let (w, h) = (args.f64("width")?, args.f64("height")?);
        if !(200.0..=8192.0).contains(&w) || !(200.0..=8192.0).contains(&h) {
            return Err("width and height must be 200..=8192".to_string());
        }
        Some(Size::new(w, h))
    } else {
        None
    };
    let session = server.require_session()?;
    session.require_window(kind)?;
    match (action, size) {
        ("focus", _) => session.shell_mut().window_focused(kind),
        ("close", _) => {
            if kind == WindowKind::Main {
                return Err(
                    "the main window cannot be closed here: session_close ends the session"
                        .to_string(),
                );
            }
            session.shell_mut().window_closed(kind);
        }
        (_, Some(size)) => {
            session.feed(kind, &[UiEvent::Resize(size)]);
            if kind == WindowKind::Main {
                session.logical = size;
            }
        }
        _ => {}
    }
    session.shell_mut().frame();
    Ok(ToolOutput::json(&json!({
        "action": action,
        "windows": session.windows_json(),
    })))
}

// ---------------------------------------------------------------- session_set_param

fn set_param_def() -> Value {
    definition(
        "session_set_param",
        "Sets a param of a device in the session's document, as turning its knob does: one undo step, clamped to the param's range. For the channel strip it is how a headless session sets the input trim (`input.gain`, dB), the polarity (`input.phase`, 0 or 1), the parallel compressor (`pcomp.on`, `pcomp.threshold`, `pcomp.ratio`, `pcomp.attack`, `pcomp.release`, `pcomp.makeup`, `pcomp.mix`) or the bypass of an insert effect (`bypass`, 0 or 1). `list_devices` with a `type_id` lists the keys and ranges; `session_status` names the device keys. Returns the value the param has now.",
        &[
            (
                "device",
                "string",
                "The device key (a channel strip's key is under console in session_status).",
            ),
            (
                "param",
                "string",
                "The param key, as `list_devices` shows it.",
            ),
            (
                "value",
                "number",
                "The plain value (dB, ms, 0 or 1 for a switch).",
            ),
        ],
        &["device", "param", "value"],
    )
}

fn set_param(server: &mut Server, args: &Args) -> Result<ToolOutput, String> {
    let device = args.str("device")?.to_owned();
    let param = args.str("param")?.to_owned();
    let value = args.f64("value")?;
    if !value.is_finite() {
        return Err("`value` must be a finite number".into());
    }
    let session = server.require_session()?;
    let key = atomik_model::DeviceKey(device.clone());
    let core = session.shell_mut().core_mut();
    let known = core.doc().device(&key).is_some_and(|d| {
        core.registry()
            .get(&d.type_id)
            .is_some_and(|f| f.descriptor.param_by_key(&param).is_some())
    });
    if !known {
        return Err(format!("no param `{param}` on a device `{device}`"));
    }
    core.apply_command(atomik_model::Command::SetParam {
        key: key.clone(),
        param: param.clone(),
        value: value as f32,
        merge: None,
    })
    .map_err(|e| e.to_string())?;
    session.shell_mut().frame();
    let core = session.shell().core();
    // An unset param (or one set back to its default) has the descriptor's default.
    let now = core.doc().device(&key).and_then(|d| {
        let p = core
            .registry()
            .get(&d.type_id)?
            .descriptor
            .param_by_key(&param)?;
        Some(p.clamp(d.params.get(&param).copied().unwrap_or(p.default)))
    });
    Ok(ToolOutput::json(&json!({
        "device": device,
        "param": param,
        "value": now,
        "can_undo": core.document().can_undo(),
    })))
}

// ---------------------------------------------------------------- session_save

fn save_def() -> Value {
    definition(
        "session_save",
        "Saves the session's document as a project (.atomik) at `path`.",
        &[("path", "string", "Where to write the project.")],
        &["path"],
    )
}

fn save(server: &mut Server, args: &Args) -> Result<ToolOutput, String> {
    let path = server.paths().write(args.str("path")?)?;
    let session = server.require_session()?;
    session.shell_mut().save_to(&path);
    session.shell_mut().frame();
    let written = path.is_file();
    Ok(ToolOutput::json(&json!({
        "path": path.to_string_lossy(),
        "written": written,
        "bytes": std::fs::metadata(&path).map(|m| m.len()).ok(),
        "status_text": session.shell().status_text(),
        "dirty": session.shell().core().is_dirty(),
    })))
}

// ---------------------------------------------------------------- session_midi

fn midi_def() -> Value {
    definition(
        "session_midi",
        "Imports or exports a Standard MIDI File, answering the dialogs of File > Import MIDI... and File > Export MIDI... (session_command file.import_midi, file.import_midi_song, file.export_midi_clip, file.export_midi_track and file.export_midi_song raise native dialogs). `action: import` reads `file`: one clip on the selected note track, or new tracks when none is selected; with `as_song` always new tracks from the start plus the file's tempo map, meter and markers. `action: export` writes `file` for `scope` (clip: the selected note clip, track: the selected note track, song: every note track as a type 1 file). One undo step per import. Returns `message` (the toast), `document_changed`, `can_undo`, and for an export the bytes written.",
        &[
            ("action", "string", "import or export."),
            ("file", "string", "The MIDI file to read, or to write."),
            (
                "scope",
                "string",
                "Export: clip, track or song (default song).",
            ),
            (
                "as_song",
                "boolean",
                "Import: new tracks from the start with the tempo, meter and markers.",
            ),
        ],
        &["action", "file"],
    )
}

fn midi(server: &mut Server, args: &Args) -> Result<ToolOutput, String> {
    let action = args.str("action")?;
    let file = args.str("file")?;
    let as_song = args.opt_bool("as_song")?.unwrap_or(false);
    let scope = match args.opt_str("scope")?.unwrap_or("song") {
        "clip" => MidiExport::Clip,
        "track" => MidiExport::Track,
        "song" => MidiExport::Song,
        other => return Err(format!("scope {other:?} is not clip, track or song")),
    };
    let path = match action {
        "import" => {
            let path = server.paths().read(file)?;
            if !path.is_file() {
                return Err(format!("{file}: not a file"));
            }
            path
        }
        "export" => server.paths().write(file)?,
        other => return Err(format!("action {other:?} is not import or export")),
    };
    let session = server.require_session()?;
    let before = session.shell().core().doc().clone();
    if action == "import" {
        session.shell_mut().import_midi(&path, as_song);
    } else {
        session.shell_mut().export_midi_to(scope, &path);
    }
    session.shell_mut().frame();
    let core = session.shell().core();
    Ok(ToolOutput::json(&json!({
        "action": action,
        "path": path.to_string_lossy(),
        "bytes": (action == "export").then(|| std::fs::metadata(&path).map(|m| m.len()).ok()).flatten(),
        "message": core.last_rejection(),
        "document_changed": core.doc() != &before,
        "tracks": core.doc().song.tracks.len(),
        "status_text": session.shell().status_text(),
        "dirty": core.is_dirty(),
        "can_undo": core.document().can_undo(),
    })))
}

// ------------------------------------------------------------ session_audio_notes

fn audio_notes_def() -> Value {
    definition(
        "session_audio_notes",
        "Runs Audio > Audio to Notes... on the first selected audio clip (select one with session_pointer first) with the given settings, waits for the worker and returns what it said. Anything left out takes the panel's default. The result is one undo step: a note clip that starts and ends with the audio clip, on a new poly synth track or on the note track selected in the rack. Returns `started`, `message` (the toast), `document_changed`, `can_undo`, and for a result the new clip's `notes` (`tick`, `length`, `key`, `velocity`, at most 500) with their `count`.",
        &[
            ("mode", "string", "pitched (one voice, default) or drums."),
            ("sensitivity", "integer", "0 to 100 (default 50)."),
            (
                "min_note_ms",
                "integer",
                "Pitched: the shortest note, in ms (default 60).",
            ),
            (
                "key_lo",
                "integer",
                "Pitched: lowest MIDI key (default 36).",
            ),
            (
                "key_hi",
                "integer",
                "Pitched: highest MIDI key (default 96).",
            ),
            (
                "drift_cents",
                "integer",
                "Pitched: pitch drift inside one note, in cents (default 70).",
            ),
            (
                "open_hats",
                "boolean",
                "Drums: tell open hats from closed ones (default true).",
            ),
            (
                "grid",
                "integer",
                "Beats timing: round starts to 1/N notes, N of 4, 8, 16 or 32 (default 0: free).",
            ),
            (
                "new_track",
                "boolean",
                "Put the notes on a new track (default true), else on the selected note track.",
            ),
        ],
        &[],
    )
}

fn audio_notes(server: &mut Server, args: &Args) -> Result<ToolOutput, String> {
    let defaults = NotesValues::default();
    let int = |name: &str, default: u32| -> Result<u32, String> {
        args.opt_u64(name)?.map_or(Ok(default), |v| {
            u32::try_from(v).map_err(|_| format!("{name} is out of range"))
        })
    };
    let key = |name: &str, default: u8| -> Result<u8, String> {
        let v = int(name, u32::from(default))?;
        u8::try_from(v)
            .ok()
            .filter(|k| *k <= 127)
            .ok_or_else(|| format!("{name} is not a MIDI key (0 to 127)"))
    };
    let mode = match args.opt_str("mode")? {
        None => defaults.mode,
        Some(id) => {
            NotesMode::from_id(id).ok_or_else(|| format!("mode {id:?} is not pitched or drums"))?
        }
    };
    let grid = int("grid", defaults.grid)?;
    if ![0, 4, 8, 16, 32].contains(&grid) {
        return Err("grid is not 0, 4, 8, 16 or 32".to_owned());
    }
    let values = NotesValues {
        mode,
        sensitivity: int("sensitivity", defaults.sensitivity)?.min(100),
        min_note_ms: int("min_note_ms", defaults.min_note_ms)?,
        key_lo: key("key_lo", defaults.key_lo)?,
        key_hi: key("key_hi", defaults.key_hi)?,
        drift_cents: int("drift_cents", defaults.drift_cents)?,
        open_hats: args.opt_bool("open_hats")?.unwrap_or(defaults.open_hats),
        grid,
        new_track: args.opt_bool("new_track")?.unwrap_or(defaults.new_track),
    };
    if values.key_lo > values.key_hi {
        return Err("key_lo is above key_hi".to_owned());
    }
    let session = server.require_session()?;
    // The audio of a project that was just opened loads on workers: let it arrive first.
    session.shell_mut().core_mut().wait_background();
    let before = session.shell().core().doc().clone();
    let started = session.shell_mut().audio_to_notes(&values);
    if started {
        session.shell_mut().core_mut().wait_background();
    }
    session.shell_mut().frame();
    let core = session.shell().core();
    let notes: Vec<Value> = core
        .arrangement()
        .selected_clips()
        .first()
        .and_then(|id| core.doc().song.clip(*id))
        .map(|(_, _, clip)| match &clip.content {
            atomik_model::ClipContent::Notes(n) if core.doc() != &before => n
                .iter()
                .take(500)
                .map(|n| {
                    json!({"tick": n.tick, "length": n.length, "key": n.key, "velocity": n.velocity})
                })
                .collect(),
            _ => Vec::new(),
        })
        .unwrap_or_default();
    let count = match core
        .arrangement()
        .selected_clips()
        .first()
        .and_then(|id| core.doc().song.clip(*id))
        .map(|(_, _, c)| &c.content)
    {
        Some(atomik_model::ClipContent::Notes(n)) if core.doc() != &before => n.len(),
        _ => 0,
    };
    Ok(ToolOutput::json(&json!({
        "started": started,
        "message": core.last_rejection(),
        "document_changed": core.doc() != &before,
        "tracks": core.doc().song.tracks.len(),
        "count": count,
        "notes": notes,
        "dirty": core.is_dirty(),
        "can_undo": core.document().can_undo(),
    })))
}

// -------------------------------------------------------------- session_meters

fn meters_def() -> Value {
    definition(
        "session_meters",
        "Reads what the spectrum and phase meter devices of the open project show at a position of the song, without an audio stream: the document is rendered offline up to `at_s` seconds (the code of session_bounce, so the devices see the bounce's audio), then the panels are filled with the readings (session_texts and session_screenshot show them) and the numbers are returned. Phase meter: `peak_l`, `peak_r`, `hold_l`, `hold_r`, `rms_l`, `rms_r` (linear), `sample_peak_dbfs`, `true_peak_dbtp`, `correlation` (-1 to 1), `balance` (-1 left to 1 right), `width` (0 mono, 0.5 unrelated, 1 opposite polarity), `mid_rms`, `side_rms` (linear), `momentary_lufs`, `short_term_lufs`, `integrated_lufs` (BS.1770; -120 for silence or loudness off). Spectrum: `peak_hz` and `peak_db` of the loudest bin with the device's settings, and `bands`, the 31 one-third-octave levels as `[centre Hz, dB]` (a full-scale sine reads 0 dB; -200 for silence). A project that uses third-party plugins is refused.",
        &[
            (
                "at_s",
                "number",
                "Position in seconds from the song start (default 1.0).",
            ),
            (
                "device",
                "string",
                "Only this device (its key; a container's inner device has its path key). Default: every spectrum and phase meter.",
            ),
        ],
        &[],
    )
}

/// A value for JSON: dB floors at -240 like the audio reports, and a non-finite number is null.
fn json_level(v: f32) -> Value {
    if v.is_finite() {
        json!((f64::from(v).max(-240.0) * 1000.0).round() / 1000.0)
    } else {
        Value::Null
    }
}

fn meters(server: &mut Server, args: &Args) -> Result<ToolOutput, String> {
    let at_s = args.opt_f64("at_s")?.unwrap_or(1.0);
    if !(0.0..=3_600.0).contains(&at_s) {
        return Err("at_s must be 0 to 3600 seconds".to_owned());
    }
    let only = args
        .opt_str("device")?
        .map(|k| atomik_model::DeviceKey(k.to_owned()));
    let session = server.require_session()?;
    let core = session.shell_mut().core_mut();
    core.wait_background();
    let sample_rate = f64::from(core.doc().sample_rate);
    let frames = ((at_s * sample_rate).round() as usize).max(64);
    let readouts = core.probe_meters(frames, only.as_ref())?;
    // The panels show the readings: lay them out and paint once.
    session.shell_mut().frame();
    let devices: Vec<Value> = readouts
        .iter()
        .map(|r| {
            let values: serde_json::Map<String, Value> = r
                .values
                .iter()
                .map(|(name, v)| ((*name).to_owned(), json_level(*v)))
                .collect();
            let mut d = json!({"key": r.key.0, "type": r.type_id, "values": values});
            if !r.bands.is_empty() {
                d["bands"] = Value::Array(
                    r.bands
                        .iter()
                        .map(|(hz, db)| {
                            json!([(f64::from(*hz) * 100.0).round() / 100.0, json_level(*db)])
                        })
                        .collect(),
                );
            }
            d
        })
        .collect();
    Ok(ToolOutput::json(&json!({
        "at_s": at_s,
        "frames": frames,
        "sample_rate": sample_rate,
        "devices": devices,
    })))
}

// -------------------------------------------------------------- session_relink

fn relink_def() -> Value {
    definition(
        "session_relink",
        "Relinks the missing audio of the open project, answering the dialogs of File > Relink \
Missing Audio... (session_command file.relink_audio shows the list; its buttons need a native \
dialog). With `file`, that audio file replaces the missing asset `asset` (its name, or the start \
of its id; omit it when exactly one is missing): every clip and sampler zone that used it is \
swapped in one undo step, or, if the file is the very same audio, the bundle just gets its file \
back. With `folder`, the folder is searched recursively and the missing files are matched by \
name. Waits for the import and returns what is still missing.",
        &[
            ("file", "string", "An audio file to relink one asset to."),
            (
                "asset",
                "string",
                "With file: the missing asset, by file name or id prefix.",
            ),
            (
                "folder",
                "string",
                "A folder to search for the missing files.",
            ),
        ],
        &[],
    )
}

fn relink(server: &mut Server, args: &Args) -> Result<ToolOutput, String> {
    let (file, folder, asset) = (
        args.opt_str("file")?,
        args.opt_str("folder")?,
        args.opt_str("asset")?,
    );
    let target = match (file, folder) {
        (Some(f), None) => {
            let path = server.paths().read(f)?;
            if !path.is_file() {
                return Err(format!("{f}: not a file"));
            }
            path
        }
        (None, Some(d)) => {
            if asset.is_some() {
                return Err("`asset` goes with `file`, not with `folder`".to_string());
            }
            let path = server.paths().read(d)?;
            if !path.is_dir() {
                return Err(format!("{d}: not a folder"));
            }
            path
        }
        _ => return Err("give exactly one of `file` and `folder`".to_string()),
    };
    let session = server.require_session()?;
    let core = session.shell_mut().core_mut();
    let before: Vec<_> = core.missing_assets().to_vec();
    if before.is_empty() {
        return Err("no audio file is missing in the open project".to_string());
    }
    let changed_from = core.doc().clone();
    if file.is_some() {
        let hash = pick_missing(&before, asset)?;
        core.relink_asset(&hash, target);
    } else {
        core.relink_search_folder(target);
    }
    core.wait_background();
    let changed = core.doc() != &changed_from;
    let after: Vec<Value> = core
        .missing_assets()
        .iter()
        .map(|m| json!({"hash": m.hash, "name": m.name}))
        .collect();
    let message = core.last_rejection().map(str::to_owned);
    session.shell_mut().frame();
    Ok(ToolOutput::json(&json!({
        "relinked": before.len().saturating_sub(after.len()),
        "still_missing": after,
        "document_changed": changed,
        "message": message,
        "status_text": session.shell().status_text(),
        "dirty": session.shell().core().is_dirty(),
        "can_undo": session.shell().core().document().can_undo(),
    })))
}

/// The missing asset `query` names (file name in any case, or the start of the id).
fn pick_missing(
    missing: &[atomik_render::bundle::MissingAsset],
    query: Option<&str>,
) -> Result<String, String> {
    let list = || {
        missing
            .iter()
            .map(|m| format!("{} ({})", m.name, &m.hash[..8.min(m.hash.len())]))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let Some(query) = query else {
        return match missing {
            [only] => Ok(only.hash.clone()),
            _ => Err(format!(
                "{} assets are missing; name one with `asset`: {}",
                missing.len(),
                list()
            )),
        };
    };
    let lower = query.to_lowercase();
    let hits: Vec<&atomik_render::bundle::MissingAsset> = missing
        .iter()
        .filter(|m| {
            m.name.to_lowercase() == lower || (!query.is_empty() && m.hash.starts_with(query))
        })
        .collect();
    match hits.as_slice() {
        [one] => Ok(one.hash.clone()),
        [] => Err(format!("`{query}` is not a missing asset: {}", list())),
        _ => Err(format!(
            "`{query}` names several missing assets: {}",
            list()
        )),
    }
}

// -------------------------------------------------------------- session_bounce

fn bounce_def() -> Value {
    definition(
        "session_bounce",
        "Bounces the session's document as it is now (edits included, saved or not) to a WAV, \
and returns the audio report; with `reference`, compares with that WAV. The same renderer as \
`bounce`. Audio clips play from the session's bundle (the project's own folder, or the scratch \
bundle of imports).",
        &[
            (
                "output",
                "string",
                "Output WAV (default target/mcp/session.wav).",
            ),
            ("format", "string", "f32 (default) or pcm24."),
            (
                "block",
                "integer",
                "Host block size in frames (default 512).",
            ),
            ("workers", "integer", "Engine worker threads (default 0)."),
            (
                "loop_count",
                "integer",
                "Play the song loop this many times.",
            ),
            (
                "window_s",
                "number",
                "Loudness window in seconds (default 0.5).",
            ),
            ("reference", "string", "A WAV to compare the render with."),
            (
                "tolerance_db",
                "number",
                "With reference: allowed loudness difference per window (default 0.5).",
            ),
        ],
        &[],
    )
}

fn bounce(server: &mut Server, args: &Args) -> Result<ToolOutput, String> {
    let block = args.opt_u64("block")?.unwrap_or(512);
    if !(1..=1_048_576).contains(&block) {
        return Err("block must be 1..=1048576".to_string());
    }
    let workers = u16::try_from(args.opt_u64("workers")?.unwrap_or(0))
        .ok()
        .filter(|w| *w <= 64)
        .ok_or("workers must be 0..=64")?;
    let loop_count = match args.opt_u64("loop_count")? {
        None => None,
        Some(n) => Some(
            u32::try_from(n)
                .ok()
                .filter(|n| (1..=10_000).contains(n))
                .ok_or("loop_count must be 1..=10000")?,
        ),
    };
    let format = match args.opt_str("format")? {
        None | Some("f32") => SampleFormat::F32,
        Some("pcm24") => SampleFormat::Pcm24,
        Some(other) => return Err(format!("format must be f32 or pcm24, not `{other}`")),
    };
    let out = match args.opt_str("output")? {
        Some(o) => server.paths().write(o)?,
        None => server
            .paths()
            .write(&server.paths().default_output("session.wav"))?,
    };
    let reference = args
        .opt_str("reference")?
        .map(|r| server.paths().read(r).map(|p| (r.to_string(), p)))
        .transpose()?;
    let session = server.require_session()?;
    let core = session.shell().core();
    let b = tier1::render_doc_checked(
        core.doc(),
        core.registry(),
        block as usize,
        workers,
        loop_count,
        core.bundle_root(),
    )?;
    write_wav_file(&out, &b.channels, b.sample_rate, format).map_err(|e| e.to_string())?;
    let window_s = args.opt_f64("window_s")?.unwrap_or(0.5);
    let mut v = bounce_json(&b, window_s);
    v["output"] = json!(out.to_string_lossy());
    if let Some((name, path)) = reference {
        let wav = read_wav_file(&path).map_err(|e| e.to_string())?;
        let c = analysis::compare(
            &b.channels,
            b.sample_rate,
            &wav.channels,
            wav.sample_rate,
            window_s,
        );
        v["comparison"] = c.to_json(args.opt_f64("tolerance_db")?.unwrap_or(0.5));
        v["comparison"]["reference"] = json!(name);
    }
    Ok(ToolOutput::json(&v))
}

// -------------------------------------------------------------- session_export

fn export_def() -> Value {
    definition(
        "session_export",
        "Exports the session's document as an audio file, answering the dialogs of File > Export \
Song / Loop as Audio File... (`session_command` file.export_song_audio and file.export_loop_audio \
raise a native dialog, reported as `pick_export_audio`). The same renderer and rules as the \
app's export, run synchronously: `span` song is from the start to the End marker plus the tail \
that is still audible (at most 10 s, cut where the output stays below -90 dBFS; `tail` false \
ends at the End), `span` loop is exactly the frames between the locators with no tail. `format` \
is pcm16 (dithered, see `dither`), pcm24 (no dither, the default) or f32; `sample_rate` converts \
from the project's rate. The file is read back for the audio report. Returns `path`, `bytes`, \
`format`, `dither`, `sample_rate`, `frames`, `seconds`, `span_frames`, `tail_frames`, `warnings` \
and `audio`.",
        &[
            ("file", "string", "The WAV file to write."),
            ("span", "string", "song (default) or loop."),
            ("format", "string", "pcm16, pcm24 (default) or f32."),
            (
                "dither",
                "string",
                "pcm16 only: none, tpdf (default) or shaped.",
            ),
            (
                "sample_rate",
                "integer",
                "Convert to this rate in Hz (8000 to 192000; default the project's).",
            ),
            (
                "tail",
                "boolean",
                "Song only: keep the audible tail after the End (default true).",
            ),
            (
                "window_s",
                "number",
                "Loudness window in seconds for the report (default 0.5).",
            ),
        ],
        &["file"],
    )
}

fn export(server: &mut Server, args: &Args) -> Result<ToolOutput, String> {
    use atomik_render::export::{
        BitDepth, DEFAULT_TAIL_SECONDS, DitherMode, ExportFormat, ExportRequest, ExportSpan,
        export_audio_file,
    };
    let file = args.str("file")?;
    let span = match args.opt_str("span")?.unwrap_or("song") {
        "song" => ExportSpan::Song,
        "loop" => ExportSpan::Loop,
        other => return Err(format!("span must be song or loop, not `{other}`")),
    };
    let bit_depth = match args.opt_str("format")?.unwrap_or("pcm24") {
        "pcm16" => BitDepth::Pcm16,
        "pcm24" => BitDepth::Pcm24,
        "f32" => BitDepth::Float32,
        other => return Err(format!("format must be pcm16, pcm24 or f32, not `{other}`")),
    };
    let dither = match args.opt_str("dither")? {
        None => bit_depth.default_dither(),
        Some(_) if bit_depth != BitDepth::Pcm16 => {
            return Err("dither applies to pcm16 only".to_string());
        }
        Some("none") => DitherMode::None,
        Some("tpdf") => DitherMode::Tpdf,
        Some("shaped") => DitherMode::Shaped,
        Some(other) => {
            return Err(format!(
                "dither must be none, tpdf or shaped, not `{other}`"
            ));
        }
    };
    let sample_rate = args
        .opt_u64("sample_rate")?
        .map(|r| {
            u32::try_from(r)
                .ok()
                .filter(|r| (8_000..=192_000).contains(r))
                .ok_or("sample_rate must be 8000..=192000")
        })
        .transpose()?;
    let tail = args.opt_bool("tail")?.unwrap_or(true);
    let window_s = args.opt_f64("window_s")?.unwrap_or(0.5);
    let out = server.paths().write(file)?;
    let session = server.require_session()?;
    let core = session.shell().core();
    let doc = core.doc();
    if !doc.plugin_devices().is_empty() {
        return Err(
            "this project uses third-party plugins; export it from the app (with the plugin \
             sandbox on)"
                .to_string(),
        );
    }
    let options = atomik_render::project::ProjectOptions {
        host_block: std::num::NonZeroUsize::new(512).unwrap_or(std::num::NonZeroUsize::MIN),
        workers: 0,
        loop_count: None,
    };
    let request = ExportRequest {
        span,
        tail_seconds: if span == ExportSpan::Song && tail {
            DEFAULT_TAIL_SECONDS
        } else {
            0.0
        },
    };
    let format = ExportFormat {
        bit_depth,
        dither,
        sample_rate,
    };
    let report = export_audio_file(
        doc,
        core.registry(),
        (&options, core.bundle_root()),
        &mut atomik_render::project::PluginBounce::default(),
        (&request, &format),
        (&out, atomik_render::plugin_pass::RenderControl::default()),
    )
    .map_err(|e| e.to_string())?
    .ok_or("the export was cancelled")?;
    let written = read_wav_file(&out).map_err(|e| e.to_string())?;
    Ok(ToolOutput::json(&json!({
        "path": out.to_string_lossy(),
        "bytes": std::fs::metadata(&out).map(|m| m.len()).ok(),
        "format": match bit_depth {
            BitDepth::Pcm16 => "pcm16",
            BitDepth::Pcm24 => "pcm24",
            BitDepth::Float32 => "f32",
        },
        "dither": match (bit_depth, dither) {
            (BitDepth::Pcm16, DitherMode::None) => "none",
            (BitDepth::Pcm16, DitherMode::Tpdf) => "tpdf",
            (BitDepth::Pcm16, DitherMode::Shaped) => "shaped",
            _ => "none",
        },
        "sample_rate": report.sample_rate,
        "frames": report.frames,
        "seconds": report.frames as f64 / f64::from(report.sample_rate),
        "span_frames": report.span_frames,
        "tail_frames": report.tail_frames,
        "warnings": report.warnings.iter().map(ToString::to_string).collect::<Vec<_>>(),
        "audio": analysis::report(&written.channels, written.sample_rate, window_s),
    })))
}

// --------------------------------------------------------------- session_close

fn close_def() -> Value {
    definition(
        "session_close",
        "Closes the open session without saving.",
        &[],
        &[],
    )
}

fn close(server: &mut Server, _args: &Args) -> Result<ToolOutput, String> {
    match server.session_mut() {
        None => Ok(ToolOutput::json(
            &json!({"closed": false, "note": "no open session"}),
        )),
        Some(s) => {
            s.shell_mut().shutdown();
            server.set_session(None);
            Ok(ToolOutput::json(&json!({"closed": true})))
        }
    }
}
