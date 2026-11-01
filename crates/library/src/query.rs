//! The search field's query language.

use crate::{AudioFormat, FileKind, FileRow};

/// A parsed search query.
///
/// Whitespace-separated tokens, all of which must match (AND):
/// - `#name` or `tag:name`: the row has that tag (case-insensitive, exact tag name).
/// - `fav` or `is:fav` (case-insensitive): the row is a favourite.
/// - `kind:x`: the row is of that kind. `x` is `folder`, `audio`, `patch`, `container`,
///   `clip`, `project`, `device`, `item` (any library item) or an audio format (`wav`, `aiff`,
///   `flac`, `mp3`, `ogg`). An unknown kind matches nothing.
/// - `cat:x`: a case-insensitive substring of the category of the row, whether the probed one
///   of a library item or one that the user gave the file.
/// - `author:x`: a case-insensitive substring of the author of the row.
/// - `in:x`: a case-insensitive substring of the name of any folder the row lies in.
/// - anything else: a case-insensitive substring of the row name or, for a library item,
///   of its category.
///
/// A lone `#` and a field with no value (`kind:`) are ignored. Folders match only on name,
/// `kind:` and `in:` terms: a query with a tag, favourite, category or author token never
/// matches a folder. The empty query matches everything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    /// Lower-cased name terms.
    terms: Vec<String>,
    /// Folded tag names.
    tags: Vec<String>,
    /// A `fav` token was present.
    favorite: bool,
    /// `kind:` values.
    kinds: Vec<KindTerm>,
    /// Folded `cat:` values.
    categories: Vec<String>,
    /// Folded `author:` values.
    authors: Vec<String>,
    /// Folded `in:` values.
    folders: Vec<String>,
}

/// The value of a `kind:` token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KindTerm {
    Folder,
    Audio,
    Format(AudioFormat),
    Patch,
    Container,
    Clip,
    Project,
    Device,
    /// Any library item (everything but folders and audio).
    Item,
    /// A word that names no kind: nothing matches.
    Unknown,
}

impl KindTerm {
    fn parse(value: &str) -> Self {
        let is = |name: &str| value.eq_ignore_ascii_case(name);
        if is("folder") || is("dir") {
            Self::Folder
        } else if is("audio") || is("sample") {
            Self::Audio
        } else if is("wav") || is("wave") {
            Self::Format(AudioFormat::Wav)
        } else if is("aif") || is("aiff") {
            Self::Format(AudioFormat::Aiff)
        } else if is("flac") {
            Self::Format(AudioFormat::Flac)
        } else if is("mp3") {
            Self::Format(AudioFormat::Mp3)
        } else if is("ogg") {
            Self::Format(AudioFormat::Ogg)
        } else if is("patch") {
            Self::Patch
        } else if is("container") {
            Self::Container
        } else if is("clip") {
            Self::Clip
        } else if is("project") {
            Self::Project
        } else if is("device") {
            Self::Device
        } else if is("item") {
            Self::Item
        } else {
            Self::Unknown
        }
    }

    fn matches(self, kind: FileKind) -> bool {
        match self {
            Self::Folder => kind == FileKind::Folder,
            Self::Audio => matches!(kind, FileKind::Audio(_)),
            Self::Format(f) => kind == FileKind::Audio(f),
            Self::Patch => kind == FileKind::Patch,
            Self::Container => kind == FileKind::Container,
            Self::Clip => kind == FileKind::Clip,
            Self::Project => kind == FileKind::Project,
            Self::Device => kind == FileKind::Device,
            Self::Item => kind.is_item(),
            Self::Unknown => false,
        }
    }
}

/// Splits `field:value` (the field name compared ignoring case); `None` when `token` is not
/// that field.
fn field_value<'a>(token: &'a str, field: &str) -> Option<&'a str> {
    let head = token.get(..field.len())?;
    let rest = token.get(field.len()..)?.strip_prefix(':')?;
    head.eq_ignore_ascii_case(field).then_some(rest)
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
            } else if let Some(v) = field_value(token, "tag") {
                if !v.is_empty() {
                    query.tags.push(fold(v));
                }
            } else if let Some(v) = field_value(token, "kind") {
                if !v.is_empty() {
                    query.kinds.push(KindTerm::parse(v));
                }
            } else if let Some(v) = field_value(token, "cat") {
                if !v.is_empty() {
                    query.categories.push(fold(v));
                }
            } else if let Some(v) = field_value(token, "author") {
                if !v.is_empty() {
                    query.authors.push(fold(v));
                }
            } else if let Some(v) = field_value(token, "in") {
                if !v.is_empty() {
                    query.folders.push(fold(v));
                }
            } else {
                query.terms.push(fold(token));
            }
        }
        query
    }

    /// True for the empty query.
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
            && self.tags.is_empty()
            && !self.favorite
            && self.kinds.is_empty()
            && self.categories.is_empty()
            && self.authors.is_empty()
            && self.folders.is_empty()
    }

    /// Whether `row` matches. Does not allocate (it runs per row per keystroke).
    pub fn matches(&self, row: &FileRow) -> bool {
        if row.kind == FileKind::Folder
            && (self.favorite
                || !self.tags.is_empty()
                || !self.categories.is_empty()
                || !self.authors.is_empty())
        {
            return false;
        }
        if self.favorite && !row.favorite {
            return false;
        }
        self.kinds.iter().all(|k| k.matches(row.kind))
            && self
                .tags
                .iter()
                .all(|want| row.tags.iter().any(|have| folded_eq(have, want)))
            && self.categories.iter().all(|want| {
                row.categories.iter().any(|c| contains_folded(c, want))
                    || row
                        .item
                        .as_ref()
                        .is_some_and(|i| contains_folded(&i.category, want))
            })
            && self
                .authors
                .iter()
                .all(|want| contains_folded(&row.author, want))
            && self.terms.iter().all(|term| {
                contains_folded(&row.name, term)
                    || row
                        .item
                        .as_ref()
                        .is_some_and(|i| contains_folded(&i.category, term))
            })
            && self.folders.iter().all(|want| in_folder(row, want))
    }
}

/// Whether the name of any folder above `row` contains the folded `needle`. Splits the path text
/// at both kinds of separator, which is much cheaper per row than walking path components.
fn in_folder(row: &FileRow, needle: &str) -> bool {
    row.path.to_str().is_some_and(|text| {
        text.rsplit(['/', '\\'])
            .skip(1)
            .any(|name| !name.is_empty() && contains_folded(name, needle))
    })
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
            author: String::new(),
            categories: Vec::new(),
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

    fn rich(path: &str, kind: FileKind) -> FileRow {
        FileRow {
            path: PathBuf::from(path),
            name: path.rsplit('/').next().unwrap_or(path).to_string(),
            kind,
            author: "Ada Quinn".to_string(),
            categories: vec!["Percussion".to_string(), "One shots".to_string()],
            ..file("x", false, &[])
        }
    }

    #[test]
    fn kind_tokens_select_rows_by_kind() {
        let wav = rich("/s/a.wav", FileKind::Audio(AudioFormat::Wav));
        let flac = rich("/s/a.flac", FileKind::Audio(AudioFormat::Flac));
        let patch = rich("/s/p.atomikpatch", FileKind::Patch);
        let dir = folder("Kicks");
        assert!(m("kind:audio", &wav) && m("kind:audio", &flac));
        assert!(!m("kind:audio", &patch) && !m("kind:audio", &dir));
        assert!(m("KIND:WAV", &wav) && !m("kind:wav", &flac));
        assert!(m("kind:flac", &flac));
        assert!(m("kind:patch", &patch) && m("kind:item", &patch) && !m("kind:item", &wav));
        assert!(m("kind:folder", &dir) && !m("kind:folder", &wav));
        assert!(!m("kind:nonsense", &wav), "an unknown kind matches nothing");
        assert!(m("kind:", &wav), "no value is ignored");
        assert!(Query::parse("kind:").is_empty());
        assert!(!Query::parse("kind:wav").is_empty());
    }

    #[test]
    fn category_tokens_match_user_and_probed_categories() {
        let mut r = rich("/s/a.wav", FileKind::Audio(AudioFormat::Wav));
        assert!(m("cat:perc", &r) && m("cat:ONE", &r) && m("cat:shots", &r));
        assert!(!m("cat:synth", &r));
        r.categories.clear();
        assert!(!m("cat:perc", &r));
        r.item = Some(crate::ItemInfo {
            category: "Instrument".to_string(),
            ..Default::default()
        });
        assert!(m("cat:instr", &r));
        assert!(
            !m("cat:instr", &folder("Instruments")),
            "folders carry no category"
        );
    }

    #[test]
    fn author_tokens_match_a_substring_of_the_author() {
        let r = rich("/s/a.wav", FileKind::Audio(AudioFormat::Wav));
        assert!(m("author:ada", &r) && m("author:QUINN", &r) && !m("author:a.q", &r));
        assert!(!m("author:bob", &r));
        let mut anon = r.clone();
        anon.author.clear();
        assert!(!m("author:ada", &anon));
        assert!(!m("author:ada", &folder("Ada")));
    }

    #[test]
    fn in_tokens_match_any_enclosing_folder_but_not_the_file_name() {
        let r = rich(
            "/music/Drums/Kicks/a.wav",
            FileKind::Audio(AudioFormat::Wav),
        );
        assert!(m("in:drums", &r) && m("in:KICK", &r) && m("in:music", &r));
        assert!(!m("in:a.wav", &r) && !m("in:snares", &r));
        assert!(m("in:drums kind:audio a", &r));
        // A folder row lies in its parents, not in itself.
        let d = FileRow {
            path: PathBuf::from("/music/Drums/Kicks"),
            ..folder("Kicks")
        };
        assert!(m("in:drums", &d) && !m("in:kicks", &d));
    }

    #[test]
    fn tag_field_is_the_same_as_the_hash_form() {
        let r = file("a.wav", false, &["808"]);
        assert!(m("tag:808", &r) && m("TAG:808", &r) && !m("tag:80", &r));
        assert!(!m("tag:808", &folder("x")));
    }

    #[test]
    fn fields_combine_with_words() {
        let r = rich("/lib/Drums/kick.wav", FileKind::Audio(AudioFormat::Wav));
        assert!(m("kick kind:wav in:drums author:ada cat:perc", &r));
        assert!(!m("kick kind:wav in:drums author:ada cat:bass", &r));
        assert!(m("kind:wav kind:audio", &r));
        assert!(!m("kind:wav kind:flac", &r));
    }

    #[test]
    fn a_word_that_only_looks_like_a_field_is_a_name_term() {
        let r = file("index:1.wav", false, &[]);
        assert!(m("index:1", &r));
        assert!(m("kindle", &file("kindle.wav", false, &[])));
        // Multi-byte text right where a field name would end must not panic.
        let _ = Query::parse("kin\u{e9}:x \u{e9}:x \u{65e5}\u{672c}\u{8a9e}:y");
    }

    #[test]
    fn sigma_folds_consistently() {
        let r = file("ΟΔΟΣ.wav", false, &["ΟΔΟΣ"]);
        assert!(m("οδοσ", &r));
        assert!(m("#οδοσ", &r));
    }
}
