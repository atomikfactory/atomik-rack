use super::*;

const SR: f64 = 48_000.0;

fn pt(tick: i64, bpm: f64, ramp: bool) -> TempoPoint {
    TempoPoint { tick, bpm, ramp }
}

fn table(points: &[TempoPoint]) -> TempoTable {
    TempoTable::new(points).unwrap()
}

#[test]
fn compile_tempo_rejects_bad_maps() {
    assert_eq!(compile_tempo(&[]), Err(MapError::TempoStart));
    assert_eq!(
        compile_tempo(&[pt(5, 120.0, false)]),
        Err(MapError::TempoStart)
    );
    assert_eq!(
        compile_tempo(&[pt(0, 120.0, false), pt(0, 90.0, false)]),
        Err(MapError::TempoOrder)
    );
    assert_eq!(
        compile_tempo(&[pt(0, 19.0, false)]),
        Err(MapError::TempoRange(19.0))
    );
    assert!(compile_tempo(&[pt(0, f64::NAN, false)]).is_err());
}

#[test]
fn step_maps_keep_the_original_bits() {
    let bar = 4 * PPQ;
    let t = table(&[
        pt(0, 120.0, false),
        pt(bar, 97.0, false),
        pt(3 * bar, 143.0, false),
    ]);
    // The step-only maths this module replaced: segment start times accumulated once.
    let old = |tick: i64| -> f64 {
        let seg = |t0: i64, s0: f64, b: f64| s0 + (((tick - t0) as f64) * 60.0) / (b * PPQ as f64);
        let s1 = (bar as f64 * 60.0) / (120.0 * PPQ as f64);
        let s2 = s1 + ((2 * bar) as f64 * 60.0) / (97.0 * PPQ as f64);
        if tick >= 3 * bar {
            seg(3 * bar, s2, 143.0)
        } else if tick >= bar {
            seg(bar, s1, 97.0)
        } else {
            seg(0, 0.0, 120.0)
        }
    };
    for tick in [0, 1, 777, bar - 1, bar, bar + 12_345, 3 * bar, 9 * bar + 5] {
        assert_eq!(t.map().seconds(tick as f64), old(tick), "tick {tick}");
    }
}

/// Midpoint-rule integration of the seconds per tick over `[0, to]` of a ramp.
fn integrate(b0: f64, b1: f64, len: i64, to: i64) -> f64 {
    let n = 200_000;
    let h = to as f64 / n as f64;
    (0..n)
        .map(|i| {
            let x = (i as f64 + 0.5) * h;
            let bpm = b0 + (b1 - b0) * x / len as f64;
            h * 60.0 / (bpm * PPQ as f64)
        })
        .sum()
}

#[test]
fn a_ramp_matches_numeric_integration_of_the_tempo() {
    let bar = 4 * PPQ;
    let t = table(&[pt(0, 60.0, true), pt(2 * bar, 180.0, false)]);
    for tick in [1, PPQ, bar, bar + 777, 2 * bar - 1, 2 * bar] {
        let exact = integrate(60.0, 180.0, 2 * bar, tick);
        let got = t.map().seconds(tick as f64);
        assert!((got - exact).abs() < 1e-9, "tick {tick}: {got} vs {exact}");
    }
    // After the ramp the target tempo holds.
    let end = t.map().seconds((2 * bar) as f64);
    let later = t.map().seconds((2 * bar + PPQ) as f64);
    assert!((later - end - 60.0 / 180.0).abs() < 1e-12);
    // Inside the ramp the tempo is the instantaneous one.
    assert!((t.map().bpm(bar as f64) - 120.0).abs() < 1e-12);
    assert_eq!(t.map().bpm(0.0), 60.0);
    assert_eq!(t.map().bpm((3 * bar) as f64), 180.0);
}

#[test]
fn a_descending_ramp_in_the_middle_follows_the_formula() {
    let bar = 4 * PPQ;
    let t = table(&[
        pt(0, 100.0, false),
        pt(bar, 200.0, true),
        pt(3 * bar, 50.0, false),
    ]);
    let start = (bar as f64 * 60.0) / (100.0 * PPQ as f64);
    let exact = start + integrate(200.0, 50.0, 2 * bar, bar);
    let got = t.map().seconds((2 * bar) as f64);
    assert!((got - exact).abs() < 1e-9, "{got} vs {exact}");
    assert!((t.map().bpm((2 * bar) as f64) - 125.0).abs() < 1e-12);
}

#[test]
fn frames_and_ticks_invert_each_other_over_ramps() {
    let bar = 4 * PPQ;
    let t = table(&[
        pt(0, 120.0, false),
        pt(bar, 40.0, true),
        pt(5 * bar, 300.0, true),
        pt(9 * bar, 90.0, false),
    ]);
    let m = t.map();
    for tick in (0..12 * bar).step_by(997) {
        let f = m.frame(tick as f64, SR);
        let back = m.tick(f, SR);
        assert!((back - tick as f64).abs() < 1e-6, "tick {tick}: {back}");
    }
    // The frame strictly increases with the tick.
    let mut last = -1.0;
    for tick in (0..12 * bar).step_by(4093) {
        let f = m.frame(tick as f64, SR);
        assert!(f > last);
        last = f;
    }
}

#[test]
fn an_equal_bpm_ramp_is_a_step() {
    let bar = 4 * PPQ;
    let ramp = table(&[pt(0, 120.0, true), pt(bar, 120.0, false)]);
    let step = table(&[pt(0, 120.0, false), pt(bar, 120.0, false)]);
    for tick in [1, PPQ, bar - 1, bar, 2 * bar] {
        assert_eq!(
            ramp.map().seconds(tick as f64),
            step.map().seconds(tick as f64)
        );
    }
    assert_eq!(ramp.map().tick(96_000.0, SR), 4.0 * PPQ as f64);
}

#[test]
fn a_nearly_equal_ramp_is_continuous() {
    let bar = 4 * PPQ;
    let a = table(&[pt(0, 120.0, true), pt(bar, 120.000_000_000_1, false)]);
    let b = table(&[pt(0, 120.0, false)]);
    let half = bar as f64 / 2.0;
    let d = (a.map().seconds(half) - b.map().seconds(half)).abs();
    assert!(d < 1e-12, "{d}");
}

#[test]
fn ticks_before_zero_extend_the_first_tempo() {
    let t = table(&[pt(0, 120.0, true), pt(PPQ, 240.0, false)]);
    assert_eq!(t.map().frame(-(PPQ as f64), SR), -24_000.0);
    assert_eq!(t.map().tick(-24_000.0, SR), -(PPQ as f64));
    assert_eq!(t.map().song_frame(-5, SR), 0);
}

#[test]
fn ticks_per_frame_follows_the_tempo_in_a_ramp() {
    let t = table(&[pt(0, 60.0, true), pt(PPQ, 120.0, false)]);
    let mid = t.map().ticks_per_frame(PPQ as f64 / 2.0, SR);
    assert!((mid - 90.0 * PPQ as f64 / (60.0 * SR)).abs() < 1e-12);
}

#[test]
fn a_constant_table_is_one_segment() {
    let t = TempoTable::constant(Tempo::new(120.0).unwrap());
    assert_eq!(t.segments().len(), 1);
    assert_eq!(t.map().song_frame(PPQ, SR), 24_000);
}
