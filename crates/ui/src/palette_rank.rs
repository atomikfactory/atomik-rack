//! Ranking for the command palette: which commands a typed query finds, in what order, and
//! which letters of the name and the menu path matched.
//!
//! The query is split into words and every word must match somewhere in the command's name, its
//! menu path (or group) or its id; the text of a word may be a stretch of one of them or letters
//! scattered through it in order (a subsequence). A word that is a whole stretch of the text beats
//! scattered letters, and a stretch that starts the text or a word beats one in the middle of a
//! word. A match in the name counts fully, one in the path or the id counts less. Ties are
//! settled by the recent commands (newest first), then the shorter name, then the name.
//!
//! With nothing typed the list is the recent commands, newest first, and then every other
//! command by name. All of it is pure: no canvas, no keymap, no clock.

/// One command the palette can run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaletteEntry {
    /// What the recent list remembers it by: the command id, with `|` and the argument when it
    /// has one.
    pub key: String,
    /// The dotted command id (`edit.quantize`), for searching.
    pub id: String,
    /// The name shown and searched.
    pub name: String,
    /// Where it lives: its menu path (`Edit > Normalise To`) or its group (`Editor`).
    pub path: String,
    /// The shortcut it has in the live keymap.
    pub shortcut: Option<String>,
    /// Whether it can run right now (the menus grey it out when not).
    pub enabled: bool,
}

/// One command that matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ranked {
    /// The place of the command in the entries given.
    pub entry: usize,
    /// How well it matched (higher first).
    pub score: i32,
    /// The characters of the name that matched, as character positions.
    pub name_hits: Vec<usize>,
    /// The characters of the path that matched, as character positions.
    pub path_hits: Vec<usize>,
}

/// Worth of a match in the name, the path and the id, in percent.
const NAME_WEIGHT: i32 = 100;
const PATH_WEIGHT: i32 = 45;
const ID_WEIGHT: i32 = 35;
/// Scattered letters never reach a whole stretch: they stop here.
const SCATTER_CAP: i32 = 90;

/// The commands that match `query`, best first. `recent` lists command keys, newest first.
pub fn rank(entries: &[PaletteEntry], query: &str, recent: &[String]) -> Vec<Ranked> {
    let words: Vec<Vec<char>> = query
        .split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| !matches!(c, '>' | '|'))
                .collect::<Vec<_>>()
        })
        .filter(|w| !w.is_empty())
        .map(|w| w.iter().map(|c| fold(*c)).collect())
        .collect();
    let recency = |e: &PaletteEntry| -> usize {
        recent
            .iter()
            .position(|k| *k == e.key)
            .unwrap_or(usize::MAX)
    };
    if words.is_empty() {
        let mut all: Vec<Ranked> = entries
            .iter()
            .enumerate()
            .map(|(entry, _)| Ranked {
                entry,
                score: 0,
                name_hits: Vec::new(),
                path_hits: Vec::new(),
            })
            .collect();
        all.sort_by(|a, b| {
            let (ea, eb) = (&entries[a.entry], &entries[b.entry]);
            recency(ea)
                .cmp(&recency(eb))
                .then_with(|| ea.name.to_lowercase().cmp(&eb.name.to_lowercase()))
                .then_with(|| ea.path.cmp(&eb.path))
                .then(a.entry.cmp(&b.entry))
        });
        return all;
    }
    let mut found: Vec<Ranked> = entries
        .iter()
        .enumerate()
        .filter_map(|(entry, e)| score_entry(entry, e, &words))
        .collect();
    found.sort_by(|a, b| {
        let (ea, eb) = (&entries[a.entry], &entries[b.entry]);
        b.score
            .cmp(&a.score)
            .then_with(|| recency(ea).cmp(&recency(eb)))
            .then_with(|| ea.name.chars().count().cmp(&eb.name.chars().count()))
            .then_with(|| ea.name.to_lowercase().cmp(&eb.name.to_lowercase()))
            .then(a.entry.cmp(&b.entry))
    });
    found
}

fn score_entry(entry: usize, e: &PaletteEntry, words: &[Vec<char>]) -> Option<Ranked> {
    let name: Vec<char> = e.name.chars().map(fold).collect();
    let path: Vec<char> = e.path.chars().map(fold).collect();
    let id: Vec<char> = e.id.chars().map(fold).collect();
    let mut total = 0;
    let mut name_hits = Vec::new();
    let mut path_hits = Vec::new();
    for word in words {
        let mut best: Option<(i32, Field, Vec<usize>)> = None;
        for (field, text, weight) in [
            (Field::Name, &name, NAME_WEIGHT),
            (Field::Path, &path, PATH_WEIGHT),
            (Field::Id, &id, ID_WEIGHT),
        ] {
            if let Some((score, hits)) = match_word(word, text) {
                let weighted = score * weight / 100;
                if best.as_ref().is_none_or(|(s, _, _)| weighted > *s) {
                    best = Some((weighted, field, hits));
                }
            }
        }
        let (score, field, hits) = best?;
        total += score;
        match field {
            Field::Name => name_hits.extend(hits),
            Field::Path => path_hits.extend(hits),
            Field::Id => {}
        }
    }
    name_hits.sort_unstable();
    name_hits.dedup();
    path_hits.sort_unstable();
    path_hits.dedup();
    Some(Ranked {
        entry,
        // Ten points a letter of score against one a letter of name: a shorter name wins a tie
        // of match quality without ever beating a better match.
        score: total * 10 - name.len() as i32,
        name_hits,
        path_hits,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Name,
    Path,
    Id,
}

/// The letter as it is compared: lower case, one character for one character.
fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Whether a word of the text starts at `at`.
fn boundary(text: &[char], at: usize) -> bool {
    at == 0 || !text[at - 1].is_alphanumeric()
}

/// How well `word` (folded) matches `text` (folded) and which characters of `text` it uses.
pub fn match_word(word: &[char], text: &[char]) -> Option<(i32, Vec<usize>)> {
    let n = word.len();
    if n == 0 || n > text.len() {
        return None;
    }
    if let Some(found) = stretch(word, text) {
        return Some(found);
    }
    scattered(word, text)
}

/// The best place `word` occurs whole in `text`.
fn stretch(word: &[char], text: &[char]) -> Option<(i32, Vec<usize>)> {
    let n = word.len();
    let mut best: Option<(i32, usize)> = None;
    for s in 0..=text.len() - n {
        if text[s..s + n] != *word {
            continue;
        }
        let mut score = 100 + 4 * n as i32;
        if s == 0 {
            score += 40;
        }
        if boundary(text, s) {
            score += 30;
        }
        if s + n == text.len() || !text[s + n].is_alphanumeric() {
            score += 30;
        }
        if best.is_none_or(|(b, _)| score > b) {
            best = Some((score, s));
        }
    }
    best.map(|(score, s)| (score, (s..s + n).collect()))
}

/// The best way to find the letters of `word` in order in `text`, with a gap costing a point a
/// letter and word starts and runs earning bonuses; `None` when there is none (see the gap rule
/// in the code).
fn scattered(word: &[char], text: &[char]) -> Option<(i32, Vec<usize>)> {
    let n = word.len();
    let m = text.len();
    if n < 2 {
        return None;
    }
    const NONE: i32 = i32::MIN / 2;
    let mut score = vec![vec![NONE; m]; n];
    let mut from = vec![vec![usize::MAX; m]; n];
    let letter = |j: usize| -> i32 {
        10 + if boundary(text, j) { 14 } else { 0 } + if j == 0 { 8 } else { 0 }
    };
    for j in 0..m {
        if text[j] == word[0] {
            score[0][j] = letter(j);
        }
    }
    for i in 1..n {
        for j in i..m {
            if text[j] != word[i] {
                continue;
            }
            let mut best = NONE;
            let mut parent = usize::MAX;
            // A letter follows the one before it closely, or starts a word (initials may be far
            // apart): a long gap into the middle of a word is not a match.
            for k in (i - 1..j).rev() {
                let gap = (j - 1 - k) as i32;
                if gap > 3 && !boundary(text, j) {
                    break;
                }
                if score[i - 1][k] == NONE {
                    continue;
                }
                let joined = score[i - 1][k] + if gap == 0 { 12 } else { -gap };
                if joined > best {
                    best = joined;
                    parent = k;
                }
            }
            if parent != usize::MAX {
                score[i][j] = best + letter(j);
                from[i][j] = parent;
            }
        }
    }
    let (mut at, mut top) = (usize::MAX, NONE);
    for (j, &s) in score[n - 1].iter().enumerate() {
        if s > top {
            top = s;
            at = j;
        }
    }
    if at == usize::MAX {
        return None;
    }
    let mut hits = vec![0; n];
    let mut j = at;
    for i in (0..n).rev() {
        hits[i] = j;
        j = from[i][j];
    }
    Some((top.clamp(1, SCATTER_CAP), hits))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, name: &str, path: &str) -> PaletteEntry {
        PaletteEntry {
            key: id.to_owned(),
            id: id.to_owned(),
            name: name.to_owned(),
            path: path.to_owned(),
            shortcut: None,
            enabled: true,
        }
    }

    fn sample() -> Vec<PaletteEntry> {
        vec![
            entry("transport.input_quantize", "Input Quantize", "Transport"),
            entry("sequencer.audio_quantize", "Quantize Audio", "Edit"),
            entry("editor.quantize", "Quantize", "Edit"),
            entry("edit.quantize_options", "Quantize Options", "Edit"),
            entry("file.export_mp3", "Export MP3", "File > Export"),
            entry(
                "file.export_song_audio",
                "Export Song Audio",
                "File > Export",
            ),
            entry("help.guided_tour", "Guided Tour", "Help"),
            entry("edit.undo", "Undo", "Edit"),
        ]
    }

    fn names(entries: &[PaletteEntry], ranked: &[Ranked]) -> Vec<String> {
        ranked
            .iter()
            .map(|r| entries[r.entry].name.clone())
            .collect()
    }

    #[test]
    fn quan_ranks_quantize_first() {
        let entries = sample();
        let ranked = rank(&entries, "quan", &[]);
        let list = names(&entries, &ranked);
        assert_eq!(list[0], "Quantize", "{list:?}");
        // The names that start with the word come before the one that has it in the middle.
        assert_eq!(list.last().unwrap(), "Input Quantize", "{list:?}");
        assert_eq!(list.len(), 4);
        assert_eq!(ranked[0].name_hits, [0, 1, 2, 3]);
    }

    #[test]
    fn several_words_must_all_match() {
        let entries = sample();
        let list = names(&entries, &rank(&entries, "exp mp3", &[]));
        assert_eq!(list, ["Export MP3"]);
        let list = names(&entries, &rank(&entries, "exp aud", &[]));
        assert_eq!(list, ["Export Song Audio"]);
        // A word may match the menu path and another the name.
        let list = names(&entries, &rank(&entries, "edit undo", &[]));
        assert_eq!(list, ["Undo"]);
        // The arrow of a menu path is not a word.
        let list = names(&entries, &rank(&entries, "edit > undo", &[]));
        assert_eq!(list, ["Undo"]);
        assert!(rank(&entries, "exp zzz", &[]).is_empty());
    }

    #[test]
    fn scattered_letters_find_a_name_but_rank_below_a_whole_word() {
        let entries = sample();
        let ranked = rank(&entries, "gdtr", &[]);
        assert_eq!(names(&entries, &ranked), ["Guided Tour"]);
        assert_eq!(ranked[0].name_hits, [0, 3, 7, 10]);
        // "tour" is a whole word of one name and only scattered letters of the others.
        let more = vec![
            entry("a.tough_route", "Tough Route", "Edit"),
            entry("b.tour", "Tour", "Edit"),
        ];
        let list = names(&more, &rank(&more, "tour", &[]));
        assert_eq!(list[0], "Tour");
    }

    #[test]
    fn a_match_in_the_id_finds_a_command_by_its_dotted_name() {
        let entries = sample();
        let list = names(&entries, &rank(&entries, "guided_tour", &[]));
        assert_eq!(list, ["Guided Tour"]);
        // A name match beats the same text in the id.
        let list = names(&entries, &rank(&entries, "undo", &[]));
        assert_eq!(list, ["Undo"]);
    }

    #[test]
    fn the_empty_query_lists_the_recent_commands_first_then_every_command_by_name() {
        let entries = sample();
        let recent = vec!["help.guided_tour".to_owned(), "edit.undo".to_owned()];
        let ranked = rank(&entries, "", &recent);
        let list = names(&entries, &ranked);
        assert_eq!(list.len(), entries.len());
        assert_eq!(&list[..2], ["Guided Tour", "Undo"]);
        assert_eq!(list[2], "Export MP3");
        assert_eq!(list[3], "Export Song Audio");
        // Only spaces and arrows is still nothing typed.
        assert_eq!(rank(&entries, "  > ", &recent).len(), entries.len());
    }

    #[test]
    fn recent_commands_win_a_tie() {
        let entries = vec![
            entry("a.one", "Fold Rack", "View"),
            entry("b.two", "Fold Zone", "View"),
        ];
        let plain = names(&entries, &rank(&entries, "fold", &[]));
        assert_eq!(plain, ["Fold Rack", "Fold Zone"]);
        let recent = vec!["b.two".to_owned()];
        let list = names(&entries, &rank(&entries, "fold", &recent));
        assert_eq!(list, ["Fold Zone", "Fold Rack"]);
    }

    #[test]
    fn matching_ignores_case_and_highlights_the_path_too() {
        let entries = vec![entry("edit.undo", "Undo", "Edit")];
        let ranked = rank(&entries, "EDIT", &[]);
        assert_eq!(ranked.len(), 1);
        assert!(ranked[0].name_hits.is_empty());
        assert_eq!(ranked[0].path_hits, [0, 1, 2, 3]);
    }

    #[test]
    fn letters_too_far_apart_do_not_match() {
        let entries = vec![entry(
            "x.y",
            "A very long name that happens to hold every letter somewhere",
            "Edit",
        )];
        assert!(rank(&entries, "zq", &[]).is_empty());
        assert!(rank(&entries, "vw", &[]).is_empty());
    }
}
