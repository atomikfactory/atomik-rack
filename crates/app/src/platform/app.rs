//! The winit 0.30 application: the main window and the detached rack and mixer windows, event
//! translation and routing, frame pacing, dialogs.

use super::gpu::{FrameResult, GpuContext, WindowSurface, render_offscreen, write_png};
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
use crate::shell::{Shell, ShellOptions, ShellRequest};
use crate::stress::{
    STRESS_AUDIO_THRESHOLD_BYTES, add_stress_character, add_stress_containers, add_stress_drums,
    add_stress_fx, add_stress_players, add_stress_plugins, add_stress_samplers, add_stress_slicers,
    stress_audio_dir, stress_drums_dir, stress_sampler_dir, stress_slicer_dir, write_stress_audio,
};
use crate::translate::{self, FileDrag};
use crate::ui_state::ui_state_path;
use crate::windows::{self, MonitorInfo, WindowHit, WindowKind, WindowRect, ZOrder};
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
    // Panes in windows of their own at start, on top of the ones ui-state.json restored.
    for kind in &cli.detach {
        let command = match kind {
            WindowKind::Console => AppCommand::DetachConsole,
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
    eprintln!("atomik: {status}");
    let event_loop = EventLoop::new().map_err(|e| format!("event loop: {e}"))?;
    let now = clock::now();
    let screenshot_at = cli
        .screenshot_after
        .filter(|s| s.is_finite() && *s >= 0.0)
        .map(|s| now + Duration::from_secs_f64(s));
    let mut app = App {
        shell,
        gpu: GpuContext::new(),
        windows: BTreeMap::new(),
        custom_cursors: BTreeMap::new(),
        ids: BTreeMap::new(),
        zorder: ZOrder::default(),
        last_focused: WindowKind::Main,
        drag_noted: false,
        text: TextEngine::new(),
        modifiers: Modifiers::NONE,
        mod_events: translate::ModifierTracker::default(),
        buttons: translate::PointerMap::default(),
        os: Os::host(),
        clicks: ClickCounter::new(),
        epoch: now,
        next_status: now,
        screenshot: cli.screenshot.clone(),
        screenshot_at,
        quit_at: cli
            .quit_after
            .filter(|s| s.is_finite() && *s >= 0.0)
            .map(|s| now + Duration::from_secs_f64(s)),
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
}

struct App {
    shell: Shell<SystemBackend>,
    gpu: GpuContext,
    /// The open windows; the main window is created at start and lives to the end.
    windows: BTreeMap<WindowKind, AppWindow>,
    /// The drawn tool cursors made so far, by picture and bitmap side.
    custom_cursors: BTreeMap<(CursorArt, u32), CustomCursor>,
    ids: BTreeMap<WindowId, WindowKind>,
    /// The windows by focus, to find the one under the pointer during a drag.
    zorder: ZOrder,
    /// The window that last had the focus: the parent of the dialogs.
    last_focused: WindowKind,
    /// The note about drags between windows was said for this drag.
    drag_noted: bool,
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
    screenshot: Option<PathBuf>,
    /// When set, the screenshot waits until then (else it is taken at the first frame).
    screenshot_at: Option<Instant>,
    quit_at: Option<Instant>,
    stopped: bool,
    /// The plugin editor windows (separate top-level windows).
    plugin_windows: PluginWindows,
    #[cfg(target_os = "macos")]
    native: Option<super::native_menu::NativeMenus>,
}

/// The monitors as [`MonitorInfo`] and the index of the primary one.
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
        for kind in self.windows.keys().copied().collect::<Vec<_>>() {
            self.report_geom(kind);
        }
        self.shell.shutdown();
        self.shell.poll_status();
        eprintln!("atomik: at exit: {}", self.shell.status_text());
        // MIDI, stream, then the engine, all dropped here on the UI thread; then the plugins
        // (editors closed, the crash journal removed: a clean exit).
        self.shell.core_mut().sink_mut().shutdown();
        self.shell.core_mut().shutdown_plugins();
    }

    /// The window the dialogs belong to: the one that last had the focus.
    fn parent_window(&self) -> Option<&Window> {
        self.windows
            .get(&self.last_focused)
            .or_else(|| self.windows.get(&WindowKind::Main))
            .map(|w| w.window.as_ref())
    }

    /// Opens and closes the OS windows to match what the shell wants (call after every call
    /// into the shell). A window that cannot be opened is attached again.
    fn sync_windows(&mut self, event_loop: &ActiveEventLoop) {
        if self.stopped {
            return;
        }
        let open: Vec<_> = self.windows.keys().copied().collect();
        let plan = windows::reconcile(&open, &self.shell.windows_wanted());
        for kind in plan.close {
            self.close_window(kind);
        }
        for kind in plan.create {
            if let Err(e) = self.create_window(event_loop, kind) {
                eprintln!("atomik: cannot create the {kind:?} window: {e}");
                if kind == WindowKind::Main {
                    event_loop.exit();
                } else {
                    self.shell.window_closed(kind);
                }
            }
        }
        self.pace_presentation();
    }

    /// Lets the surfaces wait for the vertical blank only while one window is open.
    fn pace_presentation(&mut self) {
        let vsync = window_logic::presents_with_vsync(self.windows.len());
        for w in self.windows.values_mut() {
            if let Some(surface) = &mut w.surface {
                self.gpu.set_vsync(surface, vsync);
            }
        }
    }

    /// Opens the window of `kind`: placed from its saved geometry and the monitors of the
    /// moment, told its scale and size, with a surface.
    fn create_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        kind: WindowKind,
    ) -> Result<(), String> {
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
        let placement = if kind == WindowKind::Main {
            windows::place(saved.as_ref(), default, MIN_SIZE, &monitors, primary)
        } else {
            // A detached window with nothing saved goes to a monitor the app is not on yet.
            let occupied = self.desktop_rects().unwrap_or_default();
            windows::place_detached(
                saved.as_ref(),
                default,
                MIN_SIZE,
                &monitors,
                primary,
                &occupied,
            )
        };
        let mut attributes = Window::default_attributes()
            .with_title(self.shell.title_in(kind))
            .with_inner_size(LogicalSize::new(placement.size.0, placement.size.1))
            .with_min_inner_size(LogicalSize::new(MIN_SIZE.0, MIN_SIZE.1))
            .with_maximized(placement.maximised)
            .with_window_icon(window_logic::window_icon());
        if let Some(pos) = placement.pos {
            attributes = attributes.with_position(window_logic::position_for(space, pos));
        }
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .map_err(|e| e.to_string())?,
        );
        let scale = window.scale_factor();
        let size = window.inner_size();
        let surface = self
            .gpu
            .create_surface(Arc::clone(&window), size.width, size.height)?;
        self.shell.handle_in(kind, &UiEvent::ScaleChanged(scale));
        self.shell
            .handle_in(kind, &UiEvent::Resize(translate::logical_size(size, scale)));
        if kind == WindowKind::Main {
            // The owner of the editors that live in a plugin's own process (sandbox).
            self.shell
                .core_mut()
                .set_plugin_editor_owner(super::plugin_windows::owner_of(&window));
        }
        self.ids.insert(window.id(), kind);
        self.zorder.focus(kind);
        self.windows.insert(
            kind,
            AppWindow {
                surface: Some(surface),
                window: Arc::clone(&window),
                scene: Scene::new(),
                cache: SceneCache::new(),
                pointer: Point::ZERO,
                file_drag: FileDrag::new(),
                cursor: (CursorHint::Default, 0),
                title: self.shell.title_in(kind),
            },
        );
        self.report_geom(kind);
        window.request_redraw();
        Ok(())
    }

    /// Closes the window of `kind` (the shell attached its pane).
    fn close_window(&mut self, kind: WindowKind) {
        self.windows.remove(&kind);
        self.ids.retain(|_, k| *k != kind);
        self.zorder.remove(kind);
        if self.last_focused == kind {
            self.last_focused = WindowKind::Main;
        }
    }

    /// Tells the shell where the window of `kind` is (it saves it).
    fn report_geom(&mut self, kind: WindowKind) {
        let Some(w) = self.windows.get(&kind) else {
            return;
        };
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
        if let Some(geom) = geom {
            self.shell.window_moved(kind, geom);
        }
    }

    /// Every window's rect on the desktop; `None` when the OS gives no positions (Wayland).
    fn desktop_rects(&self) -> Option<Vec<WindowRect>> {
        self.windows
            .iter()
            .map(|(kind, w)| {
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
    fn window_under_pointer(&mut self, kind: WindowKind, pos: Point) -> Option<WindowHit> {
        let rects = self.desktop_rects();
        let Some(rects) = rects.filter(|r| r.iter().any(|r| r.kind == kind)) else {
            // No positions: the pointer is over its own window or over nothing of the app.
            let size = self.shell.window_size(kind)?;
            let hit = window_logic::local_hit(kind, pos, (size.width, size.height));
            if hit.is_none() && self.windows.len() > 1 && !self.drag_noted {
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
            (pos.x * own.scale, pos.y * own.scale),
        );
        windows::window_under(&rects, point, &self.zorder)
    }

    /// Hands one event of `kind`'s window to the shell. During a drag of files the window under
    /// the pointer is worked out here, since the OS keeps sending the events to the window
    /// that got the press.
    fn dispatch(&mut self, kind: WindowKind, event: &UiEvent) -> Vec<ShellRequest> {
        let Some(pos) = window_logic::pointer_of(event).filter(|_| self.shell.cross_window_drag())
        else {
            self.drag_noted = false;
            return self.shell.handle_in(kind, event);
        };
        let under = self.window_under_pointer(kind, pos);
        self.shell.handle_routed(kind, event, under)
    }

    /// Paints `kind`'s window into its scene.
    fn paint_scene(&mut self, kind: WindowKind) {
        let Some(w) = self.windows.get_mut(&kind) else {
            return;
        };
        w.scene.reset();
        let mut canvas = SceneCanvas::new(&mut w.scene, &mut self.text).with_cache(&mut w.cache);
        self.shell.paint_in(kind, &mut canvas);
        w.cache.trim();
    }

    fn redraw(&mut self, kind: WindowKind) {
        let Some(window) = self.windows.get(&kind).map(|w| Arc::clone(&w.window)) else {
            return;
        };
        let size = window.inner_size();
        if size.width == 0 || size.height == 0 {
            return; // minimized
        }
        self.paint_scene(kind);
        let Some(w) = self.windows.get_mut(&kind) else {
            return;
        };
        let result = match &mut w.surface {
            Some(surface) => {
                self.gpu.resize(surface, size.width, size.height);
                self.gpu.render(surface, &w.scene, Color::BLACK)
            }
            None => return,
        };
        match result {
            FrameResult::Presented => {}
            // Painted but not shown: the shell has marked the window clean, so ask again.
            FrameResult::Skipped => self.shell.repaint_window(kind),
            FrameResult::Reconfigured => window.request_redraw(),
            FrameResult::Lost => {
                w.surface = None;
                match self
                    .gpu
                    .create_surface(Arc::clone(&window), size.width, size.height)
                {
                    Ok(surface) => w.surface = Some(surface),
                    Err(e) => eprintln!("atomik: {e}"),
                }
                window.request_redraw();
            }
        }
    }

    /// Renders every open window offscreen: `base` for the main window, `base-rack` and
    /// `base-console` (before the extension) for the detached ones.
    fn take_screenshots(&mut self, base: &std::path::Path) {
        for kind in self.windows.keys().copied().collect::<Vec<_>>() {
            let Some((w, h)) = self.shell.physical_size_in(kind) else {
                continue;
            };
            self.paint_scene(kind);
            // The scene was painted, but the window has not shown it yet.
            self.shell.core_mut().view_mut().request_repaint();
            let path = window_logic::screenshot_path(base, kind);
            let Some(window) = self.windows.get(&kind) else {
                continue;
            };
            match render_offscreen(&window.scene, Color::BLACK, w, h)
                .and_then(|pixels| write_png(&path, &pixels, w, h))
            {
                Ok(()) => eprintln!("atomik: screenshot {w}x{h} written to {}", path.display()),
                Err(e) => eprintln!("atomik: screenshot failed: {e}"),
            }
        }
    }

    /// Sets each window's title when it changed.
    fn update_titles(&mut self) {
        for (kind, w) in &mut self.windows {
            if let Some(title) = window_logic::changed(&w.title, &self.shell.title_in(*kind)) {
                w.window.set_title(&title);
                w.title = title;
            }
        }
    }

    /// Shows the cursor the shell asks for in each window (set only when it changes). The tool
    /// cursors are drawn bitmaps, made once per size and kept.
    fn update_cursors(&mut self, event_loop: &ActiveEventLoop) {
        for (kind, w) in &mut self.windows {
            let size = cursor_art::size_for(w.window.scale_factor());
            let want = (self.shell.cursor_in(*kind), size);
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

    /// Paints and presents every window that has something to paint.
    fn paint_due_windows(&mut self) {
        for kind in self.shell.windows_to_paint() {
            self.redraw(kind);
        }
    }

    /// Asks for a redraw of every window that has something to paint.
    fn request_redraws(&self) {
        for (kind, w) in &self.windows {
            if self.shell.needs_repaint_in(*kind) {
                w.window.request_redraw();
            }
        }
    }

    fn after_event(&mut self, event_loop: &ActiveEventLoop, requests: Vec<ShellRequest>) {
        self.sync_windows(event_loop);
        self.handle_requests(event_loop, requests);
        self.sync_windows(event_loop);
        self.update_titles();
        self.update_cursors(event_loop);
        #[cfg(target_os = "macos")]
        if let Some(native) = &mut self.native {
            native.update(&self.shell.menu_model());
        }
        self.request_redraws();
    }

    /// Brings the window of `kind` forward and gives it the keyboard.
    fn focus_window(&self, kind: WindowKind) {
        if let Some(w) = self.windows.get(&kind) {
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
                        .add_filter("Atomik Rack project or patch", &["atomik", "json"])
                        .add_filter("Atomik Rack project", &["atomik"])
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
                ShellRequest::PickExportAudio(scope) => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title(match scope {
                            atomik_ui::ExportScope::Song => "Export song as audio file",
                            atomik_ui::ExportScope::Loop => "Export loop as audio file",
                        })
                        .add_filter("WAV audio file", &["wav"])
                        .set_file_name(self.shell.export_audio_name(scope));
                    if let Some(dir) = self.shell.export_audio_dir() {
                        dialog = dialog.set_directory(dir);
                    }
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if let Some(mut path) = dialog.save_file() {
                        if path.extension().is_none() {
                            path.set_extension("wav");
                        }
                        self.shell.export_audio_to(scope, &path);
                    }
                }
                ShellRequest::PickLoadSamples => {
                    let mut dialog = rfd::FileDialog::new()
                        .set_title("Load samples")
                        .add_filter("Audio files", &crate::audio_import::AUDIO_EXTENSIONS);
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
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
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
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
                ShellRequest::ConfirmDiscard(command) => {
                    let mut dialog = rfd::MessageDialog::new()
                        .set_level(rfd::MessageLevel::Warning)
                        .set_title("Unsaved changes")
                        .set_description("The rack has unsaved changes. Discard them?")
                        .set_buttons(rfd::MessageButtons::YesNo);
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
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
                    if let Some(w) = self.parent_window() {
                        dialog = dialog.set_parent(w);
                    }
                    if dialog.show() == rfd::MessageDialogResult::Yes {
                        self.shell.open_file(&path);
                    }
                }
                ShellRequest::FocusWindow(kind) => self.focus_window(kind),
                ShellRequest::ToggleMaximise(kind) => {
                    if let Some(w) = self.windows.get(&kind) {
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
    fn flush_files(&mut self, event_loop: &ActiveEventLoop, kind: WindowKind) {
        let Some(w) = self.windows.get_mut(&kind) else {
            return;
        };
        if let Some(event) = w.file_drag.flush(w.pointer) {
            self.ui(event_loop, kind, event);
        }
    }

    fn flush_all_files(&mut self, event_loop: &ActiveEventLoop) {
        for kind in self.windows.keys().copied().collect::<Vec<_>>() {
            self.flush_files(event_loop, kind);
        }
    }

    fn ui(&mut self, event_loop: &ActiveEventLoop, kind: WindowKind, event: UiEvent) {
        let requests = self.dispatch(kind, &event);
        self.after_event(event_loop, requests);
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if !self.windows.is_empty() {
            return;
        }
        // The main window and the panes that were detached when the app last closed.
        self.sync_windows(event_loop);
        if !self.windows.contains_key(&WindowKind::Main) {
            return;
        }
        #[cfg(target_os = "macos")]
        {
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

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        // A plugin editor's own window: its resize and close go to the core.
        if self.plugin_windows.owns(id) {
            self.plugin_windows.event(id, event, self.shell.core_mut());
            return;
        }
        let Some(&kind) = self.ids.get(&id) else {
            return;
        };
        match &event {
            WindowEvent::HoveredFile(path) => {
                if let Some(w) = self.windows.get_mut(&kind) {
                    w.file_drag.hovered(path.clone());
                }
                return;
            }
            WindowEvent::HoveredFileCancelled => {
                if let Some(w) = self.windows.get_mut(&kind) {
                    w.file_drag.hover_cancelled();
                }
                return;
            }
            WindowEvent::DroppedFile(path) => {
                if let Some(w) = self.windows.get_mut(&kind) {
                    w.file_drag.dropped(path.clone());
                }
                return;
            }
            // Anything else ends the group: keep the order of events.
            _ => self.flush_files(event_loop, kind),
        }
        let Some((scale, pointer)) = self
            .windows
            .get(&kind)
            .map(|w| (w.window.scale_factor(), w.pointer))
        else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => {
                if kind == WindowKind::Main {
                    let requests = self.shell.execute(AppCommand::Quit);
                    self.after_event(event_loop, requests);
                } else {
                    // Closing a detached window attaches its pane again.
                    self.shell.window_closed(kind);
                    self.after_event(event_loop, Vec::new());
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(w) = self.windows.get_mut(&kind)
                    && let Some(surface) = &mut w.surface
                {
                    self.gpu.resize(surface, size.width, size.height);
                }
                self.ui(
                    event_loop,
                    kind,
                    UiEvent::Resize(translate::logical_size(size, scale)),
                );
                self.report_geom(kind);
            }
            WindowEvent::Moved(_) => self.report_geom(kind),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.ui(event_loop, kind, UiEvent::ScaleChanged(scale_factor));
            }
            WindowEvent::ModifiersChanged(m) => {
                self.modifiers = translate::modifiers(m.state());
                if let Some(event) = self.mod_events.update(m.state()) {
                    self.ui(event_loop, kind, event);
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let pointer = translate::logical(position, scale);
                if let Some(w) = self.windows.get_mut(&kind) {
                    w.pointer = pointer;
                }
                let event = UiEvent::PointerMove {
                    pos: pointer,
                    modifiers: self.modifiers,
                };
                self.ui(event_loop, kind, event);
            }
            WindowEvent::CursorLeft { .. } => self.ui(event_loop, kind, UiEvent::PointerLeave),
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
                self.ui(event_loop, kind, event);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let event = UiEvent::Wheel {
                    delta: translate::wheel(delta, scale),
                    pos: pointer,
                    modifiers: self.modifiers,
                };
                self.ui(event_loop, kind, event);
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
                self.ui(event_loop, kind, event);
            }
            WindowEvent::RedrawRequested => self.redraw(kind),
            WindowEvent::Focused(false) => {
                // The releases of the keys held now will never arrive.
                self.modifiers = Modifiers::NONE;
                self.mod_events.reset();
                self.shell.focus_lost();
            }
            WindowEvent::Focused(true) => {
                self.zorder.focus(kind);
                self.last_focused = kind;
                self.shell.window_focused(kind);
                self.shell.core_mut().view_mut().request_repaint();
                self.after_event(event_loop, Vec::new());
                if let Some(w) = self.windows.get(&kind) {
                    w.window.request_redraw();
                }
            }
            WindowEvent::Occluded(false) => {
                self.shell.core_mut().view_mut().request_repaint();
                if let Some(w) = self.windows.get(&kind) {
                    w.window.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.flush_all_files(event_loop);
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
        self.sync_windows(event_loop);
        if self.screenshot_at.is_some_and(|t| now >= t)
            && let Some(path) = self.screenshot.take()
        {
            self.screenshot_at = None;
            self.take_screenshots(&path);
        }
        if now >= self.next_status {
            self.next_status = now + STATUS_PERIOD;
            self.shell.poll_status();
        }
        // Only the windows with something new repaint: the meters redraw the console's window.
        // Every one of them is painted here, in turn. Leaving it to the OS's redraw requests
        // would let the one the OS serves first (the focused one, on Windows) take all the
        // time and starve the others while it animates.
        self.paint_due_windows();
        // Audio imports and loads finish on worker threads: poll them even without a stream.
        let running = self.shell.core().sink().is_running()
            || self.shell.is_busy()
            || self.shell.context_menu_pending()
            || self.shell.tooltip_pending()
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
