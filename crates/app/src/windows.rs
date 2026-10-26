//! The windows of the app as pure data: which exist, where they sit, and how a pointer that
//! leaves one window finds the next.
//!
//! The app has a main window and two panes that can leave it: the rack and the console (the
//! mixer). Nothing here touches winit or the disk; the platform layer converts its own types
//! at the edge and the shell stores the results in [`WindowsState`] (kept in `ui-state.json`,
//! see [`crate::ui_state`]).
//!
//! - [`WindowKind`], [`WindowGeom`], [`DetachState`], [`WindowsState`]: what is saved.
//! - [`place`]: where a window opens, given the monitors of the moment ([`MonitorInfo`]).
//! - [`to_desktop`], [`window_under`], [`ZOrder`]: finding the window under a pointer during a
//!   drag that started in another window (the OS keeps sending the events to the first one).
//! - [`reconcile`]: which windows to create and which to close.
//!
//! Desktop units are physical pixels on Windows and X11 and points on macOS ([`DesktopSpace`]);
//! a [`WindowGeom`] position and a [`MonitorInfo`] rect are in them, a window size is logical.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// The margin kept free around a window that is shrunk to fit its monitor, in logical units.
const FIT_MARGIN: f64 = 32.0;
/// How much of the title strip must show for a saved position to be kept: width, then height,
/// in desktop units.
const STRIP: (f64, f64) = (64.0, 32.0);

/// One of the windows of the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowKind {
    /// The main window; it always exists and closing it quits.
    Main,
    /// The rack, while detached.
    Rack,
    /// The console (mixer), while detached.
    Console,
}

/// Where a window was last, in the units the platform reports.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowGeom {
    /// The outer position, in desktop units (see [`DesktopSpace`]). May be negative on a
    /// monitor left of or above the primary.
    pub x: i32,
    /// See [`Self::x`].
    pub y: i32,
    /// The inner width in logical units.
    pub width: f64,
    /// The inner height in logical units.
    pub height: f64,
    /// The window was maximised. Position and size then hold the restored bounds: the
    /// platform stops updating them while maximised.
    pub maximised: bool,
    /// The name of the monitor it was on, to find it again.
    pub monitor: Option<String>,
}

impl Default for WindowGeom {
    fn default() -> Self {
        Self {
            x: 0,
            y: 0,
            width: 0.0,
            height: 0.0,
            maximised: false,
            monitor: None,
        }
    }
}

impl WindowGeom {
    /// The size is finite and positive.
    pub fn usable(&self) -> bool {
        self.width.is_finite() && self.height.is_finite() && self.width > 0.0 && self.height > 0.0
    }
}

/// A pane that can leave the main window: whether it has, and where its window was.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DetachState {
    /// The pane lives in its own window.
    pub detached: bool,
    /// Where that window was last.
    pub geom: Option<WindowGeom>,
}

/// Every window's saved state: the `windows` key of `ui-state.json`. An absent key means
/// everything attached and no remembered geometry.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowsState {
    /// The main window's last geometry.
    pub main: Option<WindowGeom>,
    /// The rack window.
    pub rack: DetachState,
    /// The console window.
    pub console: DetachState,
}

impl WindowsState {
    /// Whether `kind` is detached. The main window never is.
    pub fn detached(&self, kind: WindowKind) -> bool {
        match kind {
            WindowKind::Main => false,
            WindowKind::Rack => self.rack.detached,
            WindowKind::Console => self.console.detached,
        }
    }

    /// Detaches or attaches `kind`. Ignored for the main window.
    pub fn set_detached(&mut self, kind: WindowKind, detached: bool) {
        match kind {
            WindowKind::Main => {}
            WindowKind::Rack => self.rack.detached = detached,
            WindowKind::Console => self.console.detached = detached,
        }
    }

    /// The saved geometry of `kind`.
    pub fn geom(&self, kind: WindowKind) -> Option<&WindowGeom> {
        match kind {
            WindowKind::Main => self.main.as_ref(),
            WindowKind::Rack => self.rack.geom.as_ref(),
            WindowKind::Console => self.console.geom.as_ref(),
        }
    }

    /// Remembers the geometry of `kind`.
    pub fn set_geom(&mut self, kind: WindowKind, geom: WindowGeom) {
        match kind {
            WindowKind::Main => self.main = Some(geom),
            WindowKind::Rack => self.rack.geom = Some(geom),
            WindowKind::Console => self.console.geom = Some(geom),
        }
    }

    /// The windows that should exist: the main window and every detached pane, in order.
    pub fn wanted(&self) -> Vec<WindowKind> {
        [WindowKind::Main, WindowKind::Rack, WindowKind::Console]
            .into_iter()
            .filter(|&k| k == WindowKind::Main || self.detached(k))
            .collect()
    }

    /// The same state without geometry that cannot be used (a file edited by hand or written
    /// by a broken build): not finite or empty sizes.
    pub fn sanitized(mut self) -> Self {
        let keep = |g: Option<WindowGeom>| g.filter(WindowGeom::usable);
        self.main = keep(self.main);
        self.rack.geom = keep(self.rack.geom);
        self.console.geom = keep(self.console.geom);
        self
    }
}

/// The units of desktop coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopSpace {
    /// Physical pixels (Windows with per-monitor DPI, X11): one space for every monitor.
    Physical,
    /// Points (macOS): a window's physical size is its logical size times its scale, and
    /// positions are already scaled.
    Points,
}

/// A monitor as the platform reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorInfo {
    /// The monitor's name, to match a saved [`WindowGeom::monitor`].
    pub name: String,
    /// The top left corner in desktop units; negative left of or above the primary.
    pub x: i32,
    /// See [`Self::x`].
    pub y: i32,
    /// The width in desktop units. A monitor without area is ignored.
    pub width: u32,
    /// The height in desktop units.
    pub height: u32,
    /// Desktop units per logical unit: the scale factor in [`DesktopSpace::Physical`], 1 in
    /// [`DesktopSpace::Points`].
    pub units_per_logical: f64,
}

impl MonitorInfo {
    fn usable(&self) -> bool {
        self.width > 0 && self.height > 0
    }

    fn unit(&self) -> f64 {
        if self.units_per_logical.is_finite() && self.units_per_logical > 0.0 {
            self.units_per_logical
        } else {
            1.0
        }
    }

    /// The overlap between `self` and the desktop rect `r` (x, y, w, h), as (width, height).
    fn overlap(&self, r: (f64, f64, f64, f64)) -> (f64, f64) {
        let w =
            (f64::from(self.x) + f64::from(self.width)).min(r.0 + r.2) - f64::from(self.x).max(r.0);
        let h = (f64::from(self.y) + f64::from(self.height)).min(r.1 + r.3)
            - f64::from(self.y).max(r.1);
        (w.max(0.0), h.max(0.0))
    }
}

/// Where and how large a window opens.
#[derive(Debug, Clone, PartialEq)]
pub struct Placement {
    /// The outer position in desktop units; `None` leaves it to the OS (no monitor is known,
    /// as under Wayland or headless).
    pub pos: Option<(i32, i32)>,
    /// The inner size in logical units.
    pub size: (f64, f64),
    /// Open maximised; `pos` and `size` are the restored bounds.
    pub maximised: bool,
}

/// Decides where a window opens from its saved geometry and the monitors of the moment.
///
/// The saved monitor is used when it is still there, otherwise the one the window overlaps
/// most, otherwise `primary` (an index into `monitors`; the first usable monitor when it is out
/// of range or has no area). The size is the saved one or `default_size`, shrunk to fit that
/// monitor with a margin but never below `min_size`. A position is kept when at least 64x32
/// units of the title strip stay on some monitor; otherwise the window is centred on the chosen
/// monitor. Without monitors the position is left to the OS.
pub fn place(
    saved: Option<&WindowGeom>,
    default_size: (f64, f64),
    min_size: (f64, f64),
    monitors: &[MonitorInfo],
    primary: usize,
) -> Placement {
    let saved = saved.filter(|g| g.usable());
    let mut size = saved.map_or(default_size, |g| (g.width, g.height));
    let maximised = saved.is_some_and(|g| g.maximised);
    let usable: Vec<&MonitorInfo> = monitors.iter().filter(|m| m.usable()).collect();
    let primary = monitors
        .get(primary)
        .filter(|m| m.usable())
        .or_else(|| usable.first().copied());
    let Some(primary) = primary else {
        return Placement {
            pos: None,
            size: (size.0.max(min_size.0), size.1.max(min_size.1)),
            maximised,
        };
    };

    let target = saved
        .and_then(|g| {
            let by_name = g
                .monitor
                .as_deref()
                .and_then(|n| usable.iter().copied().find(|m| m.name == n));
            by_name.or_else(|| {
                let rect = |m: &MonitorInfo| {
                    let u = m.unit();
                    (f64::from(g.x), f64::from(g.y), g.width * u, g.height * u)
                };
                usable
                    .iter()
                    .copied()
                    .map(|m| {
                        let (w, h) = m.overlap(rect(m));
                        (m, w * h)
                    })
                    .filter(|&(_, area)| area > 0.0)
                    .max_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(m, _)| m)
            })
        })
        .unwrap_or(primary);

    let unit = target.unit();
    let fit = (
        f64::from(target.width) / unit - FIT_MARGIN,
        f64::from(target.height) / unit - FIT_MARGIN,
    );
    size = (
        size.0.min(fit.0).max(min_size.0),
        size.1.min(fit.1).max(min_size.1),
    );

    let (w, h) = (size.0 * unit, size.1 * unit);
    let kept = saved.filter(|g| {
        let strip = (f64::from(g.x), f64::from(g.y), w, STRIP.1);
        usable.iter().any(|m| {
            let (ow, oh) = m.overlap(strip);
            ow >= STRIP.0.min(w) && oh >= STRIP.1
        })
    });
    let pos = match kept {
        Some(g) => (g.x, g.y),
        None => (
            (f64::from(target.x) + (f64::from(target.width) - w) / 2.0).floor() as i32,
            (f64::from(target.y) + (f64::from(target.height) - h) / 2.0).floor() as i32,
        ),
    };
    Placement {
        pos: Some(pos),
        size,
        maximised,
    }
}

/// How far a cascaded window is offset from the main window, in desktop units.
const CASCADE: i32 = 32;

/// Decides where a detached window opens: [`place`] when it has a usable saved geometry, else a
/// monitor the other open app windows (`occupied`, the main window included) are not on.
///
/// The monitors are taken left to right (then top to bottom), starting after the one the main
/// window is on (`primary` when its rect is absent) and wrapping. The first free one gets the
/// window maximised, with the default size centred on it as the restored bounds. When every
/// monitor is used, or there is only one, the window is not maximised: it is cascaded
/// 32 desktop units down and right from the main window, at the default size, clamped onto
/// that monitor. A window is on the monitor that holds the centre of its rect.
pub fn place_detached(
    saved: Option<&WindowGeom>,
    default_size: (f64, f64),
    min_size: (f64, f64),
    monitors: &[MonitorInfo],
    primary: usize,
    occupied: &[WindowRect],
) -> Placement {
    if saved.is_some_and(WindowGeom::usable) {
        return place(saved, default_size, min_size, monitors, primary);
    }
    let mut order: Vec<usize> = (0..monitors.len())
        .filter(|&i| monitors[i].usable())
        .collect();
    order.sort_by_key(|&i| (monitors[i].x, monitors[i].y));
    let holding = |r: &WindowRect| {
        let centre = (r.x + r.width / 2.0, r.y + r.height / 2.0);
        order.iter().copied().find(|&i| {
            let m = &monitors[i];
            centre.0 >= f64::from(m.x)
                && centre.0 < f64::from(m.x) + f64::from(m.width)
                && centre.1 >= f64::from(m.y)
                && centre.1 < f64::from(m.y) + f64::from(m.height)
        })
    };
    let main = occupied.iter().find(|r| r.kind == WindowKind::Main);
    let main_monitor = main
        .and_then(holding)
        .or_else(|| order.contains(&primary).then_some(primary))
        .or_else(|| order.first().copied());
    let Some(main_monitor) = main_monitor else {
        return place(None, default_size, min_size, monitors, primary);
    };
    let used: Vec<usize> = occupied.iter().filter_map(holding).collect();
    let start = order.iter().position(|&i| i == main_monitor).unwrap_or(0);
    let free = (1..order.len())
        .map(|k| order[(start + k) % order.len()])
        .find(|i| !used.contains(i));
    if let Some(free) = free {
        let mut placement = place(None, default_size, min_size, monitors, free);
        placement.maximised = true;
        return placement;
    }

    let mut placement = place(None, default_size, min_size, monitors, main_monitor);
    let m = &monitors[main_monitor];
    let (w, h) = (placement.size.0 * m.unit(), placement.size.1 * m.unit());
    let from = main.map_or((f64::from(m.x), f64::from(m.y)), |r| (r.x, r.y));
    let clamp = |at: f64, origin: i32, extent: u32, size: f64| {
        let max = (f64::from(origin) + f64::from(extent) - size).max(f64::from(origin));
        at.clamp(f64::from(origin), max).floor() as i32
    };
    placement.pos = Some((
        clamp(from.0 + f64::from(CASCADE), m.x, m.width, w),
        clamp(from.1 + f64::from(CASCADE), m.y, m.height, h),
    ));
    placement
}

/// Converts a pointer position local to a window into desktop units.
///
/// `origin` is the window's inner top left in desktop units, `scale` its scale factor and
/// `local_physical` the pointer in physical pixels from that corner (what winit reports). In
/// [`DesktopSpace::Physical`] they add up; in [`DesktopSpace::Points`] the physical offset is
/// divided by this window's own scale. A scale that is zero or not finite counts as 1.
pub fn to_desktop(
    space: DesktopSpace,
    origin: (f64, f64),
    scale: f64,
    local_physical: (f64, f64),
) -> (f64, f64) {
    let k = match space {
        DesktopSpace::Physical => 1.0,
        DesktopSpace::Points => safe_scale(scale),
    };
    (
        origin.0 + local_physical.0 / k,
        origin.1 + local_physical.1 / k,
    )
}

fn safe_scale(scale: f64) -> f64 {
    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

/// A window's inner rect on the desktop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowRect {
    /// Which window.
    pub kind: WindowKind,
    /// The top left in desktop units.
    pub x: f64,
    /// See [`Self::x`].
    pub y: f64,
    /// The inner width in desktop units.
    pub width: f64,
    /// The inner height in desktop units.
    pub height: f64,
    /// The window's scale factor.
    pub scale: f64,
    /// The units of the other fields.
    pub space: DesktopSpace,
}

/// The window under a point and the point in that window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowHit {
    /// The window.
    pub kind: WindowKind,
    /// The point in logical units from the window's top left.
    pub logical: (f64, f64),
}

/// The windows from the front to the back; focusing one moves it to the front.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ZOrder(Vec<WindowKind>);

impl ZOrder {
    /// Moves `kind` to the front.
    pub fn focus(&mut self, kind: WindowKind) {
        self.0.retain(|&k| k != kind);
        self.0.insert(0, kind);
    }

    /// Forgets `kind` (its window closed).
    pub fn remove(&mut self, kind: WindowKind) {
        self.0.retain(|&k| k != kind);
    }

    /// The windows known to the order, front first.
    pub fn front_to_back(&self) -> &[WindowKind] {
        &self.0
    }

    fn rank(&self, kind: WindowKind) -> usize {
        self.0
            .iter()
            .position(|&k| k == kind)
            .unwrap_or(self.0.len())
    }
}

/// The frontmost window whose rect holds the desktop point (edges: left and top inside, right
/// and bottom outside). A window the order does not know sits behind the ones it knows, and
/// ties go to the earlier rect. Windows without area never match.
pub fn window_under(rects: &[WindowRect], point: (f64, f64), order: &ZOrder) -> Option<WindowHit> {
    rects
        .iter()
        .filter(|r| {
            r.width > 0.0
                && r.height > 0.0
                && point.0 >= r.x
                && point.0 < r.x + r.width
                && point.1 >= r.y
                && point.1 < r.y + r.height
        })
        .min_by_key(|r| order.rank(r.kind))
        .map(|r| {
            let k = match r.space {
                DesktopSpace::Physical => safe_scale(r.scale),
                DesktopSpace::Points => 1.0,
            };
            WindowHit {
                kind: r.kind,
                logical: ((point.0 - r.x) / k, (point.1 - r.y) / k),
            }
        })
}

/// What to do to bring the open windows to the wanted ones.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reconcile {
    /// Windows to create, in order.
    pub create: Vec<WindowKind>,
    /// Windows to close, in order. Never the main window.
    pub close: Vec<WindowKind>,
}

/// Compares the windows that are `open` with the `wanted` ones. Duplicates count once and the
/// main window is never closed.
pub fn reconcile(open: &[WindowKind], wanted: &[WindowKind]) -> Reconcile {
    let open: BTreeSet<_> = open.iter().copied().collect();
    let wanted: BTreeSet<_> = wanted.iter().copied().collect();
    Reconcile {
        create: wanted.difference(&open).copied().collect(),
        close: open
            .difference(&wanted)
            .copied()
            .filter(|&k| k != WindowKind::Main)
            .collect(),
    }
}

#[cfg(test)]
#[path = "windows_tests.rs"]
mod tests;
