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
//! # Track Panel hosting
//!
//! View ▸ Track Panel (`view.toggle_track_panel`) docks the Track Panel ([`Pane::TrackPanel`]) as
//! a column on the right of the window, between the menu bar and the Piano Keys strip or the
//! status bar ([`Layout::track_panel`]); the rack, divider, transport bar and lower pane end at
//! its left edge. A window too narrow to leave the rest [`MIN_REST_WIDTH`] hides it. It follows
//! the track selected in the arrangement and its choice is not saved.
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

use crate::about_box::{AboutBox, AboutOutcome};
use crate::app_core::{AppCore, AppError, FreezeThrough, MidiExport, Pane, Revealed};
use crate::commands::{AppCommand, Menu, MenuState, ScaleStep, SongList, menus_with_songs};
use crate::context_menus::{self, MenuEnable};
use crate::export_prefs::ExportPrefs;
use crate::guide::{Guide, GuideOutcome, Spot};
use crate::keymap_store::{self, WorkflowPrefs};
use crate::menubar::{MENU_BAR_HEIGHT, MenuBar, MenuBarOutcome};
use crate::session::{AudioBackend, LiveSession, SettingsChange};
use crate::settings::{self, AudioSettings, LastGood, SurfaceSettings};
use crate::settings_dialog::{DeviceLists, DialogOutcome, SettingsDialog};
use crate::start_screen::{ExampleCard, RecentRow, StartOutcome, StartScreen};
use crate::status::{AudioStatus, OverloadStats, status_text};
use crate::stress::{STRESS_CABLES, STRESS_DEVICES, stress_doc, stress_song_doc};
use crate::ui_state::UiState;
use crate::windows::{DocId, WindowHit, WindowKind};
use atomik_audio_io::StreamState;
use atomik_model::{ClipId, Command, ConsoleCommand, DeviceKey};
use atomik_ui::ContextTarget;
use atomik_ui::CursorHint;
use atomik_ui::browser::CreateKind;
use atomik_ui::context_menu::{ContextMenu, MenuOutcome};
use atomik_ui::keymap::{
    Context, ContextStack, GestureClass, KEYS_NOTE_ARGS, Keymap, KeymapEditor, Os, Profile,
    Resolution, Role, Roles, Route, Router, UserKeymap,
};
use atomik_ui::kurbo::{Affine, Point, Rect, RoundedRect, Size};
use atomik_ui::transport::TRANSPORT_HEIGHT;
use atomik_ui::{
    BassReadAction, BassReadView, DeliveryAction, DeliveryPage, DeliveryView, SoundAnalystAction,
    SoundAnalystView,
};
use atomik_ui::{
    BlindAction, BlindView, BrowseAction, Canvas, CanvasExt, ColorPickerAction, ColorPickerView,
    ColorTarget, ConsoleButton, ExportScope, ExportView, FileBrowserView, Flip, Key,
    KickBassAction, KickBassView, MacrosAction, MacrosView, MaskingAction, MaskingView,
    MidiMapsAction, MidiMapsView, MixCheckAction, MixCheckView, ModMatrixAction, ModMatrixView,
    Modifiers, NotesAction, NotesValues, NotesView, NumpadKey, ParamSearchAction, ParamSearchView,
    Place, PluginBannerView, PluginSettingsView, PointerButton, QuantizeAction, QuantizeValues,
    QuantizeView, RelinkAction, RelinkEntry, RelinkView, RepetitionAction, RepetitionView,
    ScaleAction, ScaleView, SilenceAction, SilenceValues, SilenceView, SongInfoAction,
    SongInfoView, TRACK_PANEL_WIDTH, TextRun, UiAction, UiEvent, VersionItem, VersionsAction,
    VersionsView, WarpAction, WarpValues, WarpView, ZoneAction, ZoneView,
};
use atomik_ui::{ReverbSendAction, ReverbSendValues, ReverbSendView};
use atomik_ui::{WavetableAction, WavetableView};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A stalled stream that stays silent this long is reset to the default devices when the
/// settings ask for it (a lost device is reset at once).
const AUTO_RESET_SILENCE_MS: u64 = 2_000;
/// How often the open Audio Settings dialog lists the MIDI ports again.
const PORT_RELIST_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

/// The keymap profile the app starts with: the standard
/// keys. The `legacy` profile (the keys of earlier versions) stays selectable in Settings > Keyboard.
pub const APP_DEFAULT_PROFILE: Profile = Profile::Standard;

/// Height of the status bar, logical pixels.
pub const STATUS_BAR_HEIGHT: f64 = 22.0;
/// Height of the rack / arrangement divider, logical pixels.
pub const DIVIDER_HEIGHT: f64 = 6.0;
/// The width the window keeps for everything but the file browser column.
pub const MIN_REST_WIDTH: f64 = 400.0;
/// Smallest height the divider leaves to the rack and to the song area.
pub const MIN_PANE_HEIGHT: f64 = 80.0;
/// The narrowest the Track Panel column is drawn; a window with less room hides it.
pub const MIN_TRACK_PANEL_WIDTH: f64 = 160.0;

/// What a film picked in the dialog of [`ShellRequest::PickVideoFile`] is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoPick {
    /// File > Video > Import Video: the song takes the film.
    Import,
    /// File > Video > Relink Video: the song's film points at the file.
    Relink,
    /// File > Video > Prepare Video: only the proxy is built.
    Prepare,
}

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
    /// Show a folder dialog (the relink panel's PROJECT FOLDER...), then call
    /// [`Shell::relink_project_folder`] with the picked folder.
    PickRelinkProjectFolder,
    /// Show a folder dialog (File > Move Project), then call [`Shell::move_project_to`] with the
    /// picked folder: the project goes into a folder of its own name inside it.
    PickMoveFolder,
    /// Show a save dialog for a project archive (File > Export Project Archive): an `.atomikz`
    /// file named `name`, then call [`Shell::export_archive_to`] with the path.
    PickExportArchive {
        /// The suggested file name, with its extension.
        name: String,
    },
    /// Show an open dialog for a project archive (File > Import Project Archive), then call
    /// [`Shell::import_archive_from`] with the path.
    PickImportArchive,
    /// Show an open dialog for a film (File > Video > Import Video, Relink Video or Prepare
    /// Video), then call [`Shell::video_picked`] with the same [`VideoPick`] and the path.
    PickVideoFile(VideoPick),
    /// Show a folder dialog (the Video tab's FFmpeg folder Choose...), then call
    /// [`Shell::set_ffmpeg_folder`] with the picked folder.
    PickFfmpegFolder,
    /// Show a folder dialog (the Workflow tab's Scratch folder Choose...), then call
    /// [`Shell::set_scratch_folder`] with the picked folder.
    PickScratchFolder,
    /// Show a folder in the file manager (File > New from Template > Show Template Folder). The
    /// folder is one the app made itself.
    ShowFolder(PathBuf),
    /// Show a file in the file manager with the file selected where the system allows (the file
    /// browser's Show in Folder). The path is a row the browser listed.
    Reveal(PathBuf),
    /// Show a save dialog for a MIDI mapping preset (Mixer > Export MIDI Mapping Preset, the
    /// panel's EXPORT), starting in [`Shell::midi_preset_dir`], then call
    /// [`Shell::export_midi_preset_to`] with the path.
    PickExportMidiPreset,
    /// Show an open dialog for a MIDI mapping preset (Mixer > Import MIDI Mapping Preset, the
    /// panel's IMPORT), starting in [`Shell::midi_preset_dir`], then call
    /// [`Shell::import_midi_preset_from`] with the path.
    PickImportMidiPreset,
    /// Ask for a file to save the selected device's mappings as the preset of its type; answer
    /// with [`Shell::export_midi_device_preset_to`].
    PickExportMidiDevicePreset,
    /// The rack has unsaved changes and `then` would lose them: ask Save, Don't Save or Cancel.
    /// Save: call [`Shell::save_then`]; Don't Save: call [`Shell::discard_then`]; Cancel: do
    /// nothing.
    ConfirmSave {
        /// What waits for the answer.
        then: PendingAction,
    },
    /// Show a save-file dialog for a project, then call [`Shell::save_to_then`] with the path
    /// and `then`. A cancelled dialog ends the pending action.
    PickSaveFileThen {
        /// What continues once the project is saved.
        then: PendingAction,
    },
    /// Show a save dialog for a device patch (the patch strip's SAVE, Rack > Save Patch): an
    /// `.atomikpatch` file starting in `dir` (the library's Patches folder) and named `name`; the
    /// platform dialog asks before it overwrites a file. Then call [`Shell::save_patch_to`] with
    /// the path. A cancelled dialog does nothing.
    PickSavePatch {
        /// The folder the dialog starts in.
        dir: PathBuf,
        /// The suggested file name, with its extension.
        name: String,
    },
    /// Show a save dialog for the diagnostics report (Help > Export Diagnostics), named by
    /// [`crate::diagnostics::default_file_name`], then call [`Shell::export_diagnostics_to`].
    PickExportDiagnostics,
    /// Show a save dialog for the device parameter list, named by
    /// [`crate::device_info::default_file_name`], then call [`Shell::export_device_info_to`].
    PickExportDeviceInfo,
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
    /// Show an open-file dialog for a MIDI file (the quantize panel's GROOVE FROM MIDI FILE),
    /// then call [`Shell::groove_from_midi_file`] with the picked path.
    PickGrooveMidi,
    /// Show an open-file dialog for a reference track (Mixer > Reference > Add Reference
    /// Track), then call [`Shell::add_reference_file`] with the picked path.
    PickReference,
    /// Show an open-file dialog for the audio file of the Sound Analyst (its OPEN FILE button),
    /// then call [`Shell::analyse_sound_file`] with the picked path.
    PickSoundFile,
    /// Open an address in the default browser (Help > Documentation, Report an Issue and the
    /// link of the About box). Only the project's own addresses are ever asked for.
    OpenUrl(&'static str),
    /// Bring a window to the front and give it the keyboard focus (restoring it if it is
    /// minimised).
    FocusWindow(WindowKind),
    /// Maximise a window, or restore it if it is maximised (the console's MAX button and
    /// Shift+F5 while the console is detached).
    ToggleMaximise(WindowKind),
    /// Close the window and exit.
    Quit,
    /// A new song in a window of its own ([`crate::workspace::Workspace::new_song`]). Only a
    /// workspace answers it.
    NewSong,
    /// Show an open-file dialog (projects and patches), then call
    /// [`crate::workspace::Workspace::open_song`]: the file opens in a song of its own.
    PickOpenSong,
    /// End this song and close its windows ([`crate::workspace::Workspace::close_song`]); the last
    /// song asks to quit instead. Only a workspace answers it.
    CloseSong,
    /// Move the audio and the keys to another song (the workspace answers it).
    ShowSong(SongTarget),
    /// Bring the main window of a song to the front and give it the keyboard focus.
    FocusSong(DocId),
}

/// Which song [`ShellRequest::ShowSong`] asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SongTarget {
    /// The song after the one with the audio.
    Next,
    /// The song before it.
    Previous,
    /// The song at this place of the list of open songs.
    Place(usize),
}

/// What waits for the answer to the question about unsaved changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingAction {
    /// A command that replaces or ends the song: New, Open, Close, Quit.
    Command(AppCommand),
    /// Open this project (picked in the library, dropped on the window or chosen from the
    /// recent songs).
    OpenFile(PathBuf),
    /// Restore this version of the project (File > Versions) as the current song.
    RestoreVersion(u32),
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
    /// The Track Panel column (only while it shows): on the right of the window, between the
    /// menu bar and the status bar.
    pub track_panel: Option<Rect>,
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
    /// The About box, while it is open.
    about: Option<AboutBox>,
    /// The start screen, while it is open.
    start_screen: Option<StartScreen>,
    /// The guided first song, while its panel is open (shell state: never saved in a song).
    guide: Option<Guide>,
    /// The operating system is set to a dark theme (what the System theme choice follows).
    system_dark: bool,
    /// The rack split before a double-click on the divider collapsed the rack: the next
    /// double-click puts it back.
    split_before: Option<f64>,
    /// When the open Audio Settings dialog last listed the MIDI ports again.
    ports_listed_at: Option<std::time::Instant>,
    settings_path: Option<PathBuf>,
    /// The surface settings the control surface was last set up with.
    surface_applied: Vec<SurfaceSettings>,
    ui_state_path: Option<PathBuf>,
    ui: UiState,
    /// The open windows: the main window always, a detached pane's while it is detached.
    wins: BTreeMap<WindowKind, WinState>,
    status: String,
    /// The engine fault already announced with a toast (cleared when the fault is).
    fault_announced: bool,
    /// The dead stream (stalled or lost) already announced with a toast, and already reset to
    /// the default devices (both cleared when the stream is alive again).
    stream_announced: bool,
    stream_reset: bool,
    /// The overload counters of the last status poll, and the ones last written to the log.
    overload: OverloadStats,
    /// The audio status of the last poll, for the diagnostics report.
    audio_status: AudioStatus,
    overload_logged: OverloadStats,
    /// Status polls since the counters were last logged (they are logged at most every 20).
    overload_polls: u32,
    /// The devices text last given to the crash report.
    crash_context: String,
    repaint: bool,
    /// What holds the pointer, and the window the press happened in.
    capture: Option<(WindowKind, Capture)>,
    /// The pane the pointer is over while a pane holds the capture (a drag that can leave the
    /// pane it started in); `None` over no pane.
    capture_over: Option<Pane>,
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
    /// The device a patch save dialog is open for.
    patch_device: Option<DeviceKey>,
    /// The device whose patch name opened the pop-up of its folder's patches.
    patch_menu: Option<DeviceKey>,
    /// The MIDI Out device whose port pop-up is open.
    midi_out_menu: Option<DeviceKey>,
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
    mod_matrix: Option<ModMatrixView>,
    /// The parameter search (View > Find Parameter) while it is open.
    param_search: Option<ParamSearchView>,
    /// The control a right click opened the MIDI menu for (what Learn MIDI and Forget MIDI act on).
    control_target: Option<(DeviceKey, String)>,
    /// The quantize panel (Edit > Quantize Options) while it is open.
    quantize: Option<QuantizeView>,
    /// The panel's settings, kept between openings (view state, not in the project).
    quantize_values: QuantizeValues,
    /// The Scale Tempo panel (Edit > Scale Tempo > Other) while it is open.
    scale: Option<ScaleView>,
    /// The percentage the panel opened with last time (view state, not in the project).
    scale_percent: f64,
    /// The blind A/B panel (device context menu) while it is open.
    blind: Option<BlindView>,
    /// The strip silence panel (Edit > Strip Silence) while it is open.
    silence: Option<SilenceView>,
    /// The song information panel (File > Song Information) while it is open.
    song_info: Option<SongInfoView>,
    /// The reverb send wizard (Track > Add Reverb Send) while it is open.
    reverb_send: Option<ReverbSendView>,
    /// Its choices, kept between openings.
    reverb_send_values: ReverbSendValues,
    /// The kick and bass check panel (Mixer > Kick and Bass Check) while it is open.
    kick_bass: Option<KickBassView>,
    /// The references and decision zone panel (Mixer > Reference > Reference Panel) while it is
    /// open.
    zone: Option<ZoneView>,
    /// The versions panel, while it is open.
    versions: Option<VersionsView>,
    /// The masking map panel (Mixer > Masking Map), while it is open.
    masking: Option<MaskingView>,
    /// The mix check panel (Mixer > Check Mix), while it is open.
    mixcheck: Option<MixCheckView>,
    /// The repetition panel (Edit > Find Repetition), while it is open.
    repetition: Option<RepetitionView>,
    /// The bass-read panel (Mixer > Will the Bass Read?), while it is open.
    bass_read: Option<BassReadView>,
    delivery: Option<DeliveryView>,
    /// The Sound Analyst panel (File > Sound Analyst), while it is open.
    sound_analyst: Option<SoundAnalystView>,
    /// The wavetable editor (a click on a table display of a Wavetable Synth) while it is open,
    /// with the device it edits.
    wavetable: Option<(WavetableView, DeviceKey)>,
    /// The colour picker (the colour menu's Custom item) while it is open, with what it colours.
    color_picker: Option<(ColorPickerView, ColorTarget)>,
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
    /// The Piano Keys strip is shown (View > Piano Keys).
    /// The file drag in progress (see [`Capture::FileDrag`]).
    file_drag: Option<FileDrag>,
    /// The help tooltip: what the pointer rests on and whether it shows yet.
    tips: Tips,
    /// Another song of the workspace has the audio: this one runs without a stream, a MIDI
    /// input or a control surface until it is focused ([`Self::claim_audio`]).
    silent: bool,
    /// The names of the open songs and this one's place in the list (the Window menu).
    songs: Vec<String>,
    song_at: usize,
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
        let (ui, ui_note) = options
            .ui_state_path
            .as_deref()
            .map(UiState::load_checked)
            .unwrap_or_default();
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
        let mut startup_notes: Vec<String> = ui_note.into_iter().collect();
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
            about: None,
            system_dark: true,
            split_before: None,
            ports_listed_at: None,
            settings_path: options.settings_path,
            surface_applied: Vec::new(),
            ui_state_path: options.ui_state_path,
            ui,
            wins: BTreeMap::from([(WindowKind::Main, WinState::new(MAIN_SIZE, 1.0, Pane::Rack))]),
            status: String::new(),
            fault_announced: false,
            stream_announced: false,
            stream_reset: false,
            overload: OverloadStats::default(),
            audio_status: AudioStatus::default(),
            overload_logged: OverloadStats::default(),
            overload_polls: 0,
            crash_context: String::new(),
            repaint: true,
            capture: None,
            capture_over: None,
            focus: Pane::Rack,
            focused: WindowKind::Main,
            geom_dirty: false,
            geom_saved_at: f64::NEG_INFINITY,
            pending: Vec::new(),
            load_device: None,
            patch_device: None,
            patch_menu: None,
            midi_out_menu: None,
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
            scale: None,
            scale_percent: 150.0,
            blind: None,
            silence: None,
            song_info: None,
            reverb_send: None,
            reverb_send_values: ReverbSendValues::default(),
            kick_bass: None,
            zone: None,
            versions: None,
            masking: None,
            mixcheck: None,
            repetition: None,
            bass_read: None,
            delivery: None,
            sound_analyst: None,
            wavetable: None,
            start_screen: None,
            guide: None,
            color_picker: None,
            notes: None,
            notes_values: NotesValues::default(),
            warp: None,
            warp_values: WarpValues::default(),
            silence_values: SilenceValues::default(),
            relink_target: None,
            midi_maps: None,
            macros_panel: None,
            mod_matrix: None,
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
            silent: false,
            songs: Vec::new(),
            song_at: 0,
        };
        shell.core.set_roles(&shell.roles);
        shell.configure_surface();
        shell.core.set_reference_path(
            options_library_path
                .as_deref()
                .map(crate::spectrum_reference::reference_path),
        );
        shell.core.set_library_path(options_library_path);
        shell.sync_library_root();
        // Before anything saves the window state: a save copies the panel's values.
        shell.restore_file_browser();
        shell.restore_windows();
        shell.apply_workflow_to_views();
        shell.apply_ui_state_to_views();
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
        let pre_count = shell.core.sink().settings().pre_count_bars;
        shell.core.set_pre_count_bars(pre_count);
        let default_tune = shell.core.sink().settings().master_tune_cents;
        shell.core.set_default_tune(default_tune);
        shell.layout_view();
        shell.poll_status();
        shell
    }

    /// Hands the saved window state to the views: the rack's console display preferences
    /// (shown unless the user switched them off), the arrangement, piano roll and audio editor
    /// flags, and the input quantize.
    fn apply_ui_state_to_views(&mut self) {
        {
            let view = self.core.view_mut();
            view.set_show_console_links(self.ui.show_console_links);
            view.set_navigator_visible(self.ui.show_rack_navigator);
        }
        self.core
            .set_import_audio_with_video(self.ui.video_import_audio);
        self.core.console_mut().set_link_faders(self.ui.link_faders);
        self.core.console_mut().set_meter_mode(self.ui.meter_mode);
        self.core.set_morph_seconds(self.ui.morph_seconds);
        {
            let arrangement = self.core.arrangement_mut();
            arrangement.set_tempo_lane(self.ui.show_tempo_lane);
            arrangement.set_energy_lane(self.ui.show_energy_lane);
            arrangement.set_video_row(self.ui.show_video_row);
            arrangement.set_video_row_collapsed(self.ui.video_row_collapsed);
            arrangement.set_marker_strip(self.ui.show_marker_strip);
            arrangement.set_inspector(self.ui.show_inspector);
            arrangement.set_marker_list(self.ui.show_marker_list);
        }
        self.core
            .piano_roll_mut()
            .set_inspector(self.ui.show_inspector);
        self.core
            .audio_editor_mut()
            .set_mode(self.ui.audio_editor_mode);
        self.core
            .arrangement_mut()
            .set_track_zoom(self.ui.track_zoom);
        self.core
            .piano_roll_mut()
            .set_key_height(self.ui.piano_roll_key_height);
        {
            let keys = self.core.piano_keys_mut();
            keys.set_octave(self.ui.piano_keys_octave);
            keys.set_velocity(self.ui.piano_keys_velocity);
        }
        self.core.set_video_prefs(atomik_ui::VideoPrefs {
            size: self.ui.video_size,
            overlay: atomik_ui::VideoOverlay {
                timecode: self.ui.video_timecode,
                frame_number: self.ui.video_frame_number,
            },
        });
        {
            let roll = self.core.piano_roll_mut();
            roll.set_snap_to_scale(self.ui.piano_roll_snap_scale);
            roll.set_fold(self.ui.piano_roll_fold);
            roll.set_snap(self.ui.piano_roll_snap);
            roll.set_note_audition(self.ui.piano_roll_audition);
            roll.set_snap_mode(self.ui.snap_mode);
        }
        self.core
            .transport_bar_mut()
            .set_fade_unit(self.ui.fade_unit);
        {
            let arrangement = self.core.arrangement_mut();
            arrangement.set_snap(self.ui.arrangement_snap);
            arrangement.set_snap_mode(self.ui.snap_mode);
        }
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

    /// The blind A/B panel, while it is open.
    pub fn blind_view(&self) -> Option<&BlindView> {
        self.blind.as_ref()
    }

    /// Opens the blind A/B panel for the selected effect and starts a test; says why when the
    /// selection cannot be tested.
    fn open_blind(&mut self) {
        let Some(device) = self.core.view().primary() else {
            self.core.report("Select an effect to test first");
            return;
        };
        if let Err(message) = self.core.blind_start(&device, 0) {
            self.core.report(message);
            return;
        }
        let theme = self.core.view().theme().clone();
        let mut view = BlindView::new(theme);
        view.set_scale(self.scale());
        view.set_bounds(self.plugin_modal_rect());
        self.blind = Some(view);
        self.sync_blind();
        self.repaint = true;
    }

    /// Feeds the blind panel what the test shows; the panel closes when the test is gone.
    fn sync_blind(&mut self) {
        if self.blind.is_none() {
            return;
        }
        match self.core.blind_info() {
            Some(info) => {
                if let Some(view) = &mut self.blind {
                    view.set_info(info);
                    if view.needs_repaint() {
                        self.repaint = true;
                    }
                }
            }
            None => {
                self.blind = None;
                self.repaint = true;
            }
        }
    }

    /// What the blind panel asked for.
    fn blind_action(&mut self, action: BlindAction) {
        self.core.blind_action(action);
        if action == BlindAction::Close {
            self.blind = None;
        }
        self.sync_blind();
        self.repaint = true;
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

    /// Takes the missing audio files from the picked project folder (see
    /// [`AppCore::relink_project_folder`]).
    pub fn relink_project_folder(&mut self, folder: PathBuf) {
        self.core.relink_project_folder(&folder);
        self.repaint = true;
    }

    /// File > Export Project Archive: the dialog asks for the `.atomikz` file, unless an archive
    /// job runs already.
    fn export_archive_request(&mut self) -> Vec<ShellRequest> {
        if self.core.archive_busy() {
            self.notify("A project archive is already being made".to_owned());
            return Vec::new();
        }
        let name = format!(
            "{}.{}",
            self.core.archive_name(),
            atomik_audio_file::ARCHIVE_EXTENSION
        );
        vec![ShellRequest::PickExportArchive { name }]
    }

    /// The folder picked for [`ShellRequest::PickMoveFolder`]: moves the project into it (see
    /// [`AppCore::move_project`]); the toast says what happened.
    pub fn move_project_to(&mut self, folder: &Path) {
        match self.core.move_project(folder) {
            Ok(()) => {}
            Err(AppError::NoPath) => {
                self.notify("Save the song first: Move Project needs a project file".to_owned());
            }
            Err(e) => self.notify(e.to_string()),
        }
        self.repaint = true;
    }

    /// Writes the project archive the dialog picked (see [`AppCore::start_archive_export`]); the
    /// extension is added when the name has none.
    pub fn export_archive_to(&mut self, path: &Path) {
        let mut path = path.to_path_buf();
        if path.extension().is_none() {
            path.set_extension(atomik_audio_file::ARCHIVE_EXTENSION);
        }
        // The core says why in a toast when it refuses.
        let _ = self.core.start_archive_export(path);
        self.repaint = true;
    }

    /// Uses the film the dialog picked for what it was asked for: FFmpeg is looked for and the
    /// film probed; Import puts it in the song, Relink points the song's film at it, and the
    /// proxy is built or found in the cache (see [`AppCore::import_video`],
    /// [`AppCore::relink_video`] and [`AppCore::start_prepare_video`]). The core says why in a
    /// toast when it refuses.
    pub fn video_picked(&mut self, pick: VideoPick, path: &Path) {
        let path = path.to_path_buf();
        let _ = match pick {
            VideoPick::Prepare => self.core.start_prepare_video(path),
            VideoPick::Import => self.core.import_video(path),
            VideoPick::Relink => self.core.relink_video(path),
        };
        self.repaint = true;
    }

    /// Unpacks the project archive the dialog picked beside itself; the project opens when it is
    /// done (see [`AppCore::start_archive_import`]).
    pub fn import_archive_from(&mut self, path: &Path) {
        let _ = self.core.start_archive_import(path.to_path_buf());
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

    /// Whether the reverb send wizard is open.
    pub fn reverb_send_open(&self) -> bool {
        self.reverb_send.is_some()
    }

    /// The reverb send wizard, while it is open.
    pub fn reverb_send_view(&self) -> Option<&ReverbSendView> {
        self.reverb_send.as_ref()
    }

    /// The choices of the wizard (those of the open panel, else the last ones).
    pub fn reverb_send_values(&self) -> &ReverbSendValues {
        self.reverb_send
            .as_ref()
            .map_or(&self.reverb_send_values, ReverbSendView::values)
    }

    /// Opens the reverb send wizard with the choices of the last time.
    fn open_reverb_send(&mut self) {
        let theme = self.core.view().theme().clone();
        let mut view = ReverbSendView::new(theme, self.reverb_send_values.clone());
        view.set_scale(self.scale());
        view.set_bounds(self.plugin_modal_rect());
        self.reverb_send = Some(view);
        self.sync_reverb_send();
        self.repaint = true;
    }

    /// Closes the reverb send wizard, keeping its choices for the next time.
    fn close_reverb_send(&mut self) {
        if let Some(view) = self.reverb_send.take() {
            self.reverb_send_values = view.values().clone();
            self.repaint = true;
        }
    }

    /// Feeds the wizard the track it works on and the decay that track suggests.
    fn sync_reverb_send(&mut self) {
        let Some(vocal) = self.reverb_send.as_ref().map(|v| v.values().vocal) else {
            return;
        };
        let (name, decay) = self.core.reverb_send_info(vocal).unwrap_or_default();
        if let Some(view) = &mut self.reverb_send {
            view.set_target(name, decay);
            if view.needs_repaint() {
                self.repaint = true;
            }
        }
    }

    /// What the reverb send wizard asked for.
    fn reverb_send_action(&mut self, action: ReverbSendAction) {
        match action {
            ReverbSendAction::Close => self.close_reverb_send(),
            ReverbSendAction::Add => {
                let v = self.reverb_send_values().clone();
                let options = atomik_model::reverb_send::ReverbSendOptions {
                    vocal: v.vocal,
                    decay: v.decay,
                    duck: v.duck,
                    high_pass: v.high_pass as f32,
                    low_pass: v.low_pass as f32,
                    send_db: v.send_db as f32,
                    pre_fader: v.pre_fader,
                };
                // The panel closes when the send is made; it stays, with the reason in the
                // status line, when the model refuses.
                if self.core.add_reverb_send(options) {
                    self.close_reverb_send();
                }
            }
        }
        self.sync_reverb_send();
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

    /// Whether the Scale Tempo panel is open.
    pub fn scale_open(&self) -> bool {
        self.scale.is_some()
    }

    /// The Scale Tempo panel, while it is open.
    pub fn scale_view(&self) -> Option<&ScaleView> {
        self.scale.as_ref()
    }

    /// Opens the Scale Tempo panel with the percentage of the last time; the selected clips
    /// show the result at once.
    fn open_scale(&mut self) {
        let theme = self.core.view().theme().clone();
        let mut view = ScaleView::new(theme, self.scale_percent);
        view.set_scale(self.scale());
        view.set_bounds(self.plugin_modal_rect());
        let (text, can_apply) = self.core.scale_target();
        view.set_target(text, can_apply);
        let actions = view.open_preview();
        self.scale = Some(view);
        self.scale_actions(actions);
        self.repaint = true;
    }

    /// What the Scale Tempo panel asked for.
    fn scale_actions(&mut self, actions: Vec<UiAction>) {
        for action in actions {
            if let UiAction::ScaleTempo(a) = action {
                self.scale_action(a);
            }
        }
    }

    /// One request of the Scale Tempo panel: a preview, APPLY (one undo step, then the panel
    /// closes) or CLOSE (the clips go back to how they were).
    fn scale_action(&mut self, action: ScaleAction) {
        match action {
            ScaleAction::Preview(percent) => self.core.scale_preview(percent),
            ScaleAction::Apply => {
                if let Some(percent) = self.scale.as_ref().and_then(ScaleView::percent) {
                    self.scale_percent = percent;
                    self.core.scale_apply(percent);
                }
                self.scale = None;
            }
            ScaleAction::Close => {
                self.core.scale_preview(None);
                self.scale = None;
            }
        }
        self.repaint = true;
    }

    /// Whether the song information panel is open.
    pub fn song_info_open(&self) -> bool {
        self.song_info.is_some()
    }

    /// Whether a text field has the keys, so the window system should offer its input method
    /// (composition windows, dead keys) to the window.
    pub fn wants_text_input(&self) -> bool {
        self.song_info.is_some()
            || self.versions.is_some()
            || self.quantize.as_ref().is_some_and(QuantizeView::wants_text)
            || self.scale.is_some()
            || self.color_picker.is_some()
            || self.ime_target().is_some()
    }

    /// The pane an input-method event goes to: the focused pane while one of its editable text
    /// fields (the browser's search and name entries) has the keys.
    fn ime_target(&self) -> Option<Pane> {
        let focus = self.live_pane(self.focus);
        (focus == Pane::FileBrowser && self.core.file_browser().is_text_owner()).then_some(focus)
    }

    /// The song information panel, while it is open.
    pub fn song_info_view(&self) -> Option<&SongInfoView> {
        self.song_info.as_ref()
    }

    /// Opens the song information panel with the song's texts.
    fn open_song_info(&mut self) {
        let theme = self.core.view().theme().clone();
        let mut view = SongInfoView::new(theme, self.core.song_info().clone());
        view.set_scale(self.scale());
        view.set_bounds(self.plugin_modal_rect());
        self.song_info = Some(view);
        self.repaint = true;
    }

    /// What the song information panel asked for: APPLY makes its texts the song information (one
    /// undo step) and closes it.
    fn song_info_action(&mut self, action: SongInfoAction) {
        match action {
            SongInfoAction::Close => {}
            SongInfoAction::Apply => {
                if let Some(view) = &self.song_info {
                    let info = view.info().clone();
                    // The core says why in a toast when a text is refused.
                    if self.core.set_song_info(info).is_err() {
                        return;
                    }
                }
            }
        }
        self.song_info = None;
        self.repaint = true;
    }

    /// Whether the kick and bass check panel is open.
    pub fn kick_bass_open(&self) -> bool {
        self.kick_bass.is_some()
    }

    /// The kick and bass check panel, while it is open.
    pub fn kick_bass_view(&self) -> Option<&KickBassView> {
        self.kick_bass.as_ref()
    }

    /// Opens the kick and bass check panel (the picks are guessed from the track names).
    fn open_kick_bass(&mut self) {
        self.core.kick_bass_open();
        let theme = self.core.view().theme().clone();
        let mut view = KickBassView::new(theme);
        view.set_scale(self.scale());
        view.set_bounds(self.plugin_modal_rect());
        view.set_info(self.core.kick_bass_info());
        self.kick_bass = Some(view);
        self.repaint = true;
    }

    /// Hands the open kick and bass panel what the app knows now (the worker's progress and
    /// result included).
    fn sync_kick_bass(&mut self) {
        if self.kick_bass.is_none() {
            return;
        }
        let info = self.core.kick_bass_info();
        if let Some(v) = &mut self.kick_bass {
            v.set_info(info);
            if v.needs_repaint() {
                self.repaint = true;
            }
        }
    }

    /// What the kick and bass panel asked for.
    fn kick_bass_action(&mut self, action: KickBassAction) {
        match action {
            KickBassAction::Close => {
                self.kick_bass = None;
                self.repaint = true;
                return;
            }
            KickBassAction::Kick(d) => self.core.kick_bass_pick(false, d),
            KickBassAction::Bass(d) => self.core.kick_bass_pick(true, d),
            KickBassAction::Run => self.core.kick_bass_run(),
            KickBassAction::Cancel => self.core.kick_bass_cancel(),
            KickBassAction::Apply(n) => self.core.kick_bass_apply(n),
        }
        self.sync_kick_bass();
    }

    /// Whether the references and decision zone panel is open.
    pub fn zone_open(&self) -> bool {
        self.zone.is_some()
    }

    /// The references and decision zone panel, while it is open.
    pub fn zone_view(&self) -> Option<&ZoneView> {
        self.zone.as_ref()
    }

    /// Opens the references and decision zone panel. The first time, with a reference listed, it
    /// starts measuring the mix.
    fn open_zone(&mut self) {
        let theme = self.core.view().theme().clone();
        if !self.core.zone_measured() {
            self.core.refresh_zone();
        }
        let mut view = ZoneView::new(theme, self.core.zone_data());
        view.set_scale(self.scale());
        view.set_bounds(self.plugin_modal_rect());
        self.zone = Some(view);
        self.repaint = true;
    }

    /// Keeps the open decision zone panel in step with the core (a reading that landed, a
    /// reference that was added or selected).
    fn sync_zone(&mut self) {
        if self.zone.is_none() {
            return;
        }
        let data = self.core.zone_data();
        if let Some(view) = &mut self.zone
            && view.set_data(data)
        {
            self.repaint = true;
        }
    }

    /// What the references and decision zone panel asked for.
    fn zone_action(&mut self, action: ZoneAction) {
        match action {
            ZoneAction::Close => self.zone = None,
            ZoneAction::Refresh => {
                self.core.refresh_zone();
            }
            ZoneAction::Select(index) => self.core.select_reference(index),
            ZoneAction::Listen => self.core.toggle_reference(),
        }
        self.sync_zone();
        self.repaint = true;
    }

    /// Whether the versions panel is open.
    pub fn versions_open(&self) -> bool {
        self.versions.is_some()
    }

    /// The versions panel, while it is open.
    pub fn versions_view(&self) -> Option<&VersionsView> {
        self.versions.as_ref()
    }

    /// The rows of the versions panel: the versions of the open project, newest first.
    fn version_items(&self) -> Vec<VersionItem> {
        self.core
            .versions()
            .unwrap_or_default()
            .into_iter()
            .map(|v| VersionItem {
                id: v.id,
                when: crate::logging::minute_stamp(v.time),
                name: v.name,
                automatic: v.automatic,
                protected: v.protected,
            })
            .collect()
    }

    /// Whether the masking map panel is open.
    pub fn masking_open(&self) -> bool {
        self.masking.is_some()
    }

    /// The masking map panel, while it is open.
    pub fn masking_view(&self) -> Option<&MaskingView> {
        self.masking.as_ref()
    }

    /// Mixer > Masking Map: opens the panel (with the last result, if the project has one).
    fn open_masking(&mut self) {
        let theme = self.core.view().theme().clone();
        let mut view = MaskingView::new(theme);
        view.set_scale(self.scale());
        view.set_bounds(self.plugin_modal_rect());
        view.set_data(self.core.masking_data());
        view.set_progress(self.core.masking_progress());
        self.masking = Some(view);
        self.repaint = true;
    }

    /// Keeps the open masking panel in step with the analysis: its progress, its result, and the
    /// line that says how it ended.
    fn sync_masking(&mut self) {
        let Some(view) = &mut self.masking else {
            return;
        };
        let was_busy = view.busy();
        view.set_progress(self.core.masking_progress());
        view.set_data(self.core.masking_data());
        if was_busy && !view.busy() {
            view.set_note(self.core.last_rejection().unwrap_or(""));
        }
        if view.needs_repaint() {
            self.repaint = true;
        }
    }

    /// What the masking panel asked for.
    fn masking_action(&mut self, action: MaskingAction) {
        match action {
            MaskingAction::Close => self.masking = None,
            MaskingAction::Analyse => {
                let _ = self.core.start_masking();
                self.sync_masking();
                if let Some(v) = &mut self.masking {
                    v.set_note(
                        self.core
                            .last_rejection()
                            .filter(|_| !self.core.masking_running())
                            .unwrap_or(""),
                    );
                }
            }
            MaskingAction::Cancel => self.core.cancel_masking(),
            MaskingAction::Locate { pair, range } => self.core.locate_masking(pair, range),
            MaskingAction::Demask { pair, band } => {
                let note = match self.core.demask(pair, band) {
                    Ok(_) => self.core.last_rejection().unwrap_or("").to_owned(),
                    Err(why) => why,
                };
                if let Some(v) = &mut self.masking {
                    v.set_note(&note);
                }
            }
        }
        self.repaint = true;
    }

    /// Whether the bass-read panel is open.
    pub fn bass_read_open(&self) -> bool {
        self.bass_read.is_some()
    }

    /// The bass-read panel, while it is open.
    pub fn bass_read_view(&self) -> Option<&BassReadView> {
        self.bass_read.as_ref()
    }

    /// Mixer > Will the Bass Read?: opens the panel on the selected track, else the master.
    fn open_bass_read(&mut self) {
        self.core.bass_read_preselect();
        let theme = self.core.view().theme().clone();
        let mut view = BassReadView::new(theme);
        view.set_scale(self.scale());
        view.set_bounds(self.plugin_modal_rect());
        view.set_data(self.core.bass_read_data());
        self.bass_read = Some(view);
        self.repaint = true;
    }

    /// Keeps the open bass-read panel in step with the reading: its progress and its result.
    fn sync_bass_read(&mut self) {
        if self.bass_read.is_none() {
            return;
        }
        let data = self.core.bass_read_data();
        if let Some(view) = &mut self.bass_read {
            view.set_data(data);
            if view.needs_repaint() {
                self.repaint = true;
            }
        }
    }

    /// What the bass-read panel asked for.
    fn bass_read_action(&mut self, action: BassReadAction) {
        match action {
            BassReadAction::Close => {
                self.core.cancel_bass_read();
                self.bass_read = None;
            }
            BassReadAction::Choose(n) => self.core.bass_read_choose(n),
            BassReadAction::Measure => {
                let _ = self.core.start_bass_read();
            }
            BassReadAction::Stop => self.core.cancel_bass_read(),
        }
        self.sync_bass_read();
        self.repaint = true;
    }

    /// Whether the Delivery panel is open.
    pub fn delivery_open(&self) -> bool {
        self.delivery.is_some()
    }

    /// The Delivery panel, while it is open.
    pub fn delivery_view(&self) -> Option<&DeliveryView> {
        self.delivery.as_ref()
    }

    /// File > Delivery Targets (the Targets page) and Edit > Peak Surgeon (the Peaks page): opens
    /// the panel.
    fn open_delivery(&mut self, page: DeliveryPage) {
        self.core.delivery_open(page);
        let theme = self.core.view().theme().clone();
        let mut view = DeliveryView::new(theme);
        view.set_scale(self.scale());
        view.set_bounds(self.plugin_modal_rect());
        view.set_data(self.core.delivery_data());
        self.delivery = Some(view);
        self.repaint = true;
    }

    /// Keeps the open Delivery panel in step with the measurement and the clip.
    fn sync_delivery(&mut self) {
        if self.delivery.is_none() {
            return;
        }
        let data = self.core.delivery_data();
        if let Some(view) = &mut self.delivery {
            view.set_data(data);
            if view.needs_repaint() {
                self.repaint = true;
            }
        }
    }

    /// What the Delivery panel asked for.
    fn delivery_action(&mut self, action: DeliveryAction) {
        match action {
            DeliveryAction::Close => {
                self.core.cancel_delivery();
                self.delivery = None;
            }
            DeliveryAction::Page(page) => self.core.delivery_page(page),
            DeliveryAction::Choose(n) => self.core.delivery_choose(n),
            DeliveryAction::Measure => {
                let _ = self.core.start_delivery();
            }
            DeliveryAction::Stop => self.core.cancel_delivery(),
            DeliveryAction::SetCeiling => self.core.delivery_set_ceiling(),
            DeliveryAction::PeakChoose(n) => self.core.surgeon_choose(n),
            DeliveryAction::Threshold(delta) => self.core.surgeon_threshold(delta),
            DeliveryAction::CycleStep => self.core.surgeon_cycle_step(),
            DeliveryAction::CycleWidth => self.core.surgeon_cycle_width(),
            DeliveryAction::Trim => self.core.surgeon_trim(),
            DeliveryAction::ApplyAll => self.core.surgeon_apply_all(),
            DeliveryAction::Rescan => self.core.surgeon_scan(),
        }
        self.sync_delivery();
        self.repaint = true;
    }

    /// Whether the Sound Analyst panel is open.
    pub fn sound_analyst_open(&self) -> bool {
        self.sound_analyst.is_some()
    }

    /// The Sound Analyst panel, while it is open.
    pub fn sound_analyst_view(&self) -> Option<&SoundAnalystView> {
        self.sound_analyst.as_ref()
    }

    /// File > Sound Analyst: opens the panel on what was analysed last.
    fn open_sound_analyst(&mut self) {
        let theme = self.core.view().theme().clone();
        let mut view = SoundAnalystView::new(theme);
        view.set_scale(self.scale());
        view.set_bounds(self.plugin_modal_rect());
        view.set_data(self.core.sound_data());
        self.sound_analyst = Some(view);
        self.repaint = true;
    }

    /// Keeps the open Sound Analyst panel in step with the analysis: its progress and its result.
    fn sync_sound_analyst(&mut self) {
        if self.sound_analyst.is_none() {
            return;
        }
        let data = self.core.sound_data();
        if let Some(view) = &mut self.sound_analyst {
            view.set_data(data);
            if view.needs_repaint() {
                self.repaint = true;
            }
        }
    }

    /// Starts analysing the audio file `path` (the picked or dropped file) and opens the panel
    /// when it is not open.
    pub fn analyse_sound_file(&mut self, path: &Path) {
        if self.sound_analyst.is_none() {
            self.open_sound_analyst();
        }
        // A refusal is already in the status line.
        let _ = self.core.analyse_sound_file(path);
        self.sync_sound_analyst();
    }

    /// What the Sound Analyst panel asked for.
    fn sound_analyst_action(&mut self, action: SoundAnalystAction) {
        match action {
            SoundAnalystAction::Close => {
                self.core.cancel_sound();
                self.sound_analyst = None;
            }
            SoundAnalystAction::OpenFile => self.pending.push(ShellRequest::PickSoundFile),
            SoundAnalystAction::Stop => self.core.cancel_sound(),
            SoundAnalystAction::MakePatch => {
                let _ = self.core.make_sound_patch();
            }
            SoundAnalystAction::Match => {
                let _ = self.core.start_sound_match();
            }
        }
        self.sync_sound_analyst();
        self.repaint = true;
    }

    /// Whether the wavetable editor is open.
    pub fn wavetable_open(&self) -> bool {
        self.wavetable.is_some()
    }

    /// The wavetable editor, while it is open.
    pub fn wavetable_view(&self) -> Option<&WavetableView> {
        self.wavetable.as_ref().map(|(v, _)| v)
    }

    /// A click on a table display: opens the editor on that Wavetable Synth.
    fn open_wavetable_editor(&mut self, device: DeviceKey) {
        let Some(data) = self.core.wavetable_data(&device) else {
            return;
        };
        let theme = self.core.view().theme().clone();
        let mut view = WavetableView::new(theme);
        view.set_scale(self.scale());
        view.set_bounds(self.plugin_modal_rect());
        view.set_data(data);
        self.wavetable = Some((view, device));
        self.repaint = true;
    }

    /// Keeps the open wavetable editor in step with the document (undo, redo, a new sample).
    fn sync_wavetable(&mut self) {
        let Some((_, device)) = &self.wavetable else {
            return;
        };
        let device = device.clone();
        match self.core.wavetable_data(&device) {
            Some(data) => {
                if let Some((view, _)) = &mut self.wavetable {
                    view.set_data(data);
                    if view.needs_repaint() {
                        self.repaint = true;
                    }
                }
            }
            // The device was removed.
            None => self.close_wavetable_editor(),
        }
    }

    fn close_wavetable_editor(&mut self) {
        self.wavetable = None;
        self.core.release_wavetable();
        self.repaint = true;
    }

    /// What the wavetable editor asked for.
    fn wavetable_action(&mut self, action: WavetableAction) {
        let Some((_, device)) = &self.wavetable else {
            return;
        };
        let device = device.clone();
        match action {
            WavetableAction::Close => self.close_wavetable_editor(),
            WavetableAction::Commit { keys, phase_safe } => {
                if let Err(e) = self.core.commit_wavetable(&device, &keys, phase_safe) {
                    self.core.report_wavetable(format!("table: {e}"));
                }
            }
            WavetableAction::CutCycles => self.core.cut_wavetable_cycles(&device),
            WavetableAction::UseOnOsc1 => self.core.wavetable_use_on_osc1(&device),
        }
        self.sync_wavetable();
        self.repaint = true;
    }

    /// Whether the repetition panel is open.
    pub fn repetition_open(&self) -> bool {
        self.repetition.is_some()
    }

    /// The repetition panel, while it is open.
    pub fn repetition_view(&self) -> Option<&RepetitionView> {
        self.repetition.as_ref()
    }

    /// Edit > Find Repetition: searches the song and opens the panel on what it found.
    fn open_repetition(&mut self) {
        self.core.find_repetition();
        let theme = self.core.view().theme().clone();
        let mut view = RepetitionView::new(theme);
        view.set_scale(self.scale());
        view.set_bounds(self.plugin_modal_rect());
        view.set_data(self.core.repetition_data());
        self.repetition = Some(view);
        self.repaint = true;
    }

    /// What the repetition panel asked for.
    fn repetition_action(&mut self, action: RepetitionAction) {
        let mut note = None;
        match action {
            RepetitionAction::Close => self.repetition = None,
            RepetitionAction::Find => {
                let count = self.core.find_repetition();
                note = Some(match count {
                    0 => "Nothing repeats three times in a row unchanged".to_owned(),
                    1 => "1 idea found".to_owned(),
                    n => format!("{n} ideas found"),
                });
            }
            RepetitionAction::Locate(n) => self.core.locate_repetition(n),
            RepetitionAction::Apply(n, k) => {
                note = Some(match self.core.apply_repetition(n, k) {
                    Ok(()) => self.core.last_rejection().unwrap_or("").to_owned(),
                    Err(why) => why,
                });
            }
        }
        if let Some(v) = &mut self.repetition {
            v.set_data(self.core.repetition_data());
            if let Some(note) = note {
                v.set_note(&note);
            }
        }
        self.repaint = true;
    }

    /// Whether the mix check panel is open.
    pub fn mixcheck_open(&self) -> bool {
        self.mixcheck.is_some()
    }

    /// The mix check panel, while it is open.
    pub fn mixcheck_view(&self) -> Option<&MixCheckView> {
        self.mixcheck.as_ref()
    }

    /// Mixer > Check Mix: opens the panel (with the last result, if the project has one).
    fn open_mixcheck(&mut self) {
        let theme = self.core.view().theme().clone();
        let mut view = MixCheckView::new(theme);
        view.set_scale(self.scale());
        view.set_bounds(self.plugin_modal_rect());
        view.set_data(self.core.mixcheck_data());
        view.set_progress(self.core.mixcheck_progress());
        self.mixcheck = Some(view);
        self.repaint = true;
    }

    /// Keeps the open mix check panel in step with the check: its progress, its result, and the
    /// line that says how it ended.
    fn sync_mixcheck(&mut self) {
        let Some(view) = &mut self.mixcheck else {
            return;
        };
        let was_busy = view.busy();
        view.set_progress(self.core.mixcheck_progress());
        view.set_data(self.core.mixcheck_data());
        if was_busy && !view.busy() {
            view.set_note(self.core.last_rejection().unwrap_or(""));
        }
        if view.needs_repaint() {
            self.repaint = true;
        }
    }

    /// What the mix check panel asked for.
    fn mixcheck_action(&mut self, action: MixCheckAction) {
        let mut note = None;
        match action {
            MixCheckAction::Close => self.mixcheck = None,
            MixCheckAction::Run => {
                let _ = self.core.start_mixcheck();
                self.sync_mixcheck();
                note = Some(
                    self.core
                        .last_rejection()
                        .filter(|_| !self.core.mixcheck_running())
                        .unwrap_or("")
                        .to_owned(),
                );
            }
            MixCheckAction::Cancel => self.core.cancel_mixcheck(),
            MixCheckAction::Locate(n) => self.core.locate_mixcheck(n),
            MixCheckAction::Fix(n) => {
                note = Some(match self.core.fix_mixcheck(n) {
                    Ok(()) => self.core.last_rejection().unwrap_or("").to_owned(),
                    Err(why) => why,
                });
            }
            MixCheckAction::Open(n) => match self.core.mixcheck_tool(n) {
                Some(atomik_render::mixcheck::Tool::MaskingMap) => {
                    self.mixcheck = None;
                    self.open_masking();
                    self.masking_action(MaskingAction::Analyse);
                }
                Some(atomik_render::mixcheck::Tool::KickBassCheck) => {
                    self.core.mixcheck_kick_bass(n);
                    self.mixcheck = None;
                    self.open_kick_bass();
                }
                Some(atomik_render::mixcheck::Tool::Clipper) => {
                    let _ = self.core.mixcheck_clipper(n);
                    note = Some(self.core.last_rejection().unwrap_or("").to_owned());
                }
                None => {
                    note = Some(self.core.last_rejection().unwrap_or("").to_owned());
                }
            },
        }
        if let (Some(v), Some(note)) = (&mut self.mixcheck, note) {
            v.set_note(&note);
        }
        self.repaint = true;
    }

    /// File > Save Version and File > Versions: opens the versions panel (the name has the keys;
    /// with `list` the versions do). A song with no project file yet has no versions to keep.
    fn open_versions(&mut self, list: bool) {
        if self.core.versions_file().is_none() {
            self.notify(
                "Save the song first: versions are kept next to the project file".to_owned(),
            );
            return;
        }
        let theme = self.core.view().theme().clone();
        let mut view = VersionsView::new(theme, self.version_items());
        view.set_scale(self.scale());
        view.set_bounds(self.plugin_modal_rect());
        if list {
            view.focus_list();
        }
        self.versions = Some(view);
        self.repaint = true;
    }

    /// Replaces the rows of the open versions panel with the versions the project holds now.
    fn refresh_versions(&mut self, clear_name: bool) {
        let items = self.version_items();
        if let Some(view) = &mut self.versions {
            view.set_items(items, clear_name);
        }
        self.repaint = true;
    }

    /// What the versions panel asked for.
    fn versions_action(&mut self, action: VersionsAction) {
        match action {
            VersionsAction::Close => self.versions = None,
            VersionsAction::Save(name) => match self.core.save_version(&name) {
                Ok(_) => self.refresh_versions(true),
                Err(e) => self.notify(e.to_string()),
            },
            VersionsAction::Compare { id, with } => match self.core.compare_version(id, with) {
                Ok(c) => {
                    if let Some(view) = &mut self.versions {
                        view.show_report(&c.before, &c.after, c.lines);
                    }
                }
                Err(e) => self.notify(e.to_string()),
            },
            VersionsAction::Protect { id, protected } => {
                match self.core.set_version_protected(id, protected) {
                    Ok(()) => self.refresh_versions(false),
                    Err(e) => self.notify(e.to_string()),
                }
            }
            VersionsAction::Restore(id) => {
                // A save that is still writing decides whether there is anything unsaved.
                self.core.settle_save();
                if self.core.is_dirty() {
                    self.pending.push(ShellRequest::ConfirmSave {
                        then: PendingAction::RestoreVersion(id),
                    });
                } else {
                    self.restore_version(id);
                }
            }
        }
        self.repaint = true;
    }

    /// Restores the version `id` as the current song and closes the versions panel. A version
    /// that cannot be restored is reported and the panel stays.
    fn restore_version(&mut self, id: u32) {
        match self.core.restore_version(id) {
            Ok(()) => self.versions = None,
            Err(e) => self.notify(e.to_string()),
        }
        self.repaint = true;
    }

    /// Whether the start screen is open.
    pub fn start_screen_open(&self) -> bool {
        self.start_screen.is_some()
    }

    /// The start screen, while it is open.
    pub fn start_screen(&self) -> Option<&StartScreen> {
        self.start_screen.as_ref()
    }

    /// The rows of the recent column: the recent songs as they are now, the pinned ones marked.
    fn start_recent_rows(&self) -> Vec<RecentRow> {
        self.core
            .recent()
            .iter()
            .map(|p| {
                let modified = std::fs::metadata(p)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs());
                RecentRow::new(p, modified).pinned(self.core.is_pinned(p))
            })
            .collect()
    }

    /// Pins or unpins the song at place `index` of the recent list from the start screen: it
    /// moves to or from the top of the list and the box shows the new order.
    fn toggle_start_pin(&mut self, index: usize) {
        let Some(path) = self.core.recent().get(index).cloned() else {
            return;
        };
        let done = if self.core.is_pinned(&path) {
            self.core.unpin_recent(&path)
        } else if self.core.pin_recent(&path) {
            true
        } else {
            self.notify(format!(
                "At most {} songs can be pinned: unpin one first",
                crate::recent::MAX_PINNED
            ));
            false
        };
        if done {
            let rows = self.start_recent_rows();
            let at = self.core.recent().iter().position(|p| *p == path);
            if let Some(screen) = &mut self.start_screen {
                screen.set_recent(rows, at);
            }
        }
        self.repaint = true;
    }

    /// Reads the preview of the template the start screen card is about, when it has not been
    /// read yet (once per template while the box is open).
    fn fill_template_preview(&mut self) {
        let Some(i) = self
            .start_screen
            .as_ref()
            .and_then(StartScreen::preview_wanted)
        else {
            return;
        };
        let preview = self
            .core
            .templates()
            .get(i)
            .and_then(|t| self.core.template_preview(&t.path));
        if let Some(screen) = &mut self.start_screen {
            screen.set_preview(i, preview);
        }
    }

    /// Opens the start screen with the templates and the recent songs as they are now.
    fn open_start_screen(&mut self) {
        self.core.reload_templates();
        let templates: Vec<String> = self
            .core
            .templates()
            .iter()
            .map(|t| t.name.clone())
            .collect();
        let recent = self.start_recent_rows();
        let examples: Vec<ExampleCard> = self
            .core
            .examples()
            .into_iter()
            .map(|e| ExampleCard::new(e.name, e.line))
            .collect();
        self.start_screen = Some(
            StartScreen::new(
                templates,
                recent,
                self.workflow.default_song.is_some(),
                self.workflow.show_start_screen,
            )
            .with_cards(examples, self.workflow.show_guide_card),
        );
        self.fill_template_preview();
        self.repaint = true;
    }

    /// Start-up when the command line names no project: the last song when the preference
    /// says so, otherwise the default song (when one is chosen) and the start screen (unless
    /// the preference says not to). Only the app's window calls it; the headless and MCP
    /// sessions never do, so they never show the screen.
    pub fn start_without_project(&mut self) {
        if self.restore_last_project() {
            return;
        }
        self.start_with_default_song();
        if self.workflow.show_start_screen {
            self.open_start_screen();
        }
    }

    /// An event for the start screen: a pick closes it and runs the File menu's command for that
    /// row; the check box is saved at once.
    fn start_screen_event(&mut self, event: &UiEvent) -> Vec<ShellRequest> {
        let size = self.size();
        let Some(screen) = &mut self.start_screen else {
            return Vec::new();
        };
        let outcome = screen.handle(event, size);
        if matches!(outcome, StartOutcome::Open) {
            self.fill_template_preview();
        }
        match outcome {
            StartOutcome::Open => Vec::new(),
            StartOutcome::TogglePin(i) => {
                self.toggle_start_pin(i);
                Vec::new()
            }
            StartOutcome::Close => {
                self.start_screen = None;
                Vec::new()
            }
            StartOutcome::ShowAtStart(on) => {
                self.workflow.show_start_screen = on;
                self.save_workflow();
                Vec::new()
            }
            StartOutcome::ShowGuide(on) => {
                self.workflow.show_guide_card = on;
                self.save_workflow();
                Vec::new()
            }
            StartOutcome::Run(command) => {
                self.start_screen = None;
                self.execute(command)
            }
        }
    }

    /// Whether the colour picker is open.
    pub fn color_picker_open(&self) -> bool {
        self.color_picker.is_some()
    }

    /// The colour picker, while it is open.
    pub fn color_picker_view(&self) -> Option<&ColorPickerView> {
        self.color_picker.as_ref().map(|(v, _)| v)
    }

    /// Opens the colour picker for `target`, from the colour it shows now.
    fn open_color_picker(&mut self, target: ColorTarget) {
        let song = &self.core.doc().song;
        let current = match &target {
            ColorTarget::Folder(group) => song.org.group(*group).and_then(|g| g.color),
            ColorTarget::Tracks(devices) => devices.first().and_then(|d| song.shown_color(d)),
            ColorTarget::Devices(devices) => devices
                .first()
                .and_then(|d| self.core.doc().device(d))
                .and_then(|d| d.color),
        };
        let theme = self.core.view().theme().clone();
        let mut view = ColorPickerView::new(theme, current.unwrap_or([0x3f, 0x8f, 0xd2]));
        view.set_scale(self.scale());
        view.set_bounds(self.plugin_modal_rect());
        self.color_picker = Some((view, target));
        self.repaint = true;
    }

    /// What the colour picker asked for: APPLY gives its colour to the folder, or to the tracks
    /// and the devices that play them (one undo step), and closes it.
    fn color_picker_action(&mut self, action: ColorPickerAction) {
        if let (ColorPickerAction::Apply, Some((view, target))) = (action, &self.color_picker) {
            let color = Some(view.color());
            let command = match target {
                ColorTarget::Folder(group) => {
                    Command::Song(atomik_model::SongCommand::SetGroupColor {
                        group: *group,
                        color,
                    })
                }
                ColorTarget::Devices(devices) => Command::Batch(
                    devices
                        .iter()
                        .map(|device| Command::SetColor {
                            key: device.clone(),
                            color,
                        })
                        .collect(),
                ),
                ColorTarget::Tracks(devices) => Command::Batch(
                    devices
                        .iter()
                        .flat_map(|device| {
                            [
                                Command::Song(atomik_model::SongCommand::SetTrackColor {
                                    device: device.clone(),
                                    color,
                                }),
                                Command::SetColor {
                                    key: device.clone(),
                                    color,
                                },
                            ]
                        })
                        .collect(),
                ),
            };
            // A rejection (a track that went away) is reported by the core.
            let _ = self.core.apply_command(command);
        }
        self.color_picker = None;
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
        let progress = self.core.notes_progress();
        if let Some(view) = &mut self.notes {
            view.set_target(text, can_apply);
            view.set_progress(progress);
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
            NotesAction::Cancel => {
                self.core.cancel_notes();
            }
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
                    // The scale is spent: the panel shows the clip as it is now.
                    if values.scale_pct != 100
                        && let Some(now) = self.core.warp_values()
                        && let Some(view) = &mut self.warp
                    {
                        view.set_values(now);
                    }
                }
            }
            WarpAction::Reset => {
                self.core.reset_warp();
                if let Some(now) = self.core.warp_values()
                    && let Some(view) = &mut self.warp
                {
                    view.set_values(now);
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
        self.core.quantize_audio_with(&params);
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

    /// What the quantize panel asked for; the requests are the dialogs it needs.
    fn quantize_action(&mut self, action: QuantizeAction) -> Vec<ShellRequest> {
        let pane = self.live_pane(self.focus);
        let Some(view) = &self.quantize else {
            return Vec::new();
        };
        let (params, values) = (view.params(), view.values().clone());
        match action {
            QuantizeAction::Close => self.close_quantize(),
            QuantizeAction::Apply => self.core.apply_quantize(pane, params),
            // Only the piano roll and the quantize key send these (see `pane_actions`).
            QuantizeAction::ApplyLast | QuantizeAction::ApplyClips | QuantizeAction::Options => {}
            QuantizeAction::SetLive => {
                if let Some(name) = values.groove {
                    self.core
                        .set_clip_groove(pane, Some((name, values.strength)));
                }
            }
            QuantizeAction::ClearLive => self.core.set_clip_groove(pane, None),
            QuantizeAction::Rename { from, to } => {
                if self.core.rename_groove(&from, &to) {
                    // The renamed groove stays the chosen one.
                    self.sync_quantize();
                    if let Some(view) = &mut self.quantize {
                        view.select_groove(&to);
                    }
                }
            }
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
            QuantizeAction::FromMidi => return vec![ShellRequest::PickGrooveMidi],
        }
        self.sync_quantize();
        self.repaint = true;
        Vec::new()
    }

    /// Makes a groove of the MIDI file at `path` (the file the quantize panel asked for, or one
    /// dropped on it), measured with the step and slots of the panel; the new groove becomes
    /// the chosen one. Without an open panel nothing happens.
    pub fn groove_from_midi_file(&mut self, path: &Path) {
        let Some(values) = self.quantize.as_ref().map(|v| v.values().clone()) else {
            return;
        };
        if let Some(name) =
            self.core
                .groove_from_midi(path, values.extract_step, values.extract_slots)
        {
            self.sync_quantize();
            if let Some(view) = &mut self.quantize {
                view.select_groove(&name);
            }
        }
        self.repaint = true;
    }

    /// Adds the audio file the Add Reference Track dialog picked to the song's reference
    /// tracks (a refusal goes to the status line).
    pub fn add_reference_file(&mut self, path: &Path) {
        if let Err(e) = self.core.add_reference(path) {
            self.core.report(format!("Reference track not added: {e}"));
        }
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
        view.set_ports(self.midi_port_names());
        self.midi_maps = Some(view);
        self.repaint = true;
    }

    /// The input ports the MIDI Mappings panel offers: the ones set up in Settings (the names a
    /// source is matched against), then the ports mappings already name.
    fn midi_port_names(&self) -> Vec<String> {
        let mut ports: Vec<String> = self
            .core
            .sink()
            .settings()
            .midi_inputs
            .iter()
            .map(|i| i.name.clone())
            .collect();
        for m in &self.core.doc().midi_maps {
            if let Some(p) = &m.source.port
                && !ports.contains(p)
            {
                ports.push(p.clone());
            }
        }
        ports
    }

    /// Feeds the MIDI Mappings panel the project's rows.
    fn sync_midi_maps(&mut self) {
        let rows = self.core.midi_map_rows();
        let ports = self.midi_port_names();
        if let Some(view) = &mut self.midi_maps {
            view.set_rows(rows);
            view.set_ports(ports);
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
            MidiMapsAction::ExportDevice => {
                requests.push(ShellRequest::PickExportMidiDevicePreset);
            }
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

    /// The modulation matrix panel, while it is open.
    pub fn mod_matrix_view(&self) -> Option<&ModMatrixView> {
        self.mod_matrix.as_ref()
    }

    /// Opens the modulation matrix of `device`; says why when the device has none.
    fn open_mod_matrix(&mut self, device: &DeviceKey) {
        let Some(info) = self.core.mod_matrix_info(device) else {
            self.core
                .report("Select a device with a modulation matrix first");
            return;
        };
        let theme = self.core.view().theme().clone();
        let mut view = ModMatrixView::new(theme);
        view.set_scale(self.scale());
        view.set_bounds(self.plugin_modal_rect());
        view.set_info(info);
        self.mod_matrix = Some(view);
        self.repaint = true;
    }

    /// Feeds the matrix panel the device's slots; the panel closes when the device is gone.
    fn sync_mod_matrix(&mut self) {
        let Some(device) = self.mod_matrix.as_ref().and_then(|v| v.device().cloned()) else {
            return;
        };
        match self.core.mod_matrix_info(&device) {
            Some(info) => {
                if let Some(view) = &mut self.mod_matrix {
                    view.set_info(info);
                    if view.needs_repaint() {
                        self.repaint = true;
                    }
                }
            }
            None => {
                self.mod_matrix = None;
                self.repaint = true;
            }
        }
    }

    /// What the matrix panel asked for: its edits are ordinary commands.
    fn mod_matrix_action(&mut self, action: UiAction) {
        match action {
            UiAction::ModMatrix(ModMatrixAction::Close) => self.mod_matrix = None,
            other => self.core.handle_action(other),
        }
        self.sync_mod_matrix();
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

    /// The folder of the logs and the crash reports of this run.
    pub fn log_folder(&self) -> PathBuf {
        crate::logging::log_dir(&crate::settings::config_folder(self.settings_path.as_deref()).dir)
    }

    /// The report Help > Export Diagnostics writes: the status, the counters, the settings, the
    /// plugin state, the last crash and the log, with the home folder redacted.
    pub fn diagnostics_inputs(&mut self) -> crate::diagnostics::Inputs {
        self.poll_status();
        let (plugin_cache, plugin_journal) = self.core.plugin_state_files();
        crate::diagnostics::Inputs {
            log_dir: self.log_folder(),
            settings_path: self.settings_path.clone(),
            plugin_cache,
            plugin_journal,
            status_line: self.status.clone(),
            audio: self.audio_status.clone(),
            home_dirs: crate::diagnostics::home_dirs(&|k| std::env::var_os(k)),
        }
    }

    /// Writes the diagnostics report to the picked file and says where it went.
    pub fn export_diagnostics_to(&mut self, path: &Path) {
        let inputs = self.diagnostics_inputs();
        match crate::diagnostics::export(&inputs, path) {
            Ok(()) => {
                log::info!("diagnostics written to {}", path.display());
                self.notify(format!("Diagnostics written to {}", path.display()));
            }
            Err(e) => {
                log::error!("cannot write the diagnostics to {}: {e}", path.display());
                self.notify(format!("Could not write the diagnostics: {e}"));
            }
        }
        self.repaint = true;
    }

    /// Writes the parameter list of every built-in device to the picked file.
    pub fn export_device_info_to(&mut self, path: &Path) {
        match crate::device_info::export(self.core.registry(), path) {
            Ok(()) => self.notify(format!("Device parameters written to {}", path.display())),
            Err(e) => {
                log::error!(
                    "cannot write the device parameters to {}: {e}",
                    path.display()
                );
                self.notify(format!("Could not write the device parameters: {e}"));
            }
        }
        self.repaint = true;
    }

    /// Writes the project's MIDI mappings to the picked preset file.
    pub fn export_midi_preset_to(&mut self, path: &Path) {
        self.core.export_midi_preset(path, &[]);
        self.repaint = true;
    }

    /// Writes the mappings of the selected device to the picked preset file.
    pub fn export_midi_device_preset_to(&mut self, path: &Path) {
        self.core.export_midi_device_preset(path);
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
            RelinkAction::ProjectFolder => vec![ShellRequest::PickRelinkProjectFolder],
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

    /// Sets the factory library folder (see [`crate::factory`]); `None` when the application has
    /// none. It is read-only and never the library folder.
    pub fn set_factory_folder(&mut self, folder: Option<PathBuf>) {
        self.core.set_factory_root(folder);
        self.repaint = true;
    }

    /// Hands the library folder in use to the core and the browser panel.
    fn sync_library_root(&mut self) {
        let root = self.library_root();
        self.core.set_library_root(root);
        // The templates folder sits next to the library folder.
        if self.core.reload_templates() {
            self.repaint = true;
        }
    }

    /// The scratch folder picked in the folder dialog: into the open settings dialog's draft
    /// (applied with its Apply), else applied and saved at once.
    pub fn set_scratch_folder(&mut self, folder: PathBuf) {
        if let Some(dialog) = &mut self.dialog {
            dialog.set_scratch_folder(Some(folder));
        } else {
            self.workflow.scratch_folder = Some(folder);
            self.save_workflow();
            self.core
                .set_scratch_folder(self.workflow.scratch_folder.clone());
        }
        self.repaint = true;
    }

    /// The FFmpeg folder picked in the folder dialog: into the open settings dialog's draft
    /// (applied with its Apply), else applied and saved at once.
    pub fn set_ffmpeg_folder(&mut self, folder: PathBuf) {
        if let Some(dialog) = &mut self.dialog {
            dialog.set_ffmpeg_folder(Some(folder));
        } else {
            self.workflow.ffmpeg_folder = Some(folder);
            self.save_workflow();
            self.core
                .set_video_folder(self.workflow.ffmpeg_folder.clone());
            self.core.refresh_ffmpeg_status();
        }
        self.repaint = true;
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
            // A new FFmpeg folder is looked at once, so the Video tab has the answer.
            self.core.refresh_ffmpeg_status();
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
        self.core
            .set_autosave_minutes(self.workflow.autosave_minutes);
        self.core
            .set_scratch_folder(self.workflow.scratch_folder.clone());
        self.core
            .set_video_folder(self.workflow.ffmpeg_folder.clone());
        self.core.set_record_mode(self.workflow.record_mode);
        self.core.set_keep_events(self.workflow.keep_events);
        self.core.set_warp_imports(self.workflow.warp_imports);
        self.core
            .set_master_keyboard_mode(self.workflow.master_keyboard);
        self.core.set_follow(self.workflow.follow_song);
        self.core
            .set_return_to_start_on_stop(self.workflow.return_to_start_on_stop);
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
        self.core.set_knob_range(self.workflow.knob_range);
        self.core
            .set_automation_thinning(self.workflow.automation_thinning.fraction());
        self.core.set_auto_color(self.workflow.auto_color);
        self.core
            .set_reverse_mode(self.workflow.reverse_mode.graphical());
        self.core.set_midi_resolution(self.workflow.midi_resolution);
        self.core.set_keep_versions(self.workflow.keep_versions);
        self.core.set_preview_db(self.workflow.preview_db);
        self.core
            .view_mut()
            .set_value_tips(self.workflow.value_tooltips);
        self.apply_theme();
        self.layout_view();
    }

    /// Opens the settings dialog, on `tab` when one is asked for.
    fn open_settings(&mut self, tab: Option<crate::settings_dialog::Tab>) {
        let session = self.core.sink();
        let mut lists = DeviceLists::query(session.backend());
        lists.input_note = crate::status::input_note(session.status());
        lists.stream_notes = [
            crate::status::stream_alert(session.status()),
            crate::status::latency_note(session.status()),
            crate::status::drift_note(session.status()),
        ]
        .into_iter()
        .flatten()
        .collect();
        // The tune field edits the open project's tune; the saved default only changes with it.
        let mut current = session.settings().clone();
        current.master_tune_cents = self.core.doc().tune_cents;
        let mut dialog = SettingsDialog::new(current, lists).with_preferences(
            KeymapEditor::new(self.os, self.default_profile, self.user_keymap.clone()),
            self.workflow.clone(),
        );
        if let Some(tab) = tab {
            dialog.show_tab(tab);
        }
        self.dialog = Some(dialog);
        self.menubar.as_mut().map(MenuBar::close);
    }

    /// The theme in use: the preference, with the System choice following the operating system.
    pub fn theme(&self) -> atomik_ui::Theme {
        self.workflow.theme.theme(self.system_dark)
    }

    /// Tells the shell whether the operating system is set to a dark theme (the platform calls it
    /// at start and whenever the setting changes); the System choice follows.
    pub fn set_system_dark(&mut self, dark: bool) {
        if self.system_dark != dark {
            self.system_dark = dark;
            self.apply_theme();
        }
    }

    /// Hands the theme to every view the shell and the core hold, when it is not the one they
    /// already have: a switch is live, nothing keeps the colours of the old one.
    fn apply_theme(&mut self) {
        let theme = self.theme();
        if self.core.view().theme() == &theme {
            return;
        }
        self.core.set_theme(theme.clone());
        self.banner.set_theme(theme.clone());
        if let Some(v) = &mut self.plugin_settings {
            v.set_theme(theme.clone());
        }
        if let Some(v) = &mut self.relink {
            v.set_theme(theme.clone());
        }
        if let Some(v) = &mut self.export {
            v.set_theme(theme.clone());
        }
        if let Some(v) = &mut self.midi_maps {
            v.set_theme(theme.clone());
        }
        if let Some(v) = &mut self.macros_panel {
            v.set_theme(theme.clone());
        }
        if let Some(v) = &mut self.mod_matrix {
            v.set_theme(theme.clone());
        }
        if let Some(v) = &mut self.param_search {
            v.set_theme(theme.clone());
        }
        if let Some(v) = &mut self.quantize {
            v.set_theme(theme.clone());
        }
        if let Some(v) = &mut self.scale {
            v.set_theme(theme.clone());
        }
        if let Some(v) = &mut self.blind {
            v.set_theme(theme.clone());
        }
        if let Some(v) = &mut self.silence {
            v.set_theme(theme.clone());
        }
        if let Some(v) = &mut self.song_info {
            v.set_theme(theme.clone());
        }
        if let Some(v) = &mut self.reverb_send {
            v.set_theme(theme.clone());
        }
        if let Some(v) = &mut self.kick_bass {
            v.set_theme(theme.clone());
        }
        if let Some(v) = &mut self.zone {
            v.set_theme(theme.clone());
        }
        if let Some(v) = &mut self.versions {
            v.set_theme(theme.clone());
        }
        if let Some(v) = &mut self.masking {
            v.set_theme(theme.clone());
        }
        if let Some(v) = &mut self.mixcheck {
            v.set_theme(theme.clone());
        }
        if let Some(v) = &mut self.repetition {
            v.set_theme(theme.clone());
        }
        if let Some(v) = &mut self.bass_read {
            v.set_theme(theme.clone());
        }
        if let Some(v) = &mut self.delivery {
            v.set_theme(theme.clone());
        }
        if let Some(v) = &mut self.sound_analyst {
            v.set_theme(theme.clone());
        }
        if let Some((v, _)) = &mut self.wavetable {
            v.set_theme(theme.clone());
        }
        if let Some((v, _)) = &mut self.color_picker {
            v.set_theme(theme.clone());
        }
        if let Some(v) = &mut self.notes {
            v.set_theme(theme.clone());
        }
        if let Some(v) = &mut self.warp {
            v.set_theme(theme);
        }
        self.repaint = true;
    }

    /// The application zoom as a factor (1.0 is 100 %).
    pub fn app_zoom(&self) -> f64 {
        f64::from(self.workflow.app_zoom) / 100.0
    }

    /// The scale the windows' content is drawn at: the display scale `os_scale` times the
    /// application zoom. The platform gives this, not the display scale, with every window's
    /// scale and size.
    pub fn ui_scale(&self, os_scale: f64) -> f64 {
        os_scale * self.app_zoom()
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
            "Keys moved: Record {}, Quantize {}, Split {}, Create {} / {}, Import {}, File Browser {}, \
             Piano Keys {}, Group Tracks {}; R is the razor. Settings > Keyboard > Atomik legacy restores the old keys.",
            key("transport.record"),
            key("editor.quantize"),
            key("sequencer.split_at_playhead"),
            key("create.instrument"),
            key("create.effect"),
            key("file.import_audio"),
            key("view.toggle_file_browser"),
            key("view.toggle_piano_keys"),
            key("sequencer.group_tracks"),
        )
    }

    /// Re-reads `keymap.json` when it changed on disk since it was last read or written (an
    /// edit by hand or by another run): the keymap follows without a restart. A deleted file
    /// is the defaults. Returns whether it reloaded. It is looked at when a window gets the focus
    /// and when a menu opens, not on a timer.
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
        let recent: Vec<String> = self
            .core
            .recent()
            .iter()
            .map(|p| crate::recent::menu_label(p))
            .collect();
        let templates: Vec<String> = self
            .core
            .templates()
            .iter()
            .map(|t| t.name.clone())
            .collect();
        let vcas: Vec<(u32, String)> = self
            .core
            .doc()
            .console
            .vcas
            .iter()
            .map(|v| (v.id, v.name.clone()))
            .collect();
        let buses: Vec<(u32, String)> = self
            .core
            .doc()
            .console
            .buses
            .iter()
            .enumerate()
            .map(|(i, b)| (i as u32 + 1, b.name.clone()))
            .collect();
        menus_with_songs(
            &self.menu_state(),
            self.router.keymap(),
            &recent,
            &templates,
            &SongList {
                names: &self.songs,
                current: self.song_at,
                vcas: &vcas,
                buses: &buses,
            },
        )
    }

    /// The modifiers last reported by the window.
    pub fn modifiers(&self) -> Modifiers {
        self.modifiers
    }

    /// The window lost focus: the key releases will never arrive, so forget the held keys and
    /// the modifier state (a hold-style role must not stay on).
    pub fn focus_lost(&mut self) {
        self.router.clear_held();
        let released = self.core.piano_keys_mut().release_all();
        for action in released {
            self.core.handle_action(action);
        }
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
            Pane::PianoRoll if self.core.piano_roll().is_text_owner() => {
                ContextStack::new(&[Context::Text, Context::Editor, Context::Sequencer])
            }
            Pane::PianoRoll => ContextStack::new(&[Context::Editor, Context::Sequencer]),
            Pane::AudioEditor if self.core.audio_editor().is_text_owner() => {
                ContextStack::new(&[Context::Text, Context::Editor, Context::Sequencer])
            }
            Pane::AudioEditor => ContextStack::new(&[Context::Editor, Context::Sequencer]),
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
            Pane::PianoKeys if self.core.piano_keys().is_typing() => {
                ContextStack::new(&[Context::Keys])
            }
            Pane::PianoKeys => ContextStack::global(),
            Pane::Transport
            | Pane::PluginParams
            | Pane::SpectrumEq
            | Pane::Video
            | Pane::TrackPanel => ContextStack::global(),
        }
    }

    /// What the menus show.
    pub fn menu_state(&self) -> MenuState {
        let view = self.core.view();
        let song = &self.core.doc().song;
        let (can_combine, can_uncombine) = self.core.combine_menu();
        MenuState {
            flip: view.flip(),
            cables: view.cable_visibility(),
            browser: view.browser_visible(),
            file_browser: self.ui.file_browser_visible,
            piano_keys: self.detached(WindowKind::PianoKeys),
            track_panel: self.core.track_panel_visible(),
            rack_sort: self.core.doc().sort_mode,
            single_column: view.single_column(),
            can_undo: self.core.document().can_undo(),
            can_redo: self.core.document().can_redo(),
            undo_label: self.core.document().undo_label(),
            redo_label: self.core.document().redo_label(),
            can_edit: self.core.has_selection(self.live_pane(self.focus)),
            can_paste: self.core.can_paste(self.live_pane(self.focus)),
            can_combine,
            can_uncombine,
            can_split_sub: self.core.split_sub_menu(),
            can_reverb_send: self.core.reverb_send_menu(),
            lens: self.core.lens(),
            song_visible: self.ui.show_song,
            piano_roll: self.core.piano_roll_open(),
            sample_editor: self.core.sample_editor_device().is_some(),
            step_editor: self.core.step_editor_device().is_some(),
            slice_editor: self.core.slice_editor_device().is_some(),
            playing: self.core.is_playing(),
            recording: self.core.is_recording(),
            loop_enabled: song.loop_enabled,
            metronome: self.core.metronome(),
            auto_lanes: self.core.auto_create_lanes(),
            pre_count: self.core.pre_count_bars() > 0,
            clock_out: self.core.sink().settings().sync.clock_out,
            clock_while_stopped: self.core.sink().settings().sync.send_while_stopped,
            follow_clock: self.core.sink().settings().sync.follow_clock,
            sample_rate: self.core.doc().sample_rate,
            midi_resolution: self.core.midi_resolution(),
            input_channels: self.core.doc().input_channels,
            output_channels: self.core.doc().output_channels,
            quantize: self.core.input_quantize(),
            console: self.console_visible(),
            console_maximised: self.maximised(),
            compensation: self.core.doc().console.compensation,
            align_returns: self.core.doc().console.align_returns,
            send_follow: self.core.doc().console.send_follow,
            link_faders: self.core.console().link_faders(),
            console_links: view.show_console_links(),
            cpu_meters: self.core.cpu_meters_on(),
            big_meter: view.big_meter_visible(),
            tempo_lane: self.core.arrangement().tempo_lane_visible(),
            energy_lane: self.core.arrangement().energy_lane_visible(),
            video_row: self.core.arrangement().video_row_visible(),
            marker_strip: self.core.arrangement().marker_strip_visible(),
            inspector: self.core.arrangement().inspector_visible(),
            marker_list: self.core.arrangement().marker_list_visible(),
            rack_navigator: view.navigator_visible(),
            rack: self.workflow.rack_visible,
            rack_detached: self.rack_detached(),
            console_detached: self.console_detached(),
            spectrum_eq: self.detached(WindowKind::SpectrumEq),
            video_window: self.detached(WindowKind::Video),
            video_overlay: self.core.video_window().prefs().overlay.any(),
            browser_detached: self.browser_detached(),
            sequencer_only: self.in_sequencer_only(),
            editor: !self.lower_is_console()
                && !matches!(self.lower_pane(), Pane::Arrangement | Pane::Console),
            missing_audio: !self.core.missing_assets().is_empty(),
            archive_busy: self.core.archive_busy(),
            video_busy: self.core.video_busy(),
            video_clip: self.core.video_clip().is_some(),
            video_audio: self.core.video_audio_kept().is_some(),
            video_audio_keep: self.core.video_audio_kept() == Some(true),
            video_import_audio: self.ui.video_import_audio,
            video_missing: self.core.video_missing(),
            collect_busy: self.core.collect_busy(),
            default_song: self.workflow.default_song.is_some(),
            midi_clip: self.core.can_export_midi(MidiExport::Clip),
            midi_track: self.core.can_export_midi(MidiExport::Track),
            midi_song: self.core.can_export_midi(MidiExport::Song),
            export_song: self.core.can_export_audio(ExportScope::Song),
            export_loop: self.core.can_export_audio(ExportScope::Loop),
            export_selection: self.core.can_export_audio(ExportScope::Selection),
            export_stems: self.core.can_export_audio(ExportScope::Stems),
            org: self.core.arrangement().org_menu(),
            lane: self.core.arrangement().lane_menu(),
            audio: self.core.arrangement().audio_menu(),
            vca: self.core.vca_menu(self.live_pane(self.focus)),
            strip: self.core.strip_menu(self.live_pane(self.focus)),
            snapshots: self.core.snapshot_menu(),
            reference: self.core.reference_menu(),
            midi: self.core.midi_menu(),
            freeze: self.core.freeze_menu(),
            follow_selection: self.core.follow_selection(),
            theme: self.workflow.theme,
            app_zoom: self.workflow.app_zoom,
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

    /// The Track Panel column, when it shows: on the right of the window, above the status
    /// bar, narrower when the window leaves the rest less than [`MIN_REST_WIDTH`].
    fn track_panel_rect(&self) -> Option<Rect> {
        if !self.core.track_panel_visible() {
            return None;
        }
        let left = self.file_browser_rect().map_or(0.0, |r| r.x1);
        let top = self.top();
        let bottom = (self.size().height - STATUS_BAR_HEIGHT).max(top + 1.0);
        let room = (self.size().width - left - MIN_REST_WIDTH).max(0.0);
        let width = TRACK_PANEL_WIDTH.min(room);
        (width >= MIN_TRACK_PANEL_WIDTH)
            .then(|| Rect::new(self.size().width - width, top, self.size().width, bottom))
    }

    /// The file browser column, when it shows: the full height below the menu bar. Its width is
    /// the panel's own (it resizes by drag), kept so the rest of the window keeps
    /// [`MIN_REST_WIDTH`].
    fn file_browser_rect(&self) -> Option<Rect> {
        if !self.ui.file_browser_visible || self.browser_detached() {
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
        let tp = self.track_panel_rect();
        let x0 = fb.map_or(0.0, |r| r.x1);
        let w = (tp.map_or(self.size().width, |r| r.x0) - x0).max(1.0);
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
                track_panel: tp,
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
                track_panel: tp,
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
                track_panel: tp,
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
            track_panel: tp,
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
        } else if self.browser_detached()
            && let Some((_, rect)) = self.pane_rect(Pane::FileBrowser)
        {
            self.core.file_browser_mut().set_bounds(rect);
        }
        if let Some((_, rect)) = self.pane_rect(Pane::PianoKeys) {
            self.core.piano_keys_mut().set_bounds(rect);
        }
        if let Some(panel) = l.track_panel {
            self.core.track_panel_mut().set_bounds(panel);
        }
        self.browser_edge = l.file_browser.map_or(0.0, |r| r.x1);
        if let Some(song) = l.song {
            self.core.arrangement_mut().set_bounds(song);
            self.core.piano_roll_mut().set_bounds(song);
            self.core.sample_editor_mut().set_bounds(song);
            self.core.step_editor_mut().set_bounds(song);
            self.core.slice_editor_mut().set_bounds(song);
            self.core.audio_editor_mut().set_bounds(song);
            self.core.seq_editor_mut().set_bounds(song);
            self.core.container_editor_mut().set_bounds(song);
            self.core.plugin_params_mut().set_bounds(song);
            if !self.console_detached() {
                self.core.console_mut().set_bounds(song);
            }
        }
        if let Some((_, rect)) = self.pane_rect(Pane::SpectrumEq) {
            self.core.spectrum_eq_mut().set_bounds(rect);
        }
        if let Some((_, rect)) = self.pane_rect(Pane::Video) {
            self.core.video_window_mut().set_bounds(rect);
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
        if self.mod_matrix.is_some() {
            let rect = self.plugin_modal_rect();
            if let Some(v) = &mut self.mod_matrix {
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
        if self.scale.is_some() {
            let rect = self.plugin_modal_rect();
            if let Some(v) = &mut self.scale {
                v.set_bounds(rect);
            }
        }
        if self.blind.is_some() {
            let rect = self.plugin_modal_rect();
            if let Some(v) = &mut self.blind {
                v.set_bounds(rect);
            }
        }
        if self.silence.is_some() {
            let rect = self.plugin_modal_rect();
            if let Some(v) = &mut self.silence {
                v.set_bounds(rect);
            }
        }
        if self.song_info.is_some() {
            let rect = self.plugin_modal_rect();
            if let Some(v) = &mut self.song_info {
                v.set_bounds(rect);
            }
        }
        if self.reverb_send.is_some() {
            let rect = self.plugin_modal_rect();
            if let Some(v) = &mut self.reverb_send {
                v.set_bounds(rect);
            }
        }
        if self.kick_bass.is_some() {
            let rect = self.plugin_modal_rect();
            if let Some(v) = &mut self.kick_bass {
                v.set_bounds(rect);
            }
        }
        if self.zone.is_some() {
            let rect = self.plugin_modal_rect();
            if let Some(v) = &mut self.zone {
                v.set_bounds(rect);
            }
        }
        if self.versions.is_some() {
            let rect = self.plugin_modal_rect();
            if let Some(v) = &mut self.versions {
                v.set_bounds(rect);
            }
        }
        if self.masking.is_some() {
            let rect = self.plugin_modal_rect();
            if let Some(v) = &mut self.masking {
                v.set_bounds(rect);
            }
        }
        if self.mixcheck.is_some() {
            let rect = self.plugin_modal_rect();
            if let Some(v) = &mut self.mixcheck {
                v.set_bounds(rect);
            }
        }
        if self.repetition.is_some() {
            let rect = self.plugin_modal_rect();
            if let Some(v) = &mut self.repetition {
                v.set_bounds(rect);
            }
        }
        if self.bass_read.is_some() {
            let rect = self.plugin_modal_rect();
            if let Some(v) = &mut self.bass_read {
                v.set_bounds(rect);
            }
        }
        if self.delivery.is_some() {
            let rect = self.plugin_modal_rect();
            if let Some(v) = &mut self.delivery {
                v.set_bounds(rect);
            }
        }
        if self.sound_analyst.is_some() {
            let rect = self.plugin_modal_rect();
            if let Some(v) = &mut self.sound_analyst {
                v.set_bounds(rect);
            }
        }
        if self.wavetable.is_some() {
            let rect = self.plugin_modal_rect();
            if let Some((v, _)) = &mut self.wavetable {
                v.set_bounds(rect);
            }
        }
        if self.color_picker.is_some() {
            let rect = self.plugin_modal_rect();
            if let Some((v, _)) = &mut self.color_picker {
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
        } else if self.core.piano_roll_open() {
            Pane::PianoRoll
        } else if self.core.audio_editor_clip().is_some() {
            Pane::AudioEditor
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
            Pane::FileBrowser if self.browser_detached() => Pane::FileBrowser,
            Pane::Arrangement
            | Pane::PianoRoll
            | Pane::Console
            | Pane::SampleEditor
            | Pane::StepEditor
            | Pane::SliceEditor
            | Pane::AudioEditor
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
            | Pane::AudioEditor
            | Pane::SeqEditor
            | Pane::ContainerEditor
            | Pane::PluginParams => self.song_pane(),
            Pane::FileBrowser if !self.ui.file_browser_visible => self.live_pane(Pane::Rack),
            Pane::PianoKeys if !self.detached(WindowKind::PianoKeys) => self.live_pane(Pane::Rack),
            Pane::TrackPanel if !self.core.track_panel_visible() => self.live_pane(Pane::Rack),
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
            .audio_editor_mut()
            .set_focused(pane == Pane::AudioEditor);
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
        let released = self
            .core
            .piano_keys_mut()
            .set_focused(pane == Pane::PianoKeys);
        for action in released {
            self.core.handle_action(action);
        }
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
                WindowKind::SpectrumEq => Pane::SpectrumEq,
                WindowKind::Browser => Pane::FileBrowser,
                WindowKind::PianoKeys => Pane::PianoKeys,
                WindowKind::Video => Pane::Video,
                _ => Pane::Rack,
            };
            let (_, rect) = self.pane_rect(pane)?;
            return rect.contains(pos).then_some(Ok(pane));
        }
        let l = self.layout();
        if l.track_panel.is_some_and(|r| r.contains(pos)) {
            Some(Ok(Pane::TrackPanel))
        } else if l.file_browser.is_some_and(|r| r.contains(pos)) {
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
        if self.sound_analyst.is_some() {
            // The open Sound Analyst takes the files: the first audio file is analysed, nothing
            // is imported behind it.
            if let UiEvent::FileDrop { paths, .. } = event {
                match paths
                    .iter()
                    .find(|p| !crate::midi_file::is_midi_path(p) && !is_project_item(p))
                {
                    Some(path) => self.analyse_sound_file(path),
                    None => self.notify("Drop an audio file on the panel to analyse it"),
                }
            }
            return;
        }
        if self.quantize.is_some() {
            // The open quantize panel takes the files: a MIDI file becomes a groove, nothing
            // else is imported behind it.
            if let UiEvent::FileDrop { paths, .. } = event {
                match paths.iter().find(|p| crate::midi_file::is_midi_path(p)) {
                    Some(path) => self.groove_from_midi_file(path),
                    None => self.notify("Drop a MIDI file on the panel to make a groove from it"),
                }
            }
            return;
        }
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
                // Library items hover only over the panes that take them, MIDI files only over
                // the arrangement (anywhere else they import at the playhead on release).
                let takes = if paths.iter().any(|p| crate::midi_file::is_midi_path(p)) {
                    target == Some(Pane::Arrangement)
                } else {
                    matches!(target, Some(Pane::Rack | Pane::Arrangement))
                        || !paths.iter().any(|p| is_library_item(p))
                };
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
                if !midi.is_empty() {
                    self.drop_midi(target, *pos, midi);
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
                // The copy modifier held at the drop adds a zone to a granular sampler instead
                // of replacing its sample.
                let adds =
                    self.roles
                        .is_active(Role::CopyDrag, self.modifiers, GestureClass::DragItem);
                self.core.set_drop_adds(adds);
                let taken = target.is_some_and(|pane| {
                    let actions = self.forward(pane, event);
                    pane != Pane::Rack
                        || actions
                            .iter()
                            .any(|a| matches!(a, UiAction::ImportSamples { .. }))
                });
                self.core.set_drop_adds(false);
                if !taken {
                    self.core.import_at_playhead(paths.clone());
                }
            }
            _ => {}
        }
    }

    /// MIDI files dropped at `pos`: the arrangement places them where they land (a clip on a
    /// note lane, new tracks elsewhere in the track area); a drop anywhere else, and one on the
    /// arrangement's ruler or corner, makes new tracks at the snapped playhead. The rack takes
    /// no part: a drop on an instrument there is a drop at the playhead too.
    fn drop_midi(&mut self, target: Option<Pane>, pos: Point, midi: Vec<PathBuf>) {
        let placed = target == Some(Pane::Arrangement) && {
            let event = UiEvent::FileDrop {
                pos,
                paths: midi.clone(),
            };
            self.forward(Pane::Arrangement, &event)
                .iter()
                .any(|a| matches!(a, UiAction::ImportMidi { .. }))
        };
        if !placed {
            for path in &midi {
                self.core.import_midi_tracks(path);
            }
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
                UiAction::InsertLibrary { .. }
                    | UiAction::InsertLibraryClip { .. }
                    | UiAction::ReplaceWithLibrary { .. }
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

    /// A row of the device browser dragged out of the rack over a track header of the
    /// arrangement: the header of a track whose device the row can replace is outlined while
    /// the pointer is on it, and on release the device and what replaces it are returned (the
    /// caller does the replacement after the rack has seen the release). Nothing when no row
    /// is being dragged, or the pointer is elsewhere.
    fn browser_row_over_header(
        &mut self,
        pos: Point,
        released: bool,
    ) -> Option<(DeviceKey, atomik_model::CreateSource)> {
        let over = self.capture_over == Some(Pane::Arrangement);
        let source = self.core.view().dragged_replacement();
        let device = self
            .core
            .header_replace_hover((over && source.is_some()).then_some(pos), source.as_ref());
        if released {
            self.core.header_replace_hover(None, None);
            return device.zip(source);
        }
        self.repaint = true;
        None
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

    /// Saves the device a patch dialog was opened for as the patch item `path` (the platform
    /// dialog confirmed an overwrite). See [`ShellRequest::PickSavePatch`].
    pub fn save_patch_to(&mut self, path: &Path) {
        if let Some(device) = self.patch_device.take() {
            self.core.save_device_patch(&device, path);
        }
        self.repaint = true;
    }

    /// Starts saving `device` as a patch: the platform's save dialog asks for the name.
    fn begin_save_patch(&mut self, device: DeviceKey) {
        match self.core.patch_save_suggestion(&device) {
            Some((dir, name)) => {
                self.patch_device = Some(device);
                self.pending.push(ShellRequest::PickSavePatch { dir, name });
            }
            None => self
                .core
                .report("There is no library folder: choose one in Settings > Workflow"),
        }
    }

    /// Aims the file browser at `device`: it lists the patches for its type and applies them as
    /// auditions (see `AppCore::begin_patch_browse`).
    fn browse_patches(&mut self, device: &DeviceKey) {
        self.set_file_browser_visible(true);
        self.core.begin_patch_browse(device);
        self.ui_changed();
    }

    /// Return accepts and Escape cancels the patch audition while the patch browser is aimed at
    /// a device, whichever pane has the keys (the file browser decides these two itself, from
    /// the row it has selected, and a text field keeps them). True when the key was used.
    fn patch_browse_key(&mut self, key: &Key) -> bool {
        if self.core.patch_browse_device().is_none()
            || self.live_pane(self.focus) == Pane::FileBrowser
            || self.context_stack().contexts().contains(&Context::Text)
        {
            return false;
        }
        match key {
            Key::Enter => self.core.commit_patch_browse(),
            Key::Escape => self.core.cancel_patch_browse(),
            _ => return false,
        }
        self.ui_changed();
        true
    }

    /// Switches the loudness match of the selected effects.
    fn toggle_match_of_selection(&mut self) {
        let keys = self.core.view().selection();
        self.core.toggle_match(&keys);
    }

    /// Previous (`-1`) or next (`1`) patch for the device the patch browser is aimed at, else the
    /// primary selected device.
    fn step_patch_of_selection(&mut self, delta: i32) {
        let device = self
            .core
            .patch_browse_device()
            .cloned()
            .or_else(|| self.core.view().primary());
        match device {
            Some(device) => self.core.step_patch(&device, delta),
            None => self.core.report("Select a device first"),
        }
    }

    /// Opens the pop-up of the patches of `device`'s type in its folder, under its patch name
    /// (the current one checked). A choice runs `rack.patch_pick` with the file.
    fn open_patch_siblings(&mut self, device: &DeviceKey) {
        let (items, current) = self.core.patch_siblings(device);
        let can_clean = self.core.can_clean_up_bass(device);
        let can_split = self.core.can_split_sub(device);
        if items.is_empty() && !can_clean && !can_split {
            self.core
                .report("There are no patches for this device type in the folder");
            return;
        }
        let Some(at) = self
            .core
            .view()
            .patch_strip_center(device, atomik_ui::PatchOp::Browse)
        else {
            return;
        };
        let at = self.rack_window_point(at);
        let entries = context_menus::patch_entries(&items, current, can_clean, can_split);
        self.patch_menu = Some(device.clone());
        let (window, size) = self.menu_window(Pane::Rack);
        let menu = ContextMenu::from_entries(entries, at, size);
        self.show_context_menu(Pane::Rack, window, menu);
    }

    /// Opens the pop-up of the output ports under the plate of the MIDI Out device `device`, the
    /// device's own port checked (or the default). A choice runs `rack.midi_out_pick`.
    fn open_midi_out_ports(&mut self, device: &DeviceKey) {
        let Some(current) = self.core.midi_out_port_of(device) else {
            self.core.report("Select a MIDI Out device first");
            return;
        };
        let ports = self
            .core
            .sink()
            .backend()
            .list_midi_out()
            .unwrap_or_default();
        let Some(at) = self
            .core
            .view()
            .patch_strip_center(device, atomik_ui::PatchOp::Browse)
        else {
            return;
        };
        let at = self.rack_window_point(at);
        let entries = context_menus::midi_out_entries(&ports, current.as_deref());
        self.midi_out_menu = Some(device.clone());
        let (window, size) = self.menu_window(Pane::Rack);
        let menu = ContextMenu::from_entries(entries, at, size);
        self.show_context_menu(Pane::Rack, window, menu);
    }

    /// A port chosen in the pop-up of a MIDI Out device (see [`Self::open_midi_out_ports`]); an
    /// empty name is the default.
    fn pick_midi_out_port(&mut self, port: &str) {
        let device = self
            .midi_out_menu
            .take()
            .or_else(|| self.core.view().primary());
        match device {
            Some(device) => {
                let port = (!port.is_empty()).then(|| port.to_owned());
                self.core.set_midi_out_port(&device, port);
            }
            None => self.core.report("Select a MIDI Out device first"),
        }
    }

    /// A patch chosen in the pop-up of a patch name (see [`Self::open_patch_siblings`]).
    fn pick_patch_from_menu(&mut self, path: &str) {
        let device = self
            .patch_menu
            .take()
            .or_else(|| self.core.view().primary());
        match device {
            Some(device) => self.core.pick_patch(&device, Path::new(path)),
            None => self.core.report("Select a device first"),
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
    ///
    /// When the dialog was asked for by Import File as Take, the files become takes of that
    /// group instead.
    pub fn import_files(&mut self, paths: Vec<PathBuf>) {
        if let Some(group) = self.core.take_import_request() {
            self.core.import_takes(paths, group);
        } else {
            let (target, tick) = self.core.import_dialog_target();
            self.core.import_audio(paths, target, tick);
        }
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
            if self.live_pane(self.focus) == Pane::PianoKeys {
                let actions = self.core.piano_keys_mut().set_shift(modifiers.shift);
                for action in actions {
                    self.core.handle_action(action);
                }
            }
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
        if self.mod_matrix.is_some() {
            // The matrix panel takes every event; Esc closes its menu, then the panel.
            self.repaint = true;
            let actions = self
                .mod_matrix
                .as_mut()
                .map(|v| v.handle(&text_form(event)))
                .unwrap_or_default();
            for action in actions {
                self.mod_matrix_action(action);
            }
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
        if self.scale.is_some() {
            // The Scale Tempo panel takes every event (typed text included); Esc closes it and
            // puts the clips back.
            self.repaint = true;
            if let UiEvent::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } = event
            {
                self.scale_action(ScaleAction::Close);
                return Vec::new();
            }
            let actions = self
                .scale
                .as_mut()
                .map(|v| v.handle(&text_form(event)))
                .unwrap_or_default();
            self.scale_actions(actions);
            return Vec::new();
        }
        if self.song_info.is_some() {
            // The song information panel takes every event (typed text included); Esc closes it.
            self.repaint = true;
            if let UiEvent::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } = event
            {
                self.song_info = None;
                return Vec::new();
            }
            let actions = self
                .song_info
                .as_mut()
                .map(|v| v.handle(&text_form(event)))
                .unwrap_or_default();
            for action in actions {
                if let UiAction::SongInfo(a) = action {
                    self.song_info_action(a);
                }
            }
            return Vec::new();
        }
        if self.kick_bass.is_some() {
            // The kick and bass panel takes every event; Esc closes it.
            self.repaint = true;
            if let UiEvent::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } = event
            {
                self.kick_bass = None;
                return Vec::new();
            }
            let actions = self
                .kick_bass
                .as_mut()
                .map(|v| v.handle(&text_form(event)))
                .unwrap_or_default();
            for action in actions {
                if let UiAction::KickBass(a) = action {
                    self.kick_bass_action(a);
                }
            }
            return Vec::new();
        }
        if self.masking.is_some() {
            // The masking panel takes every event; Esc closes it.
            self.repaint = true;
            if let UiEvent::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } = event
            {
                self.masking = None;
                return Vec::new();
            }
            let actions = self
                .masking
                .as_mut()
                .map(|v| v.handle(&text_form(event)))
                .unwrap_or_default();
            for action in actions {
                if let UiAction::Masking(a) = action {
                    self.masking_action(a);
                }
            }
            return Vec::new();
        }
        if self.mixcheck.is_some() {
            // The mix check panel takes every event; Esc closes it.
            self.repaint = true;
            if let UiEvent::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } = event
            {
                self.mixcheck = None;
                return Vec::new();
            }
            let actions = self
                .mixcheck
                .as_mut()
                .map(|v| v.handle(&text_form(event)))
                .unwrap_or_default();
            for action in actions {
                if let UiAction::MixCheck(a) = action {
                    self.mixcheck_action(a);
                }
            }
            return Vec::new();
        }
        if self.bass_read.is_some() {
            // The bass-read panel takes every event; Esc closes it.
            self.repaint = true;
            if let UiEvent::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } = event
            {
                self.core.cancel_bass_read();
                self.bass_read = None;
                return Vec::new();
            }
            let actions = self
                .bass_read
                .as_mut()
                .map(|v| v.handle(&text_form(event)))
                .unwrap_or_default();
            for action in actions {
                if let UiAction::BassRead(a) = action {
                    self.bass_read_action(a);
                }
            }
            return Vec::new();
        }
        if self.delivery.is_some() {
            // The Delivery panel takes every event; Esc closes it.
            self.repaint = true;
            if let UiEvent::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } = event
            {
                self.core.cancel_delivery();
                self.delivery = None;
                return Vec::new();
            }
            let actions = self
                .delivery
                .as_mut()
                .map(|v| v.handle(&text_form(event)))
                .unwrap_or_default();
            for action in actions {
                if let UiAction::Delivery(a) = action {
                    self.delivery_action(a);
                }
            }
            return Vec::new();
        }
        if self.sound_analyst.is_some() {
            // The Sound Analyst panel takes every event; Esc closes it.
            self.repaint = true;
            if let UiEvent::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } = event
            {
                self.core.cancel_sound();
                self.sound_analyst = None;
                return Vec::new();
            }
            let actions = self
                .sound_analyst
                .as_mut()
                .map(|v| v.handle(&text_form(event)))
                .unwrap_or_default();
            for action in actions {
                if let UiAction::SoundAnalyst(a) = action {
                    self.sound_analyst_action(a);
                }
            }
            return Vec::new();
        }
        if self.repetition.is_some() {
            // The repetition panel takes every event; Esc closes it.
            self.repaint = true;
            if let UiEvent::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } = event
            {
                self.repetition = None;
                return Vec::new();
            }
            let actions = self
                .repetition
                .as_mut()
                .map(|v| v.handle(&text_form(event)))
                .unwrap_or_default();
            for action in actions {
                if let UiAction::Repetition(a) = action {
                    self.repetition_action(a);
                }
            }
            return Vec::new();
        }
        if self.zone.is_some() {
            // The decision zone panel takes every event; Esc closes it.
            self.repaint = true;
            if let UiEvent::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } = event
            {
                self.zone = None;
                return Vec::new();
            }
            let actions = self
                .zone
                .as_mut()
                .map(|v| v.handle(&text_form(event)))
                .unwrap_or_default();
            for action in actions {
                if let UiAction::Zone(a) = action {
                    self.zone_action(a);
                }
            }
            return Vec::new();
        }
        if self.versions.is_some() {
            // The versions panel takes every event (typed text included); Esc closes it.
            self.repaint = true;
            if let UiEvent::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } = event
            {
                // Esc leaves a comparison for the list first, then closes the panel.
                if !self.versions.as_mut().is_some_and(|v| v.close_report()) {
                    self.versions = None;
                }
                return Vec::new();
            }
            let actions = self
                .versions
                .as_mut()
                .map(|v| v.handle(&text_form(event)))
                .unwrap_or_default();
            for action in actions {
                if let UiAction::Versions(a) = action {
                    self.versions_action(a);
                }
            }
            return Vec::new();
        }
        if self.wavetable.is_some() {
            // The wavetable editor takes every event; Esc cancels a stroke, else closes it.
            self.repaint = true;
            let dragging = self.wavetable.as_ref().is_some_and(|(v, _)| v.dragging());
            if let UiEvent::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } = event
                && !dragging
            {
                self.close_wavetable_editor();
                return Vec::new();
            }
            let actions = self
                .wavetable
                .as_mut()
                .map(|(v, _)| v.handle(&text_form(event)))
                .unwrap_or_default();
            for action in actions {
                if let UiAction::Wavetable(a) = action {
                    self.wavetable_action(a);
                }
            }
            return Vec::new();
        }
        if self.color_picker.is_some() {
            // The colour picker takes every event (typed text included); Esc closes it.
            self.repaint = true;
            if let UiEvent::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } = event
            {
                self.color_picker = None;
                return Vec::new();
            }
            let actions = self
                .color_picker
                .as_mut()
                .map(|(v, _)| v.handle(&text_form(event)))
                .unwrap_or_default();
            for action in actions {
                if let UiAction::ColorPicker(a) = action {
                    self.color_picker_action(a);
                }
            }
            return Vec::new();
        }
        if self.blind.is_some() {
            // The blind panel takes every event; Esc ends the test.
            self.repaint = true;
            if let UiEvent::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } = event
            {
                self.blind_action(BlindAction::Close);
                return Vec::new();
            }
            let actions = self
                .blind
                .as_mut()
                .map(|v| v.handle(&text_form(event)))
                .unwrap_or_default();
            for action in actions {
                if let UiAction::Blind(a) = action {
                    self.blind_action(a);
                }
            }
            return Vec::new();
        }
        if self.reverb_send.is_some() {
            // The reverb send wizard takes every event; Esc closes it.
            self.repaint = true;
            if let UiEvent::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } = event
            {
                self.close_reverb_send();
                return Vec::new();
            }
            let actions = self
                .reverb_send
                .as_mut()
                .map(|v| v.handle(&text_form(event)))
                .unwrap_or_default();
            for action in actions {
                if let UiAction::ReverbSend(a) = action {
                    self.reverb_send_action(a);
                }
            }
            self.sync_reverb_send();
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
            let mut requests = Vec::new();
            for action in actions {
                if let UiAction::Quantize(a) = action {
                    requests.extend(self.quantize_action(a));
                }
            }
            self.sync_quantize();
            return requests;
        }
        if self.start_screen.is_some() {
            self.repaint = true;
            return self.start_screen_event(event);
        }
        let size = self.size();
        if let Some(about) = &mut self.about {
            self.repaint = true;
            return match about.handle(event, size) {
                AboutOutcome::Open => Vec::new(),
                AboutOutcome::Close => {
                    self.about = None;
                    Vec::new()
                }
                AboutOutcome::OpenUrl(url) => vec![ShellRequest::OpenUrl(url)],
            };
        }
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
                DialogOutcome::PickScratchFolder => return vec![ShellRequest::PickScratchFolder],
                DialogOutcome::PickFfmpegFolder => return vec![ShellRequest::PickFfmpegFolder],
            }
            return Vec::new();
        }
        if self.context_menu.is_some() {
            return self.context_menu_event(window, event);
        }
        let bar_open = self.menubar.as_ref().is_some_and(MenuBar::is_open);
        // The guide panel takes the pointer inside its own rectangle and nothing else: never a
        // key, so the song keeps the keyboard.
        if window == WindowKind::Main
            && self.capture.is_none()
            && self.file_drag.is_none()
            && !bar_open
            && let Some(requests) = self.guide_event(event)
        {
            return requests;
        }
        if matches!(event, UiEvent::Ime(_)) {
            if let Some(pane) = self.ime_target().filter(|_| !bar_open) {
                self.repaint = true;
                self.forward(pane, event);
            }
            return Vec::new();
        }
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
                || self.core.transport_bar().is_editing_readout()
                || self.core.transport_bar().is_editing_inspector()
            {
                self.forward(Pane::Transport, &text_form(event));
                return Vec::new();
            }
            // Esc throws a running sampling away.
            if *pressed && *key == Key::Escape && self.core.sampling_device().is_some() {
                self.core.cancel_sampling();
                return Vec::new();
            }
            if *pressed && self.patch_browse_key(key) {
                return Vec::new();
            }
            self.piano_keys_modifiers(key, *modifiers, *pressed);
            let stack = self.context_stack();
            let mods = self.keys_routing_modifiers(key, *modifiers);
            match self.router.route(key, mods, *pressed, &stack) {
                Route::RepeatIgnored => {}
                Route::Commands(list) => {
                    for resolved in &list {
                        if resolved.command.starts_with("keys.") {
                            if self.keys_command(key, &resolved.command, resolved.arg.as_deref()) {
                                return Vec::new();
                            }
                            continue;
                        }
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
                    self.capture_over = self.hit_in(window, pos).and_then(Result::ok);
                    let replaced = if pane == Pane::Rack && window == WindowKind::Main {
                        self.browser_row_over_header(pos, released)
                    } else {
                        None
                    };
                    self.forward(pane, event);
                    if let Some((device, source)) = replaced {
                        self.core.replace_device_with(device, source);
                        self.core.view_mut().browser_row_taken();
                    }
                }
                Capture::Divider { grab } => self.drag_divider(pos, grab, released),
                Capture::FileDrag => {
                    let at = under.map(|h| (h.kind, Point::new(h.logical.0, h.logical.1)));
                    self.file_drag_event(event, at);
                }
            }
            if released {
                self.capture = None;
                self.capture_over = None;
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
        let window_size = self.size();
        if let Some(bar) = &mut self.menubar {
            bar.set_window(window_size);
            let before = (bar.look(), bar.deep_look());
            let was_open = bar.is_open();
            let outcome = bar.handle(event, &model);
            // A menu that opens shows the keys as the file has them now.
            let opened = !was_open && bar.is_open();
            if opened {
                self.reload_keymap_file();
            }
            let Some(bar) = &mut self.menubar else {
                return Vec::new();
            };
            match outcome {
                MenuBarOutcome::Ignored => {}
                MenuBarOutcome::Consumed => {
                    // A move that leaves the bar as it was (the pointer inside one title)
                    // shows nothing new, and one that changes it only repaints the main
                    // window, where the bar is.
                    let moved = matches!(event, UiEvent::PointerMove { .. });
                    let changed = (bar.look(), bar.deep_look()) != before;
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
                if matches!(
                    event,
                    UiEvent::PointerDown {
                        button: PointerButton::Primary,
                        clicks: 2..,
                        ..
                    }
                ) {
                    self.focused = WindowKind::Main;
                    self.toggle_rack_collapsed();
                } else if matches!(event, UiEvent::PointerDown { .. })
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
            ("automation.clear_control", None) => {
                if let Some((device, param)) = self.control_target.clone() {
                    self.core.clear_automation_of(&device, &param);
                }
            }
            ("automation.static_control", None) => {
                if let Some((device, param)) = self.control_target.clone() {
                    self.core.set_static_from_control(&device, &param);
                }
            }
            ("sequencer.delete_automation_lane", None) => {
                if let Some((device, param)) =
                    self.core.arrangement().picked_automation_lane().cloned()
                {
                    self.core.delete_automation_lane(&device, &param);
                }
            }
            ("sequencer.mute_automation_lane", None) => {
                if let Some((device, param)) =
                    self.core.arrangement().picked_automation_lane().cloned()
                {
                    self.core.toggle_automation_lane_mute(&device, &param);
                }
            }
            ("sequencer.set_lane_static", None) => {
                if let Some((device, param)) =
                    self.core.arrangement().picked_automation_lane().cloned()
                {
                    self.core.set_static_from_control(&device, &param);
                }
            }
            ("sequencer.clear_lane_static", None) => {
                if let Some((device, param)) =
                    self.core.arrangement().picked_automation_lane().cloned()
                {
                    self.core.clear_lane_static(&device, &param);
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
            ("rack.jack", Some(op)) => {
                self.repaint = true;
                self.core.jack_op(op);
            }
            ("create.device", Some(type_id)) => self.core.create_device_of_type(type_id),
            ("rack.replace_device", Some(type_id)) => self.core.replace_selected_device(type_id),
            ("sequencer.audio_normalise", None) => self.core.normalise_audio(),
            ("sequencer.audio_strip_silence", None) => self.open_silence(),
            ("sequencer.add_reverb_send", None) => self.open_reverb_send(),
            ("sequencer.scale_tempo", Some("dialog")) => self.open_scale(),
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
            ("transport.loop_move", Some(arg)) => {
                let (kind, step) = arg.split_once(':')?;
                let bars: i64 = step.parse().ok()?;
                match kind {
                    "bar" => self.core.move_loop(bars, false),
                    "length" => self.core.move_loop(bars, true),
                    _ => return None,
                }
            }
            ("transport.all_notes_off", None) => self.core.all_notes_off(),
            // Escape closes the open clip or editor, unless a menu or a drag would take it.
            ("sequencer.close_clip", None) => {
                if self.core.arrangement().escape_pending() || !self.core.close_open_editor() {
                    return None;
                }
                self.set_focus(self.live_pane(Pane::Arrangement));
                self.layout_view();
            }
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
            ("rack.save_patch", None) => match self.core.view().primary() {
                Some(device) => self.begin_save_patch(device),
                None => self.core.report("Select a device first"),
            },
            ("rack.reset_device", None) => self.core.reset_selected_device(),
            ("rack.clean_up_bass", None) => {
                self.patch_menu = None;
                self.core.clean_up_bass();
            }
            ("rack.split_sub", None) => {
                self.patch_menu = None;
                self.core.split_sub();
            }
            ("rack.preview_player", None) => self.core.preview_selected_player(),
            ("rack.combine", None) => {
                self.core.combine_selection();
            }
            ("rack.uncombine", None) => {
                self.core.uncombine_selection();
            }
            ("rack.move_out", None) => {
                self.core.move_out_selection();
            }
            ("rack.browse_patch", None) => match self.core.view().primary() {
                Some(device) => self.browse_patches(&device),
                None => self.core.report("Select a device first"),
            },
            ("rack.patch_next", None) => self.step_patch_of_selection(1),
            ("rack.patch_prev", None) => self.step_patch_of_selection(-1),
            ("rack.patch_pick", Some(path)) => self.pick_patch_from_menu(path),
            ("rack.midi_out_port", None) => match self.core.view().primary() {
                Some(device) => self.open_midi_out_ports(&device),
                None => self.core.report("Select a MIDI Out device first"),
            },
            ("rack.midi_out_pick", Some(port)) => self.pick_midi_out_port(port),
            ("video.import_audio_pick", Some(stream)) => match stream.parse::<u32>() {
                // The core says why in a toast when it refuses.
                Ok(stream) => {
                    let _ = self.core.import_video_audio(Some(stream));
                }
                Err(_) => self.core.report("That is not an audio track of the video"),
            },
            ("rack.blind_ab", None) => self.open_blind(),
            ("rack.toggle_match", None) => self.toggle_match_of_selection(),
            ("rack.mod_matrix", None) => match self.core.view().primary() {
                Some(device) => self.open_mod_matrix(&device),
                None => self
                    .core
                    .report("Select a device with a modulation matrix first"),
            },
            ("edit.copy_settings", None) => self.core.copy_settings(),
            ("edit.paste_settings", None) => self.core.paste_settings(),
            ("sequencer.save_clip_to_library", None) => self.core.save_clip_to_library(),
            ("view.cables_cycle", None) => {
                let view = self.core.view_mut();
                let next = view.cable_visibility().next();
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
            self.ui_changed();
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
        self.ui_changed();
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
        self.ui_changed();
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
                .find(|id| {
                    song.clip(*id)
                        .is_some_and(|(_, _, c)| c.content.is_notes() || c.content.is_audio())
                })
        };
        if let Some(id) = selected {
            self.reveal_lower_pane();
            let is_audio = self
                .core
                .doc()
                .song
                .clip(id)
                .is_some_and(|(_, _, c)| c.content.is_audio());
            let action = if is_audio {
                UiAction::OpenAudioEditor(id)
            } else {
                UiAction::OpenPianoRoll(id)
            };
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
    /// Opens or closes the Piano Keys window. Opening gives it the keyboard focus; closing
    /// releases its notes.
    fn set_piano_keys_visible(&mut self, visible: bool) {
        self.set_detached(WindowKind::PianoKeys, visible);
    }

    /// Shift is the sustain pedal while the strip types: tell it what the key event says.
    fn piano_keys_modifiers(&mut self, key: &Key, modifiers: Modifiers, pressed: bool) {
        if self.live_pane(self.focus) != Pane::PianoKeys {
            return;
        }
        let mut actions = self.core.piano_keys_mut().set_shift(modifiers.shift);
        // A computer key came up: its note ends (the router sees the release as well).
        if !pressed {
            actions.extend(self.core.piano_keys_mut().type_release(key));
        }
        for action in actions {
            self.core.handle_action(action);
        }
    }

    /// The modifiers a key is resolved with: while the strip types, Shift is the sustain pedal
    /// and does not change which key a letter is, so it is left out when the key plays
    /// something without it.
    fn keys_routing_modifiers(&self, key: &Key, modifiers: Modifiers) -> Modifiers {
        if !modifiers.shift
            || !self.core.piano_keys().is_typing()
            || self.live_pane(self.focus) != Pane::PianoKeys
        {
            return modifiers;
        }
        let plain = Modifiers {
            shift: false,
            ..modifiers
        };
        let keys = ContextStack::new(&[Context::Keys]);
        match self.router.keymap().resolve_event(key, plain, &keys) {
            Resolution::Bound(list) if list.iter().any(|r| r.context == Context::Keys) => plain,
            _ => modifiers,
        }
    }

    /// Runs a `keys.*` command for the key that triggered it; false when it is not one.
    fn keys_command(&mut self, key: &Key, command: &str, arg: Option<&str>) -> bool {
        let keys = self.core.piano_keys_mut();
        let actions = match command {
            "keys.note" => {
                let offset = arg.and_then(|a| KEYS_NOTE_ARGS.iter().position(|n| *n == a));
                match offset {
                    Some(o) => keys.type_note(key, o as u8),
                    None => return false,
                }
            }
            "keys.octave_up" => {
                keys.shift_octave(1);
                Vec::new()
            }
            "keys.octave_down" => {
                keys.shift_octave(-1);
                Vec::new()
            }
            "keys.velocity" => {
                match arg.and_then(|a| a.parse::<usize>().ok()) {
                    Some(step) => keys.set_velocity_step(step),
                    None => return false,
                }
                Vec::new()
            }
            _ => return false,
        };
        for action in actions {
            self.core.handle_action(action);
        }
        true
    }

    /// The characters the keymap gives the typed keys, drawn on the strip.
    fn refresh_key_labels(&mut self) {
        let keymap = self.router.keymap();
        let labels: Vec<String> = KEYS_NOTE_ARGS
            .iter()
            .map(|arg| {
                keymap
                    .chords_of("keys.note", Some(arg))
                    .into_iter()
                    .find(|(ctx, _)| *ctx == Context::Keys)
                    .map(|(_, c)| c.menu_text(keymap.os()))
                    .unwrap_or_default()
            })
            .collect();
        self.core.piano_keys_mut().set_labels(labels);
    }

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
        let now = (avail * self.ui.rack_split).clamp(lo, avail - lo);
        let wanted = rack_h.clamp(lo, avail - lo);
        // A press that did not move the divider (the first click of a double-click) leaves the
        // split, and what a collapse remembered, as they are.
        if (wanted - now).abs() >= 0.5 {
            self.ui.rack_split = wanted / avail;
            self.split_before = None;
        }
        self.layout_view();
        if released {
            self.ui_changed();
        }
    }

    /// A double-click on the divider: the rack collapses to its smallest height, so the song
    /// area has the room; the next double-click puts the split back.
    fn toggle_rack_collapsed(&mut self) {
        match self.split_before.take() {
            Some(split) => self.ui.rack_split = split,
            None => {
                self.split_before = Some(self.ui.rack_split);
                self.ui.rack_split = 0.0;
            }
        }
        self.layout_view();
        self.ui_changed();
    }

    /// The size and overlay choices of the Video window are saved when they changed.
    fn save_video_prefs(&mut self) {
        let prefs = self.core.video_window().prefs();
        let (size, timecode, frame_number) = (
            prefs.size,
            prefs.overlay.timecode,
            prefs.overlay.frame_number,
        );
        if size != self.ui.video_size
            || timecode != self.ui.video_timecode
            || frame_number != self.ui.video_frame_number
        {
            self.ui.video_size = size;
            self.ui.video_timecode = timecode;
            self.ui.video_frame_number = frame_number;
            self.ui_changed();
        }
    }

    /// The vertical zooms (track heights, key height) and the piano roll's snap to scale and
    /// fold are saved when an event changed them.
    fn save_zoom_state(&mut self) {
        let zoom = self.core.arrangement().track_zoom();
        let roll = self.core.piano_roll();
        let (keys, snap_scale, fold) = (roll.key_height(), roll.snap_to_scale(), roll.fold());
        let audio_mode = self.core.audio_editor().mode();
        let (arrangement_snap, roll_snap) = (self.core.arrangement().snap(), roll.snap());
        // The two views share one mode: whichever changed last wins.
        let (arr_mode, roll_mode) = (self.core.arrangement().snap_mode(), roll.snap_mode());
        let snap_mode = if arr_mode != self.ui.snap_mode {
            arr_mode
        } else {
            roll_mode
        };
        let strip = self.core.piano_keys();
        let (octave, velocity) = (strip.octave(), strip.velocity());
        if octave != self.ui.piano_keys_octave || velocity != self.ui.piano_keys_velocity {
            self.ui.piano_keys_octave = octave;
            self.ui.piano_keys_velocity = velocity;
            self.ui_changed();
        }
        self.save_video_prefs();
        let fade_unit = self.core.transport_bar().fade_unit();
        if fade_unit != self.ui.fade_unit {
            self.ui.fade_unit = fade_unit;
            self.ui_changed();
        }
        let audition = self.core.piano_roll().note_audition_on();
        if audition != self.ui.piano_roll_audition {
            self.ui.piano_roll_audition = audition;
            self.ui_changed();
        }
        if arr_mode != snap_mode {
            self.core.arrangement_mut().set_snap_mode(snap_mode);
        }
        if roll_mode != snap_mode {
            self.core.piano_roll_mut().set_snap_mode(snap_mode);
        }
        if arrangement_snap != self.ui.arrangement_snap
            || roll_snap != self.ui.piano_roll_snap
            || snap_mode != self.ui.snap_mode
        {
            self.ui.arrangement_snap = arrangement_snap;
            self.ui.piano_roll_snap = roll_snap;
            self.ui.snap_mode = snap_mode;
            self.ui_changed();
        }
        if (zoom - self.ui.track_zoom).abs() > f64::EPSILON
            || (keys - self.ui.piano_roll_key_height).abs() > f64::EPSILON
            || snap_scale != self.ui.piano_roll_snap_scale
            || fold != self.ui.piano_roll_fold
            || audio_mode != self.ui.audio_editor_mode
        {
            self.ui.track_zoom = zoom;
            self.ui.piano_roll_key_height = keys;
            self.ui.piano_roll_snap_scale = snap_scale;
            self.ui.piano_roll_fold = fold;
            self.ui.audio_editor_mode = audio_mode;
            self.ui_changed();
        }
        let collapsed = self.core.arrangement().video_row_collapsed();
        if collapsed != self.ui.video_row_collapsed {
            self.ui.video_row_collapsed = collapsed;
            self.ui_changed();
        }
    }

    /// The saved interface state changed: written by the next [`Self::flush_geometry`] (at most
    /// every two seconds, so a run of clicks writes once), at the latest on shutdown.
    fn ui_changed(&mut self) {
        self.geom_dirty = true;
    }

    /// Writes `ui-state.json` now when something is waiting for the next flush.
    pub fn flush_ui_state(&mut self) {
        if self.geom_dirty {
            self.write_ui_state();
        }
    }

    /// Writes `ui-state.json` now (atomically); anything waiting for the next flush is in it.
    fn write_ui_state(&mut self) {
        self.geom_dirty = false;
        let view = self.core.file_browser();
        self.ui.file_browser_width = view.width();
        self.ui.file_browser_tree_fraction = view.tree_fraction();
        self.ui.file_browser_auto_preview = view.auto_preview();
        self.ui.patch_folders.clone_from(self.core.patch_folders());
        self.ui
            .plugin_windows
            .clone_from(self.core.plugin_window_geoms());
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
            paste_settings: pane == Pane::Rack && core.can_paste_settings(),
            has_track: core
                .view()
                .primary()
                .is_some_and(|d| doc.song.track(&d).is_some()),
            join: clips.len() >= 2,
            clip_one: clips.len() == 1,
            clip_looped: clips.iter().any(|id| {
                doc.song
                    .clip(*id)
                    .is_some_and(|(_, _, c)| c.loop_length.is_some())
            }),
            loop_instance: matches!(clips.as_slice(), [id]
                if doc.song.clip(*id).is_some_and(|(_, _, c)| c.loop_length.is_some())),
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
                    .is_some_and(|(_, _, c)| c.content.is_notes() || c.content.is_audio())
            }),
            master_only: pane == Pane::Rack
                && core.view().selection_len() == 1
                && doc.console.master.is_some()
                && core.view().primary() == doc.console.master,
            mod_matrix: pane == Pane::Rack
                && core
                    .view()
                    .primary()
                    .is_some_and(|d| core.has_mod_matrix(&d)),
            effect: pane == Pane::Rack && core.view().primary().is_some_and(|d| core.has_match(&d)),
            matching: pane == Pane::Rack
                && core.view().primary().is_some_and(|d| core.match_on(&d)),
            player: pane == Pane::Rack
                && core
                    .view()
                    .primary()
                    .is_some_and(|d| core.is_player_device(&d)),
            previewing: core.previewing_player().is_some(),
            midi_out: pane == Pane::Rack
                && core
                    .view()
                    .primary()
                    .is_some_and(|d| core.midi_out_port_of(&d).is_some()),
            assign_send: pane == Pane::Rack
                && core
                    .view()
                    .assign_send_reason(doc, core.registry())
                    .is_none(),
            combine: pane == Pane::Rack
                && core.view().combine_reason(doc, core.registry()).is_none(),
            uncombine: pane == Pane::Rack
                && core.view().uncombine_reason(doc, core.registry()).is_none(),
            move_out: pane == Pane::Rack
                && core.view().move_out_reason(doc, core.registry()).is_none(),
            in_container: pane == Pane::Rack && core.view().scope_depth() > 0,
            columns: doc.columns,
            track_can_chase: core.arrangement().chase_menu().0,
            track_chase: core.arrangement().chase_menu().1,
            lane: core.arrangement().lane_menu(),
            org: core.arrangement().org_menu(),
            audio: core.arrangement().audio_menu(),
            freeze: core.freeze_menu(),
            lens_small: core.lens().voicing == atomik_engine::LensVoicing::SmallSpeaker,
            lens_club: core.lens().voicing == atomik_engine::LensVoicing::ClubSub,
            lens_mono: core.lens().mono,
            lens_off: core.lens().is_neutral(),
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
            control_has_lane: self
                .control_target
                .as_ref()
                .is_some_and(|(d, p)| core.has_lane_to_clear(d, p)),
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
        let mut specs = context_menus::specs(target, &enable);
        // The rack menus list the device types by family.
        if pane == Pane::Rack {
            specs = context_menus::with_device_menus(target, specs, &self.core.device_menus());
        }
        // It opens in the window that shows the pane, sized to that window.
        let (window, size) = self.menu_window(pane);
        let menu = ContextMenu::build(
            &specs,
            &atomik_ui::keymap::Registry::bundled(),
            self.router.keymap(),
            at,
            size,
        );
        self.show_context_menu(pane, window, menu);
    }

    /// Opens the pop-up menu of jack `jack` at the window point `at`: the rows come from the
    /// rack view's [`atomik_ui::JackMenu`], so they hold the project's own device names.
    fn open_jack_menu(&mut self, jack: &atomik_model::SocketRef, at: Point) {
        let Some(menu) = self
            .core
            .view()
            .jack_menu(jack, self.core.doc(), self.core.registry())
        else {
            return;
        };
        let (window, size) = self.menu_window(Pane::Rack);
        let menu = ContextMenu::from_entries(context_menus::jack_entries(&menu), at, size);
        self.show_context_menu(Pane::Rack, window, menu);
    }

    /// The window that shows `pane` and its size: where a menu of the pane opens.
    fn menu_window(&self, pane: Pane) -> (WindowKind, Size) {
        let window = self.window_of(pane);
        (
            window,
            self.window_size(window).unwrap_or_else(|| self.size()),
        )
    }

    /// The rack view's point `at` in window coordinates (the rack sits below the menu bar).
    fn rack_window_point(&self, at: Point) -> Point {
        let o = self
            .pane_rect(Pane::Rack)
            .map_or(Point::ORIGIN, |(_, r)| r.origin());
        Point::new(at.x + o.x, at.y + o.y)
    }

    /// Shows `menu` as the open context menu of `pane`, in `window`.
    fn show_context_menu(&mut self, pane: Pane, window: WindowKind, menu: ContextMenu) {
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

    /// Whether the context menu wants a frame now (a switch not yet timed, a scroll strip under
    /// the pointer): the platform loop keeps ticking while it does.
    pub fn context_menu_pending(&self) -> bool {
        self.context_menu
            .as_ref()
            .is_some_and(|o| o.menu.needs_tick())
    }

    /// When a submenu switch of the context menu is due, on the clock of `frame_at` (seconds):
    /// the loop sleeps until then and ticks, instead of polling.
    pub fn context_menu_wake(&self) -> Option<f64> {
        self.context_menu.as_ref().and_then(|o| o.menu.wake_at())
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
                UiAction::JackMenu { socket, at } => {
                    let at = self.rack_window_point(*at);
                    self.open_jack_menu(socket, at);
                    continue;
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
                UiAction::ImportAudioDialog => {
                    self.core.set_take_import(None);
                    self.pending.push(ShellRequest::PickImportFiles);
                }
                // Import File as Take: the next file the dialog returns becomes a take.
                UiAction::ImportTake { group, notes } => {
                    self.core.set_take_import(Some(*group));
                    self.pending.push(if *notes {
                        ShellRequest::PickImportMidi { as_song: false }
                    } else {
                        ShellRequest::PickImportFiles
                    });
                }
                // Alt-click on a control: its automation lane, as the control menu's item.
                UiAction::EditAutomation { device, param } => {
                    if self.core.show_lane_of(device, param) {
                        self.show_arrangement_focus();
                    }
                }
                // The piano roll's QUANT button: the panel's last settings, on the roll.
                UiAction::Quantize(QuantizeAction::ApplyLast) => {
                    // From the transport bar too: the open roll, else the arrangement's selection.
                    let pane = if self.core.piano_roll_open() {
                        Pane::PianoRoll
                    } else {
                        Pane::Arrangement
                    };
                    self.apply_last_quantize(pane);
                }
                // The arrangement corner's QUANT button: the same, on the selected clips.
                UiAction::Quantize(QuantizeAction::ApplyClips) => {
                    self.apply_last_quantize(Pane::Arrangement);
                }
                UiAction::Quantize(QuantizeAction::Options) => self.open_quantize(),
                // Q Rec on the transport bar: the same choice as the Input Quantize menu.
                UiAction::SetInputQuantize(grid) => {
                    let q = grid.map_or(
                        crate::record::InputQuantize::OFF,
                        crate::record::InputQuantize::on,
                    );
                    self.core.set_input_quantize(q);
                    self.ui.input_quantize = q;
                    self.ui_changed();
                }
                UiAction::PickColor(target) => self.open_color_picker(target.clone()),
                UiAction::EditWavetable(device) => self.open_wavetable_editor(device.clone()),
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
                UiAction::OpenAudioEditor(_) => self.show_audio_editor(),
                UiAction::CloseAudioEditor => {
                    if self.focus == Pane::AudioEditor {
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
                    self.ui_changed();
                }
                UiAction::Browse(BrowseAction::SetAutoPreview(_)) => self.ui_changed(),
                // The level lives in Settings (workflow.json), not in the window state.
                UiAction::Browse(BrowseAction::SetPreviewLevel(_)) => {
                    self.workflow.preview_db = self.core.preview_db();
                    self.save_workflow();
                }
                UiAction::Browse(BrowseAction::Reveal(path)) => {
                    self.pending.push(ShellRequest::Reveal(path.clone()));
                }
                // The audition ended in the file browser: the folder it used is remembered.
                UiAction::Browse(BrowseAction::Load(_) | BrowseAction::Cancel) => {
                    self.ui_changed();
                }
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
                    self.ui_changed();
                }
                UiAction::RunCommand { id, arg } => {
                    if let Some(mut requests) = self.run_command(id, arg.as_deref()) {
                        self.pending.append(&mut requests);
                    }
                }
                UiAction::DevicePatch {
                    device,
                    op: atomik_ui::PatchOp::Save,
                } => self.begin_save_patch(device.clone()),
                UiAction::DevicePatch {
                    device,
                    op: atomik_ui::PatchOp::Browse,
                } => self.browse_patches(device),
                UiAction::DevicePatch {
                    device,
                    op: atomik_ui::PatchOp::Siblings,
                } => self.open_patch_siblings(device),
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

    /// Opens the pop-up of the audio tracks of a film when Import Video Audio found several: one
    /// row per track, a choice runs `video.import_audio_pick` (see
    /// [`AppCore::take_video_audio_choice`]).
    fn sync_video_ui(&mut self) {
        let Some(choice) = self.core.take_video_audio_choice() else {
            return;
        };
        let entries = context_menus::video_audio_entries(&choice.tracks);
        let (window, size) = self.menu_window(Pane::Arrangement);
        let at = Point::new(size.width * 0.5 - 140.0, size.height * 0.3);
        let menu = ContextMenu::from_entries(entries, at, size);
        self.show_context_menu(Pane::Arrangement, window, menu);
    }

    /// Opens the project `path` now, or asks first when the rack has unsaved changes.
    fn open_project_request(&mut self, path: PathBuf) {
        // A save that is still writing decides whether there is anything unsaved.
        self.core.settle_save();
        if self.core.is_dirty() {
            self.pending.push(ShellRequest::ConfirmSave {
                then: PendingAction::OpenFile(path),
            });
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

    /// The audio clip editor was opened (the core did it): make sure the lower pane shows it and
    /// give it the keys.
    fn show_audio_editor(&mut self) {
        if self.core.audio_editor_clip().is_none() {
            return;
        }
        if !self.ui.show_song || self.lower_is_console() {
            self.ui.show_song = true;
            self.ui.show_console = false;
            self.console_changed();
        }
        self.layout_view();
        self.set_focus(Pane::AudioEditor);
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
        self.core.set_patch_folders(ui.patch_folders);
        self.core.set_plugin_window_geoms(ui.plugin_windows);
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
        self.ui_changed();
    }

    fn console_changed(&mut self) {
        if !self.console_visible() && self.focus == Pane::Console {
            self.set_focus(self.live_pane(Pane::Arrangement));
        }
        self.layout_view();
        self.ui_changed();
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
            | AppCommand::NewAtRate(_)
            | AppCommand::NewFromTemplate(_)
            | AppCommand::OpenExample(_)
            | AppCommand::GuidedTour
            | AppCommand::Open
            | AppCommand::Close
            | AppCommand::CloseSongWindow
            | AppCommand::Quit
            | AppCommand::StressRack
            | AppCommand::StressSong => {
                self.core.settle_save();
                if self.core.is_dirty() {
                    vec![ShellRequest::ConfirmSave {
                        then: PendingAction::Command(command),
                    }]
                } else {
                    self.confirmed(command)
                }
            }
            AppCommand::SaveAsTemplate => {
                self.save_as_template();
                Vec::new()
            }
            AppCommand::ShowTemplateFolder => self
                .core
                .template_folder_for_show()
                .map(|dir| vec![ShellRequest::ShowFolder(dir)])
                .unwrap_or_default(),
            AppCommand::SetDefaultSong => {
                self.set_default_song();
                Vec::new()
            }
            AppCommand::ResetDefaultSong => {
                self.workflow.default_song = None;
                self.save_workflow();
                self.notify("File > New opens the starter project again".to_owned());
                Vec::new()
            }
            AppCommand::SongInfo => {
                self.open_song_info();
                Vec::new()
            }
            AppCommand::KickBassCheck => {
                self.open_kick_bass();
                Vec::new()
            }
            AppCommand::ReferencePanel => {
                self.open_zone();
                Vec::new()
            }
            AppCommand::StartScreen => {
                self.open_start_screen();
                Vec::new()
            }
            AppCommand::SaveVersion => {
                self.open_versions(false);
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::MaskingMap => {
                self.open_masking();
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::CheckMix => {
                self.open_mixcheck();
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::Versions => {
                self.open_versions(true);
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::ExportArchive => self.export_archive_request(),
            AppCommand::ImportArchive => vec![ShellRequest::PickImportArchive],
            AppCommand::CancelArchive => {
                self.core.cancel_archive();
                Vec::new()
            }
            AppCommand::PrepareVideo => vec![ShellRequest::PickVideoFile(VideoPick::Prepare)],
            AppCommand::ImportVideo => vec![ShellRequest::PickVideoFile(VideoPick::Import)],
            AppCommand::RelinkVideo => {
                if self.core.video_clip().is_some() {
                    vec![ShellRequest::PickVideoFile(VideoPick::Relink)]
                } else {
                    self.notify("The song has no video".to_owned());
                    Vec::new()
                }
            }
            AppCommand::ImportVideoAudio => {
                // The core says why in a toast when it refuses.
                let _ = self.core.import_video_audio(None);
                self.repaint = true;
                Vec::new()
            }
            AppCommand::ImportVideoWithAudio => {
                self.ui.video_import_audio = !self.ui.video_import_audio;
                self.core
                    .set_import_audio_with_video(self.ui.video_import_audio);
                self.ui_changed();
                Vec::new()
            }
            AppCommand::KeepVideoAudio => {
                self.core.toggle_keep_video_audio();
                self.repaint = true;
                Vec::new()
            }
            AppCommand::RemoveVideo => {
                self.core.remove_video();
                Vec::new()
            }
            AppCommand::MoveVideoToPlayhead => {
                self.core.move_video_to_playhead();
                Vec::new()
            }
            AppCommand::TrimVideoStart => {
                self.core.trim_video_start_to_playhead();
                Vec::new()
            }
            AppCommand::TrimVideoEnd => {
                self.core.trim_video_end_to_playhead();
                Vec::new()
            }
            AppCommand::CheckFfmpeg => {
                // The core says why in a toast when it refuses.
                let _ = self.core.check_ffmpeg();
                Vec::new()
            }
            AppCommand::CancelVideo => {
                self.core.cancel_video();
                Vec::new()
            }
            AppCommand::ConvertRate(rate) => {
                self.core.convert_sample_rate(rate);
                Vec::new()
            }
            AppCommand::SetMidiResolution(resolution) => {
                self.workflow.midi_resolution = resolution;
                self.save_workflow();
                self.core.set_midi_resolution(resolution);
                Vec::new()
            }
            AppCommand::InputChannels(n) => {
                self.core.set_input_channels(n);
                Vec::new()
            }
            AppCommand::OutputChannels(n) => {
                self.core.set_output_channels(n);
                Vec::new()
            }
            AppCommand::OpenRecent(index) => {
                self.open_recent(index);
                Vec::new()
            }
            AppCommand::ClearRecent => {
                self.core.clear_recent();
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::Save => match self.core.save_interactive() {
                Ok(()) => Vec::new(),
                Err(AppError::NoPath) => vec![ShellRequest::PickSaveFile],
                Err(e) => {
                    self.notify(e.to_string());
                    Vec::new()
                }
            },
            AppCommand::SaveAs => vec![ShellRequest::PickSaveFile],
            AppCommand::ExportPatch => vec![ShellRequest::PickExportFile],
            AppCommand::ImportAudio => {
                self.core.set_take_import(None);
                vec![ShellRequest::PickImportFiles]
            }
            AppCommand::ImportMidi => {
                self.core.set_take_import(None);
                vec![ShellRequest::PickImportMidi { as_song: false }]
            }
            AppCommand::ImportMidiSong => {
                self.core.set_take_import(None);
                vec![ShellRequest::PickImportMidi { as_song: true }]
            }
            AppCommand::ExportMidiClip => self.export_midi_request(MidiExport::Clip),
            AppCommand::ExportMidiTrack => self.export_midi_request(MidiExport::Track),
            AppCommand::ExportMidiSong => self.export_midi_request(MidiExport::Song),
            AppCommand::ExportSongAudio => self.export_audio_request(ExportScope::Song),
            AppCommand::ExportLoopAudio => self.export_audio_request(ExportScope::Loop),
            AppCommand::ExportSelectionAudio => self.export_audio_request(ExportScope::Selection),
            AppCommand::ExportStems => self.export_audio_request(ExportScope::Stems),
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
            AppCommand::ScaleTempo(ScaleStep::Other) => {
                self.open_scale();
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::GroupTracks
            | AppCommand::UngroupTracks
            | AppCommand::ToggleFolder
            | AppCommand::RenameFolder
            | AppCommand::MoveTrackUp
            | AppCommand::MoveTrackDown
            | AppCommand::RenameTrack
            | AppCommand::TrackComment
            | AppCommand::DeleteTrackAndDevice
            | AppCommand::AddNoteLane
            | AppCommand::RenameNoteLane
            | AppCommand::DeleteNoteLane
            | AppCommand::ToggleLaneMute
            | AppCommand::MergeNoteLanes
            | AppCommand::ExplodeNotes
            | AppCommand::ToggleChase
            | AppCommand::FoldLanes(_)
            | AppCommand::TrackHeight(_)
            | AppCommand::InsertBars
            | AppCommand::RemoveBars
            | AppCommand::ReverseClips(_)
            | AppCommand::ReverseClipsDefault
            | AppCommand::ScaleTempo(_)
            | AppCommand::ChopClips(_)
            | AppCommand::CleanupAutomation(_)
            | AppCommand::AutomationIntoClip
            | AppCommand::ClearParamLanes => {
                self.track_command(command);
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            // The freeze commands render on a worker and edit the document when it is done.
            AppCommand::FreezeTrack
            | AppCommand::FreezeTrackToDevice
            | AppCommand::FreezeTail
            | AppCommand::FreezeStrip
            | AppCommand::FreezeCancel
            | AppCommand::UnfreezeTrack
            | AppCommand::UnfreezeTrackKeep
            | AppCommand::BounceInPlace
            | AppCommand::BounceInPlaceMute
            | AppCommand::BounceRange
            | AppCommand::RefreezeTrack
            | AppCommand::BounceClipDry => {
                match command {
                    AppCommand::FreezeTrack => self.core.freeze_tracks(FreezeThrough::Track),
                    AppCommand::FreezeTrackToDevice => {
                        self.core.freeze_tracks(FreezeThrough::SelectedDevice);
                    }
                    AppCommand::FreezeTail => self.core.toggle_freeze_tail(),
                    AppCommand::FreezeStrip => self.core.toggle_freeze_strip(),
                    AppCommand::FreezeCancel => self.core.cancel_freeze(),
                    AppCommand::UnfreezeTrack => self.core.unfreeze_tracks(false),
                    AppCommand::UnfreezeTrackKeep => self.core.unfreeze_tracks(true),
                    AppCommand::BounceInPlace => self.core.bounce_in_place(false),
                    AppCommand::BounceRange => self.core.bounce_range(),
                    AppCommand::RefreezeTrack => self.core.refreeze_tracks(),
                    AppCommand::BounceClipDry => self.core.bounce_clips_dry(),
                    _ => self.core.bounce_in_place(true),
                }
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            // The player stack commands act on the selected tracks and are one undo step each.
            AppCommand::DirectRecord
            | AppCommand::SendPlayersToTrack
            | AppCommand::FoldPlayers
            | AppCommand::BypassPlayers
            | AppCommand::AddPatternLane
            | AppCommand::PatternLaneToNotes => {
                match command {
                    AppCommand::DirectRecord => self.core.toggle_direct_record(),
                    AppCommand::SendPlayersToTrack => self.core.send_players_to_track(),
                    AppCommand::FoldPlayers => self.core.fold_all_players(),
                    AppCommand::AddPatternLane => self.core.create_pattern_lane(),
                    AppCommand::PatternLaneToNotes => self.core.pattern_lane_to_notes(),
                    _ => self.core.bypass_all_players(),
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
            AppCommand::AudioNormaliseTo(target) => {
                self.core.normalise_audio_with(target.params());
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::AudioStripSilence => {
                self.open_silence();
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::AddReverbSend => {
                self.open_reverb_send();
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::RumbleMaker(choice) => {
                self.core.rumble_maker(choice.mode());
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::ReverseReverbPrint(length) => {
                self.core.reverse_reverb_print(length.seconds());
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::AddTransition(choice) => {
                self.core.add_transition(choice);
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
            AppCommand::CollectFiles => {
                match self.core.collect_files() {
                    Ok(()) => {}
                    Err(AppError::NoPath) => {
                        self.notify(
                            "Save the song first: Collect Files gathers its audio next to the project file"
                                .to_owned(),
                        );
                    }
                    Err(e) => self.notify(e.to_string()),
                }
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::CollectPreview => {
                match self.core.collect_preview_message() {
                    Ok(message) => self.notify(message),
                    Err(AppError::NoPath) => {
                        self.notify(
                            "Save the song first: Collect Files gathers its audio next to the project file"
                                .to_owned(),
                        );
                    }
                    Err(e) => self.notify(e.to_string()),
                }
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::CancelCollect => {
                self.core.cancel_collect();
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::MoveProject => {
                self.menubar.as_mut().map(MenuBar::close);
                if self.core.file().is_none() {
                    self.notify(
                        "Save the song first: Move Project needs a project file to move".to_owned(),
                    );
                    return Vec::new();
                }
                vec![ShellRequest::PickMoveFolder]
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
            // Edit > Combine and Uncombine work on the rack's selection, whichever pane has the
            // focus; a refusal is the toast the key gives.
            AppCommand::Combine | AppCommand::Uncombine => {
                if let Some((id, arg)) = command.command_id() {
                    self.run_shell_command(id, arg);
                }
                self.menubar.as_mut().map(MenuBar::close);
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
            AppCommand::TogglePianoKeys => {
                self.set_piano_keys_visible(!self.detached(WindowKind::PianoKeys));
                Vec::new()
            }
            AppCommand::ToggleTrackPanel => {
                let visible = !self.core.track_panel_visible();
                self.core.set_track_panel_visible(visible);
                if !visible && self.focus == Pane::TrackPanel {
                    self.set_focus(Pane::Rack);
                }
                self.repaint = true;
                Vec::new()
            }
            AppCommand::SetRackSort(mode) => {
                // A rejection (the rack is sorted that way already) is reported in the view.
                let _ = self.core.apply_command(Command::SetRackSort(mode));
                Vec::new()
            }
            AppCommand::ToggleSingleColumn => {
                let view = self.core.view_mut();
                let single = !view.single_column();
                view.set_single_column(single);
                Vec::new()
            }
            AppCommand::ToggleFileBrowser => {
                if self.browser_detached() {
                    // Hiding the browser closes its window.
                    self.set_detached(WindowKind::Browser, false);
                    self.set_file_browser_visible(false);
                } else {
                    self.set_file_browser_visible(!self.ui.file_browser_visible);
                }
                Vec::new()
            }
            AppCommand::DetachBrowser => {
                self.set_detached(WindowKind::Browser, !self.browser_detached());
                Vec::new()
            }
            AppCommand::ToggleSong => {
                // With the rack in its own window the song area is all the main window has.
                self.ui.show_song = !self.ui.show_song || self.rack_detached();
                if !self.ui.show_song {
                    self.set_focus(self.live_pane(self.focus));
                }
                self.layout_view();
                self.ui_changed();
                Vec::new()
            }
            AppCommand::EditTrackControllers => {
                if self.core.open_track_controllers_default() {
                    if !self.ui.show_song || self.lower_is_console() {
                        self.ui.show_song = true;
                        self.ui.show_console = false;
                        self.console_changed();
                    }
                    self.reveal_lower_pane();
                    self.set_focus(Pane::PianoRoll);
                } else {
                    self.notify("no note track to edit: add or select a note track");
                }
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::TogglePianoRoll => {
                if self.core.piano_roll_open() {
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
            AppCommand::CreatePlayer => {
                self.open_create(CreateKind::Player);
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
            AppCommand::CreateMixerChannel => {
                self.core.add_mix_channel();
                Vec::new()
            }
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
            AppCommand::ToggleSpectrumEq => {
                self.set_detached(
                    WindowKind::SpectrumEq,
                    !self.detached(WindowKind::SpectrumEq),
                );
                Vec::new()
            }
            AppCommand::ToggleVideo => {
                self.set_detached(WindowKind::Video, !self.detached(WindowKind::Video));
                Vec::new()
            }
            AppCommand::ToggleVideoOverlay => {
                self.core.toggle_video_overlay();
                self.save_video_prefs();
                self.repaint = true;
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
            AppCommand::TogglePreCount => {
                let mut settings = self.core.sink().settings().clone();
                settings.pre_count_bars = self.core.pre_count_toggled();
                self.apply_settings(settings);
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::ToggleClockOut => {
                let mut settings = self.core.sink().settings().clone();
                settings.sync.clock_out = !settings.sync.clock_out;
                if settings.sync.clock_out && settings.sync.out_ports.is_empty() {
                    self.notify("MIDI clock out has no port yet: choose one in Settings > Sync");
                }
                self.apply_settings(settings);
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::ToggleClockWhileStopped => {
                let mut settings = self.core.sink().settings().clone();
                settings.sync.send_while_stopped = !settings.sync.send_while_stopped;
                self.apply_settings(settings);
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::ToggleClockIn => {
                let mut settings = self.core.sink().settings().clone();
                settings.sync.follow_clock = !settings.sync.follow_clock;
                if settings.sync.follow_clock && settings.sync.clock_in_port.is_none() {
                    self.notify("MIDI clock in has no port yet: choose one in Settings > Sync");
                }
                self.apply_settings(settings);
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::Quantize(q) => {
                self.core.set_input_quantize(q);
                self.ui.input_quantize = q;
                self.ui_changed();
                Vec::new()
            }
            AppCommand::ToggleConsoleLinks => {
                let view = self.core.view_mut();
                let show = !view.show_console_links();
                view.set_show_console_links(show);
                self.ui.show_console_links = show;
                self.ui_changed();
                Vec::new()
            }
            AppCommand::ToggleBigMeter => {
                let view = self.core.view_mut();
                let show = !view.big_meter_visible();
                view.set_big_meter_visible(show);
                Vec::new()
            }
            AppCommand::ToggleCpuMeters => {
                let on = !self.core.cpu_meters_on();
                self.core.set_cpu_meters(on);
                Vec::new()
            }
            AppCommand::ToggleTempoLane => {
                let show = !self.core.arrangement().tempo_lane_visible();
                self.core.arrangement_mut().set_tempo_lane(show);
                self.ui.show_tempo_lane = show;
                self.ui_changed();
                Vec::new()
            }
            AppCommand::ToggleEnergyLane => {
                let show = !self.core.arrangement().energy_lane_visible();
                self.core.arrangement_mut().set_energy_lane(show);
                self.ui.show_energy_lane = show;
                self.ui_changed();
                // A lane with nothing to show measures the song (the reason is in the status
                // line when it cannot).
                if show && self.core.energy().is_none() && !self.core.energy_running() {
                    let _ = self.core.start_energy();
                }
                Vec::new()
            }
            AppCommand::ToggleVideoRow => {
                let show = !self.core.arrangement().video_row_visible();
                self.core.arrangement_mut().set_video_row(show);
                self.ui.show_video_row = show;
                self.ui_changed();
                Vec::new()
            }
            AppCommand::RefreshEnergyLane => {
                if self.core.arrangement().energy_lane_visible() {
                    let _ = self.core.start_energy();
                }
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::SoundAnalyst => {
                self.open_sound_analyst();
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::BassRead => {
                self.open_bass_read();
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::DeliveryTargets => {
                self.open_delivery(DeliveryPage::Targets);
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::PeakSurgeon => {
                self.open_delivery(DeliveryPage::Peaks);
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::LensHeadphones => {
                self.core.lens_off();
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::LensSmallSpeaker => {
                self.core
                    .set_lens_voicing(atomik_engine::LensVoicing::SmallSpeaker);
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::LensClubSub => {
                self.core
                    .set_lens_voicing(atomik_engine::LensVoicing::ClubSub);
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::LensMono => {
                self.core.toggle_lens_mono();
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::SplitSub => {
                self.core.split_sub();
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::FindRepetition => {
                self.open_repetition();
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::ToggleMarkerStrip => {
                let show = !self.core.arrangement().marker_strip_visible();
                self.core.arrangement_mut().set_marker_strip(show);
                self.ui.show_marker_strip = show;
                self.ui_changed();
                Vec::new()
            }
            AppCommand::ToggleInspector => {
                let show = !self.core.arrangement().inspector_visible();
                self.core.arrangement_mut().set_inspector(show);
                self.core.piano_roll_mut().set_inspector(show);
                self.ui.show_inspector = show;
                self.ui_changed();
                Vec::new()
            }
            AppCommand::ToggleMarkerList => {
                let show = !self.core.arrangement().marker_list_visible();
                self.core.arrangement_mut().set_marker_list(show);
                self.ui.show_marker_list = show;
                self.ui_changed();
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
                self.ui_changed();
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
            AppCommand::CreateVcaForFolder => {
                // A refusal is already reported.
                self.core.create_vca_for_folder(self.live_pane(self.focus));
                Vec::new()
            }
            AppCommand::RouteFolder(bus) => {
                // A refusal is already reported.
                self.core.route_folder(self.live_pane(self.focus), bus);
                Vec::new()
            }
            AppCommand::UnrouteFolder => {
                self.core.unroute_folder(self.live_pane(self.focus));
                Vec::new()
            }
            AppCommand::LinkFaders => {
                let on = !self.core.console().link_faders();
                self.core.console_mut().set_link_faders(on);
                self.ui.link_faders = on;
                self.ui_changed();
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
            AppCommand::RouteVca(id) => {
                // A refusal is already reported.
                self.core.route_vca(self.live_pane(self.focus), id);
                Vec::new()
            }
            AppCommand::UnrouteVca(id) => {
                self.core.unroute_vca(self.live_pane(self.focus), id);
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
            AppCommand::MorphSnapshot(id) => {
                self.core.morph_snapshot(id, true);
                Vec::new()
            }
            AppCommand::MorphPreviousSnapshot => {
                self.core.morph_snapshot(None, false);
                Vec::new()
            }
            AppCommand::MeterMode(mode) => {
                self.core.console_mut().set_meter_mode(mode);
                self.ui.meter_mode = mode;
                self.ui_changed();
                Vec::new()
            }
            AppCommand::ToggleReference => {
                self.core.toggle_reference();
                Vec::new()
            }
            AppCommand::AddReference => vec![ShellRequest::PickReference],
            AppCommand::SelectReference(n) => {
                self.core.select_reference(usize::from(n));
                Vec::new()
            }
            AppCommand::NextReference => {
                self.core.next_reference();
                Vec::new()
            }
            AppCommand::RemoveReference => {
                let selected = self.core.reference_info().selected;
                self.core.remove_reference(selected);
                Vec::new()
            }
            AppCommand::ReferenceCue(on) => {
                let selected = self.core.reference_info().selected;
                self.core.set_reference_cue(selected, on);
                Vec::new()
            }
            AppCommand::ReferenceMatch => {
                let on = !self.core.reference_info().level_match;
                self.core.set_reference_level_match(on);
                Vec::new()
            }
            AppCommand::ReferenceBand(band) => {
                self.core.set_reference_band(band);
                Vec::new()
            }
            AppCommand::ReferenceSolo(solo) => {
                self.core.set_reference_solo(solo);
                Vec::new()
            }
            AppCommand::MorphTime(seconds) => {
                self.core.set_morph_seconds(seconds);
                self.ui.morph_seconds = seconds.clamp(1, 8);
                self.ui_changed();
                Vec::new()
            }
            AppCommand::SnapshotEffects => {
                self.core.toggle_snapshot_effects(None);
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
            AppCommand::CopySettings(section) => {
                self.core
                    .copy_strip_settings(self.live_pane(self.focus), section);
                Vec::new()
            }
            AppCommand::PasteSettings => {
                self.core.paste_strip_settings(self.live_pane(self.focus));
                Vec::new()
            }
            AppCommand::ResetStrip => {
                self.core.reset_strips(self.live_pane(self.focus));
                Vec::new()
            }
            AppCommand::ClearInserts => {
                self.core.clear_inserts(self.live_pane(self.focus));
                Vec::new()
            }
            AppCommand::ToggleCompensation => {
                let on = self.core.doc().console.compensation;
                let command = Command::Console(ConsoleCommand::SetCompensation(!on));
                // A rejection is already reported in the view.
                let _ = self.core.apply_command(command);
                Vec::new()
            }
            AppCommand::RemoveChannel => {
                self.core.remove_selected_strips(self.live_pane(self.focus));
                Vec::new()
            }
            AppCommand::CreateParallel => {
                self.core
                    .create_parallel_channels(self.live_pane(self.focus));
                Vec::new()
            }
            AppCommand::ToggleSendFollow => {
                let on = self.core.doc().console.send_follow;
                let command = Command::Console(ConsoleCommand::SetSendFollow(!on));
                // A rejection is already reported in the view.
                let _ = self.core.apply_command(command);
                Vec::new()
            }
            AppCommand::ToggleReturnAlignment => {
                let on = self.core.doc().console.align_returns;
                let command = Command::Console(ConsoleCommand::SetAlignReturns(!on));
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
            AppCommand::MapCommand(c) => {
                self.core.midi_arm(atomik_model::MidiTarget::Command(c));
                Vec::new()
            }
            AppCommand::LockSurfaces => {
                self.core.toggle_surface_locks();
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
            AppCommand::ExportDiagnostics => vec![ShellRequest::PickExportDiagnostics],
            AppCommand::ExportDeviceInfo => vec![ShellRequest::PickExportDeviceInfo],
            AppCommand::ClearMidiMaps => {
                if !self.core.clear_midi_maps() {
                    self.notify("There are no MIDI mappings to clear");
                }
                Vec::new()
            }
            AppCommand::AudioSettings => {
                self.open_settings(None);
                Vec::new()
            }
            AppCommand::KeyboardShortcuts => {
                self.open_settings(Some(crate::settings_dialog::Tab::Keyboard));
                Vec::new()
            }
            AppCommand::Documentation => {
                vec![ShellRequest::OpenUrl(crate::about_box::REPOSITORY_URL)]
            }
            AppCommand::ReportIssue => vec![ShellRequest::OpenUrl(crate::about_box::ISSUES_URL)],
            AppCommand::About => {
                self.about = Some(AboutBox::new(crate::about_box::VERSION));
                self.menubar.as_mut().map(MenuBar::close);
                Vec::new()
            }
            AppCommand::SetTheme(choice) => {
                self.workflow.theme = choice;
                self.save_workflow();
                self.apply_theme();
                Vec::new()
            }
            AppCommand::AppZoom(change) => {
                let zoom = change.apply(self.workflow.app_zoom);
                if zoom != self.workflow.app_zoom {
                    self.workflow.app_zoom = zoom;
                    self.save_workflow();
                    self.clear_tooltip();
                }
                Vec::new()
            }
            AppCommand::FocusWindow(kind) => {
                if kind == WindowKind::Main || self.wins.contains_key(&kind) {
                    vec![ShellRequest::FocusWindow(kind)]
                } else {
                    self.notify("That window is not open: detach it from the View menu first");
                    Vec::new()
                }
            }
            AppCommand::NewSongWindow => vec![ShellRequest::NewSong],
            AppCommand::OpenSongWindow => vec![ShellRequest::PickOpenSong],
            AppCommand::NextSong => vec![ShellRequest::ShowSong(SongTarget::Next)],
            AppCommand::PreviousSong => vec![ShellRequest::ShowSong(SongTarget::Previous)],
            AppCommand::ActivateSong(place) => {
                vec![ShellRequest::ShowSong(SongTarget::Place(place))]
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
            AppCommand::RescanMidi => {
                let session = self.core.sink_mut();
                session.rescan_midi();
                let message = match (session.midi_complete(), session.status().midi_port.clone()) {
                    (true, Some(ports)) => format!("MIDI inputs open: {ports}"),
                    (true, None) => "No MIDI input is switched on in Audio Settings".to_string(),
                    (false, _) => session
                        .status()
                        .midi_error
                        .clone()
                        .unwrap_or_else(|| "A MIDI input could not be opened".to_string()),
                };
                self.notify(message);
                self.poll_status();
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
                self.new_song();
                Vec::new()
            }
            AppCommand::Close => {
                self.core.new_document();
                Vec::new()
            }
            AppCommand::NewFromTemplate(i) => {
                self.new_from_template(i);
                Vec::new()
            }
            AppCommand::OpenExample(i) => {
                self.open_example(i);
                Vec::new()
            }
            AppCommand::GuidedTour => {
                self.start_guide();
                Vec::new()
            }
            AppCommand::NewAtRate(rate) => {
                self.core.new_document_at(rate);
                Vec::new()
            }
            AppCommand::Open => vec![ShellRequest::PickOpenFile],
            AppCommand::CloseSongWindow => {
                // A save still writing must reach the disk before the song goes.
                self.core.settle_save();
                vec![ShellRequest::CloseSong]
            }
            AppCommand::Quit => {
                // A save still writing must reach the disk before the process ends.
                self.core.settle_save();
                vec![ShellRequest::Quit]
            }
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

    /// Carries out what waited for the answer about unsaved changes.
    fn continue_pending(&mut self, then: PendingAction) -> Vec<ShellRequest> {
        match then {
            PendingAction::Command(command) => self.confirmed(command),
            PendingAction::OpenFile(path) => {
                self.open_file(&path);
                Vec::new()
            }
            PendingAction::RestoreVersion(id) => {
                self.restore_version(id);
                Vec::new()
            }
        }
    }

    /// Save, in the question about unsaved changes: saves the project, then carries on with
    /// `then`. A project with no file yet asks for one ([`ShellRequest::PickSaveFileThen`]); a
    /// save that fails is shown and `then` does not happen.
    pub fn save_then(&mut self, then: PendingAction) -> Vec<ShellRequest> {
        self.repaint = true;
        match self.core.save() {
            Ok(()) => self.continue_pending(then),
            Err(AppError::NoPath) => vec![ShellRequest::PickSaveFileThen { then }],
            Err(e) => {
                self.notify(e.to_string());
                Vec::new()
            }
        }
    }

    /// The file picked for [`ShellRequest::PickSaveFileThen`]: saves the project there, then
    /// carries on with `then`. A save that fails is shown and `then` does not happen.
    pub fn save_to_then(&mut self, path: &Path, then: PendingAction) -> Vec<ShellRequest> {
        self.repaint = true;
        match self.core.save_as(path) {
            Ok(()) => self.continue_pending(then),
            Err(e) => {
                self.notify(e.to_string());
                Vec::new()
            }
        }
    }

    /// Don't Save, in the question about unsaved changes: carries on with `then`.
    pub fn discard_then(&mut self, then: PendingAction) -> Vec<ShellRequest> {
        self.repaint = true;
        self.continue_pending(then)
    }

    /// File > Open Recent: the song at place `index` of the recent list. A song that is gone
    /// leaves the list with a message.
    fn open_recent(&mut self, index: usize) {
        let Some(path) = self.core.recent().get(index).cloned() else {
            return;
        };
        if path.exists() {
            self.open_project_request(path);
        } else {
            self.core.forget_recent(&path);
            self.notify(format!("{} no longer exists", path.display()));
        }
    }

    /// Start-up with the "Open last project at start-up" preference on: opens the newest recent
    /// song. Returns whether a song was opened. A song that is gone leaves the list with a
    /// message. Call it only when the command line names no project.
    pub fn restore_last_project(&mut self) -> bool {
        if !self.workflow.open_last_project {
            return false;
        }
        let Some(path) = self.core.recent().first().cloned() else {
            return false;
        };
        if !path.exists() {
            self.core.forget_recent(&path);
            self.notify(format!("{} no longer exists", path.display()));
            return false;
        }
        self.open_file(&path);
        true
    }

    /// File > New: the default song when one is chosen and still there, else the starter project.
    fn new_song(&mut self) {
        let mut why = None;
        if let Some(path) = self.workflow.default_song.clone() {
            if path.exists() {
                match self.core.open_template(&path) {
                    Ok(()) => return,
                    Err(e) => why = Some(format!("The default song cannot be opened: {e}")),
                }
            } else {
                why = Some(format!(
                    "The default song {} is gone; the starter project is used",
                    path.display()
                ));
            }
        }
        self.core.new_document();
        // After the new song: a new song forgets the messages of the old one.
        if let Some(why) = why {
            self.notify(why);
        }
    }

    /// Start-up with no project named: the default song, when one is chosen. Returns whether it
    /// opened. A song that is gone or broken is said in a message and the starter project stays.
    pub fn start_with_default_song(&mut self) -> bool {
        let Some(path) = self.workflow.default_song.clone() else {
            return false;
        };
        if !path.exists() {
            self.notify(format!(
                "The default song {} is gone; the starter project is used",
                path.display()
            ));
            return false;
        }
        match self.core.open_template(&path) {
            Ok(()) => true,
            Err(e) => {
                self.notify(format!("The default song cannot be opened: {e}"));
                false
            }
        }
    }

    /// File > New from Template > a template: the template at this place of the list, as an
    /// untitled song.
    fn new_from_template(&mut self, index: usize) {
        let Some(entry) = self.core.templates().get(index).cloned() else {
            return;
        };
        if !entry.path.exists() {
            self.core.reload_templates();
            self.notify(format!("{} no longer exists", entry.path.display()));
            return;
        }
        if let Err(e) = self.core.open_template(&entry.path) {
            self.notify(e.to_string());
        }
    }

    /// A factory example from the start screen: the example at this place of the list, as an
    /// untitled song (the factory copy is never the song's file).
    fn open_example(&mut self, index: usize) {
        let Some(entry) = self.core.examples().into_iter().nth(index) else {
            return;
        };
        if let Err(e) = self.core.open_template(&entry.path) {
            self.notify(e.to_string());
        }
    }

    /// Help > Guided Tour: a new starter song and the guide panel at its first step.
    fn start_guide(&mut self) {
        self.core.new_document();
        self.guide = Some(Guide::new());
        self.sync_guide();
    }

    /// Whether the guide panel is open.
    pub fn guide_open(&self) -> bool {
        self.guide.is_some()
    }

    /// The guided first song, while its panel is open.
    pub fn guide(&self) -> Option<&Guide> {
        self.guide.as_ref()
    }

    /// The area the guide panel floats in: the window between the menu bar and the status bar,
    /// without the file browser column and the Track Panel column.
    pub fn guide_bounds(&self) -> Rect {
        let size = self.size();
        let layout = self.layout();
        let x0 = layout.file_browser.map_or(0.0, |r| r.x1);
        let x1 = layout.track_panel.map_or(size.width, |r| r.x0);
        Rect::new(
            x0,
            self.bar_height(),
            x1.max(x0 + 1.0),
            (size.height - STATUS_BAR_HEIGHT).max(self.bar_height() + 1.0),
        )
    }

    /// The rectangle of the control the guide's step talks about, where the interface has one
    /// (the menus of a native menu bar have none).
    pub fn guide_outline(&self) -> Option<Rect> {
        let step = self.guide.as_ref()?.current()?;
        match step.spot() {
            Spot::Menu(title) => {
                self.menubar.as_ref()?;
                let menus = self.menu_model();
                let at = menus.iter().position(|m| m.title == title)?;
                MenuBar::title_rects(&menus).get(at).copied()
            }
            Spot::Loop => Some(
                self.core
                    .transport_bar()
                    .button_rect(atomik_ui::TransportButton::Loop),
            ),
            Spot::Song => self.layout().song,
            Spot::Rack => Some(self.layout().rack),
        }
    }

    /// Takes a new look at the song for the guide (once a frame, and when it starts).
    fn sync_guide(&mut self) {
        if self.guide.is_none() {
            return;
        }
        let progress = self.core.guide_progress();
        if let Some(guide) = &mut self.guide
            && guide.update(progress)
        {
            self.repaint = true;
        }
    }

    /// An event for the guide panel: `Some` when the panel took it.
    fn guide_event(&mut self, event: &UiEvent) -> Option<Vec<ShellRequest>> {
        let bounds = self.guide_bounds();
        let guide = self.guide.as_mut()?;
        let was_hot = guide.hovered().is_some();
        let outcome = guide.handle(event, bounds);
        match outcome {
            GuideOutcome::Ignored => {
                if was_hot {
                    self.repaint = true;
                }
                None
            }
            GuideOutcome::Consumed => {
                self.repaint = true;
                if matches!(event, UiEvent::PointerMove { .. }) {
                    self.leave_hover();
                }
                self.sync_guide();
                Some(Vec::new())
            }
            GuideOutcome::Close => {
                self.guide = None;
                self.repaint = true;
                Some(Vec::new())
            }
        }
    }

    /// File > New from Template > Save as Template: a copy of the song in the templates folder.
    fn save_as_template(&mut self) {
        let name = self.core.template_name();
        match self.core.save_as_template(&name) {
            Ok(path) => {
                let shown = path
                    .file_stem()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                self.notify(format!("Saved the template {shown}"));
            }
            Err(e) => self.notify(format!("The template was not saved: {e}")),
        }
        self.repaint = true;
    }

    /// File > New from Template > Use Song as Default Song: a copy of the song becomes the
    /// project File > New opens, and the choice is saved with the workflow preferences.
    fn set_default_song(&mut self) {
        match self.core.save_default_song() {
            Ok(path) => {
                self.workflow.default_song = Some(path);
                self.save_workflow();
                self.notify("File > New now opens a copy of this song".to_owned());
            }
            Err(e) => self.notify(format!("The default song was not saved: {e}")),
        }
        self.repaint = true;
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
        if let Err(e) = self.core.save_as_interactive(path) {
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
    /// When the dialog was asked for by Import File as Take, the file becomes a take of that
    /// group instead.
    pub fn import_midi(&mut self, path: &Path, as_song: bool) {
        if let Some(group) = self.core.take_import_request() {
            self.core.import_midi_take(path, group);
        } else {
            self.core.import_midi(path, as_song);
        }
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
        self.sync_video_ui();
        self.sync_relink();
        self.sync_export();
        self.sync_masking();
        self.sync_bass_read();
        self.sync_delivery();
        self.sync_sound_analyst();
        self.sync_wavetable();
        self.sync_mixcheck();
        self.sync_midi_maps();
        self.sync_macros();
        self.sync_mod_matrix();
        self.sync_blind();
        self.sync_param_search();
        self.sync_quantize();
        self.sync_kick_bass();
        self.sync_zone();
        self.sync_guide();
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
        self.write_ui_state();
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
        // A cable that rested on a folded device unfolds it (an edit like the fold button's).
        if let Some(key) = self.core.view_mut().hover_unfold(now_seconds) {
            let _ = self
                .core
                .apply_command(Command::SetFolded { key, folded: false });
            self.repaint = true;
        }
        self.core.transport_frame(now_seconds);
        // A template dropped into the folder shows in File > New from Template.
        if self.core.refresh_templates(now_seconds) {
            self.repaint = true;
        }
        self.save_click_level();
        self.core.plugin_tick(now_seconds);
        // A plugin window closed somewhere new: the position is kept for next time.
        if self.core.take_plugin_geoms_changed() {
            self.ui_changed();
        }
        self.core.piano_roll_tick(now_seconds);
        self.tick_tooltip(now_seconds);
        // A pointer resting on a scroll strip of a menu bar list scrolls it.
        if self.menubar.as_ref().is_some_and(MenuBar::is_open) {
            let model = self.menu_model();
            let window_size = self.size();
            if let Some(bar) = &mut self.menubar {
                bar.set_window(window_size);
                if bar.tick(now_seconds, &model) {
                    self.repaint = true;
                }
            }
        }
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
        if self.console_visible() || self.core.view().shows_mix_panels() {
            self.core.update_console_meters(now_seconds);
        }
        self.core.feed_rec_input(now_seconds);
        if self.core.view().flip() == atomik_ui::Flip::Back {
            self.core.feed_cv_activity(now_seconds);
        }
        self.flush_geometry(now_seconds);
        self.fold_repaint();
    }

    /// Opens the control surfaces the settings name (each with its driver and ports), or closes
    /// them. Nothing happens when the settings are the ones they already run with. A port that
    /// cannot be opened is reported, and a surface with no open port is not set up.
    fn configure_surface(&mut self) {
        let wanted: Vec<SurfaceSettings> = self.core.sink().settings().surfaces.clone();
        if wanted == self.surface_applied {
            return;
        }
        self.surface_applied = wanted.clone();
        let mut hosts = Vec::new();
        for s in wanted.iter().filter(|s| s.runs()) {
            let Some(id) = s.driver.as_deref() else {
                continue;
            };
            let Some(driver) = atomik_midi_io::surface::driver_by_id(id) else {
                self.notify(format!("unknown control surface {id:?}"));
                continue;
            };
            let (link, problems) =
                crate::surface::SystemLink::open(s.input.as_deref(), s.output.as_deref());
            if let Some(problem) = problems.first() {
                self.notify(format!("{}: {problem}", s.name));
            }
            if link.is_open() {
                let port = s.input.as_deref().unwrap_or(&s.name);
                hosts.push(
                    crate::surface::SurfaceHost::new(driver, Box::new(link)).named(&s.name, port),
                );
            }
        }
        self.core.set_surfaces(hosts);
    }

    /// The click-level slider changed the metronome level: keep it in `settings.json` once the
    /// pointer lets go of the slider (not on every step of a drag).
    fn save_click_level(&mut self) {
        if self.core.transport_bar().is_dragging() {
            return;
        }
        let gain = self.core.metronome_gain();
        if (gain - self.core.sink().settings().metronome_gain).abs() <= f32::EPSILON {
            return;
        }
        let mut settings = self.core.sink().settings().clone();
        settings.metronome_gain = gain;
        self.apply_settings(settings);
    }

    /// Applies the tune the Settings dialog asked for: `chosen` replaces the project's tune (one
    /// undo step) and is kept as the default for new projects when it is not the tune the dialog
    /// opened with; otherwise `saved` stays the default. Returns the default to save.
    fn apply_tune_setting(&mut self, chosen: f32, saved: f32) -> f32 {
        let project = self.core.doc().tune_cents;
        if chosen.to_bits() == project.to_bits() {
            return saved;
        }
        if let Err(e) = self
            .core
            .apply_command(atomik_model::Command::SetMasterTune(chosen))
        {
            self.notify(format!("Master tune not changed: {e}"));
            return saved;
        }
        chosen
    }

    fn apply_settings(&mut self, mut settings: AudioSettings) {
        let saved_tune = self.core.sink().settings().master_tune_cents;
        settings.master_tune_cents =
            self.apply_tune_setting(settings.master_tune_cents, saved_tune);
        self.core.set_default_tune(settings.master_tune_cents);
        let gain = settings.metronome_gain;
        let pre_count = settings.pre_count_bars;
        let before = self.core.sink().settings().clone();
        let session = self.core.sink_mut();
        let restarted = session.set_settings(settings) == SettingsChange::RestartAudio;
        if restarted {
            session.set_enabled(true);
            self.core.rebuild_engine();
        }
        // A configuration that does not open is never saved: the session fell back to the last
        // one that did (the notice says so), or audio is off and the file keeps the old devices.
        let session = self.core.sink_mut();
        let fell_back = session.take_notice();
        let mut to_save = session.settings().clone();
        let failed = restarted && !session.is_running();
        if failed {
            let why = session.status().error.clone().unwrap_or_default();
            to_save.use_devices(&LastGood::of(&before));
            self.notify(format!(
                "Audio settings not applied: {why}. The saved device settings are unchanged."
            ));
        }
        let kept_old_devices = failed || fell_back.is_some();
        if let Some(notice) = fell_back {
            self.notify(notice);
        }
        self.configure_surface();
        if (gain - self.core.metronome_gain()).abs() > f32::EPSILON {
            self.core.set_metronome_gain(gain);
        }
        if pre_count != self.core.pre_count_bars() {
            self.core.set_pre_count_bars(pre_count);
        }
        // Nothing but a rejected device choice changed: the file stays as it is.
        let unchanged = kept_old_devices && !to_save.differs_from(&before);
        if let Some(path) = &self.settings_path
            && !unchanged
            && let Err(e) = settings::save(path, &to_save)
        {
            let message = format!("settings not saved: {e}");
            self.notify(message);
        }
        self.poll_status();
    }

    /// The name the Window menu lists the song under: the window title without the app's name.
    pub fn song_name(&self) -> String {
        let title = self.core.title();
        title
            .strip_prefix("Atomik Rack \u{2014} ")
            .unwrap_or(&title)
            .to_string()
    }

    /// The names of the open songs and the place of this one in the list (what the Window menu
    /// lists), as the workspace last told it.
    pub fn song_list(&self) -> (&[String], usize) {
        (&self.songs, self.song_at)
    }

    /// Sets the list of open songs (the workspace calls it when songs open, close or are
    /// renamed).
    pub(crate) fn set_song_list(&mut self, songs: Vec<String>, at: usize) {
        if self.songs != songs || self.song_at != at {
            self.songs = songs;
            self.song_at = at;
            self.repaint = true;
        }
    }

    /// Opens the project or patch `path` into this song (the error comes back to the caller).
    pub(crate) fn open_song_file(&mut self, path: &Path) -> Result<(), AppError> {
        let opened = self.core.open(path);
        self.repaint = true;
        opened
    }

    /// Hands the window state file over to another song (the first song to have it is gone).
    pub(crate) fn take_ui_state_path(&mut self) -> Option<PathBuf> {
        self.ui_state_path.take()
    }

    /// Where this song saves the window state, when nobody else does.
    pub(crate) fn set_ui_state_path(&mut self, path: PathBuf) {
        if self.ui_state_path.is_none() {
            self.ui_state_path = Some(path);
            self.geom_dirty = true;
        }
    }

    /// The audio settings the song runs with (the devices, MIDI inputs and control surfaces that
    /// move to the song that takes the audio next).
    pub fn audio_settings(&self) -> AudioSettings {
        self.core.sink().settings().clone()
    }

    /// Whether another song has the audio.
    pub fn is_silent(&self) -> bool {
        self.silent
    }

    /// Gives the audio up: the stream, the MIDI inputs and the control surfaces close, the
    /// transport stops and the song goes on without a sound until it is focused again. Plugin
    /// instances stay loaded.
    pub(crate) fn release_audio(&mut self) {
        let mut settings = self.core.sink().settings().clone();
        // The surfaces are hardware ports: only the song that has the audio holds them.
        settings.surfaces.clear();
        let session = self.core.sink_mut();
        session.set_settings(settings);
        session.set_enabled(false);
        self.silent = true;
        self.configure_surface();
        self.core.rebuild_engine();
        self.poll_status();
        self.repaint = true;
    }

    /// Takes the audio over with `settings` (the ones the song that gave it up ran with): the
    /// engine is rebuilt on a new stream, as a change of the devices does.
    pub(crate) fn claim_audio(&mut self, settings: AudioSettings) {
        let gain = settings.metronome_gain;
        let pre_count = settings.pre_count_bars;
        let tune = settings.master_tune_cents;
        let session = self.core.sink_mut();
        session.set_settings(settings);
        session.set_enabled(true);
        self.silent = false;
        self.configure_surface();
        self.core.rebuild_engine();
        if (gain - self.core.metronome_gain()).abs() > f32::EPSILON {
            self.core.set_metronome_gain(gain);
        }
        if pre_count != self.core.pre_count_bars() {
            self.core.set_pre_count_bars(pre_count);
        }
        self.core.set_default_tune(tune);
        if let Some(notice) = self.core.sink_mut().take_notice() {
            self.notify(notice);
        }
        self.poll_status();
        self.repaint = true;
    }

    /// Marks the song as running without audio from the start (a song opened while another one
    /// has the audio): the devices and surfaces of its session stay closed.
    pub(crate) fn start_silent(&mut self) {
        self.silent = true;
        let mut settings = self.core.sink().settings().clone();
        settings.surfaces.clear();
        let session = self.core.sink_mut();
        session.set_settings(settings);
        session.set_enabled(false);
        self.configure_surface();
        self.poll_status();
    }

    /// The preferences every song shares (the keyboard shortcuts, the workflow preferences and
    /// the layout of the window): `self` takes them from `from`, so a song window opens looking
    /// like the one it was opened from and a change made in one song reaches the others.
    pub(crate) fn adopt_preferences(&mut self, from: &Shell<B>) {
        if from.user_keymap != self.user_keymap {
            self.set_user_keymap(from.user_keymap.clone(), false);
        }
        if from.workflow != self.workflow {
            self.workflow = from.workflow.clone();
            self.apply_workflow_to_views();
            self.sync_library_root();
        }
        self.export_prefs = from.export_prefs.clone();
        self.system_dark = from.system_dark;
        self.repaint = true;
    }

    /// Takes the layout of the window from `from` (the panes shown and the zoom of the views),
    /// without the windows it detached.
    pub(crate) fn adopt_layout(&mut self, from: &Shell<B>) {
        let mut ui = from.ui.clone();
        ui.windows = crate::windows::WindowsState::default();
        self.ui = ui;
        self.apply_ui_state_to_views();
        self.core.set_input_quantize(self.ui.input_quantize);
        self.layout_view();
    }

    /// The preferences [`Self::adopt_preferences`] shares, for comparing two songs'.
    pub(crate) fn shared_preferences(&self) -> (&WorkflowPrefs, &UserKeymap) {
        (&self.workflow, &self.user_keymap)
    }

    /// The overload counters of the last status poll.
    pub fn overload(&self) -> OverloadStats {
        self.overload
    }

    /// Writes the overload counters into the log when one grew: at once the first time, then at
    /// most every 20th poll (about every 5 seconds), so an xrun storm is a line, not a flood.
    /// The audio thread never logs; this is where its counters become records.
    fn log_overload(&mut self) {
        self.overload_polls = self.overload_polls.saturating_add(1);
        let (now, before) = (self.overload.fields(), self.overload_logged.fields());
        if !now.iter().zip(&before).any(|(a, b)| a.1 > b.1) {
            // Equal, or reset by a rebuilt engine: the new baseline.
            self.overload_logged = self.overload;
            return;
        }
        if self.overload_polls >= 20 || self.overload_logged.is_clear() {
            log::warn!("overload: {}", self.overload.parts().join(", "));
            self.overload_logged = self.overload;
            self.overload_polls = 0;
        }
    }

    /// Says so once when the stream stops delivering audio (the device was unplugged, or the
    /// driver stopped calling back), and, when the settings ask for it, switches to the default
    /// devices (at once for a lost device, after [`AUTO_RESET_SILENCE_MS`] for a stalled one, which
    /// may still come back). True when it replaced the stream.
    fn watch_stream(&mut self, status: &AudioStatus) -> bool {
        if !status.stream_state.is_dead() {
            self.stream_announced = false;
            // Only a stream that runs earns another automatic reset: a default device that
            // never starts is not retried in a loop.
            if status.stream_state == StreamState::Running {
                self.stream_reset = false;
            }
            return false;
        }
        let device = if status.device.is_empty() {
            "the audio device"
        } else {
            status.device.as_str()
        };
        let auto = self.core.sink().settings().device_loss.reset_to_default;
        let due = status.stream_state == StreamState::Lost
            || status.stream_silent_ms >= AUTO_RESET_SILENCE_MS;
        if auto && due && !self.stream_reset {
            self.stream_reset = true;
            log::warn!("audio stream lost on {device}: switching to the default devices");
            self.reset_to_default_devices();
            let message = match &self.core.sink().status().error {
                Some(e) => format!(
                    "Audio device lost: {device}. The default devices did not open either: {e}"
                ),
                None => format!(
                    "Audio device lost: {device}. Switched to the default devices; pick it \
                     again in Audio > Settings when it is back."
                ),
            };
            self.notify(message);
            self.stream_announced = true;
            return true;
        }
        if !self.stream_announced {
            self.stream_announced = true;
            log::warn!("audio stream {:?} on {device}", status.stream_state);
            let what = if status.stream_state == StreamState::Lost {
                format!("Audio device lost: {device} is no longer available")
            } else {
                format!("Audio stopped: {device} is not delivering audio")
            };
            self.notify(format!(
                "{what}. Audio > Restart Audio tries it again; Audio > Settings chooses another device."
            ));
        }
        false
    }

    /// Opens the host's default devices instead of the chosen ones. The chosen devices stay in
    /// the settings file until the settings are applied again.
    fn reset_to_default_devices(&mut self) {
        let session = self.core.sink_mut();
        session.fall_back_to_defaults();
        session.set_enabled(true);
        self.core.rebuild_engine();
    }

    /// Goes back to the chosen devices once they are listed again (see
    /// [`LiveSession::take_switch_back`]): at a moment that suits the song, not while it plays
    /// or records. The notice of the session ("back, switching") or of a failed try is told.
    fn switch_back_to_chosen_devices(&mut self) {
        if self.silent || self.core.is_playing() || self.core.is_recording() {
            return;
        }
        if !self.core.sink_mut().take_switch_back() {
            return;
        }
        self.core.sink_mut().set_enabled(true);
        self.core.rebuild_engine();
        if let Some(notice) = self.core.sink_mut().take_notice() {
            self.notify(notice);
        }
    }

    /// While the Audio Settings dialog is open, lists the MIDI ports again once a second so a
    /// controller plugged in or removed shows without reopening the dialog, and opens the
    /// enabled inputs that have appeared.
    fn relist_ports(&mut self, at: std::time::Instant) {
        if self.dialog.is_none() {
            self.ports_listed_at = None;
            return;
        }
        let due = self
            .ports_listed_at
            .is_none_or(|last| at.saturating_duration_since(last) >= PORT_RELIST_EVERY);
        if !due {
            return;
        }
        self.ports_listed_at = Some(at);
        let session = self.core.sink_mut();
        if !session.midi_complete() {
            session.rescan_midi();
        }
        let lists = DeviceLists::query(session.backend());
        if let Some(dialog) = &mut self.dialog {
            dialog.set_lists(lists);
        }
        self.repaint = true;
    }

    /// While the Video tab of Settings shows, looks for FFmpeg when that has not been done and
    /// gives the tab the line saying what was found.
    fn sync_video_tab(&mut self) {
        use crate::settings_dialog::Tab;
        if !self.dialog.as_ref().is_some_and(|d| d.tab() == Tab::Video) {
            return;
        }
        self.core.refresh_ffmpeg_status();
        let line = match self.core.video_status() {
            atomik_video::FfmpegStatus::Unchecked => "Looking for FFmpeg...".to_owned(),
            status => status.message(),
        };
        if let Some(dialog) = &mut self.dialog {
            dialog.set_video_status(line);
        }
    }

    /// Reads audio telemetry into the status bar; true when its text changed.
    pub fn poll_status(&mut self) -> bool {
        self.poll_status_at(crate::session::clock_now())
    }

    /// [`Self::poll_status`] with the time the stream liveness is judged at.
    pub fn poll_status_at(&mut self, at: std::time::Instant) -> bool {
        self.relist_ports(at);
        let target = self.core.notes_target_device().map(|d| d.name.clone());
        let mut status = self.core.sink_mut().poll_at(at).clone();
        let back = self.core.sink().switch_back_pending();
        // The notice that the chosen devices are back is told together with the restart; the
        // restart's own notice (it did not open) replaces it.
        if !back && let Some(notice) = self.core.sink_mut().take_notice() {
            self.notify(notice);
        }
        if back {
            self.switch_back_to_chosen_devices();
            status = self.core.sink().status().clone();
        }
        if self.watch_stream(&status) {
            // The stream was replaced: show what is true now.
            status = self.core.sink().status().clone();
        }
        status.notes_target = target;
        if self.silent && !status.running {
            status.error = Some("another song has the audio".to_string());
        }
        status.plugin_mode = self.core.plugin_mode();
        status.streaming = self.core.song_assets().values().any(|a| a.is_stream());
        status.faulted_name = status
            .faulted_device
            .and_then(|id| self.core.device_name_of_instance(id));
        if status.faulted && !self.fault_announced {
            self.fault_announced = true;
            let what = crate::status::faulted_text(&status);
            self.notify(format!(
                "{what}. Audio > Restart Audio rebuilds the engine; save your work first."
            ));
        } else if !status.faulted {
            self.fault_announced = false;
        }
        let (plugin_drops, late_batches) = self.core.plugin_overload();
        status.overload.plugin_drops = plugin_drops;
        status.overload.late_sandbox_batches = late_batches;
        self.overload = status.overload;
        self.audio_status = status.clone();
        self.log_overload();
        let context = crate::status::crash_context(&status);
        if context != self.crash_context {
            crate::crash::set_context(&context);
            self.crash_context = context;
        }
        if let Some(report) = crate::crash::flush_deferred() {
            self.notify(format!(
                "A panic was caught on an audio thread; the report is in {}",
                report.display()
            ));
        }
        let mut text = status_text(&status);
        if let Some(alert) = self.core.pdc_alert() {
            text = format!("{alert} \u{b7} {text}");
        }
        if let Some(saving) = self.core.save_progress() {
            text = format!("{saving} \u{b7} {text}");
        }
        if let Some(progress) = self.core.import_progress() {
            text = format!("{progress} \u{b7} {text}");
        }
        if let Some(progress) = self.core.notes_progress() {
            text = format!("{} \u{b7} {text}", progress.text);
        }
        self.sync_notes();
        if let Some(loading) = self.core.plugin_loading_text() {
            text = format!("{loading} \u{b7} {text}");
        }
        if let Some(progress) = self.core.freeze_progress() {
            text = format!("{progress} \u{b7} {text}");
        }
        self.sync_video_tab();
        if let Some((_, progress)) = self.core.video_progress() {
            text = format!("{progress} \u{b7} {text}");
        }
        if let Some(learn) = self.core.midi_status() {
            text = format!("{learn} \u{b7} {text}");
        }
        if let Some(lens) = self.core.lens_status() {
            text = format!("{lens} \u{b7} {text}");
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

    /// The mouse cursor to show in `window`. A drag in progress decides first
    /// ([`Self::drag_cursor`]); else the pane under the pointer: the rack's (the open hand over
    /// a draggable device), the piano roll's or the arrangement's while it is the visible song
    /// pane, the arrow elsewhere.
    pub fn cursor_in(&self, window: WindowKind) -> CursorHint {
        if let Some(hint) = self.drag_cursor() {
            return hint;
        }
        match window {
            WindowKind::Rack => self.core.view().cursor(),
            WindowKind::Main if self.ui.show_song => {
                let song = match self.song_pane() {
                    Pane::PianoRoll => self.core.piano_roll().cursor(),
                    Pane::Arrangement => self.core.arrangement().cursor(),
                    _ => CursorHint::Default,
                };
                if song == CursorHint::Default && !self.rack_detached() {
                    self.core.view().cursor()
                } else {
                    song
                }
            }
            WindowKind::Main if !self.rack_detached() => self.core.view().cursor(),
            _ => CursorHint::Default,
        }
    }

    /// The cursor of a drag that carries something, `None` when no such drag runs. Files
    /// dragged out of the file browser: the copy cursor over a place that takes them, the
    /// not-allowed cursor over every other place. A drag held by a pane (a device, a clip, a
    /// track, a palette row): the pane's own cursor, which turns not-allowed while the pointer
    /// is over another pane.
    fn drag_cursor(&self) -> Option<CursorHint> {
        match self.capture? {
            (_, Capture::FileDrag) => {
                let drag = self.file_drag.as_ref().filter(|d| !d.cancelled)?;
                let Some((window, pos)) = drag.pos else {
                    return Some(CursorHint::NotAllowed);
                };
                let target = self.file_target_in(window, pos);
                let library =
                    !drag.paths.is_empty() && drag.paths.iter().all(|p| is_library_item(p));
                let library_ok = match target {
                    Some(Pane::Rack) => true,
                    Some(Pane::Arrangement) => self.core.arrangement().clip_drop_hover().is_some(),
                    _ => false,
                };
                Some(crate::drag_cursor::file_drop_hint(
                    target.is_some(),
                    library,
                    library_ok,
                ))
            }
            // A browser row over a track header it can replace the device of: the rack's own
            // cursor, not the not-allowed one of another pane.
            (_, Capture::Pane(Pane::Rack))
                if self.capture_over == Some(Pane::Arrangement)
                    && self.core.arrangement().header_drop_target().is_some() =>
            {
                Some(self.core.view().cursor())
            }
            (_, Capture::Pane(pane)) => {
                let hint = match pane {
                    Pane::Rack => self.core.view().cursor(),
                    Pane::Arrangement => self.core.arrangement().cursor(),
                    Pane::PianoRoll => self.core.piano_roll().cursor(),
                    _ => return None,
                };
                Some(if self.capture_over == Some(pane) {
                    hint
                } else {
                    hint.outside_own_pane()
                })
            }
            (_, Capture::Divider { .. }) => None,
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
            WindowKind::SpectrumEq => {
                return self.repaint || win.dirty || self.core.spectrum_eq().needs_repaint();
            }
            WindowKind::Browser => {
                return self.repaint || win.dirty || self.core.file_browser().needs_repaint();
            }
            WindowKind::PianoKeys => return self.repaint || win.dirty,
            WindowKind::Video => {
                return self.repaint || win.dirty || self.core.video_window().needs_repaint();
            }
            WindowKind::Main => {}
        }
        self.repaint
            || win.dirty
            || (!self.rack_detached() && self.core.view().needs_repaint())
            || self.core.transport_bar().needs_repaint()
            || (self.ui.file_browser_visible
                && !self.browser_detached()
                && self.core.file_browser().needs_repaint())
            || (self.core.track_panel_visible() && self.core.track_panel().needs_repaint())
            || (self.ui.show_song
                && match self.song_pane() {
                    Pane::PianoRoll => self.core.piano_roll().needs_repaint(),
                    Pane::SampleEditor => self.core.sample_editor().needs_repaint(),
                    Pane::StepEditor => self.core.step_editor().needs_repaint(),
                    Pane::SliceEditor => self.core.slice_editor().needs_repaint(),
                    Pane::AudioEditor => self.core.audio_editor().needs_repaint(),
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
            WindowKind::Rack
            | WindowKind::Console
            | WindowKind::SpectrumEq
            | WindowKind::Video
            | WindowKind::PianoKeys
            | WindowKind::Browser => {
                self.paint_detached(window, canvas);
            }
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
        if l.track_panel.is_some() {
            self.core.sync_track_panel();
            self.core.paint_pane(Pane::TrackPanel, canvas);
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
        if let Some(view) = &self.mod_matrix {
            view.paint(canvas);
        }
        if let Some(view) = &self.param_search {
            view.paint(canvas);
        }
        if let Some(view) = &self.quantize {
            view.paint(canvas);
        }
        if let Some(view) = &self.scale {
            view.paint(canvas);
        }
        if let Some(view) = &self.blind {
            view.paint(canvas);
        }
        if let Some(view) = &self.silence {
            view.paint(canvas);
        }
        if let Some(view) = &self.song_info {
            view.paint(canvas);
        }
        if let Some(view) = &self.reverb_send {
            view.paint(canvas);
        }
        if let Some(view) = &self.kick_bass {
            view.paint(canvas);
        }
        if let Some(view) = &self.zone {
            view.paint(canvas);
        }
        if let Some(view) = &self.versions {
            view.paint(canvas);
        }
        if let Some(view) = &self.masking {
            view.paint(canvas);
        }
        if let Some(view) = &self.mixcheck {
            view.paint(canvas);
        }
        if let Some(view) = &self.repetition {
            view.paint(canvas);
        }
        if let Some(view) = &self.bass_read {
            view.paint(canvas);
        }
        if let Some(view) = &self.delivery {
            view.paint(canvas);
        }
        if let Some(view) = &self.sound_analyst {
            view.paint(canvas);
        }
        if let Some((view, _)) = &self.wavetable {
            view.paint(canvas);
        }
        if let Some((view, _)) = &self.color_picker {
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
            menubar.paint(canvas, &theme, &model, Size::new(w, h));
        }
        self.paint_overlays(WindowKind::Main, canvas, &theme, Size::new(w, h));
        if let Some(guide) = &self.guide
            && !self.modal_open()
        {
            if let Some(outline) = self.guide_outline() {
                crate::guide::paint_outline(canvas, &theme, outline);
            }
            guide.paint(canvas, &theme, self.guide_bounds());
        }
        if let Some(dialog) = &self.dialog {
            dialog.paint(canvas, &theme, self.size());
        }
        if let Some(about) = &self.about {
            about.paint(canvas, &theme, self.size());
        }
        if let Some(start) = &self.start_screen {
            start.paint(canvas, &theme, self.size());
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
        "files.delete" => (Key::Delete, Modifiers::NONE),
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
#[path = "shell_jacks_tests.rs"]
mod jacks_tests;

#[cfg(test)]
#[path = "shell_rack_menu_tests.rs"]
mod rack_menu_tests;

#[cfg(test)]
#[path = "shell_midimap_tests.rs"]
mod midimap_tests;

#[cfg(test)]
#[path = "shell_macros_tests.rs"]
mod macros_tests;

#[cfg(test)]
#[path = "shell_kick_bass_tests.rs"]
mod kick_bass_tests;

#[cfg(test)]
#[path = "shell_reverb_send_tests.rs"]
mod reverb_send_tests;

#[cfg(test)]
#[path = "shell_rumble_tests.rs"]
mod rumble_tests;

#[cfg(test)]
#[path = "shell_transition_tests.rs"]
mod transition_tests;

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
#[path = "shell_diagnostics_tests.rs"]
mod diagnostics_tests;

#[cfg(test)]
#[path = "shell_device_loss_tests.rs"]
mod device_loss_tests;

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
#[path = "shell_patch_tests.rs"]
mod shell_patch_tests;

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

#[cfg(test)]
#[path = "shell_save_tests.rs"]
mod save_tests;

#[cfg(test)]
#[path = "shell_templates_tests.rs"]
mod templates_tests;

#[cfg(test)]
#[path = "shell_video_tests.rs"]
mod video_tests;

#[cfg(test)]
#[path = "shell_versions_tests.rs"]
mod versions_tests;

#[cfg(test)]
#[path = "shell_masking_tests.rs"]
mod masking_tests;

#[cfg(test)]
#[path = "shell_mixcheck_tests.rs"]
mod mixcheck_tests;

#[cfg(test)]
#[path = "shell_energy_tests.rs"]
mod energy_tests;

#[cfg(test)]
#[path = "shell_translation_tests.rs"]
mod translation_tests;

#[cfg(test)]
#[path = "shell_delivery_tests.rs"]
mod delivery_tests;

#[cfg(test)]
#[path = "shell_sound_analyst_tests.rs"]
mod sound_analyst_tests;

#[cfg(test)]
#[path = "shell_wavetable_tests.rs"]
mod wavetable_tests;

#[cfg(test)]
#[path = "shell_piano_keys_tests.rs"]
mod piano_keys_tests;

#[cfg(test)]
#[path = "shell_interface_tests.rs"]
mod interface_tests;

#[cfg(test)]
#[path = "shell_edit_keys_tests.rs"]
mod edit_keys_tests;

#[cfg(test)]
#[path = "shell_start_screen_tests.rs"]
mod start_screen_tests;

#[cfg(test)]
#[path = "shell_guide_tests.rs"]
mod guide_tests;
