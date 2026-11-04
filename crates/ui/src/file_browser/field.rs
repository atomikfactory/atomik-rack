//! A one-line text field of the browser (search, new tag, rename tag): the shared
//! [`crate::text_field::TextField`] with the Enter and Escape keys the browser reacts to.

use crate::event::{ImeEvent, Key, Modifiers};
use crate::text_field::{FieldColors, TextField as Edit};
use vello::kurbo::{Point, Rect};

/// Longest text a field accepts, in characters.
pub(super) const MAX_CHARS: usize = 128;

/// What a key did to a [`TextField`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FieldEvent {
    /// Not a field key (the text did not change).
    Ignored,
    /// The text changed.
    Edited,
    /// Enter.
    Enter,
    /// Escape.
    Escape,
}

/// The text, caret and selection of a field.
#[derive(Debug, Clone)]
pub(super) struct TextField {
    edit: Edit,
}

impl PartialEq for TextField {
    fn eq(&self, other: &Self) -> bool {
        self.edit.text() == other.edit.text()
            && self.edit.caret() == other.edit.caret()
            && self.edit.selection() == other.edit.selection()
    }
}

impl Default for TextField {
    fn default() -> Self {
        Self::with_text("", false)
    }
}

impl TextField {
    /// A field holding `text`, optionally all selected (typing replaces it).
    pub fn with_text(text: &str, all: bool) -> Self {
        let mut edit = Edit::new(text, MAX_CHARS, false);
        if all {
            edit.select_all();
        }
        Self { edit }
    }

    /// The text.
    pub fn text(&self) -> &str {
        self.edit.text()
    }

    /// True when the whole text is selected.
    #[cfg(test)]
    pub fn is_all_selected(&self) -> bool {
        !self.edit.is_empty() && self.edit.selection() == Some((0, self.edit.len()))
    }

    /// Selects the whole text.
    pub fn select_all(&mut self) {
        self.edit.select_all();
    }

    /// Empties the field; true when it held text.
    pub fn clear(&mut self) -> bool {
        let had = !self.edit.is_empty();
        self.edit.set_text("");
        had
    }

    /// Applies a key press.
    pub fn key(&mut self, key: &Key, mods: Modifiers) -> FieldEvent {
        match key {
            Key::Escape => FieldEvent::Escape,
            Key::Enter => FieldEvent::Enter,
            _ => {
                let before = self.edit.text().to_string();
                self.edit.handle_key(key, mods);
                if self.edit.text() != before {
                    FieldEvent::Edited
                } else {
                    FieldEvent::Ignored
                }
            }
        }
    }

    /// Applies an input-method event; true when the text changed.
    pub fn ime(&mut self, event: &ImeEvent) -> bool {
        let before = self.edit.text().to_string();
        self.edit.handle_ime(event);
        self.edit.text() != before
    }

    /// Whether a composition is in progress.
    pub fn composing(&self) -> bool {
        self.edit.preedit().is_some()
    }

    /// A press at `pos` places the caret (a double click selects a word).
    pub fn press(&mut self, pos: Point, clicks: u8, extend: bool) {
        self.edit.press(pos, clicks, extend);
    }

    /// Paints the text, the selection and (when `focused`) the caret in `rect`.
    pub fn paint(
        &self,
        c: &mut dyn crate::canvas::Canvas,
        rect: Rect,
        size: f32,
        colors: FieldColors,
        focused: bool,
    ) {
        self.edit
            .paint(c, rect, size, f64::from(size) + 6.0, colors, focused);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::ImeEvent;

    fn type_str(f: &mut TextField, s: &str) {
        for c in s.chars() {
            let k = if c == ' ' { Key::Space } else { Key::Char(c) };
            f.key(&k, Modifiers::NONE);
        }
    }

    #[test]
    fn typing_and_backspace() {
        let mut f = TextField::default();
        type_str(&mut f, "kick");
        assert_eq!(f.text(), "kick");
        assert_eq!(f.key(&Key::Backspace, Modifiers::NONE), FieldEvent::Edited);
        assert_eq!(f.text(), "kic");
        assert_eq!(f.key(&Key::Backspace, Modifiers::NONE), FieldEvent::Edited);
        f.clear();
        assert_eq!(f.key(&Key::Backspace, Modifiers::NONE), FieldEvent::Ignored);
    }

    #[test]
    fn ctrl_backspace_deletes_a_word() {
        let mut f = TextField::default();
        type_str(&mut f, "big kick  ");
        f.key(&Key::Backspace, Modifiers::CTRL);
        assert_eq!(f.text(), "big ");
        f.key(&Key::Backspace, Modifiers::CTRL);
        assert_eq!(f.text(), "");
    }

    #[test]
    fn select_all_then_type_replaces() {
        let mut f = TextField::default();
        type_str(&mut f, "kick");
        f.key(&Key::Char('a'), Modifiers::CTRL);
        assert!(f.is_all_selected());
        type_str(&mut f, "s");
        assert_eq!(f.text(), "s");
        assert!(!f.is_all_selected());
        f.key(&Key::Char('A'), Modifiers::CTRL);
        f.key(&Key::Backspace, Modifiers::NONE);
        assert!(f.text().is_empty());
    }

    #[test]
    fn length_is_capped_and_shortcuts_do_not_type() {
        let mut f = TextField::default();
        f.key(&Key::Char('x'), Modifiers::CTRL);
        assert!(f.text().is_empty());
        type_str(&mut f, &"a".repeat(MAX_CHARS + 10));
        assert_eq!(f.text().chars().count(), MAX_CHARS);
    }

    #[test]
    fn enter_and_escape_are_reported() {
        let mut f = TextField::default();
        assert_eq!(f.key(&Key::Enter, Modifiers::NONE), FieldEvent::Enter);
        assert_eq!(f.key(&Key::Escape, Modifiers::NONE), FieldEvent::Escape);
    }

    #[test]
    fn the_caret_moves_and_edits_in_the_middle() {
        let mut f = TextField::default();
        type_str(&mut f, "kik");
        f.key(&Key::Left, Modifiers::NONE);
        type_str(&mut f, "c");
        assert_eq!(f.text(), "kick");
    }

    #[test]
    fn a_composition_commits_its_text() {
        let mut f = TextField::default();
        assert!(!f.ime(&ImeEvent::Preedit {
            text: "ni".into(),
            cursor: None
        }));
        assert!(f.composing());
        assert!(f.ime(&ImeEvent::Commit("\u{4f60}".into())));
        assert_eq!(f.text(), "\u{4f60}");
        assert!(!f.composing());
    }
}
