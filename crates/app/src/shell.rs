//! [`Shell`]: everything inside the window, without the window (no winit, no GPU).
//!
//! # Layout
//!
//! Top to bottom: the in-window menu bar (Windows and Linux), the rack view, a draggable
//! divider, the transport bar ([`TRANSPORT_HEIGHT`]), the lower pane and the status bar. The
//! lower pane shows the arrangement, the piano roll in its place while a clip is open, or the
//! console (Mixer ▸ Console / F5 swaps it with the arrangement). View ▸
//! Arrangement hides the lower pane (the rack then reaches the transport bar).
//!
//! **Maximised console** (Mixer ▸ Maximise Console / Shift+F5, or the console's MAX button):
//! the console fills everything from the menu bar to the transport bar, which moves to the
//! bottom; the rack is hidden. The divider position and the pane flags are app state
//! ([`UiState`], `ui-state.json`), not project data.
//!
//! # Windows
//!
//! The rack and the console can leave the main window (View ▸ Detach Rack / Detach Mixer, see
//! the `shell_windows` module): the layout above then leaves them out, and each detached pane
//! fills a window of its own. The shell stays the one owner of the document and the views and
//! knows each open window's size and scale; the platform opens the windows
//! ([`Shell::windows_wanted`]) and sends each one's events with [`Shell::handle_in`]
//! (and [`Shell::handle_routed`] while a file drag can cross windows), paints each with
//! [`Shell::paint_in`] and reports focus and geometry ([`Shell::window_focused`],
//! [`Shell::window_moved`]). The keys are global: the focused pane decides the keymap context
//! whatever window it is in.
//!
//! # Console hosting
//!
//! The console view follows the `atomik-ui` hosting contract: it gets its bounds and scale
//! with the other panes, the pointer and key routing below, and its `ToggleConsole` /
//! `MaximiseConsole` actions (toolbar buttons) run the Mixer commands. Each frame while it
//! shows, [`Shell::frame_at`] feeds it the engine meters and the monotonic time
//! ([`AppCore::update_console_meters`]). While the routing hits the 10 s compensation limit,
//! the status bar shows the alert.
//!
//! # Sample editor hosting
//!
//! The sample editor replaces the arrangement in the lower pane while a sampler is open
//! ([`Pane::SampleEditor`]: View ▸ Sample Editor, a sampler panel's Edit button or
//! double-click; the console wins over it, so opening it from the rack hides the console).
//! It gets the arrangement's bounds, scale, focus and pointer capture; file drops go to the
//! view under the pointer (the arrangement, the editor or the rack's sampler panels), and its
//! Load button becomes [`ShellRequest::PickLoadSamples`].
//!
//! # Step editor hosting
//!
//! [`Pane::StepEditor`] is hosted the same way (View ▸ Step Editor on the selected, else the
//! first drum machine; a drum panel's Edit button; a double-click on a note clip of a drum
//! machine's track, which the app turns from the piano roll into the step editor). It is
//! exclusive with the piano roll and the sample editor; files dropped on its row headers load
//! that pad, elsewhere on it they fill the empty pads. View ▸ Piano Roll still opens a drum
//! clip in the piano roll.
//!
//! # Slice editor hosting
//!
//! [`Pane::SliceEditor`] is hosted like the sample editor (View ▸ Slice Editor on the selected,
//! else the first loop slicer; a slicer panel's Edit button): exclusive with the piano roll,
//! the sample editor and the step editor, same bounds, scale, focus, pointer capture and file
//! drops (a dropped loop replaces the slicer's loop and is sliced); its Load button becomes
//! [`ShellRequest::PickLoadSamples`] too. Esc, its close button and the slicer's removal show
//! the arrangement again.
//!
//! # File browser hosting
//!
//! View ▸ File Browser (`view.toggle_file_browser`, F4) docks the file browser panel
//! ([`Pane::FileBrowser`]) as a full-height column left of everything else, from the menu bar to
//! the status bar ([`Layout::file_browser`]). Its width is the panel's own (it resizes by
//! dragging its right edge; the shell lays out again while it moves and saves on release), kept
//! so the rest of the window has [`MIN_REST_WIDTH`]; the rack, divider, transport bar and lower
//! pane all start at its right edge (the rack view's pointer events are shifted by its
//! origin). The plugin banner starts there too. Clicking the panel focuses it; hiding it moves
//! the focus to the rack.
//!
//! While the panel has the focus the key context is `Files` (over `Global`): its keys are the
//! `files.*` commands, which the shell hands to the panel as the key they stand for
//! (`files_key`), so a remap of the chord still drives the panel and they win over global
//! chords such as Space (play) and Backspace (delete). While its search or tag field owns the
//! keyboard the context is `Text` over `Files`: plain keys are typed, chords with the primary
//! modifier, function keys and Esc resolve as commands (Esc reaches the field as
//! `files.stop`). F4 and the other global chords run in both.
//!
//! The panel's `DragFiles` action starts a shell-owned drag ([`Capture::FileDrag`]): moves are
//! file hovers over the pane under the pointer (the same targeting as OS drops), the release
//! is a file drop exactly like an OS drop, and a release over the panel or over nothing that
//! takes files, or Esc, cancels. A small label (the file name, or the count) follows the
//! pointer. Width, split, auto-preview, visibility and the last folder are saved in
//! `ui-state.json` ([`UiState`]); the library file is `ShellOptions::library_path`, written on
//! [`Shell::shutdown`].
//!
//! # Routing
//!
//! - **Pointer** events go to the pane under the pointer (the rack view gets its own
//!   coordinates, shifted below the menu bar; the sequencer views get window coordinates).
//!   A press captures the pointer for that pane (or the divider) until the release, so drags
//!   that leave the pane keep working. When the pointer moves to another pane the previous
//!   one gets a `PointerLeave`.
//! - **Focus**: the last pane clicked receives the keys, except that a plain click selecting one
//!   console strip hands them to the rack (Shift/Ctrl clicks and renames keep the console).
//! - **Keys**: the key router (`atomik_ui::keymap`) first. The shell computes a
//!   context stack from the focused pane and the text state ([`Shell::context_stack`]), the
//!   router resolves the chord against the active keymap profile and gives the commands to try
//!   (most specific context first, then Global); the first the shell can run wins and a
//!   command that is not the shell's (a view's own key) declines, so the key goes on to the
//!   focused view. While the transport bar edits the tempo, every key goes there; while a text
//!   field has focus (the browser filter, a rename) only chords with the primary modifier,
//!   function keys and Esc are commands. Key repeat is derived from the held keys, releases
//!   are delivered to the focused view, and numpad keys that no command took reach the views
//!   as the characters they type. The app profile is [`APP_DEFAULT_PROFILE`] unless the user
//!   chose another; the user's remaps (`keymap.json`) and the workflow preferences
//!   (`workflow.json`) are read next to `settings.json`, applied by the Settings dialog (Keyboard
//!   and Workflow tabs) and hot-reloaded ([`Shell::reload_keymap_file`]).
//! - **Commands**: every command the router resolves is offered to the focused
//!   view first (`command()` of the arrangement, the piano roll, the rack or the step, sample and slice
//!   editors: a view's own commands such as duplicate, delete, split, quantize, tools, zoom), then to the shell
//!   itself (transport steps, pane toggles, create keys, the menu commands). The first that
//!   takes it wins, so a user binding in a context really shadows a `Global` binding of the
//!   same chord. A key that no command took reaches the view as a key (text, Escape).
//!
//! # Layout flags
//!
//! F6 hides the rack (`rack_visible` in `workflow.json`, view state, not in the project): the
//! transport bar moves to the top and the song area takes the rest. F7 (sequencer only) hides
//! the rack and the console and remembers the layout it replaced; pressing it again restores
//! that layout. F8 opens the editor of the selected clip or closes the open editor.
//!
//! What needs the platform (file dialogs, confirmation, quit) is returned as
//! [`ShellRequest`]s.

use crate::app_core::{AppCore, AppError, FreezeThrough, MidiExport, Pane, Revealed};
use crate::commands::{AppCommand, Menu, MenuState, menus};
use crate::context_menus::{self, MenuEnable};
use crate::export_prefs::ExportPrefs;
use crate::keymap_store::{self, WorkflowPrefs};
use crate::menubar::{MENU_BAR_HEIGHT, MenuBar, MenuBarOutcome};
use crate::session::{AudioBackend, LiveSession, SettingsChange};
use crate::settings::{self, AudioSettings, SurfaceSettings};
use crate::settings_dialog::{DeviceLists, DialogOutcome, SettingsDialog};
use crate::status::status_text;
use crate::stress::{STRESS_CABLES, STRESS_DEVICES, stress_doc, stress_song_doc};
use crate::ui_state::UiState;
use crate::windows::{WindowHit, WindowKind};
use atomik_model::{ClipId, Command, ConsoleCommand, DeviceKey};
use atomik_ui::ContextTarget;
use atomik_ui::CursorHint;
use atomik_ui::browser::CreateKind;
use atomik_ui::context_menu::{ContextMenu, MenuOutcome};
use atomik_ui::keymap::{
    Context, ContextStack, Keymap, KeymapEditor, Os, Profile, Roles, Route, Router, UserKeymap,
};
use atomik_ui::kurbo::{Affine, Point, Rect, RoundedRect, Size};
use atomik_ui::transport::TRANSPORT_HEIGHT;
use atomik_ui::{
    BrowseAction, Canvas, CanvasExt, ConsoleButton, ExportScope, ExportView, FileBrowserView, Flip,
    Key, MacrosAction, MacrosView, MidiMapsAction, MidiMapsView, Modifiers, NotesAction,
    NotesValues, NotesView, NumpadKey, ParamSearchAction, ParamSearchView, Place, PluginBannerView,
    PluginSettingsView, PointerButton, QuantizeAction, QuantizeValues, QuantizeView, RelinkAction,
    RelinkEntry, RelinkView, SilenceAction, SilenceValues, SilenceView, TextRun, UiAction, UiEvent,
    WarpAction, WarpValues, WarpView,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// The keymap profile the app starts with: the standard
/// keys. The `legacy` profile (the keys of earlier versions) stays selectable in Settings > Keyboard.
pub const APP_DEFAULT_PROFILE: Profile = Profile::Standard;

/// The one-time notice: the console devices show in the rack now.
pub const CONSOLE_DEVICES_NOTICE: &str = "Console devices are now shown in the rack. View > Show Console Devices in Rack hides them again.";

/// Height of the status bar, logical pixels.
pub const STATUS_BAR_HEIGHT: f64 = 22.0;
/// Height of the rack / arrangement divider, logical pixels.
pub const DIVIDER_HEIGHT: f64 = 6.0;
/// The width the window keeps for everything but the file browser column.
pub const MIN_REST_WIDTH: f64 = 400.0;
/// Smallest height the divider leaves to the rack and to the song area.
pub const MIN_PANE_HEIGHT: f64 = 80.0;

/// Something only the platform layer can do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellRequest {
    /// Show an open-file dialog (projects and patches), then call [`Shell::open_file`].
    PickOpenFile,
    /// Show a save-file dialog for a project, then call [`Shell::save_to`].
    PickSaveFile,
    /// Show a save-file dialog for a patch, then call [`Shell::export_to`].
    PickExportFile,
    /// Show a multi-select open dialog for audio files (`audio_import::AUDIO_EXTENSIONS`),
    /// then call [`Shell::import_files`] with the picked paths.
    PickImportFiles,
    /// Show an open dialog for a MIDI file (File > Import MIDI), then call
    /// [`Shell::import_midi`] with the picked path.
    PickImportMidi {
        /// Import as a song: new tracks from the start, with the tempo, meter and markers.
        as_song: bool,
    },
    /// Show a save dialog for an audio file (File > Export Song / Loop as Audio File): a WAV file
    /// named by [`Shell::export_audio_name`], starting in [`Shell::export_audio_dir`], then call
    /// [`Shell::export_audio_to`] with the scope and the path.
    PickExportAudio(ExportScope),
    /// Show a save dialog for a MIDI file (File > Export MIDI), then call
    /// [`Shell::export_midi_to`] with the scope and the path.
    PickExportMidi(MidiExport),
    /// Show a multi-select open dialog for audio files (the sample editor's Load button), then
    /// call [`Shell::load_samples`] with the picked paths.
    PickLoadSamples,
    /// Show an open dialog for one audio file (the relink panel's LOCATE...), then call
    /// [`Shell::relink_file`] with the picked path.
    PickRelinkFile,
    /// Show a folder dialog (the relink panel's SEARCH FOLDER...), then call
    /// [`Shell::relink_search_folder`] with the picked folder.
    PickRelinkFolder,
    /// Show a save dialog for a MIDI mapping preset (Mixer > Export MIDI Mapping Preset, the
    /// panel's EXPORT), starting in [`Shell::midi_preset_dir`], then call
    /// [`Shell::export_midi_preset_to`] with the path.
    PickExportMidiPreset,
    /// Show an open dialog for a MIDI mapping preset (Mixer > Import MIDI Mapping Preset, the
    /// panel's IMPORT), starting in [`Shell::midi_preset_dir`], then call
    /// [`Shell::import_midi_preset_from`] with the path.
    PickImportMidiPreset,
    /// The rack has unsaved changes: ask, then call [`Shell::confirmed`] with the command.
    ConfirmDiscard(AppCommand),
    /// A project was picked in the library (or dropped on the window) and the rack has unsaved
    /// changes: ask, then call [`Shell::open_file`] with the path.
    ConfirmDiscardOpen(PathBuf),
    /// Show a folder dialog (the Plugins settings' ADD FOLDER), then call
    /// [`Shell::add_plugin_folder`] with the picked folder.
    PickPluginFolder,
    /// Show a folder dialog (the Workflow tab's Library folder Choose...), then call
    /// [`Shell::set_library_folder`] with the picked folder.
    PickLibraryFolder,
    /// Show a save-file dialog for the keymap (the Keyboard tab's Export), then call
    /// [`Shell::export_keymap_to`].
    PickExportKeymap,
    /// Show an open-file dialog for a keymap (the Keyboard tab's Import), then call
    /// [`Shell::import_keymap_from`].
    PickImportKeymap,
    /// Bring a window to the front and give it the keyboard focus (restoring it if it is
    /// minimised).
    FocusWindow(WindowKind),
    /// Maximise a window, or restore it if it is maximised (the console's MAX button and
    /// Shift+F5 while the console is detached).
    ToggleMaximise(WindowKind),
    /// Close the window and exit.
    Quit,
}

/// How the shell is set up.
#[derive(Debug, Clone, Default)]
pub struct ShellOptions {
    /// Draw the menu bar in the window (false on macOS, which uses native menus).
    pub in_window_menu: bool,
    /// Where applied settings are saved (`None`: not saved).
    pub settings_path: Option<PathBuf>,
    /// Where the window state is read and saved (`None`: defaults, not saved).
    pub ui_state_path: Option<PathBuf>,
    /// The OS whose keys and shortcut texts apply (`None`: the host; tests pass it).
    pub os: Option<Os>,
    /// The keymap profile (`None`: [`APP_DEFAULT_PROFILE`]).
    pub profile: Option<Profile>,
    /// Where the file browser's library (favourites, tags, bookmarks) is kept
    /// (`<config dir>/library.json`; `None`: in memory only).
    pub library_path: Option<PathBuf>,
    /// The library folder for this run (`--library`; tests and the MCP server), taking
    /// precedence over the Workflow tab's choice and the default (`None`: those).
    pub library_root: Option<PathBuf>,
}

/// Where the window's parts are, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    /// The rack view.
    pub rack: Rect,
    /// The divider (only while the song area shows).
    pub divider: Option<Rect>,
    /// The transport bar.
    pub transport: Rect,
    /// The lower pane: arrangement, piano roll or console (only while it shows).
    pub song: Option<Rect>,
    /// The file browser column (only while it shows): the full height below the menu bar, to
    /// the left of everything else.
    pub file_browser: Option<Rect>,
}

/// A drag of files that left the file browser: the shell owns it until the release.
struct FileDrag {
    paths: Vec<PathBuf>,
    /// Where the pointer last was, in the window it is over (the label follows it); `None`
    /// before the first move or while it is over no window.
    pos: Option<(WindowKind, Point)>,
    /// Esc was pressed: nothing is delivered any more; the release ends the gesture.
    cancelled: bool,
}

/// An open context menu, the pane whose selection its commands work on and the window it is
/// open in.
struct OpenContextMenu {
    menu: ContextMenu,
    pane: Pane,
    window: WindowKind,
}

/// What holds the pointer between a press and its release.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Capture {
    Pane(Pane),
    /// Divider drag; `grab` is the pointer's offset from the divider top.
    Divider {
        grab: f64,
    },
    /// A drag of files out of the file browser: the shell delivers it to the pane under the
    /// pointer as file hovers and a drop.
    FileDrag,
}

/// The window content.
pub struct Shell<B: AudioBackend> {
    core: AppCore<LiveSession<B>>,
    menubar: Option<MenuBar>,
    /// The open context menu (a right click) and the pane it belongs to.
    context_menu: Option<OpenContextMenu>,
    dialog: Option<SettingsDialog>,
    settings_path: Option<PathBuf>,
    /// The surface settings the control surface was last set up with.
    surface_applied: SurfaceSettings,
    ui_state_path: Option<PathBuf>,
    ui: UiState,
    /// The open windows: the main window always, a detached pane's while it is detached.
    wins: BTreeMap<WindowKind, WinState>,
    status: String,
    repaint: bool,
    /// What holds the pointer, and the window the press happened in.
    capture: Option<(WindowKind, Capture)>,
    focus: Pane,
    /// The window that has the OS focus (the keys come from it).
    focused: WindowKind,
    /// A window moved or resized since the geometry was last written.
    geom_dirty: bool,
    /// When (the `frame_at` clock) the geometry was last written.
    geom_saved_at: f64,
    /// Requests raised outside `handle`'s own flow (view actions); returned by the next
    /// `handle`.
    pending: Vec<ShellRequest>,
    /// The sampler the Load dialog is for.
    load_device: Option<DeviceKey>,
    /// The plugin banner above the rack: the project uses plugins, or the last run
    /// ended unexpectedly.
    banner: PluginBannerView,
    /// The Plugins settings modal while it is open.
    plugin_settings: Option<PluginSettingsView>,
    /// The audio export panel (File > Export Song / Loop as Audio File) while it is open.
    export: Option<ExportView>,
    /// What the panel exports and where: the scope and the file the dialog picked.
    export_target: Option<(ExportScope, PathBuf)>,
    /// The remembered export choices (`export.json`).
    export_prefs: ExportPrefs,
    /// Where they are saved (next to the settings file).
    export_prefs_path: Option<PathBuf>,
    /// The relink panel (File > Relink Missing Audio) while it is open.
    relink: Option<RelinkView>,
    /// The MIDI Mappings panel (Mixer > MIDI Mappings) while it is open.
    midi_maps: Option<MidiMapsView>,
    /// The Macros panel (Mixer > Macros) while it is open.
    macros_panel: Option<MacrosView>,
    /// The parameter search (View > Find Parameter) while it is open.
    param_search: Option<ParamSearchView>,
    /// The control a right click opened the MIDI menu for (what Learn MIDI and Forget MIDI act on).
    control_target: Option<(DeviceKey, String)>,
    /// The quantize panel (Edit > Quantize Options) while it is open.
    quantize: Option<QuantizeView>,
    /// The panel's settings, kept between openings (view state, not in the project).
    quantize_values: QuantizeValues,
    /// The strip silence panel (Edit > Strip Silence) while it is open.
    silence: Option<SilenceView>,
    /// The Audio to Notes panel (Audio > Audio to Notes) while it is open.
    notes: Option<NotesView>,
    /// Its settings, kept between openings.
    notes_values: NotesValues,
    /// The warp panel (Audio > Warp Settings) while it is open.
    warp: Option<WarpView>,
    /// The warp panel's last values, for the next time it opens without a clip to read.
    warp_values: WarpValues,
    /// Its settings, kept between openings.
    silence_values: SilenceValues,
    /// The missing asset the open file dialog is for (LOCATE...).
    relink_target: Option<String>,
    /// The key router: the keymap and the held keys.
    router: Router,
    /// The modifiers last reported by `UiEvent::ModifiersChanged` (for hold-style roles).
    modifiers: Modifiers,
    /// The OS whose keys and texts apply.
    os: Os,
    /// The profile used until the user chooses one (`keymap.json` may name another).
    default_profile: Profile,
    /// `keymap.json` and `workflow.json` (next to `settings.json`; `None`: not saved).
    keymap_path: Option<PathBuf>,
    workflow_path: Option<PathBuf>,
    /// The user's changes over the profile, as loaded or last applied.
    user_keymap: UserKeymap,
    /// The modifier roles with the user's overrides.
    roles: Roles,
    /// The workflow preferences.
    workflow: WorkflowPrefs,
    /// The library folder forced for this run ([`ShellOptions::library_root`]).
    library_override: Option<PathBuf>,
    /// The folder used when neither the override nor the Workflow tab names one.
    library_default: Option<PathBuf>,
    /// Modification time and size of `keymap.json` as last read or written, to notice edits.
    keymap_stamp: Option<(SystemTime, u64)>,
    /// The layout F7 (sequencer only) replaced, to restore on the next F7.
    seq_only_prev: Option<SeqOnlyPrev>,
    /// The right edge of the file browser column as last laid out (0 while hidden), to notice a
    /// resize drag of the panel.
    browser_edge: f64,
    /// The file drag in progress (see [`Capture::FileDrag`]).
    file_drag: Option<FileDrag>,
    /// The help tooltip: what the pointer rests on and whether it shows yet.
    tips: Tips,
}

/// The layout flags F7 remembers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SeqOnlyPrev {
    rack: bool,
    console: bool,
    maximised: bool,
}

/// When and how big `path` is (`None` when it does not exist).
fn stamp_of(path: &Path) -> Option<(SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

impl<B: AudioBackend> std::fmt::Debug for Shell<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shell")
            .field("size", &self.size())
            .field("scale", &self.scale())
            .field("status", &self.status)
            .field("ui", &self.ui)
            .field("focus", &self.focus)
            .finish_non_exhaustive()
    }
}

impl<B: AudioBackend> Shell<B> {
    /// Wraps a core.
    pub fn new(core: AppCore<LiveSession<B>>, options: ShellOptions) -> Self {
        let banner = PluginBannerView::new(core.view().theme().clone());
        let mut ui = options
            .ui_state_path
            .as_deref()
            .map(UiState::load)
            .unwrap_or_default();
        // The one-time notice that console devices are shown now. Only with
        // a state file to remember it in, and not for a user who switched them off already.
        let console_notice = (options.ui_state_path.is_some() && !ui.console_notice_seen)
            .then_some(ui.show_console_devices);
        if console_notice.is_some() {
            ui.console_notice_seen = true;
        }
        let os = options.os.unwrap_or_else(Os::host);
        let default_profile = options.profile.unwrap_or(APP_DEFAULT_PROFILE);
        let keymap_path = options
            .settings_path
            .as_deref()
            .map(keymap_store::keymap_path);
        let workflow_path = options
            .settings_path
            .as_deref()
            .map(keymap_store::workflow_path);
        let loaded_keymap = keymap_path.as_deref().map(keymap_store::load_user_keymap);
        let loaded_workflow = workflow_path.as_deref().map(keymap_store::load_workflow);
        let mut startup_notes: Vec<String> = Vec::new();
        let user_keymap = match loaded_keymap {
            Some(l) => {
                startup_notes.extend(l.status);
                l.value
            }
            None => UserKeymap::default(),
        };
        let workflow = match loaded_workflow {
            Some(l) => {
                startup_notes.extend(l.status);
                l.value
            }
            None => WorkflowPrefs::default(),
        };
        let built = user_keymap.build(user_keymap.profile.unwrap_or(default_profile), os);
        startup_notes.extend(built.report.summary());
        let keymap_stamp = keymap_path.as_deref().and_then(stamp_of);
        let export_prefs_path = Self::export_prefs_file(options.settings_path.as_deref());
        let options_library_path = options.library_path.clone();
        let library_override = options.library_root.clone();
        let mut shell = Self {
            core,
            menubar: options.in_window_menu.then(MenuBar::new),
            context_menu: None,
            dialog: None,
            settings_path: options.settings_path,
            surface_applied: SurfaceSettings::default(),
            ui_state_path: options.ui_state_path,
            ui,
            wins: BTreeMap::from([(WindowKind::Main, WinState::new(MAIN_SIZE, 1.0, Pane::Rack))]),
            status: String::new(),
            repaint: true,
            capture: None,
            focus: Pane::Rack,
            focused: WindowKind::Main,
            geom_dirty: false,
            geom_saved_at: f64::NEG_INFINITY,
            pending: Vec::new(),
            load_device: None,
            banner,
            plugin_settings: None,
            relink: None,
            export: None,
            export_target: None,
            export_prefs: export_prefs_path
                .as_deref()
                .map(ExportPrefs::load)
                .unwrap_or_default(),
            export_prefs_path,

            quantize: None,
            quantize_values: QuantizeValues::default(),
            silence: None,
            notes: None,
            notes_values: NotesValues::default(),
            warp: None,
            warp_values: WarpValues::default(),
            silence_values: SilenceValues::default(),
            relink_target: None,
            midi_maps: None,
            macros_panel: None,
            param_search: None,
            control_target: None,
            router: Router::new(built.keymap),
            modifiers: Modifiers::NONE,
            os,
            default_profile,
            keymap_path,
            workflow_path,
            user_keymap,
            roles: built.roles,
            workflow,
            library_override,
            library_default: None,
            keymap_stamp,
            seq_only_prev: None,
            browser_edge: 0.0,
            file_drag: None,
            tips: Tips::default(),
        };
        shell.core.set_roles(&shell.roles);
        shell.configure_surface();
        shell.core.set_library_path(options_library_path);
        shell.sync_library_root();
        // Before anything saves the window state: a save copies the panel's values.
        shell.restore_file_browser();
        shell.restore_windows();
        shell.apply_workflow_to_views();
        // The rack's console display preferences (shown unless the user
        // switched them off).
        {
            let view = shell.core.view_mut();
            view.set_show_console_devices(shell.ui.show_console_devices);
            view.set_show_console_links(shell.ui.show_console_links);
            view.set_navigator_visible(shell.ui.show_rack_navigator);
        }
        {
            let arrangement = shell.core.arrangement_mut();
            arrangement.set_tempo_lane(shell.ui.show_tempo_lane);
            arrangement.set_marker_strip(shell.ui.show_marker_strip);
            arrangement.set_marker_list(shell.ui.show_marker_list);
        }
        shell
            .core
            .arrangement_mut()
            .set_track_zoom(shell.ui.track_zoom);
        shell
            .core
            .piano_roll_mut()
            .set_key_height(shell.ui.piano_roll_key_height);
        {
            let roll = shell.core.piano_roll_mut();
            roll.set_snap_to_scale(shell.ui.piano_roll_snap_scale);
            roll.set_fold(shell.ui.piano_roll_fold);
        }
        if console_notice == Some(true) {
            shell.notify(CONSOLE_DEVICES_NOTICE);
        }
        if console_notice.is_some() {
            shell.save_ui_state();
        }
        // The notice of the moved keys: a keymap problem found at start wins the status line.
        if shell.workflow_path.is_some()
            && !shell.workflow.keys_notice_seen
            && shell.profile() == Profile::Standard
        {
            let text = shell.keys_notice();
            shell.notify(text);
            shell.workflow.keys_notice_seen = true;
            shell.save_workflow();
        }
        for note in startup_notes {
            shell.notify(note);
        }
        shell.core.set_input_quantize(shell.ui.input_quantize);
        let gain = shell.core.sink().settings().metronome_gain;
        if (gain - shell.core.metronome_gain()).abs() > f32::EPSILON {
            shell.core.set_metronome_gain(gain);
        }
        shell.layout_view();
        shell.poll_status();
        shell
    }

    /// The core.
    pub fn core(&self) -> &AppCore<LiveSession<B>> {
        &self.core
    }

    /// The plugin banner above the rack.
    pub fn banner_view(&self) -> &PluginBannerView {
        &self.banner
    }

    /// Whether the Plugins settings modal is open.
    pub fn plugin_settings_open(&self) -> bool {
        self.plugin_settings.is_some()
    }

    /// The Plugins settings modal, while it is open.
    pub fn plugin_settings_view(&self) -> Option<&PluginSettingsView> {
        self.plugin_settings.as_ref()
    }

    /// Whether the relink panel is open.
    pub fn relink_open(&self) -> bool {
        self.relink.is_some()
    }

    /// Whether the quantize panel is open.
    pub fn quantize_open(&self) -> bool {
        self.quantize.is_some()
    }

    /// The quantize panel, while it is open.
    pub fn quantize_view(&self) -> Option<&QuantizeView> {
        self.quantize.as_ref()
    }

    /// The quantize panel's settings (those of the open panel, else the last ones).
    pub fn quantize_values(&self) -> &QuantizeValues {
        self.quantize
            .as_ref()
            .map_or(&self.quantize_values, QuantizeView::values)
    }

    /// Whether the strip silence panel is open.
    pub fn silence_open(&self) -> bool {
        self.silence.is_some()
    }

    /// The strip silence panel, while it is open.
    pub fn silence_view(&self) -> Option<&SilenceView> {
        self.silence.as_ref()
    }

    /// The strip silence panel's settings (those of the open panel, else the last ones).
    pub fn silence_values(&self) -> &SilenceValues {
        self.silence
            .as_ref()
            .map_or(&self.silence_values, SilenceView::values)
    }

    /// Whether the Audio to Notes panel is open.
    pub fn notes_open(&self) -> bool {
        self.notes.is_some()
    }

    /// The Audio to Notes panel, while it is open.
    pub fn notes_view(&self) -> Option<&NotesView> {
        self.notes.as_ref()
    }

    /// The Audio to Notes panel's settings (those of the open panel, else the last ones).
    pub fn notes_values(&self) -> &NotesValues {
        self.notes
            .as_ref()
            .map_or(&self.notes_values, NotesView::values)
    }

    /// Whether the warp panel is open.
    pub fn warp_open(&self) -> bool {
        self.warp.is_some()
    }

    /// The warp panel, while it is open.
    pub fn warp_view(&self) -> Option<&WarpView> {
        self.warp.as_ref()
    }

    /// The relink panel, while it is open.
    pub fn relink_view(&self) -> Option<&RelinkView> {
        self.relink.as_ref()
    }

    /// Relinks the asset LOCATE... was pressed for to the picked audio file.
    pub fn relink_file(&mut self, path: PathBuf) {
        if let Some(hash) = self.relink_target.take() {
            self.core.relink_asset(&hash, path);
        }
        self.repaint = true;
    }

    /// Searches the picked folder for the missing audio files (see
    /// [`AppCore::relink_search_folder`]).
    pub fn relink_search_folder(&mut self, folder: PathBuf) {
        self.core.relink_search_folder(folder);
        self.repaint = true;
    }

    /// Opens the relink panel, or says there is nothing to relink.
    fn open_relink(&mut self) {
        if self.core.missing_assets().is_empty() {
            self.notify("no missing audio files".to_owned());
            return;
        }
        let theme = self.core.view().theme().clone();
        let mut view = RelinkView::new(theme);
        view.set_scale(self.scale());
        view.set_bounds(self.plugin_modal_rect());
        self.relink = Some(view);
        self.sync_relink();
    }

    /// Runs a Track menu command in the arrangement. Typing a folder name or a comment moves the
    /// focus there, so the keys reach it.
    fn track_command(&mut self, command: AppCommand) {
        let Some((id, arg)) = command.command_id() else {
            return;
        };
        if let Some(actions) = self.core.pane_command(Pane::Arrangement, id, arg) {
            self.pane_actions(&actions);
        }
        if self.core.arrangement().is_text_owner() {
            self.set_focus(Pane::Arrangement);
        }
        self.repaint = true;
    }

    /// Runs an audio clip command of the Edit and Audio menus in the arrangement (the
    /// commands that need no samples), whatever pane has the focus.
    fn audio_command(&mut self, command: AppCommand) {
        let Some((id, arg)) = command.command_id() else {
            return;
        };
        match self.core.pane_command(Pane::Arrangement, id, arg) {
            Some(actions) => self.pane_actions(&actions),
            None => self.core.report("Select an audio clip first"),
        }
        self.repaint = true;
    }

    /// Opens the strip silence panel with the settings of the last time.
    fn open_silence(&mut self) {
        let theme = self.core.view().theme().clone();
        let mut view = SilenceView::new(theme, self.silence_values.clone());
        view.set_scale(self.scale());
        view.set_bounds(self.plugin_modal_rect());
        self.silence = Some(view);
        self.sync_silence();
    }

    /// Closes the strip silence panel, keeping its settings for the next time.
    fn close_silence(&mut self) {
        if let Some(view) = self.silence.take() {
            self.silence_values = view.values().clone();
            self.repaint = true;
        }
    }

    /// Feeds the strip silence panel what it applies to.
    fn sync_silence(&mut self) {
        if self.silence.is_none() {
            return;
        }
        let (text, can_apply) = self.core.strip_silence_target();
        if let Some(view) = &mut self.silence {
            view.set_target(text, can_apply);
            if view.needs_repaint() {
                self.repaint = true;
            }
        }
    }

    /// What the strip silence panel asked for.
    fn silence_action(&mut self, action: SilenceAction) {
        match action {
            SilenceAction::Close => self.close_silence(),
            SilenceAction::Apply => {
                let values = self.silence_values().clone();
                self.core.strip_silence(&values);
            }
        }
        self.sync_silence();
        self.repaint = true;
    }

    /// Opens the Audio to Notes panel with the settings of the last time.
    fn open_notes(&mut self) {
        let theme = self.core.view().theme().clone();
        let mut view = NotesView::new(theme, self.notes_values.clone());
        view.set_scale(self.scale());
        view.set_bounds(self.plugin_modal_rect());
        self.notes = Some(view);
        self.sync_notes();
    }

    /// Closes the Audio to Notes panel, keeping its settings for the next time.
    fn close_notes(&mut self) {
        if let Some(view) = self.notes.take() {
            self.notes_values = view.values().clone();
            self.repaint = true;
        }
    }

    /// Feeds the Audio to Notes panel what it applies to.
    fn sync_notes(&mut self) {
        if self.notes.is_none() {
            return;
        }
        let (text, can_apply) = self.core.audio_notes_target();
        if let Some(view) = &mut self.notes {
            view.set_target(text, can_apply);
            if view.needs_repaint() {
                self.repaint = true;
            }
        }
    }

    /// What the Audio to Notes panel asked for: EXTRACT starts the worker and closes the panel
    /// (the notes arrive when it is done); a refusal leaves the panel open.
    fn notes_action(&mut self, action: NotesAction) {
        match action {
            NotesAction::Close => self.close_notes(),
            NotesAction::Apply => {
                let values = self.notes_values().clone();
                if self.core.audio_to_notes(&values) {
                    self.close_notes();
                }
            }
        }
        self.sync_notes();
        self.repaint = true;
    }

    /// Runs Audio to Notes with `values` on the selected audio clip, as the panel's EXTRACT does
    /// (headless sessions and scripts). False, with a message, when nothing started.
    pub fn audio_to_notes(&mut self, values: &NotesValues) -> bool {
        self.notes_values = values.clone();
        let started = self.core.audio_to_notes(values);
        self.repaint = true;
        started
    }

    /// Opens the warp panel with the settings of the first selected audio clip.
    fn open_warp(&mut self) {
        let theme = self.core.view().theme().clone();
        let values = self
            .core
            .warp_values()
            .unwrap_or_else(|| self.warp_values.clone());
        let mut view = WarpView::new(theme, values);
        view.set_scale(self.scale());
        view.set_bounds(self.plugin_modal_rect());
        self.warp = Some(view);
        self.sync_warp();
    }

    /// Closes the warp panel, keeping its values for the next time.
    fn close_warp(&mut self) {
        if let Some(view) = self.warp.take() {
            self.warp_values = view.values().clone();
            self.repaint = true;
        }
    }

    /// Feeds the warp panel what it applies to.
    fn sync_warp(&mut self) {
        if self.warp.is_none() {
            return;
        }
        let (text, can_apply) = self.core.warp_target();
        if let Some(view) = &mut self.warp {
            view.set_target(text, can_apply);
            if view.needs_repaint() {
                self.repaint = true;
            }
        }
    }

    /// What the warp panel asked for.
    fn warp_action(&mut self, action: WarpAction) {
        match action {
            WarpAction::Close => self.close_warp(),
            WarpAction::Apply => {
                if let Some(values) = self.warp.as_ref().map(|v| v.values().clone()) {
                    self.core.apply_warp(&values);
                }
            }
            WarpAction::Detect => {
                if let Some(tenths) = self.core.detect_warp_tempo()
                    && let Some(view) = &mut self.warp
                {
                    let mut values = view.values().clone();
                    values.bpm_tenths = tenths;
                    view.set_values(values);
                }
            }
        }
        self.sync_warp();
        self.repaint = true;
    }

    /// Quantize Audio with the grid and strength of the quantize options.
    fn quantize_audio(&mut self) {
        let params = self.quantize_values().params();
        self.core.quantize_audio_clips(params.grid, params.strength);
        self.repaint = true;
    }

    /// Opens the quantize panel with the settings of the last time.
    fn open_quantize(&mut self) {
        let theme = self.core.view().theme().clone();
        let mut view = QuantizeView::new(theme, self.quantize_values.clone());
        view.set_scale(self.scale());
        view.set_bounds(self.plugin_modal_rect());
        self.quantize = Some(view);
        self.sync_quantize();
    }

    /// Closes the quantize panel, keeping its settings for the next time.
    fn close_quantize(&mut self) {
        if let Some(view) = self.quantize.take() {
            self.quantize_values = view.values().clone();
            self.repaint = true;
        }
    }

    /// Feeds the quantize panel what it applies to and the grooves it can pick.
    fn sync_quantize(&mut self) {
        if self.quantize.is_none() {
            return;
        }
        let pane = self.live_pane(self.focus);
        let target = self.core.quantize_target(pane);
        let can_extract = self.core.groove_source(pane).is_some();
        let grooves = self.core.groove_entries();
        if let Some(view) = &mut self.quantize {
            view.set_grooves(grooves);
            view.set_target(
                target.text(),
                target != crate::app_core::QuantizeTarget::None,
                can_extract,
            );
            if view.needs_repaint() {
                self.repaint = true;
            }
        }
    }

    /// Quantizes what `pane` works on with the panel's settings of the last time (the open
    /// panel's, while it is open), as the quantize key and the piano roll's QUANT button do.
    fn apply_last_quantize(&mut self, pane: Pane) {
        let params = self.quantize_values().params();
        self.core.apply_quantize(pane, params);
        self.sync_quantize();
        self.repaint = true;
    }

    /// What the quantize panel asked for.
    fn quantize_action(&mut self, action: QuantizeAction) {
        let pane = self.live_pane(self.focus);
        let Some(view) = &self.quantize else {
            return;
        };
        let (params, values) = (view.params(), view.values().clone());
        match action {
            QuantizeAction::Close => self.close_quantize(),
            QuantizeAction::Apply => self.core.apply_quantize(pane, params),
            // Only the piano roll and the quantize key send these (see `pane_actions`).
            QuantizeAction::ApplyLast | QuantizeAction::Options => {}
            QuantizeAction::Extract => {
                if let Some(name) =
                    self.core
                        .extract_groove(pane, values.extract_step, values.extract_slots)
                {
                    // The new groove is the one to try.
                    self.sync_quantize();
                    if let Some(view) = &mut self.quantize {
                        view.select_groove(&name);
                    }
                }
            }
            QuantizeAction::Delete(name) => self.core.delete_groove(&name),
        }
        self.sync_quantize();
        self.repaint = true;
    }

    /// Feeds the relink panel the missing list; it closes when nothing is missing any more.
    fn sync_relink(&mut self) {
        let entries: Vec<RelinkEntry> = self
            .core
            .missing_assets()
            .iter()
            .map(|m| RelinkEntry {
                hash: m.hash.clone(),
                name: m.name.clone(),
            })
            .collect();
        if entries.is_empty() {
            if self.relink.take().is_some() {
                self.repaint = true;
            }
        } else if let Some(view) = &mut self.relink {
            view.set_missing(entries);
            if view.needs_repaint() {
                self.repaint = true;
            }
        }
    }

    /// Whether the MIDI Mappings panel is open.
    pub fn midi_maps_open(&self) -> bool {
        self.midi_maps.is_some()
    }

    /// The MIDI Mappings panel, while it is open.
    pub fn midi_maps_view(&self) -> Option<&MidiMapsView> {
        self.midi_maps.as_ref()
    }

    /// Opens the MIDI Mappings panel.
    fn open_midi_maps(&mut self) {
        let theme = self.core.view().theme().clone();
        let mut view = MidiMapsView::new(theme);
        view.set_scale(self.scale());
        view.set_bounds(self.plugin_modal_rect());
        view.set_rows(self.core.midi_map_rows());
        self.midi_maps = Some(view);
        self.repaint = true;
    }

    /// Feeds the MIDI Mappings panel the project's rows.
    fn sync_midi_maps(&mut self) {
        let rows = self.core.midi_map_rows();
        if let Some(view) = &mut self.midi_maps {
            view.set_rows(rows);
            if view.needs_repaint() {
                self.repaint = true;
            }
        }
    }

    /// What the MIDI Mappings panel asked for.
    fn midi_maps_action(&mut self, action: MidiMapsAction) -> Vec<ShellRequest> {
        let mut requests = Vec::new();
        match action {
            MidiMapsAction::Edit(mapping) => {
                self.core.edit_midi_map(mapping);
            }
            MidiMapsAction::Remove(id) => {
                self.core.remove_midi_map(id);
            }
            MidiMapsAction::Relearn(id) => {
                // The panel gives way to the control being moved.
                if self.core.relearn_midi_map(id) {
                    self.midi_maps = None;
                }
            }
            MidiMapsAction::Export => requests.push(ShellRequest::PickExportMidiPreset),
            MidiMapsAction::Import => requests.push(ShellRequest::PickImportMidiPreset),
            MidiMapsAction::Clear => {
                self.core.clear_midi_maps();
            }
            MidiMapsAction::Close => self.midi_maps = None,
        }
        self.sync_midi_maps();
        self.repaint = true;
        requests
    }

    /// Whether the Macros panel is open.
    pub fn macros_open(&self) -> bool {
        self.macros_panel.is_some()
    }

    /// The Macros panel, while it is open.
    pub fn macros_view(&self) -> Option<&MacrosView> {
        self.macros_panel.as_ref()
    }

    /// Opens the Macros panel.
    fn open_macros(&mut self) {
        let theme = self.core.view().theme().clone();
        let mut view = MacrosView::new(theme);
        view.set_scale(self.scale());
        view.set_bounds(self.plugin_modal_rect());
        view.set_banks(self.core.macro_bank_rows(), self.core.macro_bank().as_ref());
        self.macros_panel = Some(view);
        self.repaint = true;
    }

    /// Feeds the Macros panel the project's banks.
    fn sync_macros(&mut self) {
        if self.macros_panel.is_none() {
            return;
        }
        let banks = self.core.macro_bank_rows();
        let current = self.core.macro_bank();
        if let Some(view) = &mut self.macros_panel {
            view.set_banks(banks, current.as_ref());
            if view.needs_repaint() {
                self.repaint = true;
            }
        }
    }

    /// What the Macros panel asked for.
    fn macros_action(&mut self, action: MacrosAction) {
        if self.core.macros_action(action) {
            self.macros_panel = None;
        }
        self.sync_macros();
        self.repaint = true;
    }

    /// Whether the parameter search is open.
    pub fn param_search_open(&self) -> bool {
        self.param_search.is_some()
    }

    /// The parameter search, while it is open.
    pub fn param_search_view(&self) -> Option<&ParamSearchView> {
        self.param_search.as_ref()
    }

    /// Opens the parameter search on an empty query.
    fn open_param_search(&mut self) {
        let theme = self.core.view().theme().clone();
        let mut view = ParamSearchView::new(theme);
        view.set_scale(self.scale());
        view.set_bounds(self.plugin_modal_rect());
        let (rows, total) = self.core.param_search("");
        view.set_rows(rows, total);
        self.param_search = Some(view);
        self.repaint = true;
    }

    /// Keeps the values of the rows the search shows current (cheap: no new search).
    fn sync_param_search(&mut self) {
        if self.param_search.is_none() {
            return;
        }
        let (rows, total) = self.core.param_search_refresh();
        if let Some(view) = &mut self.param_search {
            view.set_rows(rows, total);
            if view.needs_repaint() {
                self.repaint = true;
            }
        }
    }

    /// What the parameter search asked for.
    fn param_search_action(&mut self, action: ParamSearchAction) {
        match action {
            ParamSearchAction::Query(q) => {
                let (rows, total) = self.core.param_search(&q);
                if let Some(view) = &mut self.param_search {
                    view.set_rows(rows, total);
                }
            }
            ParamSearchAction::Reveal(row) => {
                self.param_search = None;
                match self.core.search_reveal(row) {
                    Some(Revealed::Console) => {
                        if !self.console_visible() {
                            self.execute_command(AppCommand::ToggleConsole);
                        } else if self.console_detached() {
                            self.pending
                                .push(ShellRequest::FocusWindow(WindowKind::Console));
                        }
                        self.set_focus(Pane::Console);
                    }
                    Some(Revealed::Rack) => {
                        if self.rack_detached() {
                            self.pending
                                .push(ShellRequest::FocusWindow(WindowKind::Rack));
                        } else if !self.workflow.rack_visible {
                            self.set_rack_visible(true);
                        }
                        self.set_focus(Pane::Rack);
                    }
                    None => {}
                }
            }
            ParamSearchAction::Lane(row) => {
                if self.core.search_show_lane(row) {
                    self.param_search = None;
                    self.set_focus(Pane::Arrangement);
                }
            }
            ParamSearchAction::Assign(row) => {
                self.core.search_assign(row);
            }
            ParamSearchAction::Learn(row) => {
                if self.core.search_learn(row) {
                    self.param_search = None;
                }
            }
            ParamSearchAction::Adjust { row, steps, fine } => {
                self.core.search_adjust(row, steps, fine);
            }
            ParamSearchAction::Close => self.param_search = None,
        }
        self.sync_param_search();
        self.repaint = true;
    }

    /// The folder MIDI mapping presets are kept in (`midi-presets` next to the settings file),
    /// created on demand; `None` without a settings file.
    pub fn midi_preset_dir(&self) -> Option<PathBuf> {
        let dir = self
            .settings_path
            .as_deref()?
            .parent()?
            .join("midi-presets");
        let _ = std::fs::create_dir_all(&dir);
        Some(dir)
    }

    /// Writes the project's MIDI mappings to the picked preset file.
    pub fn export_midi_preset_to(&mut self, path: &Path) {
        self.core.export_midi_preset(path, &[]);
        self.repaint = true;
    }

    /// Adds the mappings of the picked preset file to the project.
    pub fn import_midi_preset_from(&mut self, path: &Path) {
        self.core.import_midi_preset(path);
        self.sync_midi_maps();
        self.repaint = true;
    }

    /// What the relink panel asked for.
    fn relink_action(&mut self, action: RelinkAction) -> Vec<ShellRequest> {
        match action {
            RelinkAction::Locate { hash } => {
                self.relink_target = Some(hash);
                vec![ShellRequest::PickRelinkFile]
            }
            RelinkAction::SearchFolder => vec![ShellRequest::PickRelinkFolder],
            RelinkAction::Close => {
                self.relink = None;
                Vec::new()
            }
        }
    }

    /// Requests raised between events (a folder dialog, for instance): the platform loop takes
    /// them every tick.
    pub fn take_requests(&mut self) -> Vec<ShellRequest> {
        std::mem::take(&mut self.pending)
    }

    /// Adds the folder picked in the folder dialog to the plugin search paths.
    pub fn add_plugin_folder(&mut self, folder: PathBuf) {
        self.core.add_plugin_folder(folder);
        self.sync_plugin_ui();
        self.repaint = true;
    }

    /// The editor window requests for the platform layer (see
    /// [`crate::app_core::PluginWindowRequest`]).
    pub fn take_plugin_window_requests(&mut self) -> Vec<crate::app_core::PluginWindowRequest> {
        self.core.take_plugin_window_requests()
    }

    /// Opens the plugin parameter pane for `device` in place of the arrangement.
    pub fn open_plugin_params(&mut self, device: &DeviceKey) {
        self.core.open_plugin_params(device);
        self.show_plugin_params();
    }

    /// The core, mutably.
    pub fn core_mut(&mut self) -> &mut AppCore<LiveSession<B>> {
        &mut self.core
    }

    /// Whether the settings dialog is open.
    pub fn dialog_open(&self) -> bool {
        self.dialog.is_some()
    }

    /// The status bar text.
    pub fn status_text(&self) -> &str {
        &self.status
    }

    /// The window title.
    pub fn title(&self) -> String {
        self.core.title()
    }

    /// The window state (split, song area shown, input quantize).
    pub fn ui_state(&self) -> UiState {
        self.ui.clone()
    }

    /// The pane that receives keys (the last one clicked).
    pub fn focus(&self) -> Pane {
        self.focus
    }

    /// The active keymap (its OS decides the shortcut texts).
    pub fn keymap(&self) -> &Keymap {
        self.router.keymap()
    }

    /// Switches the keymap (a profile change or a user remap).
    pub fn set_keymap(&mut self, keymap: Keymap) {
        self.router.set_keymap(keymap);
        self.repaint = true;
    }

    /// The settings dialog, while it is open.
    pub fn dialog(&self) -> Option<&SettingsDialog> {
        self.dialog.as_ref()
    }

    /// The active keymap profile: the user's choice in `keymap.json`, else the app default.
    pub fn profile(&self) -> Profile {
        self.user_keymap.profile.unwrap_or(self.default_profile)
    }

    /// The user's changes over the profile, as loaded or last applied.
    pub fn user_keymap(&self) -> &UserKeymap {
        &self.user_keymap
    }

    /// The modifier roles with the user's overrides.
    pub fn roles(&self) -> &Roles {
        &self.roles
    }

    /// The workflow preferences (`workflow.json`).
    pub fn workflow(&self) -> &WorkflowPrefs {
        &self.workflow
    }

    /// The library folder in use: the run's override, else the one chosen in the Workflow tab,
    /// else the default; `None` when none of them is known.
    pub fn library_root(&self) -> Option<PathBuf> {
        self.library_override
            .clone()
            .or_else(|| self.workflow.library_folder.clone())
            .or_else(|| self.library_default.clone())
    }

    /// Sets the folder used when the user has not chosen one (the platform passes
    /// [`crate::settings::default_library_dir`]).
    pub fn set_library_default(&mut self, folder: Option<PathBuf>) {
        self.library_default = folder;
        self.sync_library_root();
    }

    /// Hands the library folder in use to the core and the browser panel.
    fn sync_library_root(&mut self) {
        let root = self.library_root();
        self.core.set_library_root(root);
    }

    /// The library folder picked in the folder dialog: into the open settings dialog's draft
    /// (applied with its Apply), else applied and saved at once.
    pub fn set_library_folder(&mut self, folder: PathBuf) {
        if let Some(dialog) = &mut self.dialog {
            dialog.set_library_folder(Some(folder));
        } else {
            self.workflow.library_folder = Some(folder);
            self.save_workflow();
            self.sync_library_root();
        }
        self.repaint = true;
    }

    /// The OS whose keys and texts apply.
    pub fn os(&self) -> Os {
        self.os
    }

    /// Makes `user` the user keymap: rebuilds the keymap and the roles, swaps them into the
    /// running router (hot reload) and, when `save`, writes `keymap.json`. Conflicts and
    /// unusable chords are reported to the user; the keymap still works.
    fn set_user_keymap(&mut self, user: UserKeymap, save: bool) {
        let built = user.build(user.profile.unwrap_or(self.default_profile), self.os);
        self.router.set_keymap(built.keymap);
        self.roles = built.roles;
        self.core.set_roles(&self.roles);
        self.repaint = true;
        if save && let Some(path) = &self.keymap_path {
            match keymap_store::save_user_keymap(path, &user) {
                Ok(()) => self.keymap_stamp = stamp_of(path),
                Err(e) => {
                    let message = format!("keyboard shortcuts not saved: {e}");
                    self.notify(message);
                }
            }
        }
        self.user_keymap = user;
        if let Some(summary) = built.report.summary() {
            self.notify(summary);
        }
    }

    /// Applies the drafts of the settings dialog: the keymap (saved and swapped in at once)
    /// and the workflow preferences (saved), each only when it changed.
    fn apply_preferences(&mut self, prefs: Option<(UserKeymap, WorkflowPrefs)>) {
        let Some((user, workflow)) = prefs else {
            return;
        };
        if user != self.user_keymap {
            self.set_user_keymap(user, true);
        }
        if workflow != self.workflow {
            self.workflow = workflow;
            self.save_workflow();
            self.apply_workflow_to_views();
            self.sync_library_root();
        }
    }

    /// Saves `workflow.json` (when there is a settings folder); a failure is shown.
    fn save_workflow(&mut self) {
        if let Some(path) = &self.workflow_path
            && let Err(e) = keymap_store::save_workflow(path, &self.workflow)
        {
            let message = format!("workflow preferences not saved: {e}");
            self.notify(message);
        }
    }

    /// Hands the workflow preferences to the views: the follow start state, whether a create
    /// browser stays open, and the rack flag (through the layout).
    fn apply_workflow_to_views(&mut self) {
        self.core.set_follow(self.workflow.follow_song);
        self.core
            .set_follow_selection(self.workflow.follow_selection);
        self.core
            .view_mut()
            .set_create_keeps_browser_open(self.workflow.create_keeps_browser_open);
        self.core
            .view_mut()
            .set_auto_route(self.workflow.auto_route);
        self.core
            .view_mut()
            .set_button_tips(self.workflow.show_tooltips);
        if !self.workflow.show_tooltips {
            self.clear_tooltip();
        }
        self.layout_view();
    }

    /// The one-time notice of the keys that moved with the standard keymap: the new chords as this
    /// OS shows them, and where the old keys come back.
    fn keys_notice(&self) -> String {
        let keymap = self.router.keymap();
        let key = |id: &str| {
            keymap
                .shortcut_text(id, None)
                .unwrap_or_else(|| "(unbound)".to_string())
        };
        format!(
            "Keys moved: Record {}, Quantize {}, Split {}, Create {} / {}, Import {}; R is the \
             razor. Settings > Keyboard > Atomik legacy restores the old keys.",
            key("transport.record"),
            key("editor.quantize"),
            key("sequencer.split_at_playhead"),
            key("create.instrument"),
            key("create.effect"),
            key("file.import_audio"),
        )
    }

    /// Re-reads `keymap.json` when it changed on disk since it was last read or written (an
    /// edit by hand or by another run): the keymap follows without a restart. A deleted file
    /// is the defaults. Returns whether it reloaded.
    pub fn reload_keymap_file(&mut self) -> bool {
        let Some(path) = self.keymap_path.clone() else {
            return false;
        };
        let stamp = stamp_of(&path);
        if stamp == self.keymap_stamp {
            return false;
        }
        let loaded = keymap_store::load_user_keymap(&path);
        // A corrupt file was moved aside by the load.
        self.keymap_stamp = stamp_of(&path);
        if let Some(status) = loaded.status {
            self.notify(status);
        }
        self.set_user_keymap(loaded.value, false);
        true
    }

    /// Uses `path` as the user keymap file instead of the one next to `settings.json`
    /// (`--keymap`): loads it now (a missing file is the defaults) and saves to it from then on.
    pub fn use_keymap_file(&mut self, path: PathBuf) {
        let loaded = keymap_store::load_user_keymap(&path);
        self.keymap_stamp = stamp_of(&path);
        self.keymap_path = Some(path);
        if let Some(status) = loaded.status {
            self.notify(status);
        }
        self.set_user_keymap(loaded.value, false);
    }

    /// Writes the keymap to `path` (the Export button): the open dialog's draft, else the
    /// applied keymap. Errors are shown in the view.
    pub fn export_keymap_to(&mut self, path: &Path) {
        let user = self
            .dialog
            .as_ref()
            .and_then(SettingsDialog::preferences)
            .map_or_else(|| self.user_keymap.clone(), |(u, _)| u);
        if let Err(e) = keymap_store::write_atomic(path, &user.to_json()) {
            self.notify(format!("keymap not exported: {e}"));
        }
        self.repaint = true;
    }

    /// Reads a keymap from `path` (the Import button): into the open dialog's draft, else
    /// applied and saved at once. A file that is not a keymap is refused with a message.
    pub fn import_keymap_from(&mut self, path: &Path) {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                self.notify(format!("{}: {e}", path.display()));
                return;
            }
        };
        match UserKeymap::from_json(&text) {
            Err(e) => self.notify(format!("{}: {e}", path.display())),
            Ok((user, notes)) => {
                if !notes.is_empty() {
                    self.notify(format!("{}: {}", path.display(), notes.join("; ")));
                }
                if let Some(dialog) = &mut self.dialog {
                    dialog.replace_user(user);
                } else {
                    self.set_user_keymap(user, true);
                }
            }
        }
        self.repaint = true;
    }

    /// The menus, with shortcut texts derived from the active keymap.
    pub fn menu_model(&self) -> Vec<Menu> {
        menus(&self.menu_state(), self.router.keymap())
    }

    /// The modifiers last reported by the window.
    pub fn modifiers(&self) -> Modifiers {
        self.modifiers
    }

    /// The window lost focus: the key releases will never arrive, so forget the held keys and
    /// the modifier state (a hold-style role must not stay on).
    pub fn focus_lost(&mut self) {
        self.router.clear_held();
        self.clear_tooltip();
        self.modifiers = Modifiers::NONE;
        self.forward_modifiers();
    }

    /// Tells the time views which modifiers are held (the alternate-tool role is a hold).
    fn forward_modifiers(&mut self) {
        let event = UiEvent::ModifiersChanged {
            modifiers: self.modifiers,
        };
        self.core.handle_pane_event(Pane::Arrangement, &event);
        self.core.handle_pane_event(Pane::PianoRoll, &event);
    }

    /// The contexts the router resolves in, from the focused pane and the text state:
    /// the rack is `Rack` unless one of its controls has focus (then only
    /// `Global`, the control takes its own keys), `Text` on top while its browser filter or a
    /// rename has focus; the arrangement is `Sequencer`; the piano roll is `Editor` over
    /// `Sequencer` (it shares the time axis and the transport keys); the other editors are
    /// `Editor`; the console is `Mixer`.
    pub fn context_stack(&self) -> ContextStack {
        match self.live_pane(self.focus) {
            Pane::Rack => {
                let view = self.core.view();
                if view.is_text_owner() {
                    ContextStack::new(&[Context::Text, Context::Browser, Context::Rack])
                } else if view.focused_control().is_some() {
                    ContextStack::global()
                } else {
                    ContextStack::new(&[Context::Rack])
                }
            }
            Pane::Arrangement if self.core.arrangement().is_text_owner() => {
                ContextStack::new(&[Context::Text, Context::Sequencer])
            }
            Pane::Arrangement => ContextStack::new(&[Context::Sequencer]),
            Pane::PianoRoll => ContextStack::new(&[Context::Editor, Context::Sequencer]),
            Pane::SeqEditor if self.core.seq_editor().is_text_owner() => {
                ContextStack::new(&[Context::Text, Context::Editor])
            }
            Pane::ContainerEditor if self.core.container_editor().is_text_owner() => {
                ContextStack::new(&[Context::Text, Context::Editor])
            }
            Pane::SampleEditor
            | Pane::StepEditor
            | Pane::SliceEditor
            | Pane::SeqEditor
            | Pane::ContainerEditor => ContextStack::new(&[Context::Editor]),
            Pane::Console if self.core.console().is_text_owner() => {
                ContextStack::new(&[Context::Text, Context::Mixer])
            }
            Pane::Console => ContextStack::new(&[Context::Mixer]),
            Pane::PluginParams if self.core.plugin_params().is_text_owner() => {
                ContextStack::new(&[Context::Text])
            }
            Pane::FileBrowser if self.core.file_browser().is_text_owner() => {
                ContextStack::new(&[Context::Text, Context::Files])
            }
            Pane::FileBrowser => ContextStack::new(&[Context::Files]),
            Pane::Transport | Pane::PluginParams => ContextStack::global(),
        }
    }

    /// What the menus show.
    pub fn menu_state(&self) -> MenuState {
        let view = self.core.view();
        let song = &self.core.doc().song;
        MenuState {
            flip: view.flip(),
            cables: view.cable_visibility(),
            browser: view.browser_visible(),
            file_browser: self.ui.file_browser_visible,
            can_undo: self.core.document().can_undo(),
            can_redo: self.core.document().can_redo(),
            can_edit: self.core.has_selection(self.live_pane(self.focus)),
            can_paste: self.core.can_paste(self.live_pane(self.focus)),
            song_visible: self.ui.show_song,
            piano_roll: self.core.piano_roll_clip().is_some(),
            sample_editor: self.core.sample_editor_device().is_some(),
            step_editor: self.core.step_editor_device().is_some(),
            slice_editor: self.core.slice_editor_device().is_some(),
            playing: self.core.is_playing(),
            recording: self.core.is_recording(),
            loop_enabled: song.loop_enabled,
            metronome: self.core.metronome(),
            quantize: self.core.input_quantize(),
            console: self.console_visible(),
            console_maximised: self.maximised(),
            compensation: self.core.doc().console.compensation,
            console_devices: view.show_console_devices(),
            console_links: view.show_console_links(),
            tempo_lane: self.core.arrangement().tempo_lane_visible(),
            marker_strip: self.core.arrangement().marker_strip_visible(),
            marker_list: self.core.arrangement().marker_list_visible(),
            rack_navigator: view.navigator_visible(),
            rack: self.workflow.rack_visible,
            rack_detached: self.rack_detached(),
            console_detached: self.console_detached(),
            sequencer_only: self.in_sequencer_only(),
            editor: !self.lower_is_console()
                && !matches!(self.lower_pane(), Pane::Arrangement | Pane::Console),
            missing_audio: !self.core.missing_assets().is_empty(),
            midi_clip: self.core.can_export_midi(MidiExport::Clip),
            midi_track: self.core.can_export_midi(MidiExport::Track),
            midi_song: self.core.can_export_midi(MidiExport::Song),
            export_song: self.core.can_export_audio(ExportScope::Song),
            export_loop: self.core.can_export_audio(ExportScope::Loop),
            org: self.core.arrangement().org_menu(),
            lane: self.core.arrangement().lane_menu(),
            audio: self.core.arrangement().audio_menu(),
            vca: self.core.vca_menu(self.live_pane(self.focus)),
            strip: self.core.strip_menu(self.live_pane(self.focus)),
            snapshots: self.core.snapshot_menu(),
            midi: self.core.midi_menu(),
            freeze: self.core.freeze_menu(),
            follow_selection: self.core.follow_selection(),
        }
    }

    /// The console is maximised over the rack in the main window.
    fn maximised(&self) -> bool {
        self.console_in_main() && self.ui.console_maximised
    }

    fn bar_height(&self) -> f64 {
        if self.menubar.is_some() {
            MENU_BAR_HEIGHT
        } else {
            0.0
        }
    }

    /// The top of everything below the menu bar and the plugin banner.
    fn top(&self) -> f64 {
        self.bar_height() + self.banner.height()
    }

    /// The Plugins settings modal's rectangle: most of the window, clear of the menu bar and the
    /// status bar.
    fn plugin_modal_rect(&self) -> Rect {
        let margin = 24.0;
        let top = self.bar_height() + margin;
        let bottom = (self.size().height - STATUS_BAR_HEIGHT - margin).max(top + 1.0);
        Rect::new(
            margin,
            top,
            (self.size().width - margin).max(margin + 1.0),
            bottom,
        )
    }

    /// The file browser column, when it shows: the full height below the menu bar. Its width is
    /// the panel's own (it resizes by drag), kept so the rest of the window keeps
    /// [`MIN_REST_WIDTH`].
    fn file_browser_rect(&self) -> Option<Rect> {
        if !self.ui.file_browser_visible {
            return None;
        }
        let top = self.bar_height();
        let bottom = (self.size().height - STATUS_BAR_HEIGHT).max(top + 1.0);
        let most = (self.size().width - MIN_REST_WIDTH).max(0.0);
        let least = FileBrowserView::MIN_WIDTH.min(most);
        let width = self.core.file_browser().width().clamp(least, most);
        Some(Rect::new(0.0, top, width, bottom))
    }

    /// Where the rack, divider, transport bar and song area are (module docs).
    pub fn layout(&self) -> Layout {
        let fb = self.file_browser_rect();
        let x0 = fb.map_or(0.0, |r| r.x1);
        let w = (self.size().width - x0).max(1.0);
        let x1 = x0 + w;
        let top = self.top();
        let bottom = (self.size().height - STATUS_BAR_HEIGHT).max(top + 1.0);
        if self.maximised() {
            let transport_top = (bottom - TRANSPORT_HEIGHT).max(top + 1.0);
            return Layout {
                rack: Rect::new(x0, top, x1, top),
                divider: None,
                transport: Rect::new(x0, transport_top, x1, bottom),
                song: Some(Rect::new(x0, top, x1, transport_top)),
                file_browser: fb,
            };
        }
        if !self.ui.show_song {
            let transport_top = (bottom - TRANSPORT_HEIGHT).max(top + 1.0);
            return Layout {
                rack: Rect::new(x0, top, x1, transport_top),
                divider: None,
                transport: Rect::new(x0, transport_top, x1, bottom),
                song: None,
                file_browser: fb,
            };
        }
        if !self.rack_in_main() {
            let transport = Rect::new(x0, top, x1, top + TRANSPORT_HEIGHT);
            return Layout {
                rack: Rect::new(x0, top, x1, top),
                divider: None,
                transport,
                song: Some(Rect::new(
                    x0,
                    transport.y1,
                    x1,
                    bottom.max(transport.y1 + 1.0),
                )),
                file_browser: fb,
            };
        }
        let avail = (bottom - top - TRANSPORT_HEIGHT - DIVIDER_HEIGHT).max(2.0);
        let lo = MIN_PANE_HEIGHT.min(avail / 2.0);
        let rack_h = (avail * self.ui.rack_split).clamp(lo, avail - lo);
        let divider = Rect::new(x0, top + rack_h, x1, top + rack_h + DIVIDER_HEIGHT);
        let transport = Rect::new(x0, divider.y1, x1, divider.y1 + TRANSPORT_HEIGHT);
        Layout {
            rack: Rect::new(x0, top, x1, divider.y0),
            divider: Some(divider),
            transport,
            song: Some(Rect::new(
                x0,
                transport.y1,
                x1,
                bottom.max(transport.y1 + 1.0),
            )),
            file_browser: fb,
        }
    }

    /// Logical size of the rack view.
    pub fn view_size(&self) -> Size {
        self.layout().rack.size()
    }

    /// Physical size of the whole window content.
    pub fn physical_size(&self) -> (u32, u32) {
        (
            (self.size().width * self.scale()).ceil().max(1.0) as u32,
            (self.size().height * self.scale()).ceil().max(1.0) as u32,
        )
    }

    fn layout_view(&mut self) {
        let l = self.layout();
        // A maximised console hides the rack: it keeps its last size.
        if let Some((_, rack)) = self.pane_rect(Pane::Rack)
            && rack.height() > 0.0
        {
            self.core.handle_event(&UiEvent::Resize(rack.size()));
        }
        self.core.transport_bar_mut().set_bounds(l.transport);
        if let Some(fb) = l.file_browser {
            self.core.file_browser_mut().set_bounds(fb);
        }
        self.browser_edge = l.file_browser.map_or(0.0, |r| r.x1);
        if let Some(song) = l.song {
            self.core.arrangement_mut().set_bounds(song);
            self.core.piano_roll_mut().set_bounds(song);
            self.core.sample_editor_mut().set_bounds(song);
            self.core.step_editor_mut().set_bounds(song);
            self.core.slice_editor_mut().set_bounds(song);
            self.core.seq_editor_mut().set_bounds(song);
            self.core.container_editor_mut().set_bounds(song);
            self.core.plugin_params_mut().set_bounds(song);
            if !self.console_detached() {
                self.core.console_mut().set_bounds(song);
            }
        }
        if let Some((_, rect)) = self.pane_rect(Pane::Console)
            && self.console_detached()
        {
            self.core.console_mut().set_bounds(rect);
        }
        let bar = self.bar_height();
        let banner_x = l.file_browser.map_or(0.0, |r| r.x1);
        self.banner.set_bounds(Rect::new(
            banner_x,
            bar,
            self.size().width.max(banner_x),
            bar + self.banner.height(),
        ));
        if self.plugin_settings.is_some() {
            let rect = self.plugin_modal_rect();
            if let Some(v) = &mut self.plugin_settings {
                v.set_bounds(rect);
            }
        }
        if self.relink.is_some() {
            let rect = self.plugin_modal_rect();
            if let Some(v) = &mut self.relink {
                v.set_bounds(rect);
            }
        }
        if self.export.is_some() {
            let rect = self.plugin_modal_rect();
            if let Some(v) = &mut self.export {
                v.set_bounds(rect);
            }
        }
        if self.midi_maps.is_some() {
            let rect = self.plugin_modal_rect();
            if let Some(v) = &mut self.midi_maps {
                v.set_bounds(rect);
            }
        }
        if self.macros_panel.is_some() {
            let rect = self.plugin_modal_rect();
            if let Some(v) = &mut self.macros_panel {
                v.set_bounds(rect);
            }
        }
        if self.param_search.is_some() {
            let rect = self.plugin_modal_rect();
            if let Some(v) = &mut self.param_search {
                v.set_bounds(rect);
            }
        }
        if self.quantize.is_some() {
            let rect = self.plugin_modal_rect();
            if let Some(v) = &mut self.quantize {
                v.set_bounds(rect);
            }
        }
        if self.silence.is_some() {
            let rect = self.plugin_modal_rect();
            if let Some(v) = &mut self.silence {
                v.set_bounds(rect);
            }
        }
        if self.notes.is_some() {
            let rect = self.plugin_modal_rect();
            if let Some(v) = &mut self.notes {
                v.set_bounds(rect);
            }
        }
        if self.warp.is_some() {
            let rect = self.plugin_modal_rect();
            if let Some(v) = &mut self.warp {
                v.set_bounds(rect);
            }
        }
        // Lights the console's MAX button (every pane change passes here: F5 / Shift+F5,
        // the Mixer menu, the ui-state restore and the CLI flags).
        let maximised = self.maximised();
        self.core.console_mut().set_maximised(maximised);
        self.repaint = true;
    }

    /// The pane shown in the lower pane (when it shows): the console, else the sample editor
    /// while a sampler is open, else the piano roll while a clip is open, else the
    /// arrangement.
    pub fn lower_pane(&self) -> Pane {
        if self.lower_is_console() {
            Pane::Console
        } else if self.core.sample_editor_device().is_some() {
            Pane::SampleEditor
        } else if self.core.step_editor_device().is_some() {
            Pane::StepEditor
        } else if self.core.slice_editor_device().is_some() {
            Pane::SliceEditor
        } else if self.core.seq_editor_device().is_some() {
            Pane::SeqEditor
        } else if self.core.container_editor_device().is_some() {
            Pane::ContainerEditor
        } else if self.core.plugin_params_device().is_some() {
            Pane::PluginParams
        } else if self.core.piano_roll_clip().is_some() {
            Pane::PianoRoll
        } else {
            Pane::Arrangement
        }
    }

    fn song_pane(&self) -> Pane {
        self.lower_pane()
    }

    /// A pane that still exists (the song panes fall back when hidden or switched).
    fn live_pane(&self, pane: Pane) -> Pane {
        match pane {
            // A detached pane lives in its window whatever the main layout is.
            Pane::Rack if self.rack_detached() => Pane::Rack,
            Pane::Console if self.console_detached() => Pane::Console,
            Pane::Arrangement
            | Pane::PianoRoll
            | Pane::Console
            | Pane::SampleEditor
            | Pane::StepEditor
            | Pane::SliceEditor
            | Pane::SeqEditor
            | Pane::ContainerEditor
            | Pane::PluginParams
                if !self.ui.show_song =>
            {
                Pane::Rack
            }
            Pane::Arrangement
            | Pane::PianoRoll
            | Pane::Console
            | Pane::SampleEditor
            | Pane::StepEditor
            | Pane::SliceEditor
            | Pane::SeqEditor
            | Pane::ContainerEditor
            | Pane::PluginParams => self.song_pane(),
            Pane::FileBrowser if !self.ui.file_browser_visible => self.live_pane(Pane::Rack),
            Pane::Rack if self.maximised() => Pane::Console,
            Pane::Rack if !self.rack_in_main() => self.song_pane(),
            other => other,
        }
    }

    fn set_focus(&mut self, pane: Pane) {
        self.focus = pane;
        self.remember_focus(pane);
        self.core
            .arrangement_mut()
            .set_focused(pane == Pane::Arrangement);
        self.core
            .piano_roll_mut()
            .set_focused(pane == Pane::PianoRoll);
        self.core
            .sample_editor_mut()
            .set_focused(pane == Pane::SampleEditor);
        self.core
            .step_editor_mut()
            .set_focused(pane == Pane::StepEditor);
        self.core
            .slice_editor_mut()
            .set_focused(pane == Pane::SliceEditor);
        self.core
            .seq_editor_mut()
            .set_focused(pane == Pane::SeqEditor);
        self.core
            .container_editor_mut()
            .set_focused(pane == Pane::ContainerEditor);
        self.core
            .plugin_params_mut()
            .set_focused(pane == Pane::PluginParams);
        self.core
            .transport_bar_mut()
            .set_focused(pane == Pane::Transport);
        self.core.console_mut().set_focused(pane == Pane::Console);
        self.core
            .file_browser_mut()
            .set_focused(pane == Pane::FileBrowser);
    }

    /// The pane under a window point, if any (the divider and the bars are not panes).
    pub fn hit_pane(&self, pos: Point) -> Option<Pane> {
        self.hit_in(WindowKind::Main, pos).and_then(Result::ok)
    }

    /// What is under a point of `window`: a pane, the divider (`Err(())`), or nothing. A detached
    /// pane fills its window.
    fn hit_in(&self, window: WindowKind, pos: Point) -> Option<Result<Pane, ()>> {
        if window != WindowKind::Main {
            let pane = match window {
                WindowKind::Console => Pane::Console,
                _ => Pane::Rack,
            };
            let (_, rect) = self.pane_rect(pane)?;
            return rect.contains(pos).then_some(Ok(pane));
        }
        let l = self.layout();
        if l.file_browser.is_some_and(|r| r.contains(pos)) {
            Some(Ok(Pane::FileBrowser))
        } else if l.rack.contains(pos) {
            Some(Ok(Pane::Rack))
        } else if l.divider.is_some_and(|d| d.contains(pos)) {
            Some(Err(()))
        } else if l.transport.contains(pos) {
            Some(Ok(Pane::Transport))
        } else if l.song.is_some_and(|s| s.contains(pos)) {
            Some(Ok(self.song_pane()))
        } else {
            None
        }
    }

    /// Handles one event of the main window (logical units). The same as
    /// `handle_in(WindowKind::Main, event)`.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<ShellRequest> {
        self.handle_in(WindowKind::Main, event)
    }

    /// Handles one event of the window `window` (logical units of that window). A window that
    /// is not open ignores it. A pointer event counts as over its own window; a platform that
    /// knows where the pointer is on the desktop during a drag calls [`Self::handle_routed`].
    pub fn handle_in(&mut self, window: WindowKind, event: &UiEvent) -> Vec<ShellRequest> {
        let under = pointer_pos(event).map(|p| WindowHit {
            kind: window,
            logical: (p.x, p.y),
        });
        self.handle_routed(window, event, under)
    }

    /// Like [`Self::handle_in`], for a platform that resolves the window under the pointer: the
    /// OS keeps sending a drag's events to the window that got the press, with positions
    /// outside its area, so `under` says which window (and where in it, logical units) the
    /// pointer is really over, or `None` over no window of the app. Only a drag of files from
    /// the file browser or the library ([`Self::cross_window_drag`]) follows `under`; every
    /// other gesture stays in its own window.
    pub fn handle_routed(
        &mut self,
        window: WindowKind,
        event: &UiEvent,
        under: Option<WindowHit>,
    ) -> Vec<ShellRequest> {
        if !self.wins.contains_key(&window) {
            return Vec::new();
        }
        let modal = self.modal_open();
        let mut requests = self.handle_event_inner(window, event, under);
        self.track_tooltip(window, event);
        self.sync_library_ui();
        self.save_zoom_state();
        requests.append(&mut self.pending);
        self.after_event(modal, &mut requests);
        requests
    }

    /// File drag and drop. Hovers and drops go to the view under the
    /// pointer: the arrangement (it maps them to a track and a snapped tick and emits
    /// `UiAction::ImportAudio`), the sample editor (`UiAction::ImportSamples`) or the rack
    /// (a sampler panel emits `ImportSamples`). The other views get a hover cancel. A drop
    /// that lands on nothing that takes files (the console, the transport bar, a rack device
    /// that is not a sampler) imports the files as new tracks at the snapped playhead.
    fn file_event(&mut self, window: WindowKind, event: &UiEvent) {
        self.repaint = true;
        let target = match event {
            UiEvent::FileHover { pos, .. } | UiEvent::FileDrop { pos, .. } => {
                self.file_target_in(window, *pos)
            }
            _ => None,
        };
        for pane in [
            Pane::Arrangement,
            Pane::SampleEditor,
            Pane::StepEditor,
            Pane::SliceEditor,
            Pane::SeqEditor,
            Pane::Rack,
        ] {
            if Some(pane) != target {
                self.forward(pane, &UiEvent::FileHoverCancel);
            }
        }
        match event {
            UiEvent::FileHover { paths, .. } => {
                // Library items hover only over the panes that take them.
                let takes = matches!(target, Some(Pane::Rack | Pane::Arrangement))
                    || !paths.iter().any(|p| is_library_item(p));
                if let Some(pane) = target
                    && takes
                {
                    self.forward(pane, event);
                }
            }
            UiEvent::FileDrop { pos, paths } => {
                // A project opens (after the question about unsaved changes) wherever it
                // lands, a MIDI file imports as File > Import MIDI does, and a library item
                // goes to the rack or the arrangement: none of them is audio. The audio files
                // among them go on to the pane under the pointer.
                let (projects, rest): (Vec<PathBuf>, Vec<PathBuf>) =
                    paths.iter().cloned().partition(|p| is_project_item(p));
                if let Some(project) = projects.into_iter().next() {
                    self.open_project_request(project);
                }
                let (midi, rest): (Vec<PathBuf>, Vec<PathBuf>) = rest
                    .into_iter()
                    .partition(|p| crate::midi_file::is_midi_path(p));
                for path in &midi {
                    self.core.import_midi_tracks(path);
                }
                let (items, rest): (Vec<PathBuf>, Vec<PathBuf>) =
                    rest.into_iter().partition(|p| is_library_item(p));
                if !items.is_empty() {
                    self.drop_library_items(target, *pos, items);
                }
                if rest.is_empty() {
                    if let Some(pane) = target {
                        self.forward(pane, &UiEvent::FileHoverCancel);
                    }
                    return;
                }
                let paths = &rest;
                let event = &UiEvent::FileDrop {
                    pos: *pos,
                    paths: rest.clone(),
                };
                let taken = target.is_some_and(|pane| {
                    let actions = self.forward(pane, event);
                    pane != Pane::Rack
                        || actions
                            .iter()
                            .any(|a| matches!(a, UiAction::ImportSamples { .. }))
                });
                if !taken {
                    self.core.import_at_playhead(paths.clone());
                }
            }
            _ => {}
        }
    }

    /// Library items dropped at `pos`: the rack takes devices, patches and containers, the
    /// arrangement a clip on a note lane; anything else is refused with a message. Never
    /// audio.
    fn drop_library_items(&mut self, target: Option<Pane>, pos: Point, items: Vec<PathBuf>) {
        let event = UiEvent::FileDrop { pos, paths: items };
        let actions = match target {
            Some(pane @ (Pane::Rack | Pane::Arrangement)) => self.forward(pane, &event),
            _ => Vec::new(),
        };
        let inserted = actions.iter().any(|a| {
            matches!(
                a,
                UiAction::InsertLibrary { .. } | UiAction::InsertLibraryClip { .. }
            )
        });
        if !inserted {
            let UiEvent::FileDrop { paths, .. } = &event else {
                return;
            };
            let message = if paths.iter().any(|p| is_clip_item(p)) {
                "Drop a clip on a note lane of an instrument track"
            } else {
                "Drop devices and patches on the rack"
            };
            self.notify(message);
        }
    }

    /// A pointer event during a drag out of the file browser: moves are file hovers over
    /// whatever takes files under the pointer (cancels over the panel or anything else), the
    /// release is a file drop exactly like an OS drop, and a release over the panel or over
    /// nothing that takes files cancels.
    ///
    /// `at` is the window the pointer is over and the point in it (`None`: over no window of the
    /// app, so there is no target and a release cancels); the file browser is in the main
    /// window, a detached rack is another place a drop can land.
    fn file_drag_event(&mut self, event: &UiEvent, at: Option<(WindowKind, Point)>) {
        self.repaint = true;
        let Some(drag) = self.file_drag.as_mut() else {
            return;
        };
        drag.pos = at;
        if drag.cancelled {
            return;
        }
        let paths = drag.paths.clone();
        let Some((window, pos)) = at else {
            // Over no window: the targets let go; the release ends the gesture.
            self.file_event(WindowKind::Main, &UiEvent::FileHoverCancel);
            return;
        };
        match event {
            UiEvent::PointerMove { .. } => {
                self.file_event(window, &UiEvent::FileHover { pos, paths });
            }
            UiEvent::PointerUp { .. } => {
                let over_panel = self.hit_in(window, pos) == Some(Ok(Pane::FileBrowser));
                if !over_panel && self.file_target_in(window, pos).is_some() {
                    self.file_event(window, &UiEvent::FileDrop { pos, paths });
                } else {
                    self.file_event(window, &UiEvent::FileHoverCancel);
                }
            }
            _ => {}
        }
    }

    /// Esc during a file drag: nothing is delivered any more and the targets let go of their
    /// hover; the release that ends the gesture is swallowed.
    fn cancel_file_drag(&mut self) {
        if let Some(drag) = &mut self.file_drag {
            drag.cancelled = true;
        }
        self.file_event(WindowKind::Main, &UiEvent::FileHoverCancel);
    }

    /// The pane files at a point of `window` go to, if it takes files.
    fn file_target_in(&self, window: WindowKind, pos: Point) -> Option<Pane> {
        match self.hit_in(window, pos)? {
            Ok(
                pane @ (Pane::Arrangement
                | Pane::SampleEditor
                | Pane::StepEditor
                | Pane::SliceEditor
                | Pane::Rack),
            ) => Some(pane),
            _ => None,
        }
    }

    /// Imports the files picked with the sample editor's Load button into the sampler it was
    /// opened for (as a drop with no key).
    pub fn load_samples(&mut self, paths: Vec<PathBuf>) {
        if let Some(device) = self.load_device.take() {
            self.core.import_samples(paths, device, None);
        }
        self.repaint = true;
    }

    /// Imports the files picked in the Import Audio dialog: onto the selected audio track, else
    /// onto new tracks, at the snapped playhead ([`AppCore::import_dialog_target`]).
    pub fn import_files(&mut self, paths: Vec<PathBuf>) {
        let (target, tick) = self.core.import_dialog_target();
        self.core.import_audio(paths, target, tick);
        self.repaint = true;
    }

    fn handle_event_inner(
        &mut self,
        window: WindowKind,
        event: &UiEvent,
        under: Option<WindowHit>,
    ) -> Vec<ShellRequest> {
        if matches!(
            event,
            UiEvent::FileHover { .. } | UiEvent::FileHoverCancel | UiEvent::FileDrop { .. }
        ) {
            if window != WindowKind::Main && self.modal_open() {
                return Vec::new();
            }
            self.file_event(window, event);
            return Vec::new();
        }
        match event {
            UiEvent::ScaleChanged(scale) => {
                if let Some(w) = self.wins.get_mut(&window) {
                    w.scale = *scale;
                }
                self.apply_scale(window);
                return Vec::new();
            }
            UiEvent::Resize(size) => {
                if let Some(w) = self.wins.get_mut(&window) {
                    w.size = *size;
                }
                if self
                    .context_menu
                    .as_ref()
                    .is_some_and(|o| o.window == window)
                {
                    self.context_menu = None;
                }
                self.layout_view();
                return Vec::new();
            }
            _ => {}
        }
        // A key from a window the shell did not know had the focus: the platform's focus event
        // may come second, and a headless caller has none.
        if matches!(event, UiEvent::Key { pressed: true, .. }) && self.focused != window {
            self.window_focused(window);
        }
        if let UiEvent::ModifiersChanged { modifiers } = event {
            // Held-modifier state only; nothing repaints until a role depends on it. The
            // settings dialog learns a role from the modifiers held.
            self.modifiers = *modifiers;
            let size = self.size();
            if let Some(dialog) = &mut self.dialog
                && dialog.capturing_keys()
            {
                dialog.handle(event, size);
                self.repaint = true;
            }
            self.forward_modifiers();
            return Vec::new();
        }
        // The modals live in the main window: while one is open the other windows are inert, and
        // a press in one brings the main window forward.
        if window != WindowKind::Main && self.modal_open() {
            return if matches!(event, UiEvent::PointerDown { .. }) {
                vec![ShellRequest::FocusWindow(WindowKind::Main)]
            } else {
                Vec::new()
            };
        }
        if self.plugin_settings.is_some() {
            // The Plugins modal takes every event; Esc closes it.
            self.repaint = true;
            if let UiEvent::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } = event
            {
                self.plugin_settings = None;
                return Vec::new();
            }
            let actions = self
                .plugin_settings
                .as_mut()
                .map(|v| v.handle(&text_form(event)))
                .unwrap_or_default();
            for action in actions {
                self.core.handle_action(action);
            }
            self.sync_plugin_ui();
            return Vec::new();
        }
        if self.macros_panel.is_some() {
            // The Macros panel takes every event; Esc closes it (or drops a name being edited).
            self.repaint = true;
            let actions = self
                .macros_panel
                .as_mut()
                .map(|v| v.handle(&text_form(event)))
                .unwrap_or_default();
            for action in actions {
                if let UiAction::Macros(a) = action {
                    self.macros_action(a);
                }
            }
            return Vec::new();
        }
        if self.param_search.is_some() {
            // The parameter search takes every event: the keys are its own, Esc closes it.
            self.repaint = true;
            let actions = self
                .param_search
                .as_mut()
                .map(|v| v.handle(&text_form(event)))
                .unwrap_or_default();
            for action in actions {
                if let UiAction::ParamSearch(a) = action {
                    self.param_search_action(a);
                }
            }
            return Vec::new();
        }
        if self.midi_maps.is_some() {
            // The MIDI Mappings panel takes every event; Esc closes it.
            self.repaint = true;
            if let UiEvent::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } = event
            {
                self.midi_maps = None;
                return Vec::new();
            }
            let actions = self
                .midi_maps
                .as_mut()
                .map(|v| v.handle(&text_form(event)))
                .unwrap_or_default();
            let mut requests = Vec::new();
            for action in actions {
                if let UiAction::MidiMaps(a) = action {
                    requests.extend(self.midi_maps_action(a));
                }
            }
            return requests;
        }
        if self.export.is_some() {
            // The export panel takes every event; Esc closes it (or stops the render).
            self.repaint = true;
            let actions = self
                .export
                .as_mut()
                .map(|v| v.handle(&text_form(event)))
                .unwrap_or_default();
            for action in actions {
                if let UiAction::Export(a) = action {
                    self.export_action(a);
                }
            }
            return Vec::new();
        }
        if self.relink.is_some() {
            // The relink panel takes every event; Esc closes it.
            self.repaint = true;
            if let UiEvent::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } = event
            {
                self.relink = None;
                return Vec::new();
            }
            let actions = self
                .relink
                .as_mut()
                .map(|v| v.handle(&text_form(event)))
                .unwrap_or_default();
            let mut requests = Vec::new();
            for action in actions {
                if let UiAction::Relink(a) = action {
                    requests.extend(self.relink_action(a));
                }
            }
            return requests;
        }
        if self.warp.is_some() {
            // The warp panel takes every event; Esc closes it.
            self.repaint = true;
            if let UiEvent::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } = event
            {
                self.close_warp();
                return Vec::new();
            }
            let actions = self
                .warp
                .as_mut()
                .map(|v| v.handle(&text_form(event)))
                .unwrap_or_default();
            for action in actions {
                if let UiAction::Warp(a) = action {
                    self.warp_action(a);
                }
            }
            self.sync_warp();
            return Vec::new();
        }
        if self.notes.is_some() {
            // The Audio to Notes panel takes every event; Esc closes it.
            self.repaint = true;
            if let UiEvent::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } = event
            {
                self.close_notes();
                return Vec::new();
            }
            let actions = self
                .notes
                .as_mut()
                .map(|v| v.handle(&text_form(event)))
                .unwrap_or_default();
            for action in actions {
                if let UiAction::AudioToNotes(a) = action {
                    self.notes_action(a);
                }
            }
            self.sync_notes();
            return Vec::new();
        }
        if self.silence.is_some() {
            // The strip silence panel takes every event; Esc closes it.
            self.repaint = true;
            if let UiEvent::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } = event
            {
                self.close_silence();
                return Vec::new();
            }
            let actions = self
                .silence
                .as_mut()
                .map(|v| v.handle(&text_form(event)))
                .unwrap_or_default();
            for action in actions {
                if let UiAction::StripSilence(a) = action {
                    self.silence_action(a);
                }
            }
            self.sync_silence();
            return Vec::new();
        }
        if self.quantize.is_some() {
            // The quantize panel takes every event; Esc closes it.
            self.repaint = true;
            if let UiEvent::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } = event
            {
                self.close_quantize();
                return Vec::new();
            }
            let actions = self
                .quantize
                .as_mut()
                .map(|v| v.handle(&text_form(event)))
                .unwrap_or_default();
            for action in actions {
                if let UiAction::Quantize(a) = action {
                    self.quantize_action(a);
                }
            }
            self.sync_quantize();
            return Vec::new();
        }
        let size = self.size();
        if let Some(dialog) = &mut self.dialog {
            self.repaint = true;
            // Learning a chord needs the keys as they are pressed (numpad keys included).
            let event = if dialog.capturing_keys() {
                event.clone()
            } else {
                text_form(event)
            };
            match dialog.handle(&event, size) {
                DialogOutcome::Open => {}
                DialogOutcome::Cancel => self.dialog = None,
                DialogOutcome::Apply(settings) => {
                    let prefs = dialog.preferences();
                    self.dialog = None;
                    self.apply_preferences(prefs);
                    self.apply_settings(settings);
                }
                DialogOutcome::ExportKeymap => return vec![ShellRequest::PickExportKeymap],
                DialogOutcome::ImportKeymap => return vec![ShellRequest::PickImportKeymap],
                DialogOutcome::PickLibraryFolder => return vec![ShellRequest::PickLibraryFolder],
            }
            return Vec::new();
        }
        if self.context_menu.is_some() {
            return self.context_menu_event(window, event);
        }
        let bar_open = self.menubar.as_ref().is_some_and(MenuBar::is_open);
        if let UiEvent::Key {
            key,
            modifiers,
            pressed,
        } = event
            && !bar_open
        {
            self.repaint = true;
            if self.file_drag.is_some() && *pressed && *key == Key::Escape {
                self.cancel_file_drag();
                return Vec::new();
            }
            // Escape ends a device dragged out of the browser, whichever pane has the focus.
            if self.core.view().is_browser_dragging() && *pressed && *key == Key::Escape {
                self.forward(Pane::Rack, event);
                return Vec::new();
            }
            if self.core.transport_bar().is_editing_tempo()
                || self.core.transport_bar().is_editing_meter()
                || self.core.transport_bar().is_editing_inspector()
            {
                self.forward(Pane::Transport, &text_form(event));
                return Vec::new();
            }
            let stack = self.context_stack();
            match self.router.route(key, *modifiers, *pressed, &stack) {
                Route::RepeatIgnored => {}
                Route::Commands(list) => {
                    for resolved in &list {
                        if let Some(requests) =
                            self.run_command(&resolved.command, resolved.arg.as_deref())
                        {
                            return requests;
                        }
                    }
                    // Every command declined: the focused view gets the key (text, Escape).
                    self.route_key(event);
                }
                Route::Release | Route::Text | Route::Unbound => self.route_key(event),
            }
            return Vec::new();
        }
        // A press keeps every pointer event until its release.
        if let Some((_, capture)) = self.capture
            && let Some(pos) = pointer_pos(event)
        {
            let released = matches!(event, UiEvent::PointerUp { .. });
            match capture {
                Capture::Pane(pane) => {
                    self.forward(pane, event);
                }
                Capture::Divider { grab } => self.drag_divider(pos, grab, released),
                Capture::FileDrag => {
                    let at = under.map(|h| (h.kind, Point::new(h.logical.0, h.logical.1)));
                    self.file_drag_event(event, at);
                }
            }
            if released {
                self.capture = None;
                self.file_drag = None;
            }
            return Vec::new();
        }
        if window != WindowKind::Main {
            return self.pointer_in_detached(window, event);
        }
        // The plugin banner takes the pointer inside its strip (below the menu bar).
        if self.banner.is_visible()
            && let Some(pos) = pointer_pos(event)
        {
            let bar = self.bar_height();
            let banner_x = self.layout().file_browser.map_or(0.0, |r| r.x1);
            if pos.x >= banner_x && pos.y >= bar && pos.y < bar + self.banner.height() {
                self.repaint = true;
                let actions = self.banner.handle(event);
                for action in actions {
                    self.core.handle_action(action);
                }
                self.sync_plugin_ui();
                self.layout_view();
                return Vec::new();
            }
        }
        let model = self.menu_model();
        if let Some(bar) = &mut self.menubar {
            let before = bar.look();
            match bar.handle(event, &model) {
                MenuBarOutcome::Ignored => {}
                MenuBarOutcome::Consumed => {
                    // A move that leaves the bar as it was (the pointer inside one title)
                    // shows nothing new, and one that changes it only repaints the main
                    // window, where the bar is.
                    let moved = matches!(event, UiEvent::PointerMove { .. });
                    let changed = bar.look() != before;
                    if !moved {
                        self.repaint = true;
                    } else if changed && let Some(w) = self.win_mut(WindowKind::Main) {
                        w.dirty = true;
                    }
                    self.leave_hover();
                    return Vec::new();
                }
                MenuBarOutcome::Command(command) => {
                    self.repaint = true;
                    return self.execute_command(command);
                }
            }
        }
        if matches!(event, UiEvent::PointerLeave) {
            self.leave_hover();
            return Vec::new();
        }
        let Some(pos) = pointer_pos(event) else {
            self.forward(Pane::Rack, event);
            return Vec::new();
        };
        match self.hit_in(WindowKind::Main, pos) {
            None => self.leave_hover(),
            Some(Err(())) => {
                self.leave_hover();
                if matches!(event, UiEvent::PointerDown { .. })
                    && let Some(d) = self.layout().divider
                {
                    self.focused = WindowKind::Main;
                    self.capture =
                        Some((WindowKind::Main, Capture::Divider { grab: pos.y - d.y0 }));
                }
            }
            Some(Ok(pane)) => {
                if self.hover_in(WindowKind::Main) != Some(pane) {
                    self.leave_hover();
                    self.set_hover_in(WindowKind::Main, Some(pane));
                }
                if matches!(event, UiEvent::PointerDown { .. }) {
                    self.focused = WindowKind::Main;
                    self.set_focus(pane);
                    self.capture = Some((WindowKind::Main, Capture::Pane(pane)));
                }
                self.forward(pane, event);
            }
        }
        Vec::new()
    }

    /// Runs one registry command: the focused view first (its own commands, so a context
    /// binding shadows a global one), then the shell's. `None`: nothing took it, the next
    /// binding of the list runs.
    fn run_command(&mut self, id: &str, arg: Option<&str>) -> Option<Vec<ShellRequest>> {
        let pane = self.live_pane(self.focus);
        if let Some(actions) = self.core.pane_command(pane, id, arg) {
            self.repaint = true;
            self.pane_actions(&actions);
            // A clip opened by key (Enter): the editor takes the keys.
            if actions
                .iter()
                .any(|a| matches!(a, UiAction::OpenPianoRoll(_)))
                && self.core.piano_roll_clip().is_some()
            {
                self.reveal_lower_pane();
                self.set_focus(Pane::PianoRoll);
            }
            return Some(Vec::new());
        }
        self.run_shell_command(id, arg)
    }

    /// The commands the shell itself runs (see [`Self::run_command`]).
    fn run_shell_command(&mut self, id: &str, arg: Option<&str>) -> Option<Vec<ShellRequest>> {
        match (id, arg) {
            // The quantize key outside an editor: the selected clips of the arrangement.
            ("editor.quantize", None) => {
                let pane = self.live_pane(self.focus);
                self.apply_last_quantize(pane);
            }
            ("automation.edit_control", None) => {
                if let Some((device, param)) = self.control_target.clone()
                    && self.core.show_lane_of(&device, &param)
                {
                    self.show_arrangement_focus();
                }
            }
            ("midi.learn_control", None) => {
                if let Some((device, param)) = self.control_target.clone() {
                    self.core
                        .midi_arm(atomik_model::MidiTarget::Param { device, param });
                }
            }
            ("macro.assign_control", Some(n)) => {
                if let (Some((device, param)), Ok(n)) =
                    (self.control_target.clone(), n.parse::<u8>())
                    && (1..=8).contains(&n)
                {
                    self.core.assign_to_macro(&device, &param, n - 1, false);
                }
            }
            ("macro.assign_control_new", Some(n)) => {
                if let (Some((device, param)), Ok(n)) =
                    (self.control_target.clone(), n.parse::<u8>())
                    && (1..=8).contains(&n)
                {
                    self.core.assign_to_macro(&device, &param, n - 1, true);
                }
            }
            ("macro.remove_control", None) => {
                if let Some((device, param)) = self.control_target.clone() {
                    self.core.remove_from_macro(&device, &param);
                }
            }
            ("midi.forget_control", None) => {
                if let Some((device, param)) = self.control_target.clone() {
                    self.core
                        .midi_forget(atomik_model::MidiTarget::Param { device, param });
                }
            }
            ("sequencer.audio_normalise", None) => self.core.normalise_audio(),
            ("sequencer.audio_strip_silence", None) => self.open_silence(),
            ("sequencer.audio_to_notes", None) => self.open_notes(),
            ("sequencer.audio_quantize", None) => self.quantize_audio(),
            ("sequencer.audio_warp_tempo", None) => self.core.warp_to_song_tempo(),
            ("sequencer.audio_warp_bars", Some(bars)) => {
                if let Ok(bars) = bars.parse() {
                    self.core.warp_bars(bars);
                }
            }
            ("sequencer.audio_warp_panel", None) => self.open_warp(),
            ("transport.stop", None) => self.core.stop_or_rewind(),
            ("transport.rewind_bar" | "transport.bar_back", None) => self.core.locate_bars(-1),
            ("transport.forward_bar" | "transport.bar_forward", None) => self.core.locate_bars(1),
            ("transport.go_locator", Some("left")) => self.core.go_locator(false),
            ("transport.go_locator", Some("right")) => self.core.go_locator(true),
            ("transport.tempo_up", None) => self.core.nudge_tempo(1.0),
            ("transport.tempo_down", None) => self.core.nudge_tempo(-1.0),
            ("rack.go_to_track", None) => {
                // The track of the selected device is selected in the arrangement, which shows.
                if self.core.go_to_track() {
                    self.reveal_lower_pane();
                    self.set_focus(Pane::Arrangement);
                }
            }
            ("rack.save_to_library", None) => self.core.save_selection_to_library(),
            ("sequencer.save_clip_to_library", None) => self.core.save_clip_to_library(),
            ("view.cables_cycle", None) => {
                let view = self.core.view_mut();
                let next = match view.cable_visibility() {
                    atomik_ui::CableVisibility::Show => atomik_ui::CableVisibility::HoverOnly,
                    atomik_ui::CableVisibility::HoverOnly => atomik_ui::CableVisibility::Hide,
                    atomik_ui::CableVisibility::Hide => atomik_ui::CableVisibility::Show,
                };
                view.set_cable_visibility(next);
            }
            (files, None) if files.starts_with("files.") => {
                let (key, modifiers) = files_key(files, self.os)?;
                self.forward(
                    Pane::FileBrowser,
                    &UiEvent::Key {
                        key,
                        modifiers,
                        pressed: true,
                    },
                );
            }
            ("view.focus_browser_search", None) => {
                self.show_rack();
                self.set_focus(Pane::Rack);
                self.core.view_mut().set_browser_visible(true);
            }
            _ => {
                let command = AppCommand::from_command(id, arg)?;
                self.repaint = true;
                return Some(self.execute_command(command));
            }
        }
        self.repaint = true;
        self.core.after_view_change();
        Some(Vec::new())
    }

    /// Shows the rack again when F6 or F7 hid it (a command that needs it, such as the create
    /// browser, calls this); a maximised console steps back too.
    fn show_rack(&mut self) {
        if self.rack_detached() {
            // Nothing to show in the main window: the rack's own window comes forward.
            self.pending
                .push(ShellRequest::FocusWindow(WindowKind::Rack));
            return;
        }
        let mut changed = false;
        if self.ui.console_maximised {
            self.ui.console_maximised = false;
            changed = true;
        }
        if !self.workflow.rack_visible {
            self.workflow.rack_visible = true;
            self.seq_only_prev = None;
            self.save_workflow();
            changed = true;
        }
        if changed {
            self.layout_view();
            self.save_ui_state();
        }
    }

    /// F6: shows or hides the rack. The song area is never hidden with it.
    fn set_rack_visible(&mut self, visible: bool) {
        self.workflow.rack_visible = visible;
        if !visible {
            self.ui.show_song = true;
        }
        self.set_focus(self.live_pane(self.focus));
        self.layout_view();
        self.save_workflow();
        self.save_ui_state();
    }

    /// Whether the layout is the sequencer-only one: no rack, no console, the song area shown.
    fn in_sequencer_only(&self) -> bool {
        self.ui.show_song && !self.workflow.rack_visible && !self.lower_is_console()
    }

    /// F7: sequencer only (no rack, no console), or back to the layout it replaced (the rack
    /// and no console when nothing was remembered).
    fn toggle_sequencer_only(&mut self) {
        if self.in_sequencer_only() {
            let prev = self.seq_only_prev.take().unwrap_or(SeqOnlyPrev {
                rack: true,
                console: false,
                maximised: false,
            });
            self.workflow.rack_visible = prev.rack;
            self.ui.show_console = prev.console;
            self.ui.console_maximised = prev.maximised;
        } else {
            self.seq_only_prev = Some(SeqOnlyPrev {
                rack: self.workflow.rack_visible,
                console: self.ui.show_console,
                maximised: self.ui.console_maximised,
            });
            self.workflow.rack_visible = false;
            self.ui.show_console = false;
            self.ui.console_maximised = false;
            self.ui.show_song = true;
        }
        self.set_focus(self.live_pane(self.focus));
        self.layout_view();
        self.save_workflow();
        self.save_ui_state();
    }

    /// Brings the arrangement into view and gives it the keys (a lane was just shown there).
    fn show_arrangement_focus(&mut self) {
        self.reveal_lower_pane();
        self.set_focus(Pane::Arrangement);
    }

    /// The song area shows the editors and the arrangement, not the console or nothing.
    fn reveal_lower_pane(&mut self) {
        if !self.ui.show_song || self.lower_is_console() {
            self.ui.show_song = true;
            self.ui.show_console = false;
            self.console_changed();
        }
    }

    /// Gives the keys to the editor that shows in the lower pane, if one does.
    fn focus_editor(&mut self) {
        let lower = self.lower_pane();
        if !self.lower_is_console() && !matches!(lower, Pane::Arrangement | Pane::Console) {
            self.set_focus(lower);
        }
    }

    /// F8: closes the open editor, or opens the editor of the selected clip (else of the
    /// selected device's first clip, else the song's first note clip).
    fn toggle_editor(&mut self) {
        if self.core.close_open_editor() {
            self.set_focus(self.live_pane(Pane::Arrangement));
            self.layout_view();
            return;
        }
        let selected: Option<ClipId> = {
            let song = &self.core.doc().song;
            self.core
                .arrangement()
                .selected_clips()
                .into_iter()
                .find(|id| song.clip(*id).is_some_and(|(_, _, c)| c.content.is_notes()))
        };
        if let Some(id) = selected {
            self.reveal_lower_pane();
            let action = UiAction::OpenPianoRoll(id);
            self.core.handle_action(action.clone());
            self.pane_actions(&[action]);
        } else {
            self.execute_command(AppCommand::TogglePianoRoll);
        }
        self.focus_editor();
    }

    /// Create Instrument / Create Effect: the device browser in create mode, on the rack
    /// (shown if it was hidden), its filter focused.
    fn open_create(&mut self, kind: CreateKind) {
        self.show_rack();
        self.set_focus(Pane::Rack);
        self.core.open_create_browser(kind);
    }

    /// Sends a key no command took to the focused view (numpad keys as the characters they
    /// type).
    fn route_key(&mut self, event: &UiEvent) {
        let focus = self.live_pane(self.focus);
        self.forward(focus, &text_form(event));
    }

    fn drag_divider(&mut self, pos: Point, grab: f64, released: bool) {
        let top = self.top();
        let bottom = (self.size().height - STATUS_BAR_HEIGHT).max(top + 1.0);
        let avail = (bottom - top - TRANSPORT_HEIGHT - DIVIDER_HEIGHT).max(2.0);
        let rack_h = (pos.y - grab - top).clamp(0.0, avail);
        let lo = MIN_PANE_HEIGHT.min(avail / 2.0);
        self.ui.rack_split = rack_h.clamp(lo, avail - lo) / avail;
        self.layout_view();
        if released {
            self.save_ui_state();
        }
    }

    /// The vertical zooms (track heights, key height) and the piano roll's snap to scale and
    /// fold are saved when an event changed them.
    fn save_zoom_state(&mut self) {
        let zoom = self.core.arrangement().track_zoom();
        let roll = self.core.piano_roll();
        let (keys, snap_scale, fold) = (roll.key_height(), roll.snap_to_scale(), roll.fold());
        if (zoom - self.ui.track_zoom).abs() > f64::EPSILON
            || (keys - self.ui.piano_roll_key_height).abs() > f64::EPSILON
            || snap_scale != self.ui.piano_roll_snap_scale
            || fold != self.ui.piano_roll_fold
        {
            self.ui.track_zoom = zoom;
            self.ui.piano_roll_key_height = keys;
            self.ui.piano_roll_snap_scale = snap_scale;
            self.ui.piano_roll_fold = fold;
            self.save_ui_state();
        }
    }

    fn save_ui_state(&mut self) {
        let view = self.core.file_browser();
        self.ui.file_browser_width = view.width();
        self.ui.file_browser_tree_fraction = view.tree_fraction();
        self.ui.file_browser_auto_preview = view.auto_preview();
        if let Some(path) = &self.ui_state_path
            && let Err(e) = self.ui.save(path)
        {
            let message = format!("window state not saved: {e}");
            self.notify(message);
        }
    }

    /// The panel was dragged wider or narrower: lay the other views out again.
    fn follow_browser_resize(&mut self) {
        let edge = self.layout().file_browser.map_or(0.0, |r| r.x1);
        if (edge - self.browser_edge).abs() > f64::EPSILON {
            self.layout_view();
        }
    }

    /// Sends an event to a pane: the rack view gets its own coordinates (below the menu
    /// bar), the sequencer views window coordinates.
    fn forward(&mut self, pane: Pane, event: &UiEvent) -> Vec<UiAction> {
        let actions = self.forward_to(pane, event);
        self.mark_forwarded(pane, event, &actions);
        actions
    }

    /// What a forwarded event repaints. Actions can change the document or another pane, so
    /// every window is marked; an event without actions changed only the view that took it,
    /// so only the window showing that pane is marked, and a pointer move or modifier change
    /// marks nothing: the views raise their own `needs_repaint` when what they show changes
    /// (a hover, a drag), so a move over nothing paints nothing.
    fn mark_forwarded(&mut self, pane: Pane, event: &UiEvent, actions: &[UiAction]) {
        if !actions.is_empty() {
            self.repaint = true;
        } else if !matches!(
            event,
            UiEvent::PointerMove { .. } | UiEvent::PointerLeave | UiEvent::ModifiersChanged { .. }
        ) {
            let window = self.window_of(pane);
            if let Some(w) = self.win_mut(window) {
                w.dirty = true;
            }
        }
    }

    fn forward_to(&mut self, pane: Pane, event: &UiEvent) -> Vec<UiAction> {
        if pane != Pane::Rack {
            let actions = self.core.handle_pane_event(pane, event);
            if matches!(pane, Pane::Console | Pane::Arrangement)
                && self.core.take_rack_focus_request()
            {
                self.set_focus(Pane::Rack);
            }
            if pane == Pane::FileBrowser {
                self.follow_browser_resize();
            }
            self.pane_actions(&actions);
            self.open_context_menus(pane, &actions);
            return actions;
        }
        let origin = self
            .pane_rect(Pane::Rack)
            .map_or(Point::ORIGIN, |(_, r)| r.origin());
        let shift = |p: Point| Point::new(p.x - origin.x, p.y - origin.y);
        let moved = match event.clone() {
            UiEvent::PointerMove { pos, modifiers } => UiEvent::PointerMove {
                pos: shift(pos),
                modifiers,
            },
            UiEvent::PointerDown {
                pos,
                button,
                modifiers,
                clicks,
            } => UiEvent::PointerDown {
                pos: shift(pos),
                button,
                modifiers,
                clicks,
            },
            UiEvent::PointerUp {
                pos,
                button,
                modifiers,
            } => UiEvent::PointerUp {
                pos: shift(pos),
                button,
                modifiers,
            },
            UiEvent::Wheel {
                delta,
                pos,
                modifiers,
            } => UiEvent::Wheel {
                delta,
                pos: shift(pos),
                modifiers,
            },
            UiEvent::FileHover { pos, paths } => UiEvent::FileHover {
                pos: shift(pos),
                paths,
            },
            UiEvent::FileDrop { pos, paths } => UiEvent::FileDrop {
                pos: shift(pos),
                paths,
            },
            other => other,
        };
        let actions = self.core.handle_event(&moved);
        self.pane_actions(&actions);
        self.open_context_menus(Pane::Rack, &actions);
        actions
    }

    // ------------------------------------------------------------------ context menus

    /// What decides which items of a context menu opened in `pane` can be chosen.
    fn menu_enable(&self, pane: Pane) -> MenuEnable {
        let core = &self.core;
        let doc = core.doc();
        let clips = core.arrangement().selected_clips();
        MenuEnable {
            selection: core.has_selection(pane),
            paste: core.can_paste(pane),
            has_track: core
                .view()
                .primary()
                .is_some_and(|d| doc.song.track(&d).is_some()),
            join: clips.len() >= 2,
            clips_muted: !clips.is_empty()
                && clips
                    .iter()
                    .all(|id| doc.song.clip(*id).is_some_and(|(_, _, c)| c.muted)),
            notes_muted: {
                let sel = core.piano_roll().selected_notes();
                let notes = core.piano_roll().notes();
                !sel.is_empty() && sel.iter().all(|i| notes.get(*i).is_some_and(|n| n.muted))
            },
            save_clip: matches!(clips.as_slice(), [id]
                if doc.song.clip(*id).is_some_and(|(_, _, c)| c.content.is_notes())),
            open_clip: clips.iter().any(|id| {
                doc.song
                    .clip(*id)
                    .is_some_and(|(_, _, c)| c.content.is_notes())
            }),
            master_only: pane == Pane::Rack
                && core.view().selection_len() == 1
                && doc.console.master.is_some()
                && core.view().primary() == doc.console.master,
            assign_send: pane == Pane::Rack
                && core
                    .view()
                    .assign_send_reason(doc, core.registry())
                    .is_none(),
            columns: doc.columns,
            track_can_chase: core.arrangement().chase_menu().0,
            track_chase: core.arrangement().chase_menu().1,
            lane: core.arrangement().lane_menu(),
            org: core.arrangement().org_menu(),
            audio: core.arrangement().audio_menu(),
            freeze: core.freeze_menu(),
            tempo_can_ramp: core.arrangement().tempo_menu().can_ramp,
            tempo_ramped: core.arrangement().tempo_menu().ramped,
            tempo_can_delete: core.arrangement().tempo_menu().can_delete,
            meter_can_delete: core.arrangement().meter_change_deletable(),
            marker_selection: !clips.is_empty(),
            notes: core.piano_roll().tools(),
            control_mapped: self
                .control_target
                .as_ref()
                .is_some_and(|(d, p)| core.is_midi_mapped(d, p)),
            control_lane: self
                .control_target
                .as_ref()
                .is_some_and(|(d, p)| core.has_lane_for(d, p)),
            control_assign: self
                .control_target
                .as_ref()
                .is_some_and(|(d, p)| core.macro_control_state(d, p).0),
            control_driven: self
                .control_target
                .as_ref()
                .is_some_and(|(d, p)| core.macro_control_state(d, p).1),
        }
    }

    /// Opens the context menu of `target` at the window point `at`, built from the registry.
    fn open_context_menu(&mut self, pane: Pane, target: ContextTarget, at: Point) {
        let enable = self.menu_enable(pane);
        let specs = context_menus::specs(target, &enable);
        // It opens in the window that shows the pane, sized to that window.
        let window = self.window_of(pane);
        let size = self.window_size(window).unwrap_or_else(|| self.size());
        let menu = ContextMenu::build(
            &specs,
            &atomik_ui::keymap::Registry::bundled(),
            self.router.keymap(),
            at,
            size,
        );
        self.context_menu = Some(OpenContextMenu { menu, pane, window });
        // The pointer press that opened it must not keep a drag alive.
        self.capture = None;
        self.leave_all_hover();
        self.repaint = true;
    }

    /// The open context menu, if any.
    pub fn context_menu(&self) -> Option<&ContextMenu> {
        self.context_menu.as_ref().map(|o| &o.menu)
    }

    /// Whether the context menu waits for a pointer to rest (the platform loop keeps ticking).
    pub fn context_menu_pending(&self) -> bool {
        self.context_menu
            .as_ref()
            .is_some_and(|o| o.menu.has_pending())
    }

    /// Closes the context menu without choosing.
    pub fn close_context_menu(&mut self) {
        if self.context_menu.take().is_some() {
            self.repaint = true;
        }
    }

    /// An event while a context menu is open: it takes the pointer and the keys. A click on an
    /// item runs its command as a key would (in the pane the menu belongs to); Escape or a
    /// click elsewhere closes it.
    fn context_menu_event(&mut self, window: WindowKind, event: &UiEvent) -> Vec<ShellRequest> {
        let Some(open) = &mut self.context_menu else {
            return Vec::new();
        };
        // The menu belongs to one window: a press in another one closes it (and is not
        // delivered); the other pointer events of that window are not the menu's.
        if open.window != window && pointer_pos(event).is_some() {
            if matches!(event, UiEvent::PointerDown { .. }) {
                self.context_menu = None;
                self.repaint = true;
            }
            return Vec::new();
        }
        if let UiEvent::Key {
            key,
            modifiers,
            pressed: false,
        } = event
        {
            // A release must still reach the router, or its held-key state would go stale.
            let stack = self.context_stack();
            let _ = self.router.route(key, *modifiers, false, &stack);
            return Vec::new();
        }
        if open.menu.wants_close(event) {
            self.context_menu = None;
            self.repaint = true;
            return Vec::new();
        }
        let pane = open.pane;
        // Only a change of what the menu shows repaints: a pointer move inside one row does
        // not, and repainting the window behind the menu is the costly part.
        let before = open.menu.look();
        match open.menu.handle(event) {
            MenuOutcome::Run { command, arg } => {
                self.context_menu = None;
                self.repaint = true;
                self.set_focus(pane);
                self.run_command(&command, arg.as_deref())
                    .unwrap_or_default()
            }
            MenuOutcome::Consumed | MenuOutcome::Ignored => {
                if open.menu.look() != before {
                    self.repaint = true;
                }
                Vec::new()
            }
        }
    }

    /// The context-menu requests among the actions a pane emitted: `at` is in the pane's
    /// coordinates (the rack view sits below the menu bar).
    fn open_context_menus(&mut self, pane: Pane, actions: &[UiAction]) {
        for action in actions {
            let (target, at) = match action {
                UiAction::ContextMenu { target, at } => (*target, *at),
                UiAction::ControlMenu { device, param, at } => {
                    self.control_target = Some((device.clone(), param.clone()));
                    (ContextTarget::Control, *at)
                }
                _ => continue,
            };
            let at = if pane == Pane::Rack {
                let o = self
                    .pane_rect(Pane::Rack)
                    .map_or(Point::ORIGIN, |(_, r)| r.origin());
                Point::new(at.x + o.x, at.y + o.y)
            } else {
                at
            };
            self.open_context_menu(pane, target, at);
        }
    }

    /// Runs the view actions only the shell can do (the console toolbar's pane switches).
    fn pane_actions(&mut self, actions: &[UiAction]) {
        for action in actions {
            match action {
                UiAction::ToggleConsole => {
                    let requests = self.execute_command(AppCommand::ToggleConsole);
                    self.pending.extend(requests);
                }
                UiAction::MaximiseConsole => {
                    let requests = self.execute_command(AppCommand::MaximiseConsole);
                    self.pending.extend(requests);
                }
                UiAction::ImportAudioDialog => self.pending.push(ShellRequest::PickImportFiles),
                // The piano roll's QUANT button: the panel's last settings, on the roll.
                UiAction::Quantize(QuantizeAction::ApplyLast) => {
                    self.apply_last_quantize(Pane::PianoRoll);
                }
                UiAction::Quantize(QuantizeAction::Options) => self.open_quantize(),
                UiAction::OpenSampleEditor(_) => self.show_sample_editor(),
                UiAction::CloseSampleEditor => {
                    if self.focus == Pane::SampleEditor {
                        self.set_focus(Pane::Arrangement);
                    }
                }
                UiAction::OpenStepEditor { .. } | UiAction::CreateStepClip { .. } => {
                    self.show_step_editor();
                }
                // A double-click on a drum machine's clip opens the step editor instead.
                UiAction::OpenPianoRoll(_) if self.core.step_editor_device().is_some() => {
                    self.show_step_editor();
                }
                UiAction::CloseStepEditor => {
                    if self.focus == Pane::StepEditor {
                        self.set_focus(Pane::Arrangement);
                    }
                }
                UiAction::OpenSeqEditor { .. } => self.show_seq_editor(),
                UiAction::OpenContainerEditor { .. } => self.show_container_editor(),
                UiAction::CloseContainerEditor => {
                    if self.focus == Pane::ContainerEditor {
                        self.set_focus(Pane::Arrangement);
                    }
                }
                UiAction::OpenPluginParams { .. } => self.show_plugin_params(),
                UiAction::ClosePluginParams => {
                    if self.focus == Pane::PluginParams {
                        self.set_focus(Pane::Arrangement);
                    }
                }
                UiAction::CloseSeqEditor => {
                    if self.focus == Pane::SeqEditor {
                        self.set_focus(Pane::Arrangement);
                    }
                }
                UiAction::OpenSliceEditor(_) => self.show_slice_editor(),
                UiAction::CloseSliceEditor => {
                    if self.focus == Pane::SliceEditor {
                        self.set_focus(Pane::Arrangement);
                    }
                }
                UiAction::Browse(BrowseAction::LayoutChanged) => {
                    // Keep what the window could show, not what the pointer asked for.
                    if let Some(fb) = self.layout().file_browser {
                        self.core.file_browser_mut().set_width(fb.width());
                    }
                    self.save_ui_state();
                }
                UiAction::Browse(BrowseAction::SetAutoPreview(_)) => self.save_ui_state(),
                UiAction::Browse(BrowseAction::DragFiles(paths)) => {
                    self.file_drag = Some(FileDrag {
                        paths: paths.clone(),
                        pos: None,
                        cancelled: false,
                    });
                    self.capture = Some((WindowKind::Main, Capture::FileDrag));
                }
                UiAction::Browse(BrowseAction::Show(Place::Folder(folder))) => {
                    self.ui.file_browser_folder = Some(folder.clone());
                    self.save_ui_state();
                }
                UiAction::RunCommand { id, arg } => {
                    if let Some(mut requests) = self.run_command(id, arg.as_deref()) {
                        self.pending.append(&mut requests);
                    }
                }
                UiAction::LoadSamplesDialog { device } => {
                    self.load_device = Some(device.clone());
                    self.pending.push(ShellRequest::PickLoadSamples);
                }
                _ => {}
            }
        }
    }

    /// The sample editor was opened (the core did it): make sure the lower pane shows it and
    /// give it the keys.
    fn show_sample_editor(&mut self) {
        if self.core.sample_editor_device().is_none() {
            return;
        }
        if !self.ui.show_song || self.lower_is_console() {
            self.ui.show_song = true;
            self.ui.show_console = false;
            self.console_changed();
        }
        self.layout_view();
        self.set_focus(Pane::SampleEditor);
    }

    /// The step editor was opened (the core did it): make sure the lower pane shows it and
    /// give it the keys.
    fn show_step_editor(&mut self) {
        if self.core.step_editor_device().is_none() {
            return;
        }
        if !self.ui.show_song || self.lower_is_console() {
            self.ui.show_song = true;
            self.ui.show_console = false;
            self.console_changed();
        }
        self.layout_view();
        self.set_focus(Pane::StepEditor);
    }

    /// The sequencer grid editor was opened (the core did it): make sure the lower pane shows it
    /// and give it the keys.
    fn show_seq_editor(&mut self) {
        if self.core.seq_editor_device().is_none() {
            return;
        }
        if !self.ui.show_song || self.lower_is_console() {
            self.ui.show_song = true;
            self.ui.show_console = false;
            self.console_changed();
        }
        self.layout_view();
        self.set_focus(Pane::SeqEditor);
    }

    /// The container editor was opened (the core did it): make sure the lower pane shows it and
    /// give it the keys.
    fn show_container_editor(&mut self) {
        if self.core.container_editor_device().is_none() {
            return;
        }
        if !self.ui.show_song || self.lower_is_console() {
            self.ui.show_song = true;
            self.ui.show_console = false;
            self.console_changed();
        }
        self.layout_view();
        self.set_focus(Pane::ContainerEditor);
    }

    /// The plugin parameter pane was opened (the core did it): make sure the lower pane shows it
    /// and give it the keys.
    fn show_plugin_params(&mut self) {
        if self.core.plugin_params_device().is_none() {
            return;
        }
        if !self.ui.show_song || self.lower_is_console() {
            self.ui.show_song = true;
            self.ui.show_console = false;
            self.console_changed();
        }
        self.layout_view();
        self.set_focus(Pane::PluginParams);
    }

    /// Moves what the library asked of the core to the shell: a project to open, which waits
    /// for the answer about unsaved changes.
    fn sync_library_ui(&mut self) {
        if let Some(path) = self.core.take_open_request() {
            self.open_project_request(path);
        }
        if self.core.take_browser_focus_request() && self.ui.file_browser_visible {
            self.set_focus(Pane::FileBrowser);
        }
    }

    /// Opens the project `path` now, or asks first when the rack has unsaved changes.
    fn open_project_request(&mut self, path: PathBuf) {
        if self.core.is_dirty() {
            self.pending.push(ShellRequest::ConfirmDiscardOpen(path));
        } else {
            self.open_file(&path);
        }
    }

    /// Feeds the plugin views what the core knows and moves what the core asks for to the
    /// platform: the banner, the settings to save, the folder dialog and the modal's data.
    fn sync_plugin_ui(&mut self) {
        if let Some(change) = self.core.take_plugin_banner_change() {
            match change {
                Some(kind) => self.banner.show(kind),
                None => self.banner.hide(),
            }
            self.layout_view();
            self.repaint = true;
        }
        if let Some(plugins) = self.core.take_plugin_settings_change() {
            let session = self.core.sink_mut();
            let mut settings = session.settings().clone();
            settings.plugins = plugins;
            session.set_settings(settings.clone());
            if let Some(path) = &self.settings_path
                && let Err(e) = settings::save(path, &settings)
            {
                let message = format!("settings not saved: {e}");
                self.notify(message);
            }
        }
        if self.core.take_plugin_folder_request() {
            self.pending.push(ShellRequest::PickPluginFolder);
        }
        if self.plugin_settings.is_some() {
            let data = self.core.plugin_settings_data();
            if let Some(v) = &mut self.plugin_settings
                && v.data() != &data
            {
                v.set_data(data);
                // The plugin views have no way to say "painted": the shell repaints on change.
                self.repaint = true;
            }
        }
    }

    /// The slice editor was opened (the core did it): make sure the lower pane shows it and
    /// give it the keys.
    fn show_slice_editor(&mut self) {
        if self.core.slice_editor_device().is_none() {
            return;
        }
        if !self.ui.show_song || self.lower_is_console() {
            self.ui.show_song = true;
            self.ui.show_console = false;
            self.console_changed();
        }
        self.layout_view();
        self.set_focus(Pane::SliceEditor);
    }

    /// Gives the panel the saved width, split, auto-preview switch and folder, then shows it
    /// when it was shown last time (the core lists the folders on the first show).
    fn restore_file_browser(&mut self) {
        let ui = self.ui.clone();
        let view = self.core.file_browser_mut();
        view.set_width(ui.file_browser_width);
        view.set_tree_fraction(ui.file_browser_tree_fraction);
        view.set_auto_preview(ui.file_browser_auto_preview);
        self.core.set_file_browser_folder(ui.file_browser_folder);
        if ui.file_browser_visible {
            self.core.set_file_browser_visible(true);
        }
    }

    /// Shows or hides the file browser column: the core is told (the first show lists the
    /// folders), focus leaves a hidden panel and the layout and the saved state follow.
    fn set_file_browser_visible(&mut self, visible: bool) {
        self.ui.file_browser_visible = visible;
        self.core.set_file_browser_visible(visible);
        self.set_focus(self.live_pane(self.focus));
        self.layout_view();
        self.save_ui_state();
    }

    fn console_changed(&mut self) {
        if !self.console_visible() && self.focus == Pane::Console {
            self.set_focus(self.live_pane(Pane::Arrangement));
        }
        self.layout_view();
        self.save_ui_state();
    }

    /// Runs a menu or shortcut command. The requests include any the shell raised on the side
    /// (such as bringing a window forward).
    pub fn execute(&mut self, command: AppCommand) -> Vec<ShellRequest> {
        let modal = self.modal_open();
        let mut requests = self.execute_command(command);
        requests.append(&mut self.pending);
        self.after_event(modal, &mut requests);
        requests
    }

    fn execute_command(&mut self, command: AppCommand) -> Vec<ShellRequest> {
        self.repaint = true;
        let requests = match command {
            AppCommand::New
            | AppCommand::Open
            | AppCommand::Quit
            | AppCommand::StressRack
            | AppCommand::StressSong => {
                if self.core.is_dirty() {
                    vec![ShellRequest::ConfirmDiscard(command)]
                } else {
                    self.confirmed(command)
                }
            }
            AppCommand::Save => match self.core.save() {
                Ok(()) => Vec::new(),
                Err(AppError::NoPath) => vec![ShellRequest::PickSaveFile],
                Err(e) => {
                    self.notify(e.to_string());
                    Vec::new()
                }
            },
            AppCommand::SaveAs => vec![ShellRequest::PickSaveFile],
            AppCommand::ExportPatch => vec![ShellRequest::PickExportFile],
            AppCommand::ImportAudio => vec![ShellRequest::PickImportFiles],
            AppCommand::ImportMidi => vec![ShellRequest::PickImportMidi { as_song: false }],
            AppCommand::ImportMidiSong => vec![ShellRequest::PickImportMidi { as_song: true }],
            AppCommand::ExportMidiClip => self.export_midi_request(MidiExport::Clip),
            AppCommand::ExportMidiTrack => self.export_midi_request(MidiExport::Track),
            AppCommand::ExportMidiSong => self.export_midi_request(MidiExport::Song),
            AppCommand::ExportSongAudio => self.export_audio_request(ExportScope::Song),
            AppCommand::ExportLoopAudio => self.export_audio_request(ExportScope::Loop),
            AppCommand::QuantizeApply => {
                let pane = self.live_pane(self.focus);
                self.apply_last_quantize(pane);
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::QuantizeOptions => {
                self.open_quantize();
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            // The Track menu: the arrangement's commands, whatever pane has the focus.
            AppCommand::GroupTracks
            | AppCommand::UngroupTracks
            | AppCommand::ToggleFolder
            | AppCommand::RenameFolder
            | AppCommand::MoveTrackUp
            | AppCommand::MoveTrackDown
            | AppCommand::RenameTrack
            | AppCommand::TrackComment
            | AppCommand::AddNoteLane
            | AppCommand::RenameNoteLane
            | AppCommand::DeleteNoteLane
            | AppCommand::ToggleLaneMute
            | AppCommand::FoldLanes(_)
            | AppCommand::TrackHeight(_) => {
                self.track_command(command);
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            // The freeze commands render on a worker and edit the document when it is done.
            AppCommand::FreezeTrack
            | AppCommand::FreezeTrackToDevice
            | AppCommand::FreezeTail
            | AppCommand::FreezeCancel
            | AppCommand::UnfreezeTrack
            | AppCommand::UnfreezeTrackKeep
            | AppCommand::BounceInPlace
            | AppCommand::BounceInPlaceMute => {
                match command {
                    AppCommand::FreezeTrack => self.core.freeze_tracks(FreezeThrough::Track),
                    AppCommand::FreezeTrackToDevice => {
                        self.core.freeze_tracks(FreezeThrough::SelectedDevice);
                    }
                    AppCommand::FreezeTail => self.core.toggle_freeze_tail(),
                    AppCommand::FreezeCancel => self.core.cancel_freeze(),
                    AppCommand::UnfreezeTrack => self.core.unfreeze_tracks(false),
                    AppCommand::UnfreezeTrackKeep => self.core.unfreeze_tracks(true),
                    AppCommand::BounceInPlace => self.core.bounce_in_place(false),
                    _ => self.core.bounce_in_place(true),
                }
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::AudioReverse
            | AppCommand::AudioWarp
            | AppCommand::AudioWarpMode(_)
            | AppCommand::AudioTranspose(_)
            | AppCommand::AudioGain(_)
            | AppCommand::AudioFadeShape(_)
            | AppCommand::AudioCrossfade(_)
            | AppCommand::AudioFades(_)
            | AppCommand::AudioClearEnvelope => {
                self.audio_command(command);
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::AudioNormalise => {
                self.core.normalise_audio();
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::AudioStripSilence => {
                self.open_silence();
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::AudioToNotes => {
                self.open_notes();
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::AudioQuantize => {
                self.quantize_audio();
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::AudioWarpTempo => {
                self.core.warp_to_song_tempo();
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::AudioWarpBars(bars) => {
                self.core.warp_bars(bars);
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::AudioWarpPanel => {
                self.open_warp();
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::RelinkAudio => {
                self.open_relink();
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::Undo => {
                self.core.undo();
                Vec::new()
            }
            AppCommand::Redo => {
                self.core.redo();
                Vec::new()
            }
            // The Edit menu: the same commands the keys run, offered to the focused pane.
            AppCommand::Cut
            | AppCommand::Copy
            | AppCommand::Paste
            | AppCommand::Duplicate
            | AppCommand::Delete
            | AppCommand::SelectAll => {
                if let Some((id, arg)) = command.command_id() {
                    let pane = self.live_pane(self.focus);
                    if let Some(actions) = self.core.pane_command(pane, id, arg) {
                        self.pane_actions(&actions);
                    }
                }
                Vec::new()
            }
            AppCommand::ToggleFlip => {
                let view = self.core.view_mut();
                let flip = match view.flip() {
                    Flip::Front => Flip::Back,
                    Flip::Back => Flip::Front,
                };
                view.set_flip(flip);
                Vec::new()
            }
            AppCommand::Cables(v) => {
                self.core.view_mut().set_cable_visibility(v);
                Vec::new()
            }
            AppCommand::ZoomIn | AppCommand::ZoomOut | AppCommand::ZoomReset => {
                let size = self.view_size();
                let view = self.core.view_mut();
                let zoom = match command {
                    AppCommand::ZoomIn => view.zoom() * ZOOM_FACTOR,
                    AppCommand::ZoomOut => view.zoom() / ZOOM_FACTOR,
                    _ => 1.0,
                };
                view.zoom_at(zoom, Point::new(size.width / 2.0, 0.0));
                Vec::new()
            }
            AppCommand::ToggleBrowser => {
                let view = self.core.view_mut();
                let visible = !view.browser_visible();
                view.set_browser_visible(visible);
                Vec::new()
            }
            AppCommand::ToggleFileBrowser => {
                self.set_file_browser_visible(!self.ui.file_browser_visible);
                Vec::new()
            }
            AppCommand::ToggleSong => {
                // With the rack in its own window the song area is all the main window has.
                self.ui.show_song = !self.ui.show_song || self.rack_detached();
                if !self.ui.show_song {
                    self.set_focus(self.live_pane(self.focus));
                }
                self.layout_view();
                self.save_ui_state();
                Vec::new()
            }
            AppCommand::TogglePianoRoll => {
                if self.core.piano_roll_clip().is_some() {
                    self.core.close_piano_roll();
                } else if self.core.open_piano_roll_default() {
                    if !self.ui.show_song || self.lower_is_console() {
                        self.ui.show_song = true;
                        self.ui.show_console = false;
                        self.console_changed();
                    }
                } else {
                    self.notify("no note clip to open: draw or record one in the arrangement");
                }
                Vec::new()
            }
            AppCommand::CreateInstrument => {
                self.open_create(CreateKind::Instrument);
                Vec::new()
            }
            AppCommand::CreateEffect => {
                self.open_create(CreateKind::Effect);
                Vec::new()
            }
            AppCommand::CreateAudioTrack => {
                self.core.add_audio_track();
                Vec::new()
            }
            AppCommand::CreateMixerChannel => self.execute_command(AppCommand::AddChannel),
            // With the rack in a window of its own F6 raises that window; the F6 choice is the
            // main window's layout, which the rack no longer takes part in.
            AppCommand::ToggleRack if self.rack_detached() => {
                self.set_focus(Pane::Rack);
                vec![ShellRequest::FocusWindow(WindowKind::Rack)]
            }
            AppCommand::ToggleRack => {
                self.set_rack_visible(!self.workflow.rack_visible);
                Vec::new()
            }
            // F7 with a pane detached has no layout to change: the main window comes forward
            // on the arrangement.
            AppCommand::SequencerOnly if self.rack_detached() || self.console_detached() => {
                self.set_focus(self.live_pane(Pane::Arrangement));
                vec![ShellRequest::FocusWindow(WindowKind::Main)]
            }
            AppCommand::SequencerOnly => {
                self.toggle_sequencer_only();
                Vec::new()
            }
            AppCommand::DetachRack => {
                self.set_detached(WindowKind::Rack, !self.rack_detached());
                Vec::new()
            }
            AppCommand::DetachConsole => {
                self.set_detached(WindowKind::Console, !self.console_detached());
                Vec::new()
            }
            AppCommand::ToggleEditor => {
                self.toggle_editor();
                Vec::new()
            }
            AppCommand::ToggleSampleEditor => {
                if self.core.sample_editor_device().is_some() {
                    self.core.close_sample_editor();
                    self.set_focus(Pane::Arrangement);
                } else if self.core.open_sample_editor_default() {
                    self.show_sample_editor();
                } else {
                    self.notify("no sampler to open: add one from the device browser");
                }
                Vec::new()
            }
            AppCommand::ToggleStepEditor => {
                if self.core.step_editor_device().is_some() {
                    self.core.close_step_editor();
                    self.set_focus(Pane::Arrangement);
                } else if self.core.open_step_editor_default() {
                    self.show_step_editor();
                } else {
                    self.notify("no drum machine to open: add one from the device browser");
                }
                Vec::new()
            }
            AppCommand::ToggleSliceEditor => {
                if self.core.slice_editor_device().is_some() {
                    self.core.close_slice_editor();
                    self.set_focus(Pane::Arrangement);
                } else if self.core.open_slice_editor_default() {
                    self.show_slice_editor();
                } else {
                    self.notify("no loop slicer to open: add one from the device browser");
                }
                Vec::new()
            }
            AppCommand::Transport(action) => {
                self.core.transport_action(action);
                Vec::new()
            }
            AppCommand::Quantize(q) => {
                self.core.set_input_quantize(q);
                self.ui.input_quantize = q;
                self.save_ui_state();
                Vec::new()
            }
            AppCommand::ToggleConsoleDevices => {
                let view = self.core.view_mut();
                let show = !view.show_console_devices();
                view.set_show_console_devices(show);
                self.ui.show_console_devices = show;
                self.save_ui_state();
                Vec::new()
            }
            AppCommand::ToggleConsoleLinks => {
                let view = self.core.view_mut();
                let show = !view.show_console_links();
                view.set_show_console_links(show);
                self.ui.show_console_links = show;
                self.save_ui_state();
                Vec::new()
            }
            AppCommand::ToggleTempoLane => {
                let show = !self.core.arrangement().tempo_lane_visible();
                self.core.arrangement_mut().set_tempo_lane(show);
                self.ui.show_tempo_lane = show;
                self.save_ui_state();
                Vec::new()
            }
            AppCommand::ToggleMarkerStrip => {
                let show = !self.core.arrangement().marker_strip_visible();
                self.core.arrangement_mut().set_marker_strip(show);
                self.ui.show_marker_strip = show;
                self.save_ui_state();
                Vec::new()
            }
            AppCommand::ToggleMarkerList => {
                let show = !self.core.arrangement().marker_list_visible();
                self.core.arrangement_mut().set_marker_list(show);
                self.ui.show_marker_list = show;
                self.save_ui_state();
                Vec::new()
            }
            // The marker commands live in the arrangement view: the menu runs them there, so
            // a menu item and a key do the same thing wherever the focus is.
            AppCommand::AddMarker
            | AppCommand::AddRangeFromLoop
            | AppCommand::SetEndMarker
            | AppCommand::SetLeftLocator
            | AppCommand::SetRightLocator
            | AppCommand::PreviousMarker
            | AppCommand::NextMarker => {
                if let Some((id, arg)) = command.command_id()
                    && let Some(actions) = self.core.pane_command(Pane::Arrangement, id, arg)
                {
                    self.pane_actions(&actions);
                }
                Vec::new()
            }
            AppCommand::ToggleRackNavigator => {
                let view = self.core.view_mut();
                let show = !view.navigator_visible();
                view.set_navigator_visible(show);
                self.ui.show_rack_navigator = show;
                self.save_ui_state();
                Vec::new()
            }
            AppCommand::ToggleConsole if self.console_detached() => {
                self.set_focus(Pane::Console);
                vec![ShellRequest::FocusWindow(WindowKind::Console)]
            }
            AppCommand::ToggleConsole => {
                if self.console_in_main() {
                    self.ui.show_console = false;
                    self.ui.console_maximised = false;
                } else {
                    self.ui.show_song = true;
                    self.ui.show_console = true;
                    self.set_focus(Pane::Console);
                }
                self.console_changed();
                Vec::new()
            }
            AppCommand::MaximiseConsole if self.console_detached() => {
                vec![ShellRequest::ToggleMaximise(WindowKind::Console)]
            }
            AppCommand::MaximiseConsole => {
                if self.maximised() {
                    self.ui.console_maximised = false;
                } else {
                    self.ui.show_song = true;
                    self.ui.show_console = true;
                    self.ui.console_maximised = true;
                    self.set_focus(Pane::Console);
                }
                self.console_changed();
                Vec::new()
            }
            AppCommand::AddChannel => {
                if !self.console_visible() {
                    self.execute_command(AppCommand::ToggleConsole);
                } else if self.console_detached() {
                    self.pending
                        .push(ShellRequest::FocusWindow(WindowKind::Console));
                }
                // The console's own "+ CHANNEL" picker (instruments without a channel).
                if let Some(p) = self.core.console().button_center(ConsoleButton::AddChannel) {
                    let (button, modifiers) = (PointerButton::Primary, atomik_ui::Modifiers::NONE);
                    self.forward(
                        Pane::Console,
                        &UiEvent::PointerDown {
                            pos: p,
                            button,
                            modifiers,
                            clicks: 1,
                        },
                    );
                    self.forward(
                        Pane::Console,
                        &UiEvent::PointerUp {
                            pos: p,
                            button,
                            modifiers,
                        },
                    );
                }
                Vec::new()
            }
            AppCommand::RouteToNewBus => {
                // A refusal is already reported (rack and console).
                let console_focus = self.live_pane(self.focus) == Pane::Console;
                self.core.route_selection_to_new_bus(console_focus);
                Vec::new()
            }
            AppCommand::CreateVca => {
                // A refusal is already reported (rack and console).
                self.core
                    .create_vca_from_selection(self.live_pane(self.focus));
                Vec::new()
            }
            AppCommand::AddToVca(id) => {
                self.core
                    .add_selection_to_vca(self.live_pane(self.focus), id);
                Vec::new()
            }
            AppCommand::RemoveFromVca => {
                self.core
                    .remove_selection_from_vca(self.live_pane(self.focus));
                Vec::new()
            }
            AppCommand::DeleteVca(id) => {
                self.core.delete_vca(self.live_pane(self.focus), id);
                Vec::new()
            }
            AppCommand::StoreSnapshot => {
                self.core.store_snapshot(None);
                Vec::new()
            }
            AppCommand::UpdateSnapshot(id) => {
                self.core.update_snapshot(id);
                Vec::new()
            }
            AppCommand::RecallSnapshot(id) => {
                self.core.recall_snapshot(id);
                Vec::new()
            }
            AppCommand::RecallNextSnapshot => {
                self.core.recall_adjacent(true);
                Vec::new()
            }
            AppCommand::RecallPreviousSnapshot => {
                self.core.recall_adjacent(false);
                Vec::new()
            }
            AppCommand::CompareSnapshot(id) => {
                self.core.compare_snapshot(id);
                Vec::new()
            }
            AppCommand::EndCompare => {
                self.core.end_compare();
                Vec::new()
            }
            AppCommand::DeleteSnapshot(id) => {
                self.core.delete_snapshot(id);
                Vec::new()
            }
            AppCommand::TogglePolarity => {
                self.core.toggle_polarity(self.live_pane(self.focus));
                Vec::new()
            }
            AppCommand::BypassAllInserts => {
                self.core.bypass_all_inserts(self.live_pane(self.focus));
                Vec::new()
            }
            AppCommand::ToggleCompensation => {
                let on = self.core.doc().console.compensation;
                let command = Command::Console(ConsoleCommand::SetCompensation(!on));
                // A rejection is already reported in the view.
                let _ = self.core.apply_command(command);
                Vec::new()
            }
            AppCommand::MidiLearn => {
                self.core.toggle_midi_learn();
                Vec::new()
            }
            AppCommand::MidiMappings => {
                self.open_midi_maps();
                Vec::new()
            }
            AppCommand::MacroPanel => {
                self.open_macros();
                Vec::new()
            }
            AppCommand::NewMacroBank => {
                if self.core.new_macro_bank() {
                    self.notify("Added a macro bank at the end of the rack");
                }
                Vec::new()
            }
            AppCommand::FindParameter => {
                self.open_param_search();
                Vec::new()
            }
            AppCommand::FollowSelection => {
                self.workflow.follow_selection = !self.workflow.follow_selection;
                self.save_workflow();
                self.apply_workflow_to_views();
                Vec::new()
            }
            AppCommand::ShowParamLane => {
                if self.core.show_touched_lane() {
                    self.show_arrangement_focus();
                }
                Vec::new()
            }
            AppCommand::ExportMidiPreset => {
                if self.core.doc().midi_maps.is_empty() {
                    self.notify("There are no MIDI mappings to export");
                    Vec::new()
                } else {
                    vec![ShellRequest::PickExportMidiPreset]
                }
            }
            AppCommand::ImportMidiPreset => vec![ShellRequest::PickImportMidiPreset],
            AppCommand::ClearMidiMaps => {
                if !self.core.clear_midi_maps() {
                    self.notify("There are no MIDI mappings to clear");
                }
                Vec::new()
            }
            AppCommand::AudioSettings => {
                let session = self.core.sink();
                let mut lists = DeviceLists::query(session.backend());
                lists.input_note = crate::status::input_note(session.status());
                let dialog = SettingsDialog::new(session.settings().clone(), lists)
                    .with_preferences(
                        KeymapEditor::new(self.os, self.default_profile, self.user_keymap.clone()),
                        self.workflow.clone(),
                    );
                self.dialog = Some(dialog);
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::PluginSettings => {
                let theme = self.core.view().theme().clone();
                let mut view = PluginSettingsView::new(theme);
                view.set_scale(self.scale());
                view.set_bounds(self.plugin_modal_rect());
                view.set_data(self.core.plugin_settings_data());
                view.set_focused(true);
                self.plugin_settings = Some(view);
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::RestartAudio => {
                self.core.sink_mut().set_enabled(true);
                self.core.rebuild_engine();
                self.poll_status();
                Vec::new()
            }
        };
        self.core.after_view_change();
        requests
    }

    /// Runs a command after the user agreed to discard unsaved changes.
    pub fn confirmed(&mut self, command: AppCommand) -> Vec<ShellRequest> {
        self.repaint = true;
        match command {
            AppCommand::New => {
                self.core.new_document();
                Vec::new()
            }
            AppCommand::Open => vec![ShellRequest::PickOpenFile],
            AppCommand::Quit => vec![ShellRequest::Quit],
            AppCommand::StressRack => {
                let doc = stress_doc(self.core.registry(), STRESS_DEVICES, STRESS_CABLES);
                self.core.open_doc(doc);
                Vec::new()
            }
            AppCommand::StressSong => {
                let doc = stress_song_doc(self.core.registry());
                self.core.open_doc(doc);
                Vec::new()
            }
            other => self.execute_command(other),
        }
    }

    /// Opens a project or patch file (errors are shown in the view).
    pub fn open_file(&mut self, path: &Path) {
        if let Err(e) = self.core.open(path) {
            self.notify(e.to_string());
        }
        self.repaint = true;
    }

    /// Saves the project to `path` (errors are shown in the view).
    pub fn save_to(&mut self, path: &Path) {
        if let Err(e) = self.core.save_as(path) {
            self.notify(e.to_string());
        }
        self.repaint = true;
    }

    /// Exports the rack as a patch v2 file to `path` (errors are shown in the view).
    pub fn export_to(&mut self, path: &Path) {
        if let Err(e) = self.core.export_patch(path) {
            self.notify(e.to_string());
        }
        self.repaint = true;
    }

    /// Imports the MIDI file picked in the Import MIDI dialog (see [`AppCore::import_midi`]).
    pub fn import_midi(&mut self, path: &Path, as_song: bool) {
        self.core.import_midi(path, as_song);
        self.repaint = true;
    }

    /// Writes the MIDI file for `what` to `path` (the answer is shown in the view).
    pub fn export_midi_to(&mut self, what: MidiExport, path: &Path) {
        self.core.export_midi(what, path);
        self.repaint = true;
    }

    /// The file name (without extension) the Export MIDI dialog offers.
    pub fn midi_export_name(&self, what: MidiExport) -> String {
        self.core.midi_export_name(what)
    }

    /// File > Export MIDI: the save dialog, or what to select first when there is nothing to
    /// write.
    fn export_midi_request(&mut self, what: MidiExport) -> Vec<ShellRequest> {
        if self.core.can_export_midi(what) {
            vec![ShellRequest::PickExportMidi(what)]
        } else {
            self.notify(what.needs());
            Vec::new()
        }
    }

    /// Shows a message in the view until the next click.
    pub fn notify(&mut self, message: impl Into<String>) {
        self.core.report(message);
    }

    /// Per-frame work: engine garbage, the transport position and recorded notes.
    pub fn frame(&mut self) {
        let busy = self.core.is_busy();
        self.core.frame();
        self.sync_plugin_ui();
        self.sync_library_ui();
        self.sync_relink();
        self.sync_export();
        self.sync_midi_maps();
        self.sync_macros();
        self.sync_param_search();
        self.sync_quantize();
        // Imports and loads finish between frames: keep painting until the last one landed.
        if busy || self.core.is_busy() {
            self.repaint = true;
        }
    }

    /// Before the app exits: the last folder and the panel's values go to `ui-state.json`
    /// and the library (favourites, tags, bookmarks) is written if it changed.
    pub fn shutdown(&mut self) {
        if let Some(folder) = self.core.file_browser_folder() {
            self.ui.file_browser_folder = Some(folder);
        }
        self.save_ui_state();
        self.core.flush_library();
    }

    /// True while audio is being imported or loaded (the platform loop keeps ticking).
    pub fn is_busy(&self) -> bool {
        self.core.is_busy()
    }

    /// [`Self::frame`], then (while the console shows) the console meters and ballistics at
    /// `now_seconds` (any monotonic clock).
    pub fn frame_at(&mut self, now_seconds: f64) {
        self.frame();
        // A device dragged to an edge of the rack scrolls it.
        if self.core.view_mut().tick(now_seconds) {
            self.repaint = true;
        }
        self.core.plugin_tick(now_seconds);
        self.core.piano_roll_tick(now_seconds);
        self.tick_tooltip(now_seconds);
        // A pointer resting on another row of a context menu switches its submenu.
        if let Some(open) = &mut self.context_menu
            && open.menu.tick(now_seconds)
        {
            self.repaint = true;
        }
        // The control path: MIDI mappings and the control surface.
        let heard = self.core.midi_received();
        self.core.midi_control_frame(now_seconds);
        self.core.surface_frame(now_seconds);
        if self.core.midi_received() != heard {
            self.repaint = true;
        }
        if self.console_visible() {
            self.core.update_console_meters(now_seconds);
        }
        self.core.feed_rec_input(now_seconds);
        if self.core.view().flip() == atomik_ui::Flip::Back {
            self.core.feed_cv_activity(now_seconds);
        }
        self.flush_geometry(now_seconds);
        self.fold_repaint();
    }

    /// Opens the control surface the settings name (its driver and ports), or closes it. Nothing
    /// happens when the settings are the ones it already runs with. A port that cannot be opened
    /// is reported, and a surface with no open port is not set up.
    fn configure_surface(&mut self) {
        let wanted = self.core.sink().settings().surface.clone();
        if wanted == self.surface_applied {
            return;
        }
        self.surface_applied = wanted.clone();
        let Some(id) = wanted.driver.as_deref() else {
            self.core.set_surface(None);
            return;
        };
        let Some(driver) = atomik_midi_io::surface::driver_by_id(id) else {
            self.core.set_surface(None);
            self.notify(format!("unknown control surface {id:?}"));
            return;
        };
        let (link, problems) =
            crate::surface::SystemLink::open(wanted.input.as_deref(), wanted.output.as_deref());
        if let Some(problem) = problems.first() {
            self.notify(problem.clone());
        }
        if link.is_open() {
            self.core.set_surface(Some(crate::surface::SurfaceHost::new(
                driver,
                Box::new(link),
            )));
        } else {
            self.core.set_surface(None);
        }
    }

    fn apply_settings(&mut self, settings: AudioSettings) {
        let gain = settings.metronome_gain;
        let session = self.core.sink_mut();
        if session.set_settings(settings.clone()) == SettingsChange::RestartAudio {
            session.set_enabled(true);
            self.core.rebuild_engine();
        }
        self.configure_surface();
        if (gain - self.core.metronome_gain()).abs() > f32::EPSILON {
            self.core.set_metronome_gain(gain);
        }
        if let Some(path) = &self.settings_path
            && let Err(e) = settings::save(path, &settings)
        {
            let message = format!("settings not saved: {e}");
            self.notify(message);
        }
        self.poll_status();
    }

    /// Reads audio telemetry into the status bar; true when its text changed.
    pub fn poll_status(&mut self) -> bool {
        self.reload_keymap_file();
        let target = self.core.notes_target_device().map(|d| d.name.clone());
        let mut status = self.core.sink_mut().poll().clone();
        status.notes_target = target;
        status.streaming = self.core.song_assets().values().any(|a| a.is_stream());
        let mut text = status_text(&status);
        if let Some(alert) = self.core.pdc_alert() {
            text = format!("{alert} \u{b7} {text}");
        }
        if let Some(progress) = self.core.import_progress() {
            text = format!("{progress} \u{b7} {text}");
        }
        if let Some(loading) = self.core.plugin_loading_text() {
            text = format!("{loading} \u{b7} {text}");
        }
        if let Some(progress) = self.core.freeze_progress() {
            text = format!("{progress} \u{b7} {text}");
        }
        if let Some(learn) = self.core.midi_status() {
            text = format!("{learn} \u{b7} {text}");
        }
        let changed = text != self.status;
        if changed {
            self.status = text;
            self.repaint = true;
        }
        changed
    }

    /// The mouse cursor to show: the piano roll's or the arrangement's while it is the visible
    /// song pane (its hover is cleared by the `PointerLeave` the shell sends when the pointer
    /// moves to another pane), the arrow elsewhere.
    pub fn cursor(&self) -> CursorHint {
        self.cursor_in(WindowKind::Main)
    }

    /// The mouse cursor to show in `window`: the detached panes ask for the arrow.
    pub fn cursor_in(&self, window: WindowKind) -> CursorHint {
        if window != WindowKind::Main || !self.ui.show_song {
            return CursorHint::Default;
        }
        match self.song_pane() {
            Pane::PianoRoll => self.core.piano_roll().cursor(),
            Pane::Arrangement => self.core.arrangement().cursor(),
            _ => CursorHint::Default,
        }
    }

    /// Whether anything must be repainted.
    pub fn needs_repaint(&self) -> bool {
        self.needs_repaint_in(WindowKind::Main)
    }

    /// The open windows that have something to paint, main window first, then the rack and the
    /// console: always in that order, whichever window has the focus. The platform paints all
    /// of them each tick, so a window that animates is never starved by another.
    pub fn windows_to_paint(&self) -> Vec<WindowKind> {
        self.wins
            .keys()
            .copied()
            .filter(|&w| self.needs_repaint_in(w))
            .collect()
    }

    /// Whether anything in `window` must be repainted (`false` for a window that is not open).
    /// A change anywhere marks every open window; painting one window clears only its own mark,
    /// and a view that animates (the console meters) keeps only its own window painting.
    pub fn needs_repaint_in(&self, window: WindowKind) -> bool {
        let Some(win) = self.wins.get(&window) else {
            return false;
        };
        match window {
            WindowKind::Rack => {
                return self.repaint || win.dirty || self.core.view().needs_repaint();
            }
            WindowKind::Console => {
                return self.repaint || win.dirty || self.core.console().needs_repaint();
            }
            WindowKind::Main => {}
        }
        self.repaint
            || win.dirty
            || (!self.rack_detached() && self.core.view().needs_repaint())
            || self.core.transport_bar().needs_repaint()
            || (self.ui.file_browser_visible && self.core.file_browser().needs_repaint())
            || (self.ui.show_song
                && match self.song_pane() {
                    Pane::PianoRoll => self.core.piano_roll().needs_repaint(),
                    Pane::SampleEditor => self.core.sample_editor().needs_repaint(),
                    Pane::StepEditor => self.core.step_editor().needs_repaint(),
                    Pane::SliceEditor => self.core.slice_editor().needs_repaint(),
                    Pane::SeqEditor => self.core.seq_editor().needs_repaint(),
                    Pane::ContainerEditor => self.core.container_editor().needs_repaint(),
                    Pane::PluginParams => self.core.plugin_params().needs_repaint(),
                    Pane::Console => self.core.console().needs_repaint(),
                    _ => self.core.arrangement().needs_repaint(),
                })
    }

    /// Paints the whole main window content in physical pixels.
    pub fn paint(&mut self, canvas: &mut dyn Canvas) {
        self.paint_in(WindowKind::Main, canvas);
    }

    /// Paints the content of `window` in physical pixels (nothing for a window that is not open).
    /// A detached pane fills its window; the main window leaves the detached panes out.
    pub fn paint_in(&mut self, window: WindowKind, canvas: &mut dyn Canvas) {
        self.fold_repaint();
        if !self.wins.contains_key(&window) {
            return;
        }
        match window {
            WindowKind::Main => self.paint_main(canvas),
            WindowKind::Rack | WindowKind::Console => self.paint_detached(window, canvas),
        }
        self.mark_painted(window);
    }

    fn paint_main(&mut self, canvas: &mut dyn Canvas) {
        let scale = self.scale();
        let l = self.layout();
        let (w, h) = (self.size().width, self.size().height);
        if l.rack.height() > 0.0 {
            canvas.push_clip(
                &Rect::new(
                    l.rack.x0 * scale,
                    l.rack.y0 * scale,
                    l.rack.x1 * scale,
                    l.rack.y1 * scale,
                )
                .into(),
            );
            canvas.push_transform(Affine::translate((l.rack.x0 * scale, l.rack.y0 * scale)));
            self.core.paint_pane(Pane::Rack, canvas);
            canvas.pop_transform();
            canvas.pop_clip();
        }
        self.core.paint_pane(Pane::Transport, canvas);
        if l.file_browser.is_some() {
            self.core.paint_pane(Pane::FileBrowser, canvas);
        }
        if l.song.is_some() {
            let pane = self.song_pane();
            self.core.paint_pane(pane, canvas);
        }
        // The plugin views scale themselves (physical pixels), like the panes above.
        self.banner.paint(canvas);
        if let Some(view) = &self.plugin_settings {
            view.paint(canvas);
        }
        if let Some(view) = &self.relink {
            view.paint(canvas);
        }
        if let Some(view) = &self.export {
            view.paint(canvas);
        }
        if let Some(view) = &self.midi_maps {
            view.paint(canvas);
        }
        if let Some(view) = &self.macros_panel {
            view.paint(canvas);
        }
        if let Some(view) = &self.param_search {
            view.paint(canvas);
        }
        if let Some(view) = &self.quantize {
            view.paint(canvas);
        }
        if let Some(view) = &self.silence {
            view.paint(canvas);
        }
        if let Some(view) = &self.notes {
            view.paint(canvas);
        }
        if let Some(view) = &self.warp {
            view.paint(canvas);
        }
        let theme = self.core.view().theme().clone();
        canvas.push_transform(Affine::scale(scale));
        if let Some(d) = l.divider {
            canvas.fill_with(d, theme.browser_bg);
            canvas.fill_with(Rect::new(d.x0, d.y0, d.x1, d.y0 + 1.0), theme.section_line);
            canvas.fill_with(Rect::new(d.x0, d.y1 - 1.0, d.x1, d.y1), theme.section_line);
            let c = d.center();
            canvas.fill_with(
                Rect::new(c.x - 18.0, c.y - 0.5, c.x + 18.0, c.y + 0.5),
                theme.text_muted,
            );
        }
        let status = Rect::new(0.0, h - STATUS_BAR_HEIGHT, w, h);
        canvas.fill_with(status, theme.browser_bg);
        canvas.fill_with(
            Rect::new(0.0, status.y0, w, status.y0 + 1.0),
            theme.section_line,
        );
        canvas.text(&TextRun::new(
            self.status.clone(),
            Point::new(10.0, h - 6.5),
            12.0,
            theme.text_muted,
        ));
        if let Some(menubar) = &self.menubar {
            let model = self.menu_model();
            menubar.paint(canvas, &theme, &model, w);
        }
        self.paint_overlays(WindowKind::Main, canvas, &theme, Size::new(w, h));
        if let Some(dialog) = &self.dialog {
            dialog.paint(canvas, &theme, self.size());
        }
        canvas.pop_transform();
    }
}

/// Zoom factor of View ▸ Zoom In / Out.
pub const ZOOM_FACTOR: f64 = 1.25;

/// The event as the views want it: a numpad key becomes the character it types (digits, `.`,
/// `+`, `-`, `*`, `/`) or Enter, so numeric entry and text fields work from the numpad.
fn text_form(event: &UiEvent) -> UiEvent {
    let UiEvent::Key {
        key: Key::Numpad(n),
        modifiers,
        pressed,
    } = event
    else {
        return event.clone();
    };
    let key = match n {
        NumpadKey::Digit(d) => Key::Char(char::from(b'0' + (*d).min(9))),
        NumpadKey::Enter => Key::Enter,
        NumpadKey::Add => Key::Char('+'),
        NumpadKey::Subtract => Key::Char('-'),
        NumpadKey::Multiply => Key::Char('*'),
        NumpadKey::Divide => Key::Char('/'),
        NumpadKey::Decimal => Key::Char('.'),
    };
    UiEvent::Key {
        key,
        modifiers: *modifiers,
        pressed: *pressed,
    }
}

/// The key the file browser panel reads for one of its `files.*` commands: the shell hands the
/// command to the panel as this key (so a user remap of the chord still drives the panel).
/// `None` for an id that is not a panel command.
fn files_key(id: &str, os: Os) -> Option<(Key, Modifiers)> {
    let command = if os == Os::Mac {
        Modifiers {
            meta: true,
            ..Modifiers::NONE
        }
    } else {
        Modifiers::CTRL
    };
    Some(match id {
        "files.up" => (Key::Up, Modifiers::NONE),
        "files.down" => (Key::Down, Modifiers::NONE),
        "files.left" => (Key::Left, Modifiers::NONE),
        "files.right" => (Key::Right, Modifiers::NONE),
        "files.page_up" => (Key::PageUp, Modifiers::NONE),
        "files.page_down" => (Key::PageDown, Modifiers::NONE),
        "files.home" => (Key::Home, Modifiers::NONE),
        "files.end" => (Key::End, Modifiers::NONE),
        "files.open" => (Key::Enter, Modifiers::NONE),
        "files.stop" => (Key::Escape, Modifiers::NONE),
        "files.toggle_preview" => (Key::Space, Modifiers::NONE),
        "files.parent" => (Key::Backspace, Modifiers::NONE),
        "files.favorite" => (Key::Char('f'), Modifiers::NONE),
        "files.rename" => (Key::F(2), Modifiers::NONE),
        "files.search" => (Key::Char('f'), command),
        "files.select_all" => (Key::Char('a'), command),
        _ => return None,
    })
}

/// What the drag label says: the file's name, or how many files.
fn drag_label(paths: &[PathBuf]) -> String {
    match paths {
        [one] => one.file_name().map_or_else(
            || one.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        ),
        many => format!("{} files", many.len()),
    }
}

/// Whether `path` is a library item the rack or the arrangement takes: a device row, a patch,
/// a container or a clip. A project is not (it opens).
fn is_library_item(path: &Path) -> bool {
    atomik_library::device_type_of(path).is_some()
        || matches!(
            atomik_library::classify(path, false),
            Some(
                atomik_library::FileKind::Patch
                    | atomik_library::FileKind::Container
                    | atomik_library::FileKind::Clip
            )
        )
}

/// Whether `path` is a note clip item.
fn is_clip_item(path: &Path) -> bool {
    atomik_library::classify(path, false) == Some(atomik_library::FileKind::Clip)
}

/// Whether `path` is a project file (`.atomik`).
fn is_project_item(path: &Path) -> bool {
    atomik_library::classify(path, false) == Some(atomik_library::FileKind::Project)
}

fn pointer_pos(event: &UiEvent) -> Option<Point> {
    match event {
        UiEvent::PointerMove { pos, .. }
        | UiEvent::PointerDown { pos, .. }
        | UiEvent::PointerUp { pos, .. }
        | UiEvent::Wheel { pos, .. } => Some(*pos),
        _ => None,
    }
}

#[path = "shell_export.rs"]
mod export_impl;

#[path = "shell_windows.rs"]
mod windows_impl;
use windows_impl::{MAIN_SIZE, WinState};

#[path = "shell_tooltip.rs"]
mod tooltip_impl;
use tooltip_impl::Tips;
pub use tooltip_impl::Tooltip;

#[cfg(test)]
#[path = "shell_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "shell_tooltip_tests.rs"]
mod tooltip_tests;

#[cfg(test)]
#[path = "shell_repaint_tests.rs"]
mod repaint_tests;

#[cfg(test)]
#[path = "shell_windows_tests.rs"]
mod windows_tests;

#[cfg(test)]
#[path = "shell_song_tests.rs"]
mod song_tests;

#[cfg(test)]
#[path = "shell_audio_tests.rs"]
mod audio_tests;

#[cfg(test)]
#[path = "shell_relink_tests.rs"]
mod relink_tests;

#[cfg(test)]
#[path = "shell_quantize_tests.rs"]
mod quantize_tests;

#[cfg(test)]
#[path = "shell_midi_tests.rs"]
mod midi_tests;

#[cfg(test)]
#[path = "shell_console_tests.rs"]
mod console_tests;

#[cfg(test)]
#[path = "shell_midimap_tests.rs"]
mod midimap_tests;

#[cfg(test)]
#[path = "shell_macros_tests.rs"]
mod macros_tests;

#[cfg(test)]
#[path = "shell_key_chord_tests.rs"]
mod key_chord_tests;

#[cfg(test)]
#[path = "shell_sampler_tests.rs"]
mod sampler_tests;

#[cfg(test)]
#[path = "shell_bypass_tests.rs"]
mod bypass_tests;

#[cfg(test)]
#[path = "shell_drums_tests.rs"]
mod drums_tests;

#[cfg(test)]
#[path = "shell_slicer_tests.rs"]
mod slicer_tests;

#[cfg(test)]
#[path = "shell_players_tests.rs"]
mod players_tests;

#[cfg(test)]
#[path = "shell_plugins_tests.rs"]
mod plugins_tests;

#[cfg(test)]
#[path = "shell_keys_tests.rs"]
mod keys_tests;

#[cfg(test)]
#[path = "shell_keymap_tests.rs"]
mod keymap_tests;

#[cfg(test)]
#[path = "shell_qa_tests.rs"]
mod qa_tests;

#[cfg(test)]
#[path = "shell_workflow_tests.rs"]
mod workflow_tests;

#[cfg(test)]
#[path = "shell_clipboard_tests.rs"]
mod clipboard_tests;

#[cfg(test)]
#[path = "shell_create_tests.rs"]
mod create_tests;

#[cfg(test)]
#[path = "shell_browser_tests.rs"]
mod shell_browser_tests;

#[cfg(test)]
#[path = "shell_editor_keys_tests.rs"]
mod editor_keys_tests;

#[cfg(test)]
#[path = "shell_tempo_tests.rs"]
mod tempo_tests;

#[cfg(test)]
#[path = "shell_marker_tests.rs"]
mod marker_tests;

#[cfg(test)]
#[path = "shell_scale_tests.rs"]
mod scale_tests;

#[cfg(test)]
#[path = "shell_menu_perf_tests.rs"]
mod menu_perf_tests;

#[cfg(test)]
#[path = "shell_organise_tests.rs"]
mod organise_tests;

#[cfg(test)]
#[path = "shell_audio_edit_tests.rs"]
mod audio_edit_tests;

#[cfg(test)]
#[path = "shell_inspector_tests.rs"]
mod inspector_tests;

#[cfg(test)]
#[path = "shell_notes_tests.rs"]
mod notes_tests;

#[cfg(test)]
#[path = "shell_warp_tests.rs"]
mod warp_tests;

#[cfg(test)]
#[path = "shell_freeze_tests.rs"]
mod freeze_tests;

#[cfg(test)]
#[path = "shell_library_tests.rs"]
mod library_tests;

#[cfg(test)]
#[path = "shell_export_tests.rs"]
mod export_tests;
