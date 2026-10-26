//! The winit 0.30 application: one window, event translation, frame pacing, dialogs.

use super::gpu::{FrameResult, Gpu, render_offscreen, write_png};
use super::plugin_windows::PluginWindows;
use crate::app_core::{AppCore, default_registry};
use crate::cli::Cli;
use crate::clicks::ClickCounter;
use crate::commands::AppCommand;
use crate::plugin_setup;
use crate::session::{LiveSession, SystemBackend};
use crate::settings::{self, AudioSettings, default_settings_path};
use crate::shell::{Shell, ShellOptions, ShellRequest};
use crate::stress::{
    STRESS_AUDIO_THRESHOLD_BYTES, add_stress_character, add_stress_containers, add_stress_drums,
    add_stress_fx, add_stress_players, add_stress_plugins, add_stress_samplers, add_stress_slicers,
    stress_audio_dir, stress_drums_dir, stress_sampler_dir, stress_slicer_dir, write_stress_audio,
};
use crate::translate::{self, FileDrag};
use crate::ui_state::ui_state_path;
use atomik_model::DeviceKey;
use atomik_ui::TransportAction;
use atomik_ui::keymap::Os;
use atomik_ui::kurbo::Point;
use atomik_ui::{CursorHint, Modifiers, SceneCache, SceneCanvas, TextEngine, UiEvent};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use vello::Scene;
use vello::peniko::Color;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

/// Redraw tick while audio runs (≈ 60 Hz, for meters and status).
const FRAME: Duration = Duration::from_micros(16_667);
/// Status bar refresh period.
const STATUS_PERIOD: Duration = Duration::from_millis(250);

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
    let settings_path = cli.settings.clone().or_else(default_settings_path);
    let (saved, settings_error) = match &settings_path {
        Some(path) => settings::load(path),
        None => (AudioSettings::default(), None),
    };
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
    let mut plugin_config = plugin_setup::plugin_config(
        plugin_settings,
        settings_path.as_deref().and_then(std::path::Path::parent),
        exe.as_deref().map(plugin_setup::scanner_command),
    );
    // Where plugins run is read once, here: the Settings switch applies at the next start.
    plugin_config.isolation = Some(plugin_setup::plugin_isolation(
        plugin_settings_sandbox,
        exe.as_deref(),
    ));
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
    core.configure_plugins(plugin_config);
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
    // Creating a plugin blocks this thread: show LOADING one frame before it starts.
    shell.core_mut().set_plugin_load_paints_first(true);
    if let Some(path) = cli.keymap.clone() {
        shell.use_keymap_file(path);
    }
    for message in [settings_error.map(|e| e.to_string()), open_error]
        .into_iter()
        .flatten()
    {
        eprintln!("atomik: {message}");
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
    if cli.play {
        shell.execute(AppCommand::Transport(TransportAction::Play));
    }
    let status = shell.status_text().to_string();
    eprintln!("atomik: {status}");
    let event_loop = EventLoop::new().map_err(|e| format!("event loop: {e}"))?;
    let now = clock::now();
    let screenshot_at = cli
        .screenshot_after
        .filter(|s| s.is_finite() && *s >= 0.0)
        .map(|s| now + Duration::from_secs_f64(s));
    let mut app = App {
        shell,
        window: None,
        gpu: None,
        scene: Scene::new(),
        text: TextEngine::new(),
        cache: SceneCache::new(),
        modifiers: Modifiers::NONE,
        mod_events: translate::ModifierTracker::default(),
        buttons: translate::PointerMap::default(),
        os: Os::host(),
        pointer: Point::ZERO,
        cursor: CursorHint::Default,
        clicks: ClickCounter::new(),
        drag: FileDrag::new(),
        epoch: now,
        next_status: now,
        screenshot: cli.screenshot.clone(),
        screenshot_at,
        quit_at: cli
            .quit_after
            .filter(|s| s.is_finite() && *s >= 0.0)
            .map(|s| now + Duration::from_secs_f64(s)),
        title: String::new(),
        stopped: false,
        plugin_windows: PluginWindows::default(),
        #[cfg(target_os = "macos")]
        native: None,
    };
    event_loop
        .run_app(&mut app)
        .map_err(|e| format!("event loop: {e}"))?;
    app.shutdown();
    Ok(())
}

struct App {
    shell: Shell<SystemBackend>,
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    scene: Scene,
    text: TextEngine,
    cache: SceneCache,
    modifiers: Modifiers,
    /// Emits `UiEvent::ModifiersChanged` once per real modifier change.
    mod_events: translate::ModifierTracker,
    /// Maps buttons (Ctrl+click is a right click on macOS).
    buttons: translate::PointerMap,
    /// The OS whose keys apply.
    os: Os,
    pointer: Point,
    /// The cursor last set on the window.
    cursor: CursorHint,
    clicks: ClickCounter,
    drag: FileDrag,
    epoch: Instant,
    next_status: Instant,
    screenshot: Option<PathBuf>,
    /// When set, the screenshot waits until then (else it is taken at the first frame).
    screenshot_at: Option<Instant>,
    quit_at: Option<Instant>,
    title: String,
    stopped: bool,
    /// The plugin editor windows (separate top-level windows).
    plugin_windows: PluginWindows,
    #[cfg(target_os = "macos")]
    native: Option<super::native_menu::NativeMenus>,
}

impl App {
    fn shutdown(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        self.shell.shutdown();
        self.shell.poll_status();
        eprintln!("atomik: at exit: {}", self.shell.status_text());
        // MIDI, stream, then the engine, all dropped here on the UI thread; then the plugins
        // (editors closed, the crash journal removed: a clean exit).
        self.shell.core_mut().sink_mut().shutdown();
        self.shell.core_mut().shutdown_plugins();
    }

    fn scale(&self) -> f64 {
        self.window.as_ref().map_or(1.0, |w| w.scale_factor())
    }

    fn paint_scene(&mut self) {
        self.scene.reset();
        let mut canvas =
            SceneCanvas::new(&mut self.scene, &mut self.text).with_cache(&mut self.cache);
        self.shell.paint(&mut canvas);
        self.cache.trim();
    }

    fn redraw(&mut self) {
        let Some(window) = self.window.clone() else {
            return;
        };
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return; // minimized
        }
        self.paint_scene();
        let result = match &mut self.gpu {
            Some(gpu) => {
                gpu.resize(size.width, size.height);
                gpu.render(&self.scene, Color::BLACK)
            }
            None => return,
        };
        match result {
            FrameResult::Presented | FrameResult::Skipped => {}
            FrameResult::Reconfigured => window.request_redraw(),
            FrameResult::Lost => {
                self.gpu = None;
                match Gpu::new(Arc::clone(&window), size.width, size.height) {
                    Ok(gpu) => self.gpu = Some(gpu),
                    Err(e) => eprintln!("atomik: {e}"),
                }
                window.request_redraw();
            }
        }
    }

    fn take_screenshot(&mut self, path: &std::path::Path) {
        let (w, h) = self.shell.physical_size();
        self.paint_scene();
        // The scene was painted, but the window has not shown it yet.
        self.shell.core_mut().view_mut().request_repaint();
        match render_offscreen(&self.scene, Color::BLACK, w, h)
            .and_then(|pixels| write_png(path, &pixels, w, h))
        {
            Ok(()) => eprintln!("atomik: screenshot {w}x{h} written to {}", path.display()),
            Err(e) => eprintln!("atomik: screenshot failed: {e}"),
        }
    }

    fn update_title(&mut self) {
        let title = self.shell.title();
        if title != self.title {
            if let Some(w) = &self.window {
                w.set_title(&title);
            }
            self.title = title;
        }
    }

    fn after_event(&mut self, event_loop: &ActiveEventLoop, requests: Vec<ShellRequest>) {
        self.handle_requests(event_loop, requests);
        self.update_title();
        self.update_cursor();
        #[cfg(target_os = "macos")]
        if let Some(native) = &mut self.native {
            native.update(&self.shell.menu_model());
        }
        if self.shell.needs_repaint()
            && let Some(w) = &self.window
        {
            w.request_redraw();
        }
    }

    /// Shows the cursor the shell asks for (set only when it changes).
    fn update_cursor(&mut self) {
        let hint = self.shell.cursor();
        if hint != self.cursor
            && let Some(w) = &self.window
        {
            w.set_cursor(translate::cursor_icon(hint));
            self.cursor = hint;
        }
    }

    fn handle_requests(&mut self, event_loop: &ActiveEventLoop, requests: Vec<ShellRequest>) {
        for request in requests {
            match request {
                ShellRequest::PickOpenFile => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Open project or patch")
                        .add_filter("Atomik Rack project or patch", &["atomik", "json"])
                        .add_filter("Atomik Rack project", &["atomik"])
                        .add_filter("Atomik Rack patch", &["json"]);
                    if let Some(w) = &self.window {
                        dialog = dialog.set_parent(w.as_ref());
                    }
                    if let Some(path) = dialog.pick_file() {
                        self.shell.open_file(&path);
                    }
                }
                ShellRequest::PickImportFiles => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Import audio")
                        .add_filter("Audio files", &crate::audio_import::AUDIO_EXTENSIONS);
                    if let Some(w) = &self.window {
                        dialog = dialog.set_parent(w.as_ref());
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
                    if let Some(w) = &self.window {
                        dialog = dialog.set_parent(w.as_ref());
                    }
                    if let Some(path) = dialog.pick_file() {
                        self.shell.import_midi(&path, as_song);
                    }
                }
                ShellRequest::PickExportMidi(what) => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title(format!("Export MIDI {}", what.word()))
                        .add_filter("MIDI file", &["mid"])
                        .set_file_name(format!("{}.mid", self.shell.midi_export_name(what)));
                    if let Some(w) = &self.window {
                        dialog = dialog.set_parent(w.as_ref());
                    }
                    if let Some(mut path) = dialog.save_file() {
                        if path.extension().is_none() {
                            path.set_extension("mid");
                        }
                        self.shell.export_midi_to(what, &path);
                    }
                }
                ShellRequest::PickLoadSamples => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Load samples")
                        .add_filter("Audio files", &crate::audio_import::AUDIO_EXTENSIONS);
                    if let Some(w) = &self.window {
                        dialog = dialog.set_parent(w.as_ref());
                    }
                    if let Some(paths) = dialog.pick_files() {
                        self.shell.load_samples(paths);
                    }
                }
                ShellRequest::PickSaveFile | ShellRequest::PickExportFile => {
                    let export = request == ShellRequest::PickExportFile;
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
                    if let Some(w) = &self.window {
                        dialog = dialog.set_parent(w.as_ref());
                    }
                    if let Some(mut path) = dialog.save_file() {
                        if path.extension().is_none() {
                            path.set_extension(ext);
                        }
                        if export {
                            self.shell.export_to(&path);
                        } else {
                            self.shell.save_to(&path);
                        }
                    }
                }
                ShellRequest::PickExportKeymap => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Export keyboard shortcuts")
                        .add_filter("Atomik keymap", &["json"])
                        .set_file_name("keymap.json");
                    if let Some(w) = &self.window {
                        dialog = dialog.set_parent(w.as_ref());
                    }
                    if let Some(mut path) = dialog.save_file() {
                        if path.extension().is_none() {
                            path.set_extension("json");
                        }
                        self.shell.export_keymap_to(&path);
                    }
                }
                ShellRequest::PickImportKeymap => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Import keyboard shortcuts")
                        .add_filter("Atomik keymap", &["json"]);
                    if let Some(w) = &self.window {
                        dialog = dialog.set_parent(w.as_ref());
                    }
                    if let Some(path) = dialog.pick_file() {
                        self.shell.import_keymap_from(&path);
                    }
                }
                ShellRequest::PickRelinkFile => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Locate the audio file")
                        .add_filter("Audio files", &crate::audio_import::AUDIO_EXTENSIONS);
                    if let Some(w) = &self.window {
                        dialog = dialog.set_parent(w.as_ref());
                    }
                    if let Some(path) = dialog.pick_file() {
                        self.shell.relink_file(path);
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
                    if let Some(w) = &self.window {
                        dialog = dialog.set_parent(w.as_ref());
                    }
                    if let Some(mut path) = dialog.save_file() {
                        if path.extension().is_none() {
                            path.set_extension("json");
                        }
                        self.shell.export_midi_preset_to(&path);
                    }
                }
                ShellRequest::PickImportMidiPreset => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Import MIDI mapping preset")
                        .add_filter("MIDI mapping preset", &["json"]);
                    if let Some(dir) = self.shell.midi_preset_dir() {
                        dialog = dialog.set_directory(dir);
                    }
                    if let Some(w) = &self.window {
                        dialog = dialog.set_parent(w.as_ref());
                    }
                    if let Some(path) = dialog.pick_file() {
                        self.shell.import_midi_preset_from(&path);
                    }
                }
                ShellRequest::PickRelinkFolder => {
                    let mut dialog =
                        rfd::FileDialog::new().set_title("Search a folder for the missing audio");
                    if let Some(w) = &self.window {
                        dialog = dialog.set_parent(w.as_ref());
                    }
                    if let Some(folder) = dialog.pick_folder() {
                        self.shell.relink_search_folder(folder);
                    }
                }
                ShellRequest::PickPluginFolder => {
                    let mut dialog = rfd::FileDialog::new().set_title("Add a plugin folder");
                    if let Some(w) = &self.window {
                        dialog = dialog.set_parent(w.as_ref());
                    }
                    if let Some(folder) = dialog.pick_folder() {
                        self.shell.add_plugin_folder(folder);
                    }
                }
                ShellRequest::PickLibraryFolder => {
                    let mut dialog = rfd::FileDialog::new().set_title("Choose the library folder");
                    if let Some(w) = &self.window {
                        dialog = dialog.set_parent(w.as_ref());
                    }
                    if let Some(folder) = dialog.pick_folder() {
                        self.shell.set_library_folder(folder);
                    }
                }
                ShellRequest::ConfirmDiscard(command) => {
                    let mut dialog = rfd::MessageDialog::new()
                        .set_level(rfd::MessageLevel::Warning)
                        .set_title("Unsaved changes")
                        .set_description("The rack has unsaved changes. Discard them?")
                        .set_buttons(rfd::MessageButtons::YesNo);
                    if let Some(w) = &self.window {
                        dialog = dialog.set_parent(w.as_ref());
                    }
                    if dialog.show() == rfd::MessageDialogResult::Yes {
                        let more = self.shell.confirmed(command);
                        self.handle_requests(event_loop, more);
                    }
                }
                ShellRequest::ConfirmDiscardOpen(path) => {
                    let mut dialog = rfd::MessageDialog::new()
                        .set_level(rfd::MessageLevel::Warning)
                        .set_title("Unsaved changes")
                        .set_description("The rack has unsaved changes. Discard them?")
                        .set_buttons(rfd::MessageButtons::YesNo);
                    if let Some(w) = &self.window {
                        dialog = dialog.set_parent(w.as_ref());
                    }
                    if dialog.show() == rfd::MessageDialogResult::Yes {
                        self.shell.open_file(&path);
                    }
                }
                // The detached windows are opened by the platform windows work (Agent C);
                // until then there is no other window to bring forward or maximise.
                ShellRequest::FocusWindow(_) | ShellRequest::ToggleMaximise(_) => {}
                ShellRequest::Quit => {
                    self.shutdown();
                    event_loop.exit();
                }
            }
        }
    }

    /// Hands the collected file drag events to the shell (as one event, see [`FileDrag`]).
    fn flush_files(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(event) = self.drag.flush(self.pointer) {
            self.ui(event_loop, event);
        }
    }

    fn ui(&mut self, event_loop: &ActiveEventLoop, event: UiEvent) {
        let requests = self.shell.handle(&event);
        self.after_event(event_loop, requests);
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title(self.shell.title())
            .with_inner_size(LogicalSize::new(1280.0, 860.0))
            .with_min_inner_size(LogicalSize::new(480.0, 320.0));
        let window = match event_loop.create_window(attributes) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("atomik: cannot create the window: {e}");
                event_loop.exit();
                return;
            }
        };
        let scale = window.scale_factor();
        let size = window.inner_size();
        self.shell.handle(&UiEvent::ScaleChanged(scale));
        self.shell
            .handle(&UiEvent::Resize(translate::logical_size(size, scale)));
        match Gpu::new(Arc::clone(&window), size.width, size.height) {
            Ok(gpu) => self.gpu = Some(gpu),
            Err(e) => {
                eprintln!("atomik: {e}");
                event_loop.exit();
                return;
            }
        }
        #[cfg(target_os = "macos")]
        {
            self.native = super::native_menu::NativeMenus::attach(&self.shell.menu_model());
        }
        // The owner of the editors that live in a plugin's own process (sandbox).
        self.shell
            .core_mut()
            .set_plugin_editor_owner(super::plugin_windows::owner_of(&window));
        self.window = Some(Arc::clone(&window));
        self.update_title();
        if self.screenshot_at.is_none()
            && let Some(path) = self.screenshot.take()
        {
            self.take_screenshot(&path);
        }
        window.request_redraw();
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        // A plugin editor's own window: its resize and close go to the core.
        if self.plugin_windows.owns(id) {
            self.plugin_windows.event(id, event, self.shell.core_mut());
            return;
        }
        let scale = self.scale();
        match &event {
            WindowEvent::HoveredFile(path) => {
                self.drag.hovered(path.clone());
                return;
            }
            WindowEvent::HoveredFileCancelled => {
                self.drag.hover_cancelled();
                return;
            }
            WindowEvent::DroppedFile(path) => {
                self.drag.dropped(path.clone());
                return;
            }
            // Anything else ends the group: keep the order of events.
            _ => self.flush_files(event_loop),
        }
        match event {
            WindowEvent::CloseRequested => {
                let requests = self.shell.execute(AppCommand::Quit);
                self.after_event(event_loop, requests);
            }
            WindowEvent::Resized(size) => {
                if let Some(gpu) = &mut self.gpu {
                    gpu.resize(size.width, size.height);
                }
                self.ui(
                    event_loop,
                    UiEvent::Resize(translate::logical_size(size, scale)),
                );
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.ui(event_loop, UiEvent::ScaleChanged(scale_factor));
            }
            WindowEvent::ModifiersChanged(m) => {
                self.modifiers = translate::modifiers(m.state());
                if let Some(event) = self.mod_events.update(m.state()) {
                    self.ui(event_loop, event);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.pointer = translate::logical(position, scale);
                let event = UiEvent::PointerMove {
                    pos: self.pointer,
                    modifiers: self.modifiers,
                };
                self.ui(event_loop, event);
            }
            WindowEvent::CursorLeft { .. } => self.ui(event_loop, UiEvent::PointerLeave),
            WindowEvent::MouseInput { state, button, .. } => {
                let mapped = match state {
                    ElementState::Pressed => self.buttons.press(button, self.modifiers, self.os),
                    ElementState::Released => self.buttons.release(button, self.modifiers),
                };
                let Some((button, modifiers)) = mapped else {
                    return;
                };
                let event = match state {
                    ElementState::Pressed => {
                        let at = clock::now().saturating_duration_since(self.epoch);
                        UiEvent::PointerDown {
                            pos: self.pointer,
                            button,
                            modifiers,
                            clicks: self.clicks.press(at, self.pointer, button),
                        }
                    }
                    ElementState::Released => UiEvent::PointerUp {
                        pos: self.pointer,
                        button,
                        modifiers,
                    },
                };
                self.ui(event_loop, event);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let event = UiEvent::Wheel {
                    delta: translate::wheel(delta, scale),
                    pos: self.pointer,
                    modifiers: self.modifiers,
                };
                self.ui(event_loop, event);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let key = translate::key_event(
                    &event.logical_key,
                    &event.physical_key,
                    self.modifiers,
                    self.os,
                );
                let event = UiEvent::Key {
                    key,
                    modifiers: self.modifiers,
                    pressed: event.state.is_pressed(),
                };
                self.ui(event_loop, event);
            }
            WindowEvent::RedrawRequested => self.redraw(),
            WindowEvent::Focused(false) => {
                // The releases of the keys held now will never arrive.
                self.modifiers = Modifiers::NONE;
                self.mod_events.reset();
                self.shell.focus_lost();
            }
            WindowEvent::Occluded(false) | WindowEvent::Focused(true) => {
                self.shell.core_mut().view_mut().request_repaint();
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.flush_files(event_loop);
        let now = clock::now();
        if self.quit_at.is_some_and(|t| now >= t) {
            self.shutdown();
            event_loop.exit();
            return;
        }
        #[cfg(target_os = "macos")]
        {
            let commands = self
                .native
                .as_ref()
                .map(super::native_menu::NativeMenus::poll)
                .unwrap_or_default();
            for command in commands {
                let requests = self.shell.execute(command);
                self.after_event(event_loop, requests);
            }
        }
        // The plugin editors the core asked for, and the requests the shell raised between events.
        self.plugin_windows.pump(event_loop, self.shell.core_mut());
        let requests = self.shell.take_requests();
        if !requests.is_empty() {
            self.handle_requests(event_loop, requests);
        }
        // Engine garbage is dropped here, on the UI thread, every tick; the transport
        // position and recorded notes are read too.
        // Meters and their ballistics use the monotonic time since start.
        self.shell
            .frame_at(now.saturating_duration_since(self.epoch).as_secs_f64());
        if self.screenshot_at.is_some_and(|t| now >= t)
            && let Some(path) = self.screenshot.take()
        {
            self.screenshot_at = None;
            self.take_screenshot(&path);
        }
        if now >= self.next_status {
            self.next_status = now + STATUS_PERIOD;
            self.shell.poll_status();
        }
        if self.shell.needs_repaint()
            && let Some(w) = &self.window
        {
            w.request_redraw();
        }
        // Audio imports and loads finish on worker threads: poll them even without a stream.
        let running = self.shell.core().sink().is_running()
            || self.shell.is_busy()
            || self.shell.core().view().is_auto_scrolling()
            || self.plugin_windows.any()
            || self.shell.core().plugin_instance_count() > 0
            || !self.shell.core().plugins_ready();
        let mut deadline = if running { Some(now + FRAME) } else { None };
        for t in [self.quit_at, self.screenshot_at].into_iter().flatten() {
            deadline = Some(deadline.map_or(t, |d| d.min(t)));
        }
        event_loop.set_control_flow(match deadline {
            Some(d) => ControlFlow::WaitUntil(d),
            None => ControlFlow::Wait,
        });
    }
}
