//! Row ordering.

use crate::{AudioFormat, FileKind, FileRow};
use std::cmp::Ordering;

/// The column a listing is sorted by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortKey {
    /// Natural name order.
    #[default]
    Name,
    /// Duration (unknown durations last).
    Duration,
    /// File size.
    Size,
    /// Audio format, then name.
    Format,
}

/// Natural, case-insensitive order: digit runs compare by value, so `"kick 2" < "kick 10"`.
/// Ties are broken by the plain string order, so the order is total and deterministic.
pub fn natural_cmp(a: &str, b: &str) -> Ordering {
    let mut ta = Tokens(a);
    let mut tb = Tokens(b);
    loop {
        let ord = match (ta.next(), tb.next()) {
            (None, None) => break,
            (None, Some(_)) => Ordering::Less,
            (Some(_), None) => Ordering::Greater,
            (Some(x), Some(y)) => x.cmp_with(y),
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    a.cmp(b)
}

/// A maximal run of ASCII digits or of anything else.
#[derive(Clone, Copy)]
enum Token<'a> {
    Num(&'a str),
    Text(&'a str),
}

impl Token<'_> {
    /// Numbers before text; numbers by value; text case-insensitively.
    fn cmp_with(self, other: Self) -> Ordering {
        match (self, other) {
            (Token::Num(a), Token::Num(b)) => {
                let (a, b) = (a.trim_start_matches('0'), b.trim_start_matches('0'));
                a.len().cmp(&b.len()).then_with(|| a.cmp(b))
            }
            (Token::Num(_), Token::Text(_)) => Ordering::Less,
            (Token::Text(_), Token::Num(_)) => Ordering::Greater,
            (Token::Text(a), Token::Text(b)) => a
                .chars()
                .flat_map(char::to_lowercase)
                .cmp(b.chars().flat_map(char::to_lowercase)),
        }
    }
}

/// Splits a string into [`Token`]s without allocating.
struct Tokens<'a>(&'a str);

impl<'a> Iterator for Tokens<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Token<'a>> {
        let first = self.0.chars().next()?;
        let digits = first.is_ascii_digit();
        let end = self
            .0
            .find(|c: char| c.is_ascii_digit() != digits)
            .unwrap_or(self.0.len());
        let (head, tail) = self.0.split_at(end);
        self.0 = tail;
        Some(if digits {
            Token::Num(head)
        } else {
            Token::Text(head)
        })
    }
}

fn duration_of(row: &FileRow) -> Option<f64> {
    row.info.and_then(|i| i.duration_secs())
}

fn format_of(row: &FileRow) -> Option<AudioFormat> {
    match row.kind {
        FileKind::Audio(f) => Some(f),
        FileKind::Folder
        | FileKind::Midi
        | FileKind::Patch
        | FileKind::Container
        | FileKind::Clip
        | FileKind::Project
        | FileKind::Device => None,
    }
}

/// Sorts `rows` in place: folders always first (by natural name, ascending), then files by
/// `key`, reversed when `descending`. Equal keys fall back to natural name order, so the
/// result is deterministic. Stable.
///
/// Descending reverses only the key comparison: ties still read in ascending name order, and
/// files with an unknown duration stay last in both directions. Folders ignore `key` and
/// `descending`.
pub fn sort_rows(rows: &mut [FileRow], key: SortKey, descending: bool) {
    rows.sort_by(|a, b| {
        let folders = (a.kind == FileKind::Folder, b.kind == FileKind::Folder);
        match folders {
            (true, false) => return Ordering::Less,
            (false, true) => return Ordering::Greater,
            (true, true) => return natural_cmp(&a.name, &b.name),
            (false, false) => {}
        }
        let by_name = natural_cmp(&a.name, &b.name);
        let directed = |ord: Ordering| if descending { ord.reverse() } else { ord };
        match key {
            SortKey::Name => directed(by_name),
            SortKey::Size => directed(a.size.cmp(&b.size)).then(by_name),
            SortKey::Format => directed(format_of(a).cmp(&format_of(b))).then(by_name),
            SortKey::Duration => match (duration_of(a), duration_of(b)) {
                (None, None) => by_name,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(x), Some(y)) => directed(x.total_cmp(&y)).then(by_name),
            },
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AudioFormat, AudioInfo, FileKind};
    use std::path::PathBuf;

    fn row(name: &str, kind: FileKind, size: u64, secs: Option<u64>) -> FileRow {
        FileRow {
            path: PathBuf::from(name),
            name: name.to_string(),
            kind,
            size,
            info: secs.map(|s| AudioInfo {
                sample_rate: 1000,
                channels: 1,
                frames: Some(s * 1000),
            }),
            item: None,
            favorite: false,
            tags: Vec::new(),
            author: String::new(),
            categories: Vec::new(),
        }
    }

    fn wav(name: &str, size: u64, secs: Option<u64>) -> FileRow {
        row(name, FileKind::Audio(AudioFormat::Wav), size, secs)
    }

    fn names(rows: &[FileRow]) -> Vec<&str> {
        rows.iter().map(|r| r.name.as_str()).collect()
    }

    #[test]
    fn digit_runs_compare_by_value() {
        assert_eq!(natural_cmp("kick 2", "kick 10"), Ordering::Less);
        assert_eq!(natural_cmp("kick 10", "kick 2"), Ordering::Greater);
        assert_eq!(natural_cmp("a9b", "a10b"), Ordering::Less);
    }

    #[test]
    fn comparison_ignores_case_first() {
        assert_eq!(natural_cmp("Kick", "kick 2"), Ordering::Less);
        assert_eq!(natural_cmp("b", "A"), Ordering::Greater);
    }

    #[test]
    fn ties_use_plain_string_order() {
        assert_eq!(natural_cmp("kick", "Kick"), Ordering::Greater);
        assert_eq!(natural_cmp("a01", "a1"), Ordering::Less);
        assert_eq!(natural_cmp("same", "same"), Ordering::Equal);
    }

    #[test]
    fn huge_digit_runs_do_not_overflow() {
        let a = format!("x{}", "9".repeat(60));
        let b = format!("x1{}", "0".repeat(60));
        assert_eq!(natural_cmp(&a, &b), Ordering::Less);
    }

    #[test]
    fn digits_sort_before_text() {
        assert_eq!(natural_cmp("1 kick", "a kick"), Ordering::Less);
    }

    #[test]
    fn folders_first_regardless_of_direction() {
        let folder = |n| row(n, FileKind::Folder, 0, None);
        for desc in [false, true] {
            let mut rows = vec![
                wav("a.wav", 1, None),
                folder("z"),
                folder("B2"),
                folder("B10"),
            ];
            sort_rows(&mut rows, SortKey::Name, desc);
            assert_eq!(names(&rows)[..3], ["B2", "B10", "z"], "desc={desc}");
        }
    }

    #[test]
    fn name_sort_and_descending() {
        let mut rows = vec![wav("k10.wav", 1, None), wav("k2.wav", 1, None)];
        sort_rows(&mut rows, SortKey::Name, false);
        assert_eq!(names(&rows), ["k2.wav", "k10.wav"]);
        sort_rows(&mut rows, SortKey::Name, true);
        assert_eq!(names(&rows), ["k10.wav", "k2.wav"]);
    }

    #[test]
    fn duration_puts_unknown_last_both_ways() {
        let mut rows = vec![
            wav("u.wav", 1, None),
            wav("long.wav", 1, Some(9)),
            wav("short.wav", 1, Some(1)),
        ];
        sort_rows(&mut rows, SortKey::Duration, false);
        assert_eq!(names(&rows), ["short.wav", "long.wav", "u.wav"]);
        sort_rows(&mut rows, SortKey::Duration, true);
        assert_eq!(names(&rows), ["long.wav", "short.wav", "u.wav"]);
    }

    #[test]
    fn size_ties_fall_back_to_name() {
        let mut rows = vec![
            wav("b.wav", 5, None),
            wav("a.wav", 5, None),
            wav("c.wav", 1, None),
        ];
        sort_rows(&mut rows, SortKey::Size, false);
        assert_eq!(names(&rows), ["c.wav", "a.wav", "b.wav"]);
        sort_rows(&mut rows, SortKey::Size, true);
        assert_eq!(names(&rows), ["a.wav", "b.wav", "c.wav"]);
    }

    #[test]
    fn format_orders_by_format_then_name() {
        let f = |n: &str, fmt| row(n, FileKind::Audio(fmt), 1, None);
        let mut rows = vec![
            f("b.ogg", AudioFormat::Ogg),
            f("z.wav", AudioFormat::Wav),
            f("a.ogg", AudioFormat::Ogg),
            f("a.wav", AudioFormat::Wav),
        ];
        sort_rows(&mut rows, SortKey::Format, false);
        assert_eq!(names(&rows), ["a.wav", "z.wav", "a.ogg", "b.ogg"]);
        sort_rows(&mut rows, SortKey::Format, true);
        assert_eq!(names(&rows), ["a.ogg", "b.ogg", "a.wav", "z.wav"]);
    }

    #[test]
    fn sort_is_stable_for_identical_keys() {
        let mut a = wav("same.wav", 1, None);
        a.path = PathBuf::from("/one/same.wav");
        let mut b = wav("same.wav", 1, None);
        b.path = PathBuf::from("/two/same.wav");
        let mut rows = vec![b.clone(), a.clone()];
        sort_rows(&mut rows, SortKey::Name, false);
        assert_eq!(rows, vec![b, a]);
    }
}
