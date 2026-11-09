//! The text field: the one place that knows how to edit a line (or a few lines) of text.
//!
//! A view that takes typed text keeps a [`TextField`] per text and forwards the keyboard, the
//! pointer and the input-method events to it:
//!
//! * **Caret and selection.** The caret is a character position (never inside a character); a
//!   selection is the span between the caret and an anchor. Left and Right move by a character,
//!   with Ctrl (Alt on macOS) by a word, Home and End to the line ends (Ctrl+Home and Ctrl+End to
//!   the text ends), Up and Down by a line in a multi-line field, all of them selecting while
//!   Shift is held. Backspace and Delete remove the selection, else a character (a word with
//!   Ctrl). Typing replaces the selection. Ctrl+A (Cmd+A) selects everything.
//! * **Clipboard.** Ctrl+C, Ctrl+X and Ctrl+V (Cmd on macOS; Ctrl+Insert, Shift+Delete and
//!   Shift+Insert too) go through [`crate::clipboard`]. A paste is sanitised like typing: a
//!   single-line field turns line breaks into spaces, control characters are dropped, and the
//!   field's limit holds.
//! * **Input methods.** [`UiEvent::Ime`] events drive [`TextField::handle_ime`]: while the user
//!   composes, the pre-edit text shows inside the field with an underline and is not part of the
//!   text; the commit inserts the final text at the caret.
//! * **Undo.** Each field keeps a small history of its own ([`HISTORY_LIMIT`] steps): Ctrl+Z
//!   (Cmd+Z) undoes, Ctrl+Shift+Z (Cmd+Shift+Z, and Ctrl+Y on Windows and Linux) redoes. Typing
//!   a run of letters, or a run of Backspace or Delete presses, is one step; a paste, a cut, a
//!   word delete, a replaced selection and an input-method commit are one step each. A field
//!   that has the keyboard takes these chords whether or not it has anything to undo.
//! * **Filter.** [`TextField::with_filter`] limits the characters a field takes (digits only,
//!   for instance), for typing, pasting and input methods alike.
//! * **AltGr.** On Windows AltGr arrives as Ctrl+Alt: a character typed with both is text, not a
//!   shortcut.
//!
//! The field draws itself ([`TextField::paint`]) with measured text: the caret and the selection
//! sit where the shaped glyphs are. The pointer ([`TextField::press`], [`TextField::drag`]) maps
//! a position to a character with the widths measured at the last paint.

use crate::canvas::{Align, Canvas, CanvasExt, TextRun, pt};
use crate::event::{ImeEvent, Key, Modifiers};
use std::cell::Cell;
use std::collections::VecDeque;
use vello::kurbo::{Point, Rect};
use vello::peniko::Color;

/// What a field did with a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKey {
    /// Not an editing key (Enter in a one-line field, Tab, Escape, a shortcut): the view's.
    Ignored,
    /// Used: the caret or selection moved.
    Moved,
    /// Used: the text changed.
    Edited,
}

impl FieldKey {
    /// Whether the field used the key.
    pub fn handled(self) -> bool {
        self != FieldKey::Ignored
    }
}

/// The colours a field paints with.
#[derive(Debug, Clone, Copy)]
pub struct FieldColors {
    /// The text.
    pub text: Color,
    /// The caret.
    pub caret: Color,
    /// The selection behind the text.
    pub selection: Color,
}

impl FieldColors {
    /// The colours of a field sitting on an LCD-style box of `theme` (a name or number being
    /// typed over a label): the theme's text, caret and a focus-tinted selection.
    pub fn of(theme: &crate::theme::Theme) -> Self {
        Self {
            text: theme.text,
            caret: theme.text,
            selection: crate::theme::mix(theme.lcd, theme.focus, 0.5),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Preedit {
    text: String,
    /// The composition's own cursor, a byte range of `text`.
    cursor: Option<(usize, usize)>,
}

/// How many undo steps a field keeps.
pub const HISTORY_LIMIT: usize = 50;

/// The command ids a field with the keyboard takes (see [`TextField::command`]).
pub const TEXT_COMMANDS: [&str; 6] = [
    "edit.undo",
    "edit.redo",
    "edit.select_all",
    "edit.copy",
    "edit.cut",
    "edit.paste",
];

/// Whether `id` is one of the [`TEXT_COMMANDS`].
pub fn is_text_command(id: &str) -> bool {
    TEXT_COMMANDS.contains(&id)
}

/// What an edit was, to decide whether it joins the step before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditKind {
    /// One typed character (`true`: a letter or digit, `false`: anything else).
    Typed(bool),
    Backspace,
    Delete,
    /// Never joins another edit.
    Other,
}

/// The text and the selection at one moment of the history.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Snap {
    text: String,
    caret: usize,
    anchor: usize,
}

/// A text being edited. See the module documentation.
#[derive(Debug, Clone)]
pub struct TextField {
    text: String,
    /// Caret, a character index.
    caret: usize,
    /// The other end of the selection (equal to `caret` when nothing is selected).
    anchor: usize,
    limit: usize,
    multiline: bool,
    preedit: Option<Preedit>,
    /// Average advance of a character at the last paint (for the pointer).
    char_width: Cell<f64>,
    /// Where the text started at the last paint (x of the first character, y of the first line
    /// top) and the line height, for the pointer.
    origin: Cell<(f64, f64, f64)>,
    /// First line shown (multi-line fields).
    first_line: Cell<usize>,
    /// The characters the field takes (`None`: any).
    filter: Option<fn(char) -> bool>,
    undo: VecDeque<Snap>,
    redo: VecDeque<Snap>,
    /// The last edit and where the caret was left, so the next one can join it.
    last: Option<(EditKind, usize)>,
}

impl TextField {
    /// A field holding `text` (cut to `limit` characters, controls dropped), caret at the end.
    pub fn new(text: &str, limit: usize, multiline: bool) -> Self {
        let mut f = Self {
            text: String::new(),
            caret: 0,
            anchor: 0,
            limit,
            multiline,
            preedit: None,
            char_width: Cell::new(6.0),
            origin: Cell::new((0.0, 0.0, 14.0)),
            first_line: Cell::new(0),
            filter: None,
            undo: VecDeque::new(),
            redo: VecDeque::new(),
            last: None,
        };
        f.set_text(text);
        f
    }

    /// The field takes only the characters `accept` says yes to (typing, paste, composition and
    /// the text it holds now).
    #[must_use]
    pub fn with_filter(mut self, accept: fn(char) -> bool) -> Self {
        self.filter = Some(accept);
        let text = std::mem::take(&mut self.text);
        self.set_text(&text);
        self
    }

    /// The text (the composition in progress is not part of it).
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Replaces the text; the caret goes to the end and nothing is selected.
    pub fn set_text(&mut self, text: &str) {
        self.text = self.clean(text, 0);
        self.caret = self.len();
        self.anchor = self.caret;
        self.preedit = None;
        self.undo.clear();
        self.redo.clear();
        self.last = None;
    }

    /// The caret, a character index.
    pub fn caret(&self) -> usize {
        self.caret
    }

    /// The selected span as character indices `(from, to)`, `None` when nothing is selected.
    pub fn selection(&self) -> Option<(usize, usize)> {
        (self.caret != self.anchor)
            .then(|| (self.caret.min(self.anchor), self.caret.max(self.anchor)))
    }

    /// The selected text.
    pub fn selected_text(&self) -> Option<&str> {
        let (a, b) = self.selection()?;
        Some(&self.text[self.byte(a)..self.byte(b)])
    }

    /// The composition in progress, if any.
    pub fn preedit(&self) -> Option<&str> {
        self.preedit.as_ref().map(|p| p.text.as_str())
    }

    /// Number of characters.
    pub fn len(&self) -> usize {
        self.text.chars().count()
    }

    /// Whether the text is empty.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Puts the caret at a character index (clamped) and drops the selection.
    pub fn set_caret(&mut self, at: usize) {
        self.caret = at.min(self.len());
        self.anchor = self.caret;
        self.last = None;
    }

    /// Selects everything.
    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.len();
        self.last = None;
    }

    fn byte(&self, char_index: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_index)
            .map_or(self.text.len(), |(b, _)| b)
    }

    /// `text` as the field takes it: no control characters (a one-line field turns line breaks
    /// and tabs into spaces, a multi-line one keeps them), at most `room` characters short of
    /// the limit after `used` characters already there.
    fn clean(&self, text: &str, used: usize) -> String {
        let room = self.limit.saturating_sub(used);
        text.chars()
            .filter_map(|c| match c {
                '\r' => None,
                '\n' | '\t' if self.multiline => Some(c),
                '\n' | '\t' => Some(' '),
                c if c.is_control() => None,
                c => Some(c),
            })
            .filter(|c| self.filter.is_none_or(|accept| accept(*c)))
            .take(room)
            .collect()
    }

    // ------------------------------------------------------------------ editing

    /// Types `text` at the caret, replacing the selection. Returns whether anything changed.
    pub fn insert(&mut self, text: &str) -> bool {
        let mut chars = text.chars();
        let kind = match (chars.next(), chars.next()) {
            (Some(c), None) if !c.is_control() => EditKind::Typed(c.is_alphanumeric()),
            _ => EditKind::Other,
        };
        self.edit(kind, |f| f.insert_raw(text))
    }

    /// Runs one edit and records it in the history: it joins the step before it when `kind`
    /// says it continues the same run at the same place, else it opens a new step.
    fn edit(&mut self, kind: EditKind, change: impl FnOnce(&mut Self) -> bool) -> bool {
        let before = self.snap();
        let joins = kind != EditKind::Other
            && self.selection().is_none()
            && self.last == Some((kind, self.caret));
        if !change(self) {
            return false;
        }
        if !joins {
            self.undo.push_back(before);
            if self.undo.len() > HISTORY_LIMIT {
                self.undo.pop_front();
            }
        }
        self.redo.clear();
        self.last = Some((kind, self.caret));
        true
    }

    fn snap(&self) -> Snap {
        Snap {
            text: self.text.clone(),
            caret: self.caret,
            anchor: self.anchor,
        }
    }

    fn restore(&mut self, snap: Snap) {
        self.text = snap.text;
        self.caret = snap.caret.min(self.len());
        self.anchor = snap.anchor.min(self.len());
        self.preedit = None;
        self.last = None;
    }

    /// Undoes the last step; false when there is none.
    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.undo.pop_back() else {
            return false;
        };
        self.redo.push_back(self.snap());
        self.restore(prev);
        true
    }

    /// Redoes the step the last undo took back; false when there is none.
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop_back() else {
            return false;
        };
        self.undo.push_back(self.snap());
        self.restore(next);
        true
    }

    /// Whether there is a step to undo.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Whether there is a step to redo.
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// How many steps the history holds.
    pub fn history_len(&self) -> usize {
        self.undo.len()
    }

    fn insert_raw(&mut self, text: &str) -> bool {
        let removed = self.delete_selection();
        let used = self.len();
        let clean = self.clean(text, used);
        if clean.is_empty() {
            return removed;
        }
        let at = self.byte(self.caret);
        self.text.insert_str(at, &clean);
        self.caret += clean.chars().count();
        self.anchor = self.caret;
        true
    }

    fn delete_selection(&mut self) -> bool {
        let Some((a, b)) = self.selection() else {
            return false;
        };
        let (ba, bb) = (self.byte(a), self.byte(b));
        self.text.replace_range(ba..bb, "");
        self.caret = a;
        self.anchor = a;
        true
    }

    fn delete_range(&mut self, from: usize, to: usize) -> bool {
        if from >= to {
            return false;
        }
        let (bf, bt) = (self.byte(from), self.byte(to));
        self.text.replace_range(bf..bt, "");
        self.caret = from;
        self.anchor = from;
        true
    }

    /// Backspace: the selection, else the character (or word) before the caret.
    pub fn backspace(&mut self, word: bool) -> bool {
        let kind = if word || self.selection().is_some() {
            EditKind::Other
        } else {
            EditKind::Backspace
        };
        self.edit(kind, |f| {
            if f.delete_selection() {
                return true;
            }
            let from = if word {
                f.word_left(f.caret)
            } else {
                f.caret.saturating_sub(1)
            };
            f.delete_range(from, f.caret)
        })
    }

    /// Delete: the selection, else the character (or word) after the caret.
    pub fn delete(&mut self, word: bool) -> bool {
        let kind = if word || self.selection().is_some() {
            EditKind::Other
        } else {
            EditKind::Delete
        };
        self.edit(kind, |f| {
            if f.delete_selection() {
                return true;
            }
            let to = if word {
                f.word_right(f.caret)
            } else {
                (f.caret + 1).min(f.len())
            };
            f.delete_range(f.caret, to)
        })
    }

    fn chars(&self) -> Vec<char> {
        self.text.chars().collect()
    }

    fn word_left(&self, from: usize) -> usize {
        let c = self.chars();
        let mut i = from.min(c.len());
        while i > 0 && c[i - 1].is_whitespace() {
            i -= 1;
        }
        if i > 0 {
            let alnum = c[i - 1].is_alphanumeric();
            while i > 0 && !c[i - 1].is_whitespace() && c[i - 1].is_alphanumeric() == alnum {
                i -= 1;
            }
        }
        i
    }

    fn word_right(&self, from: usize) -> usize {
        let c = self.chars();
        let mut i = from.min(c.len());
        if i < c.len() && !c[i].is_whitespace() {
            let alnum = c[i].is_alphanumeric();
            while i < c.len() && !c[i].is_whitespace() && c[i].is_alphanumeric() == alnum {
                i += 1;
            }
        }
        while i < c.len() && c[i].is_whitespace() {
            i += 1;
        }
        i
    }

    /// `(line, column)` of a character index (multi-line text; one line otherwise).
    fn line_col(&self, at: usize) -> (usize, usize) {
        let mut line = 0;
        let mut col = 0;
        for (i, c) in self.text.chars().enumerate() {
            if i == at {
                break;
            }
            if c == '\n' {
                line += 1;
                col = 0;
            } else {
                col += 1;
            }
        }
        (line, col)
    }

    fn line_starts(&self) -> Vec<usize> {
        let mut v = vec![0];
        v.extend(
            self.text
                .chars()
                .enumerate()
                .filter(|(_, c)| *c == '\n')
                .map(|(i, _)| i + 1),
        );
        v
    }

    fn line_len(&self, line: usize) -> usize {
        self.text
            .split('\n')
            .nth(line)
            .map_or(0, |l| l.chars().count())
    }

    fn index_of(&self, line: usize, col: usize) -> usize {
        let starts = self.line_starts();
        let line = line.min(starts.len() - 1);
        starts[line] + col.min(self.line_len(line))
    }

    fn move_to(&mut self, to: usize, select: bool) {
        self.last = None;
        self.caret = to.min(self.len());
        if !select {
            self.anchor = self.caret;
        }
    }

    // ------------------------------------------------------------------ clipboard

    /// Copies the selection; false when nothing is selected.
    pub fn copy(&self) -> bool {
        match self.selected_text() {
            Some(t) => {
                crate::clipboard::write(t);
                true
            }
            None => false,
        }
    }

    /// Cuts the selection to the clipboard.
    pub fn cut(&mut self) -> bool {
        self.copy() && self.edit(EditKind::Other, Self::delete_selection)
    }

    /// Pastes the clipboard at the caret.
    pub fn paste(&mut self) -> bool {
        match crate::clipboard::read() {
            Some(t) => self.edit(EditKind::Other, |f| f.insert_raw(&t)),
            None => false,
        }
    }

    // ------------------------------------------------------------------ input

    /// Handles a key press.
    pub fn handle_key(&mut self, key: &Key, m: Modifiers) -> FieldKey {
        if self.preedit.is_some() {
            // The input method owns the keys of a composition: it sends them back as events.
            return FieldKey::Ignored;
        }
        let command = m.command() && !(m.ctrl && m.alt && !m.meta);
        let word = m.ctrl || (m.alt && cfg!(target_os = "macos"));
        let edited = |changed: bool| {
            if changed {
                FieldKey::Edited
            } else {
                FieldKey::Moved
            }
        };
        match key {
            Key::Char(c) if !command => edited(self.insert(&c.to_string())),
            Key::Space if !command => edited(self.insert(" ")),
            Key::Char(c) if command => match c.to_ascii_lowercase() {
                'a' => {
                    self.select_all();
                    FieldKey::Moved
                }
                'c' => {
                    self.copy();
                    FieldKey::Moved
                }
                'x' => edited(self.cut()),
                'v' => edited(self.paste()),
                'z' if m.shift => edited(self.redo()),
                'z' => edited(self.undo()),
                'y' if !cfg!(target_os = "macos") && m.ctrl => edited(self.redo()),
                _ => FieldKey::Ignored,
            },
            Key::Insert if m.ctrl => {
                self.copy();
                FieldKey::Moved
            }
            Key::Insert if m.shift => edited(self.paste()),
            Key::Delete if m.shift => edited(self.cut()),
            Key::Backspace => edited(self.backspace(word)),
            Key::Delete => edited(self.delete(word)),
            Key::Enter if self.multiline && !command => edited(self.insert("\n")),
            Key::Left => {
                let to = match (self.selection(), m.shift) {
                    (Some((a, _)), false) => a,
                    _ if word => self.word_left(self.caret),
                    _ => self.caret.saturating_sub(1),
                };
                self.move_to(to, m.shift);
                FieldKey::Moved
            }
            Key::Right => {
                let to = match (self.selection(), m.shift) {
                    (Some((_, b)), false) => b,
                    _ if word => self.word_right(self.caret),
                    _ => self.caret + 1,
                };
                self.move_to(to, m.shift);
                FieldKey::Moved
            }
            Key::Home => {
                let to = if m.ctrl || !self.multiline {
                    0
                } else {
                    self.index_of(self.line_col(self.caret).0, 0)
                };
                self.move_to(to, m.shift);
                FieldKey::Moved
            }
            Key::End => {
                let to = if m.ctrl || !self.multiline {
                    self.len()
                } else {
                    let line = self.line_col(self.caret).0;
                    self.index_of(line, self.line_len(line))
                };
                self.move_to(to, m.shift);
                FieldKey::Moved
            }
            Key::Up | Key::Down if self.multiline => {
                let (line, col) = self.line_col(self.caret);
                let last = self.line_starts().len() - 1;
                let to = if *key == Key::Up {
                    if line == 0 {
                        0
                    } else {
                        self.index_of(line - 1, col)
                    }
                } else if line == last {
                    self.len()
                } else {
                    self.index_of(line + 1, col)
                };
                self.move_to(to, m.shift);
                FieldKey::Moved
            }
            _ => FieldKey::Ignored,
        }
    }

    /// Runs one of the [`TEXT_COMMANDS`] by its command id, the way the keymap resolves a
    /// primary-modifier chord a view does not see as a key: `None` for any other id.
    pub fn command(&mut self, id: &str) -> Option<FieldKey> {
        let (letter, shift) = match id {
            "edit.undo" => ('z', false),
            "edit.redo" => ('z', true),
            "edit.select_all" => ('a', false),
            "edit.copy" => ('c', false),
            "edit.cut" => ('x', false),
            "edit.paste" => ('v', false),
            _ => return None,
        };
        let mods = Modifiers {
            ctrl: true,
            shift,
            ..Modifiers::NONE
        };
        Some(self.handle_key(&Key::Char(letter), mods))
    }

    /// Handles an input-method event; true when the text or the composition changed.
    pub fn handle_ime(&mut self, event: &ImeEvent) -> bool {
        match event {
            ImeEvent::Enabled => false,
            ImeEvent::Preedit { text, cursor } => {
                let text = self.clean(text, 0);
                if text.is_empty() {
                    return self.preedit.take().is_some();
                }
                // Composing replaces what was selected.
                self.delete_selection();
                let next = Preedit {
                    text,
                    cursor: *cursor,
                };
                let changed = self.preedit.as_ref() != Some(&next);
                self.preedit = Some(next);
                changed
            }
            ImeEvent::Commit(text) => {
                let had = self.preedit.take().is_some();
                self.edit(EditKind::Other, |f| f.insert_raw(text)) || had
            }
            ImeEvent::Disabled => self.preedit.take().is_some(),
        }
    }

    /// A press at `pos` (the position and the click count): places the caret, a double click
    /// selects the word, a triple click the whole text. `extend` (Shift) keeps the anchor.
    pub fn press(&mut self, pos: Point, clicks: u8, extend: bool) {
        let at = self.index_at(pos);
        match clicks {
            0 | 1 => self.move_to(at, extend),
            2 => {
                self.anchor = self.word_left_edge(at);
                self.caret = self.word_right_edge(at);
            }
            _ => self.select_all(),
        }
    }

    /// The pointer moved with the button down: the selection follows.
    pub fn drag(&mut self, pos: Point) {
        let at = self.index_at(pos);
        self.move_to(at, true);
    }

    fn word_left_edge(&self, at: usize) -> usize {
        let c = self.chars();
        let mut i = at.min(c.len());
        let class = |ch: char| ch.is_alphanumeric();
        while i > 0
            && !c[i - 1].is_whitespace()
            && class(c[i - 1]) == c.get(at).is_none_or(|x| class(*x))
        {
            i -= 1;
        }
        i
    }

    fn word_right_edge(&self, at: usize) -> usize {
        let c = self.chars();
        let mut i = at.min(c.len());
        let Some(first) = c.get(i).copied() else {
            return i;
        };
        while i < c.len()
            && !c[i].is_whitespace()
            && c[i].is_alphanumeric() == first.is_alphanumeric()
        {
            i += 1;
        }
        i
    }

    /// The character index a position falls on, by the widths of the last paint.
    pub fn index_at(&self, pos: Point) -> usize {
        let (x0, y0, line_h) = self.origin.get();
        let w = self.char_width.get().max(1.0);
        let line = if self.multiline {
            (((pos.y - y0) / line_h.max(1.0)).floor().max(0.0) as usize) + self.first_line.get()
        } else {
            0
        };
        let col = ((pos.x - x0) / w).round().max(0.0) as usize;
        self.index_of(line, col)
    }

    // ------------------------------------------------------------------ painting

    /// Paints the field in `rect` (the box; the text starts 8 px in) at `size`. `lines` is the
    /// number of lines the box shows; a multi-line field scrolls to keep the caret in view.
    /// `focused` draws the caret and the selection.
    pub fn paint<C: Canvas + ?Sized>(
        &self,
        c: &mut C,
        rect: Rect,
        size: f32,
        line_h: f64,
        colors: FieldColors,
        focused: bool,
    ) {
        let pad = 8.0;
        let shown = self.display();
        let lines: Vec<&str> = shown.text.split('\n').collect();
        let rows = if self.multiline {
            ((rect.height() - 6.0) / line_h).floor().max(1.0) as usize
        } else {
            1
        };
        let (caret_line, _) = self.line_col(self.caret);
        let mut first = self.first_line.get().min(lines.len().saturating_sub(1));
        if caret_line < first {
            first = caret_line;
        } else if caret_line >= first + rows {
            first = caret_line + 1 - rows;
        }
        self.first_line.set(first);
        let measure = |c: &mut C, s: &str| -> f64 {
            c.text_width(&TextRun::new(s, pt(0.0, 0.0), size, colors.text))
        };
        let width = rect.width() - 2.0 * pad;
        // Horizontal scroll of a one-line field: the caret stays inside the box.
        let caret_prefix = |c: &mut C, line: usize, col: usize| {
            let l = lines.get(line).copied().unwrap_or("");
            let p: String = l.chars().take(col).collect();
            measure(c, &p)
        };
        let (dis_caret_line, dis_caret_col) = shown.caret;
        let caret_x = caret_prefix(c, dis_caret_line, dis_caret_col);
        let scroll = if self.multiline {
            0.0
        } else {
            (caret_x - width + 2.0).max(0.0)
        };
        let x0 = rect.x0 + pad - scroll;
        let y0 = rect.y0 + 3.0;
        self.origin.set((x0, y0, line_h));
        let total: usize = lines.iter().map(|l| l.chars().count()).sum();
        if total > 0 {
            let longest = lines
                .iter()
                .max_by_key(|l| l.chars().count())
                .copied()
                .unwrap_or("");
            let n = longest.chars().count().max(1) as f64;
            self.char_width.set(measure(c, longest) / n);
        }
        c.push_clip(&rect.into());
        for (row, line) in lines.iter().skip(first).take(rows).enumerate() {
            let li = first + row;
            let top = y0 + row as f64 * line_h;
            if focused && let Some((a, b)) = shown.selection {
                let (la, ca) = shown.pos(a);
                let (lb, cb) = shown.pos(b);
                if li >= la && li <= lb {
                    let from = if li == la { ca } else { 0 };
                    let to = if li == lb { cb } else { line.chars().count() };
                    let xa = caret_prefix(c, li, from);
                    let xb = caret_prefix(c, li, to);
                    c.fill_with(
                        Rect::new(x0 + xa, top, x0 + xb.max(xa + 1.0), top + line_h),
                        colors.selection,
                    );
                }
            }
            c.text(
                &TextRun::new(*line, pt(x0, top + line_h - 4.0), size, colors.text)
                    .align(Align::Start),
            );
            if let Some((pa, pb)) = shown.preedit.filter(|_| li == dis_caret_line) {
                let xa = caret_prefix(c, li, pa);
                let xb = caret_prefix(c, li, pb);
                c.fill_with(
                    Rect::new(x0 + xa, top + line_h - 2.0, x0 + xb, top + line_h - 1.0),
                    colors.text,
                );
            }
        }
        if focused && dis_caret_line >= first && dis_caret_line < first + rows {
            let top = y0 + (dis_caret_line - first) as f64 * line_h;
            c.fill_with(
                Rect::new(x0 + caret_x, top, x0 + caret_x + 1.0, top + line_h),
                colors.caret,
            );
        }
        c.pop_clip();
    }

    /// The text as painted: the composition inserted at the caret.
    fn display(&self) -> Display {
        let at = self.byte(self.caret);
        let (text, preedit_chars) = match &self.preedit {
            Some(p) => {
                let mut t = self.text.clone();
                t.insert_str(at, &p.text);
                (t, Some(p))
            }
            None => (self.text.clone(), None),
        };
        let pre_len = preedit_chars.map_or(0, |p| p.text.chars().count());
        let mut d = Display {
            text,
            caret: (0, 0),
            selection: None,
            preedit: None,
            starts: Vec::new(),
        };
        d.starts = {
            let mut v = vec![0];
            v.extend(
                d.text
                    .chars()
                    .enumerate()
                    .filter(|(_, c)| *c == '\n')
                    .map(|(i, _)| i + 1),
            );
            v
        };
        match preedit_chars {
            Some(p) => {
                let from = self.caret;
                // The composition's own cursor, else the end of it.
                let cursor_chars = p.cursor.map_or(pre_len, |(_, end)| {
                    p.text[..end.min(p.text.len())].chars().count()
                });
                d.caret = d.pos(from + cursor_chars);
                let (la, ca) = d.pos(from);
                let (_, cb) = d.pos(from + pre_len);
                d.preedit = Some((ca, cb));
                let _ = la;
            }
            None => {
                d.caret = d.pos(self.caret);
                d.selection = self.selection();
            }
        }
        d
    }
}

/// The painted form of a field.
struct Display {
    text: String,
    /// `(line, column)` of the caret.
    caret: (usize, usize),
    /// The selection as character indices.
    selection: Option<(usize, usize)>,
    /// The composition's columns on the caret's line.
    preedit: Option<(usize, usize)>,
    starts: Vec<usize>,
}

impl Display {
    fn pos(&self, at: usize) -> (usize, usize) {
        let line = self.starts.iter().rposition(|s| *s <= at).unwrap_or(0);
        (line, at - self.starts[line])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(text: &str) -> TextField {
        TextField::new(text, 64, false)
    }

    fn key(f: &mut TextField, k: Key, m: Modifiers) -> FieldKey {
        f.handle_key(&k, m)
    }

    const NONE: Modifiers = Modifiers::NONE;
    const SHIFT: Modifiers = Modifiers::SHIFT;
    const CTRL: Modifiers = Modifiers::CTRL;

    #[test]
    fn typing_inserts_at_the_caret_and_replaces_the_selection() {
        let mut f = field("abcd");
        f.set_caret(2);
        key(&mut f, Key::Char('X'), NONE);
        assert_eq!((f.text(), f.caret()), ("abXcd", 3));
        key(&mut f, Key::Home, SHIFT);
        assert_eq!(f.selection(), Some((0, 3)));
        key(&mut f, Key::Char('z'), NONE);
        assert_eq!((f.text(), f.caret(), f.selection()), ("zcd", 1, None));
    }

    #[test]
    fn arrows_move_by_character_and_word_and_a_selection_collapses_to_its_edge() {
        let mut f = field("one two  three");
        key(&mut f, Key::Left, CTRL);
        assert_eq!(f.caret(), 9, "start of 'three'");
        key(&mut f, Key::Left, CTRL);
        assert_eq!(f.caret(), 4);
        key(&mut f, Key::Right, CTRL);
        assert_eq!(f.caret(), 9, "past the word and its spaces");
        f.select_all();
        key(&mut f, Key::Left, NONE);
        assert_eq!((f.caret(), f.selection()), (0, None));
        key(&mut f, Key::End, NONE);
        assert_eq!(f.caret(), f.len());
    }

    #[test]
    fn backspace_and_delete_remove_a_selection_a_character_or_a_word() {
        let mut f = field("alpha beta");
        key(&mut f, Key::Backspace, CTRL);
        assert_eq!(f.text(), "alpha ");
        key(&mut f, Key::Backspace, NONE);
        assert_eq!(f.text(), "alpha");
        f.set_caret(0);
        key(&mut f, Key::Delete, NONE);
        assert_eq!(f.text(), "lpha");
        f.select_all();
        key(&mut f, Key::Delete, NONE);
        assert!(f.is_empty());
    }

    #[test]
    fn characters_are_never_split() {
        let mut f = field("a\u{e9}\u{4e2d}\u{1f3b5}z");
        assert_eq!(f.len(), 5);
        f.set_caret(3);
        key(&mut f, Key::Backspace, NONE);
        assert_eq!(f.text(), "a\u{e9}\u{1f3b5}z");
        key(&mut f, Key::Delete, NONE);
        assert_eq!(f.text(), "a\u{e9}z");
    }

    #[test]
    fn the_limit_holds_for_typing_and_for_a_paste() {
        let mut f = TextField::new("", 5, false);
        crate::clipboard::write("abcdefgh");
        key(&mut f, Key::Char('v'), CTRL);
        assert_eq!(f.text(), "abcde");
        assert!(!f.insert("z"), "at the limit nothing is added");
        // Selecting makes room: the replacement fits again.
        f.select_all();
        assert!(f.insert("xy"));
        assert_eq!(f.text(), "xy");
    }

    #[test]
    fn copy_cut_and_paste_go_through_the_clipboard() {
        let mut f = field("hello world");
        f.set_caret(0);
        key(&mut f, Key::Right, SHIFT);
        key(&mut f, Key::Right, SHIFT);
        key(&mut f, Key::Right, SHIFT);
        key(&mut f, Key::Right, SHIFT);
        key(&mut f, Key::Right, SHIFT);
        key(&mut f, Key::Char('c'), CTRL);
        assert_eq!(crate::clipboard::read().as_deref(), Some("hello"));
        key(&mut f, Key::Char('x'), CTRL);
        assert_eq!(f.text(), " world");
        key(&mut f, Key::End, NONE);
        key(&mut f, Key::Char('v'), CTRL);
        assert_eq!(f.text(), " worldhello");
        key(&mut f, Key::Insert, SHIFT);
        assert_eq!(f.text(), " worldhellohello");
    }

    #[test]
    fn a_paste_is_cleaned_like_typing() {
        let mut one = field("");
        crate::clipboard::write("a\r\nb\tc\u{7}d");
        one.paste();
        assert_eq!(
            one.text(),
            "a b cd",
            "line breaks and tabs are spaces, controls go"
        );
        let mut many = TextField::new("", 64, true);
        many.paste();
        assert_eq!(many.text(), "a\nb\tcd");
    }

    #[test]
    fn altgr_types_a_character_and_a_shortcut_does_not() {
        let mut f = field("");
        let altgr = Modifiers {
            ctrl: true,
            alt: true,
            ..NONE
        };
        assert_eq!(key(&mut f, Key::Char('@'), altgr), FieldKey::Edited);
        assert_eq!(f.text(), "@");
        assert_eq!(key(&mut f, Key::Char('q'), CTRL), FieldKey::Ignored);
        assert_eq!(f.text(), "@", "Ctrl+Q is not text");
    }

    #[test]
    fn keys_a_view_owns_are_not_used() {
        let mut f = field("x");
        for k in [Key::Enter, Key::Tab, Key::Escape, Key::Up, Key::F(2)] {
            assert_eq!(key(&mut f, k, NONE), FieldKey::Ignored);
        }
        let mut m = TextField::new("a\nb", 64, true);
        assert_eq!(key(&mut m, Key::Enter, NONE), FieldKey::Edited);
        assert_eq!(m.text(), "a\nb\n");
    }

    #[test]
    fn multi_line_fields_move_by_line_and_home_end_stay_on_the_line() {
        let mut f = TextField::new("abc\nde\nfghij", 64, true);
        f.set_caret(10);
        key(&mut f, Key::Up, NONE);
        assert_eq!(
            f.line_col(f.caret()),
            (1, 2),
            "the column is clamped to the shorter line"
        );
        key(&mut f, Key::Home, NONE);
        assert_eq!(f.caret(), 4);
        key(&mut f, Key::End, NONE);
        assert_eq!(f.caret(), 6);
        key(&mut f, Key::Down, NONE);
        assert_eq!(f.line_col(f.caret()), (2, 2));
        key(&mut f, Key::Home, CTRL);
        assert_eq!(f.caret(), 0);
    }

    #[test]
    fn a_composition_shows_apart_from_the_text_and_commits_at_the_caret() {
        let mut f = field("ab");
        f.set_caret(1);
        let pre = |t: &str| ImeEvent::Preedit {
            text: t.into(),
            cursor: Some((t.len(), t.len())),
        };
        assert!(f.handle_ime(&pre("\u{306b}")));
        assert_eq!(f.preedit(), Some("\u{306b}"));
        assert_eq!(f.text(), "ab", "the composition is not text yet");
        assert!(f.handle_ime(&pre("\u{306b}\u{307b}")));
        assert_eq!(f.key_ignored_while_composing(), FieldKey::Ignored);
        assert!(f.handle_ime(&ImeEvent::Commit("\u{65e5}\u{672c}".into())));
        assert_eq!(
            (f.text(), f.caret(), f.preedit()),
            ("a\u{65e5}\u{672c}b", 3, None)
        );
        // Cancelling a composition (an empty pre-edit) leaves the text alone.
        f.handle_ime(&pre("x"));
        assert!(f.handle_ime(&pre("")));
        assert_eq!((f.text(), f.preedit()), ("a\u{65e5}\u{672c}b", None));
    }

    #[test]
    fn a_composition_replaces_a_selection_and_the_commit_respects_the_limit() {
        let mut f = TextField::new("abcdef", 6, false);
        f.set_caret(0);
        key(&mut f, Key::Right, SHIFT);
        key(&mut f, Key::Right, SHIFT);
        f.handle_ime(&ImeEvent::Preedit {
            text: "z".into(),
            cursor: None,
        });
        assert_eq!(f.text(), "cdef", "composing replaced the selection");
        f.handle_ime(&ImeEvent::Commit("123456".into()));
        assert_eq!(f.len(), 6, "the limit holds");
    }

    #[test]
    fn a_double_click_selects_a_word_and_a_triple_click_everything() {
        let mut f = field("alpha beta");
        f.origin.set((0.0, 0.0, 14.0));
        f.char_width.set(10.0);
        f.press(Point::new(72.0, 5.0), 2, false);
        assert_eq!(f.selected_text(), Some("beta"));
        f.press(Point::new(5.0, 5.0), 3, false);
        assert_eq!(f.selected_text(), Some("alpha beta"));
        f.press(Point::new(30.0, 5.0), 1, false);
        assert_eq!((f.caret(), f.selection()), (3, None));
        f.drag(Point::new(70.0, 5.0));
        assert_eq!(f.selection(), Some((3, 7)));
    }

    fn type_str(f: &mut TextField, s: &str) {
        for c in s.chars() {
            let k = if c == ' ' { Key::Space } else { Key::Char(c) };
            key(f, k, NONE);
        }
    }

    const CTRL_SHIFT: Modifiers = Modifiers {
        ctrl: true,
        shift: true,
        ..Modifiers::NONE
    };

    #[test]
    fn undo_and_redo_walk_the_history() {
        let mut f = field("");
        type_str(&mut f, "abc");
        assert_eq!(f.text(), "abc");
        assert_eq!(key(&mut f, Key::Char('z'), CTRL), FieldKey::Edited);
        assert_eq!(f.text(), "", "a run of letters is one step");
        assert_eq!(key(&mut f, Key::Char('z'), CTRL), FieldKey::Moved, "nothing left, still used");
        assert_eq!(key(&mut f, Key::Char('z'), CTRL_SHIFT), FieldKey::Edited);
        assert_eq!(f.text(), "abc");
        assert_eq!(f.caret(), 3, "the caret comes back with the text");
        key(&mut f, Key::Char('z'), CTRL);
        assert_eq!(key(&mut f, Key::Char('y'), CTRL), FieldKey::Edited);
        assert_eq!(f.text(), if cfg!(target_os = "macos") { "" } else { "abc" });
    }

    #[test]
    fn typing_joins_a_word_and_a_space_or_a_move_starts_a_new_step() {
        let mut f = field("");
        type_str(&mut f, "big kick");
        assert_eq!(f.history_len(), 3, "big, the space, kick");
        f.undo();
        assert_eq!(f.text(), "big ");
        f.undo();
        assert_eq!(f.text(), "big");
        f.undo();
        assert_eq!(f.text(), "");
        let mut f = field("");
        type_str(&mut f, "ab");
        key(&mut f, Key::Left, NONE);
        key(&mut f, Key::Right, NONE);
        type_str(&mut f, "cd");
        assert_eq!(f.history_len(), 2, "moving the caret ends the run");
        f.undo();
        assert_eq!(f.text(), "ab");
    }

    #[test]
    fn a_run_of_backspaces_is_one_step_and_a_paste_or_a_replaced_selection_is_its_own() {
        let mut f = field("hello");
        key(&mut f, Key::Backspace, NONE);
        key(&mut f, Key::Backspace, NONE);
        key(&mut f, Key::Backspace, NONE);
        assert_eq!(f.text(), "he");
        assert_eq!(f.history_len(), 1);
        f.undo();
        assert_eq!(f.text(), "hello");
        f.select_all();
        type_str(&mut f, "x");
        type_str(&mut f, "y");
        assert_eq!(f.text(), "xy");
        f.undo();
        assert_eq!(f.text(), "hello", "the replaced selection and the letters after it are one");
        key(&mut f, Key::End, NONE);
        crate::clipboard::write("PASTE");
        key(&mut f, Key::Char('v'), CTRL);
        assert_eq!(f.text(), "helloPASTE");
        type_str(&mut f, "!");
        f.undo();
        assert_eq!(f.text(), "helloPASTE", "a paste is a step of its own");
        f.undo();
        assert_eq!(f.text(), "hello");
    }

    #[test]
    fn a_new_edit_drops_the_redo_steps_and_set_text_starts_a_new_history() {
        let mut f = field("");
        type_str(&mut f, "one");
        f.undo();
        assert!(f.can_redo());
        type_str(&mut f, "two");
        assert!(!f.can_redo());
        assert!(!f.redo());
        f.set_text("fresh");
        assert!(!f.can_undo());
        assert!(!f.undo());
        assert_eq!(f.text(), "fresh");
    }

    #[test]
    fn the_history_is_bounded() {
        let mut f = TextField::new("", 500, false);
        for i in 0..(HISTORY_LIMIT + 20) {
            // Alternating a letter and a space makes one step per character.
            type_str(&mut f, if i % 2 == 0 { "a" } else { " " });
        }
        assert_eq!(f.history_len(), HISTORY_LIMIT);
        let mut steps = 0;
        while f.undo() {
            steps += 1;
        }
        assert_eq!(steps, HISTORY_LIMIT);
        assert_eq!(f.len(), 20, "the oldest steps are gone");
    }

    #[test]
    fn a_filter_limits_typing_paste_and_composition() {
        let digits = |c: char| c.is_ascii_digit();
        let mut f = TextField::new("12a3", 8, false).with_filter(digits);
        assert_eq!(f.text(), "123", "the text it holds is filtered too");
        type_str(&mut f, "4x5-");
        assert_eq!(f.text(), "12345");
        crate::clipboard::write("6 7a8\n9");
        key(&mut f, Key::Char('v'), CTRL);
        assert_eq!(f.text(), "12345678", "filtered, and the limit holds");
        f.handle_ime(&ImeEvent::Commit("zz".into()));
        assert_eq!(f.text(), "12345678");
        let mut g = TextField::new("", 8, false).with_filter(digits);
        assert_eq!(key(&mut g, Key::Char('q'), NONE), FieldKey::Moved, "a refused letter is still used");
        assert!(g.text().is_empty());
        assert!(!g.can_undo(), "a refused letter is no step");
    }

    #[test]
    fn commands_run_by_id_like_the_chords() {
        let mut f = field("");
        type_str(&mut f, "hi");
        assert_eq!(f.command("edit.select_all"), Some(FieldKey::Moved));
        assert_eq!(f.selection(), Some((0, 2)));
        assert_eq!(f.command("edit.cut"), Some(FieldKey::Edited));
        assert_eq!(f.text(), "");
        assert_eq!(f.command("edit.undo"), Some(FieldKey::Edited));
        assert_eq!(f.text(), "hi");
        assert_eq!(f.command("edit.redo"), Some(FieldKey::Edited));
        assert_eq!(f.text(), "");
        assert_eq!(f.command("edit.duplicate"), None);
        assert!(is_text_command("edit.undo") && !is_text_command("edit.delete"));
    }

    impl TextField {
        fn key_ignored_while_composing(&mut self) -> FieldKey {
            self.handle_key(&Key::Char('q'), NONE)
        }
    }
}
