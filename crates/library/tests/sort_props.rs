//! Property tests: `natural_cmp` is a total order and `sort_rows` is idempotent.

use atomik_library::{AudioFormat, AudioInfo, FileKind, FileRow, SortKey, natural_cmp, sort_rows};
use proptest::prelude::*;
use std::cmp::Ordering;
use std::path::PathBuf;

fn name_strategy() -> impl Strategy<Value = String> {
    prop::string::string_regex("[a-zA-Z0-9 _.éÉßıİ0٣]{0,12}").unwrap()
}

fn row_strategy() -> impl Strategy<Value = FileRow> {
    (name_strategy(), 0u8..6, 0u64..4, prop::option::of(0u64..5)).prop_map(
        |(name, kind, size, frames)| {
            let kind = match kind {
                0 => FileKind::Folder,
                1 => FileKind::Audio(AudioFormat::Wav),
                2 => FileKind::Audio(AudioFormat::Flac),
                3 => FileKind::Audio(AudioFormat::Mp3),
                4 => FileKind::Audio(AudioFormat::Ogg),
                _ => FileKind::Audio(AudioFormat::Aiff),
            };
            FileRow {
                path: PathBuf::from(&name),
                name,
                kind,
                size,
                info: frames.map(|f| AudioInfo {
                    sample_rate: 10,
                    channels: 1,
                    frames: Some(f),
                }),
                item: None,
                favorite: false,
                tags: Vec::new(),
            }
        },
    )
}

proptest! {
    #[test]
    fn natural_cmp_is_antisymmetric(a in name_strategy(), b in name_strategy()) {
        prop_assert_eq!(natural_cmp(&a, &b), natural_cmp(&b, &a).reverse());
        prop_assert_eq!(natural_cmp(&a, &b) == Ordering::Equal, a == b);
    }

    #[test]
    fn natural_cmp_is_transitive(a in name_strategy(), b in name_strategy(), c in name_strategy()) {
        if natural_cmp(&a, &b) != Ordering::Greater && natural_cmp(&b, &c) != Ordering::Greater {
            prop_assert_ne!(natural_cmp(&a, &c), Ordering::Greater);
        }
    }

    #[test]
    fn sort_is_idempotent_and_a_permutation(
        rows in prop::collection::vec(row_strategy(), 0..24),
        key in prop::sample::select(vec![SortKey::Name, SortKey::Duration, SortKey::Size, SortKey::Format]),
        desc in any::<bool>(),
    ) {
        let mut once = rows.clone();
        sort_rows(&mut once, key, desc);
        let mut twice = once.clone();
        sort_rows(&mut twice, key, desc);
        prop_assert_eq!(&once, &twice);
        prop_assert_eq!(once.len(), rows.len());
        let folders = once.iter().take_while(|r| r.kind == FileKind::Folder).count();
        prop_assert!(once[folders..].iter().all(|r| r.kind != FileKind::Folder));
    }
}
