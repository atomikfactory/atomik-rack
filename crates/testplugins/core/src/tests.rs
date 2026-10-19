//! Tests of the shared test-plugin DSP (reference DSP).
// Justification: the DSP is exact arithmetic on known inputs; the tests compare exact values.
#![allow(clippy::float_cmp)]

use super::*;

fn block(v: f32) -> Block {
    [v; BLOCK]
}

fn ramp() -> Block {
    let mut b = [0.0; BLOCK];
    for (i, s) in b.iter_mut().enumerate() {
        *s = (i as f32 + 1.0) / 100.0;
    }
    b
}

fn peak(b: &Block) -> f32 {
    b.iter().fold(0.0f32, |m, s| m.max(s.abs()))
}

// ----- gain effect --------------------------------------------------------------------------

fn run_gain(fx: &mut GainFx, input: &[Block; 2], sc: Option<&[Block; 2]>) -> [Block; 2] {
    let mut out = [[0.0; BLOCK]; 2];
    fx.process(input, sc, &mut out);
    out
}

#[test]
fn the_gain_effect_defaults_to_a_unity_passthrough() {
    let mut fx = GainFx::new(48_000.0);
    let input = [ramp(), block(0.5)];
    let out = run_gain(&mut fx, &input, None);
    assert_eq!(out, input);
    assert_eq!(fx.latency_frames(), 0);
    assert_eq!(fx.tail_frames(), 0);
}

#[test]
fn gain_is_in_decibels_and_clamped() {
    let mut fx = GainFx::new(48_000.0);
    fx.set_param(GainFx::GAIN, -6.0206);
    let out = run_gain(&mut fx, &[block(1.0), block(1.0)], None);
    assert!((out[0][10] - 0.5).abs() < 1e-3, "{}", out[0][10]);
    fx.set_param(GainFx::GAIN, 1000.0);
    assert_eq!(fx.param(GainFx::GAIN), 12.0);
    fx.set_param(GainFx::GAIN, -1000.0);
    assert_eq!(fx.param(GainFx::GAIN), -60.0);
    fx.set_param(GainFx::GAIN, f64::NAN);
    assert_eq!(fx.param(GainFx::GAIN), 0.0, "NaN falls back to the default");
}

#[test]
fn mute_silences_the_output() {
    let mut fx = GainFx::new(48_000.0);
    fx.set_param(GainFx::MUTE, 1.0);
    let out = run_gain(&mut fx, &[block(1.0), block(1.0)], None);
    assert!(out.iter().all(|c| c.iter().all(|s| *s == 0.0)));
    fx.set_param(GainFx::MUTE, 0.0);
    let out = run_gain(&mut fx, &[block(1.0), block(1.0)], None);
    assert_eq!(out[0][0], 1.0);
}

#[test]
fn latency_delays_the_signal_by_exactly_that_many_frames_across_blocks() {
    let mut fx = GainFx::new(48_000.0);
    fx.set_param(GainFx::LATENCY, 100.0);
    assert_eq!(fx.latency_frames(), 100);
    assert_eq!(fx.tail_frames(), 100);
    let mut impulse = [[0.0; BLOCK]; 2];
    impulse[0][5] = 1.0;
    let mut heard = Vec::new();
    for b in 0..4 {
        let input = if b == 0 { impulse } else { [[0.0; BLOCK]; 2] };
        let out = run_gain(&mut fx, &input, None);
        heard.extend_from_slice(&out[0]);
    }
    let hits: Vec<usize> = heard
        .iter()
        .enumerate()
        .filter(|(_, s)| **s != 0.0)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(hits, vec![105]);
    assert_eq!(heard[105], 1.0);
}

#[test]
fn latency_is_stepped_to_whole_frames_up_to_512() {
    let mut fx = GainFx::new(48_000.0);
    fx.set_param(GainFx::LATENCY, 100.4);
    assert_eq!(fx.latency_frames(), 100);
    fx.set_param(GainFx::LATENCY, 9999.0);
    assert_eq!(fx.latency_frames(), 512);
    fx.set_param(GainFx::LATENCY, -3.0);
    assert_eq!(fx.latency_frames(), 0);
}

#[test]
fn the_sidechain_ducks_the_output_and_a_silent_one_does_not() {
    let mut fx = GainFx::new(48_000.0);
    let input = [block(0.5), block(0.5)];
    let quiet = [[0.0; BLOCK]; 2];
    let loud = [block(1.0), block(1.0)];
    let plain = run_gain(&mut fx, &input, Some(&quiet));
    assert_eq!(
        plain[0][63], 0.5,
        "a silent sidechain leaves the signal alone"
    );
    let mut fx2 = GainFx::new(48_000.0);
    let ducked = run_gain(&mut fx2, &input, Some(&loud));
    assert!(ducked[0][63] < 0.2, "{}", ducked[0][63]);
    assert!(ducked[0][63] > 0.0);
    // Depth 0 turns the ducking off.
    let mut fx3 = GainFx::new(48_000.0);
    fx3.set_param(GainFx::DUCK, 0.0);
    let off = run_gain(&mut fx3, &input, Some(&loud));
    assert_eq!(off[0][63], 0.5);
    // None and an all-zero sidechain agree bit for bit.
    let mut a = GainFx::new(48_000.0);
    let mut b = GainFx::new(48_000.0);
    assert_eq!(
        run_gain(&mut a, &input, None),
        run_gain(&mut b, &input, Some(&quiet))
    );
}

#[test]
fn the_duck_envelope_releases_over_the_following_blocks() {
    let mut fx = GainFx::new(48_000.0);
    let input = [block(1.0), block(1.0)];
    let loud = [block(1.0), block(1.0)];
    let quiet = [[0.0; BLOCK]; 2];
    run_gain(&mut fx, &input, Some(&loud));
    let mut last = 0.0;
    for _ in 0..200 {
        last = run_gain(&mut fx, &input, Some(&quiet))[0][63];
    }
    assert!(last > 0.99, "{last}");
}

#[test]
fn the_state_round_trips_and_bad_state_is_rejected() {
    let mut fx = GainFx::new(48_000.0);
    fx.set_param(GainFx::GAIN, -12.0);
    fx.set_param(GainFx::MUTE, 1.0);
    fx.set_param(GainFx::LATENCY, 33.0);
    fx.set_param(GainFx::DUCK, 0.25);
    let state = fx.save_state();
    let mut other = GainFx::new(48_000.0);
    assert!(other.load_state(&state));
    for id in [GainFx::GAIN, GainFx::MUTE, GainFx::LATENCY, GainFx::DUCK] {
        assert_eq!(other.param(id), fx.param(id), "param {id}");
    }
    assert!(!other.load_state(&[]));
    assert!(!other.load_state(&state[..state.len() - 1]));
    let mut bad = state.clone();
    bad[0] ^= 0xFF;
    assert!(!other.load_state(&bad));
    // A failed load changes nothing.
    assert_eq!(other.param(GainFx::GAIN), -12.0);
}

#[test]
fn the_gain_effect_is_deterministic() {
    let run = || {
        let mut fx = GainFx::new(44_100.0);
        fx.set_param(GainFx::GAIN, -3.0);
        fx.set_param(GainFx::LATENCY, 70.0);
        let mut all = Vec::new();
        for i in 0..20 {
            let input = [ramp(), block(i as f32 / 20.0)];
            let sc = [block((i % 3) as f32 / 3.0), block(0.0)];
            all.push(run_gain(&mut fx, &input, Some(&sc)));
        }
        all
    };
    assert_eq!(run(), run());
}

#[test]
fn reset_clears_the_delay_line_and_the_envelope() {
    let mut fx = GainFx::new(48_000.0);
    fx.set_param(GainFx::LATENCY, 64.0);
    run_gain(
        &mut fx,
        &[block(1.0), block(1.0)],
        Some(&[block(1.0), block(1.0)]),
    );
    fx.reset();
    let out = run_gain(&mut fx, &[block(0.5), block(0.5)], None);
    assert!(
        out[0].iter().all(|s| *s == 0.0),
        "delayed silence after reset"
    );
}

// ----- synth ---------------------------------------------------------------------------------

fn ev(offset: u16, event: Event) -> TimedEvent {
    TimedEvent { offset, event }
}

fn on(offset: u16, key: u8, id: i32) -> TimedEvent {
    ev(
        offset,
        Event::NoteOn {
            key,
            velocity: 1.0,
            id,
        },
    )
}

fn run_synth(s: &mut TestSynth, events: &[TimedEvent]) -> [Block; 2] {
    let mut out = [[0.0; BLOCK]; 2];
    s.process(events, &mut out);
    out
}

fn saw_synth() -> TestSynth {
    let mut s = TestSynth::new(48_000.0);
    s.set_param(TestSynth::WAVE, 1.0);
    s
}

#[test]
fn a_note_starts_at_its_exact_frame() {
    let mut s = saw_synth();
    let out = run_synth(&mut s, &[on(17, 69, 1)]);
    assert!(out[0][..17].iter().all(|x| *x == 0.0));
    assert!(out[0][17] != 0.0, "the first frame of the note sounds");
    assert_eq!(out[0], out[1], "mono source, both channels equal");
    assert!(!s.is_idle());
}

#[test]
fn an_off_releases_the_matching_voice_and_the_synth_goes_idle() {
    let mut s = saw_synth();
    run_synth(&mut s, &[on(0, 60, 7)]);
    run_synth(&mut s, &[ev(0, Event::NoteOff { key: 60, id: 7 })]);
    let mut idle_after = None;
    for b in 0..40 {
        run_synth(&mut s, &[]);
        if s.is_idle() {
            idle_after = Some(b);
            break;
        }
    }
    assert!(idle_after.is_some_and(|b| b < 10), "{idle_after:?}");
    let out = run_synth(&mut s, &[]);
    assert!(out[0].iter().all(|x| *x == 0.0));
}

#[test]
fn an_off_matches_by_id_so_two_notes_on_one_key_stay_separate() {
    let mut s = saw_synth();
    run_synth(&mut s, &[on(0, 60, 1), on(1, 60, 2)]);
    run_synth(&mut s, &[ev(0, Event::NoteOff { key: 60, id: 1 })]);
    for _ in 0..40 {
        run_synth(&mut s, &[]);
    }
    assert!(!s.is_idle(), "voice 2 still holds");
    run_synth(&mut s, &[ev(0, Event::NoteOff { key: 60, id: 2 })]);
    for _ in 0..40 {
        run_synth(&mut s, &[]);
    }
    assert!(s.is_idle());
}

#[test]
fn all_notes_off_cc_releases_every_voice() {
    let mut s = saw_synth();
    run_synth(&mut s, &[on(0, 60, 1), on(0, 64, 2), on(0, 67, 3)]);
    run_synth(
        &mut s,
        &[ev(
            0,
            Event::Cc {
                controller: 123,
                value: 0.0,
            },
        )],
    );
    for _ in 0..40 {
        run_synth(&mut s, &[]);
    }
    assert!(s.is_idle());
}

#[test]
fn pitch_bend_moves_the_pitch_by_two_semitones_at_full_deflection() {
    let zero_crossings = |bend: f32| {
        let mut s = TestSynth::new(48_000.0);
        let mut events = vec![on(0, 69, 1)];
        events.push(ev(0, Event::PitchBend(bend)));
        let mut all = Vec::new();
        for b in 0..120 {
            let e: &[TimedEvent] = if b == 0 { &events } else { &[] };
            all.extend_from_slice(&run_synth(&mut s, e)[0]);
        }
        all[200..]
            .windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count() as f64
    };
    let base = zero_crossings(0.0);
    let up = zero_crossings(1.0);
    let ratio = up / base;
    assert!((ratio - 2f64.powf(2.0 / 12.0)).abs() < 0.03, "{ratio}");
}

#[test]
fn note_expression_tuning_and_pressure_change_the_sound() {
    let render = |extra: Option<TimedEvent>| {
        let mut s = saw_synth();
        let mut ev0 = vec![on(0, 60, 9)];
        ev0.extend(extra);
        let mut all = Vec::new();
        for b in 0..10 {
            let e: &[TimedEvent] = if b == 0 { &ev0 } else { &[] };
            all.extend_from_slice(&run_synth(&mut s, e)[0]);
        }
        all
    };
    let plain = render(None);
    let tuned = render(Some(ev(
        0,
        Event::Expr {
            key: 60,
            id: 9,
            kind: ExprKind::Tuning,
            value: 3.0,
        },
    )));
    assert_ne!(plain, tuned);
    let soft = render(Some(ev(
        0,
        Event::PolyPressure {
            key: 60,
            id: 9,
            value: 0.0,
        },
    )));
    let loud = render(Some(ev(
        0,
        Event::PolyPressure {
            key: 60,
            id: 9,
            value: 1.0,
        },
    )));
    let p = |v: &[f32]| v.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(p(&soft) < p(&loud), "{} {}", p(&soft), p(&loud));
    // An expression for another id changes nothing.
    let other = render(Some(ev(
        0,
        Event::Expr {
            key: 61,
            id: 99,
            kind: ExprKind::Tuning,
            value: 3.0,
        },
    )));
    assert_eq!(plain, other);
}

#[test]
fn the_mod_wheel_and_channel_pressure_are_audible() {
    let render = |extra: Event| {
        let mut s = saw_synth();
        let events = [on(0, 60, 1), ev(0, extra)];
        let mut all = Vec::new();
        for b in 0..20 {
            let e: &[TimedEvent] = if b == 0 { &events } else { &[] };
            all.extend_from_slice(&run_synth(&mut s, e)[0]);
        }
        all
    };
    let none = render(Event::Cc {
        controller: 7,
        value: 0.3,
    });
    let wheel = render(Event::Cc {
        controller: 1,
        value: 1.0,
    });
    assert_ne!(none, wheel, "CC1 adds vibrato");
    let press = render(Event::ChannelPressure(1.0));
    assert_ne!(none, press);
}

#[test]
fn the_ninth_note_steals_the_oldest_voice() {
    let mut s = saw_synth();
    let events: Vec<TimedEvent> = (0..9).map(|i| on(i as u16, 40 + i as u8, i + 1)).collect();
    run_synth(&mut s, &events);
    assert_eq!(s.active_voices(), TestSynth::VOICES);
    // The voice of note id 1 was stolen: its Off finds nothing and the others stay.
    run_synth(&mut s, &[ev(0, Event::NoteOff { key: 40, id: 1 })]);
    assert_eq!(s.active_voices(), TestSynth::VOICES);
}

#[test]
fn the_synth_output_is_deterministic_and_finite() {
    let run = || {
        let mut s = TestSynth::new(44_100.0);
        let mut all = Vec::new();
        for b in 0..60 {
            let events = match b {
                0 => vec![on(3, 60, 1), on(40, 64, 2)],
                20 => vec![ev(5, Event::NoteOff { key: 60, id: 1 })],
                30 => vec![ev(0, Event::PitchBend(-0.7))],
                _ => vec![],
            };
            all.push(run_synth(&mut s, &events));
        }
        all
    };
    let a = run();
    assert_eq!(a, run());
    assert!(
        a.iter()
            .all(|b| b.iter().all(|c| c.iter().all(|s| s.is_finite())))
    );
    assert!(a.iter().any(|b| peak(&b[0]) > 0.01));
}

#[test]
fn the_synth_state_round_trips() {
    let mut s = TestSynth::new(48_000.0);
    s.set_param(TestSynth::WAVE, 1.0);
    s.set_param(TestSynth::LEVEL, 0.25);
    let state = s.save_state();
    let mut other = TestSynth::new(48_000.0);
    assert!(other.load_state(&state));
    assert_eq!(other.param(TestSynth::WAVE), 1.0);
    assert_eq!(other.param(TestSynth::LEVEL), 0.25);
    assert!(!other.load_state(&[1, 2, 3]));
}

// ----- note shift ----------------------------------------------------------------------------

#[test]
fn the_note_effect_transposes_and_the_off_follows_its_note_even_after_a_param_change() {
    let mut n = NoteShift::new();
    n.set_param(NoteShift::SEMITONES, 5.0);
    let mut out = Vec::with_capacity(16);
    n.process(&[on(2, 60, 11)], &mut out);
    assert_eq!(
        out,
        vec![ev(
            2,
            Event::NoteOn {
                key: 65,
                velocity: 1.0,
                id: 11
            }
        )]
    );
    n.set_param(NoteShift::SEMITONES, -7.0);
    out.clear();
    n.process(&[ev(4, Event::NoteOff { key: 60, id: 11 })], &mut out);
    assert_eq!(out, vec![ev(4, Event::NoteOff { key: 65, id: 11 })]);
    // The next note uses the new shift, clamped to the key range.
    out.clear();
    n.process(&[on(0, 3, 12)], &mut out);
    assert_eq!(
        out,
        vec![ev(
            0,
            Event::NoteOn {
                key: 0,
                velocity: 1.0,
                id: 12
            }
        )]
    );
}

#[test]
fn the_note_effect_ignores_events_it_cannot_shift_and_does_not_allocate_beyond_capacity() {
    let mut n = NoteShift::new();
    let mut out = Vec::with_capacity(2);
    let events = [
        on(0, 60, 1),
        on(1, 61, 2),
        on(2, 62, 3),
        ev(3, Event::PitchBend(0.5)),
    ];
    n.process(&events, &mut out);
    assert_eq!(
        out.len(),
        2,
        "events beyond the capacity are dropped, not grown"
    );
    assert_eq!(out.capacity(), 2);
}

#[test]
fn the_note_effect_params_clamp_ignore_junk_and_reset_forgets_open_notes() {
    let mut n = NoteShift::new();
    assert_eq!(n.param(NoteShift::SEMITONES), 0.0);
    n.set_param(NoteShift::SEMITONES, f64::NAN);
    n.set_param(999, 3.0);
    assert_eq!(
        n.param(NoteShift::SEMITONES),
        0.0,
        "NaN and unknown ids are ignored"
    );
    assert_eq!(n.param(999), 0.0, "an unknown param reads as 0");
    n.set_param(NoteShift::SEMITONES, 99.0);
    assert_eq!(n.param(NoteShift::SEMITONES), 24.0);
    n.set_param(NoteShift::SEMITONES, -99.0);
    assert_eq!(n.param(NoteShift::SEMITONES), -24.0);
    n.set_param(NoteShift::SEMITONES, 2.0);
    // A second On of an open key closes the first (at the same offset, under the new id).
    let mut out = Vec::with_capacity(8);
    n.process(&[on(0, 60, 1), on(0, 60, 2)], &mut out);
    assert_eq!(
        out,
        vec![
            ev(
                0,
                Event::NoteOn {
                    key: 62,
                    velocity: 1.0,
                    id: 1
                }
            ),
            ev(0, Event::NoteOff { key: 62, id: 2 }),
            ev(
                0,
                Event::NoteOn {
                    key: 62,
                    velocity: 1.0,
                    id: 2
                }
            ),
        ]
    );
    // After a reset the Off of a forgotten note produces nothing.
    n.reset();
    out.clear();
    n.process(&[ev(1, Event::NoteOff { key: 60, id: 2 })], &mut out);
    assert!(out.is_empty());
}

// ----- param bank ----------------------------------------------------------------------------

#[test]
fn the_param_bank_has_two_thousand_params_and_one_that_appears() {
    let mut b = ParamBank::new();
    assert_eq!(b.count(), 2_000);
    b.set(1_999, 0.75);
    assert_eq!(b.get(1_999), 0.75);
    b.set(1_999, 7.0);
    assert_eq!(b.get(1_999), 1.0, "values are clamped to 0..=1");
    assert_eq!(b.get(5_000), 0.0, "an unknown id reads 0");
    assert!(!b.extra_present());
    b.add_extra();
    assert!(b.extra_present());
    assert_eq!(b.count(), 2_001);
    b.set(ParamBank::EXTRA_ID, 0.5);
    assert_eq!(b.get(ParamBank::EXTRA_ID), 0.5);
}

// ----- the normalised mapping of the VST3 shell ------------------------------------------------

#[test]
fn the_normalised_mapping_is_linear_clamped_and_inverts_on_exact_values() {
    assert_eq!(plain_from_normalized(-60.0, 12.0, 0.0), -60.0);
    assert_eq!(plain_from_normalized(-60.0, 12.0, 1.0), 12.0);
    assert_eq!(plain_from_normalized(-60.0, 12.0, 0.5), -24.0);
    assert_eq!(plain_from_normalized(-60.0, 12.0, 7.0), 12.0, "clamped");
    assert_eq!(normalized_from_plain(-60.0, 12.0, -24.0), 0.5);
    assert_eq!(normalized_from_plain(-60.0, 12.0, -99.0), 0.0, "clamped");
    assert_eq!(normalized_from_plain(3.0, 3.0, 3.0), 0.0, "an empty range");
    // A stepped param is an index: `steps` divisions of the range.
    assert_eq!(normalized_from_plain(0.0, 512.0, 37.0), 37.0 / 512.0);
    assert_eq!(
        plain_from_normalized(0.0, 512.0, 37.0 / 512.0).round(),
        37.0
    );
}
