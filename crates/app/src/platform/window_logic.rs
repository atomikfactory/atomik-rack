//! The decisions of the platform's window code that need no window: the units of the desktop,
//! the monitor and geometry conversions, and where a pointer is during a drag.
//!
//! `app.rs` converts winit's types at the edge and calls these; everything here is plain data
//! and unit-tested. See [`crate::windows`] for the model they feed.

use crate::ui_activity::PaintReason;
use crate::windows::{
    DesktopSpace, DocId, MonitorInfo, WindowGeom, WindowHit, WindowKind, WindowRect,
};
use atomik_ui::UiEvent;
use atomik_ui::kurbo::Point;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use winit::dpi::{LogicalPosition, PhysicalPosition, Position};

/// What a window event says about the paint that follows it, for the paint counters (see
/// [`crate::ui_activity`]); `None` for an event that explains no paint.
pub(super) fn paint_cause_of(event: &winit::event::WindowEvent) -> Option<PaintReason> {
    use winit::event::WindowEvent as E;
    match event {
        E::Resized(_) | E::ScaleFactorChanged { .. } => Some(PaintReason::Resize),
        E::Focused(_) | E::Occluded(_) => Some(PaintReason::Focus),
        E::CursorMoved { .. }
        | E::CursorLeft { .. }
        | E::MouseInput { .. }
        | E::MouseWheel { .. }
        | E::KeyboardInput { .. }
        | E::ModifiersChanged(_)
        | E::Ime(_) => Some(PaintReason::Input),
        _ => None,
    }
}

/// The words of the question about unsaved changes: the same on every platform.
pub(super) const SAVE_BUTTONS: (&str, &str, &str) = ("Save", "Don't Save", "Cancel");

/// What the person answered to the question about unsaved changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SaveAnswer {
    /// Save, then carry on.
    Save,
    /// Carry on without saving.
    Discard,
    /// Leave everything as it is.
    Cancel,
}

/// The answer a message box with [`SAVE_BUTTONS`] returned. A box that could only offer Yes,
/// No and Cancel (they mean the same, in that order) is understood too; anything else, a closed
/// box included, is Cancel.
pub(super) fn save_answer(result: &rfd::MessageDialogResult) -> SaveAnswer {
    use rfd::MessageDialogResult as R;
    let (save, discard, _) = SAVE_BUTTONS;
    match result {
        R::Yes => SaveAnswer::Save,
        R::No => SaveAnswer::Discard,
        R::Custom(text) if text == save => SaveAnswer::Save,
        R::Custom(text) if text == discard => SaveAnswer::Discard,
        _ => SaveAnswer::Cancel,
    }
}

/// The icon of every app window (taskbar, Alt-Tab, title bar), from the embedded logo. `None`
/// leaves the system's default. macOS ignores it: the Dock icon comes from the bundle.
pub(super) fn window_icon() -> Option<winit::window::Icon> {
    let icon = crate::app_icon::app_icon()?;
    winit::window::Icon::from_rgba(icon.rgba.clone(), icon.size, icon.size).ok()
}

/// The units of desktop positions on this OS: points on macOS, physical pixels elsewhere
/// (per-monitor DPI on Windows, X11).
pub(super) const fn desktop_space() -> DesktopSpace {
    if cfg!(target_os = "macos") {
        DesktopSpace::Points
    } else {
        DesktopSpace::Physical
    }
}

/// A scale that is zero or not finite counts as 1.
fn safe(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// A physical position or size in desktop units.
fn units(space: DesktopSpace, physical: f64, scale: f64) -> f64 {
    match space {
        DesktopSpace::Physical => physical,
        DesktopSpace::Points => physical / safe(scale),
    }
}

/// A monitor as winit reports it (physical position and size, its own scale) in the units of
/// [`MonitorInfo`]. A monitor without a name is called `monitor`.
pub(super) fn monitor_info(
    name: Option<String>,
    position: (i32, i32),
    size: (u32, u32),
    scale: f64,
    space: DesktopSpace,
) -> MonitorInfo {
    let conv = |v: f64| units(space, v, scale).round();
    MonitorInfo {
        name: name.unwrap_or_else(|| "monitor".to_string()),
        x: conv(f64::from(position.0)) as i32,
        y: conv(f64::from(position.1)) as i32,
        width: conv(f64::from(size.0)) as u32,
        height: conv(f64::from(size.1)) as u32,
        units_per_logical: match space {
            DesktopSpace::Physical => safe(scale),
            DesktopSpace::Points => 1.0,
        },
    }
}

/// The geometry to remember for a window: its outer position (`None` where the OS gives none,
/// as on Wayland: then 0, 0), inner size, and whether it is maximised. `None` for a window with
/// no area (minimised on Windows), which says nothing about where it is.
pub(super) fn geom_of(
    space: DesktopSpace,
    outer: Option<(i32, i32)>,
    inner: (u32, u32),
    scale: f64,
    maximised: bool,
    monitor: Option<String>,
) -> Option<WindowGeom> {
    if inner.0 == 0 || inner.1 == 0 {
        return None;
    }
    let (x, y) = outer.unwrap_or((0, 0));
    let s = safe(scale);
    Some(WindowGeom {
        x: units(space, f64::from(x), scale).round() as i32,
        y: units(space, f64::from(y), scale).round() as i32,
        width: f64::from(inner.0) / s,
        height: f64::from(inner.1) / s,
        maximised,
        monitor,
    })
}

/// A window's inner rect on the desktop, from its inner top left and size in physical pixels,
/// in desktop units.
pub(super) fn window_rect(
    kind: WindowKind,
    origin: (i32, i32),
    size: (u32, u32),
    scale: f64,
    space: DesktopSpace,
) -> WindowRect {
    WindowRect {
        kind,
        x: units(space, f64::from(origin.0), scale),
        y: units(space, f64::from(origin.1), scale),
        width: units(space, f64::from(size.0), scale),
        height: units(space, f64::from(size.1), scale),
        scale,
        space,
    }
}

/// The window position winit takes for a position in desktop units.
pub(super) fn position_for(space: DesktopSpace, (x, y): (i32, i32)) -> Position {
    match space {
        DesktopSpace::Physical => PhysicalPosition::new(x, y).into(),
        DesktopSpace::Points => LogicalPosition::new(f64::from(x), f64::from(y)).into(),
    }
}

/// Where a pointer at `pos` (logical, in `kind`'s window) is when the OS gives no desktop
/// positions (Wayland): over its own window, or over nothing of the app.
pub(super) fn local_hit(kind: WindowKind, pos: Point, size: (f64, f64)) -> Option<WindowHit> {
    let inside = pos.x >= 0.0 && pos.y >= 0.0 && pos.x < size.0 && pos.y < size.1;
    inside.then_some(WindowHit {
        kind,
        logical: (pos.x, pos.y),
    })
}

/// The position of a pointer event.
pub(super) fn pointer_of(event: &UiEvent) -> Option<Point> {
    match event {
        UiEvent::PointerMove { pos, .. }
        | UiEvent::PointerDown { pos, .. }
        | UiEvent::PointerUp { pos, .. }
        | UiEvent::Wheel { pos, .. } => Some(*pos),
        _ => None,
    }
}

/// The screenshot file of a window of song `doc`: [`screenshot_path`] for the first song, the
/// same with `-song<n>` before the window's own suffix for the songs opened later.
pub(super) fn song_screenshot_path(base: &Path, doc: DocId, kind: WindowKind) -> PathBuf {
    if doc == DocId::FIRST {
        return screenshot_path(base, kind);
    }
    let stem = base
        .file_stem()
        .map_or_else(|| "screenshot".into(), |s| s.to_string_lossy());
    let name = match base.extension() {
        Some(ext) => format!("{stem}-song{}.{}", doc.0, ext.to_string_lossy()),
        None => format!("{stem}-song{}", doc.0),
    };
    screenshot_path(&base.with_file_name(name), kind)
}

/// The file a window's screenshot goes to: `base` for the main window, `base` with `-rack` or
/// `-console` before the extension for the others.
pub(super) fn screenshot_path(base: &Path, kind: WindowKind) -> PathBuf {
    let suffix = match kind {
        WindowKind::Main => return base.to_path_buf(),
        WindowKind::Rack => "rack",
        WindowKind::Console => "console",
        WindowKind::SpectrumEq => "spectrum-eq",
        WindowKind::Browser => "browser",
        WindowKind::PianoKeys => "piano-keys",
        WindowKind::Video => "video",
        WindowKind::Fretboard => "fretboard",
    };
    let stem = base
        .file_stem()
        .map_or_else(|| "screenshot".into(), |s| s.to_string_lossy());
    let name = match base.extension() {
        Some(ext) => format!("{stem}-{suffix}.{}", ext.to_string_lossy()),
        None => format!("{stem}-{suffix}"),
    };
    base.with_file_name(name)
}

/// `Some(new)` only when `new` differs from `old`: what a window property (title, cursor) needs
/// sending to the OS.
pub(super) fn changed<T: PartialEq + Clone>(old: &T, new: &T) -> Option<T> {
    (old != new).then(|| new.clone())
}

/// Whether a surface presents in step with its monitor (vsync). With several windows the app
/// paints them one after the other on one thread, so a presentation that waits for the next
/// vertical blank would stall every other window for a whole refresh of this one's monitor, and
/// the windows on monitors with different rates would drag each other down. They then present
/// without waiting and the app's own tick sets the pace.
pub(super) fn presents_with_vsync(open_windows: usize) -> bool {
    open_windows <= 1
}

/// How often the loop runs its housekeeping frame while an audio stream is open and nothing
/// moves (see [`Wakers::stream`]).
pub(super) const IDLE_STREAM_WAKE: Duration = Duration::from_millis(100);

/// How often the loop wakes while every window is minimized: nothing is painted, only the
/// housekeeping runs.
pub(super) const MINIMIZED_WAKE: Duration = Duration::from_millis(250);

/// How long after the last frame that painted something the loop stays at the full rate, so
/// the meters' ballistics and the lights settle and animations that repaint every few frames
/// keep going.
pub(super) const PAINT_TAIL: Duration = Duration::from_secs(1);

/// What keeps the event loop awake between events.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct Wakers {
    /// Something moves or is polled at the frame rate: the transport plays or records, a worker
    /// is busy, a menu or tooltip is about to open, the rack scrolls, glides or morphs, a plugin
    /// loads, is captured or has an editor open, a button is held, live MIDI was heard (the
    /// questions of the shell and the core, plus what the platform owns).
    pub animating: bool,
    /// An audio stream is open: engine garbage, meters and the plugins are looked at now and then
    /// even when nothing moves.
    pub stream: bool,
    /// A plugin instance is loaded: its notifications are polled.
    pub plugins: bool,
    /// Every window has no area (minimized): nothing is painted.
    pub minimized: bool,
    /// The time since the last frame that painted something; `None` before the first.
    pub since_paint: Option<Duration>,
}

/// How long the loop may sleep.
///
/// Three rates: the frame period while something animates or a paint was less than
/// [`PAINT_TAIL`] ago, [`IDLE_STREAM_WAKE`] while only the stream or loaded plugins want a look,
/// and [`MINIMIZED_WAKE`] instead of both while every window is minimized. With none of them the
/// loop sleeps until an event. The tail only applies where a housekeeping wake exists anyway: a
/// project with no stream and no plugin has no frames to keep going.
pub(super) fn wake_after(w: Wakers, frame: Duration) -> Option<Duration> {
    let housekeeping = w.stream || w.plugins;
    let tail = housekeeping && w.since_paint.is_some_and(|ago| ago < PAINT_TAIL);
    let wanted = w.animating || housekeeping;
    if w.minimized {
        return wanted.then_some(MINIMIZED_WAKE);
    }
    if w.animating || tail {
        Some(frame)
    } else if housekeeping {
        Some(IDLE_STREAM_WAKE)
    } else {
        None
    }
}

/// The pointer buttons held down now. A drag or a hold (a repeating button, a held key of the
/// piano) needs frames even while the pointer does not move.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) struct HeldButtons(u32);

impl HeldButtons {
    /// A button went down.
    pub(super) fn press(&mut self) {
        self.0 = self.0.saturating_add(1);
    }

    /// A button came up (a release whose press this window never saw counts for nothing).
    pub(super) fn release(&mut self) {
        self.0 = self.0.saturating_sub(1);
    }

    /// The releases will never come (the window lost the focus, the interface was reset).
    pub(super) fn reset(&mut self) {
        self.0 = 0;
    }

    /// Whether any button is held.
    pub(super) fn any(self) -> bool {
        self.0 > 0
    }
}

/// Whether a paint restarts the full-rate tail. A frame that was painted again after it was not
/// shown (`Surface`) does not, and neither does a paint that only the status poll caused
/// (`poll_only`): the poll repaints the status line a few times a second while nothing else
/// moves, and counting those would keep the loop at the full rate for ever. The person's input,
/// a view that keeps itself painting (the playhead, the meters, a scroll), a value that moved on
/// a tick, a resize and a focus change do.
pub(super) fn paint_extends_tail(reason: PaintReason, poll_only: bool) -> bool {
    !poll_only && reason != PaintReason::Surface
}

/// Whether the loop paints the windows on this wake: not while they are all minimized.
pub(super) fn paints_on_wake(minimized: bool) -> bool {
    !minimized
}

/// Whether a window with this inner size is minimized (it has no area).
pub(super) fn is_minimized(size: (u32, u32)) -> bool {
    size.0 == 0 || size.1 == 0
}

/// Whether the app counts as minimized: it has windows and none has an area. A window with a
/// plugin editor, a Learn strip or a docked frame beside it is not minimized for this purpose:
/// those windows are pumped on the frame.
pub(super) fn all_minimized(sizes: impl IntoIterator<Item = bool>, others_open: bool) -> bool {
    let mut any = false;
    for minimized in sizes {
        if !minimized {
            return false;
        }
        any = true;
    }
    any && !others_open
}

/// The earliest of the loop's own wake time and the deadlines that must be met exactly (a quit
/// time, a screenshot, a menu switch, the graphics back-off): a deadline earlier than the next
/// housekeeping wake wins.
pub(super) fn earliest(
    base: Option<Instant>,
    deadlines: impl IntoIterator<Item = Option<Instant>>,
) -> Option<Instant> {
    deadlines
        .into_iter()
        .flatten()
        .fold(base, |best, t| Some(best.map_or(t, |b| b.min(t))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows::{ZOrder, to_desktop, window_under};

    #[test]
    fn window_events_name_the_cause_of_the_paint_after_them() {
        use winit::dpi::PhysicalSize;
        use winit::event::{Modifiers, WindowEvent as E};
        let cause = |e: E| paint_cause_of(&e);
        assert_eq!(
            cause(E::Resized(PhysicalSize::new(800, 600))),
            Some(PaintReason::Resize)
        );
        assert_eq!(cause(E::Focused(true)), Some(PaintReason::Focus));
        assert_eq!(cause(E::Occluded(false)), Some(PaintReason::Focus));
        assert_eq!(
            cause(E::ModifiersChanged(Modifiers::default())),
            Some(PaintReason::Input)
        );
        assert_eq!(cause(E::RedrawRequested), None);
        assert_eq!(cause(E::Destroyed), None);
    }

    #[test]
    fn the_save_box_answers_in_its_own_words_on_every_platform() {
        use rfd::MessageDialogResult as R;
        let (save, discard, cancel) = SAVE_BUTTONS;
        assert_eq!((save, discard, cancel), ("Save", "Don't Save", "Cancel"));
        assert_eq!(save_answer(&R::Custom(save.into())), SaveAnswer::Save);
        assert_eq!(save_answer(&R::Custom(discard.into())), SaveAnswer::Discard);
        assert_eq!(save_answer(&R::Custom(cancel.into())), SaveAnswer::Cancel);
        // A box that only had Yes / No / Cancel means the same, and a closed box is Cancel.
        assert_eq!(save_answer(&R::Yes), SaveAnswer::Save);
        assert_eq!(save_answer(&R::No), SaveAnswer::Discard);
        assert_eq!(save_answer(&R::Cancel), SaveAnswer::Cancel);
        assert_eq!(save_answer(&R::Custom("other".into())), SaveAnswer::Cancel);
    }

    #[test]
    fn the_window_icon_is_built_from_the_embedded_logo() {
        assert!(window_icon().is_some());
    }

    const FRAME: Duration = Duration::from_micros(16_667);

    fn wakers() -> Wakers {
        Wakers::default()
    }

    #[test]
    fn a_project_with_no_stream_and_no_plugin_sleeps_until_an_event() {
        assert_eq!(wake_after(wakers(), FRAME), None);
        // The tail of a paint keeps nothing going where nothing wakes the loop anyway.
        let painted = Wakers {
            since_paint: Some(Duration::ZERO),
            ..wakers()
        };
        assert_eq!(wake_after(painted, FRAME), None);
    }

    #[test]
    fn every_animating_cause_runs_frames_at_the_full_rate() {
        for (stream, plugins) in [(false, false), (true, false), (false, true), (true, true)] {
            let w = Wakers {
                animating: true,
                stream,
                plugins,
                ..wakers()
            };
            assert_eq!(wake_after(w, FRAME), Some(FRAME), "{w:?}");
        }
    }

    #[test]
    fn an_open_stream_with_nothing_moving_wakes_ten_times_a_second() {
        let stream = Wakers {
            stream: true,
            ..wakers()
        };
        assert_eq!(wake_after(stream, FRAME), Some(Duration::from_millis(100)));
        let plugins = Wakers {
            plugins: true,
            ..wakers()
        };
        assert_eq!(wake_after(plugins, FRAME), Some(IDLE_STREAM_WAKE));
    }

    #[test]
    fn the_full_rate_lasts_a_second_after_the_last_paint() {
        let at = |ago_ms: u64| Wakers {
            stream: true,
            since_paint: Some(Duration::from_millis(ago_ms)),
            ..wakers()
        };
        assert_eq!(wake_after(at(0), FRAME), Some(FRAME));
        assert_eq!(wake_after(at(999), FRAME), Some(FRAME));
        assert_eq!(wake_after(at(1000), FRAME), Some(IDLE_STREAM_WAKE));
        assert_eq!(wake_after(at(60_000), FRAME), Some(IDLE_STREAM_WAKE));
        // No paint yet: the idle rate.
        let none = Wakers {
            stream: true,
            ..wakers()
        };
        assert_eq!(wake_after(none, FRAME), Some(IDLE_STREAM_WAKE));
    }

    #[test]
    fn minimized_windows_wake_every_quarter_second_and_paint_nothing() {
        let min = |animating, stream, plugins| Wakers {
            animating,
            stream,
            plugins,
            minimized: true,
            since_paint: Some(Duration::ZERO),
        };
        let quarter = Some(Duration::from_millis(250));
        assert_eq!(wake_after(min(false, true, false), FRAME), quarter);
        assert_eq!(wake_after(min(false, false, true), FRAME), quarter);
        // Something that would run frames is looked at at the minimized rate.
        assert_eq!(wake_after(min(true, true, false), FRAME), quarter);
        assert_eq!(wake_after(min(true, false, false), FRAME), quarter);
        // Nothing to look at: no wake at all.
        assert_eq!(wake_after(min(false, false, false), FRAME), None);
        assert!(!paints_on_wake(true));
        assert!(paints_on_wake(false));
    }

    #[test]
    fn only_the_status_poll_and_a_repeated_frame_leave_the_tail_alone() {
        for reason in PaintReason::ALL {
            let repeated = reason == PaintReason::Surface;
            assert_eq!(paint_extends_tail(reason, false), !repeated, "{reason:?}");
            assert!(
                !paint_extends_tail(reason, true),
                "{reason:?}: a poll-only paint"
            );
        }
    }

    #[test]
    fn held_buttons_count_presses_and_releases_and_forget_on_reset() {
        let mut held = HeldButtons::default();
        assert!(!held.any());
        held.press();
        held.press();
        held.release();
        assert!(held.any(), "one of two is still down");
        held.release();
        assert!(!held.any());
        held.release();
        held.press();
        assert!(held.any(), "a stray release does not owe a press");
        held.reset();
        assert!(!held.any());
    }

    #[test]
    fn a_zero_area_window_is_minimized_and_the_app_is_when_all_of_them_are() {
        assert!(is_minimized((0, 0)));
        assert!(is_minimized((0, 600)));
        assert!(is_minimized((800, 0)));
        assert!(!is_minimized((1, 1)));
        assert!(all_minimized([true], false));
        assert!(all_minimized([true, true], false));
        assert!(!all_minimized([true, false], false));
        // A plugin editor window beside them keeps the frames going.
        assert!(!all_minimized([true, true], true));
        // No window at all is not minimized.
        assert!(!all_minimized([], false));
    }

    #[test]
    fn a_deadline_earlier_than_the_idle_wake_wins() {
        #[allow(clippy::disallowed_methods)]
        // Justification: a test of the arithmetic on instants; nothing depends on the time.
        let now = Instant::now();
        let idle = now + IDLE_STREAM_WAKE;
        let soon = now + Duration::from_millis(30);
        let late = now + Duration::from_secs(5);
        assert_eq!(earliest(Some(idle), [Some(late), None]), Some(idle));
        assert_eq!(earliest(Some(idle), [Some(late), Some(soon)]), Some(soon));
        // The deadline alone sets the wake when the loop would otherwise sleep.
        assert_eq!(earliest(None, [None, Some(late)]), Some(late));
        assert_eq!(earliest(None, [None, None]), None);
    }

    #[test]
    fn only_a_lone_window_waits_for_the_vertical_blank() {
        assert!(presents_with_vsync(1));
        assert!(!presents_with_vsync(2));
        assert!(!presents_with_vsync(3));
    }

    #[test]
    fn monitor_info_physical_keeps_pixels_and_scale() {
        let m = monitor_info(
            Some("DP-1".into()),
            (-1920, 0),
            (1920, 1080),
            1.5,
            DesktopSpace::Physical,
        );
        assert_eq!((m.x, m.y, m.width, m.height), (-1920, 0, 1920, 1080));
        assert_eq!(m.units_per_logical, 1.5);
        assert_eq!(m.name, "DP-1");
    }

    #[test]
    fn monitor_info_points_divides_by_the_monitor_scale() {
        let m = monitor_info(None, (2880, 0), (2880, 1800), 2.0, DesktopSpace::Points);
        assert_eq!((m.x, m.y, m.width, m.height), (1440, 0, 1440, 900));
        assert_eq!(m.units_per_logical, 1.0);
        assert_eq!(m.name, "monitor");
    }

    #[test]
    fn monitor_info_bad_scale_counts_as_one() {
        let m = monitor_info(None, (0, 0), (800, 600), 0.0, DesktopSpace::Points);
        assert_eq!((m.width, m.height), (800, 600));
    }

    #[test]
    fn geom_of_physical_stores_outer_pixels_and_logical_size() {
        let g = geom_of(
            DesktopSpace::Physical,
            Some((100, 50)),
            (1920, 1080),
            1.5,
            false,
            Some("DP-1".into()),
        )
        .unwrap();
        assert_eq!((g.x, g.y), (100, 50));
        assert_eq!((g.width, g.height), (1280.0, 720.0));
        assert_eq!(g.monitor.as_deref(), Some("DP-1"));
    }

    #[test]
    fn geom_of_points_stores_points() {
        let g = geom_of(
            DesktopSpace::Points,
            Some((200, 100)),
            (2560, 1440),
            2.0,
            true,
            None,
        )
        .unwrap();
        assert_eq!((g.x, g.y), (100, 50));
        assert_eq!((g.width, g.height), (1280.0, 720.0));
        assert!(g.maximised);
    }

    #[test]
    fn geom_of_no_position_is_the_origin() {
        let g = geom_of(DesktopSpace::Physical, None, (800, 600), 1.0, false, None).unwrap();
        assert_eq!((g.x, g.y), (0, 0));
    }

    #[test]
    fn geom_of_empty_window_is_none() {
        let empty = |size| geom_of(DesktopSpace::Physical, Some((0, 0)), size, 1.0, false, None);
        assert!(empty((0, 0)).is_none());
        assert!(empty((10, 0)).is_none());
    }

    #[test]
    fn window_rect_points_scales_by_the_window() {
        let r = window_rect(
            WindowKind::Rack,
            (400, 200),
            (2000, 1000),
            2.0,
            DesktopSpace::Points,
        );
        assert_eq!((r.x, r.y, r.width, r.height), (200.0, 100.0, 1000.0, 500.0));
        assert_eq!(r.scale, 2.0);
    }

    #[test]
    fn window_rect_feeds_window_under_across_two_scales() {
        // A at 100% on the left, B at 150% on the right, physical desktop.
        let rects = [
            window_rect(
                WindowKind::Main,
                (0, 0),
                (1000, 800),
                1.0,
                DesktopSpace::Physical,
            ),
            window_rect(
                WindowKind::Rack,
                (1000, 0),
                (1500, 900),
                1.5,
                DesktopSpace::Physical,
            ),
        ];
        let desktop = to_desktop(DesktopSpace::Physical, (0.0, 0.0), 1.0, (1300.0, 300.0));
        let hit = window_under(&rects, desktop, &ZOrder::default()).unwrap();
        assert_eq!(hit.kind, WindowKind::Rack);
        assert_eq!(hit.logical, (200.0, 200.0));
    }

    #[test]
    fn position_for_physical_and_points() {
        assert_eq!(
            position_for(DesktopSpace::Physical, (-5, 7)),
            Position::Physical(PhysicalPosition::new(-5, 7))
        );
        assert_eq!(
            position_for(DesktopSpace::Points, (-5, 7)),
            Position::Logical(LogicalPosition::new(-5.0, 7.0))
        );
    }

    #[test]
    fn local_hit_inside_is_the_window_outside_is_none() {
        let k = WindowKind::Console;
        let hit = local_hit(k, Point::new(10.0, 20.0), (100.0, 50.0)).unwrap();
        assert_eq!((hit.kind, hit.logical), (k, (10.0, 20.0)));
        assert!(local_hit(k, Point::new(100.0, 20.0), (100.0, 50.0)).is_none());
        assert!(local_hit(k, Point::new(-1.0, 20.0), (100.0, 50.0)).is_none());
        assert!(local_hit(k, Point::new(5.0, 50.0), (100.0, 50.0)).is_none());
    }

    #[test]
    fn pointer_of_reads_pointer_events_only() {
        use atomik_ui::Modifiers;
        let p = Point::new(3.0, 4.0);
        let ev = UiEvent::PointerMove {
            pos: p,
            modifiers: Modifiers::NONE,
        };
        assert_eq!(pointer_of(&ev), Some(p));
        assert_eq!(pointer_of(&UiEvent::PointerLeave), None);
    }

    #[test]
    fn screenshot_path_main_is_the_base_others_get_a_suffix() {
        let base = Path::new("out/shot.png");
        assert_eq!(screenshot_path(base, WindowKind::Main), base);
        assert_eq!(
            screenshot_path(base, WindowKind::Rack),
            Path::new("out/shot-rack.png")
        );
        assert_eq!(
            screenshot_path(Path::new("shot"), WindowKind::Console),
            Path::new("shot-console")
        );
    }

    #[test]
    fn changed_is_some_only_for_a_different_value() {
        assert_eq!(changed(&"a".to_string(), &"a".to_string()), None);
        assert_eq!(
            changed(&"a".to_string(), &"b".to_string()),
            Some("b".to_string())
        );
    }
}
