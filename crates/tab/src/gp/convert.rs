//! The neutral Guitar Pro document into a [`TabScore`]: the timeline with repeats unrolled, the
//! tempo, meter, key and marker maps, the notes with their ties and hammer-ons, the chord diagrams.

use super::dom::{self, Doc, K, MAX_VOICES, MasterBar, Track as DomTrack};
use super::{GpError, Warnings, limits};
use crate::chord::ChordSymbol;
use crate::instrument::{Capo, Tuning, catalogue};
use crate::score::{BarInfo, DiagramRef, TabNote, TabScore, TabTrack, TechFlags, bar_ticks};
use crate::voicing::{Voicing, assign_fingers};

/// Turns the document into a score.
pub(crate) fn convert(mut doc: Doc, w: &mut Warnings) -> Result<TabScore, GpError> {
    if doc.master_bars.len() > limits::MAX_BARS {
        return Err(GpError::TooMany("bars"));
    }
    if doc.tracks.len() > limits::MAX_TRACKS {
        return Err(GpError::TooMany("tracks"));
    }
    dom::assign_lyrics(&mut doc);

    let mut score = TabScore::new();
    score.title = doc.info.title.clone();
    score.subtitle = doc.info.subtitle.clone();
    score.artist = doc.info.artist.clone();
    score.album = doc.info.album.clone();
    score.words_by = doc.info.words.clone();
    score.music_by = doc.info.music.clone();
    score.tab_by = doc.info.tab_by.clone();
    score.copyright = doc.info.copyright.clone();
    score.notices = doc.info.notices.clone();

    let plays = expand(&doc.master_bars, w);
    let starts = bar_starts(&doc.master_bars, &plays);

    let mut tracks: Vec<TabTrack> = doc.tracks.iter().map(|t| make_track(t, w)).collect();
    let mut linkers: Vec<Linker> = doc
        .tracks
        .iter()
        .map(|t| Linker::new(t.opens.len()))
        .collect();

    // The tempo in effect at the start of each bar in file order, and the changes inside it.
    let (bar_tempo, bar_events) = tempo_events(&doc);

    let mut seen = vec![false; doc.master_bars.len()];
    let mut first_tick = vec![None::<i64>; doc.master_bars.len()];
    let mut cur_tempo: Option<f64> = None;
    let mut cur_meter = (0u8, 0u8);
    let mut cur_key = None;
    let mut budget = limits::MAX_NOTES;
    let mut total_beats = 0usize;
    let mut truncated = false;

    for (play_no, (&bi, &tick0)) in plays.iter().zip(&starts).enumerate() {
        let mb = &doc.master_bars[bi];
        let first = !seen[bi];
        seen[bi] = true;
        if first {
            first_tick[bi] = Some(tick0);
        }
        if (mb.num, mb.den) != cur_meter {
            cur_meter = (mb.num, mb.den);
            score.meter.push((play_no as u32, mb.num, mb.den));
        }
        if mb.key.is_some() && mb.key != cur_key {
            cur_key = mb.key;
            if let Some(k) = mb.key {
                score.key.push((tick0, k));
            }
        }
        if let Some(m) = &mb.marker {
            score.markers.push((tick0, m.clone()));
        }
        // Tempo: restate the bar's starting tempo when a jump changed it, then the changes.
        let start_bpm = bar_tempo[bi];
        if cur_tempo != Some(start_bpm) {
            cur_tempo = Some(start_bpm);
            push_tempo(&mut score.tempo, tick0, start_bpm);
        }
        for (off, bpm) in &bar_events[bi] {
            if cur_tempo != Some(*bpm) {
                cur_tempo = Some(*bpm);
                push_tempo(&mut score.tempo, tick0 + units_to_ticks(*off), *bpm);
            }
        }

        for (ti, track) in doc.tracks.iter().enumerate() {
            let Some(bar) = track.bars.get(bi) else {
                continue;
            };
            for (vi, voice) in bar.voices.iter().enumerate().take(MAX_VOICES) {
                for beat in voice {
                    total_beats += 1;
                    if total_beats > limits::MAX_BEATS {
                        return Err(GpError::TooMany("beats"));
                    }
                    let tick = tick0 + units_to_ticks(beat.start);
                    let len = units_to_ticks(beat.units).max(1);
                    if first
                        && let Some(l) = &beat.lyric
                        && vi == 0
                    {
                        tracks[ti].lyrics.push((tick, l.clone()));
                    }
                    if let Some(d) = &beat.diagram {
                        add_diagram(&mut tracks[ti], tick, d);
                    }
                    for n in &beat.notes {
                        if budget == 0 {
                            truncated = true;
                            break;
                        }
                        budget -= 1;
                        let mut note = TabNote::new(tick, len, n.string, n.fret);
                        note.velocity = n.velocity;
                        note.tech = n.tech.clone();
                        note.tie = n.tie;
                        if !track.percussion {
                            linkers[ti].apply(vi, n, &mut note);
                        }
                        tracks[ti].notes.push(note);
                    }
                }
            }
        }
    }
    if truncated {
        w.warn("the score has too many notes; the rest were dropped");
    }

    // The original bar structure, one entry per bar of the file.
    let mut at = 0i64;
    for (i, mb) in doc.master_bars.iter().enumerate() {
        let len = bar_ticks(mb.num, mb.den);
        let tick = first_tick[i].unwrap_or(at);
        at = tick + len;
        score.bars.push(BarInfo {
            tick,
            len,
            repeat_open: mb.repeat_open,
            repeat_close: mb.repeat_close,
            volta: (mb.alt != 0).then_some(mb.alt),
        });
    }
    score.tracks = tracks;
    Ok(score)
}

fn units_to_ticks(u: i64) -> i64 {
    (u.max(0) + K / 2) / K
}

/// Adds a tempo change; a later one at the same tick replaces the earlier.
fn push_tempo(map: &mut Vec<(i64, f64)>, tick: i64, bpm: f64) {
    match map.last_mut() {
        Some(last) if last.0 == tick => last.1 = bpm,
        _ => map.push((tick, bpm)),
    }
}

/// The order the bars are played in, with repeats and alternative endings unrolled. The count of
/// bars and the length in ticks are capped.
pub(crate) fn expand(bars: &[MasterBar], w: &mut Warnings) -> Vec<usize> {
    let n = bars.len();
    let mut out = Vec::new();
    let (mut i, mut pass, mut start) = (0usize, 1u32, 0usize);
    let mut jumped = false;
    let mut ticks = 0i64;
    while i < n {
        let b = &bars[i];
        if out.len() >= limits::MAX_BARS || ticks > limits::MAX_TICKS {
            w.warn("the repeats make the song very long; the rest after the cap was cut");
            break;
        }
        if b.repeat_open && !jumped {
            start = i;
            pass = 1;
        }
        jumped = false;
        if b.alt != 0 && (b.alt >> (pass - 1).min(7)) & 1 == 0 {
            i += 1;
            continue;
        }
        out.push(i);
        ticks += bar_ticks(b.num, b.den);
        if let Some(count) = b.repeat_close {
            if pass < u32::from(count) {
                pass += 1;
                i = start;
                jumped = true;
                continue;
            }
            pass = 1;
            start = i + 1;
        }
        i += 1;
    }
    out
}

/// The start tick of each played bar.
fn bar_starts(bars: &[MasterBar], plays: &[usize]) -> Vec<i64> {
    let mut t = 0i64;
    plays
        .iter()
        .map(|&i| {
            let s = t;
            t += bar_ticks(bars[i].num, bars[i].den);
            s
        })
        .collect()
}

/// The tempo at the start of every bar (as it runs through the file, first pass) and the tempo
/// changes inside each bar, as `(position in sub-tick units, bpm)`.
#[allow(clippy::type_complexity)]
fn tempo_events(doc: &Doc) -> (Vec<f64>, Vec<Vec<(i64, f64)>>) {
    let mut running = if doc.tempo.is_finite() && doc.tempo > 0.0 {
        doc.tempo
    } else {
        120.0
    };
    let mut starts = Vec::with_capacity(doc.master_bars.len());
    let mut events = Vec::with_capacity(doc.master_bars.len());
    for (bi, mb) in doc.master_bars.iter().enumerate() {
        let bar_units = bar_ticks(mb.num, mb.den) * K;
        let mut ev: Vec<(i64, f64)> = Vec::new();
        for (pos, bpm) in &mb.tempo_auto {
            let off = (pos.clamp(0.0, 1.0) * bar_units as f64) as i64;
            ev.push((off, *bpm));
        }
        for t in &doc.tracks {
            if let Some(bar) = t.bars.get(bi) {
                for voice in &bar.voices {
                    for beat in voice {
                        if let Some(bpm) = beat.tempo {
                            ev.push((beat.start, bpm));
                        }
                    }
                }
            }
        }
        ev.retain(|(_, b)| b.is_finite() && *b > 0.0);
        ev.sort_by_key(|e| e.0);
        // An event at the very start replaces the bar's starting tempo.
        let mut start = running;
        let at_zero = ev.iter().take_while(|e| e.0 == 0).count();
        if at_zero > 0 {
            start = ev[at_zero - 1].1;
            ev.drain(..at_zero);
        }
        running = ev.last().map_or(start, |e| e.1);
        starts.push(start);
        events.push(ev);
    }
    (starts, events)
}

/// Remembers, per voice and string, the previous note so that ties inherit its fret and the
/// hammer-on and pull-off marks end up on the destination note.
struct Linker {
    last: Vec<Vec<Option<LastNote>>>,
}

#[derive(Clone, Copy)]
struct LastNote {
    fret: u8,
    origin: bool,
}

impl Linker {
    fn new(strings: usize) -> Self {
        Self {
            last: vec![vec![None; strings.max(1)]; MAX_VOICES],
        }
    }

    fn apply(&mut self, voice: usize, n: &dom::Note, note: &mut TabNote) {
        let Some(slot) = self
            .last
            .get_mut(voice)
            .and_then(|v| v.get_mut(usize::from(n.string)))
        else {
            return;
        };
        let prev = *slot;
        if n.tie {
            match prev {
                Some(p) => note.fret = p.fret,
                None => note.tie = false,
            }
        }
        if !note.tie
            && let Some(p) = prev
            && (p.origin || n.hopo_dest)
        {
            if note.fret > p.fret {
                note.tech.flags.insert(TechFlags::HAMMER_ON);
            } else if note.fret < p.fret {
                note.tech.flags.insert(TechFlags::PULL_OFF);
            }
        }
        let origin = n.hopo_origin || (note.tie && prev.is_some_and(|p| p.origin));
        *slot = Some(LastNote {
            fret: note.fret,
            origin,
        });
    }
}

fn add_diagram(track: &mut TabTrack, tick: i64, d: &dom::Diagram) {
    if d.frets.iter().all(Option::is_none) && d.name.is_empty() {
        return;
    }
    let frets: Vec<Option<u8>> = (0..track.tuning.len())
        .map(|i| d.frets.get(i).copied().flatten())
        .collect();
    let (fingers, barres) =
        assign_fingers(&frets).unwrap_or_else(|| (vec![None; frets.len()], Vec::new()));
    let top = frets.iter().flatten().copied().max().unwrap_or(0);
    let low = frets.iter().flatten().copied().filter(|f| *f > 0).min();
    let base_fret = match low {
        Some(l) if top > 5 => l,
        _ => 1,
    };
    track.diagrams.push(DiagramRef {
        tick,
        name: d.name.clone(),
        voicing: Voicing {
            frets,
            fingers,
            barres,
            base_fret,
        },
    });
    if let Ok(c) = ChordSymbol::parse(&d.name) {
        track.chords.push((tick, c));
    }
}

fn make_track(t: &DomTrack, w: &mut Warnings) -> TabTrack {
    let (id, tuning) = resolve_instrument(t, w);
    let mut out = TabTrack::new(&t.name, &id, tuning);
    out.capo = Capo::at(t.capo.min(crate::instrument::MAX_CAPO));
    out.midi_program = t.program.min(127);
    out.is_percussion = t.percussion;
    out
}

/// The instrument and tuning of a track: the catalogue tuning with the same open pitches when
/// there is one (so the banjo drone keeps its start fret, courses and labels), else a custom tuning
/// with the id of the family the string count and range suggest.
fn resolve_instrument(t: &DomTrack, w: &mut Warnings) -> (String, Tuning) {
    let frets = if (1..=crate::instrument::MAX_FRETS).contains(&t.frets) {
        t.frets
    } else {
        24
    };
    let opens = &t.opens;
    if t.percussion {
        return (String::new(), Tuning::from_opens(opens, 36));
    }
    let hint = if t.twelve_string {
        Some("guitar12")
    } else if t.banjo {
        Some("banjo")
    } else {
        None
    };
    let mut matches: Vec<(&'static str, &Tuning)> = Vec::new();
    for inst in catalogue() {
        for nt in &inst.tunings {
            if nt.tuning.opens() == *opens {
                matches.push((inst.id, &nt.tuning));
            }
        }
    }
    let found = hint
        .and_then(|h| matches.iter().find(|m| m.0.starts_with(h)))
        .or_else(|| matches.first())
        .copied();
    let (id, mut tuning) = match found {
        Some((id, tu)) => (id.to_owned(), tu.clone()),
        None => (fallback_id(t).to_owned(), Tuning::from_opens(opens, frets)),
    };
    if frets
        >= tuning
            .strings
            .iter()
            .map(|s| s.start_fret)
            .max()
            .unwrap_or(0)
    {
        tuning.frets = frets;
    }
    // Keep the neck inside the MIDI range.
    while tuning.validate().is_err() && tuning.frets > 1 {
        tuning.frets -= 1;
    }
    if tuning.validate().is_err() {
        w.warn("a track has a tuning outside the MIDI range; a guitar tuning was used");
        let std = crate::instrument::find_tuning("guitar6", "standard");
        return (
            "guitar6".to_owned(),
            std.map_or_else(
                || Tuning::from_opens(&[64, 59, 55, 50, 45, 40], 24),
                |n| n.tuning.clone(),
            ),
        );
    }
    (id, tuning)
}

/// A catalogue family for a tuning the catalogue does not list.
fn fallback_id(t: &DomTrack) -> &'static str {
    let top = t.opens.first().copied().unwrap_or(64);
    let low = top <= 52;
    match t.opens.len() {
        1..=3 => "ukulele_soprano",
        4 if low => "bass4",
        4 => "ukulele_soprano",
        5 if t.banjo => "banjo5",
        5 if low => "bass5",
        6 if low => "bass6",
        5 | 6 => "guitar6",
        7 => "guitar7",
        8 => "guitar8",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::score::PPQ;

    fn bar(open: bool, close: Option<u8>, alt: u8) -> MasterBar {
        let mut b = MasterBar::new(4, 4);
        b.repeat_open = open;
        b.repeat_close = close;
        b.alt = alt;
        b
    }

    fn run(bars: &[MasterBar]) -> Vec<usize> {
        expand(bars, &mut Warnings::default())
    }

    #[test]
    fn plain_bars_play_once() {
        assert_eq!(run(&vec![bar(false, None, 0); 3]), [0, 1, 2]);
        assert!(run(&[]).is_empty());
    }

    #[test]
    fn a_simple_repeat_plays_twice() {
        let b = [
            bar(false, None, 0),
            bar(true, None, 0),
            bar(false, Some(2), 0),
            bar(false, None, 0),
        ];
        assert_eq!(run(&b), [0, 1, 2, 1, 2, 3]);
    }

    #[test]
    fn a_repeat_without_an_open_goes_back_to_the_start() {
        let b = [
            bar(false, None, 0),
            bar(false, Some(3), 0),
            bar(false, None, 0),
        ];
        assert_eq!(run(&b), [0, 1, 0, 1, 0, 1, 2]);
    }

    #[test]
    fn alternative_endings_follow_the_pass() {
        // |: A B | 1. C :| 2. D | E
        let b = [
            bar(true, None, 0),
            bar(false, None, 0),
            bar(false, Some(2), 0b01),
            bar(false, None, 0b10),
            bar(false, None, 0),
        ];
        assert_eq!(run(&b), [0, 1, 2, 0, 1, 3, 4]);
    }

    #[test]
    fn two_repeats_in_a_row_are_independent() {
        let b = [
            bar(true, None, 0),
            bar(false, Some(2), 0),
            bar(true, None, 0),
            bar(false, Some(3), 0),
        ];
        assert_eq!(run(&b), [0, 1, 0, 1, 2, 3, 2, 3, 2, 3]);
    }

    #[test]
    fn endless_repeats_are_capped() {
        let b: Vec<MasterBar> = (0..50)
            .map(|i| bar(i % 2 == 0, (i % 2 == 1).then_some(255), 0))
            .collect();
        let played = run(&b);
        assert!(played.len() <= limits::MAX_BARS);
    }

    #[test]
    fn tuplet_units_add_up() {
        // Seven septuplet sixteenths fill a quarter-note slot exactly.
        let sixteenth = dom::WHOLE / 16;
        let one = sixteenth * 4 / 7;
        assert_eq!(one * 7, sixteenth * 4);
        assert_eq!(units_to_ticks(one * 7), PPQ);
    }
}
