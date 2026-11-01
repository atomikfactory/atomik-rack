//! Send follow and parallel channels reach the audio as designed: a post-fader send scales with
//! the bus fader and mute (nested buses multiply, a pre-fader send is untouched), a project
//! without the flag bounces as it always did, a parallel compressed copy sums to the strip's own
//! parallel compressor, and none of it depends on the block size or the worker count.

use atomik_core::PPQ;
use atomik_dsp::{fader_gain, fader_pos_for_db};
use atomik_model::console::OutputDest;
use atomik_model::{
    Clip, ClipContent, ClipId, Command, ConsoleCommand, DeviceKey, Document, Note, RackDoc,
    SongCommand, empty_doc,
};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc};
use std::num::NonZeroUsize;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn apply(d: &mut Document, c: Command) {
    d.apply(c.clone(), &default_registry())
        .unwrap_or_else(|e| panic!("{c:?}: {e}"));
}

fn set(k: &str, param: &str, value: f32) -> Command {
    Command::SetParam {
        key: key(k),
        param: param.into(),
        value,
        merge: None,
    }
}

fn console(c: ConsoleCommand) -> Command {
    Command::Console(c)
}

fn bus(name: &str) -> OutputDest {
    OutputDest::Bus(key(name))
}

/// One synth track `lead` (so channel `channel`) with a held note for one bar, and `ret`, a gain
/// tool on FX return 1; the channel's send 1 is on at 0 dB. The flag is off, as in a project
/// saved before it existed.
fn song() -> Document {
    let registry = default_registry();
    let mut d = Document::new(empty_doc());
    let length = 4 * PPQ;
    for c in [
        Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".into(),
            key: Some(key("lead")),
        },
        Command::Song(SongCommand::AddTrack {
            device: key("lead"),
        }),
        Command::Song(SongCommand::AddClip {
            device: key("lead"),
            lane: 0,
            clip: Clip {
                id: ClipId(0),
                start: 0,
                length,
                offset: 0,
                loop_length: None,
                muted: false,
                scale: None,
                controllers: Vec::new(),
                name: None,
                color: None,
                grid: None,
                content: ClipContent::Notes(vec![Note::new(0, length, 60, 0.8)].into()),
            },
        }),
        Command::Song(SongCommand::SetSongEnd(length)),
        Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.gain-tool".into(),
            key: Some(key("ret")),
        },
        console(ConsoleCommand::SetReturn {
            slot: 0,
            effect: Some(key("ret")),
        }),
        set("channel", "send1.on", 1.0),
    ] {
        d.apply(c.clone(), &registry)
            .unwrap_or_else(|e| panic!("{c:?}: {e}"));
    }
    d
}

/// `song()` with a bus `bus` the channel plays into, at fader position `pos`.
fn with_bus(pos: f32, follow: bool) -> Document {
    let mut d = song();
    for c in [
        console(ConsoleCommand::CreateBus {
            name: "Drums".into(),
            dest: OutputDest::Master,
        }),
        console(ConsoleCommand::SetOutput {
            strip: key("channel"),
            dest: bus("bus"),
        }),
        set("bus", "fader", pos),
    ] {
        apply(&mut d, c);
    }
    if follow {
        apply(&mut d, console(ConsoleCommand::SetSendFollow(true)));
    }
    d
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

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|s| s.to_bits()).collect()
}

fn peak(a: &[f32]) -> f32 {
    a.iter().fold(0.0, |m, s| m.max(s.abs()))
}

/// The largest difference between `a` and `scale * b`.
fn max_diff_scaled(a: &[f32], b: &[f32], scale: f32) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y * scale).abs())
        .fold(0.0, f32::max)
}

const UNITY: f32 = 0.75;

#[test]
fn a_post_fader_send_scales_with_the_bus_fader_like_the_main_signal() {
    let pos = fader_pos_for_db(-6.0);
    let g = fader_gain(pos);
    let unity = bounce(with_bus(UNITY, true).doc());
    let down = bounce(with_bus(pos, true).doc());
    let p = peak(&unity);
    assert!(p > 0.05, "the song is not silent");
    // Main and send both play: the sum scales by the bus gain, sample for sample.
    assert!(
        max_diff_scaled(&down, &unity, g) < 1e-4 * p,
        "follow on: the send moves with the bus"
    );
    // Without the flag the send stays where the channel put it: the sum no longer scales.
    let unity_off = bounce(with_bus(UNITY, false).doc());
    let down_off = bounce(with_bus(pos, false).doc());
    assert_eq!(bits(&unity), bits(&unity_off), "at unity nothing differs");
    assert!(
        max_diff_scaled(&down_off, &unity_off, g) > 0.05 * p,
        "follow off: the send ignores the bus"
    );
    assert!(max_diff_scaled(&down, &down_off, 1.0) > 0.05 * p);
}

#[test]
fn a_pre_fader_send_is_not_touched() {
    let pos = fader_pos_for_db(-6.0);
    let mut on = with_bus(pos, true);
    let mut off = with_bus(pos, false);
    for d in [&mut on, &mut off] {
        apply(d, set("channel", "send1.pre", 1.0));
    }
    assert_eq!(bits(&bounce(on.doc())), bits(&bounce(off.doc())));
}

#[test]
fn a_muted_bus_silences_the_post_fader_send_and_nested_buses_multiply() {
    let mut d = with_bus(UNITY, true);
    apply(&mut d, set("bus", "mute", 1.0));
    assert!(peak(&bounce(d.doc())) < 1e-6, "main and send are both off");
    let mut d = with_bus(UNITY, false);
    apply(&mut d, set("bus", "mute", 1.0));
    assert!(
        peak(&bounce(d.doc())) > 0.01,
        "without the flag the send leaks"
    );

    // Two buses in a row: the gains multiply, for the main signal and the send alike.
    let (a, b) = (fader_pos_for_db(-3.0), fader_pos_for_db(-4.5));
    let mut nested = song();
    for c in [
        console(ConsoleCommand::CreateBus {
            name: "Outer".into(),
            dest: OutputDest::Master,
        }),
        console(ConsoleCommand::CreateBus {
            name: "Inner".into(),
            dest: bus("bus"),
        }),
        console(ConsoleCommand::SetOutput {
            strip: key("channel"),
            dest: bus("bus-2"),
        }),
        console(ConsoleCommand::SetSendFollow(true)),
    ] {
        apply(&mut nested, c);
    }
    let reference = bounce(nested.doc());
    for c in [set("bus", "fader", a), set("bus-2", "fader", b)] {
        apply(&mut nested, c);
    }
    let both = bounce(nested.doc());
    let scale = fader_gain(a) * fader_gain(b) / (fader_gain(UNITY) * fader_gain(UNITY));
    assert!(
        max_diff_scaled(&both, &reference, scale) < 1e-4 * peak(&reference),
        "scale {scale}"
    );
}

#[test]
fn the_flag_alone_changes_nothing_while_no_strip_plays_into_a_bus() {
    let plain = bounce(song().doc());
    let mut d = song();
    apply(&mut d, console(ConsoleCommand::SetSendFollow(true)));
    assert_eq!(bits(&bounce(d.doc())), bits(&plain));
}

#[test]
fn follow_links_do_not_make_the_bounce_depend_on_the_block_or_the_workers() {
    let mut d = with_bus(fader_pos_for_db(-6.0), true);
    for c in [
        console(ConsoleCommand::CreateParallel {
            strip: key("channel"),
        }),
        set("channel", "send2.on", 1.0),
    ] {
        apply(&mut d, c);
    }
    let base = bits(&bounce_with(d.doc(), 64, 0));
    for (block, workers) in [(512, 0), (333, 0), (64, 2), (512, 3)] {
        assert_eq!(
            bits(&bounce_with(d.doc(), block, workers)),
            base,
            "block {block}, workers {workers}"
        );
    }
}

#[test]
fn a_parallel_compressed_copy_sums_to_the_strips_own_parallel_compressor() {
    // The strip's parallel compressor at a 50 % blend...
    let mut own = song();
    for (p, v) in [
        ("send1.on", 0.0),
        ("pcomp.on", 1.0),
        ("pcomp.threshold", -30.0),
        ("pcomp.ratio", 4.0),
        ("pcomp.attack", 10.0),
        ("pcomp.release", 200.0),
        ("pcomp.makeup", 0.0),
        ("pcomp.mix", 0.5),
    ] {
        apply(&mut own, set("channel", p, v));
    }
    // ...against the dry strip at half level plus a parallel copy with the same compressor as
    // its dynamics, at half level.
    let half = fader_pos_for_db(fader_gain(UNITY).log10() * 20.0 - 6.0206);
    let mut twin = song();
    apply(
        &mut twin,
        console(ConsoleCommand::CreateParallel {
            strip: key("channel"),
        }),
    );
    let copy = twin.doc().console.parallels[0].1.0.clone();
    apply(&mut twin, set("channel", "send1.on", 0.0));
    apply(&mut twin, set("channel", "fader", half));
    for (p, v) in [
        ("fader", half),
        ("comp.on", 1.0),
        ("comp.threshold", -30.0),
        ("comp.ratio", 4.0),
        ("comp.release", 200.0),
    ] {
        apply(&mut twin, set(&copy, p, v));
    }
    let a = bounce(own.doc());
    let b = bounce(twin.doc());
    let p = peak(&a);
    assert!(p > 0.05);
    let diff = max_diff_scaled(&a, &b, 1.0);
    assert!(diff < 2e-4 * p, "diff {diff} against peak {p}");
    // The copy really is in the sum.
    let mut without = twin.clone();
    apply(&mut without, set(&copy, "mute", 1.0));
    assert!(max_diff_scaled(&bounce(without.doc()), &b, 1.0) > 0.01 * p);
}

#[test]
fn a_mix_channel_plays_what_is_cabled_to_its_direct_in() {
    let mut d = song();
    apply(
        &mut d,
        console(ConsoleCommand::AddMixChannel {
            name: "Back".into(),
        }),
    );
    let mix = d.doc().console.channels.last().unwrap().channel.0.clone();
    let plain = bounce(d.doc());
    let from = |s: &str| atomik_model::SocketRef {
        device: key("lead"),
        socket: s.into(),
    };
    for side in ["l", "r"] {
        apply(
            &mut d,
            Command::Disconnect {
                to: atomik_model::SocketRef {
                    device: key("channel"),
                    socket: format!("in_{side}"),
                },
            },
        );
        apply(
            &mut d,
            Command::Connect(atomik_model::Cable {
                from: from(&format!("out_{side}")),
                to: atomik_model::SocketRef {
                    device: key(&mix),
                    socket: format!("in_{side}"),
                },
            }),
        );
    }
    let via_mix = bounce(d.doc());
    assert!(peak(&plain) > 0.05);
    assert!(peak(&via_mix) > 0.05, "the Direct In plays");
    // Same strip, same params, so the same sound as the synth's own channel (the send is
    // the one thing the mix channel does not have switched on yet).
    apply(&mut d, set(&mix, "send1.on", 1.0));
    let again = bounce(d.doc());
    assert!(max_diff_scaled(&again, &plain, 1.0) < 1e-5 * peak(&plain));
}
