//! Quantize, swing, groove and humanize in a bounce: the baked notes sound where the commands put
//! them (the same bytes as notes written at those ticks), a swung groove delays the off-beats by
//! the closed-form number of frames, humanize is reproducible (a second run, a save and load, any
//! host block and any worker count) and another seed gives another bounce, and the groove library
//! itself does not change what a project sounds like.

use atomik_core::PPQ;
use atomik_model::{
    Clip, ClipContent, ClipId, Command, DeviceKey, Document, Groove, GrooveSlot, Note,
    QuantizeParams, RackDoc, SongCommand, StepResolution, empty_doc, load_project, save_project,
};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc};
use std::num::NonZeroUsize;

const BAR: i64 = 4 * PPQ;
/// One 1/16 step.
const S: i64 = PPQ / 4;
/// Frames per tick at 120 BPM and 48 kHz: half a second per beat.
const FRAMES_PER_TICK: f64 = 48_000.0 * 0.5 / PPQ as f64;

fn apply(d: &mut Document, c: Command) {
    d.apply(c.clone(), &default_registry())
        .unwrap_or_else(|e| panic!("{c:?}: {e}"));
}

fn song(c: SongCommand) -> Command {
    Command::Song(c)
}

/// A pure-sine synth on a track with one clip of `notes` (clip 1, one bar).
fn synth_doc(notes: Vec<Note>) -> Document {
    let synth = DeviceKey("synth".into());
    let mut d = Document::new(empty_doc());
    apply(
        &mut d,
        Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".into(),
            key: Some(synth.clone()),
        },
    );
    apply(
        &mut d,
        song(SongCommand::AddTrack {
            device: synth.clone(),
        }),
    );
    apply(&mut d, song(SongCommand::SetSongEnd(2 * BAR)));
    apply(
        &mut d,
        song(SongCommand::AddClip {
            device: synth.clone(),
            lane: 0,
            clip: Clip {
                id: ClipId(0),
                start: 0,
                length: BAR,
                offset: 0,
                loop_length: None,
                muted: false,
                grid: None,
                scale: None,
                controllers: Vec::new(),
                content: ClipContent::Notes(notes),
            },
        }),
    );
    let mut doc = d.doc().clone();
    let dev = doc.devices.iter_mut().find(|x| x.key == synth).unwrap();
    for (k, v) in [
        ("osc1.wave", 3.0),
        ("osc.mix", 0.0),
        ("filter.cutoff", 20_000.0),
        ("filter.env_amount", 0.0),
        ("filter.key_track", 0.0),
        ("filter.resonance", 0.0),
        // Short notes that end cleanly, so each onset stands alone.
        ("env.amp.attack", 1.0),
        ("env.amp.release", 5.0),
    ] {
        dev.params.insert(k.to_owned(), v);
    }
    Document::new(doc)
}

/// Eight short notes of key 60, `jitter` ticks after their lines.
fn loose(jitter: i64) -> Vec<Note> {
    (0..8)
        .map(|i| Note::new(i * S + jitter, S / 2, 60, 0.9))
        .collect()
}

fn bounce_with(doc: &RackDoc, block: usize, workers: u16) -> Vec<f32> {
    let options = ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    };
    render_doc(doc, &default_registry(), &options)
        .unwrap()
        .channels
        .remove(0)
}

fn bounce(doc: &RackDoc) -> Vec<f32> {
    bounce_with(doc, 512, 0)
}

fn quantize(d: &mut Document, q: QuantizeParams) {
    apply(
        d,
        song(SongCommand::ApplyQuantize {
            clips: vec![ClipId(1)],
            quantize: q,
        }),
    );
}

fn notes_of(d: &Document) -> Vec<Note> {
    match &d.doc().song.clip(ClipId(1)).unwrap().2.content {
        ClipContent::Notes(n) => n.clone(),
        _ => unreachable!(),
    }
}

/// The first frame of each window of `win` frames whose RMS first rises above `level` after a
/// window below it: the onsets of a series of separate notes.
fn onsets(x: &[f32], win: usize, level: f32) -> Vec<usize> {
    let mut out = Vec::new();
    let mut quiet = true;
    for (i, w) in x.chunks(win).enumerate() {
        let rms = (w.iter().map(|s| s * s).sum::<f32>() / w.len() as f32).sqrt();
        if quiet && rms > level {
            out.push(i * win);
            quiet = false;
        } else if rms < level * 0.2 {
            quiet = true;
        }
    }
    out
}

#[test]
fn a_swung_quantize_delays_the_off_beats_by_the_closed_form_number_of_frames() {
    let mut d = synth_doc(loose(0));
    let straight = bounce(d.doc());
    let q = QuantizeParams {
        swing: 66,
        ..QuantizeParams::default()
    };
    quantize(&mut d, q);
    let swung = bounce(d.doc());
    assert!(swung != straight, "the swing changed the sound");
    let before = onsets(&straight, 32, 0.01);
    let after = onsets(&swung, 32, 0.01);
    assert_eq!(before.len(), 8, "{before:?}");
    assert_eq!(after.len(), 8, "{after:?}");
    let delay = (S as f64 * 0.32).round() * FRAMES_PER_TICK;
    for i in 0..8 {
        let want = if i % 2 == 1 { delay } else { 0.0 };
        let got = after[i] as f64 - before[i] as f64;
        assert!((got - want).abs() <= 33.0, "note {i}: {got} against {want}");
    }
}

#[test]
fn baked_notes_bounce_exactly_like_notes_written_at_those_ticks() {
    let mut d = synth_doc(loose(300));
    quantize(
        &mut d,
        QuantizeParams {
            swing: 60,
            ..QuantizeParams::default()
        },
    );
    let delay = (S as f64 * 0.2).round() as i64;
    let by_hand = synth_doc(
        (0..8)
            .map(|i| Note::new(i * S + if i % 2 == 1 { delay } else { 0 }, S / 2, 60, 0.9))
            .collect(),
    );
    assert_eq!(notes_of(&d), notes_of(&by_hand).as_slice());
    assert!(bounce(d.doc()) == bounce(by_hand.doc()));
}

#[test]
fn a_groove_velocity_scale_changes_the_level_of_its_slots() {
    let g = Groove {
        name: "Quiet odds".into(),
        step: StepResolution::Sixteenth,
        slots: vec![
            GrooveSlot::STRAIGHT,
            GrooveSlot {
                offset: 0.0,
                velocity: 0.25,
            },
        ],
    };
    let mut d = synth_doc(loose(0));
    apply(&mut d, song(SongCommand::SetGroove(g)));
    quantize(
        &mut d,
        QuantizeParams {
            groove: Some("Quiet odds".into()),
            ..QuantizeParams::default()
        },
    );
    let x = bounce(d.doc());
    let peak = |i: i64| {
        let a = (i * S) as f64 * FRAMES_PER_TICK;
        x[a as usize..(a + S as f64 / 2.0 * FRAMES_PER_TICK) as usize]
            .iter()
            .fold(0.0f32, |m, s| m.max(s.abs()))
    };
    let (even, odd) = (peak(2), peak(3));
    assert!(even > 0.05, "{even}");
    assert!(
        odd < even * 0.8,
        "an odd step is quieter (the synth follows velocity only partly): {odd} against {even}"
    );
}

#[test]
fn humanize_bounces_the_same_every_time_at_any_block_and_worker_count() {
    let humanize = QuantizeParams {
        strength: 0.0,
        humanize_time: 0.3,
        humanize_velocity: 0.4,
        seed: 5,
        ..QuantizeParams::default()
    };
    let mut a = synth_doc(loose(0));
    quantize(&mut a, humanize.clone());
    let mut b = synth_doc(loose(0));
    quantize(&mut b, humanize.clone());
    assert_eq!(notes_of(&a), notes_of(&b));
    let reference = bounce(a.doc());
    assert!(
        reference != bounce(synth_doc(loose(0)).doc()),
        "it moved notes"
    );
    assert!(bounce(b.doc()) == reference);
    for (block, workers) in [(1, 0), (64, 0), (333, 2), (4096, 3)] {
        assert!(
            bounce_with(a.doc(), block, workers) == reference,
            "block {block}, workers {workers}"
        );
    }
    // A save and a load in between change nothing.
    let loaded = load_project(&save_project(a.doc()), &default_registry()).unwrap();
    assert!(bounce(&loaded) == reference);
    // Another seed is another take.
    let mut c = synth_doc(loose(0));
    quantize(
        &mut c,
        QuantizeParams {
            seed: 6,
            ..humanize
        },
    );
    assert!(bounce(c.doc()) != reference);
}

#[test]
fn the_groove_library_does_not_change_what_a_project_sounds_like() {
    let mut d = synth_doc(loose(0));
    let plain = bounce(d.doc());
    apply(
        &mut d,
        song(SongCommand::SetGroove(Groove {
            name: "Unused".into(),
            step: StepResolution::Eighth,
            slots: vec![GrooveSlot {
                offset: 0.25,
                velocity: 0.5,
            }],
        })),
    );
    assert!(bounce(d.doc()) == plain);
    let loaded = load_project(&save_project(d.doc()), &default_registry()).unwrap();
    assert_eq!(loaded.song.grooves.len(), 1);
    assert!(bounce(&loaded) == plain);
}

#[test]
fn undo_of_a_quantize_brings_the_original_bounce_back() {
    let mut d = synth_doc(loose(700));
    let loose_bounce = bounce(d.doc());
    quantize(&mut d, QuantizeParams::default());
    assert!(bounce(d.doc()) != loose_bounce);
    d.undo(&default_registry()).unwrap();
    assert!(bounce(d.doc()) == loose_bounce);
}
