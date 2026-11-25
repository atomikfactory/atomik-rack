//! The versions panel: keep a named version of the song, and see, protect and restore the
//! versions the project holds.
//!
//! ```text
//! VERSIONS                                                               [ CLOSE ]
//! Name   [ Before the mix                              ]   [ SAVE VERSION ]
//! 2026-11-06 14:03     Before the mix                                     NAMED
//! 2026-11-06 13:40                                                    PROTECTED
//! 2026-11-06 13:12
//! [ RESTORE ] [ PROTECT ] [ COMPARE ] [ RENAME ] [ DELETE ]
//! Up and Down choose; Enter restores; C compares; F2 renames; Delete deletes
//! ```
//!
//! A modal view the app places over the window like the song information panel. The app opens it
//! with the versions ([`VersionsView::new`], newest first) and answers its [`UiAction::Versions`]:
//!
//! * `SAVE VERSION` (and Enter in the name) emits [`VersionsAction::Save`] with the name.
//! * `RESTORE` (Enter on the list, a double-click on a row) emits [`VersionsAction::Restore`].
//! * `PROTECT` / `UNPROTECT` emits [`VersionsAction::Protect`] for the chosen automatic version
//!   (a named version is always kept, so the button is off for it).
//! * `COMPARE` (the C key) emits [`VersionsAction::Compare`]: the chosen version against the
//!   version marked with Shift (a Shift-click, or Shift with Up and Down, which marks the row the
//!   choice leaves), or against the song now when none is marked. The app answers with
//!   [`VersionsView::show_report`]: the list gives way to a scrollable text of what differs, and
//!   BACK (or Esc, see [`VersionsView::close_report`]) returns to the list.
//! * `RENAME` (F2) puts the chosen version's name in the name field, whose button becomes RENAME;
//!   Enter or that button emits [`VersionsAction::Rename`] (an empty name clears the name), Esc
//!   cancels. Any version can be renamed.
//! * `DELETE` (the Delete key on the list) asks "Delete version <label>? This cannot be undone." in
//!   place of the name and the buttons (DELETE or Enter confirms, KEEP or Esc cancels); the answer
//!   emits [`VersionsAction::Delete`]. An automatic version that is protected is not deleted until
//!   it is unprotected: the button is off and its tooltip says why.
//! * A right-click on a row chooses it and opens a menu of the same actions.
//! * `CLOSE` emits [`VersionsAction::Close`]; the app also closes the panel on Esc (which first
//!   leaves the menu, the question, the rename or the comparison, see [`VersionsView::back`]).
//!
//! One of the name and the list has the keys at a time (click it, or Tab). The app replaces the
//! rows after every action with [`VersionsView::set_items`]; the chosen row stays where it is.

use crate::action::UiAction;
use crate::canvas::{Align, Canvas};
use crate::context_menu::{ContextMenu, Entry, MenuItemDef, MenuOutcome};
use crate::event::{Key, PointerButton, UiEvent};
use crate::i18n::tf;
use crate::paint::fit_label;
use crate::seq_paint::{self as sp, SeqTheme};
use crate::text_field::{FieldColors, TextField};
use crate::theme::{Theme, mix};
use crate::tooltip::Help;
use vello::kurbo::{Affine, Point, Rect, Size};

const PAD: f64 = 14.0;
const HEADER_H: f64 = 34.0;
const NAME_H: f64 = 34.0;
const ROW_H: f64 = 24.0;
/// How many rows the list shows.
pub const VISIBLE_ROWS: usize = 10;
const FOOT_H: f64 = 62.0;
const MARGIN: f64 = 16.0;
const MAX_WIDTH: f64 = 600.0;
const BTN_H: f64 = 22.0;
const LABEL_W: f64 = 50.0;
const REPORT_LINE_H: f64 = 16.0;
/// How many lines of a comparison the panel shows.
pub const REPORT_LINES: usize = 15;
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
    /// Summarise what differs between the version `id` and `with` (another version), or the
    /// song as it is now when `with` is `None`.
    Compare {
        /// The chosen version.
        id: u32,
        /// The marked version, if any.
        with: Option<u32>,
    },
    /// Give the version a new name (empty clears it).
    Rename {
        /// The version.
        id: u32,
        /// The new name.
        name: String,
    },
    /// Delete the version (the panel has asked already).
    Delete(u32),
    /// Close the panel.
    Close,
}

/// The commands of the row menu.
const MENU_RESTORE: &str = "versions.restore";
const MENU_COMPARE: &str = "versions.compare";
const MENU_RENAME: &str = "versions.rename";
const MENU_PROTECT: &str = "versions.protect";
const MENU_DELETE: &str = "versions.delete";

/// Why an automatic version that is protected cannot be deleted.
const PROTECTED_HELP: &str = "A protected version cannot be deleted: unprotect it first";

/// A comparison the panel shows in place of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Report {
    /// The two sides, as `before -> after`.
    title: String,
    lines: Vec<String>,
    scroll: usize,
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
    /// The second version of a comparison (Shift), by id.
    mark: Option<u32>,
    /// The comparison shown instead of the list, if any.
    report: Option<Report>,
    /// The first row shown.
    scroll: usize,
    name: TextField,
    focus: Focus,
    /// The version whose name the name field is changing.
    renaming: Option<u32>,
    /// The version the panel asks about deleting.
    confirm: Option<u32>,
    /// The menu of a row, while it is open.
    menu: Option<ContextMenu>,
    /// Opened by File > Save Version: titled for naming, and closed once the version is kept.
    save_only: bool,
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
            mark: None,
            report: None,
            scroll: 0,
            name: TextField::new("", MAX_NAME_CHARS, false),
            focus: Focus::Name,
            renaming: None,
            confirm: None,
            menu: None,
            save_only: false,
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
            self.renaming = None;
        }
        if self
            .renaming
            .is_some_and(|id| !self.items.iter().any(|v| v.id == id))
        {
            self.renaming = None;
            self.name.set_text("");
        }
        // A question about a version that is gone (or a menu over rows that changed) ends.
        if self
            .confirm
            .is_some_and(|id| !self.items.iter().any(|v| v.id == id))
        {
            self.confirm = None;
        }
        self.menu = None;
        if let Some(m) = self.mark
            && !self.items.iter().any(|v| v.id == m)
        {
            self.mark = None;
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

    /// The version marked as the second side of a comparison.
    pub fn marked_id(&self) -> Option<u32> {
        self.mark
    }

    /// Shows a comparison in place of the list: `before` and `after` name its two sides and
    /// `lines` is the text (see [`atomik_model::DocDiff::lines`]).
    pub fn show_report(&mut self, before: &str, after: &str, lines: Vec<String>) {
        self.report = Some(Report {
            title: format!("{before}  ->  {after}"),
            lines,
            scroll: 0,
        });
        self.repaint = true;
    }

    /// Whether a comparison is shown.
    pub fn report_open(&self) -> bool {
        self.report.is_some()
    }

    /// Leaves the comparison for the list; false when none was shown.
    pub fn close_report(&mut self) -> bool {
        let was = self.report.take().is_some();
        self.repaint |= was;
        was
    }

    /// What Esc leaves first: the open menu, else the question about a delete, else the rename,
    /// else the comparison. False when none of them was open (the app then closes the panel).
    pub fn back(&mut self) -> bool {
        if self.menu.take().is_some() {
            self.repaint = true;
            return true;
        }
        if self.confirm.take().is_some() {
            self.repaint = true;
            return true;
        }
        if self.renaming.is_some() {
            self.cancel_rename();
            return true;
        }
        self.close_report()
    }

    /// The version whose name the name field is changing.
    pub fn renaming_id(&self) -> Option<u32> {
        self.renaming
    }

    /// The version the panel asks about deleting.
    pub fn confirming_id(&self) -> Option<u32> {
        self.confirm
    }

    /// Whether the menu of a row is open.
    pub fn menu_open(&self) -> bool {
        self.menu.is_some()
    }

    /// The menu of a row, while it is open (for tests).
    pub fn menu(&self) -> Option<&ContextMenu> {
        self.menu.as_ref()
    }

    /// Puts the name of the chosen version in the name field and gives it the keys; Enter or
    /// RENAME then emits [`VersionsAction::Rename`]. Does nothing without a chosen version.
    pub fn begin_rename(&mut self) {
        let Some(v) = self.chosen() else { return };
        let (id, name) = (v.id, v.name.clone());
        self.menu = None;
        self.confirm = None;
        self.name.set_text(&name);
        self.renaming = Some(id);
        self.focus = Focus::Name;
        self.repaint = true;
    }

    /// Leaves the rename without changing anything and empties the name field.
    pub fn cancel_rename(&mut self) {
        if self.renaming.take().is_some() {
            self.name.set_text("");
            self.repaint = true;
        }
    }

    /// Asks whether to delete the chosen version, unless it is protected (see
    /// [`Self::can_delete`]).
    pub fn ask_delete(&mut self) {
        self.menu = None;
        if self.can_delete() {
            self.confirm = self.selected_id();
            self.repaint = true;
        }
    }

    /// Whether DELETE applies to the chosen version: there is one and it is not a protected
    /// automatic version (a named version is deleted on request).
    pub fn can_delete(&self) -> bool {
        self.chosen().is_some_and(|v| !(v.automatic && v.protected))
    }

    /// The question about deleting the version `id`.
    fn delete_question(&self, id: u32) -> String {
        let label = self
            .items
            .iter()
            .find(|v| v.id == id)
            .map_or_else(String::new, |v| {
                if v.name.is_empty() {
                    v.when.clone()
                } else {
                    format!("{} {}", v.when, v.name)
                }
            });
        tf(
            "Delete version {label}? This cannot be undone.",
            &[("label", &label)],
        )
    }

    /// The lines of the comparison shown, if any.
    pub fn report_lines(&self) -> Option<&[String]> {
        self.report.as_ref().map(|r| r.lines.as_slice())
    }

    /// The first line of the comparison that is shown.
    pub fn report_scroll(&self) -> usize {
        self.report.as_ref().map_or(0, |r| r.scroll)
    }

    fn scroll_report(&mut self, to: impl FnOnce(usize, usize) -> usize) {
        if let Some(r) = &mut self.report {
            let max = r.lines.len().saturating_sub(REPORT_LINES);
            r.scroll = to(r.scroll, max).min(max);
            self.repaint = true;
        }
    }

    /// Makes this the File > Save Version prompt: titled for naming, with the keys on the name.
    /// The app closes it once the version is kept.
    pub fn set_save_only(&mut self) {
        self.save_only = true;
        self.focus = Focus::Name;
        self.repaint = true;
    }

    /// Whether the panel was opened to name and keep a version (not to browse them).
    pub fn is_save_only(&self) -> bool {
        self.save_only
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
            // A rename belongs to the version it started on.
            if self.renaming.is_some() && self.renaming != self.selected_id() {
                self.cancel_rename();
            }
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
        let y = l.y1 + 8.0;
        Rect::new(l.x0, y, l.x0 + 100.0, y + BTN_H)
    }

    /// The PROTECT / UNPROTECT button.
    pub fn protect_rect(&self) -> Rect {
        let r = self.restore_rect();
        Rect::new(r.x1 + 8.0, r.y0, r.x1 + 8.0 + 110.0, r.y1)
    }

    /// The COMPARE button (BACK while a comparison is shown, at the place of RESTORE).
    pub fn compare_rect(&self) -> Rect {
        let r = self.protect_rect();
        Rect::new(r.x1 + 8.0, r.y0, r.x1 + 8.0 + 100.0, r.y1)
    }

    /// The RENAME button.
    pub fn rename_rect(&self) -> Rect {
        let r = self.compare_rect();
        Rect::new(r.x1 + 8.0, r.y0, r.x1 + 8.0 + 90.0, r.y1)
    }

    /// The DELETE button.
    pub fn delete_rect(&self) -> Rect {
        let r = self.rename_rect();
        Rect::new(r.x1 + 8.0, r.y0, r.x1 + 8.0 + 90.0, r.y1)
    }

    /// The BACK button of a comparison.
    pub fn back_rect(&self) -> Rect {
        self.restore_rect()
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
        if self.report.is_some() {
            if self.back_rect().contains(pos) {
                return help(self.back_rect(), "Back to the list of versions");
            }
            return None;
        }
        if self.confirm.is_some() {
            if self.restore_rect().contains(pos) {
                return help(self.restore_rect(), "Delete the version for good");
            }
            if self.protect_rect().contains(pos) {
                return help(self.protect_rect(), "Keep the version");
            }
            return None;
        }
        if self.save_rect().contains(pos) {
            return if self.renaming.is_some() {
                help(
                    self.save_rect(),
                    "Give the version this name; an empty name clears it",
                )
            } else {
                help(
                    self.save_rect(),
                    "Keep the song as it is now under this name; it is never pruned",
                )
            };
        }
        if self.rename_rect().contains(pos) {
            return help(
                self.rename_rect(),
                "Change the name of the chosen version (F2); an empty name clears it",
            );
        }
        if self.delete_rect().contains(pos) {
            let text = if self.chosen().is_some_and(|v| v.automatic && v.protected) {
                PROTECTED_HELP
            } else {
                "Delete the chosen version for good (Delete)"
            };
            return help(self.delete_rect(), text);
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
        if self.compare_rect().contains(pos) {
            return help(
                self.compare_rect(),
                "What differs from the song now (Shift-click marks a second version to compare with)",
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

    fn compare(&self) -> Option<UiAction> {
        let id = self.selected_id()?;
        let with = self.mark.filter(|m| *m != id);
        Some(UiAction::Versions(VersionsAction::Compare { id, with }))
    }

    /// Chooses row `at`; with `mark` the row the choice leaves is marked for a comparison first
    /// (unless it is the one marked already).
    fn choose_marking(&mut self, at: usize) {
        if let Some(from) = self.selected_id() {
            self.mark.get_or_insert(from);
        }
        self.choose(at);
        if self.mark == self.selected_id() {
            self.mark = None;
        }
    }

    /// SAVE VERSION, or RENAME while a name is being changed.
    fn save(&mut self) -> UiAction {
        let name = self.name.text().to_owned();
        match self.renaming {
            Some(id) => UiAction::Versions(VersionsAction::Rename { id, name }),
            None => UiAction::Versions(VersionsAction::Save(name)),
        }
    }

    /// Opens the menu of the chosen version at `at`.
    fn open_menu(&mut self, at: Point) {
        let Some(v) = self.chosen() else { return };
        let (automatic, protected) = (v.automatic, v.protected);
        let item = |command: &str, label: &str, enabled: bool| {
            Entry::Item(MenuItemDef {
                command: command.to_owned(),
                arg: None,
                label: label.to_owned(),
                shortcut: None,
                enabled,
                checked: None,
                swatch: None,
            })
        };
        let entries = vec![
            item(MENU_RESTORE, "Restore", true),
            item(MENU_COMPARE, "Compare", true),
            item(MENU_RENAME, "Rename", true),
            item(
                MENU_PROTECT,
                if automatic && protected {
                    "Unprotect"
                } else {
                    "Protect"
                },
                automatic,
            ),
            Entry::Separator,
            item(MENU_DELETE, "Delete", self.can_delete()),
        ];
        let size = Size::new(self.bounds.x1, self.bounds.y1);
        self.menu = Some(ContextMenu::from_entries(entries, at, size));
        self.repaint = true;
    }

    /// The action of a menu command.
    fn run_menu(&mut self, command: &str, out: &mut Vec<UiAction>) {
        match command {
            MENU_RESTORE => out.extend(self.restore()),
            MENU_COMPARE => out.extend(self.compare()),
            MENU_RENAME => self.begin_rename(),
            MENU_PROTECT => out.extend(self.protect()),
            MENU_DELETE => self.ask_delete(),
            _ => {}
        }
    }

    /// What the panel does with an event while it asks about a delete: DELETE (or Enter) answers
    /// yes, KEEP (or Esc, see [`Self::back`]) no, and nothing else acts.
    fn handle_confirm(&mut self, id: u32, event: &UiEvent, out: &mut Vec<UiAction>) {
        self.repaint = true;
        match event {
            UiEvent::ScaleChanged(s) => self.set_scale(*s),
            UiEvent::PointerMove { pos, .. } => self.hover = Some(*pos),
            UiEvent::PointerLeave => self.hover = None,
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => {
                if self.close_rect().contains(*pos) {
                    out.push(UiAction::Versions(VersionsAction::Close));
                } else if self.restore_rect().contains(*pos) {
                    self.confirm = None;
                    out.push(UiAction::Versions(VersionsAction::Delete(id)));
                } else if self.protect_rect().contains(*pos) {
                    self.confirm = None;
                }
            }
            UiEvent::Key {
                key: Key::Enter,
                pressed: true,
                ..
            } => {
                self.confirm = None;
                out.push(UiAction::Versions(VersionsAction::Delete(id)));
            }
            _ => {}
        }
    }

    /// Handles one event and returns what the app must do.
    pub fn handle(&mut self, event: &UiEvent) -> Vec<UiAction> {
        let mut out = Vec::new();
        if let Some(id) = self.confirm {
            self.handle_confirm(id, event, &mut out);
            return out;
        }
        if let Some(menu) = &mut self.menu {
            self.repaint = true;
            if menu.wants_close(event) {
                self.menu = None;
                // A press outside also lands on what is under it.
                if !matches!(event, UiEvent::PointerDown { .. }) {
                    return out;
                }
            } else {
                if let MenuOutcome::Run { command, .. } = menu.handle(event) {
                    self.menu = None;
                    self.run_menu(&command, &mut out);
                }
                return out;
            }
        }
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
            UiEvent::Wheel { delta, pos, .. } if self.report.is_some() => {
                if self.list_rect().contains(*pos) && delta.y != 0.0 {
                    let lines = delta.y.abs().ceil() as usize * 3;
                    if delta.y > 0.0 {
                        self.scroll_report(|at, _| at.saturating_sub(lines));
                    } else {
                        self.scroll_report(|at, _| at + lines);
                    }
                }
            }
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
                button: PointerButton::Secondary,
                ..
            } => {
                self.repaint = true;
                if self.report.is_none()
                    && let Some(at) = self.row_at(*pos)
                {
                    self.focus = Focus::List;
                    self.choose(at);
                    self.open_menu(*pos);
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
                } else if self.report.is_some() {
                    if self.back_rect().contains(*pos) {
                        self.report = None;
                    }
                } else if self.save_rect().contains(*pos) {
                    out.push(self.save());
                } else if self.rename_rect().contains(*pos) {
                    self.begin_rename();
                } else if self.delete_rect().contains(*pos) {
                    self.ask_delete();
                } else if self.restore_rect().contains(*pos) {
                    out.extend(self.restore());
                } else if self.protect_rect().contains(*pos) {
                    out.extend(self.protect());
                } else if self.compare_rect().contains(*pos) {
                    out.extend(self.compare());
                } else if self.name_rect().contains(*pos) {
                    self.focus = Focus::Name;
                    self.dragging_name = true;
                    self.name.press(*pos, *clicks, modifiers.shift);
                } else if let Some(at) = self.row_at(*pos) {
                    self.focus = Focus::List;
                    if modifiers.shift {
                        self.choose_marking(at);
                    } else {
                        self.choose(at);
                    }
                    if *clicks >= 2 && !modifiers.shift {
                        out.extend(self.restore());
                    }
                }
            }
            UiEvent::Ime(ime) => {
                if self.report.is_none() && self.focus == Focus::Name && self.name.handle_ime(ime) {
                    self.repaint = true;
                }
            }
            UiEvent::Key {
                key,
                modifiers,
                pressed: true,
            } => {
                self.repaint = true;
                if self.report.is_some() {
                    match key {
                        Key::Up => self.scroll_report(|at, _| at.saturating_sub(1)),
                        Key::Down => self.scroll_report(|at, _| at + 1),
                        Key::PageUp => self.scroll_report(|at, _| at.saturating_sub(REPORT_LINES)),
                        Key::PageDown => self.scroll_report(|at, _| at + REPORT_LINES),
                        Key::Home => self.scroll_report(|_, _| 0),
                        Key::End => self.scroll_report(|_, max| max),
                        Key::Enter | Key::Backspace => self.report = None,
                        _ => {}
                    }
                    return out;
                }
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
                    let last = self.items.len().saturating_sub(1);
                    match key {
                        Key::Up if modifiers.shift && at > 0 => self.choose_marking(at - 1),
                        Key::Down if modifiers.shift && at < last => self.choose_marking(at + 1),
                        Key::Up if at == 0 => self.focus = Focus::Name,
                        Key::Up => self.choose(at - 1),
                        Key::Down => self.choose(at + 1),
                        Key::Char('c' | 'C')
                            if !modifiers.ctrl && !modifiers.alt && !modifiers.meta =>
                        {
                            out.extend(self.compare());
                        }
                        Key::F(2) => self.begin_rename(),
                        Key::Delete => self.ask_delete(),
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

    /// Paints a comparison in place of the name and the list.
    fn paint_report(&self, c: &mut dyn Canvas, report: &Report, hovered: impl Fn(Rect) -> bool) {
        let st = &self.st;
        let t = &st.base;
        let p = self.panel();
        let name = self.name_rect();
        let head = Rect::new(p.x0 + PAD, name.y0, p.x1 - PAD, name.y1);
        sp::label(
            c,
            &fit_label(&report.title, head.width(), FONT),
            head.x0,
            head.y0 + 14.0,
            FONT,
            t.text,
            Align::Start,
        );
        let list = self.list_rect();
        sp::lcd(c, st, list, Some(t.focus));
        c.push_clip(&list.into());
        for (slot, line) in report
            .lines
            .iter()
            .skip(report.scroll)
            .take(REPORT_LINES)
            .enumerate()
        {
            let y = list.y0 + REPORT_LINE_H * (slot as f64 + 1.0) - 4.0;
            let text = line.trim_start();
            let color = if text.starts_with('+') {
                st.loop_on
            } else if text.starts_with('-') {
                mix(t.text, t.focus, 0.35)
            } else if !line.starts_with(' ') && !line.is_empty() {
                t.text_muted
            } else {
                t.text
            };
            sp::label(
                c,
                &fit_label(line, list.width() - 16.0, FONT),
                list.x0 + 8.0,
                y,
                FONT,
                color,
                Align::Start,
            );
        }
        c.pop_clip();
        let hidden = report.lines.len().saturating_sub(REPORT_LINES);
        let back = self.back_rect();
        sp::button(c, st, back, "BACK", Some(st.loop_on), hovered(back));
        let hint = if hidden > 0 {
            format!(
                "Lines {}-{} of {}; the wheel and the arrows scroll",
                report.scroll + 1,
                (report.scroll + REPORT_LINES).min(report.lines.len()),
                report.lines.len()
            )
        } else {
            "Esc or Enter goes back to the list".to_owned()
        };
        sp::label(
            c,
            &fit_label(&hint, p.x1 - PAD - back.x1 - 12.0, 9.5),
            back.x1 + 12.0,
            back.y0 + 15.0,
            9.5,
            t.text_muted,
            Align::Start,
        );
    }

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
            if self.save_only {
                "SAVE VERSION"
            } else {
                "VERSIONS"
            },
            p.x0 + PAD,
            p.y0 + 22.0,
            12.0,
            t.text,
            Align::Start,
        );
        let close = self.close_rect();
        sp::button(c, st, close, "CLOSE", None, hovered(close));
        if let Some(report) = &self.report {
            self.paint_report(c, report, hovered);
            c.pop_clip();
            c.pop_transform();
            return;
        }

        let name = self.name_rect();
        let in_name = self.focus == Focus::Name;
        if let Some(id) = self.confirm {
            // The question takes the place of the name.
            let head = Rect::new(p.x0 + PAD, name.y0, p.x1 - PAD, name.y1);
            sp::label(
                c,
                &fit_label(&self.delete_question(id), head.width(), FONT),
                head.x0,
                head.y0 + 14.0,
                FONT,
                t.text,
                Align::Start,
            );
        } else {
            sp::label(
                c,
                if self.renaming.is_some() {
                    "Rename"
                } else {
                    "Name"
                },
                p.x0 + PAD,
                name.y0 + 14.0,
                FONT,
                t.text,
                Align::Start,
            );
            sp::lcd(c, st, name, in_name.then_some(t.focus));
            let colors = FieldColors {
                text: t.text,
                caret: t.text,
                selection: mix(t.lcd, t.focus, 0.5),
            };
            self.name.paint(c, name, FONT, LINE_H, colors, in_name);
            let save = self.save_rect();
            let label = if self.renaming.is_some() {
                "RENAME"
            } else {
                "SAVE VERSION"
            };
            sp::button(c, st, save, label, Some(st.loop_on), hovered(save));
        }

        let list = self.list_rect();
        sp::lcd(c, st, list, (!in_name).then_some(t.focus));
        c.push_clip(&list.into());
        for slot in 0..VISIBLE_ROWS {
            let at = self.scroll + slot;
            let Some(v) = self.items.get(at) else { break };
            let row = self.row_rect(slot);
            if self.selected == Some(at) {
                sp::rect(c, row, mix(t.lcd, t.focus, 0.45));
            } else if self.mark == Some(v.id) {
                sp::rect(c, row, mix(t.lcd, t.focus, 0.2));
            } else if hovered(row) {
                sp::rect(c, row, mix(t.lcd, t.text, 0.08));
            }
            let y = row.y0 + 16.0;
            sp::label(c, &v.when, row.x0 + 8.0, y, FONT, t.text, Align::Start);
            let mark = match (self.mark == Some(v.id), v.automatic, v.protected) {
                (true, _, _) => "COMPARE WITH",
                (_, false, _) => "NAMED",
                (_, true, true) => "PROTECTED",
                (_, true, false) => "",
            };
            let mark_w = if mark.is_empty() { 0.0 } else { 96.0 };
            let (shown, color) = if v.name.is_empty() {
                let shown = if v.automatic {
                    "Saved automatically"
                } else {
                    "Unnamed version"
                };
                (shown, t.text_muted)
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
        let protect = self.protect_rect();
        if self.confirm.is_some() {
            // The answers to the question take the place of the buttons.
            sp::button(c, st, restore, "DELETE", Some(st.loop_on), hovered(restore));
            sp::button(c, st, protect, "KEEP", None, hovered(protect));
            self.paint_hint(c, "Enter deletes; Esc keeps the version");
            c.pop_clip();
            if let Some(menu) = &self.menu {
                menu.paint(c, t);
            }
            c.pop_transform();
            return;
        }
        let has = self.selected.is_some();
        sp::button(
            c,
            st,
            restore,
            "RESTORE",
            has.then_some(st.loop_on),
            has && hovered(restore),
        );
        if self.can_protect() {
            sp::button(c, st, protect, self.protect_label(), None, hovered(protect));
        } else {
            // Nothing to protect (no version, or a named one that is always kept).
            self.paint_off(c, protect, self.protect_label());
        }
        let compare = self.compare_rect();
        sp::button(c, st, compare, "COMPARE", None, has && hovered(compare));
        let rename = self.rename_rect();
        sp::button(c, st, rename, "RENAME", None, has && hovered(rename));
        let delete = self.delete_rect();
        if self.can_delete() {
            sp::button(c, st, delete, "DELETE", None, hovered(delete));
        } else {
            self.paint_off(c, delete, "DELETE");
        }
        self.paint_hint(
            c,
            "C compares (Shift marks a second version); F2 renames; Delete deletes",
        );
        c.pop_clip();
        if let Some(menu) = &self.menu {
            menu.paint(c, t);
        }
        c.pop_transform();
    }

    /// A button that does nothing now: drawn flat with a muted label.
    fn paint_off(&self, c: &mut dyn Canvas, r: Rect, label: &str) {
        sp::button(c, &self.st, r, "", None, false);
        sp::label(
            c,
            label,
            r.center().x,
            r.center().y + 3.5,
            9.5,
            self.st.base.text_muted,
            Align::Center,
        );
    }

    /// The line of hints under the buttons.
    fn paint_hint(&self, c: &mut dyn Canvas, text: &str) {
        let p = self.panel();
        let r = self.restore_rect();
        sp::label(
            c,
            &fit_label(text, p.x1 - 2.0 * PAD, 9.5),
            r.x0,
            r.y1 + 16.0,
            9.5,
            self.st.base.text_muted,
            Align::Start,
        );
    }
}
