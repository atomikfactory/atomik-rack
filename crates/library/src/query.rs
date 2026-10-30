//! The search field's query language.

use crate::{FileKind, FileRow};

/// A parsed search query.
///
/// Whitespace-separated tokens, all of which must match (AND):
/// - `#name` — the row has that tag (case-insensitive, exact tag name).
/// - `fav` or `is:fav` (case-insensitive) — the row is a favourite.
/// - anything else — a case-insensitive substring of the row's name or, for a library item,
///   of its category.
///
/// A lone `#` is ignored. Folders match only on name terms: a query with a tag or
/// favourite token never matches a folder. The empty query matches everything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    /// Lower-cased name terms.
    terms: Vec<String>,
    /// Folded tag names.
    tags: Vec<String>,
    /// A `fav` token was present.
    favorite: bool,
}

impl Query {
    /// Parses the search field text.
    pub fn parse(text: &str) -> Self {
        let mut query = Self::default();
        for token in text.split_whitespace() {
            if let Some(tag) = token.strip_prefix('#') {
                if !tag.is_empty() {
                    query.tags.push(fold(tag));
                }
            } else if token.eq_ignore_ascii_case("fav") || token.eq_ignore_ascii_case("is:fav") {
                query.favorite = true;
            } else {
                query.terms.push(fold(token));
            }
        }
        query
    }

    /// True for the empty query.
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty() && self.tags.is_empty() && !self.favorite
    }

    /// Whether `row` matches. Does not allocate (it runs per row per keystroke).
    pub fn matches(&self, row: &FileRow) -> bool {
        if row.kind == FileKind::Folder && (self.favorite || !self.tags.is_empty()) {
            return false;
        }
        if self.favorite && !row.favorite {
            return false;
        }
        self.tags
            .iter()
            .all(|want| row.tags.iter().any(|have| folded_eq(have, want)))
            && self.terms.iter().all(|term| {
                contains_folded(&row.name, term)
                    || row
                        .item
                        .as_ref()
                        .is_some_and(|i| contains_folded(&i.category, term))
            })
    }
}

/// Per-character lower-casing, the same fold [`crate::Tag::folded`] uses.
fn fold(s: &str) -> String {
    s.chars().flat_map(char::to_lowercase).collect()
}

/// Whether `have` folds to exactly `want` (already folded).
fn folded_eq(have: &str, want: &str) -> bool {
    have.chars().flat_map(char::to_lowercase).eq(want.chars())
}

/// Case-insensitive substring test against a folded `needle`, without allocating.
fn contains_folded(hay: &str, needle: &str) -> bool {
    if needle.is_ascii() {
        if contains_ascii(hay.as_bytes(), needle.as_bytes()) {
            return true;
        }
        if hay.is_ascii() {
            return false;
        }
    }
    let Some(first) = needle.chars().next() else {
        return true;
    };
    hay.char_indices().any(|(i, c)| {
        c.to_lowercase().next() == Some(first) && {
            let mut folded = hay[i..].chars().flat_map(char::to_lowercase);
            needle.chars().all(|n| folded.next() == Some(n))
        }
    })
}

/// ASCII-case-insensitive byte search; `needle` is lower-case ASCII.
fn contains_ascii(hay: &[u8], needle: &[u8]) -> bool {
    let Some((&first, rest)) = needle.split_first() else {
        return true;
    };
    // `b | 0x20` maps both cases of a letter to one value; other bytes may collide, so every
    // candidate is verified below.
    let key = first | 0x20;
    let mut from = 0;
    while hay.len() - from >= needle.len() {
        let window = &hay[from..=hay.len() - needle.len()];
        let Some(at) = window.iter().position(|&b| b | 0x20 == key) else {
            return false;
        };
        let start = from + at;
        if hay[start + 1..start + needle.len()].eq_ignore_ascii_case(rest) {
            return true;
        }
        from = start + 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AudioFormat, FileKind};
    use std::path::PathBuf;

    fn file(name: &str, favorite: bool, tags: &[&str]) -> FileRow {
        FileRow {
            path: PathBuf::from(name),
            name: name.to_string(),
            kind: FileKind::Audio(AudioFormat::Wav),
            size: 0,
            info: None,
            item: None,
            favorite,
            tags: tags.iter().map(|t| (*t).to_string()).collect(),
        }
    }

    fn folder(name: &str) -> FileRow {
        FileRow {
            kind: FileKind::Folder,
            ..file(name, false, &[])
        }
    }

    fn m(q: &str, row: &FileRow) -> bool {
        Query::parse(q).matches(row)
    }

    #[test]
    fn empty_query_matches_everything() {
        let q = Query::parse("   ");
        assert!(q.is_empty());
        assert!(q.matches(&file("a.wav", false, &[])));
        assert!(q.matches(&folder("Drums")));
        assert!(Query::default().is_empty());
        assert!(!Query::parse("a").is_empty());
    }

    #[test]
    fn words_are_case_insensitive_substrings() {
        let r = file("Big Kick 01.wav", false, &[]);
        assert!(m("kick", &r));
        assert!(m("KICK", &r));
        assert!(m("g k", &r));
        assert!(m("ick 0", &r));
        assert!(!m("snare", &r));
    }

    #[test]
    fn words_match_the_category_of_an_item() {
        let mut r = file("Poly Synth", false, &[]);
        r.item = Some(crate::ItemInfo {
            detail: String::new(),
            category: "Instrument".to_string(),
            ..Default::default()
        });
        assert!(m("instru", &r));
        assert!(m("poly instrument", &r));
        assert!(!m("effect", &r));
        assert!(!m("instru", &file("Poly Synth", false, &[])));
    }

    #[test]
    fn all_words_must_match_in_any_order() {
        let r = file("Big Kick 01.wav", false, &[]);
        assert!(m("kick big", &r));
        assert!(m("  01   kick ", &r));
        assert!(!m("kick snare", &r));
    }

    #[test]
    fn non_ascii_names_fold_case() {
        let r = file("Café Épique.wav", false, &[]);
        assert!(m("café", &r));
        assert!(m("ÉPIQUE", &r));
        assert!(m("épi", &r));
        assert!(!m("cafe", &r));
    }

    #[test]
    fn tag_token_matches_exact_tag_ignoring_case() {
        let r = file("a.wav", false, &["808", "Hi Hat"]);
        assert!(m("#808", &r));
        assert!(m("#HI", &file("a.wav", false, &["hi"])));
        assert!(!m("#80", &r));
        assert!(!m("#909", &r));
        assert!(!m("#808", &file("a.wav", false, &[])));
    }

    #[test]
    fn name_and_tag_combine() {
        let r = file("kick.wav", false, &["808"]);
        assert!(m("kick #808", &r));
        assert!(!m("snare #808", &r));
        assert!(!m("kick #909", &r));
        assert!(m("#808 #808", &r));
    }

    #[test]
    fn favourite_tokens() {
        let fav = file("a.wav", true, &[]);
        let not = file("a.wav", false, &[]);
        for q in ["fav", "FAV", "is:fav", "Is:Fav"] {
            assert!(m(q, &fav), "{q}");
            assert!(!m(q, &not), "{q}");
        }
        assert!(m("a fav", &fav));
        assert!(!m("fav", &file("favourite.wav", false, &[])));
    }

    #[test]
    fn lone_hash_is_ignored() {
        assert!(Query::parse("#").is_empty());
        assert!(m("# kick", &file("kick.wav", false, &[])));
    }

    #[test]
    fn folders_match_only_name_terms() {
        let d = folder("Kicks");
        assert!(m("kick", &d));
        assert!(!m("#808", &d));
        assert!(!m("fav", &d));
        assert!(!m("kick fav", &d));
    }

    #[test]
    fn sigma_folds_consistently() {
        let r = file("ΟΔΟΣ.wav", false, &["ΟΔΟΣ"]);
        assert!(m("οδοσ", &r));
        assert!(m("#οδοσ", &r));
    }
}
