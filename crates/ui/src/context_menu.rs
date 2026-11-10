//! Context menus built from the command registry.
//!
//! A menu is a list of registry command ids with an enabled flag and an optional check, and
//! submenus of the same. The label comes from the registry and the shortcut text from the active
//! keymap, so a key and a menu item cannot differ. Choosing an item produces a
//! [`MenuOutcome::Run`] with the command id and argument; the shell turns it into a command for
//! the pane the menu belongs to (the same path a key press takes). The widget is pure: it paints
//! through a [`Canvas`], takes [`UiEvent`]s in the coordinates it was opened in and owns no
//! document state.
//!
//! A submenu opens beside its row the moment the pointer is on the row (or presses it) when no
//! other submenu of that panel is open, and stays open while the pointer is over it or the rows
//! that opened it; submenus nest to any depth.
//!
//! While a submenu is open, the pointer may cross the other rows of its panel on the way to the
//! submenu: a move inside the triangle from the previous pointer position to the submenu near
//! edge that gets closer to it keeps the submenu, and any other move onto another row only
//! switches after the short grace delay [`SUBMENU_DELAY`]. The delay is counted by
//! [`ContextMenu::tick`], which the shell calls on every frame the menu asks for
//! ([`ContextMenu::needs_tick`]) and at the deadline [`ContextMenu::wake_at`] names, so the widget
//! reads no clock and the loop need not poll while it waits.
//!
//! A panel taller than the window is cut to it and scrolls (wheel, a pointer resting on the strip
//! at its top or bottom, and the keys that move the highlight); a submenu starts level with its
//! row, moves up before it scrolls and opens on the left of its parent when the right side leaves
//! the window. The geometry is [`crate::menu_layout`], shared with the menu bar.
//!
//! The keys work in and between the panels: Up and Down (wrapping) and Home and End move the one
//! highlight that the pointer moves too, skipping separators and disabled rows; Right or Enter
//! opens a submenu on its first row; Left or Escape closes the submenu; Enter or Space chooses.

use crate::canvas::{Align, Canvas, CanvasExt, TextRun};
use crate::event::{Key, PointerButton, UiEvent};
use crate::keymap::{Keymap, Registry};
use crate::menu_layout::{self as geo, ARROW_SPEED, ListLayout, WHEEL_STEP};
use crate::theme::Theme;
use vello::kurbo::{Point, Rect, RoundedRect, Size, Vec2};
use vello::peniko::Color;

pub use crate::menu_layout::{ITEM_HEIGHT, SEPARATOR_HEIGHT};
/// Approximate advance of one character of UI text at 13 px (layout estimate).
const CHAR_WIDTH: f64 = 7.2;
const MIN_WIDTH: f64 = 200.0;
const FONT: f32 = 13.0;
/// How long the pointer must rest on another row before it replaces the open submenu, seconds:
/// only a switch waits, the first submenu opens at once.
pub const SUBMENU_DELAY: f64 = 0.1;

/// One row of a menu description: a registry command, a submenu or a separator.
#[derive(Debug, Clone, PartialEq)]
pub enum Spec {
    /// A registry command.
    Item {
        /// Registry id (`edit.copy`).
        command: &'static str,
        /// The argument the command is bound with.
        arg: Option<&'static str>,
        /// Another label than the registry's (a track menu says "Duplicate Track").
        label: Option<&'static str>,
        /// Whether it can be chosen.
        enabled: bool,
        /// `Some(checked)` for a check item.
        checked: Option<bool>,
    },
    /// A row that opens a submenu.
    Submenu {
        /// The row's text.
        label: &'static str,
        /// Whether it opens.
        enabled: bool,
        /// The rows of the submenu.
        items: Vec<Spec>,
    },
    /// Rows built by the caller (names and arguments not known until the menu opens, like the
    /// device types of the Create submenu), taken into the menu as they are.
    Rows(Vec<Entry>),
    /// A separator line.
    Separator,
}

impl Spec {
    /// An enabled item of command `command`.
    pub fn item(command: &'static str) -> Self {
        Spec::Item {
            command,
            arg: None,
            label: None,
            enabled: true,
            checked: None,
        }
    }

    /// An enabled submenu row.
    pub fn submenu(label: &'static str, items: Vec<Spec>) -> Self {
        Spec::Submenu {
            label,
            enabled: true,
            items,
        }
    }

    /// The item with its enabled flag set (a submenu row too).
    pub fn enabled(self, on: bool) -> Self {
        match self {
            Spec::Item {
                command,
                arg,
                label,
                checked,
                ..
            } => Spec::Item {
                command,
                arg,
                label,
                enabled: on,
                checked,
            },
            Spec::Submenu { label, items, .. } => Spec::Submenu {
                label,
                enabled: on,
                items,
            },
            other => other,
        }
    }

    /// The item with its own label.
    pub fn labelled(self, text: &'static str) -> Self {
        match self {
            Spec::Item {
                command,
                arg,
                enabled,
                checked,
                ..
            } => Spec::Item {
                command,
                arg,
                label: Some(text),
                enabled,
                checked,
            },
            other => other,
        }
    }
}

/// A menu item as shown.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuItemDef {
    /// Registry command id.
    pub command: String,
    /// The command's argument.
    pub arg: Option<String>,
    /// The label (the registry's, or the spec's own).
    pub label: String,
    /// Shortcut text derived from the keymap and the OS (`Ctrl+C`), `None` when unbound.
    pub shortcut: Option<String>,
    /// Whether it can be chosen.
    pub enabled: bool,
    /// `Some(checked)` for a check item.
    pub checked: Option<bool>,
    /// A colour chip drawn before the label (a track's colour in a list of tracks); the chip
    /// takes the place of the check mark.
    pub swatch: Option<[u8; 3]>,
}

/// A submenu row as shown.
#[derive(Debug, Clone, PartialEq)]
pub struct SubmenuDef {
    /// The row's text.
    pub label: String,
    /// Whether it opens.
    pub enabled: bool,
    /// The rows of the submenu.
    pub entries: Vec<Entry>,
}

/// A row of an open menu.
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    /// An item.
    Item(MenuItemDef),
    /// A row that opens a submenu.
    Submenu(SubmenuDef),
    /// A separator line.
    Separator,
}

impl Entry {
    fn height(&self) -> f64 {
        match self {
            Entry::Separator => SEPARATOR_HEIGHT,
            _ => ITEM_HEIGHT,
        }
    }
}

/// What the menu did with an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuOutcome {
    /// Not for the menu (it is closed, or the event is not a pointer or key event).
    Ignored,
    /// Used by the menu (hover, a click inside, a click outside that closed it).
    Consumed,
    /// An item was chosen; the menu is closed.
    Run {
        /// Registry command id.
        command: String,
        /// The command's argument.
        arg: Option<String>,
    },
}

/// An open context menu: one panel, and the submenu panel open beside it, if any.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextMenu {
    entries: Vec<Entry>,
    origin: Point,
    width: f64,
    /// The panel's height: all the rows when they fit, else what the window leaves.
    height: f64,
    /// How far the rows are scrolled.
    offset: f64,
    /// Where the rows are now (`origin`, `width`, `height` and `offset` laid out).
    lay: ListLayout,
    hover: Option<usize>,
    bounds: Size,
    open: Option<Box<Open>>,
    /// A release is only a choice once the pointer has moved or pressed in the menu: the
    /// release that ends the press which opened the menu (a toolbar field opens one on a press,
    /// and the panel may be moved under the pointer to fit the window) chooses nothing.
    armed: bool,
    /// The previous pointer position this panel saw: the apex of the safe triangle.
    last: Option<Point>,
    /// A switch the pointer asked for by resting on another row, made by [`Self::tick`].
    pending: Option<Pending>,
    /// When the strips last scrolled a panel under the pointer (the first panel keeps it).
    scroll_clock: Option<f64>,
}

/// A row the pointer rests on while a submenu is open beside another row.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Pending {
    /// The row under the pointer (`None`: the panel padding).
    row: Option<usize>,
    /// The time of the first tick after the pointer settled, `None` until that tick.
    since: Option<f64>,
}

/// What a key did in a panel.
enum Nav {
    /// Handled; the menu stays open.
    Done,
    /// The panel asks its parent to close it (Left, Escape).
    Back,
    /// An item was chosen.
    Run(MenuOutcome),
}

/// The submenu open beside a panel: the row that opened it and its panel.
#[derive(Debug, Clone, PartialEq)]
struct Open {
    row: usize,
    menu: ContextMenu,
}

/// Whether `p` is inside the triangle `a`, `b`, `c` (edges included).
fn in_triangle(p: Point, a: Point, b: Point, c: Point) -> bool {
    let side = |u: Point, v: Point| (v.x - u.x) * (p.y - u.y) - (v.y - u.y) * (p.x - u.x);
    let (d1, d2, d3) = (side(a, b), side(b, c), side(c, a));
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}

/// Estimated width of a text run.
fn text_width(text: &str) -> f64 {
    text.chars().count() as f64 * CHAR_WIDTH
}

/// The entries of `specs`: labels from `registry`, shortcut text from `keymap`; a command the
/// registry does not know is left out, and so are doubled, leading and trailing separators.
fn build_entries(specs: &[Spec], registry: &Registry, keymap: &Keymap) -> Vec<Entry> {
    let mut entries: Vec<Entry> = Vec::new();
    for spec in specs {
        match spec {
            Spec::Separator => {
                if !matches!(entries.last(), None | Some(Entry::Separator)) {
                    entries.push(Entry::Separator);
                }
            }
            Spec::Rows(rows) => entries.extend(rows.iter().cloned()),
            Spec::Item {
                command,
                arg,
                label,
                enabled,
                checked,
            } => {
                let Some(def) = registry.get(command) else {
                    continue;
                };
                entries.push(Entry::Item(MenuItemDef {
                    command: (*command).to_string(),
                    arg: arg.map(str::to_string),
                    label: label.unwrap_or(def.label).to_string(),
                    shortcut: keymap.shortcut_text(command, *arg),
                    enabled: *enabled,
                    checked: *checked,
                    swatch: None,
                }));
            }
            Spec::Submenu {
                label,
                enabled,
                items,
            } => {
                let inner = build_entries(items, registry, keymap);
                if inner.is_empty() {
                    continue;
                }
                entries.push(Entry::Submenu(SubmenuDef {
                    label: (*label).to_string(),
                    enabled: *enabled,
                    entries: inner,
                }));
            }
        }
    }
    while matches!(entries.last(), Some(Entry::Separator)) {
        entries.pop();
    }
    entries
}

fn panel_width(entries: &[Entry]) -> f64 {
    entries
        .iter()
        .filter_map(|e| match e {
            Entry::Item(i) => Some(
                text_width(&i.label)
                    + i.shortcut.as_deref().map_or(0.0, |s| text_width(s) + 32.0)
                    + 48.0,
            ),
            Entry::Submenu(s) => Some(text_width(&s.label) + 64.0),
            Entry::Separator => None,
        })
        .fold(MIN_WIDTH, f64::max)
}

/// The height of each entry.
fn heights_of(entries: &[Entry]) -> Vec<f64> {
    entries.iter().map(Entry::height).collect()
}

/// The rows of `entries` laid out in a panel at `origin`.
fn laid(entries: &[Entry], origin: Point, width: f64, height: f64, offset: f64) -> ListLayout {
    geo::layout(
        &heights_of(entries),
        origin.x,
        origin.x + width,
        origin.y,
        height,
        offset,
    )
}

impl ContextMenu {
    /// Builds the menu of `specs`: labels from `registry`, shortcut text from `keymap`, placed
    /// at `at` and moved so it stays inside `bounds` (the window, logical pixels). A command
    /// the registry does not know is left out.
    pub fn build(
        specs: &[Spec],
        registry: &Registry,
        keymap: &Keymap,
        at: Point,
        bounds: Size,
    ) -> Self {
        Self::panel(build_entries(specs, registry, keymap), at, bounds)
    }

    /// A menu of rows built by the caller (a menu whose labels and arguments are not known
    /// until it opens, like the jack menu), placed at `at` and moved inside `bounds`.
    pub fn from_entries(entries: Vec<Entry>, at: Point, bounds: Size) -> Self {
        Self::panel(entries, at, bounds)
    }

    /// One panel of `entries` at `at`, moved inside `bounds`.
    fn panel(entries: Vec<Entry>, at: Point, bounds: Size) -> Self {
        let width = panel_width(&entries);
        let natural = geo::natural_height(&heights_of(&entries));
        // Moved up before it is made shorter; a panel the window cannot hold scrolls.
        let (top, height) = geo::fit_vertical(at.y, natural, 0.0, bounds.height, true);
        let origin = Point::new(at.x.min(bounds.width - width).max(0.0), top);
        let lay = laid(&entries, origin, width, height, 0.0);
        ContextMenu {
            entries,
            origin,
            width,
            height,
            offset: 0.0,
            lay,
            hover: None,
            bounds,
            open: None,
            armed: false,
            last: None,
            pending: None,
            scroll_clock: None,
        }
    }

    /// Lays the rows out again after the position or the scroll changed.
    fn relayout(&mut self) {
        self.lay = laid(
            &self.entries,
            self.origin,
            self.width,
            self.height,
            self.offset,
        );
        self.offset = self.lay.offset;
    }

    /// Scrolls the rows by `by` pixels (positive shows later rows); `true` when they moved. The
    /// submenus open from this panel close, as their rows moved.
    fn scroll_by(&mut self, by: f64) -> bool {
        let next = (self.offset + by).clamp(0.0, self.lay.max_offset);
        if (next - self.offset).abs() < 1e-9 {
            return false;
        }
        self.offset = next;
        self.relayout();
        self.open = None;
        self.pending = None;
        true
    }

    /// Scrolls the least that shows the highlighted row whole.
    fn reveal(&mut self) {
        let Some(row) = self.hover else { return };
        if !self.lay.scrolls() {
            return;
        }
        let top = self.lay.content_top(row);
        let to = geo::offset_to_show(
            self.offset,
            self.lay.view.height(),
            self.lay.max_offset,
            top,
            top + self.entries[row].height(),
        );
        if (to - self.offset).abs() > 1e-9 {
            self.offset = to;
            self.relayout();
        }
    }

    /// Scrolls the panel under `pos` (the innermost one) by `by`; `true` when it moved.
    fn wheel(&mut self, pos: Point, by: f64) -> bool {
        if let Some(o) = &mut self.open
            && o.menu.covers(pos)
        {
            return o.menu.wheel(pos, by);
        }
        self.rect().contains(pos) && self.scroll_by(by)
    }

    /// The panel (its depth below this one) whose scroll strip `p` is on, and the direction it
    /// scrolls: `-1` up, `1` down. A strip with nothing left to scroll does not count.
    fn strip_depth(&self, p: Point) -> Option<(usize, f64)> {
        if let Some(o) = &self.open
            && o.menu.covers(p)
        {
            return o.menu.strip_depth(p).map(|(d, dir)| (d + 1, dir));
        }
        if !self.rect().contains(p) {
            return None;
        }
        let on = |s: Option<Rect>| s.is_some_and(|s| s.contains(p));
        if on(self.lay.up_strip()) && self.offset > 0.0 {
            Some((0, -1.0))
        } else if on(self.lay.down_strip()) && self.offset < self.lay.max_offset - 1e-9 {
            Some((0, 1.0))
        } else {
            None
        }
    }

    /// Scrolls the panel `depth` levels below this one by `by`.
    fn scroll_depth(&mut self, depth: usize, by: f64) -> bool {
        if depth == 0 {
            return self.scroll_by(by);
        }
        self.open
            .as_mut()
            .is_some_and(|o| o.menu.scroll_depth(depth - 1, by))
    }

    /// The strip the pointer rests on (the first panel sees every pointer move).
    fn on_strip(&self) -> Option<(usize, f64)> {
        self.last.and_then(|p| self.strip_depth(p))
    }

    /// The rows of the first panel, top to bottom.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The items of the first panel (separators and submenu rows skipped).
    pub fn items(&self) -> impl Iterator<Item = &MenuItemDef> {
        self.entries.iter().filter_map(|e| match e {
            Entry::Item(i) => Some(i),
            _ => None,
        })
    }

    /// Every item of the menu, submenus included, depth first.
    pub fn all_items(&self) -> Vec<&MenuItemDef> {
        fn walk<'a>(entries: &'a [Entry], out: &mut Vec<&'a MenuItemDef>) {
            for e in entries {
                match e {
                    Entry::Item(i) => out.push(i),
                    Entry::Submenu(s) => walk(&s.entries, out),
                    Entry::Separator => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.entries, &mut out);
        out
    }

    /// The first panel's rectangle.
    pub fn rect(&self) -> Rect {
        self.lay.panel
    }

    /// The rectangle of each row of the first panel (separators included), cut to what shows:
    /// a row scrolled out of the panel has an empty one.
    pub fn row_rects(&self) -> Vec<Rect> {
        self.lay.rows.clone()
    }

    /// How far the first panel is scrolled (0 when it fits the window).
    pub fn scroll_offset(&self) -> f64 {
        self.offset
    }

    /// The furthest the first panel can scroll: 0 when all its rows show.
    pub fn max_scroll(&self) -> f64 {
        self.lay.max_offset
    }

    /// The centre of the row of command `command` in the innermost open panel that has one (the
    /// first panel, then the open submenus; tests click it).
    pub fn item_center(&self, command: &str) -> Option<Point> {
        self.find_center(|e| matches!(e, Entry::Item(i) if i.command == command))
    }

    /// The centre of the item of command `command` with argument `arg` in an open panel.
    pub fn item_center_with(&self, command: &str, arg: &str) -> Option<Point> {
        self.find_center(|e| {
            matches!(e, Entry::Item(i) if i.command == command && i.arg.as_deref() == Some(arg))
        })
    }

    /// The centre of the submenu row labelled `label` in an open panel.
    pub fn submenu_center(&self, label: &str) -> Option<Point> {
        self.find_center(|e| matches!(e, Entry::Submenu(s) if s.label == label))
    }

    fn find_center(&self, pred: impl Fn(&Entry) -> bool + Copy) -> Option<Point> {
        self.entries
            .iter()
            .zip(&self.lay.rows)
            .find_map(|(e, r)| (pred(e) && *r != Rect::ZERO).then(|| r.center()))
            .or_else(|| self.open.as_ref().and_then(|o| o.menu.find_center(pred)))
    }

    /// The labels of the open submenus, outermost first.
    pub fn open_path(&self) -> Vec<&str> {
        let mut out = Vec::new();
        let mut at = self;
        while let Some(o) = &at.open {
            if let Entry::Submenu(s) = &at.entries[o.row] {
                out.push(s.label.as_str());
            }
            at = &o.menu;
        }
        out
    }

    /// Whether `p` is over this panel or an open submenu of it.
    fn covers(&self, p: Point) -> bool {
        self.rect().contains(p) || self.open.as_ref().is_some_and(|o| o.menu.covers(p))
    }

    fn row_at(&self, p: Point) -> Option<usize> {
        self.lay.rows.iter().position(|r| r.contains(p))
    }

    /// Whether row `row` can be highlighted and chosen.
    fn selectable(&self, row: usize) -> bool {
        match &self.entries[row] {
            Entry::Item(i) => i.enabled,
            Entry::Submenu(s) => s.enabled,
            Entry::Separator => false,
        }
    }

    /// The label of the highlighted row of the innermost open panel.
    pub fn highlighted(&self) -> Option<&str> {
        if let Some(o) = &self.open {
            return o.menu.highlighted();
        }
        match &self.entries[self.hover?] {
            Entry::Item(i) => Some(&i.label),
            Entry::Submenu(s) => Some(&s.label),
            Entry::Separator => None,
        }
    }

    /// What the menu looks like: the highlighted row, the open submenu row and the scroll of every
    /// panel, outermost first. Two equal looks paint the same, so a caller repaints only when it
    /// changes (a pointer move inside one row, or in the padding, changes nothing).
    pub fn look(&self) -> Vec<(Option<usize>, Option<usize>, f64)> {
        let mut out = Vec::new();
        let mut at = self;
        loop {
            out.push((at.hover, at.open.as_ref().map(|o| o.row), at.offset));
            match &at.open {
                Some(o) => at = &o.menu,
                None => return out,
            }
        }
    }

    /// Whether a pointer switch is waiting for its delay.
    pub fn has_pending(&self) -> bool {
        self.pending.is_some() || self.open.as_ref().is_some_and(|o| o.menu.has_pending())
    }

    /// Whether the menu wants a tick on the very next frame: a switch whose delay has not been
    /// stamped yet, or a pointer resting on a scroll strip that can still scroll. Otherwise a
    /// pending switch only needs the tick at [`Self::wake_at`].
    pub fn needs_tick(&self) -> bool {
        self.unstamped() || self.on_strip().is_some()
    }

    fn unstamped(&self) -> bool {
        self.pending.is_some_and(|p| p.since.is_none())
            || self.open.as_ref().is_some_and(|o| o.menu.unstamped())
    }

    /// The time (the clock of [`Self::tick`]) at which a waiting switch is due, the earliest of
    /// the panels'. The loop sleeps until then instead of polling.
    pub fn wake_at(&self) -> Option<f64> {
        let own = self
            .pending
            .and_then(|p| p.since)
            .map(|t| t + SUBMENU_DELAY);
        let inner = self.open.as_ref().and_then(|o| o.menu.wake_at());
        match (own, inner) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Advances the menu to `now` (seconds, any monotonic clock): a pointer that rested on
    /// another row for [`SUBMENU_DELAY`] switches to it, and one resting on a scroll strip scrolls
    /// the panel. Returns whether the menu changed.
    pub fn tick(&mut self, now: f64) -> bool {
        let mut changed = self.tick_pending(now);
        match self.on_strip() {
            None => self.scroll_clock = None,
            Some((depth, dir)) => {
                let dt = self
                    .scroll_clock
                    .map_or(1.0 / 60.0, |t| (now - t).clamp(0.0, 0.1));
                self.scroll_clock = Some(now);
                changed |= self.scroll_depth(depth, dir * ARROW_SPEED * dt);
            }
        }
        changed
    }

    fn tick_pending(&mut self, now: f64) -> bool {
        let mut changed = self.open.as_mut().is_some_and(|o| o.menu.tick_pending(now));
        if let Some(p) = &mut self.pending {
            let since = *p.since.get_or_insert(now);
            if now >= since + SUBMENU_DELAY {
                let row = p.row;
                self.pending = None;
                self.switch_to(row);
                changed = true;
            }
        }
        changed
    }

    /// The pointer left this panel: drop what it was heading for.
    fn forget_pointer(&mut self) {
        self.last = None;
        self.pending = None;
        if let Some(o) = &mut self.open {
            o.menu.forget_pointer();
        }
    }

    /// Opens the submenu of `row`, or closes the open one, as the pointer on `row` asks.
    fn switch_to(&mut self, row: Option<usize>) {
        match row.map(|r| (r, &self.entries[r])) {
            Some((r, Entry::Submenu(s))) if s.enabled => self.open_row(r),
            _ => {
                self.open = None;
                self.hover = row.filter(|r| matches!(self.entries[*r], Entry::Item(_)));
            }
        }
    }

    /// Whether a move from `from` to `to` heads for the open submenu: inside the triangle from
    /// `from` to the submenu near edge, and closer to that edge.
    fn heads_for_submenu(&self, from: Point, to: Point) -> bool {
        let Some(o) = &self.open else { return false };
        let child = o.menu.rect();
        let near = if child.center().x > self.rect().center().x {
            child.x0
        } else {
            child.x1
        };
        let (a, b) = (Point::new(near, child.y0), Point::new(near, child.y1));
        (near - to.x).abs() < (near - from.x).abs() && in_triangle(to, from, a, b)
    }

    /// Opens the submenu of row `row` beside it (to the left when there is no room at the right).
    fn open_row(&mut self, row: usize) {
        if self.open.as_ref().is_some_and(|o| o.row == row) {
            return;
        }
        let Entry::Submenu(s) = &self.entries[row] else {
            return;
        };
        let rr = self.lay.drawn[row];
        let child_width = panel_width(&s.entries);
        // Level with its row, moved up before it scrolls, on the left when the right is full.
        let mut child = Self::panel(s.entries.clone(), Point::new(0.0, rr.y0 - 4.0), self.bounds);
        child.origin.x = geo::place_flyout_x(self.rect(), child_width, self.bounds.width);
        child.relayout();
        child.armed = true;
        self.hover = Some(row);
        self.open = Some(Box::new(Open { row, menu: child }));
    }

    /// Handles an event while the menu is open. A click outside closes it: the caller drops
    /// the menu on [`MenuOutcome::Consumed`] after a press outside every panel, on `Run`
    /// and on [`Self::wants_close`].
    pub fn handle(&mut self, event: &UiEvent) -> MenuOutcome {
        match event {
            UiEvent::PointerMove { .. } | UiEvent::PointerDown { .. } => self.armed = true,
            _ => {}
        }
        match event {
            UiEvent::PointerMove { pos, .. } => {
                let from = self.last.replace(*pos);
                if let Some(o) = &mut self.open
                    && o.menu.covers(*pos)
                {
                    self.pending = None;
                    self.hover = Some(o.row);
                    return o.menu.handle(event);
                }
                if let Some(o) = &mut self.open {
                    o.menu.forget_pointer();
                }
                if !self.rect().contains(*pos) {
                    // Outside every panel (a gap, or the pointer left): nothing changes.
                    self.pending = None;
                    return MenuOutcome::Consumed;
                }
                let row = self.row_at(*pos);
                let opener = self.open.as_ref().map(|o| o.row);
                if opener.is_none() || row == opener {
                    self.pending = None;
                    self.switch_to(row);
                } else {
                    // Another row while a submenu is open: wait, unless heading for the submenu.
                    let toward = from.is_some_and(|f| self.heads_for_submenu(f, *pos));
                    let same = self.pending.is_some_and(|p| p.row == row);
                    if toward || !same {
                        self.pending = Some(Pending { row, since: None });
                    }
                }
                MenuOutcome::Consumed
            }
            UiEvent::PointerDown { pos, button, .. } => {
                if let Some(o) = &mut self.open
                    && o.menu.covers(*pos)
                {
                    return o.menu.handle(event);
                }
                // Pressing a submenu row opens it at once.
                let hit = self
                    .row_at(*pos)
                    .filter(|r| matches!(&self.entries[*r], Entry::Submenu(s) if s.enabled));
                if let (PointerButton::Primary, Some(r)) = (button, hit) {
                    self.pending = None;
                    self.open_row(r);
                }
                MenuOutcome::Consumed
            }
            UiEvent::Wheel {
                pos,
                delta,
                modifiers,
            } => {
                // A notch up (positive) moves the rows down.
                if self.wheel(*pos, -delta.y * WHEEL_STEP) {
                    return self.handle(&UiEvent::PointerMove {
                        pos: *pos,
                        modifiers: *modifiers,
                    });
                }
                MenuOutcome::Consumed
            }
            UiEvent::PointerLeave => {
                self.last = None;
                self.scroll_clock = None;
                MenuOutcome::Ignored
            }
            UiEvent::PointerUp { pos, button, .. } => {
                if let Some(o) = &mut self.open
                    && o.menu.covers(*pos)
                {
                    return o.menu.handle(event);
                }
                if *button != PointerButton::Primary || !self.armed {
                    return MenuOutcome::Consumed;
                }
                match self.row_at(*pos).map(|r| (r, &self.entries[r])) {
                    Some((_, Entry::Item(i))) if i.enabled => MenuOutcome::Run {
                        command: i.command.clone(),
                        arg: i.arg.clone(),
                    },
                    Some((r, Entry::Submenu(s))) if s.enabled => {
                        self.open_row(r);
                        MenuOutcome::Consumed
                    }
                    _ => MenuOutcome::Consumed,
                }
            }
            UiEvent::Key {
                key, pressed: true, ..
            } => match self.key(key, false) {
                Nav::Run(outcome) => outcome,
                Nav::Done | Nav::Back => MenuOutcome::Consumed,
            },
            UiEvent::Key { .. } => MenuOutcome::Consumed,
            _ => MenuOutcome::Ignored,
        }
    }

    /// A key press in the innermost open panel; `nested` is whether this panel is a submenu.
    fn key(&mut self, key: &Key, nested: bool) -> Nav {
        self.pending = None;
        if let Some(o) = &mut self.open {
            return match o.menu.key(key, true) {
                Nav::Back => {
                    // The highlight is already on the row that opened it.
                    self.open = None;
                    Nav::Done
                }
                other => other,
            };
        }
        let n = self.entries.len();
        match key {
            Key::Down | Key::Up => {
                let down = *key == Key::Down;
                // From nothing, Down steps on to the first row and Up to the last one.
                let mut at = self.hover.unwrap_or(if down { n - 1 } else { 0 });
                for _ in 0..n {
                    at = if down { (at + 1) % n } else { (at + n - 1) % n };
                    if self.selectable(at) {
                        self.hover = Some(at);
                        break;
                    }
                }
            }
            Key::Home => self.hover = (0..n).find(|r| self.selectable(*r)).or(self.hover),
            Key::End => self.hover = (0..n).rev().find(|r| self.selectable(*r)).or(self.hover),
            Key::Right | Key::Enter | Key::Space => {
                let Some(row) = self.hover.filter(|r| self.selectable(*r)) else {
                    return Nav::Done;
                };
                match &self.entries[row] {
                    Entry::Submenu(_) => {
                        self.open_row(row);
                        if let Some(o) = &mut self.open {
                            o.menu.hover =
                                (0..o.menu.entries.len()).find(|r| o.menu.selectable(*r));
                        }
                    }
                    Entry::Item(i) if *key != Key::Right => {
                        return Nav::Run(MenuOutcome::Run {
                            command: i.command.clone(),
                            arg: i.arg.clone(),
                        });
                    }
                    _ => {}
                }
            }
            Key::Left | Key::Escape if nested => return Nav::Back,
            _ => {}
        }
        self.reveal();
        Nav::Done
    }

    /// Whether `event` closes the menu without choosing: Escape with no submenu open, or a press
    /// outside every panel.
    pub fn wants_close(&self, event: &UiEvent) -> bool {
        match event {
            // Escape closes the innermost open submenu first (see `handle`).
            UiEvent::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } => self.open.is_none(),
            UiEvent::PointerDown { pos, .. } => !self.covers(*pos),
            _ => false,
        }
    }

    /// Paints the panel and the open submenus (logical units, in the coordinates the menu was
    /// opened in).
    pub fn paint(&self, canvas: &mut dyn Canvas, theme: &Theme) {
        let panel = self.rect();
        canvas.fill_with(
            RoundedRect::from_rect(panel + Vec2::new(2.0, 3.0), 6.0),
            Color::from_rgba8(0, 0, 0, 90),
        );
        canvas.fill_with(RoundedRect::from_rect(panel, 6.0), theme.browser_bg);
        canvas.stroke_with(RoundedRect::from_rect(panel, 6.0), 1.0, theme.section_line);
        let scrolls = self.lay.scrolls();
        if scrolls {
            canvas.push_clip(&self.lay.view.into());
        }
        for (i, (entry, r)) in self
            .entries
            .iter()
            .zip(self.lay.drawn.iter().copied())
            .enumerate()
        {
            if scrolls && (r.y1 <= self.lay.view.y0 || r.y0 >= self.lay.view.y1) {
                continue;
            }
            match entry {
                Entry::Separator => {
                    let y = r.center().y;
                    canvas.fill_with(
                        Rect::new(r.x0 + 6.0, y - 0.5, r.x1 - 6.0, y + 0.5),
                        theme.section_line,
                    );
                }
                Entry::Item(item) => {
                    if self.hover == Some(i) && item.enabled {
                        canvas.fill_with(RoundedRect::from_rect(r, 4.0), theme.selection);
                    }
                    let color = if item.enabled {
                        theme.text
                    } else {
                        theme.text_muted
                    };
                    let base = r.y0 + 16.5;
                    if item.checked == Some(true) {
                        canvas.text(&TextRun::new(
                            "\u{2713}",
                            Point::new(r.x0 + 8.0, base),
                            FONT,
                            color,
                        ));
                    }
                    if let Some([red, green, blue]) = item.swatch {
                        let alpha = if item.enabled { 255 } else { 110 };
                        let chip = Rect::new(
                            r.x0 + 8.0,
                            r.center().y - 5.0,
                            r.x0 + 18.0,
                            r.center().y + 5.0,
                        );
                        canvas.fill_with(
                            RoundedRect::from_rect(chip, 2.0),
                            Color::from_rgba8(red, green, blue, alpha),
                        );
                    }
                    canvas.text(&TextRun::new(
                        &item.label,
                        Point::new(r.x0 + 28.0, base),
                        FONT,
                        color,
                    ));
                    if let Some(s) = item.shortcut.as_deref() {
                        canvas.text(
                            &TextRun::new(s, Point::new(r.x1 - 10.0, base), 12.0, theme.text_muted)
                                .align(Align::End),
                        );
                    }
                }
                Entry::Submenu(sub) => {
                    if self.hover == Some(i) && sub.enabled {
                        canvas.fill_with(RoundedRect::from_rect(r, 4.0), theme.selection);
                    }
                    let color = if sub.enabled {
                        theme.text
                    } else {
                        theme.text_muted
                    };
                    let base = r.y0 + 16.5;
                    canvas.text(&TextRun::new(
                        &sub.label,
                        Point::new(r.x0 + 28.0, base),
                        FONT,
                        color,
                    ));
                    canvas.text(
                        &TextRun::new("\u{25B8}", Point::new(r.x1 - 10.0, base), 12.0, color)
                            .align(Align::End),
                    );
                }
            }
        }
        if scrolls {
            canvas.pop_clip();
            self.paint_strips(canvas, theme);
        }
        if let Some(o) = &self.open {
            o.menu.paint(canvas, theme);
        }
    }

    /// Paints the two scroll strips of a panel that scrolls; one lights when the pointer is on it.
    fn paint_strips(&self, canvas: &mut dyn Canvas, theme: &Theme) {
        let pointer = self.last.unwrap_or(Point::new(-1.0e6, -1.0e6));
        for (strip, glyph, enabled) in [
            (self.lay.up_strip(), "\u{25B4}", self.offset > 0.0),
            (
                self.lay.down_strip(),
                "\u{25BE}",
                self.offset < self.lay.max_offset,
            ),
        ] {
            let Some(strip) = strip else { continue };
            if enabled && strip.contains(pointer) {
                canvas.fill_with(
                    RoundedRect::from_rect(strip.inset(-2.0), 4.0),
                    theme.selection,
                );
            }
            let color = if enabled {
                theme.text
            } else {
                theme.text_muted
            };
            canvas.text(
                &TextRun::new(
                    glyph,
                    Point::new(strip.center().x, strip.y1 - 2.5),
                    11.0,
                    color,
                )
                .align(Align::Center),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Modifiers;
    use crate::keymap::{Os, Profile};

    fn menu(specs: &[Spec], os: Os) -> ContextMenu {
        ContextMenu::build(
            specs,
            &Registry::bundled(),
            &Keymap::bundled(Profile::Standard, os),
            Point::new(10.0, 10.0),
            Size::new(800.0, 600.0),
        )
    }

    fn up(p: Point) -> UiEvent {
        UiEvent::PointerUp {
            pos: p,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
        }
    }

    #[test]
    fn labels_and_shortcut_text_come_from_the_registry_and_the_keymap() {
        let specs = [Spec::item("edit.copy"), Spec::item("edit.paste")];
        let win = menu(&specs, Os::Windows);
        let mac = menu(&specs, Os::Mac);
        let w: Vec<_> = win
            .items()
            .map(|i| (i.label.as_str(), i.shortcut.clone()))
            .collect();
        assert_eq!(
            w,
            [
                ("Copy", Some("Ctrl+C".into())),
                ("Paste", Some("Ctrl+V".into()))
            ]
        );
        let m: Vec<_> = mac.items().map(|i| i.shortcut.clone()).collect();
        assert_eq!(m, [Some("Cmd+C".into()), Some("Cmd+V".into())]);
    }

    #[test]
    fn an_unknown_command_is_left_out_and_separators_are_tidy() {
        let specs = [
            Spec::Separator,
            Spec::item("edit.nope"),
            Spec::item("edit.copy"),
            Spec::Separator,
            Spec::Separator,
            Spec::item("edit.paste"),
            Spec::Separator,
        ];
        let m = menu(&specs, Os::Windows);
        let kinds: Vec<bool> = m
            .entries()
            .iter()
            .map(|e| matches!(e, Entry::Item(_)))
            .collect();
        assert_eq!(kinds, [true, false, true]);
    }

    #[test]
    fn a_custom_label_replaces_the_registry_label() {
        let m = menu(
            &[Spec::item("edit.duplicate").labelled("Duplicate Track")],
            Os::Windows,
        );
        assert_eq!(m.items().next().unwrap().label, "Duplicate Track");
    }

    #[test]
    fn the_menu_stays_inside_the_window() {
        let m = ContextMenu::build(
            &[Spec::item("edit.copy"), Spec::item("edit.paste")],
            &Registry::bundled(),
            &Keymap::bundled(Profile::Standard, Os::Windows),
            Point::new(790.0, 595.0),
            Size::new(800.0, 600.0),
        );
        let r = m.rect();
        assert!(
            r.x1 <= 800.0 && r.y1 <= 600.0 && r.x0 >= 0.0 && r.y0 >= 0.0,
            "{r:?}"
        );
    }

    #[test]
    fn choosing_an_enabled_item_runs_it_and_a_disabled_one_does_nothing() {
        let specs = [
            Spec::item("edit.copy"),
            Spec::item("edit.paste").enabled(false),
        ];
        let mut m = menu(&specs, Os::Windows);
        let copy = m.item_center("edit.copy").unwrap();
        let paste = m.item_center("edit.paste").unwrap();
        m.handle(&mv(copy));
        assert_eq!(m.handle(&up(paste)), MenuOutcome::Consumed);
        assert_eq!(
            m.handle(&up(copy)),
            MenuOutcome::Run {
                command: "edit.copy".into(),
                arg: None
            }
        );
    }

    #[test]
    fn escape_and_an_outside_press_close_the_menu() {
        let m = menu(&[Spec::item("edit.copy")], Os::Windows);
        let esc = UiEvent::Key {
            key: Key::Escape,
            modifiers: Modifiers::NONE,
            pressed: true,
        };
        assert!(m.wants_close(&esc));
        let outside = UiEvent::PointerDown {
            pos: Point::new(700.0, 500.0),
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        };
        assert!(m.wants_close(&outside));
        let inside = UiEvent::PointerDown {
            pos: m.item_center("edit.copy").unwrap(),
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        };
        assert!(!m.wants_close(&inside));
    }

    #[test]
    fn every_menu_command_exists_in_the_registry() {
        // items are command ids; a typo shows as a missing row in the tests that count
        // rows, and the build skips unknown ids rather than showing a dead item.
        let registry = Registry::bundled();
        for id in [
            "edit.cut",
            "edit.copy",
            "edit.paste",
            "edit.duplicate",
            "edit.delete",
        ] {
            assert!(registry.get(id).is_some(), "{id}");
        }
    }

    fn mv(p: Point) -> UiEvent {
        UiEvent::PointerMove {
            pos: p,
            modifiers: Modifiers::NONE,
        }
    }

    fn nested() -> Vec<Spec> {
        vec![
            Spec::item("edit.copy"),
            Spec::submenu(
                "Chords",
                vec![
                    Spec::item("edit.cut"),
                    Spec::submenu("Deeper", vec![Spec::item("edit.delete")]),
                    Spec::item("edit.paste").enabled(false),
                ],
            ),
            Spec::submenu("Empty of known commands", vec![Spec::item("edit.nope")]),
            Spec::submenu("Off", vec![Spec::item("edit.duplicate")]).enabled(false),
        ]
    }

    #[test]
    fn a_submenu_opens_on_hover_and_its_items_run() {
        let mut m = menu(&nested(), Os::Windows);
        assert_eq!(
            m.entries().len(),
            3,
            "a submenu without known commands is left out"
        );
        assert!(m.item_center("edit.cut").is_none(), "closed");
        let row = m.submenu_center("Chords").unwrap();
        assert_eq!(m.handle(&mv(row)), MenuOutcome::Consumed);
        assert_eq!(m.open_path(), ["Chords"]);
        let cut = m.item_center("edit.cut").unwrap();
        assert!(cut.x > m.rect().x1 - 10.0, "beside the panel: {cut:?}");
        // The pointer travels to the submenu (over its edge) and keeps it open.
        m.handle(&mv(cut));
        assert_eq!(m.open_path(), ["Chords"]);
        assert_eq!(
            m.handle(&up(cut)),
            MenuOutcome::Run {
                command: "edit.cut".into(),
                arg: None
            }
        );
    }

    #[test]
    fn submenus_nest_and_moving_to_another_row_closes_them() {
        let mut m = menu(&nested(), Os::Windows);
        m.handle(&mv(m.submenu_center("Chords").unwrap()));
        m.handle(&mv(m.submenu_center("Deeper").unwrap()));
        assert_eq!(m.open_path(), ["Chords", "Deeper"]);
        let del = m.item_center("edit.delete").unwrap();
        assert_eq!(
            m.handle(&up(del)),
            MenuOutcome::Run {
                command: "edit.delete".into(),
                arg: None
            }
        );
        // Back to the first panel's item: everything closes.
        let mut m = menu(&nested(), Os::Windows);
        m.handle(&mv(m.submenu_center("Chords").unwrap()));
        m.handle(&mv(m.item_center("edit.copy").unwrap()));
        m.tick(0.0);
        m.tick(1.0);
        assert!(m.open_path().is_empty());
        assert!(m.item_center("edit.cut").is_none());
    }

    #[test]
    fn a_click_on_a_submenu_row_opens_it_and_runs_nothing() {
        let mut m = menu(&nested(), Os::Windows);
        let row = m.submenu_center("Chords").unwrap();
        m.handle(&mv(Point::new(0.0, 0.0)));
        assert_eq!(m.handle(&up(row)), MenuOutcome::Consumed);
        assert_eq!(m.open_path(), ["Chords"]);
        // A disabled item of the submenu does nothing.
        let paste = m.item_center("edit.paste").unwrap();
        assert_eq!(m.handle(&up(paste)), MenuOutcome::Consumed);
        // A disabled submenu row does not open.
        let mut m = menu(&nested(), Os::Windows);
        let off = m.submenu_center("Off").unwrap();
        m.handle(&mv(off));
        m.handle(&up(off));
        assert!(m.open_path().is_empty());
    }

    #[test]
    fn the_release_that_ends_the_press_which_opened_the_menu_chooses_nothing() {
        let specs = [Spec::item("edit.copy"), Spec::item("edit.paste")];
        let mut m = menu(&specs, Os::Windows);
        let copy = m.item_center("edit.copy").unwrap();
        // The panel sits under the pointer (it was moved to fit the window): the release alone
        // is not a choice.
        assert_eq!(m.handle(&up(copy)), MenuOutcome::Consumed);
        // After a press (or a move) in the menu, a release is.
        let down = UiEvent::PointerDown {
            pos: copy,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        };
        m.handle(&down);
        assert_eq!(
            m.handle(&up(copy)),
            MenuOutcome::Run {
                command: "edit.copy".into(),
                arg: None
            }
        );
    }

    #[test]
    fn a_press_inside_an_open_submenu_does_not_close_the_menu_and_one_outside_does() {
        let mut m = menu(&nested(), Os::Windows);
        m.handle(&mv(m.submenu_center("Chords").unwrap()));
        let down = |pos| UiEvent::PointerDown {
            pos,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        };
        assert!(!m.wants_close(&down(m.item_center("edit.cut").unwrap())));
        assert!(m.wants_close(&down(Point::new(790.0, 590.0))));
    }

    #[test]
    fn a_submenu_opens_to_the_left_when_there_is_no_room_and_stays_in_the_window() {
        let mut m = ContextMenu::build(
            &nested(),
            &Registry::bundled(),
            &Keymap::bundled(Profile::Standard, Os::Windows),
            Point::new(600.0, 560.0),
            Size::new(800.0, 600.0),
        );
        m.handle(&mv(m.submenu_center("Chords").unwrap()));
        let cut = m.item_center("edit.cut").unwrap();
        assert!(
            cut.x < m.rect().x0 + 40.0,
            "to the left of the panel: {cut:?}"
        );
        let rows = m.open.as_ref().unwrap().menu.rect();
        assert!(
            rows.x0 >= 0.0 && rows.x1 <= 800.0 && rows.y1 <= 600.0 && rows.y0 >= 0.0,
            "{rows:?}"
        );
    }

    #[test]
    fn items_lists_only_the_first_panel_and_all_items_reaches_into_the_submenus() {
        let m = menu(&nested(), Os::Windows);
        assert_eq!(m.items().count(), 1);
        let all: Vec<&str> = m.all_items().iter().map(|i| i.command.as_str()).collect();
        assert_eq!(
            all,
            [
                "edit.copy",
                "edit.cut",
                "edit.delete",
                "edit.paste",
                "edit.duplicate"
            ]
        );
    }

    #[test]
    fn items_with_an_argument_are_found_by_it_and_a_submenu_paints_without_panic() {
        let spec = vec![Spec::submenu(
            "Snap",
            vec![
                Spec::Item {
                    command: "sequencer.set_snap",
                    arg: Some("beat"),
                    label: Some("Beat"),
                    enabled: true,
                    checked: Some(true),
                },
                Spec::Item {
                    command: "sequencer.set_snap",
                    arg: Some("bar"),
                    label: Some("Bar"),
                    enabled: true,
                    checked: None,
                },
            ],
        )];
        let mut m = menu(&spec, Os::Windows);
        m.handle(&mv(m.submenu_center("Snap").unwrap()));
        let bar = m.item_center_with("sequencer.set_snap", "bar").unwrap();
        let beat = m.item_center_with("sequencer.set_snap", "beat").unwrap();
        assert!(bar.y > beat.y);
        assert_eq!(
            m.handle(&up(bar)),
            MenuOutcome::Run {
                command: "sequencer.set_snap".into(),
                arg: Some("bar".into())
            }
        );
        let mut list = crate::display_list::DisplayList::new();
        m.paint(&mut list, &Theme::default());
    }

    fn tick(m: &mut ContextMenu, t: f64) {
        m.tick(t);
    }

    fn key_event(key: Key) -> UiEvent {
        UiEvent::Key {
            key,
            modifiers: Modifiers::NONE,
            pressed: true,
        }
    }

    /// Presses `key` and returns what the menu made of it.
    fn press(m: &mut ContextMenu, key: Key) -> MenuOutcome {
        m.handle(&key_event(key))
    }

    /// A menu at the right of the window, so its submenus flip to the left.
    fn flipped() -> ContextMenu {
        ContextMenu::build(
            &nested(),
            &Registry::bundled(),
            &Keymap::bundled(Profile::Standard, Os::Windows),
            Point::new(600.0, 300.0),
            Size::new(800.0, 600.0),
        )
    }

    /// Moves the pointer slowly from `a` to `b` in `steps` moves, with a tick of 50 ms after each.
    fn glide(m: &mut ContextMenu, a: Point, b: Point, steps: u32, t: &mut f64) {
        for i in 1..=steps {
            m.handle(&mv(a.lerp(b, f64::from(i) / f64::from(steps))));
            *t += 0.05;
            tick(m, *t);
            assert_eq!(m.open_path().first(), Some(&"Chords"), "step {i}");
        }
    }

    #[test]
    fn pointer_move_diagonally_through_a_sibling_keeps_the_submenu_open() {
        let mut m = menu(&nested(), Os::Windows);
        let row = m.submenu_center("Chords").unwrap();
        m.handle(&mv(row));
        let paste = m.item_center("edit.paste").unwrap();
        // The straight path from the row to the submenu's last item crosses the "Off" row.
        let off = m.row_rects()[2];
        let crosses = (1..=10).any(|i| off.contains(row.lerp(paste, f64::from(i) / 10.0)));
        assert!(crosses, "the path must cross the sibling row");
        let mut t = 0.0;
        glide(&mut m, row, paste, 40, &mut t);
        tick(&mut m, t + 1.0);
        assert_eq!(m.open_path(), ["Chords"], "the pointer is in the submenu");
    }

    #[test]
    fn pointer_resting_on_a_sibling_longer_than_the_delay_switches() {
        let mut m = menu(&nested(), Os::Windows);
        m.handle(&mv(m.submenu_center("Chords").unwrap()));
        m.handle(&mv(m.item_center("edit.copy").unwrap()));
        tick(&mut m, 0.0);
        assert_eq!(m.open_path(), ["Chords"], "not yet");
        assert!(m.has_pending());
        tick(&mut m, SUBMENU_DELAY * 0.5);
        assert_eq!(m.open_path(), ["Chords"], "still waiting");
        tick(&mut m, SUBMENU_DELAY + 0.01);
        assert!(m.open_path().is_empty());
        assert_eq!(m.highlighted(), Some("Copy"));
        assert!(!m.has_pending());
    }

    #[test]
    fn pointer_moving_back_to_the_opening_row_cancels_the_switch() {
        let mut m = menu(&nested(), Os::Windows);
        let row = m.submenu_center("Chords").unwrap();
        m.handle(&mv(row));
        m.handle(&mv(m.item_center("edit.copy").unwrap()));
        tick(&mut m, 0.0);
        m.handle(&mv(row));
        tick(&mut m, 1.0);
        assert_eq!(m.open_path(), ["Chords"]);
    }

    #[test]
    fn pointer_resting_on_a_disabled_submenu_row_closes_the_open_one_after_the_delay() {
        let mut m = menu(&nested(), Os::Windows);
        m.handle(&mv(m.submenu_center("Chords").unwrap()));
        m.handle(&mv(m.submenu_center("Off").unwrap()));
        assert_eq!(m.open_path(), ["Chords"]);
        tick(&mut m, 0.0);
        tick(&mut m, 1.0);
        assert!(m.open_path().is_empty());
    }

    #[test]
    fn pointer_in_the_overlap_or_outside_every_panel_keeps_the_submenu() {
        let mut m = menu(&nested(), Os::Windows);
        m.handle(&mv(m.submenu_center("Chords").unwrap()));
        let off = m.submenu_center("Off").unwrap();
        // The strip where the submenu overlaps the parent panel, on the sibling's row.
        m.handle(&mv(Point::new(m.rect().x1 - 1.0, off.y)));
        tick(&mut m, 0.0);
        tick(&mut m, 2.0);
        assert_eq!(m.open_path(), ["Chords"], "overlap strip");
        // Outside both panels (a gap, or the pointer left): nothing closes.
        m.handle(&mv(Point::new(700.0, 500.0)));
        tick(&mut m, 4.0);
        tick(&mut m, 6.0);
        assert_eq!(m.open_path(), ["Chords"], "outside");
    }

    #[test]
    fn a_submenu_flipped_to_the_left_keeps_the_same_hover_intent() {
        let mut m = flipped();
        let row = m.submenu_center("Chords").unwrap();
        m.handle(&mv(row));
        let child = m.open.as_ref().unwrap().menu.rect();
        assert!(child.x1 <= m.rect().x0 + 3.0, "flipped: {child:?}");
        let paste = m.item_center("edit.paste").unwrap();
        let mut t = 0.0;
        glide(&mut m, row, paste, 40, &mut t);
        // Resting on a sibling switches, as on the right.
        let mut m = flipped();
        m.handle(&mv(m.submenu_center("Chords").unwrap()));
        m.handle(&mv(m.item_center("edit.copy").unwrap()));
        tick(&mut m, 0.0);
        tick(&mut m, 1.0);
        assert!(m.open_path().is_empty());
    }

    #[test]
    fn nested_submenus_keep_their_own_hover_intent() {
        let mut m = menu(&nested(), Os::Windows);
        m.handle(&mv(m.submenu_center("Chords").unwrap()));
        m.handle(&mv(m.submenu_center("Deeper").unwrap()));
        assert_eq!(m.open_path(), ["Chords", "Deeper"]);
        // Resting on the sibling of "Deeper" inside the first submenu closes only "Deeper".
        m.handle(&mv(m.item_center("edit.cut").unwrap()));
        tick(&mut m, 0.0);
        assert_eq!(m.open_path(), ["Chords", "Deeper"]);
        tick(&mut m, 1.0);
        assert_eq!(m.open_path(), ["Chords"]);
    }

    fn keyed() -> Vec<Spec> {
        vec![
            Spec::item("edit.copy"),
            Spec::Separator,
            Spec::item("edit.cut").enabled(false),
            Spec::submenu(
                "Chords",
                vec![
                    Spec::item("edit.delete"),
                    Spec::item("edit.duplicate").enabled(false),
                    Spec::Separator,
                    Spec::item("edit.paste"),
                ],
            ),
            Spec::submenu("Off", vec![Spec::item("edit.delete")]).enabled(false),
            Spec::item("edit.undo"),
        ]
    }

    /// The highlighted label after each of `keys`.
    fn walk(m: &mut ContextMenu, keys: &[Key]) -> Vec<Option<String>> {
        keys.iter()
            .map(|k| {
                press(m, k.clone());
                m.highlighted().map(str::to_string)
            })
            .collect()
    }

    #[test]
    fn arrow_keys_move_the_highlight_wrap_and_skip_separators_and_disabled_rows() {
        let mut m = menu(&keyed(), Os::Windows);
        assert_eq!(m.highlighted(), None);
        let down = walk(&mut m, &[const { Key::Down }; 4]);
        let want = ["Copy", "Chords", "Undo", "Copy"].map(|s| Some(s.to_string()));
        assert_eq!(
            down, want,
            "Down: separators, Cut and Off are skipped; wraps"
        );
        let up = walk(&mut m, &[const { Key::Up }; 4]);
        let want = ["Undo", "Chords", "Copy", "Undo"].map(|s| Some(s.to_string()));
        assert_eq!(up, want, "Up wraps from the first row to the last");
    }

    #[test]
    fn up_with_nothing_highlighted_starts_at_the_last_row() {
        let mut m = menu(&keyed(), Os::Windows);
        press(&mut m, Key::Up);
        assert_eq!(m.highlighted(), Some("Undo"));
    }

    #[test]
    fn home_and_end_jump_to_the_first_and_last_selectable_row() {
        let mut m = menu(&keyed(), Os::Windows);
        press(&mut m, Key::End);
        assert_eq!(m.highlighted(), Some("Undo"));
        press(&mut m, Key::Home);
        assert_eq!(m.highlighted(), Some("Copy"));
    }

    #[test]
    fn right_opens_a_submenu_on_its_first_item_and_left_or_escape_returns_to_the_row() {
        let mut m = menu(&keyed(), Os::Windows);
        walk(&mut m, &[Key::Down, Key::Down]);
        assert_eq!(m.highlighted(), Some("Chords"));
        assert_eq!(press(&mut m, Key::Right), MenuOutcome::Consumed);
        assert_eq!(m.open_path(), ["Chords"]);
        assert_eq!(m.highlighted(), Some("Delete"));
        // Down skips the disabled item and the separator.
        press(&mut m, Key::Down);
        assert_eq!(m.highlighted(), Some("Paste"));
        press(&mut m, Key::Left);
        assert!(m.open_path().is_empty());
        assert_eq!(m.highlighted(), Some("Chords"), "back on the parent row");
        // Escape in a submenu closes the submenu only; at the root it closes the menu.
        press(&mut m, Key::Right);
        let esc = key_event(Key::Escape);
        assert!(!m.wants_close(&esc), "a submenu is open");
        m.handle(&esc);
        assert!(m.open_path().is_empty());
        assert_eq!(m.highlighted(), Some("Chords"));
        assert!(m.wants_close(&esc), "now the root");
    }

    #[test]
    fn enter_on_a_submenu_row_opens_it_and_enter_or_space_on_an_item_runs_it() {
        let mut m = menu(&keyed(), Os::Windows);
        walk(&mut m, &[Key::Down, Key::Down]);
        assert_eq!(press(&mut m, Key::Enter), MenuOutcome::Consumed);
        assert_eq!(m.highlighted(), Some("Delete"));
        assert_eq!(
            press(&mut m, Key::Enter),
            MenuOutcome::Run {
                command: "edit.delete".into(),
                arg: None
            }
        );
        let mut m = menu(&keyed(), Os::Windows);
        press(&mut m, Key::Home);
        assert_eq!(
            press(&mut m, Key::Space),
            MenuOutcome::Run {
                command: "edit.copy".into(),
                arg: None
            }
        );
    }

    #[test]
    fn a_disabled_item_is_not_run_by_enter() {
        let mut m = menu(&keyed(), Os::Windows);
        m.handle(&mv(m.item_center("edit.cut").unwrap()));
        assert_eq!(press(&mut m, Key::Enter), MenuOutcome::Consumed);
    }

    #[test]
    fn other_keys_are_consumed_and_a_release_moves_nothing() {
        let mut m = menu(&keyed(), Os::Windows);
        press(&mut m, Key::Home);
        for k in [
            Key::Right,
            Key::Left,
            Key::Char('x'),
            Key::Tab,
            Key::PageDown,
        ] {
            assert_eq!(press(&mut m, k), MenuOutcome::Consumed);
        }
        assert_eq!(m.highlighted(), Some("Copy"));
        let release = UiEvent::Key {
            key: Key::Down,
            modifiers: Modifiers::NONE,
            pressed: false,
        };
        assert_eq!(m.handle(&release), MenuOutcome::Consumed);
        assert_eq!(m.highlighted(), Some("Copy"));
    }

    #[test]
    fn the_hover_highlight_and_the_keyboard_highlight_are_one() {
        let mut m = menu(&keyed(), Os::Windows);
        m.handle(&mv(m.item_center("edit.undo").unwrap()));
        assert_eq!(m.highlighted(), Some("Undo"));
        press(&mut m, Key::Down);
        assert_eq!(
            m.highlighted(),
            Some("Copy"),
            "continues from the hovered row"
        );
        press(&mut m, Key::Down);
        press(&mut m, Key::Right);
        // A pointer move onto a row of the submenu takes the highlight there.
        m.handle(&mv(m.item_center("edit.paste").unwrap()));
        assert_eq!(m.highlighted(), Some("Paste"));
        press(&mut m, Key::Up);
        assert_eq!(m.highlighted(), Some("Delete"));
    }

    #[test]
    fn a_key_cancels_a_pending_pointer_switch() {
        let mut m = menu(&keyed(), Os::Windows);
        m.handle(&mv(m.submenu_center("Chords").unwrap()));
        m.handle(&mv(m.item_center("edit.copy").unwrap()));
        assert!(m.has_pending());
        press(&mut m, Key::Down);
        assert!(!m.has_pending());
        tick(&mut m, 0.0);
        tick(&mut m, 5.0);
        assert_eq!(m.open_path(), ["Chords"]);
    }

    fn two_submenus() -> Vec<Spec> {
        vec![
            Spec::item("edit.copy"),
            Spec::submenu("First", vec![Spec::item("edit.cut")]),
            Spec::submenu("Second", vec![Spec::item("edit.paste")]),
        ]
    }

    fn down(p: Point) -> UiEvent {
        UiEvent::PointerDown {
            pos: p,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        }
    }

    fn wheel(p: Point, notches: f64) -> UiEvent {
        UiEvent::Wheel {
            delta: Vec2::new(0.0, notches),
            pos: p,
            modifiers: Modifiers::NONE,
        }
    }

    #[test]
    fn the_first_submenu_opens_the_moment_the_pointer_is_on_its_row() {
        let mut m = menu(&two_submenus(), Os::Windows);
        m.handle(&mv(m.submenu_center("First").unwrap()));
        assert_eq!(m.open_path(), ["First"], "no tick, no delay");
        assert!(!m.has_pending());
        assert!(!m.needs_tick());
        assert_eq!(m.wake_at(), None);
    }

    #[test]
    fn switching_to_another_submenu_waits_the_short_grace_and_wakes_at_its_deadline() {
        const { assert!(SUBMENU_DELAY <= 0.100_000_001, "a short grace") };
        let mut m = menu(&two_submenus(), Os::Windows);
        m.handle(&mv(m.submenu_center("First").unwrap()));
        m.handle(&mv(m.submenu_center("Second").unwrap()));
        assert_eq!(m.open_path(), ["First"], "still the first one");
        // Until the first tick stamps the wait, the menu asks for a frame; no deadline yet.
        assert!(m.needs_tick());
        assert_eq!(m.wake_at(), None);
        tick(&mut m, 10.0);
        // Then it sleeps until the deadline: no frames, one wake-up at the right time.
        assert!(!m.needs_tick());
        assert_eq!(m.wake_at(), Some(10.0 + SUBMENU_DELAY));
        assert!(!m.tick(10.0 + SUBMENU_DELAY * 0.5));
        assert_eq!(m.open_path(), ["First"]);
        assert!(m.tick(10.0 + SUBMENU_DELAY));
        assert_eq!(m.open_path(), ["Second"]);
        assert_eq!(m.wake_at(), None);
        assert!(!m.has_pending());
    }

    #[test]
    fn a_deadline_inside_an_open_submenu_is_reported_too() {
        let spec = vec![Spec::submenu(
            "Outer",
            vec![
                Spec::submenu("A", vec![Spec::item("edit.cut")]),
                Spec::submenu("B", vec![Spec::item("edit.paste")]),
            ],
        )];
        let mut m = menu(&spec, Os::Windows);
        m.handle(&mv(m.submenu_center("Outer").unwrap()));
        m.handle(&mv(m.submenu_center("A").unwrap()));
        m.handle(&mv(m.submenu_center("B").unwrap()));
        assert!(m.needs_tick());
        tick(&mut m, 3.0);
        assert_eq!(m.wake_at(), Some(3.0 + SUBMENU_DELAY));
        tick(&mut m, 3.0 + SUBMENU_DELAY);
        assert_eq!(m.open_path(), ["Outer", "B"]);
    }

    #[test]
    fn a_pointer_heading_for_the_open_submenu_keeps_pushing_the_deadline_back() {
        let mut m = menu(&nested(), Os::Windows);
        let row = m.submenu_center("Chords").unwrap();
        m.handle(&mv(row));
        let paste = m.item_center("edit.paste").unwrap();
        let mut t = 100.0;
        for i in 1..=30 {
            m.handle(&mv(row.lerp(paste, f64::from(i) / 30.0)));
            t += 0.03;
            tick(&mut m, t);
            assert_eq!(m.open_path().first(), Some(&"Chords"), "step {i}");
        }
    }

    #[test]
    fn a_press_on_a_submenu_row_opens_it_at_once_even_while_another_is_open() {
        let mut m = menu(&two_submenus(), Os::Windows);
        m.handle(&mv(m.submenu_center("First").unwrap()));
        let second = m.submenu_center("Second").unwrap();
        assert_eq!(m.handle(&down(second)), MenuOutcome::Consumed);
        assert_eq!(m.open_path(), ["Second"], "no grace for a click");
        assert!(!m.has_pending());
        // A press on a closed menu's row too.
        let mut m = menu(&two_submenus(), Os::Windows);
        m.handle(&down(m.submenu_center("First").unwrap()));
        assert_eq!(m.open_path(), ["First"]);
    }

    /// A panel of `n` items whose rows are told apart by their argument.
    fn many(n: usize) -> Vec<Entry> {
        (0..n)
            .map(|i| {
                Entry::Item(MenuItemDef {
                    command: "edit.copy".into(),
                    arg: Some(i.to_string()),
                    label: format!("Row {i}"),
                    shortcut: None,
                    enabled: true,
                    checked: None,
                    swatch: None,
                })
            })
            .collect()
    }

    fn tall(n: usize, at: Point, bounds: Size) -> ContextMenu {
        ContextMenu::from_entries(many(n), at, bounds)
    }

    fn at_row(m: &ContextMenu, i: usize) -> Option<Point> {
        m.item_center_with("edit.copy", &i.to_string())
    }

    #[test]
    fn a_menu_taller_than_the_window_is_clamped_to_it_and_scrolls() {
        let window = Size::new(800.0, 300.0);
        let m = tall(40, Point::new(100.0, 120.0), window);
        let r = m.rect();
        assert!(r.y0 >= 0.0 && r.y1 <= 300.0, "{r:?}");
        assert_eq!(r.y0, 0.0, "moved up first, then made shorter");
        assert!(m.max_scroll() > 0.0);
        assert!(at_row(&m, 0).is_some());
        assert!(at_row(&m, 39).is_none(), "the last rows are scrolled out");
        // A menu that fits is not touched.
        let m = tall(5, Point::new(100.0, 120.0), window);
        assert_eq!(m.max_scroll(), 0.0);
        assert_eq!(m.rect().y0, 120.0);
    }

    #[test]
    fn the_wheel_scrolls_a_tall_menu_to_both_ends() {
        let window = Size::new(800.0, 300.0);
        let mut m = tall(40, Point::new(100.0, 0.0), window);
        let inside = m.rect().center();
        assert_eq!(m.handle(&wheel(inside, -1.0)), MenuOutcome::Consumed);
        assert!(m.scroll_offset() > 0.0, "a notch down shows later rows");
        let first = m.scroll_offset();
        m.handle(&wheel(inside, -1.0));
        assert!(m.scroll_offset() > first);
        for _ in 0..40 {
            m.handle(&wheel(inside, -1.0));
        }
        assert_eq!(m.scroll_offset(), m.max_scroll());
        assert!(at_row(&m, 39).is_some(), "the last row shows at the end");
        assert!(at_row(&m, 0).is_none());
        for _ in 0..60 {
            m.handle(&wheel(inside, 1.0));
        }
        assert_eq!(m.scroll_offset(), 0.0);
        // A wheel over nothing does nothing.
        m.handle(&wheel(Point::new(700.0, 250.0), -1.0));
        assert_eq!(m.scroll_offset(), 0.0);
    }

    #[test]
    fn a_scrolled_row_is_chosen_where_it_is_drawn() {
        let mut m = tall(40, Point::new(100.0, 0.0), Size::new(800.0, 300.0));
        let inside = m.rect().center();
        for _ in 0..40 {
            m.handle(&wheel(inside, -1.0));
        }
        let p = at_row(&m, 39).unwrap();
        m.handle(&mv(p));
        assert_eq!(
            m.handle(&up(p)),
            MenuOutcome::Run {
                command: "edit.copy".into(),
                arg: Some("39".into())
            }
        );
    }

    #[test]
    fn a_pointer_resting_on_a_strip_scrolls_until_the_end_and_asks_for_frames() {
        let mut m = tall(40, Point::new(100.0, 0.0), Size::new(800.0, 300.0));
        let down_strip = m.lay.down_strip().unwrap().center();
        let up_strip = m.lay.up_strip().unwrap().center();
        assert!(!m.needs_tick());
        m.handle(&mv(up_strip));
        assert!(!m.needs_tick(), "nothing above the first row");
        m.handle(&mv(down_strip));
        assert!(m.needs_tick());
        let mut t = 0.0;
        while m.scroll_offset() < m.max_scroll() {
            t += 1.0 / 60.0;
            assert!(m.tick(t), "scrolls on every frame");
            assert!(t < 10.0, "reaches the end");
        }
        assert!(!m.needs_tick(), "at the end the strip has nothing to do");
        m.handle(&mv(up_strip));
        assert!(m.needs_tick());
        let before = m.scroll_offset();
        t += 0.1;
        m.tick(t);
        assert!(m.scroll_offset() < before);
        // Leaving the window stops it.
        m.handle(&UiEvent::PointerLeave);
        assert!(!m.needs_tick());
    }

    #[test]
    fn arrow_keys_scroll_the_highlighted_row_into_view_and_wrap_around() {
        let mut m = tall(40, Point::new(100.0, 0.0), Size::new(800.0, 300.0));
        press(&mut m, Key::Down);
        assert_eq!(m.scroll_offset(), 0.0);
        for _ in 0..20 {
            press(&mut m, Key::Down);
        }
        let row = at_row(&m, 20).expect("the highlighted row shows whole");
        assert!(m.rect().contains(row));
        assert!(m.scroll_offset() > 0.0);
        press(&mut m, Key::End);
        assert!(at_row(&m, 39).is_some());
        assert_eq!(m.scroll_offset(), m.max_scroll());
        press(&mut m, Key::Home);
        assert_eq!(m.scroll_offset(), 0.0);
        press(&mut m, Key::Up);
        assert_eq!(m.highlighted(), Some("Row 39"), "wraps to the last row");
        assert!(at_row(&m, 39).is_some());
    }

    #[test]
    fn scrolling_a_panel_closes_the_submenu_opened_from_it_and_a_scroll_repaints() {
        let mut entries = many(40);
        entries.insert(
            1,
            Entry::Submenu(SubmenuDef {
                label: "Sub".into(),
                enabled: true,
                entries: many(3),
            }),
        );
        let mut m =
            ContextMenu::from_entries(entries, Point::new(100.0, 0.0), Size::new(800.0, 300.0));
        m.handle(&mv(m.submenu_center("Sub").unwrap()));
        assert_eq!(m.open_path(), ["Sub"]);
        let before = m.look();
        m.handle(&wheel(m.rect().center(), -1.0));
        assert!(m.scroll_offset() > 0.0);
        assert!(m.open_path().is_empty(), "its row moved");
        assert_ne!(m.look(), before, "the shell repaints on a scroll");
    }

    #[test]
    fn a_submenu_near_the_bottom_moves_up_and_a_tall_one_scrolls_in_the_window() {
        let window = Size::new(800.0, 400.0);
        let mut entries = many(2);
        entries.push(Entry::Submenu(SubmenuDef {
            label: "Low".into(),
            enabled: true,
            entries: many(12),
        }));
        entries.push(Entry::Submenu(SubmenuDef {
            label: "Tall".into(),
            enabled: true,
            entries: many(60),
        }));
        let mut m = ContextMenu::from_entries(entries, Point::new(100.0, 300.0), window);
        let root = m.rect();
        assert!(root.y1 <= 400.0, "{root:?}");
        m.handle(&mv(m.submenu_center("Low").unwrap()));
        let low = m.open.as_ref().unwrap().menu.rect();
        assert!(low.y0 >= 0.0 && low.y1 <= 400.0, "stays on screen: {low:?}");
        assert!(low.y0 < m.row_rects()[2].y0, "moved up to fit");
        assert_eq!(m.open.as_ref().unwrap().menu.max_scroll(), 0.0);
        m.handle(&down(m.submenu_center("Tall").unwrap()));
        let tall = &m.open.as_ref().unwrap().menu;
        assert!(tall.rect().y0 >= 0.0 && tall.rect().y1 <= 400.0);
        assert!(tall.max_scroll() > 0.0, "scrolls");
        // The wheel over the submenu scrolls it and leaves the parent where it is.
        let at = tall.rect().center();
        let parent = m.scroll_offset();
        m.handle(&wheel(at, -1.0));
        assert!(m.open.as_ref().unwrap().menu.scroll_offset() > 0.0);
        assert_eq!(m.scroll_offset(), parent);
        assert_eq!(m.open_path(), ["Tall"]);
    }

    #[test]
    fn a_submenu_at_the_right_edge_flips_left_and_a_scrolled_parent_row_places_it() {
        let window = Size::new(500.0, 300.0);
        let mut entries = many(30);
        entries.push(Entry::Submenu(SubmenuDef {
            label: "Late".into(),
            enabled: true,
            entries: many(3),
        }));
        let mut m = ContextMenu::from_entries(entries, Point::new(290.0, 0.0), window);
        let inside = m.rect().center();
        for _ in 0..40 {
            m.handle(&wheel(inside, -1.0));
        }
        let row = m.submenu_center("Late").unwrap();
        m.handle(&mv(row));
        let child = m.open.as_ref().unwrap().menu.rect();
        assert!(
            child.x1 <= m.rect().x0 + 3.0,
            "flipped to the left: {child:?}"
        );
        assert!(child.x0 >= 0.0 && child.y1 <= 300.0 && child.y0 >= 0.0);
        assert!(
            child.y0 <= row.y && row.y <= child.y1,
            "level with its scrolled row"
        );
    }
    #[test]
    fn an_item_with_a_swatch_draws_a_chip_in_its_colour_before_the_label() {
        let item = |label: &str, swatch: Option<[u8; 3]>, enabled: bool| {
            Entry::Item(MenuItemDef {
                command: "edit.copy".into(),
                arg: None,
                label: label.into(),
                shortcut: None,
                enabled,
                checked: None,
                swatch,
            })
        };
        let m = ContextMenu::from_entries(
            vec![
                item("Kick", Some([220, 70, 60]), true),
                item("Pad", None, true),
                item("Muted", Some([60, 120, 220]), false),
            ],
            Point::new(10.0, 10.0),
            Size::new(400.0, 300.0),
        );
        let mut list = crate::DisplayList::new();
        m.paint(&mut list, &Theme::dark());
        let json = list.to_json();
        assert!(
            json.contains("#dc463cff"),
            "the chip of the first row: {json}"
        );
        assert_eq!(
            json.matches("\"rrect\":[").count(),
            3 + 2,
            "the panel (shadow, fill, outline) and two chips"
        );
        assert!(
            json.contains("#3c78dc6e"),
            "a dimmed chip on a disabled row"
        );
    }
}
