//! Chord Wheel tests: styles and timing, slot and param changes while playing, locates, loop
//! wraps, stops, bypass, the play modes, the song key, humanize and note bookkeeping.
#![allow(clippy::float_cmp)]

use super::*;
use atomik_core::progression::{self, MOOD_LABELS, Mood};
use atomik_core::theory::{Scale, ScaleKind};
use atomik_core::{HarmonyNow, LoopWrap};
use atomik_testkit::harness::DeviceHarness;
use proptest::prelude::*;

use crate::test_support::{off, on};

const SR: f64 = 48_000.0;
/// Frames per beat at 120 BPM and 48 kHz.
const BEAT_FRAMES: u64 = 24_000;
const BATCH_U: u64 = BATCH as u64;

#[derive(Debug, Clone, Copy, PartialEq)]
enum K {
    On(u8, u8),
    Off(u8),
    AllOff,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Ev {
    frame: u64,
    k: K,
}

fn rig() -> DeviceHarness {
    DeviceHarness::with_options(Box::new(ChordWheel::new()), SR, 120.0, 7)
}

fn set(h: &mut DeviceHarness, key: &str, v: f32) {
    h.set_param(key, v);
}

fn batch(h: &mut DeviceHarness) -> Vec<Ev> {
    let base = h.frame();
    h.process();
    h.notes_out("notes_out")
        .iter()
        .map(|e| Ev {
            frame: base + u64::from(e.offset),
            k: match e.kind {
                NoteKind::On { key, velocity } => K::On(key, (velocity * 127.0).round() as u8),
                NoteKind::Off { key } => K::Off(key),
                NoteKind::AllOff => K::AllOff,
                other => panic!("unexpected {other:?}"),
            },
        })
        .collect()
}

fn collect(h: &mut DeviceHarness, batches: u64) -> Vec<Ev> {
    let mut out = Vec::new();
    for _ in 0..batches {
        out.extend(batch(h));
    }
    out
}

/// Batches that cover `frames` frames.
fn batches_for(frames: u64) -> u64 {
    frames / BATCH_U + 2
}

fn ons(evs: &[Ev]) -> Vec<(u64, u8)> {
    evs.iter()
        .filter_map(|e| match e.k {
            K::On(k, _) => Some((e.frame, k)),
            _ => None,
        })
        .collect()
}

fn offs(evs: &[Ev]) -> Vec<(u64, u8)> {
    evs.iter()
        .filter_map(|e| match e.k {
            K::Off(k) => Some((e.frame, k)),
            _ => None,
        })
        .collect()
}

fn keys_at(evs: &[Ev], frame: u64) -> Vec<u8> {
    let mut v: Vec<u8> = ons(evs)
        .into_iter()
        .filter(|(f, _)| *f == frame)
        .map(|(_, k)| k)
        .collect();
    v.sort_unstable();
    v
}

/// The frames of the Ons of `key`.
fn frames_of(evs: &[Ev], key: u8) -> Vec<u64> {
    ons(evs)
        .into_iter()
        .filter(|(_, k)| *k == key)
        .map(|(f, _)| f)
        .collect()
}

fn pcs(keys: &[u8]) -> u16 {
    keys.iter().fold(0, |m, k| m | 1 << (k % 12))
}

/// A running model of what sounds: panics on an On of a sounding key or an Off of a silent one.
#[derive(Default)]
struct Tracker {
    sounding: std::collections::BTreeSet<u8>,
    peak: usize,
}

impl Tracker {
    fn feed(&mut self, evs: &[Ev]) {
        for e in evs {
            match e.k {
                K::On(k, _) => {
                    assert!(
                        self.sounding.insert(k),
                        "On of sounding key {k} at {}",
                        e.frame
                    );
                    self.peak = self.peak.max(self.sounding.len());
                }
                K::Off(k) => assert!(
                    self.sounding.remove(&k),
                    "Off of silent key {k} at {}",
                    e.frame
                ),
                K::AllOff => self.sounding.clear(),
            }
        }
    }
}

const C_MAJOR_KEYS: [u8; 3] = [60, 64, 67];

fn mid_voicing(chord: Chord, prev: Option<&Voicing>, key: Key) -> Voicing {
    voice(
        chord,
        prev,
        VoiceOpts {
            key,
            lead: true,
            register: 2,
            spread: Spread::Close,
            extend: Extend::AsWritten,
        },
    )
}

#[test]
fn descriptor_is_valid_and_matches_the_spec_table() {
    assert_eq!(DESCRIPTOR.validate(), Ok(()));
    assert_eq!(DESCRIPTOR.type_id, "atomik.chord-wheel");
    assert_eq!(DESCRIPTOR.name, "Chord Wheel");
    assert_eq!(DESCRIPTOR.params.len(), 61);
    let keys: Vec<&str> = DESCRIPTOR.params.iter().map(|p| p.key).collect();
    assert_eq!(&keys[..4], &["key.root", "key.mode", "key.source", "slots"]);
    for n in 1..=8 {
        for (f, name) in ["root", "quality", "bass", "beats", "lock"]
            .iter()
            .enumerate()
        {
            assert_eq!(keys[4 + 5 * (n - 1) + f], format!("slot.{n}.{name}"));
        }
    }
    assert_eq!(
        &keys[44..],
        &[
            "gen.mood",
            "gen.spice",
            "gen.seed",
            "voice.lead",
            "voice.register",
            "voice.spread",
            "voice.extend",
            "style",
            "gate",
            "strum",
            "velocity",
            "accent",
            "humanize",
            "bass",
            "play.mode",
            "thru",
            "bypass"
        ]
    );
    // The consts the code uses are the indices of the table.
    for (c, key) in [
        (P_KEY_ROOT, "key.root"),
        (P_KEY_MODE, "key.mode"),
        (P_KEY_SOURCE, "key.source"),
        (P_SLOTS, "slots"),
        (P_MOOD, "gen.mood"),
        (P_SPICE, "gen.spice"),
        (P_SEED, "gen.seed"),
        (P_LEAD, "voice.lead"),
        (P_REGISTER, "voice.register"),
        (P_SPREAD, "voice.spread"),
        (P_EXTEND, "voice.extend"),
        (P_STYLE, "style"),
        (P_GATE, "gate"),
        (P_STRUM, "strum"),
        (P_VELOCITY, "velocity"),
        (P_ACCENT, "accent"),
        (P_HUMANIZE, "humanize"),
        (P_BASS, "bass"),
        (P_PLAY, "play.mode"),
        (P_THRU, "thru"),
        (P_BYPASS, "bypass"),
    ] {
        assert_eq!(DESCRIPTOR.params[c as usize].key, key);
    }
    let get = |k: &str| *DESCRIPTOR.param_by_key(k).unwrap();
    assert_eq!(get("slots").default, 4.0);
    assert_eq!(get("slot.3.quality").default, 1.0);
    assert_eq!(get("slot.2.root").default, 7.0);
    assert_eq!(get("slot.1.beats").default, 3.0);
    assert_eq!(get("gen.spice").default, 0.3);
    assert_eq!(get("gate").default, 0.9);
    assert_eq!(get("velocity").default, 0.8);
    assert_eq!(get("accent").default, 0.3);
    assert_eq!(get("bass").default, 1.0);
    assert_eq!(get("voice.register").default, 2.0);
    assert_eq!(get("voice.lead").default, 1.0);
    assert_eq!(get("gen.seed").max, 9999.0);
    assert_eq!(get("gate").min, 0.05);
    assert_eq!(DESCRIPTOR.params.last().unwrap().key, "bypass");
    assert_eq!(DESCRIPTOR.sockets.len(), 2);
    assert_eq!(DESCRIPTOR.sockets[0].key, "notes_in");
    assert_eq!(DESCRIPTOR.sockets[1].key, "notes_out");
}

#[test]
fn the_labels_match_the_core() {
    assert_eq!(QUALITY_LABELS.len(), ChordKind::ALL.len());
    for (label, kind) in QUALITY_LABELS.iter().zip(ChordKind::ALL) {
        assert_eq!(*label, kind.label());
    }
    let Unit::Choice(moods) = DESCRIPTOR.param_by_key("gen.mood").unwrap().unit else {
        panic!("choice")
    };
    assert_eq!(moods, MOOD_LABELS);
    assert_eq!(moods.len(), Mood::ALL.len());
    assert_eq!(BEATS_LABELS.len(), BEATS.len());
    for (label, beats) in BEATS_LABELS.iter().zip(BEATS) {
        assert_eq!(*label, beats.to_string());
    }
    assert_eq!(BASS_LABELS[0], "Root");
    for pc in 0..12 {
        assert_eq!(BASS_LABELS[pc + 1], PITCH_NAMES[pc]);
    }
}

#[test]
fn it_is_a_utility_note_player_in_the_player_list() {
    assert_eq!(DESCRIPTOR.kind, DeviceKind::Utility);
    assert!(crate::is_player("atomik.chord-wheel"));
}

#[test]
fn the_default_progression_sustains_c_g_am_f_with_a_bass() {
    let mut h = rig();
    let evs = collect(&mut h, batches_for(16 * BEAT_FRAMES));
    // Slot 1: C close around middle C, the root two registers down.
    assert_eq!(keys_at(&evs, 0), vec![36, 60, 64, 67]);
    let key = Key::new(0, KeyMode::Major);
    let c = mid_voicing(Chord::new(0, ChordKind::Major), None, key);
    assert_eq!(c.as_slice(), &C_MAJOR_KEYS);
    // Slot 2 (G) starts after four beats and leads from the C voicing.
    let g = mid_voicing(Chord::new(7, ChordKind::Major), Some(&c), key);
    let mut want: Vec<u8> = g.as_slice().to_vec();
    want.push(31);
    want.sort_unstable();
    assert_eq!(keys_at(&evs, 4 * BEAT_FRAMES), want);
    // The chord ends at the gate, the bass at the end of the slot.
    let chord_off: Vec<u64> = offs(&evs)
        .into_iter()
        .filter(|(f, k)| *k == 60 && *f < 4 * BEAT_FRAMES)
        .map(|(f, _)| f)
        .collect();
    assert_eq!(chord_off, vec![(3.6 * BEAT_FRAMES as f64) as u64]);
    let bass_off: Vec<u64> = offs(&evs)
        .into_iter()
        .filter(|(_, k)| *k == 36)
        .map(|(f, _)| f)
        .collect();
    assert_eq!(bass_off, vec![4 * BEAT_FRAMES]);
    // Slot 3 is Am (bass A) and slot 4 F.
    let am = mid_voicing(Chord::new(9, ChordKind::Minor), Some(&g), key);
    let got = keys_at(&evs, 8 * BEAT_FRAMES);
    assert_eq!(pcs(&got), pcs(am.as_slice()) | 1 << 9);
    assert!(got.contains(&33), "{got:?}");
    let f = mid_voicing(Chord::new(5, ChordKind::Major), Some(&am), key);
    let got = keys_at(&evs, 12 * BEAT_FRAMES);
    assert_eq!(pcs(&got), pcs(f.as_slice()));
    // The loop wraps to C after 16 beats.
    assert_eq!(keys_at(&evs, 16 * BEAT_FRAMES), vec![36, 60, 64, 67]);
}

#[test]
fn velocity_accent_and_gate_shape_the_hits() {
    let mut h = rig();
    set(&mut h, "style", 2.0);
    set(&mut h, "velocity", 0.9);
    set(&mut h, "accent", 0.5);
    set(&mut h, "gate", 0.5);
    let evs = collect(&mut h, batches_for(BEAT_FRAMES));
    let hits: Vec<(u64, u8)> = evs
        .iter()
        .filter_map(|e| match e.k {
            K::On(60, v) => Some((e.frame, v)),
            _ => None,
        })
        .collect();
    assert_eq!(hits[0], (0, (0.9f32 * 127.0).round() as u8));
    // The second hit is softer by 45 percent of the accent: 0.9 * (1 - 0.225).
    assert_eq!(hits[1].0, BEAT_FRAMES / 2);
    assert_eq!(hits[1].1, (0.9f32 * (1.0 - 0.225) * 127.0).round() as u8);
    // Half gate: an eighth is 12000 frames, the note lasts 6000.
    let off60 = offs(&evs).into_iter().find(|(_, k)| *k == 60).unwrap();
    assert_eq!(off60.0, 6000);
}

fn style_hits(style: f32, key: u8, beats: u64) -> Vec<u64> {
    let mut h = rig();
    set(&mut h, "style", style);
    set(&mut h, "bass", 0.0);
    let evs = collect(&mut h, batches_for(beats * BEAT_FRAMES));
    frames_of(&evs, key)
        .into_iter()
        .filter(|f| *f < beats * BEAT_FRAMES)
        .collect()
}

#[test]
fn pulse_quarter_hits_every_beat() {
    let hits = style_hits(1.0, 60, 4);
    assert_eq!(hits, vec![0, BEAT_FRAMES, 2 * BEAT_FRAMES, 3 * BEAT_FRAMES]);
}

#[test]
fn pulse_eighth_hits_every_half_beat() {
    let hits = style_hits(2.0, 60, 2);
    assert_eq!(
        hits,
        vec![0, BEAT_FRAMES / 2, BEAT_FRAMES, 3 * BEAT_FRAMES / 2]
    );
}

#[test]
fn offbeat_stabs_the_off_eighths() {
    let hits = style_hits(3.0, 60, 4);
    let half = BEAT_FRAMES / 2;
    assert_eq!(
        hits,
        vec![
            half,
            half + BEAT_FRAMES,
            half + 2 * BEAT_FRAMES,
            half + 3 * BEAT_FRAMES
        ]
    );
}

#[test]
fn syncopated_plays_three_three_two() {
    let hits = style_hits(7.0, 60, 4);
    assert_eq!(hits, vec![0, 3 * BEAT_FRAMES / 2, 3 * BEAT_FRAMES]);
    // The last hit of the pattern is cut at the slot end: two eighths, one beat.
    let mut h = rig();
    set(&mut h, "style", 7.0);
    set(&mut h, "bass", 0.0);
    set(&mut h, "gate", 1.0);
    let evs = collect(&mut h, batches_for(5 * BEAT_FRAMES));
    let last_off = offs(&evs)
        .into_iter()
        .filter(|(_, k)| *k == 60)
        .map(|(f, _)| f)
        .nth(2)
        .unwrap();
    assert_eq!(last_off, 4 * BEAT_FRAMES);
}

#[test]
fn broken_plays_low_high_mid_high_in_eighths() {
    let mut h = rig();
    set(&mut h, "style", 6.0);
    set(&mut h, "bass", 0.0);
    let evs = collect(&mut h, batches_for(2 * BEAT_FRAMES));
    let seq: Vec<(u64, u8)> = ons(&evs)
        .into_iter()
        .filter(|(f, _)| *f < 2 * BEAT_FRAMES)
        .collect();
    let q = BEAT_FRAMES / 2;
    assert_eq!(seq, vec![(0, 60), (q, 67), (2 * q, 64), (3 * q, 67)]);
}

#[test]
fn push_anticipates_the_change_by_an_eighth() {
    let mut h = rig();
    set(&mut h, "style", 4.0);
    set(&mut h, "bass", 0.0);
    set(&mut h, "gate", 1.0);
    let evs = collect(&mut h, batches_for(8 * BEAT_FRAMES));
    // The song starts inside the first chord, which sounds at once.
    assert_eq!(keys_at(&evs, 0).len(), 3);
    // The second chord starts an eighth before the bar line, and the first ends there.
    let at = 4 * BEAT_FRAMES - BEAT_FRAMES / 2;
    assert_eq!(keys_at(&evs, at).len(), 3);
    assert!(offs(&evs).iter().any(|(f, k)| *f == at && *k == 60));
    // And the one after another eighth before the next bar.
    assert_eq!(keys_at(&evs, 8 * BEAT_FRAMES - BEAT_FRAMES / 2).len(), 3);
}

#[test]
fn strum_rolls_the_chord_over_at_least_25_ms() {
    let mut h = rig();
    set(&mut h, "style", 5.0);
    set(&mut h, "bass", 0.0);
    let evs = collect(&mut h, batches_for(BEAT_FRAMES));
    // 25 ms is 1200 frames across the three notes, low to high.
    assert_eq!(ons(&evs), vec![(0, 60), (600, 64), (1200, 67)]);
    // The strum knob widens it: full is 60 ms = 2880 frames.
    let mut h = rig();
    set(&mut h, "style", 5.0);
    set(&mut h, "bass", 0.0);
    set(&mut h, "strum", 1.0);
    let evs = collect(&mut h, batches_for(BEAT_FRAMES));
    assert_eq!(ons(&evs), vec![(0, 60), (1440, 64), (2880, 67)]);
    // Sustain style with the strum knob rolls too, at the knob.
    let mut h = rig();
    set(&mut h, "bass", 0.0);
    set(&mut h, "strum", 0.5);
    let evs = collect(&mut h, batches_for(BEAT_FRAMES));
    assert_eq!(ons(&evs), vec![(0, 60), (720, 64), (1440, 67)]);
}

#[test]
fn the_bass_follows_its_mode() {
    // Root: one note for the chord.
    let mut h = rig();
    let evs = collect(&mut h, batches_for(4 * BEAT_FRAMES));
    assert_eq!(frames_of(&evs, 36), vec![0]);
    // Pulse: the bass plays on every hit.
    let mut h = rig();
    set(&mut h, "bass", 2.0);
    set(&mut h, "style", 1.0);
    let evs = collect(&mut h, batches_for(4 * BEAT_FRAMES));
    assert_eq!(
        frames_of(&evs, 36)
            .into_iter()
            .filter(|f| *f < 4 * BEAT_FRAMES)
            .collect::<Vec<_>>(),
        vec![0, BEAT_FRAMES, 2 * BEAT_FRAMES, 3 * BEAT_FRAMES]
    );
    // Off: nothing below the chord.
    let mut h = rig();
    set(&mut h, "bass", 0.0);
    let evs = collect(&mut h, batches_for(BEAT_FRAMES));
    assert_eq!(keys_at(&evs, 0), vec![60, 64, 67]);
    // The register moves it: high puts the root two octaves below C5.
    let mut h = rig();
    set(&mut h, "voice.register", 3.0);
    let evs = collect(&mut h, batches_for(BEAT_FRAMES));
    assert!(keys_at(&evs, 0).contains(&48));
}

#[test]
fn a_slash_bass_sounds_in_the_bass_and_under_the_chord_when_the_bass_is_off() {
    // C/E: slot 1 over E.
    let mut h = rig();
    set(&mut h, "slot.1.bass", 5.0);
    let evs = collect(&mut h, batches_for(BEAT_FRAMES));
    let bass: Vec<u8> = keys_at(&evs, 0).into_iter().filter(|k| *k < 55).collect();
    assert_eq!(bass.len(), 1);
    assert_eq!(bass[0] % 12, 4);
    let mut h = rig();
    set(&mut h, "slot.1.bass", 5.0);
    set(&mut h, "bass", 0.0);
    let evs = collect(&mut h, batches_for(BEAT_FRAMES));
    let keys = keys_at(&evs, 0);
    // The E sits an octave or less below the chord, which stays C major.
    assert_eq!(keys.len(), 4);
    assert_eq!(keys[0] % 12, 4);
    assert!(60 - keys[0] <= 12);
}

#[test]
fn spread_register_and_extend_change_the_voicing() {
    let mut h = rig();
    set(&mut h, "bass", 0.0);
    set(&mut h, "voice.spread", 2.0);
    let evs = collect(&mut h, batches_for(BEAT_FRAMES));
    let wide = keys_at(&evs, 0);
    assert_eq!(wide.len(), 4);
    assert_eq!(wide[1] - wide[0], 12);
    let mut h = rig();
    set(&mut h, "bass", 0.0);
    set(&mut h, "voice.extend", 1.0);
    let evs = collect(&mut h, batches_for(BEAT_FRAMES));
    assert_eq!(pcs(&keys_at(&evs, 0)), 0b1000_1001_0001);
    let mut h = rig();
    set(&mut h, "bass", 0.0);
    set(&mut h, "voice.register", 0.0);
    let evs = collect(&mut h, batches_for(BEAT_FRAMES));
    assert_eq!(keys_at(&evs, 0), vec![36, 40, 43]);
}

#[test]
fn voice_leading_off_voices_every_chord_in_root_position() {
    let mut h = rig();
    set(&mut h, "bass", 0.0);
    set(&mut h, "voice.lead", 0.0);
    let evs = collect(&mut h, batches_for(8 * BEAT_FRAMES));
    let g = keys_at(&evs, 4 * BEAT_FRAMES);
    assert_eq!(g, vec![55, 59, 62]);
}

#[test]
fn slot_lengths_and_count_set_the_cycle() {
    let mut h = rig();
    set(&mut h, "bass", 0.0);
    set(&mut h, "slots", 2.0);
    // Slot 1 is 2 beats, slot 2 is 8.
    set(&mut h, "slot.1.beats", 1.0);
    set(&mut h, "slot.2.beats", 5.0);
    let evs = collect(&mut h, batches_for(24 * BEAT_FRAMES));
    // The cycle is 10 beats: C at 0, 10 and 20 beats, G in between.
    for at in [0, 10, 20] {
        assert_eq!(
            pcs(&keys_at(&evs, at * BEAT_FRAMES)),
            pcs(&[0, 4, 7]),
            "{at}"
        );
    }
    for at in [2, 12, 22] {
        assert_eq!(
            pcs(&keys_at(&evs, at * BEAT_FRAMES)),
            pcs(&[7, 11, 2]),
            "{at}"
        );
    }
}

#[test]
fn changing_the_playing_slot_re_voices_at_once_and_other_slots_leave_it_alone() {
    let mut h = rig();
    set(&mut h, "bass", 0.0);
    let mut t = Tracker::default();
    t.feed(&collect(&mut h, 100));
    // A change of a slot that is not playing is silent.
    set(&mut h, "slot.3.root", 2.0);
    let evs = collect(&mut h, 4);
    assert!(evs.is_empty(), "{evs:?}");
    // A change of the playing slot closes the chord and starts the new one on the next batch.
    set(&mut h, "slot.1.quality", 1.0);
    let evs = batch(&mut h);
    t.feed(&evs);
    let now = h.frame() - BATCH_U;
    assert_eq!(offs(&evs).len(), 3);
    assert_eq!(ons(&evs).len(), 3);
    assert!(evs.iter().all(|e| e.frame == now));
    let c_minor = pcs(&ons(&evs).iter().map(|(_, k)| *k).collect::<Vec<_>>());
    assert_eq!(c_minor, pcs(&[0, 3, 7]));
    // The length of an earlier slot moves the cycle: the playing chord is restarted.
    set(&mut h, "slot.1.beats", 5.0);
    let evs = batch(&mut h);
    t.feed(&evs);
    assert!(!evs.is_empty());
}

#[test]
fn a_locate_stop_and_start_close_every_note() {
    let mut h = rig();
    let mut t = Tracker::default();
    // Locate into slot 3 mid-slot: the chord of that slot starts at once, nothing hangs.
    t.feed(&collect(&mut h, 50));
    h.time_mut().tick = (9 * PPQ) as f64;
    let evs = batch(&mut h);
    t.feed(&evs);
    let now = h.frame() - BATCH_U;
    assert!(offs(&evs).iter().all(|(f, _)| *f == now));
    let on_keys: Vec<u8> = ons(&evs).iter().map(|(_, k)| *k).collect();
    assert_eq!(
        pcs(&on_keys) & (1 << 9),
        1 << 9,
        "A sounds in Am: {on_keys:?}"
    );
    assert!(ons(&evs).iter().all(|(f, _)| *f == now));
    // Stop: every note is closed on the first stopped batch and nothing follows.
    h.time_mut().playing = false;
    let evs = batch(&mut h);
    t.feed(&evs);
    assert!(t.sounding.is_empty());
    assert!(collect(&mut h, 10).is_empty());
    // Start again: from the playhead.
    h.time_mut().playing = true;
    let evs = batch(&mut h);
    t.feed(&evs);
    assert!(!ons(&evs).is_empty());
}

#[test]
fn a_loop_wrap_closes_the_notes_and_plays_from_the_loop_start() {
    let mut h = rig();
    let mut t = Tracker::default();
    // Play to the middle of slot 2, then wrap to tick 0 at frame 20.
    let to = 6 * BEAT_FRAMES / BATCH_U;
    t.feed(&collect(&mut h, to));
    h.time_mut().loop_wrap = Some(LoopWrap {
        offset: 20,
        to_tick: 0.0,
    });
    let evs = batch(&mut h);
    t.feed(&evs);
    let base = h.frame() - BATCH_U;
    // The old notes end at the wrap frame and C starts there.
    assert!(offs(&evs).iter().all(|(f, _)| *f == base + 20));
    assert_eq!(keys_at(&evs, base + 20), vec![36, 60, 64, 67]);
    // A wrap into the middle of a slot restarts the chord there too.
    let mut h = rig();
    let mut t = Tracker::default();
    t.feed(&collect(&mut h, 30));
    h.time_mut().loop_wrap = Some(LoopWrap {
        offset: 10,
        to_tick: (PPQ / 2) as f64,
    });
    let evs = batch(&mut h);
    t.feed(&evs);
    assert_eq!(ons(&evs).len(), 4);
    assert!(
        ons(&evs)
            .iter()
            .all(|(f, _)| *f == h.frame() - BATCH_U + 10)
    );
}

#[test]
fn the_cycle_follows_the_tempo() {
    // Twice the tempo: the same cycle in half the frames.
    let mut h = DeviceHarness::with_options(Box::new(ChordWheel::new()), SR, 240.0, 0);
    set(&mut h, "bass", 0.0);
    let evs = collect(&mut h, batches_for(4 * BEAT_FRAMES));
    let starts: Vec<u64> = ons(&evs)
        .iter()
        .map(|(f, _)| *f)
        .filter(|f| *f > 0)
        .collect();
    assert_eq!(starts[0], 4 * BEAT_FRAMES / 2);
}

#[test]
fn bypass_closes_what_it_sent_and_passes_the_input_on() {
    let mut h = rig();
    let mut t = Tracker::default();
    t.feed(&collect(&mut h, 40));
    assert!(!t.sounding.is_empty());
    set(&mut h, "bypass", 1.0);
    h.push_note("notes_in", on(5, 70, 0.5));
    let evs = batch(&mut h);
    t.feed(&evs);
    // Everything that sounded is closed at frame 0 and the input key goes through.
    assert_eq!(t.sounding.iter().copied().collect::<Vec<_>>(), vec![70]);
    h.push_note("notes_in", off(3, 70));
    let evs = batch(&mut h);
    t.feed(&evs);
    assert!(t.sounding.is_empty());
    // Nothing is generated while bypassed.
    assert!(collect(&mut h, 200).is_empty());
    // And it starts again from the transport when the switch goes off.
    set(&mut h, "bypass", 0.0);
    let evs = collect(&mut h, 2);
    t.feed(&evs);
    assert!(!ons(&evs).is_empty());
}

#[test]
fn thru_passes_the_input_in_transport_mode_only() {
    let mut h = rig();
    set(&mut h, "thru", 1.0);
    h.push_note("notes_in", on(10, 72, 0.5));
    let evs = batch(&mut h);
    assert!(ons(&evs).contains(&(10, 72)));
    h.push_note("notes_in", off(4, 72));
    let evs = batch(&mut h);
    assert!(offs(&evs).contains(&(h.frame() - BATCH_U + 4, 72)));
    // Off: the input is swallowed.
    let mut h = rig();
    h.push_note("notes_in", on(10, 72, 0.5));
    let evs = batch(&mut h);
    assert!(!ons(&evs).iter().any(|(_, k)| *k == 72));
    // Switching it off while a key is held closes the key.
    let mut h = rig();
    set(&mut h, "thru", 1.0);
    h.push_note("notes_in", on(10, 72, 0.5));
    let mut t = Tracker::default();
    t.feed(&batch(&mut h));
    assert!(t.sounding.contains(&72));
    set(&mut h, "thru", 0.0);
    t.feed(&batch(&mut h));
    assert!(!t.sounding.contains(&72));
}

#[test]
fn thru_never_starts_a_key_that_is_still_sounding() {
    // A second On of a key the input has not released is a retrigger: the key is closed first.
    let mut h = rig();
    set(&mut h, "thru", 1.0);
    h.push_note("notes_in", on(0, 100, 0.7));
    h.push_note("notes_in", on(0, 100, 0.7));
    let mut t = Tracker::default();
    let evs = batch(&mut h);
    t.feed(&evs);
    assert_eq!(ons(&evs).iter().filter(|(_, k)| *k == 100).count(), 2);
    assert!(offs(&evs).contains(&(0, 100)), "the first start is closed");
    // The release closes it once.
    h.push_note("notes_in", off(0, 100));
    t.feed(&batch(&mut h));
    assert!(!t.sounding.contains(&100));
}

#[test]
fn a_passed_key_the_chord_also_plays_is_closed_before_either_starts_it_again() {
    // Pass the bass key (36) through while the wheel plays its bass, and the other way round: the key held
    // on the input when the chord reaches it. Neither order starts a sounding key.
    for pressed_first in [false, true] {
        let mut h = rig();
        set(&mut h, "thru", 1.0);
        let mut t = Tracker::default();
        if pressed_first {
            h.push_note("notes_in", on(0, 36, 0.7));
            t.feed(&batch(&mut h));
        }
        t.feed(&collect(&mut h, 20));
        if !pressed_first {
            assert!(t.sounding.contains(&36), "the chord sounds the key");
            h.push_note("notes_in", on(5, 36, 0.7));
            t.feed(&batch(&mut h));
        }
        t.feed(&collect(&mut h, batches_for(16 * BEAT_FRAMES)));
        h.push_note("notes_in", off(0, 36));
        h.time_mut().playing = false;
        for _ in 0..4 {
            t.feed(&batch(&mut h));
        }
        assert!(t.sounding.is_empty(), "{:?}", t.sounding);
    }
}

#[test]
fn a_bypass_switch_closes_a_key_passed_through_by_thru() {
    // The switch starts the device from nothing: a key that thru passed on is closed with the
    // chord notes, or it would hang until its input Off, which the bypass swallows.
    let mut h = rig();
    set(&mut h, "thru", 1.0);
    h.push_note("notes_in", on(0, 100, 0.7));
    let mut t = Tracker::default();
    t.feed(&batch(&mut h));
    assert!(t.sounding.contains(&100));
    set(&mut h, "bypass", 1.0);
    t.feed(&batch(&mut h));
    assert!(!t.sounding.contains(&100), "closed at the switch");
    h.push_note("notes_in", off(3, 100));
    set(&mut h, "bypass", 0.0);
    t.feed(&collect(&mut h, 3));
    h.time_mut().playing = false;
    for _ in 0..4 {
        t.feed(&batch(&mut h));
    }
    assert!(t.sounding.is_empty(), "{:?}", t.sounding);
}
#[test]
fn held_mode_sounds_only_while_a_key_is_down_and_swallows_the_keys() {
    let mut h = rig();
    set(&mut h, "play.mode", 1.0);
    set(&mut h, "bass", 0.0);
    assert!(collect(&mut h, 100).is_empty());
    let mut t = Tracker::default();
    // Press mid-slot: the chord starts at once from the key (a held chord).
    h.push_note("notes_in", on(30, 40, 0.5));
    let evs = batch(&mut h);
    t.feed(&evs);
    let now = h.frame() - BATCH_U;
    assert_eq!(ons(&evs).len(), 3);
    assert!(ons(&evs).iter().all(|(f, k)| *f == now + 30 && *k != 40));
    t.feed(&collect(&mut h, 20));
    // Release: everything closes on that frame.
    h.push_note("notes_in", off(7, 40));
    let evs = batch(&mut h);
    t.feed(&evs);
    assert!(t.sounding.is_empty());
    assert_eq!(offs(&evs).len(), 3);
    assert!(collect(&mut h, 20).is_empty());
    // Two keys: only the last release stops it.
    h.push_note("notes_in", on(0, 40, 0.5));
    h.push_note("notes_in", on(0, 43, 0.5));
    t.feed(&batch(&mut h));
    assert!(!t.sounding.is_empty());
    h.push_note("notes_in", off(0, 40));
    t.feed(&batch(&mut h));
    assert!(!t.sounding.is_empty());
    h.push_note("notes_in", off(0, 43));
    t.feed(&batch(&mut h));
    assert!(t.sounding.is_empty());
}

#[test]
fn step_mode_plays_the_next_chord_for_as_long_as_the_key_is_held() {
    let mut h = rig();
    set(&mut h, "play.mode", 2.0);
    h.time_mut().playing = false;
    let mut t = Tracker::default();
    assert!(collect(&mut h, 50).is_empty());
    let key = Key::new(0, KeyMode::Major);
    let c = mid_voicing(Chord::new(0, ChordKind::Major), None, key);
    h.push_note("notes_in", on(12, 50, 1.0));
    let evs = batch(&mut h);
    t.feed(&evs);
    let now = h.frame() - BATCH_U;
    let mut want = c.as_slice().to_vec();
    want.push(36);
    want.sort_unstable();
    assert_eq!(keys_at(&evs, now + 12), want);
    // At the velocity of the key.
    assert!(evs.iter().all(|e| !matches!(e.k, K::On(_, v) if v != 127)));
    // It sounds until the key goes up.
    assert!(collect(&mut h, 3000).is_empty());
    h.push_note("notes_in", off(5, 50));
    let evs = batch(&mut h);
    t.feed(&evs);
    assert!(t.sounding.is_empty());
    assert_eq!(offs(&evs).len(), 4);
    // The next press plays the next slot: G.
    h.push_note("notes_in", on(0, 51, 0.5));
    let evs = batch(&mut h);
    t.feed(&evs);
    let notes: Vec<u8> = ons(&evs).iter().map(|(_, k)| *k).collect();
    assert_eq!(pcs(&notes) & (1 << 7), 1 << 7);
    assert_eq!(pcs(&notes) & (1 << 11), 1 << 11);
    // Four presses come round to C again.
    h.push_note("notes_in", off(0, 51));
    t.feed(&batch(&mut h));
    for k in [52u8, 53] {
        h.push_note("notes_in", on(0, k, 0.5));
        t.feed(&batch(&mut h));
        h.push_note("notes_in", off(0, k));
        t.feed(&batch(&mut h));
    }
    h.push_note("notes_in", on(0, 54, 0.5));
    let evs = batch(&mut h);
    t.feed(&evs);
    assert!(ons(&evs).iter().any(|(_, k)| *k == 60));
}

#[test]
fn step_mode_strum_cancels_what_has_not_started_when_the_key_goes_up() {
    let mut h = rig();
    set(&mut h, "play.mode", 2.0);
    set(&mut h, "bass", 0.0);
    set(&mut h, "strum", 1.0);
    h.time_mut().playing = false;
    let mut t = Tracker::default();
    h.push_note("notes_in", on(0, 50, 1.0));
    h.push_note("notes_in", off(40, 50));
    t.feed(&batch(&mut h));
    // The key went up at frame 40: the first note started, the others never do.
    assert!(t.sounding.is_empty());
    assert!(t.peak >= 1);
    t.feed(&collect(&mut h, 100));
    assert!(t.sounding.is_empty());
}

#[test]
fn step_mode_never_sounds_more_than_sixteen_notes() {
    let mut h = rig();
    set(&mut h, "play.mode", 2.0);
    set(&mut h, "voice.spread", 2.0);
    set(&mut h, "slot.1.quality", 18.0);
    set(&mut h, "slot.2.quality", 18.0);
    h.time_mut().playing = false;
    let mut t = Tracker::default();
    for k in [50u8, 51, 52] {
        h.push_note("notes_in", on(0, k, 0.9));
        t.feed(&batch(&mut h));
    }
    assert!(t.peak <= MAX_SOUNDING, "{}", t.peak);
    for k in [50u8, 51, 52] {
        h.push_note("notes_in", off(0, k));
        t.feed(&batch(&mut h));
    }
    assert!(t.sounding.is_empty());
}

#[test]
fn following_the_song_key_moves_the_progression() {
    let mut h = rig();
    set(&mut h, "bass", 0.0);
    set(&mut h, "key.source", 1.0);
    // The song is in D major: the C progression sits two semitones up.
    let d = Scale::new(2, ScaleKind::Major);
    h.time_mut().harmony = HarmonyNow::single(0, Some(d), None);
    let evs = collect(&mut h, batches_for(8 * BEAT_FRAMES));
    assert_eq!(pcs(&keys_at(&evs, 0)), pcs(&[2, 6, 9]));
    let second = keys_at(&evs, 4 * BEAT_FRAMES);
    assert_eq!(pcs(&second), pcs(&[9, 1, 4]));
    // Without a song key it plays as written.
    let mut h = rig();
    set(&mut h, "bass", 0.0);
    set(&mut h, "key.source", 1.0);
    let evs = collect(&mut h, 4);
    assert_eq!(keys_at(&evs, 0), vec![60, 64, 67]);
    // The device key source does not look at the song at all.
    let mut h = rig();
    set(&mut h, "bass", 0.0);
    h.time_mut().harmony = HarmonyNow::single(0, Some(d), None);
    let evs = collect(&mut h, 4);
    assert_eq!(keys_at(&evs, 0), vec![60, 64, 67]);
}

#[test]
fn a_song_key_of_the_other_mode_maps_to_the_relative_key() {
    let mut h = rig();
    set(&mut h, "bass", 0.0);
    set(&mut h, "key.source", 1.0);
    // A minor song key for a C major progression: C major is its relative, no shift.
    let am = Scale::new(9, ScaleKind::NaturalMinor);
    h.time_mut().harmony = HarmonyNow::single(0, Some(am), None);
    let evs = collect(&mut h, 4);
    assert_eq!(keys_at(&evs, 0), vec![60, 64, 67]);
    // E minor: its relative major is G, seven up.
    let mut h = rig();
    set(&mut h, "bass", 0.0);
    set(&mut h, "key.source", 1.0);
    let em = Scale::new(4, ScaleKind::NaturalMinor);
    h.time_mut().harmony = HarmonyNow::single(0, Some(em), None);
    let evs = collect(&mut h, 4);
    assert_eq!(pcs(&keys_at(&evs, 0)), pcs(&[7, 11, 2]));
}

#[test]
fn a_change_of_the_song_key_re_voices_the_playing_chord() {
    let mut h = rig();
    set(&mut h, "bass", 0.0);
    set(&mut h, "key.source", 1.0);
    let c = Scale::new(0, ScaleKind::Major);
    h.time_mut().harmony = HarmonyNow::single(0, Some(c), None);
    let mut t = Tracker::default();
    t.feed(&collect(&mut h, 20));
    let d = Scale::new(2, ScaleKind::Major);
    h.time_mut().harmony = HarmonyNow::single(0, Some(d), None);
    let evs = batch(&mut h);
    t.feed(&evs);
    let sounding: Vec<u8> = t.sounding.iter().copied().collect();
    assert_eq!(pcs(&sounding), pcs(&[2, 6, 9]));
}

#[test]
fn humanize_is_deterministic_bounded_and_off_at_zero() {
    let run = |human: f32, seed: u64| {
        let mut h = DeviceHarness::with_options(Box::new(ChordWheel::new()), SR, 120.0, seed);
        set(&mut h, "style", 2.0);
        set(&mut h, "bass", 0.0);
        set(&mut h, "humanize", human);
        collect(&mut h, batches_for(8 * BEAT_FRAMES))
    };
    let flat = run(0.0, 1);
    assert_eq!(run(0.0, 2), flat, "no humanize, no seed dependence");
    let a = run(1.0, 1);
    assert_eq!(run(1.0, 1), a, "same seed, same notes");
    assert_ne!(run(1.0, 2), a, "another seed, other notes");
    assert_ne!(a, flat);
    // Timing within 10 ms = 480 frames of the grid, velocity within 12 percent.
    let eighth = BEAT_FRAMES / 2;
    for (f, k) in ons(&a) {
        let nearest = (f + eighth / 2) / eighth * eighth;
        let d = f.abs_diff(nearest);
        assert!(d <= 481, "{f} {k}: {d}");
    }
    let vels: Vec<u8> = a
        .iter()
        .filter_map(|e| match e.k {
            K::On(_, v) => Some(v),
            _ => None,
        })
        .collect();
    let base = (0.8f32 * 127.0).round();
    let soft = (0.8f32 * (1.0 - 0.45 * 0.3) * 127.0).round();
    let within = |v: f32, of: f32| v >= of * 0.87 - 1.0 && v <= of * 1.13 + 1.0;
    assert!(
        vels.iter()
            .all(|v| within(f32::from(*v), base) || within(f32::from(*v), soft))
    );
    assert!(
        vels.iter()
            .any(|v| f32::from(*v) != base && f32::from(*v) != soft)
    );
}

#[test]
fn humanize_never_leaves_a_note_hanging_after_a_locate() {
    let mut h = rig();
    set(&mut h, "style", 2.0);
    set(&mut h, "humanize", 1.0);
    let mut t = Tracker::default();
    t.feed(&collect(&mut h, 30));
    h.time_mut().tick = (PPQ / 2) as f64 * 5.0;
    t.feed(&batch(&mut h));
    t.feed(&collect(&mut h, 2000));
    h.time_mut().playing = false;
    t.feed(&collect(&mut h, 2));
    assert!(t.sounding.is_empty());
}

#[test]
fn the_meter_reads_the_playing_slot_and_the_sounding_keys() {
    let mut h = rig();
    let mut m = [0.0f32; 9];
    h.process();
    assert_eq!(h.device().meter_values(&mut m), 9);
    assert_eq!(m[0], 1.0);
    assert_eq!(&m[1..5], &[36.0, 60.0, 64.0, 67.0]);
    assert_eq!(m[5], 0.0);
    // Slot 2 after four beats.
    for _ in 0..batches_for(4 * BEAT_FRAMES) {
        h.process();
    }
    h.device().meter_values(&mut m);
    assert_eq!(m[0], 2.0);
    // Stopped: no slot, no keys.
    h.time_mut().playing = false;
    h.process();
    h.device().meter_values(&mut m);
    assert!(m.iter().all(|v| *v == 0.0));
    assert!(h.device().has_meter_values());
}

#[test]
fn locks_and_generator_params_do_not_touch_the_notes() {
    let mut h = rig();
    set(&mut h, "bass", 0.0);
    let mut t = Tracker::default();
    t.feed(&collect(&mut h, 10));
    for (k, v) in [
        ("slot.1.lock", 1.0),
        ("slot.3.lock", 1.0),
        ("gen.mood", 5.0),
        ("gen.spice", 0.9),
        ("gen.seed", 1234.0),
        ("gate", 0.5),
        ("velocity", 0.3),
        ("humanize", 0.4),
    ] {
        set(&mut h, k, v);
        assert!(batch(&mut h).is_empty(), "{k}");
    }
}

#[test]
fn reset_flushes_with_an_all_off() {
    let mut h = rig();
    collect(&mut h, 10);
    h.device_mut().reset();
    let evs = batch(&mut h);
    assert_eq!(evs[0].k, K::AllOff);
    // Silent resets say nothing.
    let mut h = rig();
    h.device_mut().reset();
    assert!(batch(&mut h).iter().all(|e| matches!(e.k, K::On(..))));
}

#[test]
fn a_retriggered_key_is_closed_before_it_starts_again() {
    // Pulse with a full gate retriggers the same keys at every hit.
    let mut h = rig();
    set(&mut h, "style", 2.0);
    set(&mut h, "gate", 1.0);
    set(&mut h, "voice.spread", 2.0);
    set(&mut h, "slot.1.quality", 18.0);
    set(&mut h, "slot.2.quality", 18.0);
    set(&mut h, "bass", 2.0);
    let mut t = Tracker::default();
    t.feed(&collect(&mut h, batches_for(8 * BEAT_FRAMES)));
    assert!(t.peak <= MAX_SOUNDING, "{}", t.peak);
    h.time_mut().playing = false;
    t.feed(&collect(&mut h, 3));
    assert!(t.sounding.is_empty());
}

#[test]
fn the_cycle_is_a_pure_function_of_the_transport() {
    // Playing through the cycle and locating to the second bar agree on what comes next.
    let setup = |h: &mut DeviceHarness| {
        set(h, "style", 2.0);
        set(h, "humanize", 0.7);
    };
    let reference = {
        let mut h = rig();
        setup(&mut h);
        collect(&mut h, batches_for(24 * BEAT_FRAMES))
    };
    let mut h = rig();
    setup(&mut h);
    let to = 4 * BEAT_FRAMES;
    h.time_mut().tick = to as f64 * 0.64;
    h.time_mut().frame = to;
    let part = collect(&mut h, batches_for(8 * BEAT_FRAMES));
    // From the first hit after the locate point the streams are identical.
    let window = |evs: &[Ev]| -> Vec<Ev> {
        evs.iter()
            .copied()
            .filter(|e| e.frame >= to + BEAT_FRAMES && e.frame < to + 6 * BEAT_FRAMES)
            .collect()
    };
    assert_eq!(window(&part), window(&reference));
    assert!(!window(&part).is_empty());
}

fn random_param_ops() -> impl Strategy<Value = Vec<(usize, f32)>> {
    prop::collection::vec((0usize..61, 0.0f32..1.0), 0..40)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// Whatever the params do, locates, stops, wraps and bypass flips do, every note closes and
    /// no key is started twice without an end.
    #[test]
    fn random_play_never_hangs_a_note(
        ops in random_param_ops(),
        locates in prop::collection::vec((0u64..4000, 0u8..4), 0..6),
        thru_keys in prop::collection::vec(0u8..127, 0..4),
    ) {
        let mut h = rig();
        let mut t = Tracker::default();
        for k in &thru_keys {
            h.push_note("notes_in", on(0, *k, 0.7));
        }
        let mut locates = locates.into_iter();
        let params = DESCRIPTOR.params;
        for (round, chunk) in ops.chunks(4).enumerate() {
            for (i, norm) in chunk {
                let p = &params[*i];
                if p.key == "bypass" || (p.key == "play.mode" && *norm > 0.5) {
                    continue;
                }
                let v = p.min + (p.max - p.min) * norm;
                h.set_param(p.key, v);
            }
            for _ in 0..40 {
                t.feed(&batch(&mut h));
            }
            if let Some((tick, what)) = locates.next() {
                match what {
                    0 => h.time_mut().tick = tick as f64 * 31.0,
                    1 => h.time_mut().playing = !h.time_mut().playing,
                    2 => {
                        h.time_mut().loop_wrap = Some(LoopWrap {
                            offset: (tick % 63 + 1) as u16,
                            to_tick: (tick * 17) as f64,
                        });
                    }
                    _ => h.set_param("bypass", (round % 2) as f32),
                }
            }
            prop_assert!(t.peak <= MAX_SOUNDING + 4);
        }
        // Stop and let everything end.
        h.set_param("bypass", 0.0);
        h.time_mut().playing = false;
        for k in &thru_keys {
            h.push_note("notes_in", off(0, *k));
        }
        for _ in 0..8 {
            t.feed(&batch(&mut h));
        }
        prop_assert!(t.sounding.is_empty(), "{:?}", t.sounding);
    }
}

#[test]
fn the_generator_writes_what_the_device_plays() {
    // The params a generated progression would write play as that progression.
    let key = Key::new(0, KeyMode::Major);
    let p = progression::generate(key, Mood::Pop, 0.0, 3, 4, &[None; 8]);
    let mut h = rig();
    set(&mut h, "bass", 0.0);
    for k in 0..4 {
        set(
            &mut h,
            &format!("slot.{}.root", k + 1),
            f32::from(p.chords[k].root),
        );
        let q = ChordKind::ALL
            .iter()
            .position(|x| *x == p.chords[k].kind)
            .unwrap();
        set(&mut h, &format!("slot.{}.quality", k + 1), q as f32);
        set(
            &mut h,
            &format!("slot.{}.beats", k + 1),
            progression::beats_index(p.beats[k]) as f32,
        );
    }
    let evs = collect(&mut h, batches_for(4 * 4 * BEAT_FRAMES));
    assert_eq!(
        pcs(&keys_at(&evs, 0)),
        atomik_core::SongChord::new(p.chords[0].root, p.chords[0].kind).pitch_mask()
    );
}
