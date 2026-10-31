//! A bounce never sends MIDI: a project with a MIDI Out device (a track of notes on it, a program,
//! a port, a CV input wired to a modulator) renders the same bits as the project without it, for
//! every block size and worker count. The device has no audio output; offline it is silent.

use atomik_core::PPQ;
use atomik_model::{
    Cable, Clip, ClipContent, ClipId, Command, DeviceKey, Document, Note, RackDoc, SocketRef,
    SongCommand, WorkflowCommand, empty_doc,
};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc_in};
use std::num::NonZeroUsize;

const BAR: i64 = 4 * PPQ;
const NOTES: [(i64, i64, u8, f32); 3] = [
    (0, PPQ, 48, 0.8),
    (PPQ, PPQ, 52, 0.6),
    (2 * PPQ, PPQ, 55, 0.9),
];

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
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
            type_id: type_id.into(),
            key: Some(key(k)),
        },
    );
}

fn track(d: &mut Document, device: &str) {
    apply(
        d,
        Command::Song(SongCommand::AddTrack {
            device: key(device),
        }),
    );
    let notes = NOTES
        .iter()
        .map(|&(s, l, k, v)| Note::new(s, l, k, v))
        .collect();
    apply(
        d,
        Command::Song(SongCommand::AddClip {
            device: key(device),
            lane: 0,
            clip: Clip {
                id: ClipId(0),
                start: 0,
                length: BAR,
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
    apply(d, Command::Song(SongCommand::SetSongEnd(BAR)));
}

fn project(with_midi_out: bool) -> RackDoc {
    let mut d = Document::new(empty_doc());
    add(&mut d, "atomik.poly-synth", "synth");
    track(&mut d, "synth");
    if with_midi_out {
        add(&mut d, "atomik.midi-out", "out");
        add(&mut d, "atomik.mod-source", "lfo");
        apply(
            &mut d,
            Command::Connect(Cable {
                from: SocketRef {
                    device: key("lfo"),
                    socket: "lfo1_cv_out".into(),
                },
                to: SocketRef {
                    device: key("out"),
                    socket: "cv1".into(),
                },
            }),
        );
        for (p, v) in [("program", 7.0), ("channel", 4.0), ("transpose", 5.0)] {
            apply(
                &mut d,
                Command::SetParam {
                    key: key("out"),
                    param: p.into(),
                    value: v,
                    merge: None,
                },
            );
        }
        apply(
            &mut d,
            Command::Workflow(WorkflowCommand::SetMidiOutPort {
                device: key("out"),
                port: Some("Example Synth".into()),
            }),
        );
        track(&mut d, "out");
    }
    d.doc().clone()
}

fn bounce(doc: &RackDoc, block: usize, workers: u16) -> Vec<u32> {
    let options = ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    };
    let r = render_doc_in(doc, &default_registry(), &options, None).unwrap();
    r.channels.iter().flatten().map(|x| x.to_bits()).collect()
}

#[test]
fn a_project_bounces_to_the_same_bits_with_and_without_a_midi_out_device() {
    let plain = bounce(&project(false), 512, 0);
    assert!(
        plain.iter().any(|b| f32::from_bits(*b).abs() > 1e-3),
        "the synth is audible"
    );
    let with = project(true);
    for (block, workers) in [(512, 0), (64, 0), (333, 0), (512, 2), (1000, 3)] {
        assert!(
            bounce(&with, block, workers) == plain,
            "block {block}, workers {workers}"
        );
    }
}
