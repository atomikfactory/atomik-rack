//! The versions panel: keep a named version of the song, and see, protect and restore the
//! versions the project holds.
//!
//! ```text
//! VERSIONS                                                               [ CLOSE ]
//! Name   [ Before the mix                              ]   [ SAVE VERSION ]
//! 2026-11-06 14:03     Before the mix                                     NAMED
//! 2026-11-06 13:40                                                    PROTECTED
//! 2026-11-06 13:12
//! [ RESTORE ]  [ PROTECT ]   Up and Down choose; Enter restores; double-click restores
//! ```
//!
//! A modal view the app places over the window like the song information panel. The app opens it
//! with the versions ([`VersionsView::new`], newest first) and answers its [`UiAction::Versions`]:
//!
//! * `SAVE VERSION` (and Enter in the name) emits [`VersionsAction::Save`] with the name.
//! * `RESTORE` (Enter on the list, a double-click on a row) emits [`VersionsAction::Restore`].
//! * `PROTECT` / `UNPROTECT` emits [`VersionsAction::Protect`] for the chosen automatic version
//!   (a named version is always kept, so the button is off for it).
//! * `CLOSE` emits [`VersionsAction::Close`]; the app also closes the panel on Esc.
//!
//! One of the name and the list has the keys at a time (click it, or Tab). The app replaces the
//! rows after every action with [`VersionsView::set_items`]; the chosen row stays where it is.

use crate::action::UiAction;
use crate::canvas::{Align, Canvas};
use crate::event::{Key, PointerButton, UiEvent};
use crate::paint::fit_label;
use crate::seq_paint::{self as sp, SeqTheme};
use crate::text_field::{FieldColors, TextField};
use crate::theme::{Theme, mix};
use crate::tooltip::Help;
use vello::kurbo::{Affine, Point, Rect};

const PAD: f64 = 14.0;
const HEADER_H: f64 = 34.0;
const NAME_H: f64 = 34.0;
const ROW_H: f64 = 24.0;
/// How many rows the list shows.
pub const VISIBLE_ROWS: usize = 10;
const FOOT_H: f64 = 44.0;
const MARGIN: f64 = 16.0;
const MAX_WIDTH: f64 = 600.0;
const BTN_H: f64 = 22.0;
const LABEL_W: f64 = 50.0;
const SAVE_W: f64 = 110.0;
const WHEN_W: f64 = 132.0;
const FONT: f32 = 10.5;
const LINE_H: f64 = 14.0;
/// The longest name a version takes, in characters.
const MAX_NAME_CHARS: usize = 80;

/// One version in the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionItem {
    /// The number the app answers with.
    pub id: u32,
    /// When it was made, as text.
    pub when: String,
    /// Its name (empty for a version a save kept).
    pub name: String,
    /// A save kept it; the oldest of these are pruned.
    pub automatic: bool,
    /// Pruning never removes it (a named version always is).
    pub protected: bool,
}

/// What the panel asks the app to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VersionsAction {
    /// Keep the song as it is as a version with this name.
    Save(String),
    /// Open the version as the current song.
    Restore(u32),
    /// Protect an automatic version from pruning, or lift that.
    Protect {
        /// The version.
        id: u32,
        /// The new state.
        protected: bool,
    },
    /// Close the panel.
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Name,
    List,
}

/// The versions panel. See the module documentation.
#[derive(Debug)]
pub struct VersionsView {
    st: SeqTheme,
    bounds: Rect,
    scale: f64,
    items: Vec<VersionItem>,
    selected: Option<usize>,
    /// The first row shown.
    scroll: usize,
    name: TextField,
    focus: Focus,
    dragging_name: bool,
    hover: Option<Point>,
    repaint: bool,
}

impl VersionsView {
    /// Switches to `theme` and repaints.
    pub fn set_theme(&mut self, theme: Theme) {
        self.st = SeqTheme::from_theme(&theme);
        self.repaint = true;
    }

    /// A panel listing `items` (newest first), with the keys on the name; call
    /// [`Self::set_bounds`]. The newest version is chosen.
    pub fn new(theme: Theme, items: Vec<VersionItem>) -> Self {
        let selected = (!items.is_empty()).then_some(0);
        Self {
            st: SeqTheme::from_theme(&theme),
            bounds: Rect::new(0.0, 0.0, 900.0, 600.0),
            scale: 1.0,
            items,
            selected,
            scroll: 0,
            name: TextField::new("", MAX_NAME_CHARS, false),
            focus: Focus::Name,
            dragging_name: false,
            hover: None,
            repaint: true,
        }
    }

    /// Places the panel: the area it may use (logical pixels). The panel is centred in it.
    pub fn set_bounds(&mut self, bounds: Rect) {
        if bounds != self.bounds {
            self.bounds = bounds;
            self.repaint = true;
        }
    }

    /// Sets the display scale factor.
    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.1);
        self.repaint = true;
    }

    /// Replaces the rows (newest first). The chosen version stays chosen when it is still there,
    /// else the row at the same place (or the last one); the name is cleared when `clear_name`.
    pub fn set_items(&mut self, items: Vec<VersionItem>, clear_name: bool) {
        let chosen = self.selected_id();
        self.items = items;
        self.selected = match chosen.and_then(|id| self.items.iter().position(|v| v.id == id)) {
            Some(at) => Some(at),
            None => self
                .selected
                .map(|at| at.min(self.items.len().saturating_sub(1)))
                .filter(|_| !self.items.is_empty()),
        };
        if self.selected.is_none() && !self.items.is_empty() {
            self.selected = Some(0);
        }
        if clear_name {
            self.name.set_text("");
        }
        self.keep_selection_visible();
        self.repaint = true;
    }

    /// The versions listed.
    pub fn items(&self) -> &[VersionItem] {
        &self.items
    }

    /// The text of the name.
    pub fn name(&self) -> &str {
        self.name.text()
    }

    /// Puts `text` in the name (for tests and for a suggested name).
    pub fn set_name(&mut self, text: &str) {
        self.name.set_text(text);
        self.repaint = true;
    }

    /// The chosen version.
    pub fn selected_id(&self) -> Option<u32> {
        self.selected
            .and_then(|at| self.items.get(at))
            .map(|v| v.id)
    }

    /// Gives the list the keys (File > Versions opens the panel this way).
    pub fn focus_list(&mut self) {
        self.focus = Focus::List;
        self.repaint = true;
    }

    /// Whether the name (not the list) has the keys.
    pub fn name_has_keys(&self) -> bool {
        self.focus == Focus::Name
    }

    /// True when something changed since the last [`Self::paint`].
    pub fn needs_repaint(&self) -> bool {
        self.repaint
    }

    /// Whether the panel takes typed text (always: it is modal and every key goes to it).
    pub fn is_text_owner(&self) -> bool {
        true
    }

    fn keep_selection_visible(&mut self) {
        let Some(at) = self.selected else {
            self.scroll = 0;
            return;
        };
        if at < self.scroll {
            self.scroll = at;
        } else if at >= self.scroll + VISIBLE_ROWS {
            self.scroll = at + 1 - VISIBLE_ROWS;
        }
        self.scroll = self.scroll.min(self.max_scroll());
    }

    fn max_scroll(&self) -> usize {
        self.items.len().saturating_sub(VISIBLE_ROWS)
    }

    fn choose(&mut self, at: usize) {
        if at < self.items.len() {
            self.selected = Some(at);
            self.keep_selection_visible();
            self.repaint = true;
        }
    }

    // ------------------------------------------------------------------ geometry

    /// The panel box.
    pub fn panel(&self) -> Rect {
        let width = (self.bounds.width() - 2.0 * MARGIN).clamp(1.0, MAX_WIDTH);
        let x0 = self.bounds.x0 + (self.bounds.width() - width) / 2.0;
        let room = (self.bounds.height() - 2.0 * MARGIN).max(1.0);
        let y0 = self.bounds.y0 + MARGIN;
        Rect::new(x0, y0, x0 + width, y0 + Self::height().min(room))
    }

    fn height() -> f64 {
        HEADER_H + NAME_H + 8.0 + ROW_H * VISIBLE_ROWS as f64 + FOOT_H + PAD
    }

    /// The CLOSE button.
    pub fn close_rect(&self) -> Rect {
        let p = self.panel();
        Rect::new(
            p.x1 - PAD - 80.0,
            p.y0 + 6.0,
            p.x1 - PAD,
            p.y0 + 6.0 + BTN_H,
        )
    }

    /// The box of the name (the area that takes a click).
    pub fn name_rect(&self) -> Rect {
        let p = self.panel();
        let y = p.y0 + HEADER_H + 4.0;
        Rect::new(
            p.x0 + PAD + LABEL_W,
            y,
            p.x1 - PAD - SAVE_W - 10.0,
            y + NAME_H - 8.0,
        )
    }

    /// The SAVE VERSION button.
    pub fn save_rect(&self) -> Rect {
        let n = self.name_rect();
        let p = self.panel();
        Rect::new(
            p.x1 - PAD - SAVE_W,
            n.y0 + 1.0,
            p.x1 - PAD,
            n.y0 + 1.0 + BTN_H,
        )
    }

    /// The list (all the visible rows).
    pub fn list_rect(&self) -> Rect {
        let p = self.panel();
        let y = p.y0 + HEADER_H + NAME_H + 8.0;
        Rect::new(p.x0 + PAD, y, p.x1 - PAD, y + ROW_H * VISIBLE_ROWS as f64)
    }

    /// The box of the visible row `slot` (0 is the first one shown).
    pub fn row_rect(&self, slot: usize) -> Rect {
        let l = self.list_rect();
        let y = l.y0 + ROW_H * slot as f64;
        Rect::new(l.x0, y, l.x1, y + ROW_H)
    }

    /// The RESTORE button.
    pub fn restore_rect(&self) -> Rect {
        let l = self.list_rect();
        let y = l.y1 + (FOOT_H - BTN_H) / 2.0;
        Rect::new(l.x0, y, l.x0 + 100.0, y + BTN_H)
    }

    /// The PROTECT / UNPROTECT button.
    pub fn protect_rect(&self) -> Rect {
        let r = self.restore_rect();
        Rect::new(r.x1 + 8.0, r.y0, r.x1 + 8.0 + 110.0, r.y1)
    }

    fn row_at(&self, pos: Point) -> Option<usize> {
        if !self.list_rect().contains(pos) {
            return None;
        }
        let slot = ((pos.y - self.list_rect().y0) / ROW_H) as usize;
        let at = self.scroll + slot;
        (at < self.items.len()).then_some(at)
    }

    fn chosen(&self) -> Option<&VersionItem> {
        self.selected.and_then(|at| self.items.get(at))
    }

    /// Whether PROTECT applies to the chosen version (an automatic one).
    pub fn can_protect(&self) -> bool {
        self.chosen().is_some_and(|v| v.automatic)
    }

    fn protect_label(&self) -> &'static str {
        if self.chosen().is_some_and(|v| v.automatic && v.protected) {
            "UNPROTECT"
        } else {
            "PROTECT"
        }
    }

    // ------------------------------------------------------------------ events

    /// What the button under `pos` does, for its tooltip.
    pub fn help_at(&self, pos: Point) -> Option<Help> {
        let help = |rect: Rect, text: &'static str| Some(Help::new(rect, text));
        if self.close_rect().contains(pos) {
            return help(self.close_rect(), "Close the panel");
        }
        if self.save_rect().contains(pos) {
            return help(
                self.save_rect(),
                "Keep the song as it is now under this name; it is never pruned",
            );
        }
        if self.restore_rect().contains(pos) {
            return help(
                self.restore_rect(),
                "Open the chosen version as the song; the song now is kept as a version first",
            );
        }
        if self.protect_rect().contains(pos) {
            return help(
                self.protect_rect(),
                "Protect an automatic version so that pruning never removes it",
            );
        }
        None
    }

    fn restore(&self) -> Option<UiAction> {
        self.selected_id()
            .map(|id| UiAction::Versions(VersionsAction::Restore(id)))
    }

    fn protect(&self) -> Option<UiAction> {
        let v = self.chosen().filter(|v| v.automatic)?;
        Some(UiAction::Versions(VersionsAction::Protect {
            id: v.id,
            protected: !v.protected,
        }))
    }

    fn save(&self) -> UiAction {
        UiAction::Versions(VersionsAction::Save(self.name.text().to_owned()))
    }

    /// Handles one event and returns what the app must do.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        let mut out = Vec::new();
        match event {
            UiEvent::ScaleChanged(s) => self.set_scale(*s),
            UiEvent::PointerMove { pos, .. } => {
                self.hover = Some(*pos);
                if self.dragging_name {
                    self.name.drag(*pos);
                }
                self.repaint = true;
            }
            UiEvent::PointerLeave => {
                self.hover = None;
                self.repaint = true;
            }
            UiEvent::PointerUp {
                button: PointerButton::Primary,
                ..
            } => self.dragging_name = false,
            UiEvent::Wheel { delta, pos, .. } => {
                if self.list_rect().contains(*pos) && delta.y != 0.0 {
                    let rows = delta.y.abs().ceil() as usize * 2;
                    self.scroll = if delta.y > 0.0 {
                        self.scroll.saturating_sub(rows)
                    } else {
                        (self.scroll + rows).min(self.max_scroll())
                    };
                    self.repaint = true;
                }
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                modifiers,
                clicks,
            } => {
                self.repaint = true;
                if self.close_rect().contains(*pos) {
                    out.push(UiAction::Versions(VersionsAction::Close));
                } else if self.save_rect().contains(*pos) {
                    out.push(self.save());
                } else if self.restore_rect().contains(*pos) {
                    out.extend(self.restore());
                } else if self.protect_rect().contains(*pos) {
                    out.extend(self.protect());
                } else if self.name_rect().contains(*pos) {
                    self.focus = Focus::Name;
                    self.dragging_name = true;
                    self.name.press(*pos, *clicks, modifiers.shift);
                } else if let Some(at) = self.row_at(*pos) {
                    self.focus = Focus::List;
                    self.choose(at);
                    if *clicks >= 2 {
                        out.extend(self.restore());
                    }
                }
            }
            UiEvent::Ime(ime) => {
                if self.focus == Focus::Name && self.name.handle_ime(ime) {
                    self.repaint = true;
                }
            }
            UiEvent::Key {
                key,
                modifiers,
                pressed: true,
            } => {
                self.repaint = true;
                if self.focus == Focus::Name {
                    if self.name.handle_key(key, *modifiers).handled() {
                        return out;
                    }
                    match key {
                        Key::Enter => out.push(self.save()),
                        Key::Tab | Key::Down => self.focus = Focus::List,
                        _ => {}
                    }
                } else {
                    let at = self.selected.unwrap_or(0);
                    match key {
                        Key::Up if at == 0 => self.focus = Focus::Name,
                        Key::Up => self.choose(at - 1),
                        Key::Down => self.choose(at + 1),
                        Key::Home => self.choose(0),
                        Key::End => self.choose(self.items.len().saturating_sub(1)),
                        Key::PageUp => self.choose(at.saturating_sub(VISIBLE_ROWS)),
                        Key::PageDown => {
                            self.choose(
                                (at + VISIBLE_ROWS).min(self.items.len().saturating_sub(1)),
                            );
                        }
                        Key::Enter => out.extend(self.restore()),
                        Key::Tab => self.focus = Focus::Name,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        out
    }

    // ------------------------------------------------------------------ painting

    /// Paints the panel (scaled by the scale factor).
    pub fn paint(&self, c: &mut dyn Canvas) {
        let st = &self.st;
        let t = &st.base;
        let p = self.panel();
        let hovered = |r: Rect| self.hover.is_some_and(|h| r.contains(h));
        c.push_transform(Affine::scale(self.scale));
        c.push_clip(&p.inflate(1.0, 1.0).into());
        sp::rect(c, p, st.background);
        sp::rect(c, Rect::new(p.x0, p.y0, p.x1, p.y0 + HEADER_H), st.header);
        sp::line(c, p.x0, p.y0 + 0.5, p.x1, p.y0 + 0.5, t.highlight);
        sp::line(c, p.x0, p.y1 - 0.5, p.x1, p.y1 - 0.5, st.separator);
        sp::line(c, p.x0 + 0.5, p.y0, p.x0 + 0.5, p.y1, st.separator);
        sp::line(c, p.x1 - 0.5, p.y0, p.x1 - 0.5, p.y1, st.separator);
        sp::label(
            c,
            "VERSIONS",
            p.x0 + PAD,
            p.y0 + 22.0,
            12.0,
            t.text,
            Align::Start,
        );
        let close = self.close_rect();
        sp::button(c, st, close, "CLOSE", None, hovered(close));

        let name = self.name_rect();
        sp::label(
            c,
            "Name",
            p.x0 + PAD,
            name.y0 + 14.0,
            FONT,
            t.text,
            Align::Start,
        );
        let in_name = self.focus == Focus::Name;
        sp::lcd(c, st, name, in_name.then_some(t.focus));
        let colors = FieldColors {
            text: t.text,
            caret: t.text,
            selection: mix(t.lcd, t.focus, 0.5),
        };
        self.name.paint(c, name, FONT, LINE_H, colors, in_name);
        let save = self.save_rect();
        sp::button(c, st, save, "SAVE VERSION", Some(st.loop_on), hovered(save));

        let list = self.list_rect();
        sp::lcd(c, st, list, (!in_name).then_some(t.focus));
        c.push_clip(&list.into());
        for slot in 0..VISIBLE_ROWS {
            let at = self.scroll + slot;
            let Some(v) = self.items.get(at) else { break };
            let row = self.row_rect(slot);
            if self.selected == Some(at) {
                sp::rect(c, row, mix(t.lcd, t.focus, 0.45));
            } else if hovered(row) {
                sp::rect(c, row, mix(t.lcd, t.text, 0.08));
            }
            let y = row.y0 + 16.0;
            sp::label(c, &v.when, row.x0 + 8.0, y, FONT, t.text, Align::Start);
            let mark = match (v.automatic, v.protected) {
                (false, _) => "NAMED",
                (true, true) => "PROTECTED",
                (true, false) => "",
            };
            let mark_w = if mark.is_empty() { 0.0 } else { 78.0 };
            let (shown, color) = if v.name.is_empty() {
                ("Saved automatically", t.text_muted)
            } else {
                (v.name.as_str(), t.text)
            };
            let room = row.width() - 8.0 - WHEN_W - mark_w - 16.0;
            sp::label(
                c,
                &fit_label(shown, room, FONT),
                row.x0 + 8.0 + WHEN_W,
                y,
                FONT,
                color,
                Align::Start,
            );
            if !mark.is_empty() {
                sp::label(c, mark, row.x1 - 8.0, y, 9.0, t.text_muted, Align::End);
            }
        }
        if self.items.is_empty() {
            sp::label(
                c,
                "No versions yet: every save keeps the file it replaces",
                list.x0 + 8.0,
                list.y0 + 18.0,
                FONT,
                t.text_muted,
                Align::Start,
            );
        }
        c.pop_clip();

        let restore = self.restore_rect();
        let has = self.selected.is_some();
        sp::button(
            c,
            st,
            restore,
            "RESTORE",
            has.then_some(st.loop_on),
            has && hovered(restore),
        );
        let protect = self.protect_rect();
        if self.can_protect() {
            sp::button(c, st, protect, self.protect_label(), None, hovered(protect));
        } else {
            // Nothing to protect (no version, or a named one that is always kept).
            sp::button(c, st, protect, "", None, false);
            sp::label(
                c,
                self.protect_label(),
                protect.center().x,
                protect.center().y + 3.5,
                9.5,
                t.text_muted,
                Align::Center,
            );
        }
        sp::label(
            c,
            &fit_label(
                "Enter saves from the name and restores from the list",
                p.x1 - PAD - protect.x1 - 12.0,
                9.5,
            ),
            protect.x1 + 12.0,
            protect.y0 + 15.0,
            9.5,
            t.text_muted,
            Align::Start,
        );
        c.pop_clip();
        c.pop_transform();
    }
}
