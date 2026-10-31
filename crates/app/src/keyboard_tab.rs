//! The Keyboard tab of the settings dialog: every command by
//! area with its chords on this OS, a search field, capture-a-chord rebinding, the conflict
//! dialog, restore one or all, the profile choice, the modifier roles, export and import.
//!
//! The state machine is plain data on top of [`KeymapEditor`] (which owns the rules: conflicts,
//! reserved chords, the diff), so it is tested headlessly through the dialog. Nothing is saved
//! until the dialog's **Apply**; the shell then writes `keymap.json` and swaps the running keymap.

use crate::menubar::text_width;
use atomik_ui::keymap::{
    Chord, Clash, Group, KeymapEditor, Mods, Os, Profile, Rebind, Resolve, Role, Row, RowKey, Slot,
    Source, UserKeymap,
};
use atomik_ui::kurbo::{Point, Rect, RoundedRect, Vec2};
use atomik_ui::peniko::Color;
use atomik_ui::{Align, Canvas, CanvasExt, Key, Modifiers, PointerButton, TextRun, Theme, UiEvent};

pub(crate) const PAD: f64 = 20.0;
pub(crate) const TITLE: f64 = 44.0;
pub(crate) const FOOTER: f64 = 52.0;
const SEARCH_H: f64 = 26.0;
const MESSAGE_H: f64 = 22.0;
const ROW: f64 = 24.0;
const ROW_PITCH: f64 = 26.0;
const HEADER: f64 = 24.0;
const SECTION_GAP: f64 = 10.0;
const SCROLL_STEP: f64 = 40.0;

/// What a handled event asks of the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TabOutcome {
    /// Nothing further.
    Open,
    /// Apply and close (Enter).
    Apply,
    /// Close without changes (Esc with nothing to abort).
    Cancel,
    /// Ask the platform for a file to export to.
    Export,
    /// Ask the platform for a file to import from.
    Import,
    /// Ask the platform for the library folder (the Workflow tab's Choose).
    PickLibraryFolder,
    /// Ask the platform for the scratch folder (the Workflow tab's Choose).
    PickScratchFolder,
}

/// One chord chip of a row.
#[derive(Debug, Clone, PartialEq)]
pub struct Chip {
    /// Where it is.
    pub rect: Rect,
    /// The slot a click edits.
    pub slot: Slot,
    /// The chord text, or `Not set` / `Add`.
    pub text: String,
    /// No chord there yet.
    pub empty: bool,
}

/// One line of the list.
#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    /// A group heading.
    Group {
        /// Where it is.
        rect: Rect,
        /// The group title.
        title: String,
    },
    /// A command and its chords.
    Row {
        /// The whole line.
        rect: Rect,
        /// The row.
        row: Row,
        /// The command text position and string.
        label: (Point, String),
        /// The chord chips (existing chords, up to two, then an add chip).
        chips: Vec<Chip>,
        /// The context text.
        context: (Point, String),
        /// Chords beyond the two shown (the file may hold more).
        more: Option<(Point, String)>,
        /// The reset button (only for the user's changes).
        reset: Option<Rect>,
    },
    /// A heading of the roles section.
    RoleTitle {
        /// Where it is.
        rect: Rect,
    },
    /// A modifier role.
    Role {
        /// The whole line.
        rect: Rect,
        /// The role.
        role: Role,
        /// Its plain label.
        label: (Point, String),
        /// The modifier chip.
        chip: Rect,
        /// The modifiers as this OS shows them.
        text: String,
        /// The user changed it.
        changed: bool,
        /// The reset button (only when changed).
        reset: Option<Rect>,
    },
}

impl Item {
    /// The top edge.
    pub fn top(&self) -> f64 {
        self.rect().y0
    }

    /// The bottom edge.
    pub fn bottom(&self) -> f64 {
        self.rect().y1
    }

    fn rect(&self) -> Rect {
        match self {
            Item::Group { rect, .. }
            | Item::Row { rect, .. }
            | Item::RoleTitle { rect }
            | Item::Role { rect, .. } => *rect,
        }
    }
}

/// The conflict dialog over the tab.
#[derive(Debug, Clone, PartialEq)]
pub struct ConflictLayout {
    /// The box.
    pub panel: Rect,
    /// The question, naming both commands.
    pub text: String,
    /// Swap button.
    pub swap: Rect,
    /// Unassign-the-other button.
    pub unassign: Rect,
    /// The unassign button's text.
    pub unassign_text: String,
    /// Cancel button.
    pub cancel: Rect,
}

/// Where everything of the tab is.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyboardLayout {
    /// The search field.
    pub search: Rect,
    /// The profile chips.
    pub profiles: Vec<(Rect, Profile, &'static str)>,
    /// The scrolling list (items are clipped to it).
    pub list: Rect,
    /// The list lines, already scrolled.
    pub items: Vec<Item>,
    /// The height of all lines (for scrolling).
    pub content_height: f64,
    /// Restore-all button.
    pub restore_all: Rect,
    /// Export button.
    pub export: Rect,
    /// Import button.
    pub import: Rect,
    /// The message line.
    pub message: Option<(Point, String)>,
    /// The conflict dialog, while one is open.
    pub conflict: Option<ConflictLayout>,
}

#[derive(Debug, Clone)]
struct Pending {
    key: RowKey,
    slot: Slot,
    chord: Chord,
    clashes: Vec<Clash>,
}

/// The Keyboard tab state.
#[derive(Debug, Clone)]
pub struct KeyboardTab {
    editor: KeymapEditor,
    search: String,
    scroll: f64,
    learn: Option<(RowKey, Slot)>,
    role_learn: Option<(Role, Mods)>,
    conflict: Option<Pending>,
    message: Option<String>,
    hover: Option<Point>,
}

fn button_width(text: &str) -> f64 {
    text_width(text) + 24.0
}

impl KeyboardTab {
    /// A tab editing `editor`.
    pub fn new(editor: KeymapEditor) -> Self {
        let message = editor.report().summary();
        Self {
            editor,
            search: String::new(),
            scroll: 0.0,
            learn: None,
            role_learn: None,
            conflict: None,
            message,
            hover: None,
        }
    }

    /// The editor (the drafts).
    pub fn editor(&self) -> &KeymapEditor {
        &self.editor
    }

    /// The message line.
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }

    /// Whether a chord is being learned.
    pub fn is_learning(&self) -> bool {
        self.learn.is_some()
    }

    /// Sets the search text (the list shows the commands that match).
    pub fn set_search(&mut self, text: &str) {
        self.search = text.to_string();
        self.scroll = 0.0;
    }

    /// Whether the conflict dialog is open (it takes every click and key).
    pub fn has_conflict(&self) -> bool {
        self.conflict.is_some()
    }

    /// Whether every key must reach the tab as pressed (a chord or a role is being learned),
    /// so the shell must not turn numpad keys into characters.
    pub fn is_capturing(&self) -> bool {
        self.learn.is_some() || self.role_learn.is_some()
    }

    /// Replaces the edited keymap (an import).
    pub fn replace_user(&mut self, user: UserKeymap) {
        self.editor.replace_user(user);
        self.learn = None;
        self.role_learn = None;
        self.conflict = None;
        self.message = self
            .editor
            .report()
            .summary()
            .or_else(|| Some("Imported the keymap".to_string()));
    }

    fn os(&self) -> Os {
        self.editor.os()
    }

    // ------------------------------------------------------------------ layout

    /// Lays the tab out inside `panel`.
    pub fn layout(&self, panel: Rect) -> KeyboardLayout {
        let x0 = panel.x0 + PAD;
        let inner = panel.width() - 2.0 * PAD;
        let top = panel.y0 + TITLE;
        let search = Rect::new(x0, top, x0 + (inner * 0.45).max(120.0), top + SEARCH_H);
        let mut profiles = Vec::new();
        let mut right = x0 + inner;
        for (profile, label) in [
            (Profile::Legacy, "Atomik legacy"),
            (Profile::Standard, "Standard"),
        ] {
            let w = button_width(label);
            profiles.push((
                Rect::new(right - w, top, right, top + SEARCH_H),
                profile,
                label,
            ));
            right -= w + 6.0;
        }
        profiles.reverse();
        let list = Rect::new(
            x0,
            top + SEARCH_H + 10.0,
            x0 + inner,
            (panel.y1 - FOOTER - MESSAGE_H).max(top + SEARCH_H + 11.0),
        );
        let (mut items, content_height) = self.items(inner);
        let max_scroll = (content_height - list.height()).max(0.0);
        let scroll = self.scroll.clamp(0.0, max_scroll);
        let shift = Vec2::new(list.x0, list.y0 - scroll);
        for item in &mut items {
            item.translate(shift);
        }
        let by = panel.y1 - FOOTER + 12.0;
        let mut x = x0;
        let mut next = |label: &str| {
            let w = button_width(label);
            let r = Rect::new(x, by, x + w, by + 28.0);
            x += w + 8.0;
            r
        };
        let restore_all = next("Restore all");
        let export = next("Export");
        let import = next("Import");
        let message = self
            .message
            .clone()
            .map(|m| (Point::new(x0, list.y1 + 15.0), m));
        let conflict = self
            .conflict
            .as_ref()
            .map(|p| self.conflict_layout(panel, p));
        KeyboardLayout {
            search,
            profiles,
            list,
            items,
            content_height,
            restore_all,
            export,
            import,
            message,
            conflict,
        }
    }

    fn items(&self, inner: f64) -> (Vec<Item>, f64) {
        let os = self.os();
        let mut items = Vec::new();
        let mut y = 0.0;
        let mut group: Option<Group> = None;
        let chip_w = inner * 0.19;
        let chip_x = [inner * 0.35, inner * 0.55];
        let ctx_x = inner * 0.76;
        let reset_w = 52.0;
        for row in self.editor.rows(&self.search) {
            if group != Some(row.group) {
                group = Some(row.group);
                if !items.is_empty() {
                    y += SECTION_GAP;
                }
                items.push(Item::Group {
                    rect: Rect::new(0.0, y, inner, y + HEADER),
                    title: row.group.title().to_string(),
                });
                y += HEADER;
            }
            let rect = Rect::new(0.0, y, inner, y + ROW);
            let mut chips: Vec<Chip> = row
                .chords
                .iter()
                .take(2)
                .enumerate()
                .map(|(i, c)| Chip {
                    rect: Rect::new(chip_x[i], y, chip_x[i] + chip_w, y + ROW),
                    slot: Slot::Replace(i),
                    text: c.menu_text(os),
                    empty: false,
                })
                .collect();
            if chips.len() < 2 {
                let i = chips.len();
                chips.push(Chip {
                    rect: Rect::new(chip_x[i], y, chip_x[i] + chip_w, y + ROW),
                    slot: Slot::Add,
                    text: if i == 0 { "Not set" } else { "+ Add" }.to_string(),
                    empty: true,
                });
            }
            let more = (row.chords.len() > 2).then(|| {
                (
                    Point::new(chip_x[1] + chip_w + 4.0, y + 16.5),
                    format!("+{}", row.chords.len() - 2),
                )
            });
            let reset =
                (row.source == Source::User).then(|| Rect::new(inner - reset_w, y, inner, y + ROW));
            items.push(Item::Row {
                rect,
                label: (Point::new(0.0, y + 16.5), row.label.clone()),
                context: (
                    Point::new(ctx_x + if more.is_some() { 12.0 } else { 0.0 }, y + 16.5),
                    row.key.context.name().to_string(),
                ),
                chips,
                more,
                reset,
                row,
            });
            y += ROW_PITCH;
        }
        if self.search.trim().is_empty() {
            if !items.is_empty() {
                y += SECTION_GAP;
            }
            items.push(Item::RoleTitle {
                rect: Rect::new(0.0, y, inner, y + HEADER),
            });
            y += HEADER;
            for role in Role::listed() {
                let Some(binding) = self.editor.roles().binding(role) else {
                    continue;
                };
                let changed = self.editor.user().roles.contains_key(role.id());
                let learning = self.role_learn.is_some_and(|(r, _)| r == role);
                let text = if learning {
                    "Hold modifiers".to_string()
                } else {
                    binding.mods.menu_text(os)
                };
                items.push(Item::Role {
                    rect: Rect::new(0.0, y, inner, y + ROW),
                    role,
                    label: (Point::new(0.0, y + 16.5), role.label().to_string()),
                    chip: Rect::new(chip_x[0], y, chip_x[0] + chip_w, y + ROW),
                    text,
                    changed,
                    reset: changed.then(|| Rect::new(inner - reset_w, y, inner, y + ROW)),
                });
                y += ROW_PITCH;
            }
        }
        (items, y)
    }

    fn conflict_layout(&self, panel: Rect, p: &Pending) -> ConflictLayout {
        let os = self.os();
        let registry = atomik_ui::keymap::Registry::bundled();
        let mine = registry
            .get(&p.key.command)
            .map_or(p.key.command.as_str(), |d| d.label);
        let others: Vec<&str> = p.clashes.iter().map(|c| c.label.as_str()).collect();
        let others = others.join(" and ");
        let text = format!(
            "{} is already used by {others}. Assign it to {mine} anyway?",
            p.chord.menu_text(os)
        );
        let w = (panel.width() - 80.0).clamp(280.0, 520.0);
        let h = 112.0;
        let c = panel.center();
        let box_ = Rect::new(c.x - w / 2.0, c.y - h / 2.0, c.x + w / 2.0, c.y + h / 2.0);
        let by = box_.y1 - 38.0;
        let unassign_text = format!(
            "Unassign {}",
            p.clashes.first().map_or("other", |c| c.label.as_str())
        );
        let widths = [
            button_width("Swap"),
            button_width(&unassign_text),
            button_width("Cancel"),
        ];
        let mut x = box_.x0 + 16.0;
        let mut rect = |w: f64| {
            let r = Rect::new(x, by, x + w, by + 28.0);
            x += w + 8.0;
            r
        };
        ConflictLayout {
            panel: box_,
            text,
            swap: rect(widths[0]),
            unassign: rect(widths[1]),
            unassign_text,
            cancel: rect(widths[2]),
        }
    }

    // ------------------------------------------------------------------ events

    /// Handles an event inside `panel` (the dialog took the buttons and tabs already).
    pub(crate) fn handle(&mut self, event: &UiEvent, panel: Rect) -> TabOutcome {
        match event {
            UiEvent::Key {
                key,
                modifiers,
                pressed: true,
            } => return self.key(key, *modifiers),
            UiEvent::ModifiersChanged { modifiers } => self.modifiers_changed(*modifiers),
            UiEvent::PointerMove { pos, .. } => self.hover = Some(*pos),
            UiEvent::PointerLeave => self.hover = None,
            UiEvent::Wheel { delta, .. } => {
                let l = self.layout(panel);
                let max = (l.content_height - l.list.height()).max(0.0);
                self.scroll = (self.scroll - delta.y * SCROLL_STEP).clamp(0.0, max);
            }
            UiEvent::PointerDown {
                pos,
                button: PointerButton::Primary,
                ..
            } => return self.click(*pos, panel),
            _ => {}
        }
        TabOutcome::Open
    }

    fn key(&mut self, key: &Key, modifiers: Modifiers) -> TabOutcome {
        if self.conflict.is_some() {
            if *key == Key::Escape {
                self.conflict = None;
            }
            return TabOutcome::Open;
        }
        if self.role_learn.is_some() {
            if *key == Key::Escape {
                self.role_learn = None;
            }
            return TabOutcome::Open;
        }
        if let Some((row, slot)) = self.learn.clone() {
            match key {
                Key::Escape => {
                    self.learn = None;
                    self.message = None;
                }
                Key::Backspace if !modifiers.ctrl && !modifiers.meta && !modifiers.alt => {
                    self.learn = None;
                    if let Slot::Replace(i) = slot {
                        self.editor.clear(&row, i);
                        self.message = Some("Binding cleared".to_string());
                    }
                }
                other => {
                    if let Some(chord) = Chord::from_event(other, modifiers, self.os()) {
                        self.learn = None;
                        self.assign(row, slot, chord);
                    }
                }
            }
            return TabOutcome::Open;
        }
        match key {
            Key::Escape => {
                if self.search.is_empty() {
                    return TabOutcome::Cancel;
                }
                self.search.clear();
                self.scroll = 0.0;
            }
            Key::Enter => return TabOutcome::Apply,
            Key::Backspace => {
                self.search.pop();
                self.scroll = 0.0;
            }
            Key::Space => {
                self.search.push(' ');
                self.scroll = 0.0;
            }
            Key::Char(c) if !c.is_control() && !modifiers.ctrl && !modifiers.meta => {
                self.search.push(*c);
                self.scroll = 0.0;
            }
            Key::PageDown => self.scroll += 300.0,
            Key::PageUp => self.scroll = (self.scroll - 300.0).max(0.0),
            _ => {}
        }
        TabOutcome::Open
    }

    fn assign(&mut self, key: RowKey, slot: Slot, chord: Chord) {
        let os = self.os();
        match self.editor.rebind(&key, slot, chord) {
            Rebind::Applied { notes } => {
                let label = self.label_of(&key);
                let mut text = format!("{label} is now {}", chord.menu_text(os));
                for n in notes {
                    text.push_str(". ");
                    text.push_str(&n);
                }
                self.message = Some(text);
            }
            Rebind::Unchanged => self.message = None,
            Rebind::Reserved(why) => {
                self.message = Some(format!("{} is {why}", chord.menu_text(os)));
            }
            Rebind::Conflict(clashes) => {
                self.conflict = Some(Pending {
                    key,
                    slot,
                    chord,
                    clashes,
                });
            }
        }
    }

    fn label_of(&self, key: &RowKey) -> String {
        atomik_ui::keymap::Registry::bundled()
            .get(&key.command)
            .map_or_else(|| key.command.clone(), |d| d.label.to_string())
    }

    fn settle(&mut self, how: Resolve) {
        let Some(p) = self.conflict.take() else {
            return;
        };
        match self.editor.resolve(&p.key, p.slot, p.chord, how) {
            Rebind::Applied { notes } => {
                let mut text = format!(
                    "{} is now {}",
                    self.label_of(&p.key),
                    p.chord.menu_text(self.os())
                );
                for n in notes {
                    text.push_str(". ");
                    text.push_str(&n);
                }
                self.message = Some(text);
            }
            Rebind::Reserved(why) => {
                self.message = Some(format!("{} is {why}", p.chord.menu_text(self.os())));
            }
            Rebind::Unchanged | Rebind::Conflict(_) => {}
        }
    }

    fn modifiers_changed(&mut self, held: Modifiers) {
        let Some((role, seen)) = self.role_learn else {
            return;
        };
        let mac = self.os() == Os::Mac;
        let now = Mods {
            cmd: if mac { held.meta } else { held.ctrl },
            ctrl: mac && held.ctrl,
            alt: held.alt,
            shift: held.shift,
            meta: !mac && held.meta,
        };
        if now.is_empty() {
            self.role_learn = None;
            if seen.is_empty() {
                return;
            }
            self.message = Some(match self.editor.set_role(role, seen) {
                Ok(()) => format!("{} is now {}", role.label(), seen.menu_text(self.os())),
                Err(e) => e,
            });
        } else {
            let merged = Mods {
                cmd: seen.cmd || now.cmd,
                ctrl: seen.ctrl || now.ctrl,
                alt: seen.alt || now.alt,
                shift: seen.shift || now.shift,
                meta: seen.meta || now.meta,
            };
            self.role_learn = Some((role, merged));
        }
    }

    fn click(&mut self, pos: Point, panel: Rect) -> TabOutcome {
        let l = self.layout(panel);
        if let Some(c) = &l.conflict {
            if c.swap.contains(pos) {
                self.settle(Resolve::Swap);
            } else if c.unassign.contains(pos) {
                self.settle(Resolve::UnassignOther);
            } else if c.cancel.contains(pos) {
                self.settle(Resolve::Cancel);
            }
            return TabOutcome::Open;
        }
        // Any click ends a learn that was not on its own chip.
        self.learn = None;
        self.role_learn = None;
        if l.export.contains(pos) {
            return TabOutcome::Export;
        }
        if l.import.contains(pos) {
            return TabOutcome::Import;
        }
        if l.restore_all.contains(pos) {
            self.editor.restore_all();
            self.message = Some("Restored every default".to_string());
            return TabOutcome::Open;
        }
        if let Some((_, profile, _)) = l.profiles.iter().find(|(r, ..)| r.contains(pos)) {
            self.editor.set_profile(*profile);
            self.message = self.editor.report().summary();
            return TabOutcome::Open;
        }
        if !l.list.contains(pos) {
            return TabOutcome::Open;
        }
        for item in &l.items {
            match item {
                Item::Row {
                    row, chips, reset, ..
                } => {
                    if reset.is_some_and(|r| r.contains(pos)) {
                        let conflicts = self.editor.restore(&row.key);
                        self.message = Some(match conflicts.first() {
                            Some(c) => format!(
                                "Restored, but {} is now used twice",
                                c.chord.menu_text(self.os())
                            ),
                            None => format!("Restored {}", row.label),
                        });
                        return TabOutcome::Open;
                    }
                    if let Some(chip) = chips.iter().find(|c| c.rect.contains(pos)) {
                        self.learn = Some((row.key.clone(), chip.slot));
                        self.message = Some(
                            "Press the new keys, Backspace to clear, Esc to cancel".to_string(),
                        );
                        return TabOutcome::Open;
                    }
                }
                Item::Role {
                    role, chip, reset, ..
                } => {
                    if reset.is_some_and(|r| r.contains(pos)) {
                        self.editor.reset_role(*role);
                        self.message = Some(format!("Restored {}", role.label()));
                        return TabOutcome::Open;
                    }
                    if chip.contains(pos) {
                        self.role_learn = Some((*role, Mods::default()));
                        self.message =
                            Some("Hold the modifiers, then let go. Esc cancels".to_string());
                        return TabOutcome::Open;
                    }
                }
                _ => {}
            }
        }
        TabOutcome::Open
    }

    // ------------------------------------------------------------------ painting

    /// Paints the tab inside `panel` (the dialog painted the frame, title and buttons).
    pub fn paint(&self, canvas: &mut dyn Canvas, theme: &Theme, panel: Rect) {
        let l = self.layout(panel);
        // Search field.
        let field = RoundedRect::from_rect(l.search, 5.0);
        canvas.fill_with(field, theme.background);
        canvas.stroke_with(field, 1.0, theme.focus);
        let (text, color) = if self.search.is_empty() {
            ("Search commands".to_string(), theme.text_muted)
        } else {
            (format!("{}|", self.search), theme.text)
        };
        canvas.text(&TextRun::new(
            text,
            Point::new(l.search.x0 + 8.0, l.search.y0 + 17.5),
            13.0,
            color,
        ));
        for (r, profile, label) in &l.profiles {
            let on = self.editor.profile() == *profile;
            let shape = RoundedRect::from_rect(*r, 5.0);
            if on {
                canvas.fill_with(shape, theme.selection);
            } else if self.hover.is_some_and(|p| r.contains(p)) {
                canvas.fill_with(shape, theme.browser_row);
            }
            canvas.stroke_with(shape, 1.0, theme.section_line);
            canvas.text(
                &TextRun::new(
                    *label,
                    Point::new(r.center().x, r.y0 + 17.5),
                    13.0,
                    theme.text,
                )
                .align(Align::Center),
            );
        }
        canvas.push_clip(&l.list.into());
        for item in &l.items {
            self.paint_item(canvas, theme, item);
        }
        canvas.pop_clip();
        if let Some((p, m)) = &l.message {
            canvas.text(&TextRun::new(m.clone(), *p, 11.5, theme.text_muted));
        }
        for (r, label) in [
            (l.restore_all, "Restore all"),
            (l.export, "Export"),
            (l.import, "Import"),
        ] {
            self.paint_button(canvas, theme, r, label, false);
        }
        if let Some(c) = &l.conflict {
            self.paint_conflict(canvas, theme, c, panel);
        }
    }

    fn paint_button(
        &self,
        canvas: &mut dyn Canvas,
        theme: &Theme,
        r: Rect,
        label: &str,
        primary: bool,
    ) {
        let shape = RoundedRect::from_rect(r, 6.0);
        let hovered = self.hover.is_some_and(|p| r.contains(p));
        canvas.fill_with(
            shape,
            if primary {
                theme.focus
            } else if hovered {
                theme.selection
            } else {
                theme.browser_row
            },
        );
        canvas.text(
            &TextRun::new(
                label,
                Point::new(r.center().x, r.y0 + 18.5),
                13.0,
                if primary {
                    theme.background
                } else {
                    theme.text
                },
            )
            .align(Align::Center),
        );
    }

    fn paint_item(&self, canvas: &mut dyn Canvas, theme: &Theme, item: &Item) {
        match item {
            Item::Group { rect, title } => {
                canvas.text(
                    &TextRun::new(
                        title.clone(),
                        Point::new(rect.x0, rect.y0 + 17.0),
                        12.0,
                        theme.text_muted,
                    )
                    .weight(600),
                );
            }
            Item::RoleTitle { rect } => {
                canvas.text(
                    &TextRun::new(
                        "Modifier roles (held with the mouse)",
                        Point::new(rect.x0, rect.y0 + 17.0),
                        12.0,
                        theme.text_muted,
                    )
                    .weight(600),
                );
            }
            Item::Row {
                row,
                label,
                chips,
                context,
                more,
                reset,
                ..
            } => {
                let user = row.source == Source::User;
                canvas.text(&TextRun::new(
                    label.1.clone(),
                    label.0,
                    13.0,
                    if user { theme.focus } else { theme.text },
                ));
                let learning = self
                    .learn
                    .as_ref()
                    .filter(|(k, _)| *k == row.key)
                    .map(|(_, s)| *s);
                for chip in chips {
                    let on = learning == Some(chip.slot);
                    // The spare second chip stays out of the way until the pointer is on it.
                    let spare = chip.empty && chip.slot == Slot::Add && chip.text == "+ Add";
                    if spare && !on && !self.hover.is_some_and(|p| chip.rect.contains(p)) {
                        continue;
                    }
                    self.paint_chip(canvas, theme, chip.rect, &chip.text, chip.empty, on);
                }
                if let Some((p, t)) = more {
                    canvas.text(&TextRun::new(t.clone(), *p, 11.5, theme.text_muted));
                }
                canvas.text(&TextRun::new(
                    context.1.clone(),
                    context.0,
                    11.5,
                    theme.text_muted,
                ));
                if let Some(r) = reset {
                    self.paint_button_small(canvas, theme, *r, "Reset");
                }
            }
            Item::Role {
                label,
                chip,
                text,
                changed,
                reset,
                role,
                ..
            } => {
                canvas.text(&TextRun::new(
                    label.1.clone(),
                    label.0,
                    13.0,
                    if *changed { theme.focus } else { theme.text },
                ));
                let on = self.role_learn.is_some_and(|(r, _)| r == *role);
                self.paint_chip(canvas, theme, *chip, text, false, on);
                if let Some(r) = reset {
                    self.paint_button_small(canvas, theme, *r, "Reset");
                }
            }
        }
    }

    fn paint_chip(
        &self,
        canvas: &mut dyn Canvas,
        theme: &Theme,
        r: Rect,
        text: &str,
        empty: bool,
        learning: bool,
    ) {
        let shape = RoundedRect::from_rect(r, 5.0);
        let hovered = self.hover.is_some_and(|p| r.contains(p));
        if learning {
            canvas.fill_with(shape, theme.selection);
        } else if hovered {
            canvas.fill_with(shape, theme.browser_row);
        }
        canvas.stroke_with(
            shape,
            if learning { 1.6 } else { 1.0 },
            if learning {
                theme.focus
            } else {
                theme.section_line
            },
        );
        let label = if learning && !text.starts_with("Hold") {
            "Press keys"
        } else {
            text
        };
        canvas.text(
            &TextRun::new(
                label,
                Point::new(r.center().x, r.y0 + 16.5),
                12.5,
                if empty && !learning {
                    theme.text_muted
                } else {
                    theme.text
                },
            )
            .align(Align::Center),
        );
    }

    fn paint_button_small(&self, canvas: &mut dyn Canvas, theme: &Theme, r: Rect, label: &str) {
        let shape = RoundedRect::from_rect(r, 5.0);
        if self.hover.is_some_and(|p| r.contains(p)) {
            canvas.fill_with(shape, theme.browser_row);
        }
        canvas.stroke_with(shape, 1.0, theme.section_line);
        canvas.text(
            &TextRun::new(
                label,
                Point::new(r.center().x, r.y0 + 16.5),
                11.5,
                theme.text,
            )
            .align(Align::Center),
        );
    }

    fn paint_conflict(
        &self,
        canvas: &mut dyn Canvas,
        theme: &Theme,
        c: &ConflictLayout,
        panel: Rect,
    ) {
        canvas.fill_with(panel, Color::from_rgba8(0, 0, 0, 110));
        let shape = RoundedRect::from_rect(c.panel, 8.0);
        canvas.fill_with(shape, theme.browser_bg);
        canvas.stroke_with(shape, 1.0, theme.focus);
        canvas.text(
            &TextRun::new(
                "Shortcut conflict",
                Point::new(c.panel.x0 + 16.0, c.panel.y0 + 24.0),
                13.0,
                theme.text,
            )
            .weight(600),
        );
        canvas.text(&TextRun::new(
            c.text.clone(),
            Point::new(c.panel.x0 + 16.0, c.panel.y0 + 46.0),
            12.0,
            theme.text_muted,
        ));
        self.paint_button(canvas, theme, c.swap, "Swap", true);
        self.paint_button(canvas, theme, c.unassign, &c.unassign_text, false);
        self.paint_button(canvas, theme, c.cancel, "Cancel", false);
    }
}

impl Item {
    fn translate(&mut self, d: Vec2) {
        match self {
            Item::Group { rect, .. } | Item::RoleTitle { rect } => *rect = *rect + d,
            Item::Row {
                rect,
                label,
                chips,
                context,
                more,
                reset,
                ..
            } => {
                *rect = *rect + d;
                label.0 += d;
                context.0 += d;
                for c in chips {
                    c.rect = c.rect + d;
                }
                if let Some((p, _)) = more {
                    *p += d;
                }
                if let Some(r) = reset {
                    *r = *r + d;
                }
            }
            Item::Role {
                rect,
                label,
                chip,
                reset,
                ..
            } => {
                *rect = *rect + d;
                label.0 += d;
                *chip = *chip + d;
                if let Some(r) = reset {
                    *r = *r + d;
                }
            }
        }
    }
}
