//! Property tests of the search language: parsing never panics, and a query is exactly the
//! conjunction of its tokens.

use atomik_library::{AudioFormat, FileKind, FileRow, ItemInfo, Query};
use proptest::prelude::*;
use std::path::PathBuf;

fn word() -> impl Strategy<Value = String> {
    prop::string::string_regex("[a-zA-Z0-9éÉ_.]{1,6}").unwrap()
}

/// One query token: a plain word or one of the field forms.
fn token() -> impl Strategy<Value = String> {
    (0u8..9, word()).prop_map(|(form, w)| match form {
        0 => w,
        1 => format!("#{w}"),
        2 => format!("tag:{w}"),
        3 => format!("kind:{w}"),
        4 => format!("cat:{w}"),
        5 => format!("author:{w}"),
        6 => format!("in:{w}"),
        7 => "fav".to_string(),
        _ => format!(
            "kind:{}",
            ["audio", "wav", "folder", "patch", "item"][w.len() % 5]
        ),
    })
}

fn row() -> impl Strategy<Value = FileRow> {
    (
        word(),
        word(),
        word(),
        prop::collection::vec(word(), 0..3),
        prop::collection::vec(word(), 0..3),
        0u8..4,
        any::<bool>(),
    )
        .prop_map(|(name, folder, author, tags, categories, kind, favorite)| {
            let kind = match kind {
                0 => FileKind::Folder,
                1 => FileKind::Audio(AudioFormat::Wav),
                2 => FileKind::Audio(AudioFormat::Flac),
                _ => FileKind::Patch,
            };
            FileRow {
                path: PathBuf::from(format!("/lib/{folder}/{name}")),
                name,
                kind,
                size: 0,
                info: None,
                item: Some(ItemInfo {
                    category: "Instrument".into(),
                    ..ItemInfo::default()
                }),
                favorite,
                tags,
                author,
                categories,
            }
        })
}

proptest! {
    #[test]
    fn parsing_arbitrary_text_never_panics(text in prop::string::string_regex(r"\PC{0,40}").unwrap()) {
        let q = Query::parse(&text);
        let r = FileRow {
            path: PathBuf::from("/a/b.wav"),
            name: "b.wav".into(),
            kind: FileKind::Audio(AudioFormat::Wav),
            size: 0,
            info: None,
            item: None,
            favorite: false,
            tags: Vec::new(),
            author: String::new(),
            categories: Vec::new(),
        };
        let _ = q.matches(&r);
    }

    #[test]
    fn a_query_is_the_conjunction_of_its_tokens(
        tokens in prop::collection::vec(token(), 0..5),
        r in row(),
    ) {
        let all = Query::parse(&tokens.join(" "));
        let each = tokens.iter().all(|t| Query::parse(t).matches(&r));
        prop_assert_eq!(all.matches(&r), each, "{:?} on {:?}", tokens, r);
    }

    #[test]
    fn token_order_and_spacing_do_not_matter(
        tokens in prop::collection::vec(token(), 0..5),
        r in row(),
    ) {
        let mut reversed = tokens.clone();
        reversed.reverse();
        let a = Query::parse(&tokens.join(" ")).matches(&r);
        let b = Query::parse(&format!("  {}  ", reversed.join("   "))).matches(&r);
        prop_assert_eq!(a, b);
    }

    #[test]
    fn only_the_empty_query_is_empty(tokens in prop::collection::vec(token(), 0..4)) {
        let q = Query::parse(&tokens.join(" "));
        prop_assert_eq!(q.is_empty(), tokens.is_empty());
    }
}
