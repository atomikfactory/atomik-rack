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

/// Whether a window is made hidden and shown after its first frame. Only where a hidden window
/// still gets its redraw requests (Windows): elsewhere it is shown at once, as before.
pub(super) const HIDE_UNTIL_FIRST_FRAME: bool = cfg!(windows);

/// Whether a window made hidden is shown after a frame with this `result`: after any frame
/// that was tried, except the one that only reconfigured the surface (the redraw it asks for
/// comes right after). A frame that was skipped or lost shows the window too: it must never
/// stay hidden.
pub(super) fn reveals_window(result: &super::gpu::FrameResult) -> bool {
    !matches!(result, super::gpu::FrameResult::Reconfigured)
}

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
///
/// At Low power no window waits for the vertical blank: a lone window too is held to the frame
/// slot, whose period is 30 Hz there, which a presentation in step with a 60 Hz monitor could
/// not do for the paints that input asks for.
pub(super) fn presents_with_vsync(open_windows: usize, low_power: bool) -> bool {
    open_windows <= 1 && !low_power
}

/// How often the loop runs its housekeeping frame while an audio stream is open and nothing
/// moves (see [`Wakers::stream`]).
pub(super) const IDLE_STREAM_WAKE: Duration = Duration::from_millis(100);

/// How often the loop wakes while every window is minimized and nothing animates: nothing is
/// painted, only the housekeeping runs. Something that animates (the transport plays, a glide runs)
/// keeps the frame period even then, because its timed work runs on the frame.
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
/// and [`MINIMIZED_WAKE`] instead of the idle rate while every window is minimized (the paint tail does
/// not apply then: nothing is painted). A minimized app with something animating keeps the frame
/// period and paints nothing. With none of them the loop sleeps until an event. The tail only applies where a housekeeping wake exists anyway: a
/// project with no stream and no plugin has no frames to keep going.
pub(super) fn wake_after(w: Wakers, frame: Duration) -> Option<Duration> {
    let housekeeping = w.stream || w.plugins;
    let tail = housekeeping && w.since_paint.is_some_and(|ago| ago < PAINT_TAIL);
    if w.minimized {
        return if w.animating {
            Some(frame)
        } else {
            housekeeping.then_some(MINIMIZED_WAKE)
        };
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

/// What the frame cap says about a paint that is wanted now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PaintGate {
    /// Paint now. `opens_slot`: this paint starts a new frame slot (see [`paint_gate`]).
    Now { opens_slot: bool },
    /// Paint later: the rest of the frame slot.
    Wait(Duration),
}

/// Whether a paint for `reason` may be held back by the frame cap. The person's input, a view
/// that keeps itself painting and a change something marked can wait for the end of the frame
/// slot (the paint then shows the latest state). A resize or scale change must not show
/// stale or stretched content, a focus change or an uncovered window is an exposure the system
/// is waiting on, and a frame that was painted but not shown is a retry: none of them waits.
pub(super) fn paint_deferrable(reason: PaintReason) -> bool {
    match reason {
        PaintReason::Input | PaintReason::Live | PaintReason::Update => true,
        PaintReason::Resize | PaintReason::Focus | PaintReason::Surface => false,
    }
}

/// How early a paint that arrives by a timer still counts as due: an eighth of the period. The
/// animation tick is a timer of one period and the timer itself rounds to milliseconds; a tick
/// that came a hair early must start the next slot, not fall to every other tick. The person's
/// input is not a timer and gets no tolerance, so a sweep of the pointer paints a window at
/// most once per period.
pub(super) fn frame_tolerance(period: Duration) -> Duration {
    period / 8
}

/// Whether a window is painted now or waits.
///
/// The windows that present without waiting share ONE frame phase, so that they are all painted
/// by the same wake-up of the loop. Time is cut into slots of `period`; the slot began
/// `since_slot` ago (`None` before the first). A window that presents with vsync is paced by
/// its present call and is never held back (a timer beating against the vertical blank would
/// halve the visible rate). For the others:
/// - a paint that [`paint_deferrable`] does not allow is made at once and does not move the
///   slot (a resize does not push the animation phase around);
/// - a window not yet painted in the current slot (`painted_in_slot` false) is painted at
///   once, in that slot;
/// - a window already painted in it waits for the slot to end, and when it has (within the
///   tolerance, none for input) the paint is made and opens the next slot.
pub(super) fn paint_gate(
    vsync: bool,
    since_slot: Option<Duration>,
    painted_in_slot: bool,
    period: Duration,
    reason: PaintReason,
) -> PaintGate {
    if vsync || !paint_deferrable(reason) {
        return PaintGate::Now { opens_slot: false };
    }
    let Some(since) = since_slot else {
        return PaintGate::Now { opens_slot: true };
    };
    if !painted_in_slot {
        return PaintGate::Now { opens_slot: false };
    }
    let slack = if reason == PaintReason::Input {
        Duration::ZERO
    } else {
        frame_tolerance(period)
    };
    if since + slack >= period {
        PaintGate::Now { opens_slot: true }
    } else {
        PaintGate::Wait(period - since)
    }
}

/// When the loop wakes next: its own wake (`wake`, from [`wake_after`], counted from `now`) or
/// the time a held-back paint is due (`held_until`), whichever is earlier. It holds with no
/// other waker: a project with no stream and no plugin still wakes for a window that waits.
pub(super) fn loop_deadline(
    now: Instant,
    wake: Option<Duration>,
    held_until: Option<Instant>,
) -> Option<Instant> {
    earliest(wake.map(|d| now + d), [held_until])
}

/// The earlier of two optional times: the due time of the paint that waits soonest.
pub(super) fn sooner(a: Option<Instant>, b: Instant) -> Option<Instant> {
    Some(a.map_or(b, |a| a.min(b)))
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
    fn minimized_and_idle_wakes_every_quarter_second_minimized_and_animating_keeps_the_frame_period_and_nothing_paints()
     {
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
        // The tail of a paint does not apply while minimized (these have since_paint zero).
        // Something that animates keeps the frame period, minimized or not.
        assert_eq!(wake_after(min(true, true, false), FRAME), Some(FRAME));
        assert_eq!(wake_after(min(true, false, false), FRAME), Some(FRAME));
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
    fn a_hidden_window_is_shown_after_any_frame_but_a_reconfigure() {
        use super::super::gpu::FrameResult;
        assert!(reveals_window(&FrameResult::Presented));
        // A frame that was skipped or lost must not leave the window hidden.
        assert!(reveals_window(&FrameResult::Skipped));
        assert!(reveals_window(&FrameResult::Lost));
        // The reconfigure asks for a redraw at once; the window is shown after that one.
        assert!(!reveals_window(&FrameResult::Reconfigured));
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
        assert!(presents_with_vsync(1, false));
        assert!(!presents_with_vsync(2, false));
        assert!(!presents_with_vsync(3, false));
    }

    #[test]
    fn at_low_power_not_even_a_lone_window_waits_for_the_vertical_blank() {
        for open in 1..=3 {
            assert!(!presents_with_vsync(open, true), "{open}");
        }
    }

    #[test]
    fn the_frame_period_of_each_performance_level_is_the_slot_the_cap_uses() {
        use crate::performance::Level;
        let periods: Vec<Duration> = [Level::Full, Level::Low].map(Level::frame_period).into();
        assert_eq!(periods[0], FRAME);
        assert_eq!(periods[1], Duration::from_micros(33_333));
        for period in periods {
            // A window already painted in the slot waits for the rest of it, and the next slot
            // opens when it is over: at 30 Hz as at 60 Hz.
            let since = 5 * MS;
            assert_eq!(
                paint_gate(false, Some(since), true, period, PaintReason::Input),
                PaintGate::Wait(period - since)
            );
            assert_eq!(
                paint_gate(false, Some(period), true, period, PaintReason::Input),
                OPENS
            );
            // The loop wakes once per period while something animates.
            let animating = Wakers {
                animating: true,
                ..wakers()
            };
            assert_eq!(wake_after(animating, period), Some(period));
            let tol = frame_tolerance(period);
            assert!(tol >= MS && tol < period / 4, "{tol:?}");
        }
    }

    const MS: Duration = Duration::from_millis(1);

    fn gate(
        vsync: bool,
        since_slot: Option<Duration>,
        painted: bool,
        reason: PaintReason,
    ) -> PaintGate {
        paint_gate(vsync, since_slot, painted, FRAME, reason)
    }

    const OPENS: PaintGate = PaintGate::Now { opens_slot: true };
    const JOINS: PaintGate = PaintGate::Now { opens_slot: false };

    #[test]
    fn the_first_paint_opens_the_first_slot_and_a_window_not_yet_painted_in_a_slot_joins_it() {
        for reason in [PaintReason::Input, PaintReason::Live, PaintReason::Update] {
            assert_eq!(gate(false, None, false, reason), OPENS, "{reason:?}");
            // A window that has not been painted in this slot is painted now, in it.
            for ago in [Duration::ZERO, 5 * MS, FRAME - MS] {
                assert_eq!(gate(false, Some(ago), false, reason), JOINS, "{reason:?}");
            }
        }
    }

    #[test]
    fn a_window_already_painted_in_the_slot_waits_for_its_end() {
        for reason in [PaintReason::Input, PaintReason::Live, PaintReason::Update] {
            assert_eq!(
                gate(false, Some(Duration::ZERO), true, reason),
                PaintGate::Wait(FRAME),
                "{reason:?}"
            );
            // The wait is exactly the rest of the slot.
            for ago in [1u64, 4_000, 10_000, 14_000] {
                let since = Duration::from_micros(ago);
                assert_eq!(
                    gate(false, Some(since), true, PaintReason::Input),
                    PaintGate::Wait(FRAME - since),
                    "{ago}"
                );
            }
        }
        // At the end of the slot and after it: due, and it opens the next slot.
        assert_eq!(gate(false, Some(FRAME), true, PaintReason::Input), OPENS);
        assert_eq!(
            gate(false, Some(FRAME + MS), true, PaintReason::Input),
            OPENS
        );
    }

    #[test]
    fn the_paints_the_system_waits_for_are_never_held_and_do_not_move_the_slot() {
        for reason in [
            PaintReason::Resize,
            PaintReason::Focus,
            PaintReason::Surface,
        ] {
            assert!(!paint_deferrable(reason));
            for slot in [None, Some(Duration::ZERO), Some(FRAME)] {
                for painted in [false, true] {
                    assert_eq!(gate(false, slot, painted, reason), JOINS, "{reason:?}");
                }
            }
        }
    }

    #[test]
    fn a_window_that_presents_with_vsync_is_never_held_back() {
        for reason in PaintReason::ALL {
            for ago in [None, Some(Duration::ZERO), Some(8 * MS)] {
                assert_eq!(gate(true, ago, true, reason), JOINS, "{reason:?}");
            }
        }
    }

    #[test]
    fn a_tick_that_comes_a_hair_early_starts_the_next_slot_but_input_is_held_to_the_period() {
        let tol = frame_tolerance(FRAME);
        assert!(tol >= MS && tol < FRAME / 4, "{tol:?}");
        for reason in [PaintReason::Live, PaintReason::Update] {
            for early in [Duration::ZERO, MS, tol] {
                assert_eq!(gate(false, Some(FRAME - early), true, reason), OPENS);
            }
            let since = FRAME - tol - MS;
            assert_eq!(
                gate(false, Some(since), true, reason),
                PaintGate::Wait(tol + MS)
            );
        }
        assert_eq!(
            gate(false, Some(FRAME - MS), true, PaintReason::Input),
            PaintGate::Wait(MS)
        );
    }

    /// The loop on the pure functions, with `n` windows that present without waiting. Time is a
    /// `Duration` since the start. Mirrors the platform: a window is painted by an event's
    /// redraw (`redraw`) or by a wake-up of the loop (`tick`, which also recomputes the loop's
    /// next wake from the tick's one `now`). With `per_window` it follows the first version of
    /// the cap, where each window was gated against its own last paint, to show what that did.
    struct Sim {
        vsync: bool,
        per_window: bool,
        /// The event's reason also applies to the paints of the tick that follows it, as the
        /// platform's `event_cause` does; off for an exposure the system asks for with no event.
        event_sets_cause: bool,
        animating: bool,
        slot: Option<Duration>,
        last: Vec<Option<Duration>>,
        marked: Vec<bool>,
        held: Option<Duration>,
        next_tick: Option<Duration>,
        cause: PaintReason,
        paints: Vec<Vec<Duration>>,
        wakes: Vec<Duration>,
    }

    impl Sim {
        fn new(n: usize, vsync: bool, animating: bool) -> Self {
            Sim {
                vsync,
                per_window: false,
                event_sets_cause: true,
                animating,
                slot: None,
                last: vec![None; n],
                marked: vec![false; n],
                held: None,
                next_tick: None,
                cause: PaintReason::Live,
                paints: vec![Vec::new(); n],
                wakes: Vec::new(),
            }
        }

        fn redraw(&mut self, w: usize, now: Duration, reason: PaintReason) {
            if !self.marked[w] {
                return;
            }
            let decision = if self.per_window {
                // The first version: the time since this window's own last paint.
                let since = self.last[w].map(|l| now - l);
                match (self.vsync, since) {
                    (true, _) | (_, None) => JOINS,
                    _ if !paint_deferrable(reason) => JOINS,
                    (_, Some(s)) => {
                        let slack = if reason == PaintReason::Input {
                            Duration::ZERO
                        } else {
                            frame_tolerance(FRAME)
                        };
                        if s + slack >= FRAME {
                            JOINS
                        } else {
                            PaintGate::Wait(FRAME - s)
                        }
                    }
                }
            } else {
                let painted = matches!((self.last[w], self.slot), (Some(l), Some(s)) if l >= s);
                paint_gate(
                    self.vsync,
                    self.slot.map(|s| now - s),
                    painted,
                    FRAME,
                    reason,
                )
            };
            match decision {
                PaintGate::Now { opens_slot } => {
                    if opens_slot {
                        self.slot = Some(now);
                    }
                    self.last[w] = Some(now);
                    self.marked[w] = false;
                    self.paints[w].push(now);
                }
                PaintGate::Wait(d) => {
                    self.held = Some(self.held.map_or(now + d, |h| h.min(now + d)));
                }
            }
        }

        fn tick(&mut self, now: Duration) {
            self.wakes.push(now);
            if self.animating {
                self.marked.iter_mut().for_each(|m| *m = true);
            }
            self.held = None;
            for w in 0..self.marked.len() {
                self.redraw(w, now, self.cause);
            }
            // The cause of the paints is spent unless something was held back.
            if self.held.is_none() {
                self.cause = PaintReason::Live;
            }
            self.next_tick = self.animating.then_some(now + FRAME);
        }

        /// Runs to `until`; `events` are (time, window, reason), in order.
        fn run(&mut self, until: Duration, events: &[(Duration, usize, PaintReason)]) {
            if self.wakes.is_empty() {
                self.tick(Duration::ZERO);
            }
            let mut events = events.iter().peekable();
            loop {
                let timer = [self.next_tick, self.held].into_iter().flatten().min();
                let event = events.peek().map(|e| e.0);
                let (at, is_event) = match (timer, event) {
                    (Some(t), Some(e)) if e <= t => (e, true),
                    (Some(t), _) => (t, false),
                    (None, Some(e)) => (e, true),
                    (None, None) => return,
                };
                if at > until {
                    return;
                }
                if is_event {
                    let &(_, w, reason) = events.next().unwrap();
                    self.marked[w] = true;
                    if self.event_sets_cause {
                        self.cause = reason;
                    }
                    self.redraw(w, at, reason);
                }
                self.tick(at);
            }
        }

        fn wakes_in(&self, from: Duration, to: Duration) -> usize {
            self.wakes
                .iter()
                .filter(|t| **t >= from && **t < to)
                .count()
        }

        fn paints_in(&self, w: usize, from: Duration, to: Duration) -> usize {
            self.paints[w]
                .iter()
                .filter(|t| **t >= from && **t < to)
                .count()
        }
    }

    const SECOND: Duration = Duration::from_secs(1);

    #[test]
    fn steady_animating_windows_share_one_wake_up_per_period_and_all_paint_on_it() {
        for n in [1, 2, 3] {
            let mut sim = Sim::new(n, false, true);
            sim.run(SECOND * 2, &[]);
            let wakes = sim.wakes_in(SECOND, SECOND * 2);
            assert!((59..=61).contains(&wakes), "{n} windows: {wakes} wake-ups");
            for w in 0..n {
                assert_eq!(
                    sim.paints_in(w, SECOND, SECOND * 2),
                    wakes,
                    "window {w} of {n} is painted on every wake-up"
                );
            }
        }
    }

    /// One event at `offset` after a tick, then a second counted after two periods.
    fn after_a_perturbation(
        n: usize,
        window: usize,
        offset: Duration,
        reason: PaintReason,
        per_window: bool,
        event_sets_cause: bool,
    ) -> Sim {
        let mut sim = Sim::new(n, false, true);
        sim.per_window = per_window;
        sim.event_sets_cause = event_sets_cause;
        let at = FRAME * 30 + offset;
        sim.run(SECOND * 4, &[(at, window, reason)]);
        sim
    }

    fn assert_back_to_one_wake_up(sim: &Sim, n: usize, label: &str) {
        // Two periods after the event (at 30 periods + offset), a whole second.
        let from = FRAME * 33;
        let to = from + SECOND;
        let wakes = sim.wakes_in(from, to);
        assert!(
            (59..=61).contains(&wakes),
            "{label}: {wakes} wake-ups a second"
        );
        for w in 0..n {
            let paints = sim.paints_in(w, from, to);
            assert!(
                (55..=61).contains(&paints),
                "{label}: window {w} has {paints} paints a second"
            );
        }
    }

    #[test]
    fn a_paint_off_the_tick_does_not_leave_the_windows_in_two_phases() {
        for n in [2, 3] {
            for ms in [1u32, 5, 8, 12, 15] {
                for reason in [PaintReason::Input, PaintReason::Resize] {
                    for (window, cause) in (0..n).flat_map(|w| [(w, true), (w, false)]) {
                        let sim = after_a_perturbation(n, window, MS * ms, reason, false, cause);
                        let label = format!(
                            "{n} windows, {reason:?} of {window} at +{ms} ms, event cause {cause}"
                        );
                        assert_back_to_one_wake_up(&sim, n, &label);
                        // Never more than two wake-ups close together, whatever the event.
                        let from = FRAME * 33;
                        for pair in sim.wakes.windows(2).filter(|p| p[0] >= from) {
                            assert!(
                                pair[1] - pair[0] + frame_tolerance(FRAME) >= FRAME,
                                "{label}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn with_a_phase_per_window_the_loop_would_wake_twice_a_frame() {
        // What the first version of the cap did (each window gated on its own last paint): a
        // paint that is never held (an exposure, a resize) 8 ms after a tick leaves the two
        // windows half a period apart for good, and the loop wakes for each of them.
        let sim = after_a_perturbation(2, 0, MS * 8, PaintReason::Resize, true, false);
        let wakes = sim.wakes_in(FRAME * 33, FRAME * 33 + SECOND);
        assert!(
            wakes >= 110,
            "{wakes} wake-ups a second with a phase per window"
        );
    }

    #[test]
    fn a_pointer_sweep_paints_once_per_slot_and_the_last_state_is_painted() {
        let events: Vec<_> = (0..1000u32)
            .map(|i| (MS * i, 0, PaintReason::Input))
            .collect();
        let mut sim = Sim::new(1, false, false);
        sim.run(SECOND + FRAME * 4, &events);
        let paints = &sim.paints[0];
        let in_sweep = sim.paints_in(0, Duration::ZERO, SECOND);
        assert!(
            in_sweep as u128 <= SECOND.as_micros() / FRAME.as_micros() + 1,
            "{in_sweep} paints in the sweep"
        );
        assert!(
            in_sweep >= 55,
            "{in_sweep}: the cap must not starve the sweep"
        );
        // The last event (999 ms) is painted within one period, with no further event.
        let last = *paints.last().unwrap();
        assert!(last >= MS * 999 && last <= MS * 999 + FRAME, "{last:?}");
        // Slots are at least a period apart: no window paints twice in one.
        for w in paints.windows(2) {
            assert!(w[1] - w[0] >= FRAME, "{w:?}");
        }
    }

    #[test]
    fn a_sweep_over_one_of_two_animating_windows_caps_it_and_keeps_the_other_at_the_tick() {
        let events: Vec<_> = (0..1000u32)
            .map(|i| (MS * i + MS * 3, 0, PaintReason::Input))
            .collect();
        let mut sim = Sim::new(2, false, true);
        sim.run(SECOND + FRAME * 4, &events);
        for w in 0..2 {
            let paints = sim.paints_in(w, Duration::ZERO, SECOND);
            assert!((55..=62).contains(&paints), "window {w}: {paints} paints");
        }
        // Back to one wake-up per period after the sweep.
        let wakes = sim.wakes_in(SECOND + FRAME * 2, SECOND + FRAME * 4);
        assert!(wakes <= 3, "{wakes}");
    }

    #[test]
    fn the_same_sweep_on_a_window_with_vsync_paints_on_every_event() {
        let events: Vec<_> = (0..1000u32)
            .map(|i| (MS * i, 0, PaintReason::Input))
            .collect();
        let mut sim = Sim::new(1, true, false);
        sim.run(SECOND + FRAME * 4, &events);
        assert_eq!(sim.paints[0].len(), 1000);
    }

    #[test]
    fn steady_ticks_with_jitter_paint_on_every_tick() {
        let tol = frame_tolerance(FRAME);
        for jitter in [Duration::ZERO, MS, tol] {
            let mut slot: Option<Duration> = None;
            let mut painted = 0;
            for tick in 0..120u32 {
                // Every second tick comes early by the jitter.
                let early = if tick % 2 == 1 {
                    jitter
                } else {
                    Duration::ZERO
                };
                let t = FRAME * tick - early;
                let in_slot = slot.is_some();
                let decision = gate(false, slot.map(|s| t - s), in_slot, PaintReason::Live);
                if let PaintGate::Now { opens_slot } = decision {
                    painted += 1;
                    if opens_slot {
                        slot = Some(t);
                    }
                }
            }
            assert_eq!(painted, 120, "jitter {jitter:?}");
        }
    }

    #[test]
    fn one_window_keeps_its_deadlines_and_a_waiting_window_adds_one() {
        #[allow(clippy::disallowed_methods)]
        // Justification: a test of the arithmetic on instants; nothing depends on the time.
        let now = Instant::now();
        // Nothing waits: the deadline is the loop's own, for every combination of wakers.
        for bits in 0..16u32 {
            for since in [None, Some(Duration::ZERO), Some(Duration::from_secs(5))] {
                let w = Wakers {
                    animating: bits & 1 != 0,
                    stream: bits & 2 != 0,
                    plugins: bits & 4 != 0,
                    minimized: bits & 8 != 0,
                    since_paint: since,
                };
                assert_eq!(
                    loop_deadline(now, wake_after(w, FRAME), None),
                    wake_after(w, FRAME).map(|d| now + d),
                    "{w:?}"
                );
            }
        }
        // A waiting window wakes the loop when nothing else would.
        let due = now + Duration::from_millis(9);
        assert_eq!(loop_deadline(now, None, Some(due)), Some(due));
        // An earlier wake of the loop wins, a later one does not.
        assert_eq!(
            loop_deadline(now, Some(Duration::from_millis(3)), Some(due)),
            Some(now + Duration::from_millis(3))
        );
        assert_eq!(
            loop_deadline(now, Some(IDLE_STREAM_WAKE), Some(due)),
            Some(due)
        );
        // The soonest of several waiting windows is kept.
        let first = sooner(None, now + Duration::from_millis(12));
        let both = sooner(first, due);
        assert_eq!(both, Some(due));
        assert_eq!(sooner(both, now + Duration::from_millis(15)), Some(due));
    }

    #[test]
    fn with_nothing_animating_a_held_paint_still_wakes_the_loop() {
        // No stream, no plugin, no animation: only the held paint wakes the loop, and the last
        // state is painted one period after the first paint of the burst.
        let events = [
            (Duration::ZERO, 0, PaintReason::Input),
            (MS * 3, 0, PaintReason::Input),
        ];
        let mut sim = Sim::new(2, false, false);
        sim.run(SECOND, &events);
        assert_eq!(sim.paints[0], vec![Duration::ZERO, FRAME]);
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
