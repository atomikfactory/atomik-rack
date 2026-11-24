//! The winit 0.30 application: the main window and the detached rack and mixer windows, event
//! translation and routing, frame pacing, dialogs.

use super::cursor;
use super::dock_frames::DockFrames;
use super::gpu::{FrameResult, GpuContext, WindowSurface, render_offscreen, write_png};
use super::gpu_recovery::{self, ForcedLoss, Recovery};
use super::gpu_select::{self, AdapterDesc};
use super::learn_strips::{Dock, LearnStrips};
use super::plugin_windows::PluginWindows;
use super::window_logic;
use crate::app_core::{AppCore, default_registry};
use crate::cli::Cli;
use crate::clicks::ClickCounter;
use crate::commands::AppCommand;
use crate::cursor_art::{self, CursorArt};
use crate::plugin_setup;
use crate::session::{LiveSession, SystemBackend};
use crate::settings::{self, AudioSettings, default_settings_path};
use crate::shell::{Shell, ShellOptions, ShellRequest, VideoPick};
use crate::stress::{
    STRESS_AUDIO_THRESHOLD_BYTES, add_stress_character, add_stress_containers, add_stress_drums,
    add_stress_fx, add_stress_players, add_stress_plugins, add_stress_samplers, add_stress_slicers,
    stress_audio_dir, stress_drums_dir, stress_sampler_dir, stress_slicer_dir, write_stress_audio,
};
use crate::translate::{self, FileDrag};
use crate::ui_activity::PaintReason;
use crate::ui_guard::{self, Dispatch, ForcedPanic, UiGuard};
use crate::ui_state::ui_state_path;
use crate::windows::{self, DocId, MonitorInfo, WindowHit, WindowKind, WindowRect, ZOrder};
use crate::workspace::Workspace;
use atomik_model::DeviceKey;
use atomik_ui::TransportAction;
use atomik_ui::keymap::Os;
use atomik_ui::kurbo::Point;
use atomik_ui::{CursorHint, Modifiers, SceneCache, SceneCanvas, TextEngine, UiEvent};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use vello::Scene;
use vello::peniko::Color;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{CustomCursor, Window, WindowId};

/// A window of the app: the song it belongs to and its place in that song.
type Key = (DocId, WindowKind);

/// Redraw tick while audio runs (≈ 60 Hz, for meters and status).
const FRAME: Duration = Duration::from_micros(16_667);
/// Status bar refresh period.
const STATUS_PERIOD: Duration = Duration::from_millis(250);
/// How often the cursor is read while files from the system hover over a window and the system
/// sends no cursor events for the drag.
const FILE_DRAG_POLL: Duration = Duration::from_millis(30);

mod clock {
    use std::time::Instant;

    /// Wall clock for UI timing only (click counting, frame pacing, status refresh).
    #[allow(clippy::disallowed_methods)] // Justification: UI-thread timing only; it never
    // reaches the engine or any rendered audio.
    pub(super) fn now() -> Instant {
        Instant::now()
    }
}

/// Runs the app until the window closes.
pub fn run(cli: Cli) -> Result<(), String> {
    // The one place the language of the machine is read: the System choice of the language
    // setting follows it.
    atomik_ui::i18n::set_system_locale(sys_locale::get_locale());
    let settings_path = cli.settings.clone().or_else(default_settings_path);
    let (saved, settings_error) = match &settings_path {
        Some(path) => settings::load(path),
        None => (AudioSettings::default(), None),
    };
    let config = settings::config_folder(settings_path.as_deref());
    let mut settings = cli.apply(saved);
    if cli.stress_audio && settings.stream_threshold_bytes.is_none() {
        settings.stream_threshold_bytes = Some(STRESS_AUDIO_THRESHOLD_BYTES);
    }
    let registry = default_registry();
    let mut open_error = None;
    let doc = cli.start_document(&registry);
    // Third-party plugins: the cache, the journal and the scanner come from the config
    // directory and the executable's folder; plugins stay off until the user enables them, except
    // for this run of --stress-plugins.
    let exe = std::env::current_exe().ok();
    let mut plugin_settings = settings.plugins.clone();
    if cli.stress_plugins {
        plugin_settings.enabled = true;
    }
    let plugin_settings_sandbox = plugin_settings.sandbox;
    let plugins_on = plugin_settings.enabled;
    let mut plugin_config = plugin_setup::plugin_config(
        plugin_settings,
        Some(config.dir.as_path()),
        exe.as_deref().map(plugin_setup::scanner_command),
    );
    // Where plugins run is read once, here: the Settings switch applies at the next start.
    let (isolation, isolation_warning) =
        plugin_setup::plugin_isolation(plugin_settings_sandbox, exe.as_deref());
    plugin_config.isolation = Some(isolation);
    // What a song opened later is built from (see `song_factory` below).
    let song_runner = plugin_config.runner.clone();
    let song_isolation = plugin_config.isolation.clone();
    let song_config_dir = config.dir.clone();
    let song_settings_path = settings_path.clone();
    let song_library_root = cli.library.clone();
    if cli.stress_plugins
        && let Some(exe) = exe.as_deref()
    {
        // The in-repo test plugins (CLAP, and VST3 when built) are scanned like any plugin: by the
        // scanner program, out of process (the cache remembers them).
        let result = plugin_setup::scan_stress_plugins(
            &mut plugin_config,
            exe,
            std::time::Duration::from_secs(30),
        );
        if let Err(e) = result {
            open_error = Some(format!("stress plugins: {e}"));
        }
    }
    let session = LiveSession::new(SystemBackend, settings, !cli.no_audio);
    let mut core = AppCore::with_doc(session, doc, None);
    install_video(&mut core);
    // The recent songs are read before a project named on the command line is opened, so that
    // it takes its place in the list.
    if let Some(path) = settings_path.as_deref().map(crate::recent::recent_path) {
        core.set_recent_path(path);
    }
    core.configure_plugins(plugin_config);
    if let Some(warning) = isolation_warning.filter(|_| plugins_on) {
        core.warn_plugin_isolation(&warning);
    }
    if cli.stress_audio {
        // 16 generated audio tracks next to the 8 instrument tracks (files are reused
        // between runs); wait for the import so playback starts with everything loaded.
        let rate = core.doc().sample_rate;
        match write_stress_audio(&stress_audio_dir(), rate) {
            Ok(paths) => {
                core.import_audio(paths, atomik_ui::ImportTarget::NewTracks, 0);
                core.wait_background();
            }
            Err(e) => open_error = Some(format!("stress audio: {e}")),
        }
    }
    if cli.stress_sampler {
        // 4 samplers x 32 voices, with generated samples dropped through the normal
        // import path (files are reused between runs).
        if let Err(e) = add_stress_samplers(&mut core, &stress_sampler_dir()) {
            open_error = Some(format!("stress sampler: {e}"));
        }
    }
    if cli.stress_drums {
        // 4 drum machines with 16 loaded pads and dense 1/32 patterns, generated sounds
        // dropped through the normal import path (files are reused between runs).
        if let Err(e) = add_stress_drums(&mut core, &stress_drums_dir()) {
            open_error = Some(format!("stress drums: {e}"));
        }
    }
    if cli.stress_slicer {
        // 4 loop slicers with a generated 2-bar loop sliced at its transients and dense
        // retriggered 1/32 patterns (the loop goes through the normal import path).
        if let Err(e) = add_stress_slicers(&mut core, &stress_slicer_dir()) {
            open_error = Some(format!("stress slicer: {e}"));
        }
    }
    if cli.stress_players {
        // 4 sequencer -> arpeggiator -> synth chains with dense 1/32 patterns,
        // added before the effects so the fx load still lands on the first eight channels.
        if let Err(e) = add_stress_players(&mut core) {
            open_error = Some(format!("stress players: {e}"));
        }
    }
    if cli.stress_containers {
        // 8 containers (synth, compressor, delay, nested effect container) with
        // macros moving by automation, added before the effects like the players.
        if let Err(e) = add_stress_containers(&mut core) {
            open_error = Some(format!("stress containers: {e}"));
        }
    }
    if cli.stress_character {
        // 8 synth-into-Character containers whose presets step every bar through
        // all 16, added before the effects like the containers.
        if let Err(e) = add_stress_character(&mut core) {
            open_error = Some(format!("stress character: {e}"));
        }
    }
    if cli.stress_plugins {
        // 8 test synths (CLAP and VST3) with dense chords and 8 test effects, created one per
        // frame like a project open; waited for here so playback starts with everything loaded.
        match add_stress_plugins(&mut core, cli.stress_plugins_real) {
            Ok(()) => core.wait_plugins(),
            Err(e) => open_error = Some(e),
        }
    }
    if cli.stress_fx {
        // 8 effect inserts, 2 reverbs on returns and mastering on the master,
        // added after the other loads so the first eight (instrument) channels get the inserts.
        if let Err(e) = add_stress_fx(&mut core) {
            open_error = Some(format!("stress fx: {e}"));
        }
    }
    // The project or patch given on the command line opens like File > Open: a project with
    // audio finds its bundle and loads the assets in the background.
    if let Some(path) = cli
        .patch
        .as_ref()
        .filter(|_| !cli.stress && !cli.stress_song_loads())
        && let Err(e) = core.open(path)
    {
        open_error = Some(format!("{}: {e}", path.display()));
    }
    let mut shell = Shell::new(
        core,
        ShellOptions {
            os: None,
            profile: None,
            in_window_menu: !cfg!(target_os = "macos"),
            ui_state_path: settings_path.as_deref().map(ui_state_path),
            library_path: settings_path.as_deref().map(crate::ui_state::library_path),
            library_root: cli.library.clone(),
            settings_path,
        },
    );
    shell.set_library_default(crate::settings::default_library_dir(
        crate::settings::TargetOs::current(),
        &|k| std::env::var_os(k),
    ));
    let factory_folder = crate::factory::locate();
    shell.set_factory_folder(factory_folder.clone());
    shell.apply_language_globally();
    // Creating a plugin blocks this thread: show LOADING one frame before it starts.
    shell.core_mut().set_plugin_load_paints_first(true);
    // A save writes beside the window instead of freezing it.
    // Panels nobody looks at are not analysed.
    shell.core_mut().set_analyse_only_shown(true);
    shell.core_mut().set_background_save(true);
    if let Some(path) = cli.keymap.clone() {
        shell.use_keymap_file(path);
    }
    // With the preference on, a start that names no project continues with the last song.
    // Otherwise the default song, when one is chosen, takes the place of the starter project,
    // and the start screen offers the new song, the templates and the recent songs.
    // The very first start (no settings file yet) opens the audio and MIDI setup before the
    // start screen; the update question waits behind it. A screenshot or stress run skips it.
    if cli.screenshot.is_none() && !cli.stress && !cli.stress_song_loads() {
        shell.first_run_setup();
    }
    if cli.starts_empty() {
        shell.start_without_project();
    }
    // The program's only network client, installed here and nowhere else (a shell without one
    // cannot reach the network). It is used for the optional update check: with no answer to the
    // question yet the notice asks it and nothing is sent. A screenshot or stress run never checks.
    shell.set_update_http(Arc::new(atomik_update::UreqGet));
    // The camera, the hand models and their one-time download: the application window only, and
    // never before the user turns the camera on.
    shell
        .core_mut()
        .set_vision(Arc::new(crate::vision_link::SystemVision::new(&config.dir)));
    if cli.screenshot.is_none() && !cli.stress && !cli.stress_song_loads() {
        shell.start_update_check();
    }
    let no_config = config
        .fallback
        .then(|| settings::NO_CONFIG_NOTE.to_string());
    for message in [settings_error.map(|e| e.to_string()), open_error, no_config]
        .into_iter()
        .flatten()
    {
        log::warn!("{message}");
        shell.notify(message);
    }
    if cli.back {
        shell.execute(AppCommand::ToggleFlip);
    }
    if let Some(key) = &cli.select {
        shell.core_mut().select(Some(DeviceKey(key.clone())));
    }
    if cli.piano_roll && shell.core().piano_roll_clip().is_none() {
        shell.execute(AppCommand::TogglePianoRoll);
    }
    if cli.console_max {
        if !shell.menu_state().console_maximised {
            shell.execute(AppCommand::MaximiseConsole);
        }
    } else if cli.console {
        // The console in the lower pane, not maximised (whatever ui-state.json said).
        if shell.menu_state().console_maximised {
            shell.execute(AppCommand::MaximiseConsole);
        }
        if !shell.menu_state().console {
            shell.execute(AppCommand::ToggleConsole);
        }
    }
    if let Some(folder) = &cli.file_browser {
        if let Some(dir) = folder {
            shell.core_mut().set_file_browser_folder(Some(dir.clone()));
        }
        if !shell.menu_state().file_browser {
            shell.execute(AppCommand::ToggleFileBrowser);
        }
    }
    // Panes in windows of their own at start, on top of the ones ui-state.json restored.
    for kind in &cli.detach {
        let command = match kind {
            WindowKind::Console => AppCommand::DetachConsole,
            WindowKind::SpectrumEq => AppCommand::ToggleSpectrumEq,
            WindowKind::Browser => AppCommand::DetachBrowser,
            WindowKind::PianoKeys => AppCommand::TogglePianoKeys,
            WindowKind::Video => AppCommand::ToggleVideo,
            WindowKind::Fretboard => AppCommand::DetachFretboard,
            WindowKind::Main | WindowKind::Rack => AppCommand::DetachRack,
        };
        if *kind != WindowKind::Main && !shell.detached(*kind) {
            shell.execute(command);
        }
    }
    if cli.play {
        shell.execute(AppCommand::Transport(TransportAction::Play));
    }
    let status = shell.status_text().to_string();
    log::info!("{status}");
    let event_loop = EventLoop::new().map_err(|e| format!("event loop: {e}"))?;
    // Live MIDI input wakes the loop at once: the idle loop sleeps between its slow polls, and
    // a mapped control or a transport command must not wait for the next one.
    let midi_wake = event_loop.create_proxy();
    shell.core().control_hub().set_waker(Arc::new(move || {
        // A closed loop has nothing left to wake.
        let _ = midi_wake.send_event(());
    }));
    let unattended = cli.unattended();
    let config_dir = config.dir.clone();
    crate::text_clipboard::install();
    let now = clock::now();
    let screenshot_at = cli
        .screenshot_after
        .filter(|s| s.is_finite() && *s >= 0.0)
        .map(|s| now + Duration::from_secs_f64(s));
    // A song opened next to the first: a new song with its own plugin host, running without audio
    // until it is focused. Its recovery copies live in a folder of its own and its plugins keep
    // no crash journal (the first song's journal names what a crash lost).
    let song_factory: crate::workspace::SongFactory<SystemBackend> =
        Box::new(move |from: &Shell<SystemBackend>, id: DocId| {
            let mut audio = from.audio_settings();
            audio.surfaces.clear();
            let session = LiveSession::new(SystemBackend, audio.clone(), false);
            let mut core = AppCore::new(session);
            install_video(&mut core);
            if let Some(path) = song_settings_path
                .as_deref()
                .map(crate::recent::recent_path)
            {
                core.set_recent_path(path);
            }
            core.configure_plugins(plugin_setup::song_plugin_config(
                audio.plugins.clone(),
                Some(song_config_dir.as_path()),
                id.0,
                song_runner.clone(),
                song_isolation.clone(),
            ));
            let mut song = Shell::new(
                core,
                ShellOptions {
                    os: None,
                    profile: None,
                    in_window_menu: !cfg!(target_os = "macos"),
                    ui_state_path: None,
                    library_path: song_settings_path
                        .as_deref()
                        .map(crate::ui_state::library_path),
                    library_root: song_library_root.clone(),
                    settings_path: song_settings_path.clone(),
                },
            );
            song.set_library_default(crate::settings::default_library_dir(
                crate::settings::TargetOs::current(),
                &|k| std::env::var_os(k),
            ));
            song.set_factory_folder(factory_folder.clone());
            song.core_mut().set_plugin_load_paints_first(true);
            song.core_mut().set_analyse_only_shown(true);
            song.core_mut().set_background_save(true);
            song.start_with_default_song();
            song
        });
    let mut shell = Workspace::new(shell, song_factory);
    // Songs that were open next to the first one when the last run was cut off come back in
    // windows of their own (the first song's own banner offers its copy).
    let crashed =
        crate::recovery::crashed_song_snapshots(&config.dir.join(crate::recovery::RECOVERY_FOLDER));
    shell.recover_songs(&crashed);
    let mut app = App {
        ime_on: false,
        shell,
        gpu: GpuContext::new(),
        windows: BTreeMap::new(),
        custom_cursors: BTreeMap::new(),
        ids: BTreeMap::new(),
        zorder: BTreeMap::new(),
        last_focused: (DocId::FIRST, WindowKind::Main),
        drag_noted: false,
        foreign_hover: None,
        text: TextEngine::new(),
        modifiers: Modifiers::NONE,
        mod_events: translate::ModifierTracker::default(),
        buttons: translate::PointerMap::default(),
        os: Os::host(),
        clicks: ClickCounter::new(),
        epoch: now,
        next_status: now,
        frame_watch: crate::ui_stall::FrameWatch::default(),
        activity: crate::ui_activity::Activity::new(),
        last_paint: None,
        frame: FRAME,
        frame_started: None,
        held_until: None,
        held_buttons: window_logic::HeldButtons::default(),
        poll_paint: false,
        event_cause: None,
        retry_cause: None,
        screenshot: cli.screenshot.clone(),
        screenshot_at,
        quit_at: cli
            .quit_after
            .filter(|s| s.is_finite() && *s >= 0.0)
            .map(|s| now + Duration::from_secs_f64(s)),
        stopped: false,
        plugin_windows: PluginWindows::default(),
        learn_strips: LearnStrips::default(),
        dock_frames: DockFrames::default(),
        recovery: Recovery::default(),
        forced_loss: gpu_recovery::forced_period(
            std::env::var(gpu_recovery::FORCE_LOSS_ENV).ok().as_deref(),
        )
        .map(|period| ForcedLoss::new(period, Duration::ZERO)),
        graphics_ok: false,
        rebuilding: false,
        ui_guard: UiGuard::default(),
        forced_panic: ForcedPanic::parse(std::env::var(ui_guard::FORCE_PANIC_ENV).ok().as_deref()),
        start_failure: None,
        #[cfg(target_os = "macos")]
        native: None,
    };
    event_loop
        .run_app(&mut app)
        .map_err(|e| format!("event loop: {e}"))?;
    let failure = app.start_failure.take();
    app.shutdown();
    if let Some(failure) = failure {
        let log_path =
            crate::logging::RotatingFile::path_of(&crate::logging::log_dir(&config_dir), 0);
        let report = gpu_select::start_report(
            &failure.found,
            &failure.reason,
            &log_path.display().to_string(),
            unattended,
        );
        // The one place a message box appears for graphics: the start, and only when someone is
        // at the screen (a scripted run logs and exits).
        if let Some((title, text)) = &report.dialog {
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title(title)
                .set_description(text)
                .set_buttons(rfd::MessageButtons::Ok)
                .show();
        }
        return Err(report.log);
    }
    // A scripted run that contained a panic must not look like a success.
    if let Some(why) = ui_guard::exit_failure(unattended, app.ui_guard.total()) {
        return Err(why);
    }
    Ok(())
}

/// The size the main window opens with when nothing was saved (logical units).
const MAIN_SIZE: (f64, f64) = (1280.0, 860.0);
/// The size a detached window opens with when the shell has none to say.
const DETACHED_SIZE: (f64, f64) = (1280.0, 720.0);
/// The smallest a window can be (logical units).
const MIN_SIZE: (f64, f64) = (480.0, 320.0);
/// Said once per drag when the OS gives no positions to find the window under the pointer.
const WAYLAND_DRAG_NOTE: &str = "Drag between windows is not available on Wayland";

/// One window of the app: the OS window, its surface and everything it paints and remembers.
struct AppWindow {
    /// Dropped before `window`: the surface keeps its own handle to the window.
    surface: Option<WindowSurface>,
    window: Arc<Window>,
    scene: Scene,
    cache: SceneCache,
    /// The pointer in this window's logical units.
    pointer: Point,
    /// The OS file drag over this window.
    file_drag: FileDrag,
    /// The cursor last set on the window, and the bitmap side it was made for.
    cursor: (CursorHint, u32),
    title: String,
    /// The application zoom the window was last told (it is told again when it changes).
    zoom: f64,
    /// When this window was last painted (`None` before the first), on the clock of the frame
    /// slots. The frame cap reads it; it is not the app-wide [`App::last_paint`], which only
    /// drives the paint tail.
    last_painted: Option<Instant>,
}

struct App {
    /// The input method is on for the windows (a text field has the keys).
    ime_on: bool,
    shell: Workspace<SystemBackend>,
    gpu: GpuContext,
    /// The open windows: the main window of every song and the panes it detached.
    windows: BTreeMap<Key, AppWindow>,
    /// The drawn tool cursors made so far, by picture and bitmap side.
    custom_cursors: BTreeMap<(CursorArt, u32), CustomCursor>,
    ids: BTreeMap<WindowId, Key>,
    /// The windows of each song by focus, to find the one under the pointer during a drag.
    zorder: BTreeMap<DocId, ZOrder>,
    /// The window that last had the focus: the parent of the dialogs.
    last_focused: Key,
    /// The note about drags between windows was said for this drag.
    drag_noted: bool,
    /// The song whose window the files of a drag out of another song's file browser hover over.
    foreign_hover: Option<DocId>,
    text: TextEngine,
    modifiers: Modifiers,
    /// Emits `UiEvent::ModifiersChanged` once per real modifier change.
    mod_events: translate::ModifierTracker,
    /// Maps buttons (Ctrl+click is a right click on macOS).
    buttons: translate::PointerMap,
    /// The OS whose keys apply.
    os: Os,
    clicks: ClickCounter,
    epoch: Instant,
    next_status: Instant,
    /// Decides which slow frames are written to the log.
    frame_watch: crate::ui_stall::FrameWatch,
    /// The wake-up and paint counters (see [`crate::ui_activity`]).
    activity: crate::ui_activity::Activity,
    /// When a window was last painted: the loop stays at the full frame rate for a while after
    /// it (see [`window_logic::PAINT_TAIL`]).
    last_paint: Option<Instant>,
    /// The length of the frame slots of the windows that present without waiting for the
    /// vertical blank (see [`window_logic::paint_gate`]): [`FRAME`] now, one place to change.
    frame: Duration,
    /// When the current frame slot began: the windows that present without waiting share it.
    frame_started: Option<Instant>,
    /// When the earliest paint that the frame cap held back in this tick is due; the loop wakes
    /// then even when nothing else would.
    held_until: Option<Instant>,
    /// The pointer buttons held down now: a drag or a hold needs frames even when the pointer
    /// does not move.
    held_buttons: window_logic::HeldButtons,
    /// The paints of this tick were asked for by the status poll alone.
    poll_paint: bool,
    /// What the last window event says about the paints that follow it (an input, a resize, a
    /// focus change); cleared at the end of the tick that painted them.
    event_cause: Option<PaintReason>,
    /// A cause for the paint after the next one: a frame that was not shown is painted again.
    retry_cause: Option<PaintReason>,
    screenshot: Option<PathBuf>,
    /// When set, the screenshot waits until then (else it is taken at the first frame).
    screenshot_at: Option<Instant>,
    quit_at: Option<Instant>,
    stopped: bool,
    /// The plugin editor windows (separate top-level windows).
    plugin_windows: PluginWindows,
    /// The Learn strips docked to the plugin editor windows.
    learn_strips: LearnStrips,
    /// The plugin editors that sit inside the main window.
    dock_frames: DockFrames,
    /// The loss of the graphics device and the way back (see [`super::gpu_recovery`]).
    recovery: Recovery,
    /// The test switch that takes the device away every so often (`ATOMIK_GPU_LOSS_EVERY`).
    forced_loss: Option<ForcedLoss>,
    /// A surface was made at least once: from then on a graphics failure is a loss to recover
    /// from, not a start that cannot work.
    graphics_ok: bool,
    /// A rebuild of the graphics is under way: a panic that finds it set ended the attempt half
    /// way.
    rebuilding: bool,
    /// The panics of the UI thread that were contained (see [`crate::ui_guard`]).
    ui_guard: UiGuard,
    /// The test switch that makes a dispatch point panic (`ATOMIK_UI_PANIC`).
    forced_panic: Option<ForcedPanic>,
    /// Why the first surface could not be made (no acceptable adapter, no device): the start
    /// ends with a message and a non-zero exit code.
    start_failure: Option<StartFailure>,
    #[cfg(target_os = "macos")]
    native: Option<super::native_menu::NativeMenus>,
}

/// A start that found no usable graphics: the adapters seen and the error.
struct StartFailure {
    found: Vec<AdapterDesc>,
    reason: String,
}

/// The monitors as [`MonitorInfo`] and the index of the primary one.
/// Opens `url` in the default browser. The address is one of the project's own constants
/// (`ShellRequest::OpenUrl`), never typed by the user or read from a file; a failure is logged.
/// Gives a core the real FFmpeg programs and the per-user folder of video proxies (the core
/// itself starts without any, so no test or headless run starts a program).
fn install_video(core: &mut AppCore<LiveSession<SystemBackend>>) {
    core.set_video_tool(Arc::new(atomik_video::SystemTool));
    if let Some(dir) = settings::default_video_cache_dir() {
        core.set_video_cache(dir);
    }
}

fn open_url(url: &str) {
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("explorer.exe").arg(url).spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(url).spawn();
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(url).spawn();
    if let Err(e) = result {
        log::warn!("could not open {url}: {e}");
    }
}

/// Shows `path` in the file manager, selecting the file where the system can (Windows and
/// macOS; elsewhere the folder that holds it opens). Only an existing path is passed on.
fn reveal(path: &std::path::Path) {
    if !path.exists() {
        log::warn!("cannot show {} in its folder: it is gone", path.display());
        return;
    }
    #[cfg(target_os = "windows")]
    let result = {
        let mut arg = std::ffi::OsString::from("/select,");
        arg.push(path);
        std::process::Command::new("explorer.exe").arg(arg).spawn()
    };
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open")
        .arg("-R")
        .arg(path)
        .spawn();
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let result = std::process::Command::new("xdg-open")
        .arg(path.parent().unwrap_or(path))
        .spawn();
    if let Err(e) = result {
        log::warn!("could not show {} in its folder: {e}", path.display());
    }
}

/// Shows a folder in the file manager.
fn open_folder(dir: &std::path::Path) {
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("explorer.exe").arg(dir).spawn();
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(dir).spawn();
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let result = std::process::Command::new("xdg-open").arg(dir).spawn();
    if let Err(e) = result {
        log::warn!("could not open {}: {e}", dir.display());
    }
}

fn monitors(event_loop: &ActiveEventLoop) -> (Vec<MonitorInfo>, usize) {
    let handles: Vec<_> = event_loop.available_monitors().collect();
    let infos = handles
        .iter()
        .map(|m| {
            let (p, s) = (m.position(), m.size());
            window_logic::monitor_info(
                m.name(),
                (p.x, p.y),
                (s.width, s.height),
                m.scale_factor(),
                window_logic::desktop_space(),
            )
        })
        .collect();
    let primary = event_loop
        .primary_monitor()
        .and_then(|p| handles.iter().position(|m| *m == p))
        .unwrap_or(0);
    (infos, primary)
}

impl App {
    fn shutdown(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        // The geometry the shell saves is the last one the windows had.
        for key in self.windows.keys().copied().collect::<Vec<_>>() {
            self.report_geom(key);
        }
        // Every song: its state is written, then MIDI, stream and engine are dropped here on the
        // UI thread, then its plugins (editors closed, the crash journal removed: a clean exit).
        self.shell.shutdown_all();
    }

    /// Asks where to save the project (or the patch, with `export`); the chosen path, with the
    /// extension added when the name has none, or `None` when the dialog is cancelled.
    fn pick_save_path(&self, export: bool) -> Option<PathBuf> {
        let (ext, filter, title) = if export {
            ("json", "Atomik Rack patch", "Export patch")
        } else {
            ("atomik", "Atomik Rack project", "Save project")
        };
        let stem = self
            .shell
            .core()
            .file()
            .and_then(|p| p.file_stem())
            .map_or_else(|| "project".to_string(), |n| n.to_string_lossy().into());
        let mut dialog = rfd::FileDialog::new()
            .set_title(title)
            .add_filter(filter, &[ext])
            .set_file_name(format!("{stem}.{ext}"));
        if let Some(w) = self.parent_window() {
            dialog = dialog.set_parent(w);
        }
        let mut path = dialog.save_file()?;
        if path.extension().is_none() {
            path.set_extension(ext);
        }
        Some(path)
    }

    /// The window the dialogs belong to: the one that last had the focus.
    fn parent_window(&self) -> Option<&Window> {
        self.windows
            .get(&self.last_focused)
            .or_else(|| self.windows.get(&(self.shell.routed(), WindowKind::Main)))
            .or_else(|| self.windows.values().next())
            .map(|w| w.window.as_ref())
    }

    /// Opens and closes the OS windows to match what the shell wants (call after every call
    /// into the shell). A window that cannot be opened is attached again.
    fn sync_windows(&mut self, event_loop: &ActiveEventLoop) {
        if self.stopped || self.start_failure.is_some() {
            return;
        }
        let routed = self.shell.routed();
        let songs = self.shell.ids();
        // The windows of a song that is closed go with it.
        for key in self.windows.keys().copied().collect::<Vec<_>>() {
            if !songs.contains(&key.0) {
                self.close_window(key);
            }
        }
        for id in songs {
            if !self.shell.select(id) {
                continue;
            }
            let open: Vec<_> = self
                .windows
                .keys()
                .filter(|k| k.0 == id)
                .map(|k| k.1)
                .collect();
            let plan = windows::reconcile(&open, &self.shell.windows_wanted());
            for kind in plan.close {
                self.close_window((id, kind));
            }
            for kind in plan.create {
                if let Err(e) = self.create_window(event_loop, (id, kind)) {
                    log::error!("cannot create the {kind:?} window of song {}: {e}", id.0);
                    if self.start_failure.is_some() {
                        // No graphics at the start: nothing can be shown, and the run ends
                        // with a message (see `run`).
                        event_loop.exit();
                        return;
                    }
                    if kind != WindowKind::Main {
                        self.shell.window_closed(kind);
                    } else if !self.shell.close_song(id) {
                        // The only song has no window: nothing is left to show.
                        event_loop.exit();
                    }
                    break;
                }
            }
        }
        self.shell.select(routed);
        self.pace_presentation();
    }

    /// Whether the surfaces wait for the vertical blank now: the one rule that sets the present
    /// mode and decides whether the frame cap applies.
    fn presents_with_vsync(&self) -> bool {
        window_logic::presents_with_vsync(self.windows.len())
    }

    /// Lets the surfaces wait for the vertical blank only while one window is open.
    fn pace_presentation(&mut self) {
        let vsync = self.presents_with_vsync();
        for w in self.windows.values_mut() {
            if let Some(surface) = &mut w.surface {
                self.gpu.set_vsync(surface, vsync);
            }
        }
    }

    /// Opens the window of `kind`: placed from its saved geometry and the monitors of the
    /// moment, told its scale and size, with a surface.
    fn create_window(&mut self, event_loop: &ActiveEventLoop, key: Key) -> Result<(), String> {
        let (doc, kind) = key;
        self.shell.select(doc);
        let space = window_logic::desktop_space();
        let (monitors, primary) = monitors(event_loop);
        let default = match kind {
            WindowKind::Main => MAIN_SIZE,
            _ => self
                .shell
                .window_size(kind)
                .map_or(DETACHED_SIZE, |s| (s.width, s.height)),
        };
        let saved = self.shell.saved_geom(kind);
        // A window with the menu bar is never narrower than the bar.
        let min_size = (MIN_SIZE.0.max(self.shell.menu_bar_width(kind)), MIN_SIZE.1);
        let placement = if kind == WindowKind::Main {
            windows::place(saved.as_ref(), default, min_size, &monitors, primary)
        } else {
            // A detached window with nothing saved goes to a monitor the app is not on yet.
            let occupied = self.desktop_rects(doc).unwrap_or_default();
            windows::place_detached(
                saved.as_ref(),
                default,
                min_size,
                &monitors,
                primary,
                &occupied,
            )
        };
        let mut attributes = Window::default_attributes()
            .with_title(self.shell.title_in(kind))
            .with_inner_size(LogicalSize::new(placement.size.0, placement.size.1))
            .with_min_inner_size(LogicalSize::new(min_size.0, min_size.1))
            .with_maximized(placement.maximised)
            .with_window_icon(window_logic::window_icon());
        // The desktop shell matches a window to its launcher (name and icon) by this id: the id of
        // the desktop entry of the Linux packages (packaging/linux).
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            use winit::platform::wayland::WindowAttributesExtWayland;
            use winit::platform::x11::WindowAttributesExtX11;
            const APP_ID: &str = "io.github.atomikfactory.AtomikRack";
            attributes = WindowAttributesExtWayland::with_name(attributes, APP_ID, APP_ID);
            attributes = WindowAttributesExtX11::with_name(attributes, APP_ID, APP_ID);
        }
        if let Some(pos) = placement.pos {
            attributes = attributes.with_position(window_logic::position_for(space, pos));
        }
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .map_err(|e| e.to_string())?,
        );
        crate::startup::mark(crate::startup::FIRST_WINDOW);
        // The windows are told the display scale times the application zoom: everything the
        // shell does in logical units follows.
        let scale = self.shell.ui_scale(window.scale_factor());
        let size = window.inner_size();
        // While the graphics are down the window opens without a surface; the rebuild gives it one.
        let surface = if self.recovery.is_down() || self.gpu.is_lost() {
            None
        } else {
            match self
                .gpu
                .create_surface(Arc::clone(&window), size.width, size.height)
            {
                Ok(surface) => {
                    self.graphics_ok = true;
                    Some(surface)
                }
                Err(reason) => {
                    if !self.graphics_ok && self.start_failure.is_none() {
                        self.start_failure = Some(StartFailure {
                            found: self.gpu.found().to_vec(),
                            reason: reason.clone(),
                        });
                    }
                    return Err(reason);
                }
            }
        };
        if let Some(theme) = window.theme() {
            self.shell
                .set_system_dark(theme == winit::window::Theme::Dark);
        }
        self.shell.handle_in(kind, &UiEvent::ScaleChanged(scale));
        self.shell
            .handle_in(kind, &UiEvent::Resize(translate::logical_size(size, scale)));
        if kind == WindowKind::Main {
            // The owner of the editors that live in a plugin's own process (sandbox).
            let owner = super::plugin_windows::owner_of(&window);
            // A window of another process can be a child of ours on Windows and X11 only.
            self.shell.core_mut().set_plugin_docking_available(matches!(
                owner,
                Some(
                    atomik_plugin_host::NativeParent::Win32(_)
                        | atomik_plugin_host::NativeParent::X11(_)
                )
            ));
            self.shell.core_mut().set_plugin_editor_owner(owner);
        }
        self.ids.insert(window.id(), key);
        self.zorder.entry(doc).or_default().focus(kind);
        let zoom = self.shell.app_zoom();
        self.windows.insert(
            key,
            AppWindow {
                surface,
                window: Arc::clone(&window),
                scene: Scene::new(),
                cache: SceneCache::new(),
                pointer: Point::ZERO,
                file_drag: FileDrag::new(),
                cursor: (CursorHint::Default, 0),
                title: self.shell.title_in(kind),
                zoom,
                last_painted: None,
            },
        );
        self.report_geom(key);
        window.request_redraw();
        Ok(())
    }

    /// Closes the window of `kind` (the shell attached its pane).
    fn close_window(&mut self, key: Key) {
        self.windows.remove(&key);
        self.ids.retain(|_, k| *k != key);
        if let Some(order) = self.zorder.get_mut(&key.0) {
            order.remove(key.1);
        }
        if self.last_focused == key {
            self.last_focused = (self.shell.play_focus(), WindowKind::Main);
        }
    }

    /// Tells the shell where the window of `kind` is (it saves it).
    fn report_geom(&mut self, key: Key) {
        let Some(w) = self.windows.get(&key) else {
            return;
        };
        let kind = key.1;
        let window = &w.window;
        if window.is_minimized() == Some(true) {
            return;
        }
        let size = window.inner_size();
        let outer = window.outer_position().ok().map(|p| (p.x, p.y));
        let geom = window_logic::geom_of(
            window_logic::desktop_space(),
            outer,
            (size.width, size.height),
            window.scale_factor(),
            window.is_maximized(),
            window.current_monitor().and_then(|m| m.name()),
        );
        if let Some(geom) = geom
            && self.shell.select(key.0)
        {
            self.shell.window_moved(kind, geom);
        }
    }

    /// The rect on the desktop of every window of song `doc`; `None` when the OS gives no
    /// positions (Wayland).
    fn desktop_rects(&self, doc: DocId) -> Option<Vec<WindowRect>> {
        self.windows
            .iter()
            .filter(|(key, _)| key.0 == doc)
            .map(|((_, kind), w)| {
                let origin = w.window.inner_position().ok()?;
                let size = w.window.inner_size();
                Some(window_logic::window_rect(
                    *kind,
                    (origin.x, origin.y),
                    (size.width, size.height),
                    w.window.scale_factor(),
                    window_logic::desktop_space(),
                ))
            })
            .collect()
    }

    /// The window under a pointer that is at `pos` in `kind`'s window, during a drag that
    /// can cross windows: the front window of the app that holds the point, or `None`.
    fn window_under_pointer(&mut self, key: Key, pos: Point) -> Option<WindowHit> {
        let (doc, kind) = key;
        self.shell.select(doc);
        let rects = self.desktop_rects(doc);
        let Some(rects) = rects.filter(|r| r.iter().any(|r| r.kind == kind)) else {
            // No positions: the pointer is over its own window or over nothing of the app.
            let size = self.shell.window_size(kind)?;
            let hit = window_logic::local_hit(kind, pos, (size.width, size.height));
            if hit.is_none()
                && self.windows.keys().filter(|k| k.0 == doc).count() > 1
                && !self.drag_noted
            {
                self.drag_noted = true;
                self.shell.notify(WAYLAND_DRAG_NOTE);
            }
            return hit;
        };
        let own = rects.iter().find(|r| r.kind == kind)?;
        let point = windows::to_desktop(
            own.space,
            (own.x, own.y),
            own.scale,
            (
                pos.x * own.scale * self.shell.app_zoom(),
                pos.y * own.scale * self.shell.app_zoom(),
            ),
        );
        windows::window_under(&rects, point, self.zorder.entry(doc).or_default())
    }

    /// The window of another song under a pointer that is at `pos` in `kind`'s window, during a
    /// drag: the song and the point in its window. The song that last had the focus is looked
    /// at first, then the others in the order they were opened; `None` when the OS gives no
    /// positions (Wayland) or no other song has a window there.
    fn other_song_under(&mut self, key: Key, pos: Point) -> Option<(DocId, WindowHit)> {
        let (doc, kind) = key;
        let rects = self.desktop_rects(doc)?;
        let own = rects.iter().find(|r| r.kind == kind)?;
        let zoom = self.shell.song(doc).map_or(1.0, |s| s.app_zoom());
        let point = windows::to_desktop(
            own.space,
            (own.x, own.y),
            own.scale,
            (pos.x * own.scale * zoom, pos.y * own.scale * zoom),
        );
        let mut docs: Vec<DocId> = self.windows.keys().map(|k| k.0).collect();
        docs.dedup();
        docs.retain(|d| *d != doc);
        let focused = self.last_focused.0;
        docs.sort_by_key(|d| (*d != focused, *d));
        for other in docs {
            let Some(rects) = self.desktop_rects(other) else {
                continue;
            };
            let Some(mut hit) =
                windows::window_under(&rects, point, self.zorder.entry(other).or_default())
            else {
                continue;
            };
            // The windows of a song take their points in its own zoom.
            let zoom = self.shell.song(other).map_or(1.0, |s| s.app_zoom());
            hit.logical = (hit.logical.0 / zoom, hit.logical.1 / zoom);
            return Some((other, hit));
        }
        None
    }

    /// Hands one event of `kind`'s window to the shell. During a drag of files the window under
    /// the pointer is worked out here, since the OS keeps sending the events to the window
    /// that got the press. A drag of the selection (clips, devices, a track) and a drag of files
    /// can end over a window of another song: the selection is copied there, the files are
    /// dropped there.
    fn dispatch(&mut self, key: Key, event: &UiEvent) -> Vec<ShellRequest> {
        self.shell.select(key.0);
        let kind = key.1;
        if let UiEvent::PointerUp { pos, .. } = event
            && self.shell.song_drag().is_some()
            && self.window_under_pointer(key, *pos).is_none()
            && let Some((to, hit)) = self.other_song_under(key, *pos)
            && self.shell.drag_to_song(key.0, to, hit)
        {
            return Vec::new();
        }
        let Some(pos) = window_logic::pointer_of(event).filter(|_| self.shell.cross_window_drag())
        else {
            self.drag_noted = false;
            if let Some(left) = self.foreign_hover.take() {
                self.shell.files_left_song(left);
            }
            return self.shell.handle_in(kind, event);
        };
        let under = self.window_under_pointer(key, pos);
        let over = match under {
            Some(_) => None,
            None => self.other_song_under(key, pos),
        };
        // The song the files hover over changed: the one they left lets go of its hover.
        let now = over.as_ref().map(|(to, _)| *to);
        if let Some(left) = self.foreign_hover.filter(|s| Some(*s) != now) {
            self.shell.files_left_song(left);
        }
        self.foreign_hover = None;
        if let Some((to, hit)) = over {
            let release = matches!(event, UiEvent::PointerUp { .. });
            if self.shell.files_to_song(key.0, to, hit, release) {
                self.foreign_hover = (!release).then_some(to);
                // This song sees the pointer over no window of its own: its hover ends and a
                // release ends the gesture.
                return self.shell.handle_routed(kind, event, None);
            }
        }
        self.shell.handle_routed(kind, event, under)
    }

    /// Docks a Learn strip to every visible plugin editor window whose frame is known: the
    /// embedded ones (our windows) and the ones the plugins' own processes show.
    fn sync_learn_strips(&mut self, event_loop: &ActiveEventLoop) {
        let ids = self.shell.ids();
        self.learn_strips.keep_songs(&ids);
        self.dock_frames.keep_songs(&ids);
        for id in ids {
            let Some(song) = self.shell.song(id) else {
                continue;
            };
            let theme = song.theme();
            let scale = self
                .windows
                .get(&(id, WindowKind::Main))
                .map_or(1.0, |w| w.window.scale_factor());
            let core = song.core();
            let owner = match core.plugin_editor_owner() {
                Some(atomik_plugin_host::NativeParent::Win32(h)) => Some(h as isize),
                _ => None,
            };
            let mut docks = self.plugin_windows.docks(id);
            docks.extend(
                core.external_editor_frames()
                    .into_iter()
                    .map(|(device, frame)| {
                        (
                            device,
                            Dock {
                                frame,
                                scale,
                                owner,
                            },
                        )
                    }),
            );
            self.learn_strips.sync(
                event_loop,
                &mut self.gpu,
                &mut self.text,
                &theme,
                id,
                core,
                &docks,
            );
        }
    }

    /// Keeps the frames of the plugin editors that sit inside each song's main window.
    fn sync_dock_frames(&mut self, event_loop: &ActiveEventLoop) {
        for id in self.shell.ids() {
            let Some(main) = self.windows.get(&(id, WindowKind::Main)) else {
                continue;
            };
            let scale = main.window.scale_factor();
            let window = Arc::clone(&main.window);
            let Some(song) = self.shell.song_mut(id) else {
                continue;
            };
            let theme = song.theme();
            self.dock_frames.sync(
                event_loop,
                &mut self.gpu,
                &mut self.text,
                &theme,
                id,
                &window,
                scale,
                song.core_mut(),
            );
        }
    }

    /// Paints `kind`'s window into its scene.
    fn paint_scene(&mut self, key: Key) {
        let kind = key.1;
        if !self.shell.select(key.0) {
            return;
        }
        let Some(w) = self.windows.get_mut(&key) else {
            return;
        };
        w.scene.reset();
        let mut canvas = SceneCanvas::new(&mut w.scene, &mut self.text).with_cache(&mut w.cache);
        self.shell.paint_in(kind, &mut canvas);
        w.cache.trim();
    }

    /// A whole tick or redraw that began at `began` is over: a slow one is logged (see
    /// [`crate::ui_stall`]). Only this live, windowed loop calls it, so bounces, the QA server
    /// and the tests never log frame stalls.
    fn note_frame_time(&mut self, began: Instant) {
        let now = clock::now();
        if let Some(line) = self
            .frame_watch
            .note(now.saturating_duration_since(began), now)
        {
            log::warn!("{line}");
        }
    }

    /// The reason the window `kind` of song `doc` is painted now: the person's input, a window
    /// event, a change something marked, or a view that keeps itself painting.
    fn paint_reason(&self, doc: DocId, kind: WindowKind) -> PaintReason {
        if let Some(cause) = self.event_cause {
            return cause;
        }
        let marked = self
            .shell
            .song(doc)
            .is_some_and(|s| s.repaint_marked_in(kind));
        if marked {
            PaintReason::Update
        } else {
            PaintReason::Live
        }
    }

    /// Counts the seconds that ended (publishing them for the diagnostics report) and writes the
    /// debug line of the counters now and then.
    fn advance_activity(&mut self) {
        let at = clock::now().saturating_duration_since(self.epoch);
        if self.activity.advance(at) {
            crate::ui_activity::publish(self.activity.snapshot());
            if let Some(line) = self.activity.debug_line() {
                log::debug!("{line}");
            }
        }
    }

    /// Whether the window `key` is painted now for `reason` or waits (see
    /// [`window_logic::paint_gate`]). A redraw the system asks for while nothing is marked is an
    /// exposure and is never held back. `now` is the one time of the tick (or of the redraw
    /// request), the same for every window painted in it.
    fn frame_gate(&self, key: Key, reason: PaintReason, now: Instant) -> window_logic::PaintGate {
        let marked = self
            .shell
            .song(key.0)
            .is_some_and(|s| s.needs_repaint_in(key.1));
        let Some(w) = self.windows.get(&key) else {
            return window_logic::PaintGate::Now { opens_slot: false };
        };
        if !marked {
            return window_logic::PaintGate::Now { opens_slot: false };
        }
        window_logic::paint_gate(
            self.presents_with_vsync(),
            self.frame_started
                .map(|started| now.saturating_duration_since(started)),
            matches!((w.last_painted, self.frame_started), (Some(l), Some(s)) if l >= s),
            self.frame,
            reason,
        )
    }

    fn redraw(&mut self, key: Key, reason: PaintReason, now: Instant) {
        let kind = key.1;
        self.service_graphics();
        if self.recovery.is_down() {
            return; // no surface until the graphics are back; the rebuild repaints everything
        }
        let Some(window) = self.windows.get(&key).map(|w| Arc::clone(&w.window)) else {
            return;
        };
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return; // minimized
        }
        self.advance_activity();
        let opens_slot = match self.frame_gate(key, reason, now) {
            window_logic::PaintGate::Now { opens_slot } => opens_slot,
            window_logic::PaintGate::Wait(wait) => {
                // Held back: nothing is painted and the shell's marks stay, so the window is
                // asked for again; the loop wakes when the slot is over.
                self.activity.deferred(kind);
                self.held_until = window_logic::sooner(self.held_until, now + wait);
                return;
            }
        };
        if opens_slot {
            self.frame_started = Some(now);
        }
        if let Some(w) = self.windows.get_mut(&key) {
            w.last_painted = Some(now);
        }
        self.activity.paint(kind, reason);
        if window_logic::paint_extends_tail(reason, self.poll_paint) {
            self.last_paint = Some(clock::now());
        }
        self.paint_scene(key);
        let Some(w) = self.windows.get_mut(&key) else {
            return;
        };
        let result = match &mut w.surface {
            Some(surface) => {
                self.gpu.resize(surface, size.width, size.height);
                self.gpu.render(surface, &w.scene, Color::BLACK)
            }
            // A window that was minimized while the graphics were rebuilt has no surface yet.
            None => {
                match self
                    .gpu
                    .create_surface(Arc::clone(&window), size.width, size.height)
                {
                    Ok(surface) => {
                        w.surface = Some(surface);
                        self.shell.repaint_window(kind);
                        window.request_redraw();
                    }
                    Err(e) => self.gpu.health().mark_lost(&e),
                }
                return;
            }
        };
        match result {
            FrameResult::Presented => crate::startup::mark(crate::startup::FIRST_FRAME),
            // Painted but not shown: the shell has marked the window clean, so ask again.
            FrameResult::Skipped => {
                self.retry_cause = Some(PaintReason::Surface);
                self.shell.repaint_window(kind);
            }
            FrameResult::Reconfigured => {
                self.retry_cause = Some(PaintReason::Surface);
                window.request_redraw();
            }
            // The device (or the surface) is gone: the flag is set, and the next check at the
            // start of a frame tears the graphics down and builds them again.
            FrameResult::Lost => {
                self.retry_cause = Some(PaintReason::Surface);
                self.shell.repaint_window(kind);
                self.service_graphics();
            }
        }
    }

    /// Checked at the start of every frame: forces the test loss when the switch is on, says
    /// what the graphics driver reported, and tears the graphics down and builds them again when
    /// the device is lost. Audio, the document and the plugins are not touched.
    fn service_graphics(&mut self) {
        let at = clock::now().saturating_duration_since(self.epoch);
        if let Some(forced) = &mut self.forced_loss
            && forced.due(at)
            && !self.recovery.is_down()
            && !self.gpu.is_lost()
        {
            log::info!(
                "graphics device loss forced by {}",
                gpu_recovery::FORCE_LOSS_ENV
            );
            self.gpu.force_loss();
        }
        for note in self.gpu.health().take_notes() {
            log::warn!("graphics error (the device is still usable): {note}");
        }
        let lost = self.gpu.is_lost();
        if lost && !self.recovery.is_down() {
            let why = self.gpu.health().take_reason().unwrap_or_default();
            log::warn!("graphics device lost: {why}");
        }
        let decision = self.recovery.poll(at, lost);
        if decision.teardown {
            self.tear_down_graphics();
        }
        if decision.rebuild {
            self.rebuild_graphics(at);
        }
    }

    /// Lets go of every surface of every window (main, detached, docked frames, Learn strips)
    /// while the device they belong to is dead. The windows stay; they are black until the
    /// graphics are back.
    fn tear_down_graphics(&mut self) {
        for w in self.windows.values_mut() {
            w.surface = None;
        }
        self.dock_frames.drop_surfaces();
        self.learn_strips.drop_surfaces();
    }

    /// One attempt to build the graphics again: a new context (instance, adapter, device and
    /// renderer), a surface for every window, and a repaint of everything. A failure is logged
    /// and tried again after the back-off; the first one asks for a recovery snapshot so the
    /// work is safe even if graphics never come back.
    fn rebuild_graphics(&mut self, at: Duration) {
        self.rebuilding = true;
        self.tear_down_graphics();
        // The old context goes with its dead device before the new one is made.
        self.gpu = GpuContext::rebuilt();
        let attempt = self.recovery.attempts();
        match self.recreate_surfaces() {
            Ok(()) => {
                self.recovery.finished(at, true);
                let adapter = self.gpu.adapter_line().unwrap_or_default();
                log::info!("graphics restored (attempt {attempt}); {adapter}");
                self.repaint_everything();
            }
            Err(e) => {
                self.tear_down_graphics();
                let snapshot = self.recovery.finished(at, false);
                let wait = self
                    .recovery
                    .next_attempt()
                    .map_or(Duration::ZERO, |t| t.saturating_sub(at));
                log::warn!(
                    "graphics rebuild attempt {attempt} failed: {e}; next attempt in {:.1} s",
                    wait.as_secs_f64()
                );
                if snapshot {
                    for id in self.shell.ids() {
                        if let Some(song) = self.shell.song_mut(id) {
                            song.core_mut().request_recovery_snapshot();
                        }
                    }
                }
            }
        }
        self.rebuilding = false;
    }

    /// Gives every window, docked frame and Learn strip a surface on the new context.
    fn recreate_surfaces(&mut self) -> Result<(), String> {
        for w in self.windows.values_mut() {
            let size = w.window.inner_size();
            if size.width == 0 || size.height == 0 {
                continue; // minimized: the surface is made when the window is drawn again
            }
            w.surface = Some(self.gpu.create_surface(
                Arc::clone(&w.window),
                size.width,
                size.height,
            )?);
        }
        self.dock_frames
            .recreate_surfaces(&mut self.gpu, &mut self.text)?;
        self.learn_strips
            .recreate_surfaces(&mut self.gpu, &mut self.text)
    }

    /// Marks every window of every song to be painted and asks the system for a redraw.
    fn repaint_everything(&mut self) {
        self.pace_presentation();
        for doc in self.shell.ids() {
            if !self.shell.select(doc) {
                continue;
            }
            self.shell.core_mut().view_mut().request_repaint();
            for ((d, kind), w) in &self.windows {
                if *d == doc {
                    self.shell.repaint_window(*kind);
                    w.window.request_redraw();
                }
            }
        }
        self.shell.select(self.shell.play_focus());
    }

    /// Renders every open window offscreen: `base` for the main window, `base-rack` and
    /// `base-console` (before the extension) for the detached ones.
    fn take_screenshots(&mut self, base: &std::path::Path) {
        for key in self.windows.keys().copied().collect::<Vec<_>>() {
            let kind = key.1;
            if !self.shell.select(key.0) {
                continue;
            }
            let Some((w, h)) = self.shell.physical_size_in(kind) else {
                continue;
            };
            self.paint_scene(key);
            // The scene was painted, but the window has not shown it yet.
            self.shell.core_mut().view_mut().request_repaint();
            let path = window_logic::song_screenshot_path(base, key.0, kind);
            let Some(window) = self.windows.get(&key) else {
                continue;
            };
            match render_offscreen(&window.scene, Color::BLACK, w, h)
                .and_then(|pixels| write_png(&path, &pixels, w, h))
            {
                Ok(()) => log::info!("screenshot {w}x{h} written to {}", path.display()),
                Err(e) => log::error!("screenshot failed: {e}"),
            }
        }
    }

    /// Sets each window's title when it changed.
    fn update_titles(&mut self) {
        for ((doc, kind), w) in &mut self.windows {
            let Some(song) = self.shell.song(*doc) else {
                continue;
            };
            if let Some(title) = window_logic::changed(&w.title, &kind.title(song.title())) {
                w.window.set_title(&title);
                w.title = title;
            }
        }
    }

    /// Shows the cursor the shell asks for in each window (set only when it changes). The tool
    /// cursors are drawn bitmaps, made once per size and kept.
    fn update_cursors(&mut self, event_loop: &ActiveEventLoop) {
        for ((doc, kind), w) in &mut self.windows {
            let Some(song) = self.shell.song(*doc) else {
                continue;
            };
            let size = cursor_art::size_for(w.window.scale_factor());
            let want = (song.cursor_in(*kind), size);
            let Some((hint, size)) = window_logic::changed(&w.cursor, &want) else {
                continue;
            };
            let custom = translate::cursor_art(hint).and_then(|art| {
                if let Some(c) = self.custom_cursors.get(&(art, size)) {
                    return Some(c.clone());
                }
                let b = cursor_art::render(art, size);
                let source = CustomCursor::from_rgba(
                    b.rgba,
                    b.size as u16,
                    b.size as u16,
                    b.hotspot.0 as u16,
                    b.hotspot.1 as u16,
                )
                .ok()?;
                let c = event_loop.create_custom_cursor(source);
                self.custom_cursors.insert((art, size), c.clone());
                Some(c)
            });
            match custom {
                Some(c) => w.window.set_cursor(c),
                None => w.window.set_cursor(translate::cursor_icon(hint)),
            }
            w.cursor = (hint, size);
        }
    }

    /// Whether any window of any song has something to paint.
    fn anything_to_paint(&self) -> bool {
        self.shell.ids().into_iter().any(|id| {
            self.shell
                .song(id)
                .is_some_and(|s| !s.windows_to_paint().is_empty())
        })
    }

    /// Tells every song which of its windows are minimized (no area, or the system says so): the
    /// analysis for the panels in a minimized window stops.
    fn report_minimized(&mut self) {
        for (key, w) in &self.windows {
            let size = w.window.inner_size();
            let minimized = window_logic::is_minimized((size.width, size.height))
                || w.window.is_minimized() == Some(true);
            if let Some(song) = self.shell.song_mut(key.0) {
                song.set_window_minimized(key.1, minimized);
            }
        }
    }

    /// Whether every window of the app has no area (or the system says it is minimized) and
    /// no plugin window needs the frames: nothing is painted and the loop wakes slowly.
    fn all_minimized(&self) -> bool {
        let sizes = self.windows.values().map(|w| {
            let size = w.window.inner_size();
            window_logic::is_minimized((size.width, size.height))
                || w.window.is_minimized() == Some(true)
        });
        let others = self.plugin_windows.any() || self.learn_strips.any() || self.dock_frames.any();
        window_logic::all_minimized(sizes, others)
    }

    /// Paints and presents every window that has something to paint.
    fn paint_due_windows(&mut self, now: Instant) {
        for id in self.shell.ids() {
            let due = self.shell.song(id).map(|s| s.windows_to_paint());
            // The reasons are read before the first paint clears the marks.
            let due: Vec<_> = due
                .unwrap_or_default()
                .into_iter()
                .map(|kind| (kind, self.paint_reason(id, kind)))
                .collect();
            for (kind, reason) in due {
                self.redraw((id, kind), reason, now);
            }
        }
        // The cause of this tick's paints is spent; a frame that was not shown keeps its own, and
        // so does a paint that was held back (it is counted by what asked for it).
        let held = self.held_until.is_some();
        let retry = self.retry_cause.take();
        self.event_cause = if held {
            retry.or(self.event_cause)
        } else {
            retry
        };
        self.poll_paint = false;
    }

    /// Asks for a redraw of every window that has something to paint.
    fn request_redraws(&self) {
        for ((doc, kind), w) in &self.windows {
            if self
                .shell
                .song(*doc)
                .is_some_and(|s| s.needs_repaint_in(*kind))
            {
                w.window.request_redraw();
            }
        }
    }

    /// Tells every window its new scale and size when the application zoom changed.
    fn apply_zoom(&mut self) {
        for ((doc, kind), w) in &mut self.windows {
            let Some(zoom) = self.shell.song(*doc).map(|s| s.app_zoom()) else {
                continue;
            };
            if (zoom - w.zoom).abs() < f64::EPSILON {
                continue;
            }
            w.zoom = zoom;
            self.shell.select(*doc);
            let scale = self.shell.ui_scale(w.window.scale_factor());
            let size = w.window.inner_size();
            self.shell.handle_in(*kind, &UiEvent::ScaleChanged(scale));
            self.shell.handle_in(
                *kind,
                &UiEvent::Resize(translate::logical_size(size, scale)),
            );
        }
    }

    fn after_event(&mut self, event_loop: &ActiveEventLoop, requests: Vec<ShellRequest>) {
        self.shell.sync_preferences();
        self.apply_zoom();
        self.sync_windows(event_loop);
        self.handle_requests(event_loop, requests);
        self.sync_windows(event_loop);
        self.update_titles();
        self.update_cursors(event_loop);
        self.update_ime();
        #[cfg(target_os = "macos")]
        if let Some(native) = &mut self.native {
            let routed = self.shell.routed();
            self.shell.select(self.shell.play_focus());
            native.update(&self.shell.menu_model());
            self.shell.select(routed);
        }
        self.request_redraws();
    }

    /// Switches the input method on while a text field has the keys and off otherwise, so the
    /// IME window only appears where text is typed.
    fn update_ime(&mut self) {
        let want = self
            .shell
            .ids()
            .into_iter()
            .any(|id| self.shell.song(id).is_some_and(|s| s.wants_text_input()));
        if want == self.ime_on {
            return;
        }
        self.ime_on = want;
        for w in self.windows.values() {
            w.window.set_ime_allowed(want);
        }
    }

    /// Brings the window of `kind` forward and gives it the keyboard.
    fn focus_window(&self, key: Key) {
        if let Some(w) = self.windows.get(&key) {
            if w.window.is_minimized() == Some(true) {
                w.window.set_minimized(false);
            }
            w.window.focus_window();
        }
    }

    fn handle_requests(&mut self, event_loop: &ActiveEventLoop, requests: Vec<ShellRequest>) {
        for request in requests {
            match request {
                ShellRequest::PickOpenFile => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Open project or patch")
                        .add_filter(
                            "Atomik Rack project, share or patch",
                            &["atomik", "atomikz", "json"],
                        )
                        .add_filter("Atomik Rack project", &["atomik"])
                        .add_filter("Atomik Rack shared project", &["atomikz"])
                        .add_filter("Atomik Rack patch", &["json"]);
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(path) = dialog.pick_file() {
                        self.shell.open_file(&path);
                    }
                }
                ShellRequest::PickImportFiles => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Import audio")
                        .add_filter("Audio files", &crate::audio_import::AUDIO_EXTENSIONS);
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(paths) = dialog.pick_files() {
                        self.shell.import_files(paths);
                    }
                }
                ShellRequest::PickImportMidi { as_song } => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title(if as_song {
                            "Import MIDI as song"
                        } else {
                            "Import MIDI"
                        })
                        .add_filter("MIDI files", &crate::midi_file::MIDI_EXTENSIONS);
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(path) = dialog.pick_file() {
                        self.shell.import_midi(&path, as_song);
                    }
                }
                ShellRequest::PickImportTab { as_song } => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title(if as_song {
                            "Import tab or score as song"
                        } else {
                            "Import tab or score"
                        })
                        .add_filter(
                            "Guitar Pro and MusicXML files",
                            &["gp3", "gp4", "gp5", "gpx", "gp", "musicxml", "xml", "mxl"],
                        );
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(path) = dialog.pick_file() {
                        self.shell.import_tab(&path, as_song);
                    }
                }
                ShellRequest::PickImportSheet => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Import chord sheet")
                        .add_filter(
                            "Chord sheets",
                            &["txt", "crd", "chopro", "chordpro", "cho", "pro"],
                        );
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(path) = dialog.pick_file() {
                        self.shell.import_sheet(&path);
                    }
                }
                ShellRequest::PickExportTab(format) => {
                    let (title, filter, exts): (&str, &str, &[&str]) = match format {
                        crate::app_core::TabExportFormat::MusicXml => (
                            "Export tab as MusicXML",
                            "MusicXML file",
                            &["musicxml", "mxl"],
                        ),
                        crate::app_core::TabExportFormat::Text => {
                            ("Export tab as text", "Text file", &["txt"])
                        }
                        crate::app_core::TabExportFormat::Pdf => {
                            ("Export tab as PDF", "PDF file", &["pdf"])
                        }
                    };
                    let mut dialog = rfd::FileDialog::new()
                        .set_title(title)
                        .add_filter(filter, exts)
                        .set_file_name(format!("{}.{}", self.shell.tab_export_name(), exts[0]));
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(mut path) = dialog.save_file() {
                        if path.extension().is_none() {
                            path.set_extension(exts[0]);
                        }
                        self.shell.export_tab_to(format, &path);
                    }
                }
                ShellRequest::PickExportSheet => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Export chord sheet as text")
                        .add_filter("Text file", &["txt"])
                        .set_file_name("chord sheet.txt");
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(mut path) = dialog.save_file() {
                        if path.extension().is_none() {
                            path.set_extension("txt");
                        }
                        self.shell.export_sheet_to(&path);
                    }
                }
                ShellRequest::PickExportSheetPdf => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Export chord sheet as PDF")
                        .add_filter("PDF file", &["pdf"])
                        .set_file_name("chord sheet.pdf");
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(mut path) = dialog.save_file() {
                        if path.extension().is_none() {
                            path.set_extension("pdf");
                        }
                        self.shell.export_sheet_pdf_to(&path);
                    }
                }
                ShellRequest::PickExportMidi(what) => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title(format!("Export MIDI {}", what.word()))
                        .add_filter("MIDI file", &["mid"])
                        .set_file_name(format!("{}.mid", self.shell.midi_export_name(what)));
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(mut path) = dialog.save_file() {
                        if path.extension().is_none() {
                            path.set_extension("mid");
                        }
                        self.shell.export_midi_to(what, &path);
                    }
                }
                ShellRequest::PickExportAudio(atomik_ui::ExportScope::Stems) => {
                    // The folder the stems folder is made in.
                    let mut dialog = rfd::FileDialog::new().set_title("Export stems to a folder");
                    if let Some(dir) = self.shell.export_audio_dir() {
                        dialog = dialog.set_directory(dir);
                    }
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(parent) = dialog.pick_folder() {
                        let name = self.shell.export_audio_name(atomik_ui::ExportScope::Stems);
                        self.shell
                            .export_audio_to(atomik_ui::ExportScope::Stems, &parent.join(name));
                    }
                }
                ShellRequest::PickExportAudio(scope) => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title(match scope {
                            atomik_ui::ExportScope::Loop => "Export loop as audio file",
                            atomik_ui::ExportScope::Selection => "Export selection as audio file",
                            _ => "Export song as audio file",
                        })
                        .add_filter(
                            "Audio file",
                            &["wav", "aif", "aiff", "flac", "mp3", "m4a", "opus"],
                        )
                        .set_file_name(self.shell.export_audio_name(scope));
                    if let Some(dir) = self.shell.export_audio_dir() {
                        dialog = dialog.set_directory(dir);
                    }
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(mut path) = dialog.save_file() {
                        if path.extension().is_none() {
                            path.set_extension(self.shell.export_audio_extension());
                        }
                        self.shell.export_audio_to(scope, &path);
                    }
                }
                ShellRequest::PickExportFilm => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Export film with mix")
                        .add_filter("Film", &["mp4", "mov"])
                        .set_file_name(self.shell.export_film_name());
                    if let Some(dir) = self.shell.export_audio_dir() {
                        dialog = dialog.set_directory(dir);
                    }
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(mut path) = dialog.save_file() {
                        if path.extension().is_none() {
                            path.set_extension("mp4");
                        }
                        self.shell.export_film_to(&path);
                    }
                }
                ShellRequest::PickTonalFiles => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Learn a tonal balance target from audio files")
                        .add_filter("Audio files", &crate::audio_import::AUDIO_EXTENSIONS);
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(paths) = dialog.pick_files() {
                        self.shell.learn_tonal_files(paths);
                    }
                }
                ShellRequest::PickLoadSamples => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Load samples")
                        .add_filter("Audio files", &crate::audio_import::AUDIO_EXTENSIONS)
                        .add_filter("Instruments", &["sfz", "sf2"]);
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(paths) = dialog.pick_files() {
                        self.shell.load_samples(paths);
                    }
                }
                ShellRequest::PickSaveFile | ShellRequest::PickExportFile => {
                    let export = request == ShellRequest::PickExportFile;
                    if let Some(path) = self.pick_save_path(export) {
                        if export {
                            self.shell.export_to(&path);
                        } else {
                            self.shell.save_to(&path);
                        }
                    }
                }
                ShellRequest::PickSaveFileThen { then } => {
                    // A cancelled dialog ends the pending action: nothing is lost, nothing happens.
                    if let Some(path) = self.pick_save_path(false) {
                        let more = self.shell.save_to_then(&path, then);
                        self.handle_requests(event_loop, more);
                    }
                }
                ShellRequest::PickSavePatch { dir, name } => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Save device patch")
                        .add_filter("Atomik patch", &[atomik_library::PATCH_EXT])
                        .set_file_name(name);
                    if dir.is_dir() {
                        dialog = dialog.set_directory(dir);
                    }
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(mut path) = dialog.save_file() {
                        if path.extension().is_none() {
                            path.set_extension(atomik_library::PATCH_EXT);
                        }
                        self.shell.save_patch_to(&path);
                    }
                }
                ShellRequest::PickSaveMasterChain { dir, name } => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Save master chain")
                        .add_filter("Atomik patch", &[atomik_library::PATCH_EXT])
                        .set_file_name(name);
                    if dir.is_dir() {
                        dialog = dialog.set_directory(dir);
                    }
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(mut path) = dialog.save_file() {
                        if path.extension().is_none() {
                            path.set_extension(atomik_library::PATCH_EXT);
                        }
                        self.shell.save_master_chain_to(&path);
                    }
                }
                ShellRequest::PickLoadMasterChain { dir } => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Load master chain")
                        .add_filter("Atomik patch", &[atomik_library::PATCH_EXT]);
                    if dir.is_dir() {
                        dialog = dialog.set_directory(dir);
                    }
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(path) = dialog.pick_file() {
                        self.shell.load_master_chain_from(&path);
                    }
                }
                ShellRequest::PickExportKeymap => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Export keyboard shortcuts")
                        .add_filter("Atomik keymap", &["json"])
                        .set_file_name("keymap.json");
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(mut path) = dialog.save_file() {
                        if path.extension().is_none() {
                            path.set_extension("json");
                        }
                        self.shell.export_keymap_to(&path);
                    }
                }
                ShellRequest::PickImportHeadphone { device } => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Import headphone data")
                        .add_filter(
                            "Frequency response or parametric EQ",
                            &["csv", "txt", "tsv", "dat", "frd", "eq"],
                        )
                        .add_filter("All files", &["*"]);
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(path) = dialog.pick_file() {
                        self.shell.import_headphone_from(&device, &path);
                    }
                }
                ShellRequest::PickImportKeymap => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Import keyboard shortcuts")
                        .add_filter("Atomik keymap", &["json"]);
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(path) = dialog.pick_file() {
                        self.shell.import_keymap_from(&path);
                    }
                }
                ShellRequest::PickExportTheme(name) => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Export theme")
                        .add_filter("Atomik theme", &["json"])
                        .set_file_name(name);
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(mut path) = dialog.save_file() {
                        if path.extension().is_none() {
                            path.set_extension("json");
                        }
                        self.shell.export_theme_to(&path);
                    }
                }
                ShellRequest::PickImportTheme => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Import theme")
                        .add_filter("Atomik theme", &["json"]);
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(path) = dialog.pick_file() {
                        self.shell.import_theme_from(&path);
                    }
                }
                ShellRequest::OpenFolder(dir) => open_folder(&dir),
                ShellRequest::PickExportController => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Export controller profile")
                        .add_filter("Controller profile", &["json"])
                        .set_file_name("controller.json");
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(mut path) = dialog.save_file() {
                        if path.extension().is_none() {
                            path.set_extension("json");
                        }
                        self.shell.export_controller_to(&path);
                    }
                }
                ShellRequest::PickImportController => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Import controller profile")
                        .add_filter("Controller profile", &["json"]);
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(path) = dialog.pick_file() {
                        self.shell.import_controller_from(&path);
                    }
                }
                ShellRequest::PickGrooveMidi => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Make a groove from a MIDI file")
                        .add_filter("MIDI files", &["mid", "midi"]);
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(path) = dialog.pick_file() {
                        self.shell.groove_from_midi_file(&path);
                    }
                }
                ShellRequest::PickReference => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Add a reference track")
                        .add_filter("Audio files", &crate::audio_import::AUDIO_EXTENSIONS);
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(path) = dialog.pick_file() {
                        self.shell.add_reference_file(&path);
                    }
                }
                ShellRequest::PickSoundFile => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Analyse a sound")
                        .add_filter("Audio files", &crate::audio_import::AUDIO_EXTENSIONS);
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(path) = dialog.pick_file() {
                        self.shell.analyse_sound_file(&path);
                    }
                }
                ShellRequest::PickRelinkFile => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Locate the audio file")
                        .add_filter("Audio files", &crate::audio_import::AUDIO_EXTENSIONS);
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(path) = dialog.pick_file() {
                        self.shell.relink_file(path);
                    }
                }
                ShellRequest::PickExportDiagnostics => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Export diagnostics")
                        .add_filter("Text report", &["txt"])
                        .set_file_name(crate::diagnostics::default_file_name());
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(mut path) = dialog.save_file() {
                        if path.extension().is_none() {
                            path.set_extension("txt");
                        }
                        self.shell.export_diagnostics_to(&path);
                    }
                }
                ShellRequest::PickExportDeviceInfo => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Export device parameter list")
                        .add_filter("Text table", &["txt"])
                        .set_file_name(crate::device_info::default_file_name());
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(mut path) = dialog.save_file() {
                        if path.extension().is_none() {
                            path.set_extension("txt");
                        }
                        self.shell.export_device_info_to(&path);
                    }
                }
                ShellRequest::PickExportMidiPreset => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Export MIDI mapping preset")
                        .add_filter("MIDI mapping preset", &["json"])
                        .set_file_name("midi-mappings.json");
                    if let Some(dir) = self.shell.midi_preset_dir() {
                        dialog = dialog.set_directory(dir);
                    }
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(mut path) = dialog.save_file() {
                        if path.extension().is_none() {
                            path.set_extension("json");
                        }
                        self.shell.export_midi_preset_to(&path);
                    }
                }
                ShellRequest::PickExportMidiDevicePreset => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Export MIDI mappings of the selected device")
                        .add_filter("MIDI mapping preset", &["json"])
                        .set_file_name("device-mappings.json");
                    if let Some(dir) = self.shell.midi_preset_dir() {
                        dialog = dialog.set_directory(dir);
                    }
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(mut path) = dialog.save_file() {
                        if path.extension().is_none() {
                            path.set_extension("json");
                        }
                        self.shell.export_midi_device_preset_to(&path);
                    }
                }
                ShellRequest::PickImportMidiPreset => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Import MIDI mapping preset")
                        .add_filter("MIDI mapping preset", &["json"]);
                    if let Some(dir) = self.shell.midi_preset_dir() {
                        dialog = dialog.set_directory(dir);
                    }
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(path) = dialog.pick_file() {
                        self.shell.import_midi_preset_from(&path);
                    }
                }
                ShellRequest::PickRelinkFolder => {
                    let mut dialog =
                        rfd::FileDialog::new().set_title("Search a folder for the missing audio");
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(folder) = dialog.pick_folder() {
                        self.shell.relink_search_folder(folder);
                    }
                }
                ShellRequest::PickRelinkProjectFolder => {
                    let mut dialog = rfd::FileDialog::new().set_title(
                        "Choose the folder of the project (the one with its Audio folder)",
                    );
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(folder) = dialog.pick_folder() {
                        self.shell.relink_project_folder(folder);
                    }
                }
                ShellRequest::PickMoveFolder => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Move the project into a folder (it gets a folder of its own)");
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(folder) = dialog.pick_folder() {
                        self.shell.move_project_to(&folder);
                    }
                }
                ShellRequest::PickExportArchive { name } => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Export a project archive")
                        .add_filter("Atomik project archive", &["atomikz"])
                        .set_file_name(name);
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(path) = dialog.save_file() {
                        self.shell.export_archive_to(&path);
                    }
                }
                ShellRequest::PickShareFile { name } => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Share the project as one file")
                        .add_filter("Atomik shared project", &["atomikz"])
                        .set_file_name(name);
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(path) = dialog.save_file() {
                        self.shell.share_to(&path);
                    }
                }
                ShellRequest::PickShareFolder { archive, start } => {
                    let mut dialog =
                        rfd::FileDialog::new().set_title("Unpack the shared project into a folder");
                    if let Some(start) = start {
                        let _ = std::fs::create_dir_all(&start);
                        dialog = dialog.set_directory(start);
                    }
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(folder) = dialog.pick_folder() {
                        self.shell.share_open_to(&archive, &folder);
                    }
                }
                ShellRequest::PickImportArchive => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Import a project archive")
                        .add_filter("Atomik project archive", &["atomikz"]);
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(path) = dialog.pick_file() {
                        self.shell.import_archive_from(&path);
                    }
                }
                ShellRequest::PickFfmpegFolder => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Choose the folder that holds ffmpeg and ffprobe");
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(folder) = dialog.pick_folder() {
                        self.shell.set_ffmpeg_folder(folder);
                    }
                }
                ShellRequest::PickBackupFolder => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Choose the folder that keeps the backup copies");
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(folder) = dialog.pick_folder() {
                        self.shell.set_backup_folder(folder);
                    }
                }
                ShellRequest::PickVideoFile(pick) => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title(match pick {
                            VideoPick::Import => "Import a video",
                            VideoPick::Relink => "Find the video",
                            VideoPick::Prepare => "Prepare a video",
                        })
                        .add_filter(
                            "Video",
                            &[
                                "mp4", "m4v", "mov", "mkv", "webm", "avi", "mxf", "mpg", "mpeg",
                                "ts", "m2ts", "wmv", "ogv",
                            ],
                        )
                        .add_filter("All files", &["*"]);
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(path) = dialog.pick_file() {
                        self.shell.video_picked(pick, &path);
                    }
                }
                ShellRequest::PickScratchFolder => {
                    let mut dialog = rfd::FileDialog::new().set_title("Choose the scratch folder");
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(folder) = dialog.pick_folder() {
                        self.shell.set_scratch_folder(folder);
                    }
                }
                ShellRequest::ShowFolder(folder) => open_url(&folder.to_string_lossy()),
                ShellRequest::Reveal(path) => reveal(&path),
                ShellRequest::PickPluginFolder => {
                    let mut dialog = rfd::FileDialog::new().set_title("Add a plugin folder");
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(folder) = dialog.pick_folder() {
                        self.shell.add_plugin_folder(folder);
                    }
                }
                ShellRequest::PickLibraryFolder => {
                    let mut dialog = rfd::FileDialog::new().set_title("Choose the library folder");
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(folder) = dialog.pick_folder() {
                        self.shell.set_library_folder(folder);
                    }
                }
                ShellRequest::ConfirmSave { then } => {
                    // Save saves, Don't Save discards, Cancel leaves everything as it is. The
                    // buttons carry these words on every platform (a task dialog on Windows).
                    let (save, discard, cancel) = window_logic::SAVE_BUTTONS;
                    let mut dialog = rfd::MessageDialog::new()
                        .set_level(rfd::MessageLevel::Warning)
                        .set_title("Unsaved changes")
                        .set_description("The rack has unsaved changes. Save them first?")
                        .set_buttons(rfd::MessageButtons::YesNoCancelCustom(
                            save.into(),
                            discard.into(),
                            cancel.into(),
                        ));
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    let more = match window_logic::save_answer(&dialog.show()) {
                        window_logic::SaveAnswer::Save => self.shell.save_then(then),
                        window_logic::SaveAnswer::Discard => self.shell.discard_then(then),
                        window_logic::SaveAnswer::Cancel => Vec::new(),
                    };
                    self.handle_requests(event_loop, more);
                }
                ShellRequest::FocusWindow(kind) => self.focus_window((self.shell.routed(), kind)),
                ShellRequest::FocusSong(id) => self.focus_window((id, WindowKind::Main)),
                ShellRequest::PickOpenSong => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Open project or patch in a new window")
                        .add_filter("Atomik Rack project or patch", &["atomik", "json"])
                        .add_filter("Atomik Rack project", &["atomik"])
                        .add_filter("Atomik Rack patch", &["json"]);
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(path) = dialog.pick_file()
                        && let Some(id) = self.shell.open_song(&path)
                    {
                        // The window of the new song opens next, and the song has the focus.
                        self.sync_windows(event_loop);
                        self.focus_window((id, WindowKind::Main));
                    }
                }
                // The workspace answers these before they get here.
                ShellRequest::NewSong | ShellRequest::CloseSong | ShellRequest::ShowSong(_) => {}
                ShellRequest::OpenUrl(url) => open_url(url),
                ShellRequest::OpenRelease(url) => open_url(&url),
                ShellRequest::ToggleMaximise(kind) => {
                    if let Some(w) = self.windows.get(&(self.shell.routed(), kind)) {
                        w.window.set_maximized(!w.window.is_maximized());
                    }
                }
                ShellRequest::Quit => {
                    self.shutdown();
                    event_loop.exit();
                }
            }
        }
    }

    /// Hands the file drag events collected for `kind`'s window to the shell (as one event, see
    /// [`FileDrag`]), at that window's pointer.
    fn flush_files(&mut self, event_loop: &ActiveEventLoop, key: Key) {
        let scale_zoom = self.shell.app_zoom();
        let Some(w) = self.windows.get_mut(&key) else {
            return;
        };
        // An operating-system drag sends no cursor events on Windows: the position of a hover
        // and of the drop is the cursor as the system has it, not the last one the window saw.
        if w.file_drag.is_pending()
            && let Some(at) = cursor::os_cursor(&w.window, w.window.scale_factor() * scale_zoom)
        {
            w.pointer = at;
        }
        if let Some(event) = w.file_drag.flush(w.pointer) {
            self.ui(event_loop, key, event);
        }
    }

    /// While files hover over a window and the system sends no cursor events for the drag, reads
    /// the cursor and asks for the hover again when it moved, so the drop indicator follows it.
    /// True while such a drag is on (the loop keeps waking to poll).
    fn follow_file_drags(&mut self) -> bool {
        let zoom = self.shell.app_zoom();
        let mut following = false;
        for w in self.windows.values_mut() {
            if !w.file_drag.is_hovering() {
                continue;
            }
            let Some(at) = cursor::os_cursor(&w.window, w.window.scale_factor() * zoom) else {
                continue;
            };
            following = true;
            if at != w.pointer {
                w.pointer = at;
                w.file_drag.rehover();
            }
        }
        following
    }

    fn flush_all_files(&mut self, event_loop: &ActiveEventLoop) {
        for key in self.windows.keys().copied().collect::<Vec<_>>() {
            self.flush_files(event_loop, key);
        }
    }

    /// Runs one dispatch point of the loop with a panic contained (see [`crate::ui_guard`]): the
    /// panic hook has written the crash report by the time it is caught here.
    fn guarded(
        &mut self,
        event_loop: &ActiveEventLoop,
        point: Dispatch,
        f: impl FnOnce(&mut Self, &ActiveEventLoop),
    ) {
        let outcome = ui_guard::run(|| {
            if self.forced_panic.as_mut().is_some_and(|f| f.take(point)) {
                panic!("UI panic forced by {}", ui_guard::FORCE_PANIC_ENV);
            }
            f(self, event_loop);
        });
        match outcome {
            Ok(()) => self.ui_guard.completed(point),
            Err(message) => self.after_ui_panic(event_loop, point, &message),
        }
    }

    /// A panic of `point` was caught: the state it may have left half done is repaired, a
    /// recovery copy is asked for, the notice is raised (once, see [`UiGuard`]) and the app
    /// goes on. Audio, the document and the plugins are not touched.
    fn after_ui_panic(&mut self, event_loop: &ActiveEventLoop, point: Dispatch, message: &str) {
        let at = clock::now().saturating_duration_since(self.epoch);
        let verdict = self.ui_guard.caught(point, at);
        if verdict.notice {
            log::error!(
                "internal error in the interface ({} #{}): {message}; a recovery copy was asked \
                 for and the app keeps running{}",
                point.name(),
                self.ui_guard.total(),
                if verdict.halt {
                    "; repeated errors paused the content of the window"
                } else {
                    ""
                }
            );
        } else {
            log::debug!(
                "internal error in the interface again ({}): {message}",
                point.name()
            );
        }
        // A graphics rebuild that ended half way counts as a failed attempt: the device is
        // treated as lost and the back-off decides when to try again.
        if std::mem::take(&mut self.rebuilding) {
            let _ = ui_guard::run(|| {
                self.tear_down_graphics();
                self.recovery.finished(at, false);
            });
        }
        // What the event or the frame was in the middle of is gone with it.
        self.event_cause = None;
        self.retry_cause = None;
        self.modifiers = Modifiers::NONE;
        self.mod_events.reset();
        self.buttons = translate::PointerMap::default();
        self.held_buttons.reset();
        let halted = self.ui_guard.is_halted();
        let _ = ui_guard::run(|| {
            self.shell.after_ui_panic(verdict.notice, halted);
            self.shell.select(self.shell.play_focus());
        });
        let _ = ui_guard::run(|| self.repaint_everything());
        if point == Dispatch::Frame {
            // The tick did not get to say when it wants to run again.
            let wake = clock::now() + ui_guard::RETRY_AFTER_PANIC;
            event_loop.set_control_flow(ControlFlow::WaitUntil(wake));
        }
    }

    fn ui(&mut self, event_loop: &ActiveEventLoop, key: Key, event: UiEvent) {
        let requests = self.dispatch(key, &event);
        self.after_event(event_loop, requests);
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.guarded(event_loop, Dispatch::Event, Self::on_resumed);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if matches!(event, WindowEvent::RedrawRequested) {
            let began = clock::now();
            self.guarded(event_loop, Dispatch::Paint, |app, event_loop| {
                app.on_window_event(event_loop, id, event);
            });
            self.note_frame_time(began);
        } else {
            self.guarded(event_loop, Dispatch::Event, |app, event_loop| {
                app.on_window_event(event_loop, id, event);
            });
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let began = clock::now();
        self.guarded(event_loop, Dispatch::Frame, Self::on_tick);
        self.note_frame_time(began);
    }
}

impl App {
    fn on_resumed(&mut self, event_loop: &ActiveEventLoop) {
        if !self.windows.is_empty() {
            return;
        }
        // The main window and the panes that were detached when the app last closed.
        self.sync_windows(event_loop);
        if !self.windows.contains_key(&(DocId::FIRST, WindowKind::Main)) {
            return;
        }
        #[cfg(target_os = "macos")]
        {
            self.shell.select(DocId::FIRST);
            self.native = super::native_menu::NativeMenus::attach(&self.shell.menu_model());
        }
        self.update_titles();
        if self.screenshot_at.is_none()
            && let Some(path) = self.screenshot.take()
        {
            self.take_screenshots(&path);
        }
        self.request_redraws();
    }

    fn on_window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        self.shell.apply_language_globally();
        // The frame of a plugin editor inside the main window: its buttons and drags.
        if let Some(doc) = self.dock_frames.doc_of(id) {
            if let Some(song) = self.shell.song_mut(doc) {
                self.dock_frames
                    .event(&mut self.gpu, &mut self.text, id, event, song.core_mut());
            }
            return;
        }
        // A Learn strip of a plugin editor: its buttons go to the core.
        if let Some(doc) = self.learn_strips.doc_of(id) {
            if let Some(song) = self.shell.song_mut(doc) {
                self.learn_strips
                    .event(&mut self.gpu, &mut self.text, id, event, song.core_mut());
            }
            return;
        }
        // A plugin editor's own window: its resize and close go to the core.
        if let Some(doc) = self.plugin_windows.doc_of(id) {
            if let Some(song) = self.shell.song_mut(doc) {
                self.plugin_windows.event(id, event, song.core_mut());
            }
            return;
        }
        let Some(&key) = self.ids.get(&id) else {
            return;
        };
        let kind = key.1;
        match &event {
            WindowEvent::HoveredFile(path) => {
                if let Some(w) = self.windows.get_mut(&key) {
                    w.file_drag.hovered(path.clone());
                }
                return;
            }
            WindowEvent::HoveredFileCancelled => {
                if let Some(w) = self.windows.get_mut(&key) {
                    w.file_drag.hover_cancelled();
                }
                return;
            }
            WindowEvent::DroppedFile(path) => {
                if let Some(w) = self.windows.get_mut(&key) {
                    w.file_drag.dropped(path.clone());
                }
                return;
            }
            // Anything else ends the group: keep the order of events.
            _ => self.flush_files(event_loop, key),
        }
        self.advance_activity();
        if let Some(cause) = window_logic::paint_cause_of(&event) {
            self.event_cause = Some(cause);
        }
        self.shell.select(key.0);
        let zoom = self.shell.app_zoom();
        let Some((scale, pointer)) = self
            .windows
            .get(&key)
            .map(|w| (w.window.scale_factor() * zoom, w.pointer))
        else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => {
                if kind == WindowKind::Main {
                    // The window of one song closes that song (after its own question about
                    // unsaved changes); the last window closing is quitting.
                    let command = if self.shell.len() > 1 {
                        AppCommand::CloseSongWindow
                    } else {
                        AppCommand::Quit
                    };
                    let requests = self.shell.execute(command);
                    self.after_event(event_loop, requests);
                } else {
                    // Closing a detached window attaches its pane again.
                    self.shell.window_closed(kind);
                    self.after_event(event_loop, Vec::new());
                }
            }
            WindowEvent::Resized(size) => {
                // A window with no area is minimized: the graphics target is not reallocated and
                // nothing is laid out until it comes back with a real size.
                if window_logic::is_minimized((size.width, size.height)) {
                    return;
                }
                if let Some(w) = self.windows.get_mut(&key)
                    && let Some(surface) = &mut w.surface
                {
                    self.gpu.resize(surface, size.width, size.height);
                }
                self.ui(
                    event_loop,
                    key,
                    UiEvent::Resize(translate::logical_size(size, scale)),
                );
                self.report_geom(key);
            }
            WindowEvent::Moved(_) => self.report_geom(key),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let scale = self.shell.ui_scale(scale_factor);
                self.ui(event_loop, key, UiEvent::ScaleChanged(scale));
            }
            WindowEvent::ThemeChanged(theme) => {
                self.shell
                    .set_system_dark(theme == winit::window::Theme::Dark);
                self.after_event(event_loop, Vec::new());
            }
            WindowEvent::ModifiersChanged(m) => {
                self.modifiers = translate::modifiers(m.state());
                if let Some(event) = self.mod_events.update(m.state()) {
                    self.ui(event_loop, key, event);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let pointer = translate::logical(position, scale);
                if let Some(w) = self.windows.get_mut(&key) {
                    w.pointer = pointer;
                }
                let event = UiEvent::PointerMove {
                    pos: pointer,
                    modifiers: self.modifiers,
                };
                self.ui(event_loop, key, event);
            }
            WindowEvent::CursorLeft { .. } => self.ui(event_loop, key, UiEvent::PointerLeave),
            WindowEvent::MouseInput { state, button, .. } => {
                if state == ElementState::Pressed
                    && let Some(w) = self.windows.get(&key)
                {
                    // A docked plugin editor may hold the keyboard: a click here takes it back.
                    self.dock_frames.main_clicked(key.0, &w.window);
                }
                let mapped = match state {
                    ElementState::Pressed => self.buttons.press(button, self.modifiers, self.os),
                    ElementState::Released => self.buttons.release(button, self.modifiers),
                };
                match state {
                    ElementState::Pressed => self.held_buttons.press(),
                    ElementState::Released => self.held_buttons.release(),
                }
                let Some((button, modifiers)) = mapped else {
                    return;
                };
                let event = match state {
                    ElementState::Pressed => {
                        let at = clock::now().saturating_duration_since(self.epoch);
                        UiEvent::PointerDown {
                            pos: pointer,
                            button,
                            modifiers,
                            clicks: self.clicks.press(at, pointer, button),
                        }
                    }
                    ElementState::Released => UiEvent::PointerUp {
                        pos: pointer,
                        button,
                        modifiers,
                    },
                };
                self.ui(event_loop, key, event);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let event = UiEvent::Wheel {
                    delta: translate::wheel(delta, scale),
                    pos: pointer,
                    modifiers: self.modifiers,
                };
                self.ui(event_loop, key, event);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let pressed = translate::key_event(
                    &event.logical_key,
                    &event.physical_key,
                    self.modifiers,
                    self.os,
                );
                let event = UiEvent::Key {
                    key: pressed,
                    modifiers: self.modifiers,
                    pressed: event.state.is_pressed(),
                };
                self.ui(event_loop, key, event);
            }
            WindowEvent::Ime(ime) => {
                let event = translate::ime(&ime);
                self.ui(event_loop, key, event);
            }
            WindowEvent::RedrawRequested => {
                let reason = self.paint_reason(key.0, kind);
                self.redraw(key, reason, clock::now());
            }
            WindowEvent::Focused(false) => {
                self.held_buttons.reset();
                // The releases of the keys held now will never arrive.
                self.modifiers = Modifiers::NONE;
                self.mod_events.reset();
                self.shell.focus_lost();
            }
            WindowEvent::Focused(true) => {
                self.zorder.entry(key.0).or_default().focus(kind);
                self.last_focused = key;
                // The song this window belongs to takes the audio and the keys.
                self.shell.focus(key.0);
                self.shell.window_focused(kind);
                self.shell.core_mut().view_mut().request_repaint();
                self.after_event(event_loop, Vec::new());
                if let Some(w) = self.windows.get(&key) {
                    w.window.request_redraw();
                }
            }
            WindowEvent::Occluded(false) => {
                self.shell.core_mut().view_mut().request_repaint();
                if let Some(w) = self.windows.get(&key) {
                    w.window.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn on_tick(&mut self, event_loop: &ActiveEventLoop) {
        self.advance_activity();
        self.activity.wake();
        self.shell.apply_language_globally();
        let dragging = self.follow_file_drags();
        self.flush_all_files(event_loop);
        let now = clock::now();
        if self.quit_at.is_some_and(|t| now >= t) {
            self.shutdown();
            event_loop.exit();
            return;
        }
        self.service_graphics();
        #[cfg(target_os = "macos")]
        {
            let commands = self
                .native
                .as_ref()
                .map(super::native_menu::NativeMenus::poll)
                .unwrap_or_default();
            self.shell.select(self.shell.play_focus());
            for command in commands {
                let requests = self.shell.execute(command);
                self.after_event(event_loop, requests);
            }
        }
        // The plugin editors each song's core asked for, and the requests its shell raised
        // between events.
        for id in self.shell.ids() {
            if !self.shell.select(id) {
                continue;
            }
            self.plugin_windows
                .pump(event_loop, id, self.shell.core_mut());
            let requests = self.shell.take_requests();
            if !requests.is_empty() {
                self.handle_requests(event_loop, requests);
            }
        }
        self.sync_learn_strips(event_loop);
        self.sync_dock_frames(event_loop);
        self.shell.select(self.shell.play_focus());
        // Engine garbage is dropped here, on the UI thread, every tick; the transport
        // position and recorded notes are read too.
        // Meters and their ballistics use the monotonic time since start.
        self.shell
            .frame_all(now.saturating_duration_since(self.epoch).as_secs_f64());
        self.sync_windows(event_loop);
        if self.screenshot_at.is_some_and(|t| now >= t)
            && let Some(path) = self.screenshot.take()
        {
            self.screenshot_at = None;
            self.take_screenshots(&path);
        }
        // A paint that only the status poll asks for does not keep the loop at the full rate.
        let marked_before = self.anything_to_paint();
        if now >= self.next_status {
            self.next_status = now + STATUS_PERIOD;
            self.shell.poll_all();
        }
        self.poll_paint = !marked_before && self.anything_to_paint();
        // Only the windows with something new repaint: the meters redraw the console's window.
        // Every one of them is painted here, in turn. Leaving it to the OS's redraw requests
        // would let the one the OS serves first (the focused one, on Windows) take all the
        // time and starve the others while it animates.
        self.report_minimized();
        let minimized = self.all_minimized();
        // What the frame cap holds back is found again below (a window it did not paint stays
        // marked), so the deadline starts empty.
        self.held_until = None;
        if window_logic::paints_on_wake(minimized) {
            self.paint_due_windows(now);
        }
        // Audio imports and loads finish on worker threads: poll them even without a stream.
        let songs: Vec<_> = self
            .shell
            .ids()
            .into_iter()
            .filter_map(|id| self.shell.song(id))
            .collect();
        let seconds = now.saturating_duration_since(self.epoch).as_secs_f64();
        let wakers = window_logic::Wakers {
            animating: songs.iter().any(|s| s.wants_fast_frames(seconds))
                || self.held_buttons.any()
                || self.plugin_windows.any()
                || self.learn_strips.any()
                || self.dock_frames.any(),
            stream: songs.iter().any(|s| s.core().sink().is_running()),
            plugins: songs.iter().any(|s| s.core().plugin_instance_count() > 0),
            minimized,
            since_paint: self.last_paint.map(|t| now.saturating_duration_since(t)),
        };
        let deadline = window_logic::loop_deadline(
            now,
            window_logic::wake_after(wakers, self.frame),
            self.held_until,
        );
        // A submenu switch waits for its deadline: wake exactly then.
        let menu_wake = songs
            .iter()
            .filter_map(|s| s.context_menu_wake())
            .reduce(f64::min)
            .map(|t| self.epoch + Duration::from_secs_f64(t.max(0.0)));
        // A file drag the system sends no cursor events for is followed by polling the cursor.
        let drag_poll = dragging.then(|| now + FILE_DRAG_POLL);
        // A rebuild of the graphics that is waiting for its back-off, and the next forced loss.
        let graphics_wake = [
            self.recovery.next_attempt(),
            self.forced_loss.as_ref().map(ForcedLoss::next),
        ]
        .into_iter()
        .flatten()
        .min()
        .map(|t| self.epoch + t);
        let deadline = window_logic::earliest(
            deadline,
            [
                self.quit_at,
                self.screenshot_at,
                menu_wake,
                drag_poll,
                graphics_wake,
            ],
        );
        event_loop.set_control_flow(match deadline {
            Some(d) => ControlFlow::WaitUntil(d),
            None => ControlFlow::Wait,
        });
    }
}
