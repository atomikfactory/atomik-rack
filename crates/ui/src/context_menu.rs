//! Context menus built from the command registry.
//!
//! A menu is a list of registry command ids with an enabled flag and an optional check. The
//! label comes from the registry and the shortcut text from the active keymap, so a key and a
//! menu item cannot differ. Choosing an item produces a [`MenuOutcome::Run`] with the
//! command id and argument; the shell turns it into a command for the pane the menu belongs
//! to (the same path a key press takes). The widget is pure: it paints through a [`Canvas`],
//! takes [`UiEvent`]s in the coordinates it was opened in and owns no document state.

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

/// One row of a menu description: a registry command or a separator.
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

    /// The item with its enabled flag set.
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

/// A row of an open menu.
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    /// An item.
    Item(MenuItemDef),
    /// A separator line.
    Separator,
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

/// An open context menu.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextMenu {
    entries: Vec<Entry>,
    origin: Point,
    width: f64,
    hover: Option<usize>,
}

/// Estimated width of a text run.
fn text_width(text: &str) -> f64 {
    text.chars().count() as f64 * CHAR_WIDTH
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
        let mut entries: Vec<Entry> = Vec::new();
        for spec in specs {
            match spec {
                Spec::Separator => {
                    // No leading, trailing or doubled separators.
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
            }
        }
        while matches!(entries.last(), Some(Entry::Separator)) {
            entries.pop();
        }
        let width = entries
            .iter()
            .filter_map(|e| match e {
                Entry::Item(i) => Some(
                    text_width(&i.label)
                        + i.shortcut.as_deref().map_or(0.0, |s| text_width(s) + 32.0)
                        + 48.0,
                ),
                Entry::Separator => None,
            })
            .fold(MIN_WIDTH, f64::max);
        let mut menu = ContextMenu {
            entries,
            origin: at,
            width,
            hover: None,
        };
        let size = menu.rect().size();
        menu.origin = Point::new(
            at.x.min(bounds.width - size.width).max(0.0),
            at.y.min(bounds.height - size.height).max(0.0),
        );
        menu
    }

    /// The rows, top to bottom.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The items (separators skipped).
    pub fn items(&self) -> impl Iterator<Item = &MenuItemDef> {
        self.entries.iter().filter_map(|e| match e {
            Entry::Item(i) => Some(i),
            Entry::Separator => None,
        })
    }

    /// The panel rectangle.
    pub fn rect(&self) -> Rect {
        let h: f64 = self
            .entries
            .iter()
            .map(|e| match e {
                Entry::Item(_) => ITEM_HEIGHT,
                Entry::Separator => SEPARATOR_HEIGHT,
            })
            .sum();
        Rect::new(
            self.origin.x,
            self.origin.y,
            self.origin.x + self.width,
            self.origin.y + h + 8.0,
        )
    }

    /// The rectangle of each row (separators included).
    pub fn row_rects(&self) -> Vec<Rect> {
        let mut y = self.origin.y + 4.0;
        self.entries
            .iter()
            .map(|e| {
                let h = match e {
                    Entry::Item(_) => ITEM_HEIGHT,
                    Entry::Separator => SEPARATOR_HEIGHT,
                };
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

    /// The centre of the row of command `command` (tests click it).
    pub fn item_center(&self, command: &str) -> Option<Point> {
        let rows = self.row_rects();
        self.entries.iter().zip(rows).find_map(|(e, r)| match e {
            Entry::Item(i) if i.command == command => Some(r.center()),
            _ => None,
        })
    }

    fn item_at(&self, p: Point) -> Option<usize> {
        let rows = self.row_rects();
        let i = rows.iter().position(|r| r.contains(p))?;
        matches!(self.entries[i], Entry::Item(_)).then_some(i)
    }

    /// Handles an event while the menu is open. A click outside closes it: the caller drops
    /// the menu on [`MenuOutcome::Consumed`] after a press outside [`Self::rect`], on `Run`
    /// and on [`Self::wants_close`].
    pub fn handle(&mut self, event: &UiEvent) -> MenuOutcome {
        match event {
            UiEvent::PointerMove { pos, .. } => {
                let hover = self.item_at(*pos);
                if hover != self.hover {
                    self.hover = hover;
                }
                MenuOutcome::Consumed
            }
            UiEvent::PointerDown { .. } | UiEvent::Wheel { .. } => MenuOutcome::Consumed,
            UiEvent::PointerUp { pos, button, .. } => {
                if *button != PointerButton::Primary {
                    return MenuOutcome::Consumed;
                }
                match self.item_at(*pos).map(|i| &self.entries[i]) {
                    Some(Entry::Item(i)) if i.enabled => MenuOutcome::Run {
                        command: i.command.clone(),
                        arg: i.arg.clone(),
                    },
                    _ => MenuOutcome::Consumed,
                }
            }
            UiEvent::Key { .. } => MenuOutcome::Consumed,
            _ => MenuOutcome::Ignored,
        }
    }

    /// Whether `event` closes the menu without choosing: Escape, or a press outside the
    /// panel.
    pub fn wants_close(&self, event: &UiEvent) -> bool {
        match event {
            UiEvent::Key {
                key: Key::Escape,
                pressed: true,
                ..
            } => true,
            UiEvent::PointerDown { pos, .. } => !self.rect().contains(*pos),
            _ => false,
        }
    }

    /// Paints the panel (logical units, in the coordinates the menu was opened in).
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
            }
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
}
