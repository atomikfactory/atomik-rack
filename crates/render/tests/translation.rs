//! "Will the bass read?" on rendered songs, and the level and the stereo image of Split Sub: the
//! meter reads high for a saw bass and low for a pure sine, a stopped render returns nothing, and a
//! split bass keeps its level while everything under the split frequency becomes mono.

atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::PPQ;
use atomik_dsp::bass_read::Verdict;
use atomik_dsp::{BiquadCoeffs, BiquadShape};
use atomik_model::song::ClipId;
use atomik_model::{
    Clip, ClipContent, Command, CreateOptions, CreateSource, DeviceKey, Document, Note, RackDoc,
    SongCommand, StemKind, WorkflowCommand, new_project_doc, stem_sources,
};
use atomik_render::default_registry;
use atomik_render::plugin_pass::RenderControl;
use atomik_render::project::{PluginBounce, ProjectOptions};
use atomik_render::translation::render_reading;
use std::sync::atomic::AtomicBool;

const BAR: i64 = 4 * PPQ;
const SYNTH: &str = "atomik.poly-synth";
const BASS: &str = "atomik.mono-bass";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn apply(d: &mut Document, c: Command) {
    let registry = default_registry();
    d.apply(c.clone(), &registry)
        .unwrap_or_else(|e| panic!("{c:?}: {e}"));
}

/// A bass line: C2, C2, F2, G2, each a bar.
fn line() -> Clip {
    let keys = [36u8, 36, 41, 43];
    Clip {
        id: ClipId(0),
        start: 0,
        length: 4 * BAR,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        param_lanes: Vec::new(),
        midi: Default::default(),
        groove: None,
        controllers: Vec::new(),
        name: None,
        color: None,
        grid: None,
        content: ClipContent::Notes(
            keys.iter()
                .enumerate()
                .map(|(i, &k)| Note::new(i as i64 * BAR, BAR - PPQ / 2, k, 0.9))
                .collect::<Vec<_>>()
                .into(),
        ),
    }
}

/// A song with one bass track of `type_id` with the params `set`.
fn song(type_id: &str, set: &[(&str, f32)]) -> Document {
    let mut d = Document::new(new_project_doc());
    apply(
        &mut d,
        Command::Workflow(WorkflowCommand::CreateInstrument {
            index: usize::MAX,
            source: CreateSource::Type(type_id.into()),
            key: Some(key("bass")),
            options: CreateOptions::default(),
        }),
    );
    for &(p, v) in set {
        apply(
            &mut d,
            Command::SetParam {
                key: key("bass"),
                param: p.into(),
                value: v,
                merge: None,
            },
        );
    }
    apply(
        &mut d,
        Command::Song(SongCommand::AddClip {
            device: key("bass"),
            lane: 0,
            clip: line(),
        }),
    );
    apply(&mut d, Command::Song(SongCommand::SetSongEnd(5 * BAR)));
    d
}

fn options() -> ProjectOptions {
    ProjectOptions {
        host_block: std::num::NonZeroUsize::new(512).unwrap(),
        workers: 0,
        loop_count: None,
    }
}

/// The reading of the track `device` (or of the master).
fn reading(doc: &RackDoc, device: Option<&str>) -> atomik_dsp::bass_read::Reading {
    let registry = default_registry();
    let sources = stem_sources(doc, &registry);
    let source = sources
        .iter()
        .find(|s| match device {
            Some(d) => {
                s.kind == StemKind::Channel && s.dry.as_ref().is_some_and(|t| t[0].device == key(d))
            }
            None => s.kind == StemKind::Master,
        })
        .unwrap_or_else(|| panic!("no strip {device:?} in {sources:?}"));
    render_reading(
        doc,
        &registry,
        (&options(), None),
        &mut PluginBounce::default(),
        source,
        RenderControl::default(),
    )
    .expect("renders")
    .expect("not stopped")
}

/// Bright saw (no filter) or a pure sine, from the first oscillator only.
fn raw(wave: f32) -> Vec<(&'static str, f32)> {
    vec![
        ("osc1.wave", wave),
        ("osc.mix", 0.0),
        ("filter.cutoff", 20_000.0),
        ("filter.env_amount", 0.0),
        ("filter.key_track", 0.0),
        ("chorus.mix", 0.0),
        ("env.amp.attack", 5.0),
        ("env.amp.sustain", 1.0),
        ("env.amp.release", 50.0),
        ("master.level", -6.0),
    ]
}

#[test]
fn the_meter_reads_high_for_a_saw_bass_and_low_for_a_pure_sine() {
    let saw = song(SYNTH, &raw(0.0));
    let sine = song(SYNTH, &raw(3.0));
    let (saw_read, sine_read) = (
        reading(&saw.doc().clone(), Some("bass")),
        reading(sine.doc(), Some("bass")),
    );
    assert_eq!(saw_read.verdict, Verdict::Reads, "{saw_read:?}");
    assert!(saw_read.share > 0.1, "{saw_read:?}");
    assert_eq!(sine_read.verdict, Verdict::Lost, "{sine_read:?}");
    assert!(sine_read.share < 0.01, "{sine_read:?}");
    // The master of the same songs says the same: the bass is all there is.
    let master = reading(saw.doc(), None);
    assert_eq!(master.verdict, Verdict::Reads, "{master:?}");
    assert_eq!(reading(sine.doc(), None).verdict, Verdict::Lost);
}

#[test]
fn a_muted_track_has_no_bass_to_read() {
    let mut d = song(SYNTH, &raw(0.0));
    apply(
        &mut d,
        Command::Song(SongCommand::SetTrackFlags {
            device: key("bass"),
            muted: true,
            solo: false,
            armed: false,
        }),
    );
    assert_eq!(reading(d.doc(), Some("bass")).verdict, Verdict::NoBass);
}

#[test]
fn a_stopped_render_returns_nothing() {
    let d = song(SYNTH, &raw(0.0));
    let registry = default_registry();
    let sources = stem_sources(d.doc(), &registry);
    let stop = AtomicBool::new(true);
    let result = render_reading(
        d.doc(),
        &registry,
        (&options(), None),
        &mut PluginBounce::default(),
        &sources[0],
        RenderControl {
            cancel: Some(&stop),
            progress: None,
        },
    )
    .expect("no error");
    assert!(result.is_none());
}

fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len().max(1) as f64).sqrt()
}

/// `x` through a 4th-order Butterworth low-pass at `hz`.
fn low_passed(x: &[f32], hz: f64, rate: f64) -> Vec<f32> {
    let mut stages = [
        (
            BiquadCoeffs::design(BiquadShape::Lowpass, hz, 0.5412, 0.0, rate),
            atomik_dsp::Biquad::default(),
        ),
        (
            BiquadCoeffs::design(BiquadShape::Lowpass, hz, 1.3066, 0.0, rate),
            atomik_dsp::Biquad::default(),
        ),
    ];
    x.iter()
        .map(|&s| {
            let mut v = f64::from(s);
            for (c, b) in &mut stages {
                v = b.process(c, v);
            }
            v as f32
        })
        .collect()
}

fn bounce(doc: &RackDoc) -> atomik_render::Rendered {
    common::render_guarded(doc, &default_registry(), &options()).expect("renders")
}

fn split(d: &mut Document, freq: f32) {
    apply(
        d,
        Command::Workflow(WorkflowCommand::SplitSub {
            device: key("bass"),
            freq,
        }),
    );
}

#[test]
fn a_split_bass_keeps_its_level() {
    // (patch, how far the level may move in dB). With the filter open the sum is the bass; the
    // resonance and the drive of the factory sound change the weight of the fundamental.
    let open = [
        ("filter.resonance", 0.0),
        ("filter.drive", 0.0),
        ("filter.cutoff", 20_000.0),
        ("filter.env_amount", 0.0),
    ];
    let patches: [(Vec<(&str, f32)>, f64); 5] = [
        (vec![], 3.0),
        (vec![("filter.resonance", 0.0)], 3.0),
        (open.to_vec(), 1.0),
        (raw(0.0), 1.5),
        (vec![("osc2.wave", 0.0), ("osc.mix", 0.5)], 1.5),
    ];
    for (set, allowed) in patches {
        let mut d = song(BASS, &set);
        let before = bounce(d.doc());
        split(&mut d, 120.0);
        let after = bounce(d.doc());
        assert_eq!(before.channels[0].len(), after.channels[0].len());
        let (a, b) = (rms(&before.channels[0]), rms(&after.channels[0]));
        let db = 20.0 * (b / a).log10();
        assert!(a > 0.01, "the bass sounds: {a}");
        assert!(
            db.abs() < allowed,
            "{set:?}: the level moved by {db:.2} dB (at most {allowed})"
        );
    }
}

#[test]
fn a_split_bass_is_mono_under_the_split_and_wide_above_it() {
    // A wide chorus on the bass makes the low end differ between the sides.
    let set = [
        ("chorus.mix", 1.0),
        ("chorus.depth", 1.0),
        ("chorus.rate", 1.2),
    ];
    let mut d = song(BASS, &set);
    let before = bounce(d.doc());
    split(&mut d, 120.0);
    let after = bounce(d.doc());
    let rate = f64::from(before.sample_rate);
    let side = |r: &atomik_render::Rendered, hz: Option<f64>| {
        let diff: Vec<f32> = r.channels[0]
            .iter()
            .zip(&r.channels[1])
            .map(|(l, r)| l - r)
            .collect();
        match hz {
            Some(hz) => rms(&low_passed(&diff, hz, rate)),
            None => rms(&diff),
        }
    };
    let mid = |r: &atomik_render::Rendered, hz: f64| {
        let sum: Vec<f32> = r.channels[0]
            .iter()
            .zip(&r.channels[1])
            .map(|(l, r)| l + r)
            .collect();
        rms(&low_passed(&sum, hz, rate))
    };
    let (side_before, side_after) = (side(&before, Some(80.0)), side(&after, Some(80.0)));
    let ratio_before = side_before / mid(&before, 80.0);
    let ratio_after = side_after / mid(&after, 80.0);
    assert!(
        ratio_before > 0.05,
        "the chorus makes the low end wide: {ratio_before}"
    );
    assert!(
        ratio_after < ratio_before / 4.0,
        "under the split it is mono now: {ratio_before} then {ratio_after}"
    );
    // Above the split the width is still there.
    assert!(side(&after, None) > 0.0);
    let high_before = side(&before, None) - side_before;
    let high_after = side(&after, None) - side_after;
    assert!(
        high_after > 0.3 * high_before,
        "{high_before} then {high_after}"
    );
}
