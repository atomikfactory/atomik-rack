//! The decisions of the platform's window code that need no window: the units of the desktop,
//! the monitor and geometry conversions, and where a pointer is during a drag.
//!
//! `app.rs` converts winit's types at the edge and calls these; everything here is plain data
//! and unit-tested. See [`crate::windows`] for the model they feed.

use crate::windows::{DesktopSpace, MonitorInfo, WindowGeom, WindowHit, WindowKind, WindowRect};
use atomik_ui::UiEvent;
use atomik_ui::kurbo::Point;
use std::path::{Path, PathBuf};
use std::time::Duration;
use winit::dpi::{LogicalPosition, PhysicalPosition, Position};

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

/// The file a window's screenshot goes to: `base` for the main window, `base` with `-rack` or
/// `-console` before the extension for the others.
pub(super) fn screenshot_path(base: &Path, kind: WindowKind) -> PathBuf {
    let suffix = match kind {
        WindowKind::Main => return base.to_path_buf(),
        WindowKind::Rack => "rack",
        WindowKind::Console => "console",
        WindowKind::SpectrumEq => "spectrum-eq",
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

/// How often the loop looks at the plugins (their main-thread work, parameter notifications,
/// dirty flags) while nothing else asks for frames.
pub(super) const PLUGIN_IDLE_POLL: Duration = Duration::from_millis(100);

/// What keeps the event loop awake between events.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct Wakers {
    /// Audio runs, a worker is busy, a menu or tooltip is about to open, the rack auto-scrolls,
    /// a plugin is being created, or a plugin editor is open: frames at the full rate.
    pub frames: bool,
    /// A plugin instance is loaded: its notifications are polled.
    pub plugins: bool,
}

/// How long the loop may sleep: a frame while something animates, the plugin poll period while
/// only loaded plugins need looking at, forever (until an event) otherwise.
pub(super) fn wake_after(w: Wakers, frame: Duration) -> Option<Duration> {
    if w.frames {
        Some(frame)
    } else if w.plugins {
        Some(PLUGIN_IDLE_POLL)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows::{ZOrder, to_desktop, window_under};

    #[test]
    fn the_window_icon_is_built_from_the_embedded_logo() {
        assert!(window_icon().is_some());
    }

    #[test]
    fn an_idle_project_with_a_plugin_polls_at_ten_hertz_and_one_without_sleeps_on() {
        let frame = Duration::from_micros(16_667);
        assert_eq!(wake_after(Wakers::default(), frame), None);
        let plugins = Wakers {
            plugins: true,
            ..Wakers::default()
        };
        assert_eq!(wake_after(plugins, frame), Some(Duration::from_millis(100)));
        let busy = Wakers {
            frames: true,
            plugins: true,
        };
        assert_eq!(wake_after(busy, frame), Some(frame));
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
