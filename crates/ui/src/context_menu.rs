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
//! A submenu opens beside its row when the pointer rests on the row (or clicks it) and stays
//! open while the pointer is over it or the rows that opened it; submenus nest to any depth.
//!
//! While a submenu is open, the pointer may cross the other rows of its panel on the way to the
//! submenu: a move inside the triangle from the previous pointer position to the submenu near
//! edge that gets closer to it keeps the submenu, and any other move onto another row only
//! switches after [`SUBMENU_DELAY`] seconds. The delay is counted by [`ContextMenu::tick`], which
//! the shell calls once a frame, so the widget reads no clock.
//!
//! The keys work in and between the panels: Up and Down (wrapping) and Home and End move the one
//! highlight that the pointer moves too, skipping separators and disabled rows; Right or Enter
//! opens a submenu on its first row; Left or Escape closes the submenu; Enter or Space chooses.

use crate::canvas::{Align, Canvas, CanvasExt, TextRun};
use crate::event::{Key, PointerButton, UiEvent};
use crate::keymap::{Keymap, Registry};
use crate::theme::Theme;
use vello::kurbo::{Point, Rect, RoundedRect, Size, Vec2};
use vello::peniko::Color;

/// Height of one item, logical pixels.
pub const ITEM_HEIGHT: f64 = 24.0;
/// Height of a separator.
pub const SEPARATOR_HEIGHT: f64 = 9.0;
/// Approximate advance of one character of UI text at 13 px (layout estimate).
const CHAR_WIDTH: f64 = 7.2;
const MIN_WIDTH: f64 = 200.0;
const FONT: f32 = 13.0;
/// How far a submenu overlaps the panel it opens from.
const OVERLAP: f64 = 2.0;
/// How long the pointer must rest on another row before it replaces the open submenu, seconds.
pub const SUBMENU_DELAY: f64 = 0.28;

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

    /// One panel of `entries` at `at`, moved inside `bounds`.
    fn panel(entries: Vec<Entry>, at: Point, bounds: Size) -> Self {
        let width = panel_width(&entries);
        let mut menu = ContextMenu {
            entries,
            origin: at,
            width,
            hover: None,
            bounds,
            open: None,
            armed: false,
            last: None,
            pending: None,
        };
        let size = menu.rect().size();
        menu.origin = Point::new(
            at.x.min(bounds.width - size.width).max(0.0),
            at.y.min(bounds.height - size.height).max(0.0),
        );
        menu
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
        let h: f64 = self.entries.iter().map(Entry::height).sum();
        Rect::new(
            self.origin.x,
            self.origin.y,
            self.origin.x + self.width,
            self.origin.y + h + 8.0,
        )
    }

    /// The rectangle of each row of the first panel (separators included).
    pub fn row_rects(&self) -> Vec<Rect> {
        let mut y = self.origin.y + 4.0;
        self.entries
            .iter()
            .map(|e| {
                let h = e.height();
                let r = Rect::new(
                    self.origin.x + 4.0,
                    y,
                    self.origin.x + self.width - 4.0,
                    y + h,
                );
                y += h;
                r
            })
            .collect()
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
        let rows = self.row_rects();
        self.entries
            .iter()
            .zip(rows)
            .find_map(|(e, r)| pred(e).then(|| r.center()))
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
        self.row_rects().iter().position(|r| r.contains(p))
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

    /// Whether a pointer switch is waiting for its delay (the caller keeps ticking).
    pub fn has_pending(&self) -> bool {
        self.pending.is_some() || self.open.as_ref().is_some_and(|o| o.menu.has_pending())
    }

    /// Advances the hover delay to `now` (seconds, any monotonic clock): a pointer that rested on
    /// another row for [`SUBMENU_DELAY`] switches to it. Returns whether the menu changed.
    pub fn tick(&mut self, now: f64) -> bool {
        let mut changed = self.open.as_mut().is_some_and(|o| o.menu.tick(now));
        if let Some(p) = &mut self.pending {
            let since = *p.since.get_or_insert(now);
            if now - since >= SUBMENU_DELAY {
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
        let rr = self.row_rects()[row];
        let child_width = panel_width(&s.entries);
        let mut child = Self::panel(s.entries.clone(), Point::new(0.0, rr.y0 - 4.0), self.bounds);
        let right = self.origin.x + self.width - OVERLAP;
        let x = if right + child_width <= self.bounds.width {
            right
        } else {
            (self.origin.x - child_width + OVERLAP).max(0.0)
        };
        child.origin.x = x;
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
            UiEvent::PointerDown { .. } | UiEvent::Wheel { .. } => MenuOutcome::Consumed,
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
        for (i, (entry, r)) in self.entries.iter().zip(self.row_rects()).enumerate() {
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
        if let Some(o) = &self.open {
            o.menu.paint(canvas, theme);
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
}
