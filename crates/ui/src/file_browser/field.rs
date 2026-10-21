//! A one-line text field state (search, new tag, rename tag): the caret is always at the end.

use crate::event::{Key, Modifiers};

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

/// The text and the select-all flag of a field.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct TextField {
    text: String,
    all: bool,
}

impl TextField {
    /// A field holding `text`, optionally all selected (typing replaces it).
    pub fn with_text(text: &str, all: bool) -> Self {
        Self {
            text: text.chars().take(MAX_CHARS).collect(),
            all: all && !text.is_empty(),
        }
    }

    /// The text.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// True when the whole text is selected.
    pub fn is_all_selected(&self) -> bool {
        self.all
    }

    /// Selects the whole text.
    pub fn select_all(&mut self) {
        self.all = !self.text.is_empty();
    }

    /// Empties the field; true when it held text.
    pub fn clear(&mut self) -> bool {
        self.all = false;
        let had = !self.text.is_empty();
        self.text.clear();
        had
    }

    fn insert(&mut self, c: char) -> bool {
        if self.all {
            self.text.clear();
            self.all = false;
        }
        if self.text.chars().count() >= MAX_CHARS {
            return false;
        }
        self.text.push(c);
        true
    }

    fn backspace(&mut self, word: bool) -> bool {
        if self.all {
            return self.clear();
        }
        if self.text.is_empty() {
            return false;
        }
        if word {
            while self.text.ends_with(char::is_whitespace) {
                self.text.pop();
            }
            while self
                .text
                .chars()
                .next_back()
                .is_some_and(|c| !c.is_whitespace())
            {
                self.text.pop();
            }
        } else {
            self.text.pop();
        }
        true
    }

    /// Applies a key press.
    pub fn key(&mut self, key: &Key, mods: Modifiers) -> FieldEvent {
        let edited = |changed: bool| {
            if changed {
                FieldEvent::Edited
            } else {
                FieldEvent::Ignored
            }
        };
        match key {
            Key::Escape => FieldEvent::Escape,
            Key::Enter => FieldEvent::Enter,
            Key::Backspace => edited(self.backspace(mods.command())),
            Key::Delete => edited(self.all && self.clear()),
            Key::Space if !mods.command() => edited(self.insert(' ')),
            Key::Char(c) if mods.command() => {
                if c.eq_ignore_ascii_case(&'a') {
                    self.select_all();
                }
                FieldEvent::Ignored
            }
            Key::Char(c) if !mods.alt && !c.is_control() => edited(self.insert(*c)),
            _ => FieldEvent::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
