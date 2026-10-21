//! The Character panel's
//! meter feed, the tiered latency reaching delay compensation live, and the live twins of the
//! `character-demo` bounce (host blocks 64, 480 and 512, after a preset change across the latency
//! classes, after undo and redo and after an engine restart).

use super::*;
use crate::offline_sink::OfflineSink;
use crate::plugin_test_support::stepped;
use crate::sink::RecordingSink;
use atomik_core::PPQ;
use atomik_devices::character::chain::{long_latency, short_latency};
use atomik_model::{
    AutomationPoint, Cable, Clip, ClipContent, ClipId, Command, ConsoleCommand, ContainerCommand,
    Curve, DeviceKey, MacroTarget, Note, SocketRef, SongCommand, empty_doc,
};

const SYNTH: &str = "atomik.poly-synth";
const CHARACTER: &str = "atomik.character";
const RATE: f32 = 48_000.0;
/// The engine's fixed output latency (one batch) that the live render carries over the bounce.
const SKIP: usize = atomik_core::BATCH;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn add<S: EngineSink>(c: &mut AppCore<S>, type_id: &str, k: &str) {
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: type_id.into(),
        key: Some(key(k)),
    })
    .unwrap_or_else(|e| panic!("add {type_id}: {e}"));
}

fn set<S: EngineSink>(c: &mut AppCore<S>, k: &str, p: &str, v: f32) {
    c.apply_command(Command::SetParam {
        key: key(k),
        param: p.into(),
        value: v,
        merge: None,
    })
    .unwrap_or_else(|e| panic!("{k} {p}: {e}"));
}

fn sref(d: &str, s: &str) -> SocketRef {
    SocketRef {
        device: key(d),
        socket: s.to_string(),
    }
}

fn connect<S: EngineSink>(c: &mut AppCore<S>, from: (&str, &str), to: (&str, &str)) {
    c.apply_command(Command::Connect(Cable {
        from: sref(from.0, from.1),
        to: sref(to.0, to.1),
    }))
    .unwrap_or_else(|e| panic!("{from:?} -> {to:?}: {e}"));
}

fn channel_of<S: EngineSink>(c: &AppCore<S>, source: &str) -> String {
    c.doc()
        .console
        .channel_of(&key(source))
        .expect("channel")
        .channel
        .0
        .clone()
}

/// Cables the insert of channel `host` through the effect `fx`.
fn insert<S: EngineSink>(c: &mut AppCore<S>, host: &str, fx: &str) {
    for side in ["l", "r"] {
        connect(
            c,
            (host, &format!("insert_send_{side}")),
            (fx, &format!("in_{side}")),
        );
        connect(
            c,
            (fx, &format!("out_{side}")),
            (host, &format!("insert_return_{side}")),
        );
    }
}

fn clip(length: i64, content: ClipContent) -> Clip {
    Clip {
        id: ClipId(0),
        start: 0,
        length,
        offset: 0,
        loop_length: None,
        muted: false,
        grid: None,
        content,
    }
}

/// A track of `device` that plays a note a beat for `beats` beats.
fn track<S: EngineSink>(c: &mut AppCore<S>, device: &str, beats: i64, k: u8) {
    c.apply_command(Command::Song(SongCommand::AddTrack {
        device: key(device),
    }))
    .unwrap();
    let notes = (0..beats)
        .map(|b| Note::new(b * PPQ, PPQ / 2, k, 0.9))
        .collect();
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key(device),
        lane: 0,
        clip: clip(beats * PPQ, ClipContent::Notes(notes)),
    }))
    .unwrap();
}

fn play_voice(c: &mut AppCore<OfflineSink>) {
    add(c, SYNTH, "synth");
    add(c, CHARACTER, "chr");
    let ch = channel_of(c, "synth");
    insert(c, &ch, "chr");
    track(c, "synth", 8, 48);
}

// ------------------------------------------------------------------ the meter feed

#[test]
fn the_four_character_values_reach_the_view_and_the_peaks_fall_in_db() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, CHARACTER, "chr");
    let id = c.device_id(&key("chr")).unwrap();
    assert_eq!(atomik_ui::fx::FxKind::Character.meter_count(), 4);
    // (in peak, out peak, transform_eff, activity)
    c.sink_mut()
        .device_values
        .insert(id, (vec![0.5, 0.25, 0.75, 0.6], 1));
    c.frame();
    let shown = c.view().device_meters(&key("chr")).to_vec();
    assert_eq!(shown.len(), 4, "{shown:?}");
    assert!(
        (shown[0] - (-6.0)).abs() < 0.6,
        "input peak in dB: {shown:?}"
    );
    assert!(
        (shown[1] - (-12.0)).abs() < 0.6,
        "output peak in dB: {shown:?}"
    );
    assert!(
        (shown[2] - 0.75).abs() < 0.02,
        "transform as published: {shown:?}"
    );
    assert!((shown[3] - 0.6).abs() < 0.03, "activity: {shown:?}");
    // Silence from the engine lets the peaks fall and the activity fall off.
    c.sink_mut()
        .device_values
        .insert(id, (vec![0.0, 0.0, 0.75, 0.0], 2));
    c.frame();
    let later = c.view().device_meters(&key("chr")).to_vec();
    assert!(later[0] < shown[0] && later[1] < shown[1], "{later:?}");
    assert!(later[3] < shown[3], "{later:?}");
}

#[test]
fn a_real_engine_feeds_the_character_meters_while_music_plays() {
    let mut c = AppCore::new(OfflineSink::new());
    play_voice(&mut c);
    set(&mut c, "chr", "transform", 0.8);
    c.transport_action(atomik_ui::TransportAction::Play);
    for _ in 0..20 {
        c.sink_mut().render(4_800);
        c.frame();
    }
    let m = c.view().device_meters(&key("chr")).to_vec();
    assert_eq!(m.len(), 4, "{m:?}");
    assert!(
        m[0] > -60.0 && m[1] > -60.0,
        "both peaks follow the signal: {m:?}"
    );
    assert!((m[2] - 0.8).abs() < 0.02, "the effective Transform: {m:?}");
    assert!(m[3] > 0.0, "the activity moves: {m:?}");
    // Stopped for a while: the peaks fall back.
    c.transport_action(atomik_ui::TransportAction::Stop);
    for _ in 0..40 {
        c.sink_mut().render(4_800);
        c.frame();
    }
    let quiet = c.view().device_meters(&key("chr")).to_vec();
    assert!(quiet[0] < m[0] - 20.0, "{quiet:?} after {m:?}");
}

// ------------------------------------------------------------------ the tiered latency, live

#[test]
fn the_engine_publishes_the_tiered_latency_when_the_lossy_preset_is_selected() {
    let mut c = AppCore::new(OfflineSink::new());
    play_voice(&mut c);
    c.sink_mut().render(256);
    c.frame();
    let id = c.device_id(&key("chr")).unwrap();
    let (short, long) = (short_latency(RATE), long_latency(RATE));
    assert_eq!(c.topology().latency(id), Some(short), "reel: short class");
    let epoch = |c: &AppCore<OfflineSink>| c.sink().latency_epoch();
    let e0 = epoch(&c);
    // Inside the short class nothing is republished, even through a crossfade.
    for preset in [5.0, 13.0, 6.0] {
        set(&mut c, "chr", "preset", preset);
        c.sink_mut().render(4_800);
        c.frame();
        assert_eq!(
            epoch(&c),
            e0,
            "preset {preset}: no new latency inside the class"
        );
        assert_eq!(c.topology().latency(id), Some(short));
    }
    // Across the classes: lossy publishes the long latency once, when the fade has ended.
    set(&mut c, "chr", "preset", 8.0);
    c.sink_mut().render(128);
    c.frame();
    assert_eq!(
        c.topology().latency(id),
        Some(short),
        "not before the fade ends"
    );
    c.sink_mut().render(4_800);
    c.frame();
    assert_eq!(c.topology().latency(id), Some(long), "lossy: long class");
    assert_eq!(epoch(&c), e0 + 1, "one epoch bump");
    // Inside the long class and back to the short one.
    c.sink_mut().render(4_800);
    c.frame();
    assert_eq!(epoch(&c), e0 + 1);
    set(&mut c, "chr", "preset", 0.0);
    c.sink_mut().render(4_800);
    c.frame();
    assert_eq!(c.topology().latency(id), Some(short));
    assert_eq!(epoch(&c), e0 + 2);
}

#[test]
fn a_class_change_realigns_the_other_channels_through_the_routing() {
    let mut c = AppCore::new(RecordingSink::new());
    add(&mut c, SYNTH, "a");
    add(&mut c, SYNTH, "b");
    add(&mut c, CHARACTER, "chr");
    let ch_b = channel_of(&c, "b");
    insert(&mut c, &ch_b, "chr");
    c.frame();
    let id = c.device_id(&key("chr")).unwrap();
    let (short, long) = (short_latency(RATE), long_latency(RATE));
    for (frames, other) in [(short, 0), (long, short)] {
        c.sink_mut().latencies.insert(id, frames);
        c.sink_mut().epoch += 1;
        c.frame();
        assert_eq!(c.topology().latency(id), Some(frames));
        let pdc = c.sink().pdc.as_ref().expect("report");
        assert!(pdc.active);
        assert!(
            pdc.cable_delays
                .iter()
                .any(|(_, _, f)| *f == frames - other || *f == frames),
            "the path without the Character is delayed: {pdc:?}"
        );
    }
}

// ------------------------------------------------------------------ live equals bounce

fn bounce_of(doc: &atomik_model::RackDoc) -> Vec<Vec<f32>> {
    atomik_render::project::render_doc_in(
        doc,
        &default_registry(),
        &atomik_render::project::ProjectOptions::default(),
        None,
    )
    .unwrap()
    .channels
}

fn same_bits(live: &[Vec<f32>], bounce: &[Vec<f32>]) -> bool {
    live.len() == bounce.len()
        && live.iter().zip(bounce).all(|(a, b)| {
            a[SKIP..]
                .iter()
                .zip(b)
                .all(|(x, y)| x.to_bits() == y.to_bits())
                && a[..SKIP].iter().all(|x| *x == 0.0)
        })
}

fn first_difference(live: &[Vec<f32>], bounce: &[Vec<f32>]) -> Option<usize> {
    live[0][SKIP..]
        .iter()
        .zip(&bounce[0])
        .position(|(x, y)| x.to_bits() != y.to_bits())
}

fn demo_file() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/projects/character-demo/character-demo.atomik")
}

fn demo_core() -> AppCore<OfflineSink> {
    let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    core.open(&demo_file()).unwrap();
    core.wait_background();
    assert!(
        core.last_rejection().is_none(),
        "{:?}",
        core.last_rejection()
    );
    core
}

fn play(core: &mut AppCore<OfflineSink>, frames: usize, block: usize) -> Vec<Vec<f32>> {
    core.transport_action(atomik_ui::TransportAction::Play);
    core.sink_mut().render_blocks(frames + SKIP, block)
}

#[test]
fn the_live_path_plays_the_character_demo_exactly_like_the_bounce() {
    let reference = bounce_of(demo_core().doc());
    let frames = reference[0].len();
    assert!(reference[0].iter().any(|s| s.abs() > 1e-3), "it sounds");
    for block in [64, 480, 512] {
        let mut core = demo_core();
        let live = play(&mut core, frames, block);
        assert!(
            same_bits(&live, &reference),
            "block {block}: first difference at {:?}",
            first_difference(&live, &reference)
        );
    }
}

#[test]
fn the_character_demo_opens_plays_and_feeds_every_character() {
    let mut c = demo_core();
    c.transport_action(atomik_ui::TransportAction::Play);
    for _ in 0..40 {
        c.sink_mut().render(4_800);
        c.frame();
    }
    for k in ["bass-fx", "pad-fx"] {
        let m = c.view().device_meters(&key(k)).to_vec();
        assert_eq!(m.len(), 4, "{k}: {m:?}");
        assert!(m[0] > -90.0, "{k} has a signal: {m:?}");
    }
    // The Character inside the Lead container publishes by its path key (4 values).
    let mut buf = [0.0f32; 32];
    let (n, _) = c
        .device_meter_values(&key("lead/chr"), &mut buf)
        .expect("the inner Character publishes");
    assert_eq!(n, 4);
    assert!(buf[0] > 0.0, "the inner Character has a signal");
    assert!(c.last_rejection().is_none());
}

/// Moves the bass Character from `ion-sweep` (short class) to `lossy` (long class) through the
/// app, before the engine has rendered anything (a cold engine like the bounce's).
fn change_class(core: &mut AppCore<OfflineSink>) {
    set(core, "bass-fx", "preset", 8.0);
}

/// Plays `frames` live the way a running session does: one batch per frame, the engine's
/// published latencies followed after each batch (`AppCore::frame`), like the bounce's loop.
fn play_stepped(core: &mut AppCore<OfflineSink>, frames: usize) -> Vec<Vec<f32>> {
    core.transport_action(atomik_ui::TransportAction::Play);
    stepped(core, frames + SKIP)
}

#[test]
fn a_preset_change_across_the_latency_classes_plays_live_like_the_bounce_with_undo_and_redo() {
    let before = bounce_of(demo_core().doc());
    let mut core = demo_core();
    change_class(&mut core);
    let reference = bounce_of(&core.doc().clone());
    assert!(reference != before, "the class change is audible");
    let frames = reference[0].len();
    let live = play_stepped(&mut core, frames);
    assert!(
        same_bits(&live, &reference),
        "after the change: first difference at {:?}",
        first_difference(&live, &reference)
    );
    // Undo goes back across the classes and redo forward again; each time the live engine, still
    // cold, follows the document like a bounce of it.
    let mut core = demo_core();
    change_class(&mut core);
    assert!(core.undo());
    let live = play_stepped(&mut core, before[0].len());
    assert!(
        same_bits(&live, &before),
        "after undo: first difference at {:?}",
        first_difference(&live, &before)
    );
    let mut core = demo_core();
    change_class(&mut core);
    assert!(core.undo());
    assert!(core.redo());
    let live = play_stepped(&mut core, frames);
    assert!(
        same_bits(&live, &reference),
        "after redo: first difference at {:?}",
        first_difference(&live, &reference)
    );
}

#[test]
fn an_engine_restart_after_a_class_change_keeps_the_params_and_plays_like_the_bounce() {
    let before = bounce_of(demo_core().doc());
    let mut edited = demo_core();
    change_class(&mut edited);
    let reference = bounce_of(&edited.doc().clone());
    let frames = reference[0].len();
    for block in [64, 480, 512] {
        let mut core = demo_core();
        change_class(&mut core);
        core.rebuild_engine();
        for k in ["bass-fx", "pad-fx"] {
            assert_eq!(core.doc().device(&key(k)).unwrap().params["preset"], 8.0);
        }
        let live = play(&mut core, frames, block);
        assert!(
            same_bits(&live, &reference),
            "block {block}: first difference at {:?}",
            first_difference(&live, &reference)
        );
    }
    // Undo, then a restart: the original sound again.
    let mut core = demo_core();
    change_class(&mut core);
    assert!(core.undo());
    core.rebuild_engine();
    let live = play(&mut core, before[0].len(), 480);
    assert!(
        same_bits(&live, &before),
        "after undo and a restart: first difference at {:?}",
        first_difference(&live, &before)
    );
}

/// A container `box` (a click synth and a Character at mix 0, `macro1` on the preset) next to a
/// dry click synth `dry`; the song automates `macro1` from `reel` to `lossy` at beat 4.
fn crossing_core() -> AppCore<OfflineSink> {
    let mut c = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    let click = |c: &mut AppCore<OfflineSink>, k: &str| {
        for (p, v) in [
            ("osc1.wave", 1.0),
            ("osc.mix", 0.0),
            ("filter.cutoff", 20_000.0),
            ("filter.env_amount", 0.0),
            ("filter.key_track", 0.0),
            ("filter.resonance", 0.0),
            ("env.amp.attack", 0.5),
            ("env.amp.decay", 25.0),
            ("env.amp.sustain", 0.0),
            ("env.amp.release", 0.5),
            ("voices", 1.0),
        ] {
            set(c, k, p, v);
        }
    };
    add(&mut c, SYNTH, "dry");
    click(&mut c, "dry");
    add(&mut c, "atomik.container", "box");
    for (type_id, local, auto_split) in [(SYNTH, "syn", true), (CHARACTER, "chr", false)] {
        c.apply_command(Command::Container(ContainerCommand::AddInner {
            container: key("box"),
            index: usize::MAX,
            type_id: type_id.into(),
            key: Some(key(local)),
            auto_split,
        }))
        .unwrap();
    }
    click(&mut c, "box/syn");
    set(&mut c, "box/chr", "mix", 0.0);
    for side in ["l", "r"] {
        connect(
            &mut c,
            ("box/syn", &format!("out_{side}")),
            ("box/chr", &format!("in_{side}")),
        );
        connect(
            &mut c,
            ("box/chr", &format!("out_{side}")),
            ("box/boundary", &format!("out_{side}")),
        );
    }
    c.apply_command(Command::Container(ContainerCommand::SetMacro {
        container: key("box"),
        index: 0,
        target: MacroTarget {
            device: key("chr"),
            param: "preset".into(),
            min: 0.0,
            max: 15.0,
        },
        merge: None,
    }))
    .unwrap();
    c.apply_command(Command::Console(ConsoleCommand::AddChannel {
        source: key("box"),
    }))
    .unwrap();
    let (a, b) = (channel_of(&c, "dry"), channel_of(&c, "box"));
    set(&mut c, &a, "pan", -1.0);
    set(&mut c, &b, "pan", 1.0);
    track(&mut c, "dry", 12, 84);
    track(&mut c, "box", 12, 84);
    c.apply_command(Command::Song(SongCommand::AddAutomationLane {
        device: key("box"),
        param: "macro1".into(),
    }))
    .unwrap();
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key("box"),
        lane: 1,
        clip: clip(
            12 * PPQ,
            ClipContent::Automation(vec![
                AutomationPoint {
                    tick: 0,
                    value: 0.0,
                    curve: Curve::Step,
                },
                AutomationPoint {
                    tick: 4 * PPQ,
                    value: 8.0 / 15.0,
                    curve: Curve::Step,
                },
            ]),
        ),
    }))
    .unwrap();
    c.apply_command(Command::Song(SongCommand::SetSongEnd(12 * PPQ)))
        .unwrap();
    c
}

#[test]
fn a_class_change_in_the_song_is_followed_alike_by_the_live_path_and_the_bounce() {
    let mut core = crossing_core();
    stepped(&mut core, 64 * 16);
    let doc = core.doc().clone();
    let reference = bounce_of(&doc);
    assert!(reference[0].iter().any(|s| s.abs() > 1e-3), "it sounds");
    core.transport_action(atomik_ui::TransportAction::Play);
    let live = stepped(&mut core, reference[0].len() + SKIP);
    assert!(
        same_bits(&live, &reference),
        "live and bounce differ from frame {:?}",
        first_difference(&live, &reference)
    );
    // The change really crosses the classes in the song: the engine ends in the long class.
    let id = core.device_id(&key("box/chr")).unwrap();
    assert_eq!(core.topology().latency(id), Some(long_latency(RATE)));
}
