//! How the results of listen-and-grade look: the colour and the badge of each verdict, and the
//! marks the app hands to the Tab Editor and the fretboard. The grading itself is
//! [`atomik_tab::grade`]; nothing here decides anything.

use crate::theme::Theme;
use atomik_tab::grade::Verdict;
use vello::peniko::Color;

/// The verdict of one note of the open clip, by its content tick and key (the Tab Editor looks
/// notes up by them).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GradeMark {
    /// Content tick of the note.
    pub tick: i64,
    /// Its MIDI key.
    pub key: u8,
    /// How it was played.
    pub verdict: Verdict,
}

/// The verdict of a note by where it is played, for the fretboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FretGrade {
    /// Content tick of the note.
    pub tick: i64,
    /// The string, in tab order.
    pub string: u8,
    /// The fret, relative to the capo.
    pub fret: u8,
    /// How it was played.
    pub verdict: Verdict,
}

/// The colour of a verdict: green for a hit, amber for early or late, red for a wrong note and a
/// dimmer red for a missed one.
pub fn colour(theme: &Theme, verdict: Verdict) -> Color {
    match verdict {
        Verdict::Hit => theme.meter_low,
        Verdict::Early | Verdict::Late => theme.meter_mid,
        Verdict::Wrong => theme.clip,
        Verdict::Missed => theme.warning,
    }
}

/// The small mark that tells verdicts apart without colour: none for a hit.
pub fn badge(verdict: Verdict) -> Option<&'static str> {
    match verdict {
        Verdict::Hit => None,
        Verdict::Early => Some("<"),
        Verdict::Late => Some(">"),
        Verdict::Wrong => Some("x"),
        Verdict::Missed => Some("-"),
    }
}

/// The marks of notes `(tick, key)` sorted for lookup.
pub fn sorted(mut marks: Vec<GradeMark>) -> Vec<GradeMark> {
    marks.sort_by_key(|m| (m.tick, m.key));
    marks
}

/// The verdict of the note at `(tick, key)` in marks made by [`sorted`].
pub fn verdict_of(marks: &[GradeMark], tick: i64, key: u8) -> Option<Verdict> {
    marks
        .binary_search_by_key(&(tick, key), |m| (m.tick, m.key))
        .ok()
        .map(|i| marks[i].verdict)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_are_found_by_tick_and_key_and_every_verdict_has_its_own_look() {
        let marks = sorted(vec![
            GradeMark {
                tick: 960,
                key: 45,
                verdict: Verdict::Late,
            },
            GradeMark {
                tick: 0,
                key: 40,
                verdict: Verdict::Hit,
            },
            GradeMark {
                tick: 0,
                key: 52,
                verdict: Verdict::Wrong,
            },
        ]);
        assert_eq!(verdict_of(&marks, 0, 52), Some(Verdict::Wrong));
        assert_eq!(verdict_of(&marks, 960, 45), Some(Verdict::Late));
        assert_eq!(verdict_of(&marks, 960, 46), None);
        let t = Theme::dark();
        let all = [
            Verdict::Hit,
            Verdict::Early,
            Verdict::Late,
            Verdict::Wrong,
            Verdict::Missed,
        ];
        let looks: Vec<_> = all
            .iter()
            .map(|v| (format!("{:?}", colour(&t, *v)), badge(*v)))
            .collect();
        // Early and late share a colour but not a badge; no two others share either.
        assert_ne!(looks[1].1, looks[2].1);
        assert_eq!(looks[1].0, looks[2].0);
        for (i, a) in looks.iter().enumerate() {
            for (j, b) in looks.iter().enumerate() {
                if i < j && !(i == 1 && j == 2) {
                    assert!(a != b, "{i} and {j} look alike");
                }
            }
        }
    }
}
