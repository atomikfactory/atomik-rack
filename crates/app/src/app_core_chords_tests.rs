//! Chords and keys from audio in the app core: a worker listens to generated chord progressions in
//! temporary WAV files, the preview scores the kept analysis, and putting the chords in the lane
//! (with the song key) is one undo step.

use super::*;
use crate::sink::RecordingSink;
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_core::PPQ;
use atomik_core::theory::ChordKind;
use atomik_model::empty_doc;
use atomik_model::harmony::{ChordEvent, Key};
use atomik_model::theory::ScaleKind;
use atomik_ui::{ChordChanges, ChordsValues, ImportTarget, NotesMode, NotesValues};
use std::sync::atomic::{AtomicU64, Ordering};

const SR: u32 = 48_000;
/// 120 BPM in 4/4: a bar is two seconds.
const BAR: i64 = 4 * PPQ;

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-app-chords-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn core() -> AppCore<RecordingSink> {
    let mut c = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    c.set_warp_imports(crate::import_warp::WarpImports::Off);
    c.sink_mut().take();
    c
}

fn frames(seconds: f64) -> usize {
    (seconds * f64::from(SR)).round() as usize
}

/// A piano-like tone of `key` (six partials, each decaying faster than the one below).
fn tone(x: &mut [f32], key: u8, from: f64, to: f64) {
    let (a, b) = (frames(from), frames(to).min(x.len()));
    let f0 = 440.0 * ((f64::from(key) - 69.0) / 12.0).exp2();
    let (attack, release) = (frames(0.004) as f64, frames(0.02) as f64);
    for (i, v) in x[a..b].iter_mut().enumerate() {
        let t = i as f64 / f64::from(SR);
        let env = (i as f64 / attack)
            .min((b - a - i) as f64 / release)
            .min(1.0);
        let mut s = 0.0;
        for h in 1..=6 {
            let fall = (-t * 0.6 * (1.0 + 0.6 * h as f64)).exp();
            s += fall / h as f64 * (std::f64::consts::TAU * f0 * h as f64 * t).sin();
        }
        *v += (0.08 * env * s) as f32;
    }
}

/// The chords, a bar each, struck on beats 1 and 3.
fn progression(chords: &[&[u8]]) -> Vec<f32> {
    let mut x = vec![0.0f32; frames(2.0 * chords.len() as f64)];
    for (i, keys) in chords.iter().enumerate() {
        for half in 0..2 {
            let from = 2.0 * i as f64 + f64::from(half);
            for k in *keys {
                tone(&mut x, *k, from, from + 1.0);
            }
        }
    }
    x
}

/// C, G, Am, F with the root in the bass.
const POP: [&[u8]; 4] = [
    &[36, 60, 64, 67],
    &[43, 67, 71, 74],
    &[45, 69, 72, 76],
    &[41, 65, 69, 72],
];

/// Am, Dm, Em, Am.
const MINOR: [&[u8]; 4] = [
    &[45, 69, 72, 76],
    &[38, 62, 65, 69],
    &[40, 64, 67, 71],
    &[45, 69, 72, 76],
];

/// Imports a generated file at tick 0 on a new track and selects its clip.
fn import(c: &mut AppCore<RecordingSink>, dir: &Tmp, data: Vec<f32>) -> ClipId {
    let path = dir.0.join("chords.wav");
    write_wav_f32(&path, &AudioData::new(SR, vec![data]).unwrap()).unwrap();
    c.import_audio(vec![path], ImportTarget::NewTracks, 0);
    c.wait_background();
    assert!(c.last_rejection().is_none(), "{:?}", c.last_rejection());
    let id = c.doc().song.tracks.last().unwrap().lanes[0].clips[0].id;
    c.arrangement_mut().set_selected_clips(&[id]);
    id
}

fn per_bar() -> ChordsValues {
    ChordsValues {
        changes: ChordChanges::Bar,
        ..ChordsValues::default()
    }
}

fn listened(chords: &[&[u8]]) -> (AppCore<RecordingSink>, Tmp) {
    let dir = Tmp::new("pop");
    let mut c = core();
    import(&mut c, &dir, progression(chords));
    assert!(c.detect_chords_from_audio(), "{:?}", c.last_rejection());
    assert!(c.chords_busy());
    c.wait_background();
    assert!(!c.chords_busy());
    (c, dir)
}

#[test]
fn listening_to_a_clip_previews_its_chords_bar_by_bar() {
    let (mut c, _dir) = listened(&POP);
    assert!(
        c.chords_target().starts_with("chords"),
        "{}",
        c.chords_target()
    );
    let preview = c.chords_preview(&per_bar()).unwrap();
    let got: Vec<(i64, i64, &str)> = preview
        .chords
        .iter()
        .map(|p| (p.tick, p.length, p.symbol.as_str()))
        .collect();
    assert_eq!(
        got,
        vec![
            (0, BAR, "C"),
            (BAR, BAR, "G"),
            (2 * BAR, BAR, "Am"),
            (3 * BAR, BAR, "F")
        ]
    );
    assert!(preview.chords.iter().all(|p| p.confidence > 0.5));
    assert_eq!(preview.span, (0, 4 * BAR));
    assert_eq!(preview.key.as_deref(), Some("C major"));
    // Per beat the same chords hold, each over its four beats.
    let beats = ChordsValues {
        changes: ChordChanges::Beat,
        ..per_bar()
    };
    let fine = c.chords_preview(&beats).unwrap();
    let symbols: Vec<&str> = fine.chords.iter().map(|p| p.symbol.as_str()).collect();
    assert_eq!(symbols, ["C", "G", "Am", "F"]);
}

#[test]
fn putting_the_chords_in_the_lane_is_one_undo_step_with_the_key() {
    let (mut c, _dir) = listened(&POP);
    let before = c.document.history_len();
    assert!(c.apply_detected_chords(&per_bar(), false));
    assert_eq!(c.document.history_len(), before + 1);
    let lane: Vec<(i64, ChordKind, u8)> = c
        .harmony()
        .chords
        .iter()
        .map(|e| (e.tick, e.quality, e.root))
        .collect();
    assert_eq!(
        lane,
        vec![
            (0, ChordKind::Major, 0),
            (BAR, ChordKind::Major, 7),
            (2 * BAR, ChordKind::Minor, 9),
            (3 * BAR, ChordKind::Major, 5)
        ]
    );
    assert_eq!(c.harmony().key, Some(Key::new(0, ScaleKind::Major)));
    c.undo();
    assert!(c.harmony().chords.is_empty());
    assert_eq!(c.harmony().key, None);
}

#[test]
fn without_set_key_the_key_stays_and_merge_keeps_the_chords_there() {
    let (mut c, _dir) = listened(&POP);
    let mine = ChordEvent::new(BAR, BAR, 2, ChordKind::Sus4);
    let h = c.harmony().with_chord(mine).unwrap();
    c.set_harmony(h).unwrap();
    let values = ChordsValues {
        set_key: false,
        ..per_bar()
    };
    let before = c.document.history_len();
    assert!(c.apply_detected_chords(&values, true));
    assert_eq!(c.document.history_len(), before + 1);
    assert_eq!(c.harmony().key, None);
    let symbols: Vec<String> = c.harmony().chords.iter().map(|e| e.symbol()).collect();
    assert_eq!(symbols, ["C", "Dsus4", "Am", "F"]);
    // Replace puts the detected G back over the bar.
    assert!(c.apply_detected_chords(&values, false));
    let symbols: Vec<String> = c.harmony().chords.iter().map(|e| e.symbol()).collect();
    assert_eq!(symbols, ["C", "G", "Am", "F"]);
}

#[test]
fn a_song_with_a_key_is_not_offered_another() {
    let (mut c, _dir) = listened(&POP);
    c.set_song_key(Some(Key::new(2, ScaleKind::Dorian)))
        .unwrap();
    assert_eq!(c.chords_preview(&per_bar()).unwrap().key, None);
    assert!(c.apply_detected_chords(&per_bar(), false));
    assert_eq!(c.harmony().key, Some(Key::new(2, ScaleKind::Dorian)));
}

#[test]
fn nothing_selected_and_no_loop_is_refused() {
    let dir = Tmp::new("none");
    let mut c = core();
    import(&mut c, &dir, progression(&POP));
    c.arrangement_mut().set_selected_clips(&[]);
    assert!(!c.detect_chords_from_audio());
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.starts_with("Select audio clips"))
    );
    assert!(!c.apply_detected_chords(&per_bar(), false));
}

#[test]
fn a_stopped_analysis_changes_nothing() {
    let dir = Tmp::new("stop");
    let mut c = core();
    import(&mut c, &dir, progression(&POP));
    assert!(c.detect_chords_from_audio());
    assert!(c.cancel_chords());
    c.wait_background();
    assert!(!c.chords_busy());
    assert!(c.chords_preview(&per_bar()).is_none());
    assert!(c.harmony().is_empty());
    assert!(!c.cancel_chords(), "nothing runs any more");
}

#[test]
fn the_key_of_a_minor_progression_is_read_from_the_audio() {
    let dir = Tmp::new("key");
    let mut c = core();
    import(&mut c, &dir, progression(&MINOR));
    let before = c.document.history_len();
    assert!(c.detect_key_from_audio());
    c.wait_background();
    assert_eq!(c.harmony().key, Some(Key::new(9, ScaleKind::NaturalMinor)));
    assert_eq!(c.document.history_len(), before + 1);
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.starts_with("Song key set to A minor from the audio")),
        "{:?}",
        c.last_rejection()
    );
}

#[test]
fn detect_song_key_reads_the_audio_when_there_are_no_notes() {
    let dir = Tmp::new("fallback");
    let mut c = core();
    import(&mut c, &dir, progression(&POP));
    c.arrangement_mut().set_selected_clips(&[]);
    assert_eq!(c.detect_song_key(), None, "the key comes from the worker");
    c.wait_background();
    assert_eq!(c.harmony().key, Some(Key::new(0, ScaleKind::Major)));
}

#[test]
fn polyphonic_audio_to_notes_makes_the_notes_of_a_chord() {
    let dir = Tmp::new("poly");
    let mut c = core();
    let mut x = vec![0.0f32; frames(2.0)];
    for k in [60, 64, 67] {
        tone(&mut x, k, 0.5, 1.5);
    }
    import(&mut c, &dir, x);
    let values = NotesValues {
        mode: NotesMode::Polyphonic,
        ..NotesValues::default()
    };
    assert!(c.audio_to_notes(&values), "{:?}", c.last_rejection());
    c.wait_background();
    let clip = &c.doc().song.tracks.last().unwrap().lanes[0].clips[0];
    let ClipContent::Notes(notes) = &clip.content else {
        panic!("no note clip");
    };
    let mut keys: Vec<u8> = notes.iter().map(|n| n.key).collect();
    keys.sort_unstable();
    assert_eq!(keys, [60, 64, 67]);
    for n in notes.iter() {
        assert!((n.tick - PPQ).abs() <= PPQ / 16, "{n:?}");
    }
}
