//! The interface languages: a lookup from the English text to the text of the chosen language.
//!
//! The English text is the key. [`t`] takes it and gives back the same text or, when another
//! language is chosen and has an entry for it, the entry. The lookup never allocates: the
//! entries live in a catalogue parsed once from a file of `crates/ui/locales/` that is part of the
//! program ([`include_str!`]), so a missing file or a missing entry can only fall back to English.
//!
//! # The file format
//!
//! One entry per line, `English text => Texte français`. A line that starts with `#` and a blank
//! line are comments; `\n` and `\\` are the two escapes. A text with values in it names them in
//! braces (`Track {name} not found`), and the translation spells the same names (the order may
//! differ): [`tf`] puts the values in. A text that changes with a count has a singular and a
//! plural entry: [`tn`] picks the one the language uses for the count.
//!
//! # What is translated
//!
//! Every [`crate::TextRun`] goes through [`t`] when it is made, so a label written as a literal
//! (or returned by a `fn label() -> &'static str`) is translated where it is drawn, in every
//! panel, with no call of its own. The words of the user (song, track, clip and file names) are
//! drawn with [`crate::TextRun::verbatim`], which never looks anything up. Text built from parts
//! (`format!`) is translated with [`tf`] or [`tn`] where it is built, and the texts that are not
//! drawn by the program (the native menus, the message boxes) call [`t`] themselves. The shell
//! applies a language change at once: the menus, the panels and the caches are rebuilt for it.
//!
//! # Tests
//!
//! The tests never read the language of the system. The process starts in English; a test that
//! wants another language runs inside [`with_language`], which holds for that thread only, and
//! [`with_misses`] lists the English texts that were looked up and had no entry.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

/// A language of the interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Lang {
    /// English (the texts of the program itself).
    English,
    /// French.
    French,
}

impl Lang {
    /// Every language, in the order the setting lists them.
    pub const ALL: [Lang; 2] = [Lang::English, Lang::French];

    /// The language code written in the settings file.
    pub fn code(self) -> &'static str {
        match self {
            Lang::English => "en",
            Lang::French => "fr",
        }
    }

    /// The name of the language in the language itself (never translated).
    pub fn name(self) -> &'static str {
        match self {
            Lang::English => "English",
            Lang::French => "Fran\u{e7}ais",
        }
    }

    /// The language of a code (`fr`), or of a locale (`fr-CA`, `fr_FR.UTF-8`).
    pub fn from_code(code: &str) -> Option<Lang> {
        let lang = code.trim().split(['-', '_', '.', '@']).next()?;
        Lang::ALL
            .into_iter()
            .find(|l| l.code().eq_ignore_ascii_case(lang))
    }

    fn index(self) -> u8 {
        match self {
            Lang::English => 0,
            Lang::French => 1,
        }
    }

    fn from_index(i: u8) -> Lang {
        match i {
            1 => Lang::French,
            _ => Lang::English,
        }
    }

    /// The text of the language file, `None` for English (the keys themselves).
    fn source(self) -> Option<&'static str> {
        match self {
            Lang::English => None,
            Lang::French => Some(include_str!("../locales/fr.txt")),
        }
    }
}

/// What the language setting holds: a language, or the language of the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LanguageChoice {
    /// The language of the system when the program has one, English otherwise.
    #[default]
    System,
    /// This language.
    Is(Lang),
}

impl LanguageChoice {
    /// Every choice, in the order the setting lists them.
    pub const ALL: [LanguageChoice; 3] = [
        LanguageChoice::System,
        LanguageChoice::Is(Lang::English),
        LanguageChoice::Is(Lang::French),
    ];

    /// The value written in the settings file (`system`, `en`, `fr`).
    pub fn code(self) -> &'static str {
        match self {
            LanguageChoice::System => "system",
            LanguageChoice::Is(l) => l.code(),
        }
    }

    /// The choice of a settings value; anything unknown is `System`.
    pub fn from_code(code: &str) -> Self {
        if code.trim().eq_ignore_ascii_case("system") {
            return LanguageChoice::System;
        }
        Lang::from_code(code).map_or(LanguageChoice::System, LanguageChoice::Is)
    }

    /// The language this choice means, given the locale of the system (`fr-FR`, from the
    /// platform; `None` when it is unknown).
    pub fn resolve(self, system_locale: Option<&str>) -> Lang {
        match self {
            LanguageChoice::Is(l) => l,
            LanguageChoice::System => system_locale
                .and_then(Lang::from_code)
                .unwrap_or(Lang::English),
        }
    }
}

/// One language's entries: the plain ones sorted by English text for a binary search, and the
/// ones that name values (`Track {name} not found`), which match any text of their shape.
struct Catalog {
    entries: Vec<(String, String)>,
    /// The texts of the language itself, sorted: a text that is already one is not a miss.
    values: Vec<String>,
    patterns: Vec<Pattern>,
}

/// An entry with `{name}` places in its English text.
struct Pattern {
    /// The text between the places (one more than there are places).
    literals: Vec<String>,
    /// The names of the places.
    names: Vec<String>,
    value: String,
}

impl Pattern {
    fn new(key: &str, value: &str) -> Option<Pattern> {
        let mut literals = vec![String::new()];
        let mut names = Vec::new();
        let mut rest = key;
        while let Some(open) = rest.find('{') {
            let after = &rest[open + 1..];
            let close = after.find('}')?;
            literals.last_mut()?.push_str(&rest[..open]);
            names.push(after[..close].to_owned());
            literals.push(String::new());
            rest = &after[close + 1..];
        }
        literals.last_mut()?.push_str(rest);
        if names.is_empty() {
            return None;
        }
        Some(Pattern {
            literals,
            names,
            value: value.to_owned(),
        })
    }

    /// The text of the pattern's translation for `text`, when `text` has the pattern's shape.
    fn apply(&self, text: &str) -> Option<String> {
        let first = self.literals.first()?;
        let last = self.literals.last()?;
        let body = text.strip_prefix(first.as_str())?;
        let body = body.strip_suffix(last.as_str())?;
        let mut values: Vec<&str> = Vec::with_capacity(self.names.len());
        let mut rest = body;
        for lit in &self.literals[1..self.literals.len() - 1] {
            let at = rest.find(lit.as_str())?;
            values.push(&rest[..at]);
            rest = &rest[at + lit.len()..];
        }
        values.push(rest);
        if values.iter().any(|v| v.is_empty()) {
            return None;
        }
        let pairs: Vec<(&str, &str)> = self
            .names
            .iter()
            .map(String::as_str)
            .zip(values.iter().copied())
            .collect();
        Some(fill(&self.value, &pairs))
    }

    fn weight(&self) -> usize {
        self.literals.iter().map(String::len).sum()
    }
}

impl Catalog {
    fn parse(source: &str) -> Self {
        let mut entries: Vec<(String, String)> = source
            .lines()
            .filter_map(|line| {
                let line = line.trim_end_matches('\r');
                if line.trim().is_empty() || line.starts_with('#') {
                    return None;
                }
                let (key, value) = line.split_once(" => ")?;
                Some((unescape(key.trim()), unescape(value.trim())))
            })
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        entries.dedup_by(|a, b| a.0 == b.0);
        let mut patterns: Vec<Pattern> = entries
            .iter()
            .filter_map(|(k, v)| Pattern::new(k, v))
            .collect();
        // The most specific shape first: the one with the most fixed text.
        patterns.sort_by_key(|p| std::cmp::Reverse(p.weight()));
        let mut values: Vec<String> = entries.iter().map(|(_, v)| v.clone()).collect();
        values.sort();
        values.dedup();
        Self {
            entries,
            values,
            patterns,
        }
    }

    /// Whether `text` is the translation of some entry (a menu built in the language, shown
    /// again).
    fn is_value(&self, text: &str) -> bool {
        self.values
            .binary_search_by(|v| v.as_str().cmp(text))
            .is_ok()
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .binary_search_by(|(k, _)| k.as_str().cmp(key))
            .ok()
            .map(|i| self.entries[i].1.as_str())
    }

    fn matching(&self, text: &str) -> Option<String> {
        if text.len() > 240 {
            return None;
        }
        self.patterns.iter().find_map(|p| p.apply(text))
    }
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

fn catalog(lang: Lang) -> Option<&'static Catalog> {
    static FRENCH: OnceLock<Catalog> = OnceLock::new();
    match lang {
        Lang::English => None,
        Lang::French => Some(FRENCH.get_or_init(|| Catalog::parse(lang.source().unwrap_or("")))),
    }
}

/// The entries of `lang`, as `(English text, text)` pairs sorted by English text (empty for
/// English, whose texts are the keys).
pub fn entries(lang: Lang) -> Vec<(&'static str, &'static str)> {
    catalog(lang).map_or_else(Vec::new, |c| {
        c.entries
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect()
    })
}

static CHOSEN: AtomicU8 = AtomicU8::new(0);

thread_local! {
    static OVERRIDE: Cell<Option<Lang>> = const { Cell::new(None) };
    static MISSES: RefCell<Option<BTreeSet<String>>> = const { RefCell::new(None) };
}

/// The language the interface is in now.
pub fn language() -> Lang {
    OVERRIDE
        .with(Cell::get)
        .unwrap_or_else(|| Lang::from_index(CHOSEN.load(Ordering::Relaxed)))
}

/// Chooses the language of the interface for the whole process.
pub fn set_language(lang: Lang) {
    CHOSEN.store(lang.index(), Ordering::Relaxed);
}

/// Runs `f` with `lang` as the language of the calling thread (other threads keep theirs). The
/// tests of every language go through this, so none depends on the system or on another test.
pub fn with_language<R>(lang: Lang, f: impl FnOnce() -> R) -> R {
    let before = OVERRIDE.with(|o| o.replace(Some(lang)));
    let out = f();
    OVERRIDE.with(|o| o.set(before));
    out
}

/// Runs `f` and returns what it gave with the English texts that were looked up in a language
/// other than English and had no entry (texts without a letter are left out).
pub fn with_misses<R>(f: impl FnOnce() -> R) -> (R, BTreeSet<String>) {
    let before = MISSES.with(|m| m.replace(Some(BTreeSet::new())));
    let out = f();
    let found = MISSES.with(|m| m.replace(before)).unwrap_or_default();
    (out, found)
}

fn note_miss(key: &str) {
    if !key.chars().any(|c| c.is_ascii_alphabetic()) {
        return;
    }
    MISSES.with(|m| {
        if let Some(set) = m.borrow_mut().as_mut() {
            set.insert(key.to_owned());
        }
    });
}

/// The text of `key` in the current language: the entry, or `key` itself when the language is
/// English or has no entry for it. Only an entry with the whole text as its English side is
/// found; see [`tr`] for the texts that name values.
pub fn t(key: &str) -> &str {
    let Some(cat) = catalog(language()) else {
        return key;
    };
    match cat.get(key) {
        Some(found) => found,
        None => {
            if !cat.is_value(key) {
                note_miss(key);
            }
            key
        }
    }
}

/// The name of a key or a modifier as the current language writes it on a keyboard (`Shift`
/// is `Maj` in French); the name itself when the language has no other.
pub fn key_name(name: &str) -> String {
    let Some(cat) = catalog(language()) else {
        return name.to_owned();
    };
    let key = format!("key:{name}");
    cat.get(&key).unwrap_or(name).to_owned()
}

static SYSTEM_LOCALE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Records the locale the operating system reports (`fr-FR`), for the System choice. Only the
/// program's start-up calls this: the tests never read the language of the machine.
pub fn set_system_locale(locale: Option<String>) {
    if let Ok(mut held) = SYSTEM_LOCALE.lock() {
        *held = locale;
    }
}

/// The locale last given to [`set_system_locale`].
pub fn system_locale() -> Option<String> {
    SYSTEM_LOCALE.lock().ok().and_then(|held| held.clone())
}

/// The text of `text` in the current language, as [`t`] finds it, and otherwise through the
/// entries that name values: `Saved {name}` gives `Enregistre {name}` for any `Saved ...`, so a
/// message the program builds with `format!` is translated by naming its shape once. Text no
/// entry fits (a song name, a file name) is given back as it is.
pub fn tr(text: &str) -> std::borrow::Cow<'_, str> {
    translate(text).unwrap_or(std::borrow::Cow::Borrowed(text))
}

/// [`tr`], giving `None` when the text stays as it is.
pub fn translate(text: &str) -> Option<std::borrow::Cow<'static, str>> {
    use std::borrow::Cow;
    let cat = catalog(language())?;
    if let Some(found) = cat.get(text) {
        return Some(Cow::Borrowed(found));
    }
    // "Open..." and "Open…" are one text: the file spells the ellipsis as a character.
    if let Some(stem) = text.strip_suffix("...") {
        let key = format!("{stem}…");
        if let Some(found) = cat.get(&key) {
            return Some(Cow::Owned(found.replace('…', "...")));
        }
    }
    if let Some(found) = cat.matching(text) {
        return Some(Cow::Owned(found));
    }
    if !cat.is_value(text) {
        note_miss(text);
    }
    None
}

/// Whether `key` has an entry in `lang` (English has them all).
pub fn has(lang: Lang, key: &str) -> bool {
    let Some(cat) = catalog(lang) else {
        return true;
    };
    if cat.get(key).is_some() {
        return true;
    }
    match key.strip_suffix("...") {
        Some(stem) => cat.get(&format!("{stem}…")).is_some(),
        None => false,
    }
}

/// Whether `text` is a piece of the text of an entry of `lang`: a line of a long note that the
/// program wrapped after translating it. The tests that list the texts without an entry leave
/// these out.
pub fn is_part_of_an_entry(lang: Lang, text: &str) -> bool {
    let Some(cat) = catalog(lang) else {
        return false;
    };
    text.chars().count() >= 8 && cat.values.iter().any(|v| v.contains(text))
}

/// `template` with each `{name}` replaced by the value of that name in `values`; a name with no
/// value stays as written.
pub fn fill(template: &str, values: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                let name = &after[..close];
                match values.iter().find(|(n, _)| *n == name) {
                    Some((_, v)) => out.push_str(v),
                    None => {
                        out.push('{');
                        out.push_str(name);
                        out.push('}');
                    }
                }
                rest = &after[close + 1..];
            }
            None => {
                out.push_str(&rest[open..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// [`t`] of `key`, with the `{name}` values put in.
pub fn tf(key: &str, values: &[(&str, &str)]) -> String {
    fill(t(key), values)
}

/// The text for a count: `one` when the language says `count` is a single thing, `other`
/// otherwise, translated, with `{n}` replaced by the count (French counts 0 and 1 as singular).
pub fn tn(count: u64, one: &str, other: &str) -> String {
    let singular = match language() {
        Lang::English => count == 1,
        Lang::French => count < 2,
    };
    fill(
        t(if singular { one } else { other }),
        &[("n", &count.to_string())],
    )
}

/// The names between braces in `text`, in order.
pub fn placeholders(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else { break };
        names.push(after[..close].to_owned());
        rest = &after[close + 1..];
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_is_the_key() {
        assert_eq!(with_language(Lang::English, || t("Save")), "Save");
    }

    #[test]
    fn a_missing_entry_falls_back_to_english() {
        let (shown, misses) =
            with_misses(|| with_language(Lang::French, || t("No such text anywhere")));
        assert_eq!(shown, "No such text anywhere");
        assert!(misses.contains("No such text anywhere"));
    }

    #[test]
    fn a_language_holds_for_its_thread_only() {
        with_language(Lang::French, || {
            assert_eq!(language(), Lang::French);
            std::thread::spawn(|| assert_eq!(language(), Lang::English))
                .join()
                .unwrap();
        });
        assert_eq!(language(), Lang::English);
    }

    #[test]
    fn codes_and_locales_name_a_language() {
        assert_eq!(Lang::from_code("fr"), Some(Lang::French));
        assert_eq!(Lang::from_code("fr-CA"), Some(Lang::French));
        assert_eq!(Lang::from_code("fr_FR.UTF-8"), Some(Lang::French));
        assert_eq!(Lang::from_code("EN-us"), Some(Lang::English));
        assert_eq!(Lang::from_code("de-DE"), None);
        assert_eq!(Lang::from_code(""), None);
    }

    #[test]
    fn the_system_choice_follows_the_locale_and_falls_back_to_english() {
        let system = LanguageChoice::System;
        assert_eq!(system.resolve(Some("fr-FR")), Lang::French);
        assert_eq!(system.resolve(Some("de-DE")), Lang::English);
        assert_eq!(system.resolve(None), Lang::English);
        assert_eq!(
            LanguageChoice::Is(Lang::French).resolve(Some("en-US")),
            Lang::French
        );
        for choice in LanguageChoice::ALL {
            assert_eq!(LanguageChoice::from_code(choice.code()), choice);
        }
        assert_eq!(LanguageChoice::from_code("klingon"), LanguageChoice::System);
    }

    #[test]
    fn values_go_into_named_places() {
        assert_eq!(
            fill("{b} then {a} then {a}", &[("a", "1"), ("b", "2")]),
            "2 then 1 then 1"
        );
        assert_eq!(fill("keeps {unknown}", &[]), "keeps {unknown}");
        assert_eq!(fill("open {brace", &[("brace", "x")]), "open {brace");
        assert_eq!(placeholders("{a} of {b}"), ["a", "b"]);
    }

    #[test]
    fn counts_pick_the_singular_or_the_plural_of_the_language() {
        let one = "{n} item";
        let other = "{n} items";
        assert_eq!(tn(1, one, other), "1 item");
        assert_eq!(tn(0, one, other), "0 items");
        with_language(Lang::French, || {
            assert_eq!(tn(0, one, other), "0 item");
            assert_eq!(tn(1, one, other), "1 item");
            assert_eq!(tn(2, one, other), "2 items");
        });
    }

    #[test]
    fn the_file_format_has_comments_and_escapes() {
        let c = Catalog::parse(
            "# note\n\nA => B\nTwo\\nlines => Deux\\nlignes\nback\\\\slash => x\r\n",
        );
        assert_eq!(c.get("A"), Some("B"));
        assert_eq!(c.get("Two\nlines"), Some("Deux\nlignes"));
        assert_eq!(c.get("back\\slash"), Some("x"));
        assert_eq!(c.get("# note"), None);
    }
}
