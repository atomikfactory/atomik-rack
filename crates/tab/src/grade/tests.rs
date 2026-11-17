use super::*;

/// A chroma with the classes of `keys` at 1 and a little everywhere else.
fn chroma_of(keys: &[u8]) -> [f32; 12] {
    let mut c = [0.03f32; 12];
    for k in keys {
        c[usize::from(k % 12)] = 1.0;
    }
    c
}

fn note(tick: f64, key: i32) -> Played {
    Played {
        tick,
        key: Some(key),
        chroma: chroma_of(&[u8::try_from(key).unwrap()]),
    }
}

fn strum(tick: f64, keys: &[u8]) -> Played {
    Played {
        tick,
        key: None,
        chroma: chroma_of(keys),
    }
}

/// 80 ms at 1 tick per ms: a window of 80 ticks, on time within 40.
fn params() -> Params {
    Params::from_ms(DEFAULT_WINDOW_MS, 1.0)
}

fn verdicts(g: &PassGrade) -> Vec<Verdict> {
    g.notes.iter().map(|n| n.verdict).collect()
}

#[test]
fn notes_are_graded_by_timing_zone() {
    let exp = [
        Expected::new(0, [40]),
        Expected::new(500, [45]),
        Expected::new(1000, [47]),
        Expected::new(1500, [52]),
    ];
    let played = [
        note(10.0, 40),   // on time
        note(450.0, 45),  // 50 early: early
        note(1060.0, 47), // 60 late: late
    ];
    let g = grade_pass(&exp, &played, &params());
    assert_eq!(
        verdicts(&g),
        [Verdict::Hit, Verdict::Early, Verdict::Late, Verdict::Missed]
    );
    assert_eq!(g.extras, 0);
    assert_eq!(g.notes[1].offset_ticks, Some(-50.0));
    assert_eq!(g.notes[3].offset_ticks, None);
    let t = g.tally();
    assert_eq!((t.hit, t.early, t.late, t.missed), (1, 1, 1, 1));
}

#[test]
fn the_window_edges_are_inclusive_and_beyond_them_is_missed_and_extra() {
    let exp = [Expected::new(0, [40])];
    let at_edge = grade_pass(&exp, &[note(80.0, 40)], &params());
    assert_eq!(at_edge.notes[0].verdict, Verdict::Late);
    let beyond = grade_pass(&exp, &[note(80.5, 40)], &params());
    assert_eq!(beyond.notes[0].verdict, Verdict::Missed);
    assert_eq!(beyond.extras, 1);
}

#[test]
fn a_wrong_pitch_is_wrong_not_missed_and_a_right_one_nearby_wins() {
    let exp = [Expected::new(0, [40])];
    let wrong = grade_pass(&exp, &[note(5.0, 43)], &params());
    assert_eq!(wrong.notes[0].verdict, Verdict::Wrong);
    assert_eq!(wrong.extras, 0);
    // The right note a little later beats the wrong one that came first.
    let both = grade_pass(&exp, &[note(5.0, 43), note(30.0, 40)], &params());
    assert_eq!(both.notes[0].verdict, Verdict::Hit);
    assert_eq!(both.extras, 1);
    // An octave away is the same note.
    assert_eq!(
        grade_pass(&exp, &[note(0.0, 52)], &params()).notes[0].verdict,
        Verdict::Hit
    );
}

#[test]
fn extra_notes_are_counted_and_cost_accuracy() {
    let exp = [Expected::new(0, [40]), Expected::new(400, [45])];
    let played = [
        note(0.0, 40),
        note(400.0, 45),
        note(200.0, 50),
        note(900.0, 40),
    ];
    let g = grade_pass(&exp, &played, &params());
    assert_eq!(g.extras, 2);
    assert_eq!(g.tally().hit, 2);
    // Two hits over two groups and two extras.
    assert!((g.accuracy().unwrap() - 0.5).abs() < 1e-6);
    assert_eq!(g.percent(), 50);
}

#[test]
fn an_event_answers_one_group_only() {
    let exp = [Expected::new(0, [40]), Expected::new(60, [40])];
    let g = grade_pass(&exp, &[note(30.0, 40)], &params());
    // Both groups see the event; the first (equally near) takes it, the other is missed.
    assert_eq!(verdicts(&g), [Verdict::Hit, Verdict::Missed]);
}

#[test]
fn chords_are_graded_by_their_chroma() {
    let c = [48u8, 52, 55, 60, 64];
    let exp = [
        Expected::new(0, c),
        Expected::new(1000, [43, 47, 50, 55]), // G
        Expected::new(2000, c),
    ];
    let played = [
        strum(15.0, &c),              // the right chord
        strum(1000.0, &c),            // C where G was asked
        strum(2020.0, &[48, 52, 55]), // a thinner C is fine
    ];
    let g = grade_pass(&exp, &played, &params());
    assert_eq!(verdicts(&g), [Verdict::Hit, Verdict::Wrong, Verdict::Hit]);
}

#[test]
fn one_note_of_a_chord_is_not_the_chord_and_a_strum_can_answer_a_note() {
    let c = [48u8, 52, 55];
    assert!(chord_score(&chroma_of(&c), pc_mask(&c)) >= DEFAULT_CHORD_THRESHOLD);
    let lone = chord_score(&chroma_of(&[48]), pc_mask(&c));
    assert!(lone < DEFAULT_CHORD_THRESHOLD, "{lone}");
    let two = chord_score(&chroma_of(&[48, 55]), pc_mask(&c));
    assert!(two >= DEFAULT_CHORD_THRESHOLD, "{two}");
    assert_eq!(chord_score(&[0.0; 12], pc_mask(&c)), 0.0);
    // A note asked for, heard without a pitch but with a clear chroma.
    let exp = [Expected::new(0, [57])];
    let g = grade_pass(&exp, &[strum(0.0, &[57])], &params());
    assert_eq!(g.notes[0].verdict, Verdict::Hit);
}

#[test]
fn the_window_follows_the_tick_rate_so_it_shrinks_at_half_speed() {
    // 0.48 ticks per ms at 100 %, 0.24 at half speed: 80 ms is 38.4 or 19.2 ticks.
    let full = Params::from_ms(80.0, 0.48);
    let half = Params::from_ms(80.0, 0.24);
    assert!((full.window_ticks - 38.4).abs() < 1e-9);
    assert!((half.window_ticks - 19.2).abs() < 1e-9);
    assert!((half.hit_ticks - 9.6).abs() < 1e-9);
    // Thirty ticks off is inside the window at full speed and a miss at half speed; the same
    // wall-clock error at half speed is 15 ticks, which is inside.
    let exp = [Expected::new(0, [40])];
    assert_eq!(
        grade_pass(&exp, &[note(30.0, 40)], &full).notes[0].verdict,
        Verdict::Late
    );
    assert_eq!(
        grade_pass(&exp, &[note(30.0, 40)], &half).notes[0].verdict,
        Verdict::Missed
    );
    assert_eq!(
        grade_pass(&exp, &[note(15.0, 40)], &half).notes[0].verdict,
        Verdict::Late
    );
}

#[test]
fn notes_that_start_together_are_one_group() {
    let g = group_notes([(0, 52), (480, 40), (0, 40), (0, 52), (480, 45)]);
    assert_eq!(
        g,
        [Expected::new(0, [40, 52]), Expected::new(480, [40, 45])]
    );
    assert!(g[0].is_chord());
    assert!(!Expected::new(0, [40]).is_chord());
}

#[test]
fn accuracy_of_an_empty_pass_is_none() {
    let g = grade_pass(&[], &[], &params());
    assert_eq!(g.accuracy(), None);
    assert_eq!(g.percent(), 0);
    let only_extra = grade_pass(&[], &[note(0.0, 40)], &params());
    assert_eq!(only_extra.accuracy(), Some(0.0));
}

#[test]
fn the_session_keeps_history_streak_and_best() {
    let exp: Vec<Expected> = (0..4).map(|i| Expected::new(i * 500, [40])).collect();
    let all: Vec<Played> = (0..4).map(|i| note(f64::from(i) * 500.0, 40)).collect();
    let mut s = Session::default();
    assert_eq!(s.last(), None);
    let a = s.record(&grade_pass(&exp, &all, &params()), 75);
    assert_eq!((a.percent, a.speed), (100, 75));
    assert_eq!((s.streak(), s.best_streak()), (4, 4));
    // A pass with the third note missed: the streak ends there and one more follows.
    let mut some = all.clone();
    some.remove(2);
    let b = s.record(&grade_pass(&exp, &some, &params()), 75);
    assert_eq!(b.percent, 75);
    assert_eq!(b.tally.missed, 1);
    assert_eq!((s.streak(), s.best_streak(), s.passes()), (1, 6, 2));
    assert_eq!(s.history().len(), 2);
    for _ in 0..20 {
        s.record(&grade_pass(&exp, &all, &params()), 100);
    }
    assert_eq!(s.history().len(), HISTORY_LEN);
    assert_eq!(s.passes(), 22);
    s.clear();
    assert_eq!((s.passes(), s.streak(), s.best_streak()), (0, 0, 0));
    assert!(s.history().is_empty());
}

#[test]
fn calibration_is_the_median_lateness() {
    // A player 40 ms late with two stray hits.
    let offsets = [38.0, 41.0, 44.0, 40.0, 39.0, 210.0, 500.0];
    let off = calibration_offset_ms(&offsets).unwrap();
    assert!((off - 40.5).abs() < 1e-9, "{off}");
    assert_eq!(calibration_offset_ms(&[10.0, 12.0]), None);
    assert_eq!(calibration_offset_ms(&[f64::NAN, 1.0, 2.0]), None);
    // Even count: the mean of the two middle values.
    let even = calibration_offset_ms(&[-10.0, -20.0, -30.0, -40.0]).unwrap();
    assert!((even + 25.0).abs() < 1e-9);
}
#[test]
fn offsets_to_the_nearest_beat() {
    let o = beat_offsets(&[10.0, 470.0, 1000.0, 1450.0], 480.0);
    let want = [10.0, -10.0, 40.0, 10.0];
    for (a, b) in o.iter().zip(want) {
        assert!((a - b).abs() < 1e-9, "{o:?}");
    }
    assert!(beat_offsets(&[1.0], 0.0).is_empty());
}
