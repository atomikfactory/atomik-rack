//! A plugin's latency in the bounce and in the live path, both formats.
//!
//! A plugin whose latency comes from a param (a saved value, a container macro driven by the
//! song) reaches delay compensation through a restart: the plugin announces the change, the
//! manager restarts it, the engine publishes the latency, the routing is recompiled. The live app
//! does this on its main thread once per frame; the bounce did not do it at all, so a bounced
//! project lacked the compensation that the same project had live (found with the real plugins, see the
//! plan). These are the live-versus-bounce twins that go well past the first frames:
//!
//! * a **saved** latency param, in both formats: the bounce equals the live path bit for bit, in
//!   both run modes, and the latency shifts the whole mix by exactly its frames and changes
//!   nothing else (the other channel was compensated from the first frame);
//! * a latency that **changes mid-song** (a container macro automated by the song moves the
//!   latency param of a plugin inside it): the live path, stepped one batch per frame like the
//!   bounce's loop, and the bounce agree bit for bit through the restart and the recompile.

use super::*;
use crate::offline_sink::OfflineSink;
use crate::plugin_test_support::*;
use atomik_core::PPQ;
use atomik_device_sdk::RunMode;
use atomik_model::{
    AutomationPoint, Cable, Clip, ClipContent, ClipId, ConsoleCommand, ContainerCommand, Curve,
    MacroTarget, Note, PluginCommand, SocketRef, SongCommand, empty_doc,
};
use atomik_render::plugins::{MissingPlugins, PluginHost, bounce_settings};
use atomik_render::project::{PluginBounce, ProjectOptions, render_doc_with};

const SKIP: usize = atomik_core::BATCH;
const VST3_GAIN: &str = "Atomik Test Gain";

fn both_core() -> AppCore<OfflineSink> {
    let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    core.configure_plugins(config(true, &[PKG, PKG_VST3]));
    core
}

fn notes_clip(length: i64, notes: Vec<Note>) -> Clip {
    Clip {
        id: ClipId(0),
        start: 0,
        length,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        grid: None,
        content: ClipContent::Notes(notes),
    }
}

fn note(tick: i64, length: i64, key: u8) -> Note {
    Note::new(tick, length, key, 0.8)
}

fn track(core: &mut AppCore<OfflineSink>, device: &DeviceKey, length: i64, notes: Vec<Note>) {
    apply(
        core,
        Command::Song(SongCommand::AddTrack {
            device: device.clone(),
        }),
    );
    apply(
        core,
        Command::Song(SongCommand::AddClip {
            device: device.clone(),
            lane: 0,
            clip: notes_clip(length, notes),
        }),
    );
}

fn bounce_of(doc: &atomik_model::RackDoc, mode: RunMode) -> Vec<Vec<f32>> {
    let cache = cache_of(&[PKG, PKG_VST3]);
    let mut registry = default_registry();
    let mut host = PluginHost::new();
    host.set_catalog(&cache, &bounce_settings(), &mut registry);
    register_stand_ins(doc, &mut registry).unwrap();
    let mut plugins = PluginBounce {
        host: Some(&mut host),
        mode,
        missing: MissingPlugins::Error,
    };
    render_doc_with(
        doc,
        &registry,
        &ProjectOptions::default(),
        None,
        &mut plugins,
    )
    .unwrap()
    .channels
}

/// True when the live output (from the first frame after the engine's fixed latency) equals the
/// bounce bit for bit.
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

// ------------------------------------------------------------------ a saved latency param

/// `lead` (a synth, with `echo` on its console insert) and a dry `bass` synth on a channel of
/// its own, both playing from tick 0 for 3 beats, `echo` delaying by `latency` frames.
fn insert_project(synth: &str, effect: &str, latency: f32) -> AppCore<OfflineSink> {
    let mut core = both_core();
    add_plugin(&mut core, synth, "lead");
    add_plugin(&mut core, SYNTH, "bass");
    add_plugin(&mut core, effect, "echo");
    let channel = core
        .doc()
        .console
        .channel_of(&key("lead"))
        .unwrap()
        .channel
        .clone();
    apply(
        &mut core,
        Command::Console(ConsoleCommand::SetInsert {
            channel,
            slot: 0,
            effect: Some(atomik_model::InsertEffect::Existing(key("echo"))),
        }),
    );
    set_param(&mut core, "echo", "p3", latency);
    set_param(&mut core, "bass", "p1", 1.0);
    track(
        &mut core,
        &key("lead"),
        3 * PPQ,
        vec![note(0, PPQ, 60), note(PPQ, PPQ, 64), note(2 * PPQ, PPQ, 67)],
    );
    track(
        &mut core,
        &key("bass"),
        3 * PPQ,
        vec![note(0, 3 * PPQ / 2, 36), note(3 * PPQ / 2, 3 * PPQ / 2, 43)],
    );
    core
}

fn formats(core: &AppCore<OfflineSink>) -> Vec<(&'static str, String, String)> {
    vec![
        ("clap", SYNTH.to_string(), GAIN.to_string()),
        (
            "vst3",
            vst3_id(core, "Atomik Test Synth"),
            vst3_id(core, VST3_GAIN),
        ),
    ]
}

#[test]
fn a_saved_latency_makes_the_live_path_and_the_bounce_agree_in_both_formats_and_both_modes() {
    let ids = formats(&both_core());
    for (name, synth, effect) in ids {
        let mut core = insert_project(&synth, &effect, 100.0);
        settle_steps(&mut core);
        let doc = core.doc().clone();
        let realtime = bounce_of(&doc, RunMode::Realtime);
        let offline = bounce_of(&doc, RunMode::Offline);
        assert!(
            realtime[0].iter().any(|s| s.abs() > 1e-3),
            "{name}: it sounds"
        );
        assert_eq!(
            realtime, offline,
            "{name}: the test plugins give the same bits in both modes"
        );
        core.transport_action(atomik_ui::TransportAction::Play);
        let live = stepped(&mut core, realtime[0].len() + SKIP);
        assert!(
            same_bits(&live, &realtime),
            "{name}: live and bounce differ from frame {:?}",
            first_difference(&live, &realtime)
        );
    }
}

/// The largest difference between `delayed` shifted back by `shift` frames and `plain`.
fn shift_error(plain: &[f32], delayed: &[f32], shift: usize) -> f32 {
    let n = plain.len() - 128;
    delayed[shift..shift + n]
        .iter()
        .zip(&plain[..n])
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max)
}

#[test]
fn a_saved_latency_shifts_the_whole_bounce_by_its_frames_and_nothing_else() {
    let ids = formats(&both_core());
    for (name, synth, effect) in ids {
        let bounce = |latency: f32| {
            let core = insert_project(&synth, &effect, latency);
            bounce_of(core.doc(), RunMode::Realtime)
        };
        let (none, delayed) = (bounce(0.0), bounce(100.0));
        let peak = none[0].iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak > 1e-3, "{name}: it sounds");
        // The dry bass was delayed to match the effect from the first frame, so the mix is the
        // undelayed mix, 100 frames later (the channel strips differ in rounding noise only).
        for ch in 0..2 {
            assert!(
                delayed[ch][..100].iter().all(|s| *s == 0.0),
                "{name}: nothing before the latency"
            );
            let exact = shift_error(&none[ch], &delayed[ch], 100);
            assert!(
                exact < peak * 1e-4,
                "{name}: channel {ch}: shifted by 100 frames the mix is the plain mix ({exact} against a peak of {peak})"
            );
            for wrong in [99, 101] {
                assert!(
                    shift_error(&none[ch], &delayed[ch], wrong) > exact * 100.0,
                    "{name}: channel {ch}: a shift of {wrong} frames is clearly worse"
                );
            }
        }
    }
}

// ------------------------------------------------------------------ a latency that changes mid-song

/// A container `box` with a synth `syn` and a gain effect `fx` (cabled in series), macro 1 moving
/// `fx`'s latency param between 0 and 256 frames, a channel and a track that plays notes and
/// automates `macro1`: 0 until beat `step_beat`, then `value` (a stepped curve). A dry `bass`
/// synth plays beside it, so a missing compensation is audible.
fn macro_project(synth: &str, effect: &str, step_beat: i64, value: f32) -> AppCore<OfflineSink> {
    let mut core = both_core();
    let boxed = key("box");
    apply(
        &mut core,
        Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.container".into(),
            key: Some(boxed.clone()),
        },
    );
    for (type_id, local, auto_split) in [(synth, "syn", true), (effect, "fx", false)] {
        let plugin = atomik_device_sdk::PluginRef::from_type_id(type_id).unwrap();
        let layout = core.plugin_catalog().entry(&plugin).unwrap().layout.clone();
        apply(
            &mut core,
            Command::Plugin(PluginCommand::Add {
                index: usize::MAX,
                container: Some(boxed.clone()),
                plugin,
                layout,
                path_hint: None,
                key: Some(key(local)),
                auto_split,
            }),
        );
    }
    let sref = |d: &str, s: &str| SocketRef {
        device: key(d),
        socket: s.to_string(),
    };
    for side in ["l", "r"] {
        for (from, to) in [
            (
                sref("box/syn", &format!("out_{side}")),
                sref("box/fx", &format!("in_{side}")),
            ),
            (
                sref("box/fx", &format!("out_{side}")),
                sref("box/boundary", &format!("out_{side}")),
            ),
        ] {
            apply(&mut core, Command::Connect(Cable { from, to }));
        }
    }
    apply(
        &mut core,
        Command::Container(ContainerCommand::SetMacro {
            container: boxed.clone(),
            index: 0,
            target: MacroTarget {
                device: key("fx"),
                param: "p3".into(),
                min: 0.0,
                max: 256.0,
            },
            merge: None,
        }),
    );
    apply(
        &mut core,
        Command::Console(ConsoleCommand::AddChannel {
            source: boxed.clone(),
        }),
    );
    add_plugin(&mut core, SYNTH, "bass");
    set_param(&mut core, "bass", "p1", 1.0);
    let length = 8 * PPQ;
    track(
        &mut core,
        &boxed,
        length,
        (0..16)
            .map(|i| note(i * PPQ / 2, PPQ / 2, [60, 64, 67, 72][(i % 4) as usize]))
            .collect(),
    );
    track(
        &mut core,
        &key("bass"),
        length,
        (0..8)
            .map(|i| note(i * PPQ, PPQ, 36 + (i % 3) as u8 * 3))
            .collect(),
    );
    apply(
        &mut core,
        Command::Song(SongCommand::AddAutomationLane {
            device: boxed.clone(),
            param: "macro1".into(),
        }),
    );
    apply(
        &mut core,
        Command::Song(SongCommand::AddClip {
            device: boxed.clone(),
            lane: 1,
            clip: Clip {
                content: ClipContent::Automation(vec![
                    AutomationPoint {
                        tick: 0,
                        value: 0.0,
                        curve: Curve::Step,
                    },
                    AutomationPoint {
                        tick: step_beat * PPQ,
                        value,
                        curve: Curve::Step,
                    },
                ]),
                ..notes_clip(length, Vec::new())
            },
        }),
    );
    core
}

#[test]
fn a_latency_that_changes_mid_song_is_followed_alike_by_the_live_path_and_the_bounce() {
    let ids = formats(&both_core());
    for (name, synth, effect) in ids {
        let mut core = macro_project(&synth, &effect, 2, 0.5);
        settle_steps(&mut core);
        let doc = core.doc().clone();
        let reference = bounce_of(&doc, RunMode::Realtime);
        assert!(
            reference[0].iter().any(|s| s.abs() > 1e-3),
            "{name}: it sounds"
        );
        // The latency really changes in the song: the same project without the step differs
        // after the step and not before it (beat 2 of 120 bpm is frame 48 000).
        let flat = bounce_of(
            macro_project(&synth, &effect, 2, 0.0).doc(),
            RunMode::Realtime,
        );
        let step_frame = 48_000;
        assert_eq!(
            reference[0][..step_frame - 256],
            flat[0][..step_frame - 256],
            "{name}: identical before the step"
        );
        assert!(
            reference[0][step_frame..] != flat[0][step_frame..],
            "{name}: audibly different after the step (the latency and its compensation)"
        );
        core.transport_action(atomik_ui::TransportAction::Play);
        let live = stepped(&mut core, reference[0].len() + SKIP);
        assert!(
            same_bits(&live, &reference),
            "{name}: live and bounce differ from frame {:?}",
            first_difference(&live, &reference)
        );
        // And the bounce is stable: the same project twice, and in the other mode.
        assert_eq!(reference, bounce_of(&doc, RunMode::Realtime), "{name}");
        assert_eq!(reference, bounce_of(&doc, RunMode::Offline), "{name}");
    }
}
