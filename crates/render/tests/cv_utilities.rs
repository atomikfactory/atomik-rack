//! The CV utilities in a patch bounce: an envelope follower, CV math and a gain tool duck a synth
//! phrase by its own level, and a sample-and-hold, quantizer, CV math and slew chain moves the
//! level of a held note in steps. Both render the same bits at every host block size, and the
//! allocation guard wraps every render.
atomik_testkit::install_alloc_guard!();

use atomik_engine::{OfflineJob, render_offline_with};
use atomik_render::build::{BuildOptions, build_with};
use atomik_render::default_registry;
use atomik_render::patch::Patch;
use atomik_testkit::no_alloc::assert_no_alloc;

const SYNTH: &str = r#"{"id": "s", "type": "atomik.poly-synth",
    "params": {"osc1.wave": 1, "osc.mix": 0.0, "voices": 1, "filter.cutoff": 12000,
               "env.amp.attack": 2, "env.amp.decay": 200, "env.amp.sustain": 1.0,
               "env.amp.release": 80}}"#;

/// One long synth note (so the level is steady and only the CV moves it) through `devices` and
/// `cables`; the last audio stage must feed `interface:out_1/2` through the cables given.
fn patch_json(devices: &[&str], cables: &[&str], seed: u64) -> String {
    let mut all = vec![SYNTH];
    all.extend_from_slice(devices);
    format!(
        r#"{{"format": "atomik-patch", "version": 1, "sample_rate": 48000, "tempo_bpm": 120,
            "seed": {seed}, "length_beats": 8, "output_channels": 2,
            "devices": [{}], "cables": [{}],
            "notes": [{{"device": "s", "socket": "notes_in", "at_beats": 0, "length_beats": 8,
                        "key": 52, "velocity": 0.9}}]}}"#,
        all.join(","),
        cables.join(",")
    )
}

fn render(json: &str, host_block: usize) -> Vec<Vec<f32>> {
    let patch = Patch::from_json(json).unwrap_or_else(|e| panic!("{e}\n{json}"));
    let built = build_with(&patch, &default_registry(), &BuildOptions::default()).unwrap();
    let job = OfflineJob {
        frames: built.frames,
        host_block,
        notes: &built.notes,
        params: &[],
        inputs: &[],
    };
    render_offline_with(built.plan, built.config, &job, |r| assert_no_alloc(r))
}

/// The peak of `x[from..to]`.
fn peak(x: &[f32], from: usize, to: usize) -> f32 {
    x[from..to].iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

fn same_bits(a: &[Vec<f32>], b: &[Vec<f32>]) -> bool {
    a.iter()
        .zip(b)
        .all(|(x, y)| x.iter().zip(y).all(|(p, q)| p.to_bits() == q.to_bits()))
}

/// The synth through an envelope follower (which passes it on); its CV, inverted by CV math,
/// turns a gain tool down (24 dB per unit of CV at full trim).
fn ducking_json(duck: bool) -> String {
    let follower = r#"{"id": "f", "type": "atomik.envelope-follower",
        "params": {"mode": 0, "attack": 20, "release": 200}}"#;
    let math = r#"{"id": "m", "type": "atomik.cv-math", "params": {"scale": -1}}"#;
    let gain = r#"{"id": "g", "type": "atomik.gain-tool"}"#;
    let mut cables = vec![
        r#"{"from": "s:out_l", "to": "f:in_l"}"#,
        r#"{"from": "s:out_r", "to": "f:in_r"}"#,
        r#"{"from": "f:out_l", "to": "g:in_l"}"#,
        r#"{"from": "f:out_r", "to": "g:in_r"}"#,
        r#"{"from": "g:out_l", "to": "interface:out_1"}"#,
        r#"{"from": "g:out_r", "to": "interface:out_2"}"#,
    ];
    if duck {
        cables.push(r#"{"from": "f:cv_out", "to": "m:a_in"}"#);
        cables.push(r#"{"from": "m:out", "to": "g:gain_cv_in"}"#);
    }
    patch_json(&[follower, math, gain], &cables, 1)
}

#[test]
fn the_follower_ducks_the_phrase_by_its_own_level_after_the_attack() {
    let dry = render(&ducking_json(false), 64);
    let wet = render(&ducking_json(true), 64);
    assert!(dry.iter().flatten().all(|x| x.is_finite()));
    assert!(wet.iter().flatten().all(|x| x.is_finite()));
    let n = dry[0].len();
    // The first milliseconds are untouched (the follower has not risen yet), the tail is quieter.
    let early = peak(&dry[0], 0, 48);
    assert!(
        (peak(&wet[0], 0, 48) - early).abs() < early * 0.2,
        "the start is close to the dry one"
    );
    let (d, w) = (
        peak(&dry[0], n / 2, n - 4800),
        peak(&wet[0], n / 2, n - 4800),
    );
    assert!(d > 0.1, "the dry phrase sounds ({d})");
    assert!(w < d * 0.6, "the follower ducked it: {w} against {d}");
    assert!(w > d * 0.02, "and did not mute it: {w} against {d}");
    for block in [333, 1024] {
        assert!(
            same_bits(&wet, &render(&ducking_json(true), block)),
            "block {block} differs"
        );
    }
}

/// Seeded noise held eight times a second, quantized to a pentatonic scale, scaled down, slewed
/// (or not) and used as the gain of the synth.
fn stepped_json(scale: u32, slew_ms: f32) -> String {
    let hold = r#"{"id": "h", "type": "atomik.sample-hold",
        "params": {"clock": 1, "rate": 8}}"#;
    let quant = format!(
        r#"{{"id": "q", "type": "atomik.cv-quantizer", "params": {{"scale": {scale}, "root": 0}}}}"#
    );
    let math = r#"{"id": "m", "type": "atomik.cv-math", "params": {"scale": 0.5}}"#;
    let slew = format!(
        r#"{{"id": "w", "type": "atomik.slew-limiter", "params": {{"rise": {slew_ms}, "fall": {slew_ms}}}}}"#
    );
    let gain = r#"{"id": "g", "type": "atomik.gain-tool"}"#;
    let cables = [
        r#"{"from": "s:out_l", "to": "g:in_l"}"#,
        r#"{"from": "s:out_r", "to": "g:in_r"}"#,
        r#"{"from": "g:out_l", "to": "interface:out_1"}"#,
        r#"{"from": "g:out_r", "to": "interface:out_2"}"#,
        r#"{"from": "h:cv_out", "to": "q:pitch_cv_in"}"#,
        r#"{"from": "q:pitch_cv_out", "to": "m:a_in"}"#,
        r#"{"from": "m:out", "to": "w:cv_in"}"#,
        r#"{"from": "w:cv_out", "to": "g:gain_cv_in"}"#,
    ];
    patch_json(&[hold, &quant, math, &slew, gain], &cables, 5)
}

#[test]
fn a_held_quantized_cv_steps_the_level_and_a_slew_smooths_the_steps() {
    // The synth is one steady note, so the level of each eighth of a second follows the CV.
    let hard = render(&stepped_json(10, 0.1), 64);
    let smooth = render(&stepped_json(10, 80.0), 64);
    let chromatic = render(&stepped_json(0, 0.1), 64);
    for r in [&hard, &smooth, &chromatic] {
        assert!(r.iter().flatten().all(|x| x.is_finite()));
        assert!(peak(&r[0], 0, r[0].len()) > 0.01, "silent");
    }
    // Peak of each 1/8 s step (6000 samples), away from the edges.
    let steps = |r: &[Vec<f32>]| -> Vec<f32> {
        (0..8)
            .map(|k| peak(&r[0], k * 6000 + 2000, k * 6000 + 5000))
            .collect()
    };
    let h = steps(&hard);
    assert!(
        h.iter().any(|v| (v - h[0]).abs() > h[0] * 0.05),
        "the level moves: {h:?}"
    );
    // Quantizing to a scale changes the held pitches, so the steps differ from the chromatic ones.
    let c = steps(&chromatic);
    assert_ne!(
        h.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        c.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
    );
    // Inside a step the unslewed level is constant; the slewed one is still moving.
    let inside = |r: &[Vec<f32>], k: usize| {
        let a = peak(&r[0], k * 6000 + 300, k * 6000 + 600);
        let b = peak(&r[0], k * 6000 + 5400, k * 6000 + 5700);
        (a - b).abs() / a.max(b)
    };
    let moved = (1..8).map(|k| inside(&smooth, k)).fold(0.0f32, f32::max);
    let flat = (1..8).map(|k| inside(&hard, k)).fold(0.0f32, f32::max);
    assert!(
        moved > flat,
        "slewed steps glide ({moved}) where hard ones do not ({flat})"
    );
    for block in [333, 1024] {
        assert!(
            same_bits(&smooth, &render(&stepped_json(10, 80.0), block)),
            "block {block} differs"
        );
    }
}
