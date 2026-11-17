use super::*;
use atomik_tab::grade::{PassGrade, Verdict};
use std::f64::consts::TAU;

pub(crate) const SR: u32 = 48_000;
/// Frames between two frames of the app (about 16.7 ms).
pub(crate) const STEP: usize = 800;
/// 120 bpm: two beats a second, 15360 ticks a beat.
pub(crate) const TPF_FULL: f64 = 2.0 * 15_360.0 / 48_000.0;
pub(crate) const BAR: i64 = 4 * 15_360;

fn hz_of(key: i32) -> f64 {
    440.0 * ((f64::from(key) - 69.0) / 12.0).exp2()
}

/// A plucked-string stand-in: fast decaying partials after a 1 ms attack.
pub(crate) fn pluck(out: &mut [f32], start: usize, key: i32) {
    let hz = hz_of(key);
    for (i, o) in out.iter_mut().enumerate().skip(start) {
        let t = (i - start) as f64 / f64::from(SR);
        let attack = (t / 0.001).min(1.0);
        let s = 0.8 * (-9.0 * t).exp() * (TAU * hz * t).sin()
            + 1.0 * (-12.0 * t).exp() * (TAU * hz * 2.0 * t).sin()
            + 0.5 * (-16.0 * t).exp() * (TAU * hz * 3.0 * t).sin();
        *o += (0.3 * attack * s) as f32;
    }
}

/// The frame a song tick is played at for a transport running at `tpf` ticks per frame.
pub(crate) fn frame_of(tick: f64, tpf: f64) -> usize {
    (tick / tpf).round() as usize
}

/// Frames of idle input before the transport starts (the tracker has been listening a while, as
/// it has when a player starts a run).
pub(crate) const LEAD: usize = 12 * STEP;

fn idle_view(frame: usize) -> TransportView {
    TransportView {
        playing: false,
        engine_frame: frame as u64,
        tick: 0,
        loop_count: 0,
        loop_range: Some((0, BAR)),
    }
}

/// Feeds [`LEAD`] frames of silence with the transport stopped.
fn lead_in(l: &mut Listening) {
    for f in (0..LEAD).step_by(STEP) {
        l.anchor(&idle_view(f), 100);
        l.samples(f as u64, &[0.0; STEP]);
    }
}

/// Plays `laps` laps of a one-bar loop: the transport states and the input `audio` are fed a
/// UI frame at a time, then the transport stops and the tracker gets time to finish. The
/// transport starts [`LEAD`] frames in; `audio` is indexed from its start.
fn drive(l: &mut Listening, audio: &[f32], tpf: f64, laps: u64, speed: u16) -> Vec<Located> {
    lead_in(l);
    let total = frame_of((laps * BAR as u64) as f64, tpf);
    let mut located = Vec::new();
    let mut f = 0usize;
    while f < total {
        let unwrapped = f as f64 * tpf;
        let lap = (unwrapped / BAR as f64).floor() as u64;
        let view = TransportView {
            playing: true,
            engine_frame: (f + LEAD) as u64,
            tick: (unwrapped - lap as f64 * BAR as f64).round() as i64,
            loop_count: lap,
            loop_range: Some((0, BAR)),
        };
        l.anchor(&view, speed);
        let end = (f + STEP).min(audio.len());
        located.extend(l.samples((f + LEAD) as u64, &audio[f.min(end)..end]));
        f += STEP;
    }
    // Stopped: a second of what the input still holds, then silence.
    let stop = TransportView {
        playing: false,
        engine_frame: (f + LEAD) as u64,
        tick: 0,
        loop_count: laps,
        loop_range: Some((0, BAR)),
    };
    for _ in 0..60 {
        l.anchor(&stop, speed);
        let end = (f + STEP).min(audio.len());
        let chunk: Vec<f32> = if f < end {
            audio[f..end].to_vec()
        } else {
            vec![0.0; STEP]
        };
        located.extend(l.samples((f + LEAD) as u64, &chunk));
        f += STEP;
    }
    located
}

fn signal(tpf: f64, laps: u64, notes: &[(i64, i32)], delay_frames: usize) -> Vec<f32> {
    let frames = frame_of((laps * BAR as u64) as f64, tpf) + SR as usize;
    let mut x = vec![0.0f32; frames];
    for lap in 0..laps as i64 {
        for (tick, key) in notes {
            let at = frame_of((lap * BAR + tick) as f64, tpf) + delay_frames;
            pluck(&mut x, at, *key);
        }
    }
    x
}

/// Four quarter notes: E2 A2 D3 G3.
pub(crate) const RIFF: [(i64, i32); 4] = [(0, 40), (15_360, 45), (30_720, 50), (46_080, 55)];

fn expected_notes() -> Vec<(i64, u8)> {
    RIFF.iter().map(|(t, k)| (*t, *k as u8)).collect()
}

fn graded(l: &mut Listening) -> Vec<(ClosedPass, Option<PassGrade>)> {
    let notes = expected_notes();
    l.take_closed()
        .into_iter()
        .map(|p| {
            let g = grade_closed(&p, &notes, 80.0).map(|(g, _)| g);
            (p, g)
        })
        .collect()
}

fn verdicts(g: &PassGrade) -> Vec<Verdict> {
    g.notes.iter().map(|n| n.verdict).collect()
}

#[test]
fn a_clean_run_grades_every_lap_a_hit() {
    let mut l = Listening::new(SR);
    let x = signal(TPF_FULL, 3, &RIFF, 0);
    drive(&mut l, &x, TPF_FULL, 3, 100);
    let passes = graded(&mut l);
    assert_eq!(passes.len(), 3, "{passes:?}");
    for (p, g) in &passes {
        let g = g.as_ref().expect("a grade");
        assert_eq!(verdicts(g), [Verdict::Hit; 4], "{:?} {p:?}", p.key);
        assert_eq!(g.extras, 0);
        assert_eq!(g.percent(), 100);
    }
    // Laps 0 and 1 ran to the loop end; the last one stopped.
    assert!(passes[0].0.wrapped && passes[1].0.wrapped && !passes[2].0.wrapped);
}

#[test]
fn a_pass_is_not_ready_until_the_tracker_has_caught_up() {
    let mut l = Listening::new(SR);
    lead_in(&mut l);
    let x = signal(TPF_FULL, 1, &RIFF, 0);
    // One lap, then the next lap starts: the first is over but the last note is still in the
    // tracker.
    let total = frame_of(BAR as f64, TPF_FULL);
    let mut f = 0;
    while f < total + STEP {
        let unwrapped = f as f64 * TPF_FULL;
        let lap = (unwrapped / BAR as f64).floor() as u64;
        l.anchor(
            &TransportView {
                playing: true,
                engine_frame: (f + LEAD) as u64,
                tick: (unwrapped - lap as f64 * BAR as f64) as i64,
                loop_count: lap,
                loop_range: Some((0, BAR)),
            },
            100,
        );
        let end = (f + STEP).min(x.len());
        l.samples((f + LEAD) as u64, &x[f..end]);
        f += STEP;
    }
    assert!(l.take_closed().is_empty(), "the last note is not out yet");
    // Silence goes on: now it is.
    let mut more = (f + LEAD) as u64;
    for _ in 0..40 {
        l.anchor(
            &TransportView {
                playing: true,
                engine_frame: more,
                tick: 100,
                loop_count: 1,
                loop_range: Some((0, BAR)),
            },
            100,
        );
        l.samples(more, &[0.0; STEP]);
        more += STEP as u64;
    }
    let closed = l.take_closed();
    assert_eq!(closed.len(), 1);
    assert!(closed[0].wrapped);
}

#[test]
fn latency_compensation_moves_the_notes_back_onto_the_beat() {
    // The input is 50 ms behind (2400 frames).
    let delay = 2_400;
    let x = signal(TPF_FULL, 2, &RIFF, delay);
    let mut raw = Listening::new(SR);
    drive(&mut raw, &x, TPF_FULL, 2, 100);
    let late = graded(&mut raw);
    assert!(
        late[0].1.as_ref().unwrap().tally().late >= 3,
        "{:?}",
        late[0].1
    );
    // The sink's compensation takes it off.
    let mut by_sink = Listening::new(SR);
    by_sink.set_compensation(delay as i64, 0);
    drive(&mut by_sink, &x, TPF_FULL, 2, 100);
    assert_eq!(graded(&mut by_sink)[0].1.as_ref().unwrap().tally().hit, 4);
    // So does the manual offset (positive places notes earlier).
    let mut by_hand = Listening::new(SR);
    by_hand.set_compensation(0, 50);
    drive(&mut by_hand, &x, TPF_FULL, 2, 100);
    assert_eq!(graded(&mut by_hand)[0].1.as_ref().unwrap().tally().hit, 4);
    // A negative offset the wrong way makes it worse.
    let mut wrong_way = Listening::new(SR);
    wrong_way.set_compensation(0, -50);
    drive(&mut wrong_way, &x, TPF_FULL, 2, 100);
    assert!(graded(&mut wrong_way)[0].1.as_ref().unwrap().tally().hit < 4);
}

#[test]
fn half_speed_follows_the_slowed_ticks() {
    // At 50 % the song runs at half the ticks per frame; the player plays in step with it.
    let tpf = TPF_FULL / 2.0;
    let x = signal(tpf, 2, &RIFF, 0);
    let mut l = Listening::new(SR);
    drive(&mut l, &x, tpf, 2, 50);
    let passes = graded(&mut l);
    assert_eq!(passes.len(), 2);
    for (p, g) in &passes {
        assert_eq!(p.speed, 50);
        // Ticks per millisecond of wall-clock time: half of the full-speed rate.
        assert!(
            (p.ticks_per_ms - tpf * 48.0).abs() < 0.2,
            "{}",
            p.ticks_per_ms
        );
        assert_eq!(g.as_ref().unwrap().tally().hit, 4);
    }
}

#[test]
fn a_missed_a_wrong_and_an_extra_note_are_told_apart() {
    // The second note is not played, the third is a wrong pitch, an extra note comes between
    // the third and the fourth.
    let mut x = vec![0.0f32; frame_of(BAR as f64, TPF_FULL) + SR as usize];
    pluck(&mut x, frame_of(0.0, TPF_FULL), 40);
    pluck(&mut x, frame_of(30_720.0, TPF_FULL), 47);
    pluck(&mut x, frame_of(38_400.0, TPF_FULL), 52);
    pluck(&mut x, frame_of(46_080.0, TPF_FULL), 55);
    let mut l = Listening::new(SR);
    drive(&mut l, &x, TPF_FULL, 1, 100);
    let passes = graded(&mut l);
    let g = passes[0].1.as_ref().unwrap();
    assert_eq!(
        verdicts(g),
        [Verdict::Hit, Verdict::Missed, Verdict::Wrong, Verdict::Hit]
    );
    assert_eq!(g.extras, 1);
    // Two points of four and one extra.
    assert_eq!(g.percent(), 40);
}

#[test]
fn a_run_that_starts_mid_loop_grades_only_what_it_reached() {
    // The transport starts at the third beat and is stopped at the end of the loop's bar.
    let mut l = Listening::new(SR);
    lead_in(&mut l);
    let mut x = vec![0.0f32; SR as usize * 4];
    let start_tick = 30_720.0;
    for (tick, key) in &RIFF[2..] {
        pluck(&mut x, frame_of(*tick as f64 - start_tick, TPF_FULL), *key);
    }
    let frames = frame_of(BAR as f64 - start_tick, TPF_FULL);
    let mut f = 0;
    while f < frames {
        l.anchor(
            &TransportView {
                playing: true,
                engine_frame: (f + LEAD) as u64,
                tick: (start_tick + f as f64 * TPF_FULL) as i64,
                loop_count: 0,
                loop_range: Some((0, BAR)),
            },
            100,
        );
        l.samples((f + LEAD) as u64, &x[f..f + STEP]);
        f += STEP;
    }
    for _ in 0..60 {
        l.anchor(
            &TransportView {
                playing: false,
                engine_frame: (f + LEAD) as u64,
                tick: BAR,
                loop_count: 0,
                loop_range: Some((0, BAR)),
            },
            100,
        );
        l.samples((f + LEAD) as u64, &x[f..f + STEP]);
        f += STEP;
    }
    let passes = graded(&mut l);
    assert_eq!(passes.len(), 1);
    let g = passes[0].1.as_ref().unwrap();
    assert_eq!(g.notes.len(), 2, "only the notes from the start: {g:?}");
    assert_eq!(g.tally().hit, 2);
}

#[test]
fn a_new_start_is_a_new_run_and_an_engine_restart_forgets() {
    let mut l = Listening::new(SR);
    let play = |l: &mut Listening, frame: u64, playing: bool| {
        l.anchor(
            &TransportView {
                playing,
                engine_frame: frame,
                tick: 0,
                loop_count: 0,
                loop_range: None,
            },
            100,
        );
    };
    play(&mut l, 1_000, true);
    play(&mut l, 2_000, false);
    play(&mut l, 3_000, true);
    assert_eq!(l.clock.last().unwrap().key.run, 2);
    // The frame counter went back: the engine restarted.
    play(&mut l, 100, true);
    assert_eq!(l.clock.last().unwrap().frame, 100);
    assert_eq!(l.passes.len(), 1);
}

#[test]
fn the_clock_interpolates_extrapolates_and_wraps() {
    let mut c = Clock::new(SR);
    let k0 = PassKey { run: 1, lap: 0 };
    let k1 = PassKey { run: 1, lap: 1 };
    let range = Some((0, 1000));
    for (frame, tick, key) in [(0, 0, k0), (100, 500, k0), (200, 990, k0), (400, 100, k1)] {
        c.push(Anchor {
            frame,
            tick,
            key,
            loop_range: range,
        });
    }
    // Between two anchors of one lap.
    let (k, t, rate) = c.locate(50.0).unwrap();
    assert_eq!((k, t), (k0, 250.0));
    assert!((rate - 5.0).abs() < 1e-9);
    // Past the lap's last anchor and before the next lap's: it wraps into the next lap.
    let (k, t, _) = c.locate(300.0).unwrap();
    assert_eq!(k, k1, "{t}");
    assert!((t - (990.0 + 100.0 * 4.9 - 1000.0)).abs() < 1.0, "{t}");
    // Just before the lap's end it stays in the lap.
    let (k, t, _) = c.locate(202.0).unwrap();
    assert_eq!(k, k0);
    assert!(t > 990.0 && t < 1000.0);
    // Past the newest anchor by a little: extrapolated, by a lot: nothing.
    assert!(c.locate(1_000.0).is_some());
    assert!(c.locate(400.0 + 2.0 * f64::from(SR)).is_none());
    // Before the first by a little: allowed; by a lot: not.
    assert!(c.locate(-100.0).is_some());
    assert!(c.locate(-0.2 * f64::from(SR)).is_none());
    // A repeated frame is ignored.
    let before = c.anchors.len();
    c.push(Anchor {
        frame: 400,
        tick: 5,
        key: k1,
        loop_range: range,
    });
    assert_eq!(c.anchors.len(), before);
    assert!(Clock::new(SR).locate(1.0).is_none());
}

#[test]
fn the_judge_asks_for_the_threshold_and_lets_ungraded_passes_through() {
    let mut j = AccuracyJudge::new(90);
    let report = |accuracy| PassReport {
        index: 1,
        speed: 80,
        accuracy,
    };
    assert!(j.is_clean(&report(Some(1.0))));
    assert!(j.is_clean(&report(Some(0.90))));
    assert!(j.is_clean(&report(Some(0.896))), "rounds like the bar");
    assert!(!j.is_clean(&report(Some(0.89))));
    assert!(
        j.is_clean(&report(None)),
        "nothing graded: counts as before"
    );
    let mut strict = AccuracyJudge::new(100);
    assert!(!strict.is_clean(&report(Some(0.95))));
    assert_eq!(AccuracyJudge::new(10).threshold, 0.5);
    assert_eq!(AccuracyJudge::new(200).threshold, 1.0);
}

#[test]
fn a_lap_with_no_notes_has_nothing_to_grade() {
    let l = ClosedPass {
        key: PassKey { run: 1, lap: 1 },
        first_tick: 0,
        last_tick: 1000,
        loop_range: Some((0, 1000)),
        speed: 100,
        wrapped: true,
        ticks_per_ms: 0.5,
        events: Vec::new(),
    };
    assert!(grade_closed(&l, &[(5_000, 40)], 80.0).is_none());
    // The same pass with a note inside it and nothing heard: missed.
    let (g, _) = grade_closed(&l, &[(500, 40)], 80.0).unwrap();
    assert_eq!(verdicts(&g), [Verdict::Missed]);
}

#[test]
fn a_note_a_little_before_the_loop_end_counts_for_the_next_lap() {
    // From the second lap on the first note comes 20 ms early: just before the wrap. It is still
    // the loop's first note, and the lap it fell in does not count it as an extra.
    let laps = 3;
    let mut x = vec![0.0f32; frame_of((laps * BAR as u64) as f64, TPF_FULL) + SR as usize];
    for lap in 0..laps as i64 {
        for (tick, key) in &RIFF {
            let mut at = frame_of((lap * BAR + tick) as f64, TPF_FULL);
            if lap > 0 && *tick == 0 {
                at -= 960;
            }
            pluck(&mut x, at, *key);
        }
    }
    let mut l = Listening::new(SR);
    drive(&mut l, &x, TPF_FULL, laps, 100);
    let passes = graded(&mut l);
    assert_eq!(passes.len(), 3);
    for (p, g) in &passes {
        let g = g.as_ref().unwrap();
        assert_eq!(g.extras, 0, "{:?}", p.key);
        assert_eq!(g.tally().missed, 0, "{:?}: {g:?}", p.key);
        assert_eq!(g.notes[0].verdict, Verdict::Hit);
    }
}
