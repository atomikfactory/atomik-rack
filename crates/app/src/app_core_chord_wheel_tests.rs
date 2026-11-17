//! `AppCore` tests of the Chord Wheel commands: Generate Progression, editing a slot, Send
//! Progression to Chord Lane and to Clip, and the audition.

use super::*;
use crate::app_core::tests::rack_only_core;
use crate::sink::{RecordingSink, SinkCall};
use atomik_core::NoteKind;
use atomik_model::theory::{ChordKind, ScaleKind};
use atomik_model::{ChordEvent, Harmony, Key as SongKey, SongCommand};

const SYNTH: &str = "atomik.poly-synth";
const BAR: i64 = 4 * PPQ;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

/// A core with synth `a` on a track and a Chord Wheel above it, the track selected.
fn with_wheel() -> (AppCore<RecordingSink>, DeviceKey) {
    let mut c = rack_only_core();
    for command in [
        Command::AddDevice {
            index: usize::MAX,
            type_id: SYNTH.to_string(),
            key: Some(key("a")),
        },
        Command::Song(SongCommand::AddTrack { device: key("a") }),
        Command::Workflow(atomik_model::WorkflowCommand::CreatePlayer {
            type_id: CHORD_WHEEL.into(),
            target: atomik_model::CreateTarget::Selected(key("a")),
            options: atomik_model::CreateOptions::default(),
        }),
    ] {
        c.apply_command(command).expect("accepted");
    }
    c.arrangement_mut().set_selected_track(Some(key("a")));
    c.sink_mut().take();
    let wheel = c.chord_wheel_selected().expect("the wheel is found");
    (c, wheel)
}

fn set(c: &mut AppCore<RecordingSink>, wheel: &DeviceKey, param: &str, value: f32) {
    c.apply_command(Command::SetParam {
        key: wheel.clone(),
        param: param.into(),
        value,
        merge: None,
    })
    .expect("param set");
}

fn value(c: &AppCore<RecordingSink>, wheel: &DeviceKey, param: &str) -> f32 {
    c.wheel_value(wheel, param)
}

/// The chords and lengths of the active slots as the device holds them.
fn progression(c: &AppCore<RecordingSink>, wheel: &DeviceKey) -> Vec<(Chord, u8)> {
    (0..c.wheel_slots(wheel))
        .map(|k| (c.wheel_chord(wheel, k), c.wheel_beats(wheel, k)))
        .collect()
}

fn lane(c: &AppCore<RecordingSink>) -> Vec<(i64, i64, u8, ChordKind)> {
    c.harmony()
        .chords
        .iter()
        .map(|e| (e.tick, e.length, e.root, e.quality))
        .collect()
}

fn roots(c: &AppCore<RecordingSink>) -> Vec<u8> {
    c.harmony().chords.iter().map(|e| e.root).collect()
}

fn set_loop(c: &mut AppCore<RecordingSink>, range: (i64, i64), enabled: bool) {
    c.apply_command(Command::Song(SongCommand::SetLoop { range, enabled }))
        .expect("loop set");
}

fn chord(root: u8, kind: ChordKind) -> Chord {
    Chord::new(root, kind)
}

fn live_keys(c: &mut AppCore<RecordingSink>) -> Vec<u8> {
    c.sink_mut()
        .take()
        .into_iter()
        .filter_map(|call| match call {
            SinkCall::LiveNote {
                kind: NoteKind::On { key, .. },
                ..
            } => Some(key),
            _ => None,
        })
        .collect()
}

#[test]
fn the_selected_wheel_is_found_from_the_device_or_the_track() {
    let (mut c, wheel) = with_wheel();
    c.view_mut().set_selected(None);
    assert_eq!(
        c.chord_wheel_selected(),
        Some(wheel.clone()),
        "by the track"
    );
    assert!(c.freeze_menu().wheel);
    c.arrangement_mut().set_selected_track(None);
    assert_eq!(c.chord_wheel_selected(), None);
    assert!(!c.freeze_menu().wheel);
    c.view_mut().set_selected(Some(wheel.clone()));
    assert_eq!(c.chord_wheel_selected(), Some(wheel), "by the device");
    c.view_mut().set_selected(Some(key("a")));
    assert_eq!(c.chord_wheel_selected(), None, "a synth is no wheel");
}

#[test]
fn the_commands_need_a_selected_wheel() {
    let (mut c, _) = with_wheel();
    c.arrangement_mut().set_selected_track(None);
    c.view_mut().set_selected(None);
    let song = c.doc().song.clone();
    assert!(!c.wheel_generate_selected());
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.contains("Chord Wheel"))
    );
    assert!(!c.wheel_to_chord_lane_selected());
    assert!(!c.wheel_to_clip_selected());
    assert!(!c.wheel_generate(&key("a")), "a synth is no wheel");
    assert_eq!(c.doc().song, song);
}

#[test]
fn generate_keeps_the_locked_slots_in_one_undo_step_and_differs_per_press() {
    let (mut c, wheel) = with_wheel();
    set(&mut c, &wheel, "slots", 6.0);
    let kept = chord(2, ChordKind::Minor);
    assert!(c.wheel_set_chord(&wheel, 1, kept));
    assert!(c.wheel_toggle_lock(&wheel, 1));
    assert!(c.wheel_locked(&wheel, 1));
    let before = c.doc().device(&wheel).unwrap().params.clone();
    let beats_before = c.wheel_beats(&wheel, 1);

    let mut seen: Vec<Vec<(Chord, u8)>> = Vec::new();
    for press in 1..=8 {
        let held = c.doc().device(&wheel).unwrap().params.clone();
        assert!(c.wheel_generate(&wheel));
        assert_eq!(
            value(&c, &wheel, "gen.seed"),
            press as f32,
            "the seed advances"
        );
        let now = progression(&c, &wheel);
        assert_eq!(now.len(), 6);
        assert_eq!(now[1].0, kept, "the locked chord stays");
        assert_eq!(now[1].1, beats_before, "and its length");
        seen.push(now);
        // One press is one undo step: the seed and every chord come back.
        assert!(c.undo());
        assert_eq!(c.doc().device(&wheel).unwrap().params, held);
        assert!(c.redo());
    }
    assert!(
        seen.iter().any(|p| *p != seen[0]),
        "other seeds give other progressions"
    );
    assert_ne!(c.doc().device(&wheel).unwrap().params, before);
    // The generator is deterministic: the same params and seed give the same chords.
    let (mut fresh, w) = with_wheel();
    set(&mut fresh, &w, "slots", 6.0);
    assert!(fresh.wheel_set_chord(&w, 1, kept));
    assert!(fresh.wheel_toggle_lock(&w, 1));
    assert!(fresh.wheel_generate(&w));
    assert_eq!(progression(&fresh, &w), seen[0]);
}

#[test]
fn generate_wraps_the_seed_and_leaves_the_unused_slots_alone() {
    let (mut c, wheel) = with_wheel();
    set(&mut c, &wheel, "gen.mood", 11.0);
    set(&mut c, &wheel, "gen.seed", 9999.0);
    let spare = chord(1, ChordKind::Dim);
    assert!(c.wheel_set_chord(&wheel, 6, spare));
    assert!(c.wheel_generate(&wheel));
    assert_eq!(
        value(&c, &wheel, "gen.seed"),
        0.0,
        "past 9999 the seed starts again"
    );
    assert_eq!(
        c.wheel_chord(&wheel, 6),
        spare,
        "a slot past `slots` is not written"
    );
    for k in 0..4 {
        assert!(atomik_core::progression::BEATS.contains(&c.wheel_beats(&wheel, k)));
    }
}

#[test]
fn a_slot_is_set_in_one_step_and_consecutive_sets_of_it_merge() {
    let (mut c, wheel) = with_wheel();
    let start = c.doc().device(&wheel).unwrap().params.clone();
    let over = Chord::new(7, ChordKind::MinorSeventh).over(10);
    assert!(c.wheel_set_chord(&wheel, 2, over));
    assert_eq!(c.wheel_chord(&wheel, 2), over);
    assert_eq!(value(&c, &wheel, "slot.3.bass"), 11.0, "slash bass B flat");
    assert!(c.wheel_set_chord(&wheel, 2, chord(4, ChordKind::MajorSeventh)));
    assert_eq!(c.wheel_chord(&wheel, 2), chord(4, ChordKind::MajorSeventh));
    assert!(c.undo());
    assert_eq!(
        c.doc().device(&wheel).unwrap().params,
        start,
        "two sets of one slot are one step"
    );
    assert!(c.wheel_set_chord(&wheel, 0, chord(2, ChordKind::Minor)));
    assert!(c.wheel_set_chord(&wheel, 1, chord(9, ChordKind::Minor)));
    assert!(c.undo());
    assert_eq!(c.wheel_chord(&wheel, 0), chord(2, ChordKind::Minor));
    assert_ne!(
        c.wheel_chord(&wheel, 1),
        chord(9, ChordKind::Minor),
        "another slot"
    );
    assert!(!c.wheel_set_chord(&wheel, 8, chord(0, ChordKind::Major)));
}

#[test]
fn locks_and_the_slot_count_are_one_undo_step_each() {
    let (mut c, wheel) = with_wheel();
    assert!(c.wheel_toggle_lock(&wheel, 3));
    assert!(c.wheel_locked(&wheel, 3));
    assert!(c.wheel_toggle_lock(&wheel, 3));
    assert!(!c.wheel_locked(&wheel, 3));
    assert!(c.undo());
    assert!(c.wheel_locked(&wheel, 3));
    assert!(c.wheel_set_slots(&wheel, 8));
    assert_eq!(c.wheel_slots(&wheel), 8);
    assert!(c.wheel_set_slots(&wheel, 99));
    assert_eq!(c.wheel_slots(&wheel), 8, "clamped");
    assert!(c.wheel_set_slots(&wheel, 0));
    assert_eq!(c.wheel_slots(&wheel), 1);
    assert!(c.undo());
    assert_eq!(c.wheel_slots(&wheel), 8);
}

#[test]
fn the_progression_goes_to_the_chord_lane_once_from_the_left_locator() {
    let (mut c, wheel) = with_wheel();
    set_loop(&mut c, (2 * BAR, 10 * BAR), false);
    assert!(c.harmony().is_empty());
    assert!(c.wheel_to_chord_lane(&wheel));
    assert_eq!(
        lane(&c),
        vec![
            (2 * BAR, BAR, 0, ChordKind::Major),
            (3 * BAR, BAR, 7, ChordKind::Major),
            (4 * BAR, BAR, 9, ChordKind::Minor),
            (5 * BAR, BAR, 5, ChordKind::Major),
        ]
    );
    assert_eq!(
        c.harmony().key,
        Some(SongKey::new(0, ScaleKind::Major)),
        "a song with no key gets the device key"
    );
    // One undo step takes the chords and the key away.
    assert!(c.undo());
    assert!(c.harmony().is_empty());
}

#[test]
fn slot_lengths_and_slash_basses_set_the_lane_spans() {
    let (mut c, wheel) = with_wheel();
    set(&mut c, &wheel, "slots", 3.0);
    set(&mut c, &wheel, "slot.1.beats", 5.0); // 8 beats
    set(&mut c, &wheel, "slot.2.beats", 1.0); // 2 beats
    set(&mut c, &wheel, "slot.3.beats", 0.0); // 1 beat
    assert!(c.wheel_set_chord(&wheel, 1, Chord::new(7, ChordKind::Major).over(11)));
    set_loop(&mut c, (0, BAR), false);
    assert!(c.wheel_to_chord_lane(&wheel));
    let spans: Vec<(i64, i64)> = c
        .harmony()
        .chords
        .iter()
        .map(|e| (e.tick, e.length))
        .collect();
    assert_eq!(spans, [(0, 8 * PPQ), (8 * PPQ, 2 * PPQ), (10 * PPQ, PPQ)]);
    assert_eq!(c.harmony().chords[1].bass, Some(11));
    assert_eq!(c.harmony().chords[0].bass, None);
}

#[test]
fn the_loop_repeats_the_progression_and_cuts_the_last_chord_at_its_end() {
    let (mut c, wheel) = with_wheel();
    // Five and a half bars from bar 2: C G Am F, then C, then half of G.
    set_loop(&mut c, (BAR, BAR + 5 * BAR + 2 * PPQ), true);
    assert!(c.wheel_to_chord_lane(&wheel));
    assert_eq!(roots(&c), [0, 7, 9, 5, 0, 7]);
    let last = c.harmony().chords.last().unwrap();
    assert_eq!((last.tick, last.end()), (6 * BAR, 6 * BAR + 2 * PPQ));
    assert_eq!(c.harmony().chords[0].tick, BAR, "from the loop start");
}

#[test]
fn a_following_wheel_moves_the_chords_to_the_song_key() {
    let (mut c, wheel) = with_wheel();
    set(&mut c, &wheel, "key.source", 1.0);
    let song_key = SongKey::new(2, ScaleKind::Major);
    c.set_song_key(Some(song_key)).unwrap();
    set_loop(&mut c, (0, BAR), false);
    assert!(c.wheel_to_chord_lane(&wheel));
    assert_eq!(roots(&c), [2, 9, 11, 7], "D A Bm G");
    assert_eq!(c.harmony().key, Some(song_key), "the song key stays");

    // A minor song key maps to its relative major: F minor holds the chords of A flat major.
    c.undo();
    c.set_song_key(Some(SongKey::new(5, ScaleKind::NaturalMinor)))
        .unwrap();
    assert!(c.wheel_to_chord_lane(&wheel));
    assert_eq!(roots(&c), [8, 3, 5, 1], "Ab Eb Fm Db");
    // A device that does not follow the song writes the chords as they are.
    c.undo();
    set(&mut c, &wheel, "key.source", 0.0);
    assert!(c.wheel_to_chord_lane(&wheel));
    assert_eq!(roots(&c), [0, 7, 9, 5]);
    assert_eq!(
        c.harmony().key,
        Some(SongKey::new(5, ScaleKind::NaturalMinor)),
        "a song key that is there is not replaced"
    );
}

#[test]
fn the_chords_inside_the_span_are_replaced_and_the_others_stay() {
    let (mut c, wheel) = with_wheel();
    let mut h = Harmony::default();
    for (tick, length, root) in [(0, 8 * BAR, 3u8), (20 * BAR, BAR, 6)] {
        h = h
            .with_chord(ChordEvent::new(tick, length, root, ChordKind::Minor))
            .unwrap();
    }
    c.set_harmony(h).unwrap();
    set_loop(&mut c, (2 * BAR, 6 * BAR), false);
    assert!(c.wheel_to_chord_lane(&wheel));
    assert_eq!(
        lane(&c),
        vec![
            (0, 2 * BAR, 3, ChordKind::Minor),
            (2 * BAR, BAR, 0, ChordKind::Major),
            (3 * BAR, BAR, 7, ChordKind::Major),
            (4 * BAR, BAR, 9, ChordKind::Minor),
            (5 * BAR, BAR, 5, ChordKind::Major),
            (6 * BAR, 2 * BAR, 3, ChordKind::Minor),
            (20 * BAR, BAR, 6, ChordKind::Minor),
        ],
        "the chord that ran through the span keeps its two ends"
    );
    assert!(c.undo());
    assert_eq!(c.harmony().chords.len(), 2, "one undo step");
    // Sending twice changes nothing the second time and adds no undo step.
    assert!(c.wheel_to_chord_lane(&wheel));
    assert!(c.wheel_to_chord_lane(&wheel));
    assert!(c.undo());
    assert_eq!(c.harmony().chords.len(), 2);
}

#[test]
fn a_chord_click_plays_the_voiced_chord_through_the_wheels_track() {
    let (mut c, wheel) = with_wheel();
    c.wheel_audition(&wheel, Chord::new(9, ChordKind::Minor).over(4));
    let ons = live_keys(&mut c);
    assert!(ons.len() >= 4, "the triad and the slash bass: {ons:?}");
    let mut classes: Vec<u8> = ons.iter().map(|k| k % 12).collect();
    classes.sort_unstable();
    classes.dedup();
    assert_eq!(classes, [0, 4, 9], "A minor over E");
    assert_eq!(ons[0] % 12, 4, "the bass comes first and lowest");
    assert!(ons[0] < *ons[1..].iter().min().unwrap());
    assert!(
        ons[1..].iter().all(|k| (48..=84).contains(k)),
        "the register: {ons:?}"
    );
    assert!(c.doc().song.harmony.is_empty(), "the document is untouched");
    // The notes end by themselves.
    c.feed_clip_preview(c.now_seconds + 2.0);
    let offs = c
        .sink_mut()
        .take()
        .into_iter()
        .filter(|call| {
            matches!(
                call,
                SinkCall::LiveNote {
                    kind: NoteKind::Off { .. },
                    ..
                }
            )
        })
        .count();
    assert_eq!(offs, ons.len());
}

#[test]
fn the_audition_follows_the_register_and_is_silent_while_the_song_plays() {
    let (mut c, wheel) = with_wheel();
    set(&mut c, &wheel, "voice.register", 0.0);
    c.wheel_audition(&wheel, chord(0, ChordKind::Major));
    let low = live_keys(&mut c);
    assert!(
        !low.is_empty() && low.iter().all(|k| *k < 48),
        "low: {low:?}"
    );
    c.stop_preview();
    set(&mut c, &wheel, "voice.register", 3.0);
    c.wheel_audition(&wheel, chord(0, ChordKind::Major));
    let high = live_keys(&mut c);
    assert!(
        !high.is_empty() && high.iter().all(|k| *k >= 60),
        "high: {high:?}"
    );
    c.stop_preview();
    c.sink_mut().take();
    c.playing = true;
    c.wheel_audition(&wheel, chord(0, ChordKind::Major));
    assert!(
        live_keys(&mut c).is_empty(),
        "no audition while the song plays"
    );
    assert!(
        !c.sink()
            .calls
            .iter()
            .any(|call| matches!(call, SinkCall::PreviewStart { .. })),
        "nor the built-in voice"
    );
}

fn clip_notes(c: &AppCore<RecordingSink>, lane: usize) -> Vec<(i64, i64, u8)> {
    let t = c.doc().song.track(&key("a")).unwrap();
    let ClipContent::Notes(notes) = &t.lanes[lane].clips[0].content else {
        panic!("a note clip");
    };
    notes.iter().map(|n| (n.tick, n.length, n.key)).collect()
}

/// The pitch classes of the notes that start at `tick`.
fn classes_at(notes: &[(i64, i64, u8)], tick: i64) -> Vec<u8> {
    let mut pcs: Vec<u8> = notes
        .iter()
        .filter(|n| n.0 == tick)
        .map(|n| n.2 % 12)
        .collect();
    pcs.sort_unstable();
    pcs.dedup();
    pcs
}

#[test]
fn send_to_clip_writes_what_the_stack_plays_over_the_span_in_one_step() {
    let (mut c, wheel) = with_wheel();
    let lanes = c.doc().song.track(&key("a")).unwrap().note_lane_count();
    assert!(c.wheel_to_clip(&wheel), "{:?}", c.last_rejection());
    let t = c.doc().song.track(&key("a")).unwrap();
    assert_eq!(t.note_lane_count(), lanes + 1);
    assert_eq!(t.lanes[lanes].name.as_deref(), Some("Chord Wheel"));
    let clip = &t.lanes[lanes].clips[0];
    assert_eq!(
        (clip.start, clip.length),
        (0, 4 * BAR),
        "the progression once"
    );
    let notes = clip_notes(&c, lanes);
    assert_eq!(classes_at(&notes, 0), [0, 4, 7], "C");
    assert_eq!(classes_at(&notes, BAR), [2, 7, 11], "G");
    assert_eq!(classes_at(&notes, 2 * BAR), [0, 4, 9], "Am");
    assert_eq!(classes_at(&notes, 3 * BAR), [0, 5, 9], "F");
    assert!(
        notes.iter().all(|n| n.0 + n.1 <= 4 * BAR),
        "nothing past the span"
    );
    assert!(c.undo());
    assert_eq!(
        c.doc().song.track(&key("a")).unwrap().note_lane_count(),
        lanes,
        "the lane and the clip are one undo step"
    );
}

#[test]
fn send_to_clip_repeats_the_progression_from_the_left_locator_to_the_loop_end() {
    let (mut c, wheel) = with_wheel();
    // From bar 2 (a phase the song clock does not share with the progression) for 6 bars.
    set_loop(&mut c, (BAR, 7 * BAR), true);
    assert!(c.wheel_to_clip(&wheel), "{:?}", c.last_rejection());
    let lane = c.doc().song.track(&key("a")).unwrap().note_lane_count() - 1;
    let clip = c.doc().song.track(&key("a")).unwrap().lanes[lane].clips[0].clone();
    assert_eq!((clip.start, clip.length), (BAR, 6 * BAR));
    let notes = clip_notes(&c, lane);
    assert_eq!(
        classes_at(&notes, 0),
        [0, 4, 7],
        "slot 1 starts at the left locator"
    );
    assert_eq!(
        classes_at(&notes, 4 * BAR),
        [0, 4, 7],
        "and again after one pass"
    );
    assert_eq!(classes_at(&notes, 5 * BAR), [2, 7, 11]);
    assert!(
        notes.iter().all(|n| n.0 + n.1 <= 6 * BAR),
        "cut at the loop end"
    );
    // The chord lane over the same span starts at the same tick.
    assert!(c.wheel_to_chord_lane(&wheel));
    assert_eq!(c.harmony().chords[0].tick, clip.start);
}

#[test]
fn send_to_clip_needs_notes_and_a_track() {
    let (mut c, wheel) = with_wheel();
    set(&mut c, &wheel, "play.mode", 1.0); // held: no keys, no notes
    let song = c.doc().song.clone();
    assert!(!c.wheel_to_clip(&wheel));
    assert!(c.last_rejection().is_some_and(|m| m.contains("no notes")));
    assert_eq!(c.doc().song, song);
    // A wheel that sits on no track.
    let mut c = rack_only_core();
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: CHORD_WHEEL.to_string(),
        key: Some(key("w")),
    })
    .unwrap();
    assert!(!c.wheel_to_clip(&key("w")));
    assert!(c.last_rejection().is_some_and(|m| m.contains("track")));
}
