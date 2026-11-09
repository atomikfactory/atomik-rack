//! The one-line search box of the side panels (device browser, parameter search, plugin
//! parameters): the shared [`crate::text_field::TextField`] with a fixed length limit and a
//! small API for the panels. It has a caret and a selection; Ctrl/Cmd+A selects the whole text,
//! Ctrl/Cmd+C, X and V copy, cut and paste, and typing over a selection replaces it.

use crate::canvas::Canvas;
use crate::event::{Key, Modifiers};
use crate::text_field::{FieldColors, TextField};
use vello::kurbo::{Point, Rect};

/// Longest search text, in characters.
pub const SEARCH_MAX_CHARS: usize = 128;

/// What a key did to a [`SearchField`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchKey {
    /// Not a field key.
    Ignored,
    /// Used, the text is as it was (a caret or selection move, a copy).
    Moved,
    /// Used, the text changed.
    Edited,
}

/// The text, caret and selection of a search box.
#[derive(Debug, Clone)]
pub struct SearchField {
    edit: TextField,
}

impl Default for SearchField {
    fn default() -> Self {
        Self {
            edit: TextField::new("", SEARCH_MAX_CHARS, false),
        }
    }
}

impl SearchField {
    /// An empty field that takes at most `limit` characters.
    pub fn with_limit(limit: usize) -> Self {
        Self {
            edit: TextField::new("", limit, false),
        }
    }

    /// The text.
    pub fn text(&self) -> &str {
        self.edit.text()
    }

    /// True when there is no text.
    pub fn is_empty(&self) -> bool {
        self.edit.is_empty()
    }

    /// Replaces the text (caret at the end, nothing selected).
    pub fn set_text(&mut self, text: &str) {
        self.edit.set_text(text);
    }

    /// Empties the field.
    pub fn clear(&mut self) {
        self.edit.set_text("");
    }

    /// Selects the whole text.
    pub fn select_all(&mut self) {
        self.edit.select_all();
    }

    /// The selected range as character indices, `None` when nothing is selected.
    pub fn selection(&self) -> Option<(usize, usize)> {
        self.edit.selection()
    }

    /// True when the whole (non-empty) text is selected.
    pub fn is_all_selected(&self) -> bool {
        !self.edit.is_empty() && self.edit.selection() == Some((0, self.edit.len()))
    }

    /// Applies a key press (text, caret moves, delete, and the chords Cmd/Ctrl+A, C, X, V).
    pub fn key(&mut self, key: &Key, mods: Modifiers) -> SearchKey {
        match self.edit.handle_key(key, mods) {
            crate::text_field::FieldKey::Ignored => SearchKey::Ignored,
            crate::text_field::FieldKey::Moved => SearchKey::Moved,
            crate::text_field::FieldKey::Edited => SearchKey::Edited,
        }
    }

    /// Select all, copy, cut or paste by the command id the keymap resolves (`edit.select_all`,
    /// `edit.copy`, `edit.cut`, `edit.paste`). `None` for any other id; `Some(true)` when the
    /// text changed.
    pub fn command(&mut self, id: &str) -> Option<bool> {
        let (key, changed) = match id {
            "edit.select_all" => ('a', false),
            "edit.copy" => ('c', false),
            "edit.cut" => ('x', true),
            "edit.paste" => ('v', true),
            _ => return None,
        };
        let mods = Modifiers::CTRL;
        let before = self.edit.text().to_string();
        self.edit.handle_key(&Key::Char(key), mods);
        Some(changed && self.edit.text() != before)
    }

    /// A press at `pos` places the caret (a double click selects a word).
    pub fn press(&mut self, pos: Point, clicks: u8, extend: bool) {
        self.edit.press(pos, clicks, extend);
    }

    /// Paints the text, the selection and (when `focused`) the caret in `rect`.
    pub fn paint(
        &self,
        c: &mut dyn Canvas,
        rect: Rect,
        size: f32,
        line_h: f64,
        colors: FieldColors,
        focused: bool,
    ) {
        self.edit.paint(c, rect, size, line_h, colors, focused);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn type_str(f: &mut SearchField, s: &str) {
        for c in s.chars() {
            f.key(&Key::Char(c), Modifiers::NONE);
        }
    }

    #[test]
    fn ctrl_a_selects_everything_and_typing_replaces_it() {
        let mut f = SearchField::default();
        type_str(&mut f, "bass");
        assert_eq!(f.key(&Key::Char('a'), Modifiers::CTRL), SearchKey::Moved);
        assert!(f.is_all_selected());
        type_str(&mut f, "x");
        assert_eq!(f.text(), "x");
        assert!(!f.is_all_selected());
    }

    #[test]
    fn backspace_deletes_the_selection() {
        let mut f = SearchField::default();
        type_str(&mut f, "bass");
        f.select_all();
        assert_eq!(f.key(&Key::Backspace, Modifiers::NONE), SearchKey::Edited);
        assert!(f.is_empty());
    }

    #[test]
    fn commands_select_copy_cut_and_paste() {
        let mut f = SearchField::default();
        type_str(&mut f, "bass");
        assert_eq!(f.command("edit.select_all"), Some(false));
        assert!(f.is_all_selected());
        assert_eq!(f.command("edit.cut"), Some(true));
        assert!(f.is_empty());
        assert_eq!(f.command("edit.paste"), Some(true));
        assert_eq!(f.text(), "bass");
        assert_eq!(f.command("edit.duplicate"), None);
    }

    #[test]
    fn the_caret_edits_in_the_middle() {
        let mut f = SearchField::default();
        type_str(&mut f, "bss");
        f.key(&Key::Left, Modifiers::NONE);
        f.key(&Key::Left, Modifiers::NONE);
        type_str(&mut f, "a");
        assert_eq!(f.text(), "bass");
    }

    #[test]
    fn the_text_is_capped() {
        let mut f = SearchField::default();
        type_str(&mut f, &"a".repeat(SEARCH_MAX_CHARS + 5));
        assert_eq!(f.text().chars().count(), SEARCH_MAX_CHARS);
    }
}
