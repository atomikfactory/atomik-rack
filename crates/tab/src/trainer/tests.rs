use super::*;
use crate::grade::pc_mask;
use crate::instrument::{Capo, find_tuning};
use crate::voicing::{VoicingOptions, generate};

/// A chroma with the classes of `mask` at 1 and a little everywhere else.
fn chroma_of(mask: u16) -> [f32; 12] {
    let mut c = [0.03f32; 12];
    for (pc, v) in c.iter_mut().enumerate() {
        if mask >> pc & 1 == 1 {
            *v = 1.0;
        }
    }
    c
}

fn names(chords: &[ChordSymbol]) -> Vec<String> {
    chords.iter().map(ChordSymbol::name).collect()
}

fn mask_of(c: &ChordSymbol) -> u16 {
    pc_mask(&c.pitch_classes())
}

fn set(list: &str) -> (Vec<String>, Vec<u16>) {
    let l = parse_chord_list(list);
    (names(&l.chords), l.chords.iter().map(mask_of).collect())
}

fn cfg(mode: DrillMode, order: Order, bpm: u16, beats: u8) -> DrillConfig {
    DrillConfig {
        mode,
        order,
        bpm,
        beats,
        ..DrillConfig::default()
    }
}

fn drill(list: &str, mode: DrillMode, order: Order, bpm: u16, beats: u8) -> (Drill, Vec<u16>) {
    let (n, m) = set(list);
    (
        Drill::new(n, m.clone(), cfg(mode, order, bpm, beats), (0, 1), 1),
        m,
    )
}

fn hear(d: &mut Drill, time: f64, mask: u16) -> Vec<DrillEvent> {
    d.hear(&Heard {
        time,
        chroma: chroma_of(mask),
    })
}

// ------------------------------------------------------------------------ chord sets

#[test]
fn the_triads_of_g_major_are_the_diatonic_chords() {
    let set = diatonic(key_of(7, false), false);
    assert_eq!(names(&set), ["G", "Am", "Bm", "C", "D", "Em", "F#dim"]);
}

#[test]
fn the_sevenths_of_c_major_and_a_minor() {
    assert_eq!(
        names(&diatonic(key_of(0, false), true)),
        ["Cmaj7", "Dm7", "Em7", "Fmaj7", "G7", "Am7", "Bm7b5"]
    );
    assert_eq!(
        names(&diatonic(key_of(9, true), true)),
        ["Am7", "Bm7b5", "Cmaj7", "Dm7", "Em7", "Fmaj7", "G7"]
    );
}

#[test]
fn flat_keys_are_spelled_with_flats() {
    assert_eq!(
        names(&diatonic(key_of(3, false), false)),
        ["Eb", "Fm", "Gm", "Ab", "Bb", "Cm", "Ddim"]
    );
}

#[test]
fn every_key_gives_seven_distinct_chords_on_its_tonic() {
    for pc in 0..12u8 {
        for minor in [false, true] {
            for sevenths in [false, true] {
                let chords = diatonic(key_of(pc, minor), sevenths);
                assert_eq!(chords.len(), 7);
                let mut n = names(&chords);
                n.sort();
                n.dedup();
                assert_eq!(n.len(), 7, "key {pc} minor {minor}");
                assert_eq!(chords[0].root_pc(), pc);
            }
        }
    }
}

#[test]
fn the_key_chords_have_a_fingering_on_every_instrument_including_banjo_open_g() {
    let chords = diatonic(key_of(7, false), false);
    let tunings = [
        ("guitar6", "standard"),
        ("banjo5", "open_g"),
        ("mandolin", "standard"),
        ("bass4", "standard"),
    ];
    let mut checked = 0;
    for (inst, tuning) in tunings {
        let Some(t) = find_tuning(inst, tuning) else {
            continue;
        };
        checked += 1;
        let opts = if inst.starts_with("bass") {
            VoicingOptions::bass()
        } else {
            VoicingOptions::default()
        };
        for c in &chords[..6] {
            let v = generate(&t.tuning, Capo::NONE, c, &opts);
            assert!(!v.is_empty(), "{} on {inst}", c.name());
        }
    }
    assert!(checked >= 3, "the catalogue holds the instruments");
    let banjo = find_tuning("banjo5", "open_g").expect("banjo open G");
    let g = ChordSymbol::parse("G").unwrap();
    let v = generate(&banjo.tuning, Capo::NONE, &g, &VoicingOptions::default());
    // The open strings of open G are the G chord.
    assert_eq!(
        v[0].pc_mask(&banjo.tuning, Capo::NONE) & mask_of(&g),
        mask_of(&g)
    );
}

#[test]
fn a_typed_list_reports_the_words_that_are_not_chords() {
    let l = parse_chord_list("Am, F C; G | Am xyz H7q\nEm");
    assert_eq!(names(&l.chords), ["Am", "F", "C", "G", "Em"]);
    assert_eq!(l.invalid, ["xyz", "H7q"]);
    assert_eq!(parse_chord_list("").chords.len(), 0);
}

#[test]
fn a_list_is_cut_at_the_most_chords() {
    let text =
        "C D E F G A B Cm Dm Em Fm Gm Am Bm C7 D7 E7 F7 G7 A7 B7 Cmaj7 Dmaj7 Emaj7 Fmaj7 Gmaj7";
    assert_eq!(parse_chord_list(text).chords.len(), MAX_CHORDS);
}

// ------------------------------------------------------------------------ order

#[test]
fn sequential_order_goes_round_the_set() {
    let (mut d, _) = drill("C G Am", DrillMode::Timed, Order::Sequential, 120, 1);
    d.start(0.0);
    let mut seen = vec![d.current()];
    for step in 1..=6 {
        d.advance(f64::from(step) * 0.5);
        seen.push(d.current());
    }
    assert_eq!(seen, [0, 1, 2, 0, 1, 2, 0]);
}

#[test]
fn random_order_never_repeats_a_chord_and_is_the_same_for_a_seed() {
    let run = |seed| {
        let (n, m) = set("C G Am F Dm");
        let mut d = Drill::new(
            n,
            m,
            cfg(DrillMode::Timed, Order::Random, 120, 1),
            (0, 1),
            seed,
        );
        d.start(0.0);
        let mut seen = vec![d.current()];
        for step in 1..=40 {
            d.advance(f64::from(step) * 0.5);
            seen.push(d.current());
        }
        seen
    };
    let a = run(7);
    assert!(a.windows(2).all(|w| w[0] != w[1]), "{a:?}");
    assert_eq!(a, run(7));
    assert_ne!(a, run(8));
    let mut all = a.clone();
    all.sort_unstable();
    all.dedup();
    assert_eq!(all.len(), 5, "every chord comes up: {a:?}");
}

#[test]
fn a_pair_drill_alternates_between_the_two_chords() {
    let (n, m) = set("C G Am F");
    let mut d = Drill::new(n, m, cfg(DrillMode::Timed, Order::Pair, 120, 2), (3, 1), 1);
    d.start(0.0);
    assert_eq!((d.current(), d.next()), (3, 1));
    let mut seen = vec![d.current()];
    for step in 1..=5 {
        d.advance(f64::from(step));
        seen.push(d.current());
    }
    assert_eq!(seen, [3, 1, 3, 1, 3, 1]);
}

#[test]
fn a_pair_of_one_chord_is_made_two() {
    let (n, m) = set("C G Am");
    let d = Drill::new(n, m, cfg(DrillMode::Timed, Order::Pair, 120, 2), (1, 1), 1);
    assert_ne!(d.current(), d.next());
}

#[test]
fn a_drill_of_one_chord_does_not_start() {
    let (n, m) = set("C");
    let mut d = Drill::new(n, m, DrillConfig::default(), (0, 0), 1);
    d.start(0.0);
    assert!(!d.running());
}

// ------------------------------------------------------------------------ timed

#[test]
fn the_clock_gives_one_click_per_beat_and_an_accent_per_chord() {
    // 120 bpm: a beat every half second; 4 beats per chord.
    let (mut d, _) = drill("C G", DrillMode::Timed, Order::Sequential, 120, 4);
    d.start(10.0);
    assert_eq!(d.advance(10.0), [DrillEvent::Click { accent: true }]);
    assert_eq!(d.advance(10.49), []);
    let clicks: Vec<bool> = d
        .advance(11.6)
        .iter()
        .filter_map(|e| match e {
            DrillEvent::Click { accent } => Some(*accent),
            _ => None,
        })
        .collect();
    assert_eq!(
        clicks,
        [false, false, false],
        "beats at 10.5, 11.0 and 11.5"
    );
    assert_eq!(
        d.advance(12.0),
        [
            DrillEvent::Click { accent: true },
            DrillEvent::Changed { from: 0, to: 1 }
        ]
    );
    assert_eq!(d.changes(), 1);
    assert_eq!(d.beat_in_chord(), 0);
}

#[test]
fn a_late_frame_catches_up_every_beat_it_skipped() {
    let (mut d, _) = drill("C G", DrillMode::Timed, Order::Sequential, 60, 2);
    d.start(0.0);
    let ev = d.advance(5.0);
    let clicks = ev
        .iter()
        .filter(|e| matches!(e, DrillEvent::Click { .. }))
        .count();
    assert_eq!(clicks, 6, "beats 0 to 5");
    assert_eq!(d.changes(), 2);
}

#[test]
fn changes_per_minute_and_the_session_timer_follow_the_clock() {
    let (mut d, _) = drill("C G", DrillMode::Timed, Order::Sequential, 120, 2);
    d.start(100.0);
    d.advance(130.0);
    assert!((d.elapsed(130.0) - 30.0).abs() < 1e-9);
    // A change every second: 30 changes in 30 seconds.
    assert_eq!(d.changes(), 30);
    assert!((d.changes_per_minute(130.0) - 60.0).abs() < 0.01);
    d.stop(130.0);
    assert!((d.elapsed(500.0) - 30.0).abs() < 1e-9, "frozen at the stop");
    assert!(d.advance(500.0).is_empty());
}

#[test]
fn a_plain_timed_drill_judges_nothing() {
    let (mut d, _) = drill("C G", DrillMode::Timed, Order::Sequential, 120, 2);
    d.start(0.0);
    d.advance(20.0);
    d.stop(20.0);
    assert!(d.outcomes().is_empty());
    assert!(d.pair_results().is_empty());
}

// ------------------------------------------------------------------------ verified

/// Plays `seconds` of a one-second-window drill: each chord is heard 0.1 s after its beat,
/// except in the windows listed in `silent`.
fn play(d: &mut Drill, masks: &[u16], seconds: f64, silent: &[usize]) {
    let mut t = 0.0;
    let mut window = 0usize;
    while t < seconds {
        d.advance(t);
        if !silent.contains(&window) {
            hear(d, t + 0.1, masks[d.current()]);
        }
        window += 1;
        t += 1.0;
    }
    d.advance(seconds + 0.5);
}

#[test]
fn verified_windows_are_hits_when_the_chord_is_heard_in_them() {
    let (mut d, m) = drill("C G", DrillMode::Verified, Order::Sequential, 120, 2);
    d.start(0.0);
    play(&mut d, &m, 10.0, &[]);
    d.stop(10.5);
    let (hits, judged) = d.hits();
    assert_eq!(judged, hits);
    assert!(judged >= 9, "{judged}");
    let results = d.pair_results();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].key, "C|G");
    assert!((results[0].hit_rate() - 1.0).abs() < 1e-6);
    // One change per second: 60 a minute.
    assert!((results[0].cpm - 60.0).abs() < 0.01);
}

#[test]
fn verified_windows_without_the_chord_are_misses() {
    let (mut d, m) = drill("C G", DrillMode::Verified, Order::Sequential, 120, 2);
    d.start(0.0);
    play(&mut d, &m, 8.0, &[1, 3]);
    d.stop(9.0);
    let (hits, judged) = d.hits();
    assert!(d.outcomes().iter().any(|o| !o.hit));
    assert!(hits > 0 && hits < judged, "{hits} of {judged}");
}

#[test]
fn the_old_chord_ringing_into_a_window_is_not_the_new_chord() {
    let (mut d, m) = drill("C G", DrillMode::Verified, Order::Sequential, 120, 2);
    d.start(0.0);
    d.advance(0.0);
    d.advance(1.0); // window 1 (G) opens
    hear(&mut d, 1.2, m[0]);
    d.advance(3.0);
    assert!(d.outcomes().iter().all(|o| !o.hit));
    // Heard in time, it counts.
    let (mut d, m) = drill("C G", DrillMode::Verified, Order::Sequential, 120, 2);
    d.start(0.0);
    d.advance(1.0);
    hear(&mut d, 1.4, m[1]);
    d.advance(3.0);
    let o = d.outcomes();
    assert_eq!(o.len(), 1);
    assert!(o[0].hit);
}

#[test]
fn a_chord_played_a_little_early_still_counts() {
    let (mut d, m) = drill("C G", DrillMode::Verified, Order::Sequential, 120, 2);
    d.start(0.0);
    d.advance(0.5); // window 0 only (G opens at 1.0)
    // 0.2 s before G's beat, within the tolerance of a quarter second.
    hear(&mut d, 0.8, m[1]);
    d.advance(1.0);
    d.advance(3.0);
    assert!(d.outcomes()[0].hit);
}

// ------------------------------------------------------------------------ listen

#[test]
fn listen_mode_moves_on_when_the_chord_is_heard() {
    let (mut d, m) = drill("C G Am", DrillMode::Listen, Order::Sequential, 60, 4);
    d.start(5.0);
    assert!(d.advance(9.0).is_empty(), "no clock in the listen mode");
    // The wrong chord and a chord from before it was shown change nothing.
    hear(&mut d, 6.0, m[1]);
    hear(&mut d, 4.0, m[0]);
    assert_eq!((d.current(), d.recognised()), (0, false));
    let ev = hear(&mut d, 6.5, m[0]);
    assert_eq!(ev, [DrillEvent::Changed { from: 0, to: 1 }]);
    assert_eq!(d.current(), 1);
    // The first chord's own recognition is no change.
    assert!(d.outcomes().is_empty());
    hear(&mut d, 8.0, m[1]);
    assert_eq!(d.current(), 2);
    let o = d.outcomes();
    assert_eq!(o.len(), 1);
    assert_eq!((o[0].from, o[0].to, o[0].hit), (0, 1, true));
    assert!((o[0].seconds.unwrap() - 1.5).abs() < 1e-9);
}

#[test]
fn skipping_a_chord_is_a_miss_and_only_in_the_listen_mode() {
    let (mut d, m) = drill("C G", DrillMode::Listen, Order::Sequential, 60, 4);
    d.start(0.0);
    hear(&mut d, 1.0, m[0]);
    assert!(d.skip(2.0));
    let o = d.outcomes();
    assert_eq!((o.len(), o[0].hit), (1, false));
    let (mut t, _) = drill("C G", DrillMode::Timed, Order::Sequential, 60, 4);
    t.start(0.0);
    assert!(!t.skip(1.0));
}

#[test]
fn the_listen_speed_is_the_mean_time_to_hear_a_chord() {
    let (mut d, m) = drill("Am C", DrillMode::Listen, Order::Pair, 60, 4);
    d.start(0.0);
    let mut t = 0.0;
    for _ in 0..7 {
        t += 2.0;
        let c = d.current();
        hear(&mut d, t, m[c]);
    }
    let r = d.pair_results();
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].key, "Am|C");
    // The first hit has no change into it; six changes two seconds apart: 30 a minute.
    assert_eq!(r[0].attempts, 6);
    assert!((r[0].cpm - 30.0).abs() < 0.01, "{}", r[0].cpm);
}

#[test]
fn the_recognised_indicator_follows_the_current_chord_in_the_timed_modes() {
    let (mut d, m) = drill("C G", DrillMode::Verified, Order::Sequential, 120, 2);
    d.start(0.0);
    d.advance(0.0);
    assert!(!d.recognised());
    hear(&mut d, 0.2, m[0]);
    assert!(d.recognised());
    d.advance(1.0);
    assert!(!d.recognised(), "a new chord starts unheard");
}

// ------------------------------------------------------------------------ scores

fn result(key: &str, attempts: u32, hits: u32, cpm: f32) -> PairResult {
    PairResult {
        key: key.to_owned(),
        attempts,
        hits,
        cpm,
    }
}

#[test]
fn pair_keys_ignore_the_direction() {
    assert_eq!(pair_key("C", "Am"), pair_key("Am", "C"));
    assert_eq!(pair_label(&pair_key("G", "C")), "C - G");
}

#[test]
fn the_best_speed_counts_only_with_a_clean_hit_rate() {
    let mut s = Scores::new();
    assert!(s.record(&result("Am|C", 8, 7, 40.0)));
    let b = *s.get("Am|C").unwrap();
    assert_eq!((b.cpm, b.sessions), (40.0, 1));
    assert!((b.hit_rate - 0.875).abs() < 1e-6);
    // Faster but sloppy (50 %): the speed does not count, the session does.
    assert!(s.record(&result("Am|C", 8, 4, 60.0)));
    let b = *s.get("Am|C").unwrap();
    assert_eq!((b.cpm, b.sessions), (40.0, 2));
    // Faster and clean: the best moves.
    assert!(s.record(&result("Am|C", 8, 8, 55.0)));
    let b = *s.get("Am|C").unwrap();
    assert_eq!(b.cpm, 55.0);
    assert!((b.hit_rate - 1.0).abs() < 1e-6);
}

#[test]
fn a_slower_or_worse_session_keeps_the_best() {
    let mut s = Scores::new();
    s.record(&result("C|G", 10, 10, 50.0));
    s.record(&result("C|G", 10, 8, 30.0));
    let b = *s.get("C|G").unwrap();
    assert_eq!(b.cpm, 50.0);
    assert!((b.hit_rate - 1.0).abs() < 1e-6);
    assert_eq!(b.sessions, 2);
}

#[test]
fn sessions_with_too_few_changes_are_ignored() {
    let mut s = Scores::new();
    assert!(!s.record(&result("C|G", MIN_ATTEMPTS - 1, 2, 90.0)));
    assert!(s.is_empty());
}

#[test]
fn scores_come_back_from_storage_sanitised() {
    let best = |cpm, hit_rate| Best {
        cpm,
        hit_rate,
        sessions: 2,
    };
    let s = Scores::from_rows([
        ("C|G".to_owned(), best(5000.0, 3.0)),
        (String::new(), best(1.0, 1.0)),
        ("Am|C".to_owned(), best(f32::NAN, 0.5)),
    ]);
    assert_eq!(s.len(), 1);
    let b = s.get("C|G").unwrap();
    assert_eq!((b.cpm, b.hit_rate), (1000.0, 1.0));
}

#[test]
fn a_finished_session_updates_the_scores_of_its_pairs() {
    let (mut d, m) = drill("C G", DrillMode::Verified, Order::Sequential, 120, 2);
    d.start(0.0);
    play(&mut d, &m, 10.0, &[]);
    d.stop(10.5);
    let mut s = Scores::new();
    assert!(s.record_all(&d.pair_results()));
    let b = s.get("C|G").unwrap();
    assert!((b.cpm - 60.0).abs() < 0.01);
    s.clear();
    assert!(s.is_empty());
}
