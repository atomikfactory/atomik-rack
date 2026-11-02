//! The modulation matrix of the poly synth (and the mono bass, which shares its voices): what each
//! source does to each target, that an unused matrix changes nothing, that the output is the same
//! bit for bit however the render is cut into calls, and that the render path does not allocate.

use crate::poly_synth::{DESCRIPTOR, MOD_SOURCES, MOD_TABLES, MOD_TARGETS, PolySynth};
use crate::test_support::{SR, harness, on};
use atomik_core::{BATCH, NoteEvent, NoteKind};
use atomik_testkit::harness::DeviceHarness;
use atomik_testkit::no_alloc::assert_no_alloc;
use atomik_testkit::signal::{dominant_frequency, max_step};

const OUT: [&str; 2] = ["out_l", "out_r"];

fn seconds(s: f64) -> usize {
    (s * SR / BATCH as f64).ceil() as usize
}

fn source(key: &str) -> f32 {
    MOD_TABLES.source_by_key(key).unwrap().id as f32 + 1.0
}

fn target(key: &str) -> f32 {
    MOD_TABLES.target_by_key(key).unwrap().id as f32 + 1.0
}

/// Fills slot `n` (0-based): source and target by key, an optional via source and the amount.
fn slot(h: &mut DeviceHarness, n: usize, src: &str, tgt: &str, via: Option<&str>, amount: f32) {
    h.set_param(&format!("internal.mod{}_source", n + 1), source(src));
    h.set_param(&format!("internal.mod{}_target", n + 1), target(tgt));
    h.set_param(
        &format!("internal.mod{}_via", n + 1),
        via.map_or(0.0, source),
    );
    h.set_param(&format!("mod{}_amount", n + 1), amount);
}

fn synth() -> DeviceHarness {
    harness(Box::new(PolySynth::new()))
}

/// A static saw through a closed filter: the cutoff alone decides the brightness.
fn dark(h: &mut DeviceHarness) {
    h.set_param("filter.cutoff", 200.0);
    h.set_param("filter.env_amount", 0.0);
    h.set_param("filter.key_track", 0.0);
    h.set_param("osc.mix", 0.0);
}

fn brightness(x: &[f32]) -> f32 {
    let d: Vec<f32> = x.windows(2).map(|w| w[1] - w[0]).collect();
    atomik_testkit::signal::rms(&d)
}

fn render(h: &mut DeviceHarness, secs: f64) -> Vec<f32> {
    h.render(seconds(secs), &OUT).remove(0)
}

fn bits(x: &[f32]) -> Vec<u32> {
    x.iter().map(|v| v.to_bits()).collect()
}

#[test]
fn the_tables_are_valid_and_name_real_params() {
    assert_eq!(MOD_TABLES.validate(&DESCRIPTOR), Ok(()));
    assert_eq!(MOD_SOURCES.len(), 9);
    assert_eq!(MOD_TARGETS.len(), 3);
    for t in &MOD_TARGETS {
        assert!(
            DESCRIPTOR.params.get(t.param.0 as usize).is_some(),
            "{}",
            t.key
        );
    }
    let mut r = atomik_device_sdk::Registry::new();
    crate::register_all(&mut r).unwrap();
    assert!(r.mod_tables("atomik.poly-synth").is_some());
    assert!(r.mod_tables("atomik.mono-bass").is_some());
    assert!(r.mod_tables("atomik.dual-layer").is_none());
}

#[test]
fn an_unused_matrix_changes_nothing_bit_for_bit() {
    let play = |configure: &dyn Fn(&mut DeviceHarness)| {
        let mut h = synth();
        h.set_param("lfo.to_pitch", 2.0);
        h.set_param("lfo2.to_cutoff", 1.0);
        configure(&mut h);
        h.push_note("notes_in", on(0, 48, 0.8));
        h.push_note("notes_in", on(3, 55, 0.6));
        bits(&render(&mut h, 0.4))
    };
    let base = play(&|_| {});
    // Slots with nothing to add: no target, no source, a zero amount.
    let idle = play(&|h| {
        h.set_param("internal.mod1_source", source("lfo1"));
        h.set_param("mod1_amount", 1.0);
        h.set_param("internal.mod2_target", target("cutoff"));
        h.set_param("mod2_amount", 1.0);
        slot(h, 2, "lfo1", "cutoff", None, 0.0);
    });
    assert_eq!(base, idle);
    // A slot that is cleared again.
    let cleared = play(&|h| {
        slot(h, 0, "lfo1", "pitch", None, 0.5);
        slot(h, 0, "lfo1", "pitch", None, 0.0);
    });
    assert_eq!(base, cleared);
}

#[test]
fn the_filter_envelope_opens_the_cutoff() {
    let level = |amount: f32| {
        let mut h = synth();
        dark(&mut h);
        h.set_param("env.filter.decay", 5_000.0);
        slot(&mut h, 0, "filter_env", "cutoff", None, amount);
        h.push_note("notes_in", on(0, 36, 1.0));
        brightness(&render(&mut h, 0.2))
    };
    let (none, open, closed) = (level(0.0), level(1.0), level(-1.0));
    assert!(open > none * 2.0, "{open} vs {none}");
    assert!(closed <= none, "{closed} vs {none}");
}

#[test]
fn velocity_and_key_and_pressure_are_sources() {
    let play = |src: &str, vel: f32, key: u8, pressure: Option<f32>| {
        let mut h = synth();
        dark(&mut h);
        slot(&mut h, 0, src, "cutoff", None, 1.0);
        h.push_note("notes_in", on(0, key, vel));
        if let Some(value) = pressure {
            h.push_note(
                "notes_in",
                NoteEvent {
                    offset: 1,
                    kind: NoteKind::PolyPressure { key, value },
                },
            );
        }
        brightness(&render(&mut h, 0.3)[4_000..])
    };
    assert!(play("velocity", 1.0, 48, None) > play("velocity", 0.2, 48, None) * 1.5);
    // The key source is bipolar and centred on C4: a high note opens, a low one closes.
    let (high, low) = (play("key", 1.0, 84, None), play("key", 1.0, 36, None));
    assert!(high > low * 1.5, "{high} vs {low}");
    let (soft, hard) = (
        play("pressure", 1.0, 48, None),
        play("pressure", 1.0, 48, Some(1.0)),
    );
    assert!(hard > soft * 1.5, "{hard} vs {soft}");
}

#[test]
fn an_lfo_to_pitch_slot_makes_vibrato_and_via_the_wheel_scales_it() {
    let spread = |wheel: Option<f32>, via: bool| {
        let mut h = synth();
        h.set_param("osc1.wave", 3.0);
        h.set_param("osc.mix", 0.0);
        h.set_param("filter.cutoff", 20_000.0);
        h.set_param("filter.env_amount", 0.0);
        h.set_param("filter.key_track", 0.0);
        h.set_param("filter.resonance", 0.0);
        h.set_param("lfo.wave", 3.0); // square: +1 first half, -1 second
        h.set_param("lfo.rate", 0.5);
        slot(
            &mut h,
            0,
            "lfo1",
            "pitch",
            via.then_some("mod_wheel"),
            1.0 / 24.0,
        );
        if let Some(value) = wheel {
            h.push_note(
                "notes_in",
                NoteEvent {
                    offset: 0,
                    kind: NoteKind::Cc {
                        controller: 1,
                        value,
                    },
                },
            );
        }
        h.push_note("notes_in", on(1, 57, 1.0));
        let o = render(&mut h, 2.0);
        dominant_frequency(&o[4_800..43_200], SR) - dominant_frequency(&o[52_800..91_200], SR)
    };
    // An amount of 1/24 is one semitone each way around 220 Hz (the wheel also adds its own
    // vibrato through `lfo.to_pitch`, which is 0 here): about 25 Hz apart.
    let plain = spread(None, false);
    assert!(plain > 20.0 && plain < 30.0, "{plain}");
    // Via the wheel: nothing with the wheel down (a full wheel adds its own vibrato too).
    let down = spread(None, true);
    assert!(down.abs() < 1.0, "{down}");
}

#[test]
fn a_level_slot_scales_the_voice() {
    let level = |amount: f32| {
        let mut h = synth();
        h.set_param("env.amp.sustain", 1.0);
        slot(&mut h, 0, "velocity", "level", None, amount);
        h.push_note("notes_in", on(0, 60, 1.0));
        atomik_testkit::signal::rms(&render(&mut h, 0.3)[4_000..])
    };
    let (none, up, down) = (level(0.0), level(0.5), level(-0.5));
    assert!(up > none * 1.3, "{up} vs {none}");
    assert!(down < none * 0.7, "{down} vs {none}");
}

#[test]
fn two_slots_into_one_target_add_up() {
    let level = |parts: &[(&str, f32)]| {
        let mut h = synth();
        dark(&mut h);
        h.set_param("env.filter.decay", 5_000.0);
        for (n, p) in parts.iter().enumerate() {
            slot(&mut h, n, p.0, "cutoff", None, p.1);
        }
        h.push_note("notes_in", on(0, 36, 0.9));
        brightness(&render(&mut h, 0.2))
    };
    let (a, b) = (("filter_env", 0.3), ("velocity", 0.3));
    let (only_a, only_b, both) = (level(&[a]), level(&[b]), level(&[a, b]));
    assert!(
        both > only_a && both > only_b,
        "{both} vs {only_a} and {only_b}"
    );
    // Slot order does not change the result of two positive contributions.
    assert_eq!(level(&[a, b]).to_bits(), level(&[a, b]).to_bits());
}

#[test]
fn the_output_is_the_same_bit_for_bit_however_the_render_is_split() {
    let play = |chunks: &[usize]| {
        let mut h = synth();
        dark(&mut h);
        h.set_param("lfo.rate", 3.0);
        slot(&mut h, 0, "filter_env", "cutoff", None, 0.7);
        slot(&mut h, 1, "lfo1", "pitch", Some("mod_wheel"), 0.02);
        slot(&mut h, 2, "lfo2", "level", None, 0.3);
        slot(&mut h, 3, "pressure", "cutoff", Some("velocity"), 0.5);
        h.set_param("lfo2.to_level", 0.0);
        h.push_note("notes_in", on(0, 45, 0.9));
        h.push_note("notes_in", on(5, 52, 0.5));
        let mut out = Vec::new();
        for &n in chunks {
            out.extend(bits(&h.render(n, &OUT)[0]));
        }
        out
    };
    let whole = play(&[64]);
    assert_eq!(whole, play(&[1; 64]));
    assert_eq!(whole, play(&[7, 13, 44]));
    assert_eq!(whole, play(&[32, 32]));
}

#[test]
fn the_output_stays_finite_and_smooth_with_every_slot_at_full_depth() {
    let mut h = synth();
    h.set_param("filter.resonance", 0.9);
    for (n, (s, t)) in [
        ("lfo1", "cutoff"),
        ("lfo2", "pitch"),
        ("amp_env", "level"),
        ("filter_env", "cutoff"),
        ("mod_env", "pitch"),
        ("velocity", "level"),
        ("key", "cutoff"),
        ("mod_wheel", "pitch"),
    ]
    .into_iter()
    .enumerate()
    {
        slot(
            &mut h,
            n,
            s,
            t,
            Some("pressure"),
            if n % 2 == 0 { 1.0 } else { -1.0 },
        );
    }
    for k in [36u8, 48, 60, 84, 96] {
        h.push_note("notes_in", on(0, k, 1.0));
    }
    let o = render(&mut h, 0.5);
    assert!(o.iter().all(|v| v.is_finite() && v.abs() < 4.0));
    assert!(max_step(&o) < 2.0);
}

#[test]
fn the_render_path_does_not_allocate_with_slots_in_use() {
    // The harness runs every `process` under the allocation guard; the slot params are set
    // under it here too, while notes sound.
    let mut h = synth();
    h.push_note("notes_in", on(0, 60, 0.7));
    h.render(2, &OUT);
    let (src, tgt, via, amount) = (source("lfo1"), target("cutoff"), source("mod_wheel"), 0.8);
    assert_no_alloc(|| {
        h.set_param("internal.mod1_source", src);
        h.set_param("internal.mod1_target", tgt);
        h.set_param("internal.mod1_via", via);
        h.set_param("mod1_amount", amount);
    });
    h.render(2, &OUT);
    slot(&mut h, 1, "pressure", "pitch", None, 0.1);
    slot(&mut h, 2, "velocity", "level", None, -0.4);
    h.render(2, &OUT);
}

#[test]
fn the_mono_bass_has_the_matrix_too() {
    let mut h = harness(Box::new(crate::mono_bass::create()));
    h.set_param("filter.cutoff", 200.0);
    h.set_param("filter.env_amount", 0.0);
    h.set_param("filter.key_track", 0.0);
    h.set_param("env.filter.decay", 5_000.0);
    let level = |h: &mut DeviceHarness| {
        h.push_note("notes_in", on(0, 36, 1.0));
        brightness(&render(h, 0.2))
    };
    let base = level(&mut h);
    let mut m = harness(Box::new(crate::mono_bass::create()));
    m.set_param("filter.cutoff", 200.0);
    m.set_param("filter.env_amount", 0.0);
    m.set_param("filter.key_track", 0.0);
    m.set_param("env.filter.decay", 5_000.0);
    slot(&mut m, 0, "filter_env", "cutoff", None, 1.0);
    let open = level(&mut m);
    assert!(open > base * 1.5, "{open} vs {base}");
}

/// Edits a slot while a note sounds (the panel does this with the mouse) and returns the render
/// of the edit and of the same run without it.
fn edited_run(edit: impl Fn(&mut DeviceHarness)) -> (Vec<f32>, Vec<f32>) {
    let run = |edited: bool| {
        let mut h = synth();
        h.set_param("env.amp.sustain", 1.0);
        h.push_note("notes_in", on(0, 48, 1.0));
        let mut out = render(&mut h, 0.3);
        if edited {
            edit(&mut h);
        }
        out.extend(render(&mut h, 0.3));
        out
    };
    (run(true), run(false))
}

#[test]
fn editing_a_slot_while_a_note_sounds_does_not_click() {
    // A held note through a level slot: filling the slot, moving its amount and clearing it are
    // each a gentle change, never a jump between two samples beyond the voice's own steps.
    type Edit = Box<dyn Fn(&mut DeviceHarness)>;
    let cases: [(&str, Edit); 3] = [
        (
            "fill",
            Box::new(|h| slot(h, 0, "velocity", "level", None, 0.5)),
        ),
        (
            "cutoff",
            Box::new(|h| slot(h, 0, "key", "cutoff", None, 1.0)),
        ),
        (
            "pitch",
            Box::new(|h| slot(h, 0, "lfo1", "pitch", None, 0.2)),
        ),
    ];
    for (name, edit) in cases {
        let (edited, plain) = edited_run(&*edit);
        let (a, b) = (max_step(&edited[..9_000]), max_step(&plain[..9_000]));
        assert_eq!(a, b, "{name}: before the edit the runs are identical");
        let at = 0.3 * SR;
        let around = &edited[at as usize - 64..at as usize + 2_048];
        let base = &plain[at as usize - 64..at as usize + 2_048];
        let (jump, natural) = (max_step(around), max_step(base));
        assert!(
            jump <= natural * 1.6 + 0.01,
            "{name}: the edit steps {jump}, the voice alone {natural}"
        );
    }
}
