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
}

/// The submenu open beside a panel: the row that opened it and its panel.
#[derive(Debug, Clone, PartialEq)]
struct Open {
    row: usize,
    menu: ContextMenu,
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
        self.hover = Some(row);
        self.open = Some(Box::new(Open { row, menu: child }));
    }

    /// Handles an event while the menu is open. A click outside closes it: the caller drops
    /// the menu on [`MenuOutcome::Consumed`] after a press outside every panel, on `Run`
    /// and on [`Self::wants_close`].
    pub fn handle(&mut self, event: &UiEvent) -> MenuOutcome {
        match event {
            UiEvent::PointerMove { pos, .. } => {
                if let Some(o) = &mut self.open
                    && o.menu.covers(*pos)
                {
                    self.hover = Some(o.row);
                    return o.menu.handle(event);
                }
                if self.rect().contains(*pos) {
                    let row = self.row_at(*pos);
                    match row.map(|r| (r, &self.entries[r])) {
                        Some((r, Entry::Submenu(s))) if s.enabled => self.open_row(r),
                        _ => {
                            self.open = None;
                            self.hover = row.filter(|r| matches!(self.entries[*r], Entry::Item(_)));
                        }
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
                if *button != PointerButton::Primary {
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
            UiEvent::Key { .. } => MenuOutcome::Consumed,
            _ => MenuOutcome::Ignored,
        }
    }

    /// Whether `event` closes the menu without choosing: Escape, or a press outside every panel.
    pub fn wants_close(&self, event: &UiEvent) -> bool {
        match event {
            UiEvent::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } => true,
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
        assert!(m.open_path().is_empty());
        assert!(m.item_center("edit.cut").is_none());
    }

    #[test]
    fn a_click_on_a_submenu_row_opens_it_and_runs_nothing() {
        let mut m = menu(&nested(), Os::Windows);
        let row = m.submenu_center("Chords").unwrap();
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
}
