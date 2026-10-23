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

use crate::app_core::{AppCore, AppError, Pane};
use crate::commands::{AppCommand, Menu, MenuState, menus};
use crate::context_menus::{self, MenuEnable};
use crate::keymap_store::{self, WorkflowPrefs};
use crate::menubar::{MENU_BAR_HEIGHT, MenuBar, MenuBarOutcome};
use crate::session::{AudioBackend, LiveSession, SettingsChange};
use crate::settings::{self, AudioSettings};
use crate::settings_dialog::{DeviceLists, DialogOutcome, SettingsDialog};
use crate::status::status_text;
use crate::stress::{STRESS_CABLES, STRESS_DEVICES, stress_doc, stress_song_doc};
use crate::ui_state::UiState;
use atomik_model::{ClipId, Command, ConsoleCommand, DeviceKey};
use atomik_ui::ContextTarget;
use atomik_ui::browser::CreateKind;
use atomik_ui::context_menu::{ContextMenu, MenuOutcome};
use atomik_ui::keymap::{
    Context, ContextStack, Keymap, KeymapEditor, Os, Profile, Roles, Route, Router, UserKeymap,
};
use atomik_ui::kurbo::{Affine, Point, Rect, RoundedRect, Size};
use atomik_ui::transport::TRANSPORT_HEIGHT;
use atomik_ui::{
    BrowseAction, Canvas, CanvasExt, ConsoleButton, FileBrowserView, Flip, Key, Modifiers,
    NumpadKey, Place, PluginBannerView, PluginSettingsView, PointerButton, TextRun, UiAction,
    UiEvent,
};
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    /// Show a multi-select open dialog for audio files (the sample editor's Load button), then
    /// call [`Shell::load_samples`] with the picked paths.
    PickLoadSamples,
    /// The rack has unsaved changes: ask, then call [`Shell::confirmed`] with the command.
    ConfirmDiscard(AppCommand),
    /// Show a folder dialog (the Plugins settings' ADD FOLDER), then call
    /// [`Shell::add_plugin_folder`] with the picked folder.
    PickPluginFolder,
    /// Show a save-file dialog for the keymap (the Keyboard tab's Export), then call
    /// [`Shell::export_keymap_to`].
    PickExportKeymap,
    /// Show an open-file dialog for a keymap (the Keyboard tab's Import), then call
    /// [`Shell::import_keymap_from`].
    PickImportKeymap,
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
    /// Where the pointer last was (the label follows it); `None` before the first move.
    pos: Option<Point>,
    /// Esc was pressed: nothing is delivered any more; the release ends the gesture.
    cancelled: bool,
}

/// An open context menu and the pane whose selection its commands work on.
struct OpenContextMenu {
    menu: ContextMenu,
    pane: Pane,
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
    ui_state_path: Option<PathBuf>,
    ui: UiState,
    size: Size,
    scale: f64,
    status: String,
    repaint: bool,
    capture: Option<Capture>,
    hover: Option<Pane>,
    focus: Pane,
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
    /// Modification time and size of `keymap.json` as last read or written, to notice edits.
    keymap_stamp: Option<(SystemTime, u64)>,
    /// The layout F7 (sequencer only) replaced, to restore on the next F7.
    seq_only_prev: Option<SeqOnlyPrev>,
    /// The right edge of the file browser column as last laid out (0 while hidden), to notice a
    /// resize drag of the panel.
    browser_edge: f64,
    /// The file drag in progress (see [`Capture::FileDrag`]).
    file_drag: Option<FileDrag>,
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
            .field("size", &self.size)
            .field("scale", &self.scale)
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
        let options_library_path = options.library_path.clone();
        let mut shell = Self {
            core,
            menubar: options.in_window_menu.then(MenuBar::new),
            context_menu: None,
            dialog: None,
            settings_path: options.settings_path,
            ui_state_path: options.ui_state_path,
            ui,
            size: Size::new(1280.0, 800.0),
            scale: 1.0,
            status: String::new(),
            repaint: true,
            capture: None,
            hover: None,
            focus: Pane::Rack,
            pending: Vec::new(),
            load_device: None,
            banner,
            plugin_settings: None,
            router: Router::new(built.keymap),
            modifiers: Modifiers::NONE,
            os,
            default_profile,
            keymap_path,
            workflow_path,
            user_keymap,
            roles: built.roles,
            workflow,
            keymap_stamp,
            seq_only_prev: None,
            browser_edge: 0.0,
            file_drag: None,
        };
        shell.core.set_roles(&shell.roles);
        shell.core.set_library_path(options_library_path);
        // Before anything saves the window state: a save copies the panel's values.
        shell.restore_file_browser();
        shell.apply_workflow_to_views();
        // The rack's console display preferences (shown unless the user
        // switched them off).
        {
            let view = shell.core.view_mut();
            view.set_show_console_devices(shell.ui.show_console_devices);
            view.set_show_console_links(shell.ui.show_console_links);
            view.set_navigator_visible(shell.ui.show_rack_navigator);
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
            .view_mut()
            .set_create_keeps_browser_open(self.workflow.create_keeps_browser_open);
        self.core
            .view_mut()
            .set_auto_route(self.workflow.auto_route);
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
            console: self.console_shown(),
            console_maximised: self.console_shown() && self.ui.console_maximised,
            compensation: self.core.doc().console.compensation,
            console_devices: view.show_console_devices(),
            console_links: view.show_console_links(),
            rack_navigator: view.navigator_visible(),
            rack: self.workflow.rack_visible,
            sequencer_only: self.in_sequencer_only(),
            editor: !self.ui.show_console
                && !matches!(self.lower_pane(), Pane::Arrangement | Pane::Console),
        }
    }

    /// Whether the console shows (in the lower pane, or maximised).
    fn console_shown(&self) -> bool {
        self.ui.show_song && self.ui.show_console
    }

    fn maximised(&self) -> bool {
        self.console_shown() && self.ui.console_maximised
    }

    /// The rack is hidden by F6 (the song area shows; a maximised console hides it too, but
    /// that is its own state).
    fn rack_hidden(&self) -> bool {
        self.ui.show_song && !self.workflow.rack_visible && !self.maximised()
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
        let bottom = (self.size.height - STATUS_BAR_HEIGHT - margin).max(top + 1.0);
        Rect::new(
            margin,
            top,
            (self.size.width - margin).max(margin + 1.0),
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
        let bottom = (self.size.height - STATUS_BAR_HEIGHT).max(top + 1.0);
        let most = (self.size.width - MIN_REST_WIDTH).max(0.0);
        let least = FileBrowserView::MIN_WIDTH.min(most);
        let width = self.core.file_browser().width().clamp(least, most);
        Some(Rect::new(0.0, top, width, bottom))
    }

    /// Where the rack, divider, transport bar and song area are (module docs).
    pub fn layout(&self) -> Layout {
        let fb = self.file_browser_rect();
        let x0 = fb.map_or(0.0, |r| r.x1);
        let w = (self.size.width - x0).max(1.0);
        let x1 = x0 + w;
        let top = self.top();
        let bottom = (self.size.height - STATUS_BAR_HEIGHT).max(top + 1.0);
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
        if self.rack_hidden() {
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
            (self.size.width * self.scale).ceil().max(1.0) as u32,
            (self.size.height * self.scale).ceil().max(1.0) as u32,
        )
    }

    fn layout_view(&mut self) {
        let l = self.layout();
        // A maximised console hides the rack: it keeps its last size.
        if l.rack.height() > 0.0 {
            self.core.handle_event(&UiEvent::Resize(l.rack.size()));
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
            self.core.console_mut().set_bounds(song);
        }
        let bar = self.bar_height();
        let banner_x = l.file_browser.map_or(0.0, |r| r.x1);
        self.banner.set_bounds(Rect::new(
            banner_x,
            bar,
            self.size.width.max(banner_x),
            bar + self.banner.height(),
        ));
        if self.plugin_settings.is_some() {
            let rect = self.plugin_modal_rect();
            if let Some(v) = &mut self.plugin_settings {
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
        if self.ui.show_console {
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
            Pane::Rack if self.rack_hidden() => self.song_pane(),
            other => other,
        }
    }

    fn set_focus(&mut self, pane: Pane) {
        self.focus = pane;
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
        self.hit(pos).and_then(Result::ok)
    }

    /// What is under a window point: a pane, the divider (`Err(())`), or nothing.
    fn hit(&self, pos: Point) -> Option<Result<Pane, ()>> {
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

    /// Handles one window event (logical units).
    pub fn handle(&mut self, event: &UiEvent) -> Vec<ShellRequest> {
        let mut requests = self.handle_event_inner(event);
        requests.append(&mut self.pending);
        requests
    }

    /// File drag and drop. Hovers and drops go to the view under the
    /// pointer: the arrangement (it maps them to a track and a snapped tick and emits
    /// `UiAction::ImportAudio`), the sample editor (`UiAction::ImportSamples`) or the rack
    /// (a sampler panel emits `ImportSamples`). The other views get a hover cancel. A drop
    /// that lands on nothing that takes files (the console, the transport bar, a rack device
    /// that is not a sampler) imports the files as new tracks at the snapped playhead.
    fn file_event(&mut self, event: &UiEvent) {
        self.repaint = true;
        let target = match event {
            UiEvent::FileHover { pos, .. } | UiEvent::FileDrop { pos, .. } => {
                self.file_target(*pos)
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
            UiEvent::FileHover { .. } => {
                if let Some(pane) = target {
                    self.forward(pane, event);
                }
            }
            UiEvent::FileDrop { paths, .. } => {
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

    /// A pointer event during a drag out of the file browser: moves are file hovers over
    /// whatever takes files under the pointer (cancels over the panel or anything else), the
    /// release is a file drop exactly like an OS drop, and a release over the panel or over
    /// nothing that takes files cancels.
    fn file_drag_event(&mut self, event: &UiEvent, pos: Point) {
        self.repaint = true;
        let Some(drag) = self.file_drag.as_mut() else {
            return;
        };
        drag.pos = Some(pos);
        if drag.cancelled {
            return;
        }
        let paths = drag.paths.clone();
        match event {
            UiEvent::PointerMove { .. } => self.file_event(&UiEvent::FileHover { pos, paths }),
            UiEvent::PointerUp { .. } => {
                let over_panel = self.hit_pane(pos) == Some(Pane::FileBrowser);
                if !over_panel && self.file_target(pos).is_some() {
                    self.file_event(&UiEvent::FileDrop { pos, paths });
                } else {
                    self.file_event(&UiEvent::FileHoverCancel);
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
        self.file_event(&UiEvent::FileHoverCancel);
    }

    /// The pane files at a window point go to, if it takes files.
    fn file_target(&self, pos: Point) -> Option<Pane> {
        match self.hit(pos)? {
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

    fn handle_event_inner(&mut self, event: &UiEvent) -> Vec<ShellRequest> {
        if matches!(
            event,
            UiEvent::FileHover { .. } | UiEvent::FileHoverCancel | UiEvent::FileDrop { .. }
        ) {
            self.file_event(event);
            return Vec::new();
        }
        match event {
            UiEvent::ScaleChanged(scale) => {
                self.scale = *scale;
                self.core.handle_event(event);
                self.core.arrangement_mut().set_scale(*scale);
                self.core.piano_roll_mut().set_scale(*scale);
                self.core.sample_editor_mut().set_scale(*scale);
                self.core.step_editor_mut().set_scale(*scale);
                self.core.slice_editor_mut().set_scale(*scale);
                self.core.seq_editor_mut().set_scale(*scale);
                self.core.container_editor_mut().set_scale(*scale);
                self.core.plugin_params_mut().set_scale(*scale);
                self.banner.set_scale(*scale);
                if let Some(v) = &mut self.plugin_settings {
                    v.set_scale(*scale);
                }
                self.core.transport_bar_mut().set_scale(*scale);
                self.core.console_mut().set_scale(*scale);
                self.core.file_browser_mut().set_scale(*scale);
                self.repaint = true;
                return Vec::new();
            }
            UiEvent::Resize(size) => {
                self.size = *size;
                self.context_menu = None;
                self.layout_view();
                return Vec::new();
            }
            _ => {}
        }
        if let UiEvent::ModifiersChanged { modifiers } = event {
            // Held-modifier state only; nothing repaints until a role depends on it. The
            // settings dialog learns a role from the modifiers held.
            self.modifiers = *modifiers;
            if let Some(dialog) = &mut self.dialog
                && dialog.capturing_keys()
            {
                dialog.handle(event, self.size);
                self.repaint = true;
            }
            self.forward_modifiers();
            return Vec::new();
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
        if let Some(dialog) = &mut self.dialog {
            self.repaint = true;
            // Learning a chord needs the keys as they are pressed (numpad keys included).
            let event = if dialog.capturing_keys() {
                event.clone()
            } else {
                text_form(event)
            };
            match dialog.handle(&event, self.size) {
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
            }
            return Vec::new();
        }
        if self.context_menu.is_some() {
            return self.context_menu_event(event);
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
            if self.core.transport_bar().is_editing_tempo() {
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
        if let Some(capture) = self.capture
            && let Some(pos) = pointer_pos(event)
        {
            let released = matches!(event, UiEvent::PointerUp { .. });
            match capture {
                Capture::Pane(pane) => {
                    self.forward(pane, event);
                }
                Capture::Divider { grab } => self.drag_divider(pos, grab, released),
                Capture::FileDrag => self.file_drag_event(event, pos),
            }
            if released {
                self.capture = None;
                self.file_drag = None;
            }
            return Vec::new();
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
            match bar.handle(event, &model) {
                MenuBarOutcome::Ignored => {}
                MenuBarOutcome::Consumed => {
                    self.repaint = true;
                    self.leave_hover();
                    return Vec::new();
                }
                MenuBarOutcome::Command(command) => {
                    self.repaint = true;
                    return self.execute(command);
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
        match self.hit(pos) {
            None => self.leave_hover(),
            Some(Err(())) => {
                self.leave_hover();
                if matches!(event, UiEvent::PointerDown { .. })
                    && let Some(d) = self.layout().divider
                {
                    self.capture = Some(Capture::Divider { grab: pos.y - d.y0 });
                }
            }
            Some(Ok(pane)) => {
                if self.hover != Some(pane) {
                    self.leave_hover();
                    self.hover = Some(pane);
                }
                if matches!(event, UiEvent::PointerDown { .. }) {
                    self.set_focus(pane);
                    self.capture = Some(Capture::Pane(pane));
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
                return Some(self.execute(command));
            }
        }
        self.repaint = true;
        self.core.after_view_change();
        Some(Vec::new())
    }

    /// Shows the rack again when F6 or F7 hid it (a command that needs it, such as the create
    /// browser, calls this); a maximised console steps back too.
    fn show_rack(&mut self) {
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
        self.ui.show_song && !self.workflow.rack_visible && !self.ui.show_console
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

    /// The song area shows the editors and the arrangement, not the console or nothing.
    fn reveal_lower_pane(&mut self) {
        if !self.ui.show_song || self.ui.show_console {
            self.ui.show_song = true;
            self.ui.show_console = false;
            self.console_changed();
        }
    }

    /// Gives the keys to the editor that shows in the lower pane, if one does.
    fn focus_editor(&mut self) {
        let lower = self.lower_pane();
        if !self.ui.show_console && !matches!(lower, Pane::Arrangement | Pane::Console) {
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
            self.execute(AppCommand::TogglePianoRoll);
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

    fn leave_hover(&mut self) {
        if let Some(pane) = self.hover.take() {
            self.forward(pane, &UiEvent::PointerLeave);
        }
    }

    /// Sends a key no command took to the focused view (numpad keys as the characters they
    /// type).
    fn route_key(&mut self, event: &UiEvent) {
        let focus = self.live_pane(self.focus);
        self.forward(focus, &text_form(event));
    }

    fn drag_divider(&mut self, pos: Point, grab: f64, released: bool) {
        let top = self.top();
        let bottom = (self.size.height - STATUS_BAR_HEIGHT).max(top + 1.0);
        let avail = (bottom - top - TRANSPORT_HEIGHT - DIVIDER_HEIGHT).max(2.0);
        let rack_h = (pos.y - grab - top).clamp(0.0, avail);
        let lo = MIN_PANE_HEIGHT.min(avail / 2.0);
        self.ui.rack_split = rack_h.clamp(lo, avail - lo) / avail;
        self.layout_view();
        if released {
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
        self.repaint = true;
        if pane != Pane::Rack {
            let actions = self.core.handle_pane_event(pane, event);
            if pane == Pane::Console && self.core.take_rack_focus_request() {
                self.set_focus(Pane::Rack);
            }
            if pane == Pane::FileBrowser {
                self.follow_browser_resize();
            }
            self.pane_actions(&actions);
            self.open_context_menus(pane, &actions);
            return actions;
        }
        let origin = self.layout().rack.origin();
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
        }
    }

    /// Opens the context menu of `target` at the window point `at`, built from the registry.
    fn open_context_menu(&mut self, pane: Pane, target: ContextTarget, at: Point) {
        let enable = self.menu_enable(pane);
        let specs = context_menus::specs(target, &enable);
        let menu = ContextMenu::build(
            &specs,
            &atomik_ui::keymap::Registry::bundled(),
            self.router.keymap(),
            at,
            self.size,
        );
        self.context_menu = Some(OpenContextMenu { menu, pane });
        // The pointer press that opened it must not keep a drag alive.
        self.capture = None;
        self.leave_hover();
        self.repaint = true;
    }

    /// The open context menu, if any.
    pub fn context_menu(&self) -> Option<&ContextMenu> {
        self.context_menu.as_ref().map(|o| &o.menu)
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
    fn context_menu_event(&mut self, event: &UiEvent) -> Vec<ShellRequest> {
        self.repaint = true;
        let Some(open) = &mut self.context_menu else {
            return Vec::new();
        };
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
            return Vec::new();
        }
        let pane = open.pane;
        match open.menu.handle(event) {
            MenuOutcome::Run { command, arg } => {
                self.context_menu = None;
                self.set_focus(pane);
                self.run_command(&command, arg.as_deref())
                    .unwrap_or_default()
            }
            MenuOutcome::Consumed | MenuOutcome::Ignored => Vec::new(),
        }
    }

    /// The context-menu requests among the actions a pane emitted: `at` is in the pane's
    /// coordinates (the rack view sits below the menu bar).
    fn open_context_menus(&mut self, pane: Pane, actions: &[UiAction]) {
        for action in actions {
            if let UiAction::ContextMenu { target, at } = action {
                let at = if pane == Pane::Rack {
                    {
                        let o = self.layout().rack.origin();
                        Point::new(at.x + o.x, at.y + o.y)
                    }
                } else {
                    *at
                };
                self.open_context_menu(pane, *target, at);
            }
        }
    }

    /// Runs the view actions only the shell can do (the console toolbar's pane switches).
    fn pane_actions(&mut self, actions: &[UiAction]) {
        for action in actions {
            match action {
                UiAction::ToggleConsole => {
                    self.execute(AppCommand::ToggleConsole);
                }
                UiAction::MaximiseConsole => {
                    self.execute(AppCommand::MaximiseConsole);
                }
                UiAction::ImportAudioDialog => self.pending.push(ShellRequest::PickImportFiles),
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
                    self.capture = Some(Capture::FileDrag);
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
        if !self.ui.show_song || self.ui.show_console {
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
        if !self.ui.show_song || self.ui.show_console {
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
        if !self.ui.show_song || self.ui.show_console {
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
        if !self.ui.show_song || self.ui.show_console {
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
        if !self.ui.show_song || self.ui.show_console {
            self.ui.show_song = true;
            self.ui.show_console = false;
            self.console_changed();
        }
        self.layout_view();
        self.set_focus(Pane::PluginParams);
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
        if !self.ui.show_song || self.ui.show_console {
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
        if !self.console_shown() && self.focus == Pane::Console {
            self.set_focus(self.live_pane(Pane::Arrangement));
        }
        self.layout_view();
        self.save_ui_state();
    }

    /// Runs a menu or shortcut command.
    pub fn execute(&mut self, command: AppCommand) -> Vec<ShellRequest> {
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
                self.ui.show_song = !self.ui.show_song;
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
                    if !self.ui.show_song || self.ui.show_console {
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
            AppCommand::CreateMixerChannel => self.execute(AppCommand::AddChannel),
            AppCommand::ToggleRack => {
                self.set_rack_visible(!self.workflow.rack_visible);
                Vec::new()
            }
            AppCommand::SequencerOnly => {
                self.toggle_sequencer_only();
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
            AppCommand::ToggleRackNavigator => {
                let view = self.core.view_mut();
                let show = !view.navigator_visible();
                view.set_navigator_visible(show);
                self.ui.show_rack_navigator = show;
                self.save_ui_state();
                Vec::new()
            }
            AppCommand::ToggleConsole => {
                if self.console_shown() {
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
                if !self.console_shown() {
                    self.execute(AppCommand::ToggleConsole);
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
            AppCommand::ToggleCompensation => {
                let on = self.core.doc().console.compensation;
                let command = Command::Console(ConsoleCommand::SetCompensation(!on));
                // A rejection is already reported in the view.
                let _ = self.core.apply_command(command);
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
                view.set_scale(self.scale);
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
            other => self.execute(other),
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

    /// Shows a message in the view until the next click.
    pub fn notify(&mut self, message: impl Into<String>) {
        self.core.report(message);
    }

    /// Per-frame work: engine garbage, the transport position and recorded notes.
    pub fn frame(&mut self) {
        let busy = self.core.is_busy();
        self.core.frame();
        self.sync_plugin_ui();
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
        if self.console_shown() {
            self.core.update_console_meters(now_seconds);
        }
        if self.core.view().flip() == atomik_ui::Flip::Back {
            self.core.feed_cv_activity(now_seconds);
        }
    }

    fn apply_settings(&mut self, settings: AudioSettings) {
        let gain = settings.metronome_gain;
        let session = self.core.sink_mut();
        if session.set_settings(settings.clone()) == SettingsChange::RestartAudio {
            session.set_enabled(true);
            self.core.rebuild_engine();
        }
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
        let changed = text != self.status;
        if changed {
            self.status = text;
            self.repaint = true;
        }
        changed
    }

    /// Whether anything must be repainted.
    pub fn needs_repaint(&self) -> bool {
        self.repaint
            || self.core.view().needs_repaint()
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

    /// Paints the whole window content in physical pixels.
    pub fn paint(&mut self, canvas: &mut dyn Canvas) {
        let scale = self.scale;
        let l = self.layout();
        let (w, h) = (self.size.width, self.size.height);
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
        if let Some(open) = &self.context_menu {
            open.menu.paint(canvas, &theme);
        }
        if let Some(drag) = &self.file_drag
            && !drag.cancelled
            && let Some(pos) = drag.pos
        {
            let text = drag_label(&drag.paths);
            let width = text.chars().count() as f64 * 7.0 + 16.0;
            let at = Point::new(
                (pos.x + 14.0).min((w - width).max(0.0)),
                (pos.y + 16.0).min(h - 24.0),
            );
            let label = Rect::new(at.x, at.y, at.x + width, at.y + 20.0);
            canvas.fill_with(RoundedRect::from_rect(label, 4.0), theme.browser_row);
            canvas.text(&TextRun::new(
                text,
                Point::new(at.x + 8.0, at.y + 14.0),
                12.0,
                theme.text,
            ));
        }
        if let Some(dialog) = &self.dialog {
            dialog.paint(canvas, &theme, self.size);
        }
        canvas.pop_transform();
        self.repaint = false;
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

fn pointer_pos(event: &UiEvent) -> Option<Point> {
    match event {
        UiEvent::PointerMove { pos, .. }
        | UiEvent::PointerDown { pos, .. }
        | UiEvent::PointerUp { pos, .. }
        | UiEvent::Wheel { pos, .. } => Some(*pos),
        _ => None,
    }
}

#[cfg(test)]
#[path = "shell_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "shell_song_tests.rs"]
mod song_tests;

#[cfg(test)]
#[path = "shell_audio_tests.rs"]
mod audio_tests;

#[cfg(test)]
#[path = "shell_console_tests.rs"]
mod console_tests;

#[cfg(test)]
#[path = "shell_sampler_tests.rs"]
mod sampler_tests;

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
