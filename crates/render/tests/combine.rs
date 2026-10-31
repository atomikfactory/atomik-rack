//! The expansion theorem for `Combine`: a project bounces bit for bit as it did before its devices
//! were combined into a container, for an instrument selection, an effect selection and both
//! together, and a bypassed container bounces as its inside never was there.
//!
//! The devices here draw no random numbers: a device that draws from the project seed gets its
//! seed from its place in the flat device list, which a container's `~split` moves by one.

use atomik_core::PPQ;
use atomik_model::{
    Clip, ClipContent, ClipId, Command, ConsoleCommand, ContainerCommand, DeviceKey, Document,
    InsertEffect, Note, RackDoc, SocketRef, SongCommand, empty_doc,
};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc_in};
use std::num::NonZeroUsize;

const BAR: i64 = 4 * PPQ;
const SYNTH: &str = "atomik.poly-synth";
const DELAY: &str = "atomik.stereo-delay";
const MIXER: &str = "atomik.line-mixer";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn sock(spec: &str) -> SocketRef {
    let (device, socket) = spec.split_once(':').expect("device:socket");
    SocketRef {
        device: key(device),
        socket: socket.to_owned(),
    }
}

fn apply(d: &mut Document, c: Command) {
    d.apply(c.clone(), &default_registry())
        .unwrap_or_else(|e| panic!("{c:?}: {e}"));
}

fn add(d: &mut Document, type_id: &str, k: &str) {
    apply(
        d,
        Command::AddDevice {
            index: usize::MAX,
            type_id: type_id.to_owned(),
            key: Some(key(k)),
        },
    );
}

fn set(d: &mut Document, k: &str, param: &str, value: f32) {
    apply(
        d,
        Command::SetParam {
            key: key(k),
            param: param.to_owned(),
            value,
            merge: None,
        },
    );
}

fn connect(d: &mut Document, from: &str, to: &str) {
    apply(
        d,
        Command::Connect(atomik_model::Cable {
            from: sock(from),
            to: sock(to),
        }),
    );
}

fn tune(d: &mut Document) {
    // A plain saw with no noise, a short envelope: nothing here depends on the project seed.
    for (p, v) in [
        ("osc1.wave", 1.0),
        ("osc.mix", 0.0),
        ("voices", 4.0),
        ("filter.cutoff", 1800.0),
        ("env.amp.attack", 8.0),
        ("env.amp.decay", 250.0),
        ("env.amp.sustain", 0.7),
        ("env.amp.release", 180.0),
    ] {
        set(d, "synth", p, v);
    }
    set(d, "delay", "time.ms", 250.0);
    set(d, "delay", "feedback", 0.4);
    set(d, "delay", "mix", 0.35);
}

/// A track of two bars of notes on `synth`.
fn play(d: &mut Document) {
    apply(
        d,
        Command::Song(SongCommand::AddTrack {
            device: key("synth"),
        }),
    );
    let notes: Vec<Note> = [(0, 2, 48), (2, 2, 55), (4, 2, 52), (6, 2, 60)]
        .iter()
        .map(|&(beat, beats, k)| Note::new(beat * PPQ, beats * PPQ, k, 0.8))
        .collect();
    apply(
        d,
        Command::Song(SongCommand::AddClip {
            device: key("synth"),
            lane: 0,
            clip: Clip {
                id: ClipId(0),
                start: 0,
                length: 2 * BAR,
                offset: 0,
                loop_length: None,
                muted: false,
                scale: None,
                controllers: Vec::new(),
                grid: None,
                content: ClipContent::Notes(notes),
            },
        }),
    );
    apply(d, Command::Song(SongCommand::SetSongEnd(2 * BAR)));
    apply(
        d,
        Command::Song(SongCommand::SetLoop {
            range: (0, 2 * BAR),
            enabled: false,
        }),
    );
}

/// synth -> delay -> mixer -> interface, played by a track on the synth.
fn chain() -> Document {
    let mut d = Document::new(empty_doc());
    for (t, k) in [(SYNTH, "synth"), (DELAY, "delay"), (MIXER, "mixer")] {
        add(&mut d, t, k);
    }
    // The synth came with a console channel and the master with the interface: this rack is
    // wired by hand instead.
    let channel = d.doc().console.channels[0].channel.clone();
    apply(
        &mut d,
        Command::Console(ConsoleCommand::RemoveChannel { channel }),
    );
    for out in ["interface:out_1", "interface:out_2"] {
        apply(&mut d, Command::Disconnect { to: sock(out) });
    }
    for (f, t) in [
        ("synth:out_l", "delay:in_l"),
        ("synth:out_r", "delay:in_r"),
        ("delay:out_l", "mixer:ch1_in_l"),
        ("delay:out_r", "mixer:ch1_in_r"),
        ("mixer:master_l", "interface:out_1"),
        ("mixer:master_r", "interface:out_2"),
    ] {
        connect(&mut d, f, t);
    }
    tune(&mut d);
    play(&mut d);
    d
}

/// A synth with its console channel and a delay on the channel's first insert.
fn console() -> Document {
    let mut d = Document::new(empty_doc());
    add(&mut d, SYNTH, "synth");
    add(&mut d, DELAY, "delay");
    let channel = d.doc().console.channels[0].channel.clone();
    apply(
        &mut d,
        Command::Console(ConsoleCommand::SetInsert {
            channel,
            slot: 0,
            effect: Some(InsertEffect::Existing(key("delay"))),
        }),
    );
    tune(&mut d);
    play(&mut d);
    d
}

fn combine(d: &mut Document, keys: &[&str]) {
    apply(
        d,
        Command::Container(ContainerCommand::Combine {
            keys: keys.iter().map(|k| key(k)).collect(),
            name: String::new(),
        }),
    );
}

fn options(block: usize, workers: u16) -> ProjectOptions {
    ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    }
}

fn bounce(doc: &RackDoc, block: usize, workers: u16) -> Vec<u32> {
    render_doc_in(doc, &default_registry(), &options(block, workers), None)
        .unwrap()
        .channels
        .iter()
        .flatten()
        .map(|x| x.to_bits())
        .collect()
}

fn peak(doc: &RackDoc) -> f32 {
    render_doc_in(doc, &default_registry(), &options(512, 0), None)
        .unwrap()
        .channels
        .iter()
        .flatten()
        .fold(0.0f32, |m, x| m.max(x.abs()))
}

#[test]
fn the_reference_rack_sounds() {
    for doc in [chain(), console()] {
        assert!(peak(doc.doc()) > 0.01, "the project must be audible");
    }
}

#[test]
fn combining_an_instrument_and_an_effect_bounces_as_before() {
    let mut d = chain();
    let before = bounce(d.doc(), 512, 0);
    combine(&mut d, &["synth", "delay"]);
    assert!(d.doc().has_containers());
    for (block, workers) in [(512, 0), (64, 0), (480, 2), (1000, 3)] {
        assert!(
            bounce(d.doc(), block, workers) == before,
            "block {block}, workers {workers}"
        );
    }
}

#[test]
fn combining_an_effect_alone_bounces_as_before() {
    let mut d = chain();
    let before = bounce(d.doc(), 512, 0);
    combine(&mut d, &["delay"]);
    assert!(bounce(d.doc(), 512, 0) == before);
    assert!(bounce(d.doc(), 96, 2) == before);
}

#[test]
fn combining_an_instrument_with_its_channel_bounces_as_before() {
    let mut d = console();
    let before = bounce(d.doc(), 512, 0);
    combine(&mut d, &["synth"]);
    assert_eq!(d.doc().console.channels[0].source, Some(key("container")));
    assert!(bounce(d.doc(), 512, 0) == before);
}

#[test]
fn combining_the_insert_effect_of_a_channel_bounces_as_before() {
    let mut d = console();
    let before = bounce(d.doc(), 512, 0);
    combine(&mut d, &["delay"]);
    assert!(bounce(d.doc(), 512, 0) == before);
}

#[test]
fn combine_then_uncombine_bounces_as_before() {
    let mut d = chain();
    let before = bounce(d.doc(), 512, 0);
    combine(&mut d, &["synth", "delay"]);
    apply(
        &mut d,
        Command::Container(ContainerCommand::Uncombine {
            container: key("container"),
        }),
    );
    assert!(!d.doc().has_containers());
    assert!(bounce(d.doc(), 512, 0) == before);
}

#[test]
fn a_bypassed_container_bounces_as_if_its_inside_were_not_there() {
    let mut d = chain();
    combine(&mut d, &["delay"]);
    let with_delay = bounce(d.doc(), 512, 0);
    set(&mut d, "container", "bypass", 1.0);
    let bypassed = bounce(d.doc(), 512, 0);
    assert!(bypassed != with_delay, "the delay is heard when it is on");
    // The reference: the delay is not in the rack and the synth feeds the mixer.
    let mut dry = chain();
    apply(&mut dry, Command::RemoveDevice { key: key("delay") });
    connect(&mut dry, "synth:out_l", "mixer:ch1_in_l");
    connect(&mut dry, "synth:out_r", "mixer:ch1_in_r");
    for (block, workers) in [(512, 0), (64, 0), (480, 2)] {
        assert!(
            bounce(d.doc(), block, workers) == bounce(dry.doc(), block, workers),
            "block {block}, workers {workers}"
        );
    }
    // Switched off again, the original bounce is back.
    set(&mut d, "container", "bypass", 0.0);
    assert!(bounce(d.doc(), 512, 0) == with_delay);
}
