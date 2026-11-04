//! Container boundary CV, bounced: a cable from a modulation source to `container:cv1`, spliced
//! to an inner synth's cutoff CV, sounds bit for bit like the same patch built without a
//! container (the expansion theorem), whatever the host block and worker count; a container
//! without CV cabling bounces as it always did.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::PPQ;
use atomik_model::{
    Cable, Clip, ClipContent, ClipId, Command, ConsoleCommand, ContainerCommand, DeviceKey,
    Document, Note, RackDoc, SocketRef, SongCommand, empty_doc,
};
use atomik_render::default_registry;
use atomik_render::project::ProjectOptions;
use std::num::NonZeroUsize;

const SYNTH: &str = "atomik.poly-synth";
const CONTAINER: &str = "atomik.container";
const MOD: &str = "atomik.mod-source";
const BARS: i64 = 2;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn sref(spec: &str) -> SocketRef {
    let (device, socket) = spec.split_once(':').expect("device:socket");
    SocketRef {
        device: key(device),
        socket: socket.to_string(),
    }
}

struct Build {
    doc: Document,
}

impl Build {
    fn new() -> Self {
        Self {
            doc: Document::new(empty_doc()),
        }
    }

    fn apply(&mut self, c: Command) {
        let registry = default_registry();
        self.doc
            .apply(c.clone(), &registry)
            .unwrap_or_else(|e| panic!("{c:?}: {e}"));
    }

    fn add(&mut self, type_id: &str, k: &str) {
        self.apply(Command::AddDevice {
            index: usize::MAX,
            type_id: type_id.into(),
            key: Some(key(k)),
        });
    }

    fn inner(&mut self, container: &str, type_id: &str, k: &str) {
        self.apply(Command::Container(ContainerCommand::AddInner {
            container: key(container),
            index: usize::MAX,
            type_id: type_id.into(),
            key: Some(key(k)),
            auto_split: true,
        }));
    }

    fn set(&mut self, path: &str, p: &str, v: f32) {
        self.apply(Command::SetParam {
            key: key(path),
            param: p.into(),
            value: v,
            merge: None,
        });
    }

    fn connect(&mut self, from: &str, to: &str) {
        self.apply(Command::Connect(Cable {
            from: sref(from),
            to: sref(to),
        }));
    }

    /// A track on `device` that plays a held chord over two bars.
    fn song(&mut self, device: &str) {
        self.apply(Command::Song(SongCommand::AddTrack {
            device: key(device),
        }));
        let length = BARS * 4 * PPQ;
        let mut notes: Vec<Note> = [48u8, 55, 60, 64]
            .iter()
            .map(|k| Note::new(0, length - PPQ, *k, 0.8))
            .collect();
        notes.sort_by_key(|n| (n.tick, n.key));
        self.apply(Command::Song(SongCommand::AddClip {
            device: key(device),
            lane: 0,
            clip: Clip {
                id: ClipId(0),
                start: 0,
                length,
                offset: 0,
                loop_length: None,
                muted: false,
                scale: None,
                param_lanes: Vec::new(),
                midi: Default::default(),
                controllers: Vec::new(),
                name: None,
                color: None,
                grid: None,
                content: ClipContent::Notes(notes.into()),
            },
        }));
        self.apply(Command::Song(SongCommand::SetSongEnd(length)));
        self.apply(Command::Song(SongCommand::SetLoop {
            range: (0, length),
            enabled: false,
        }));
    }
}

/// `lfo` (a fast LFO) feeds the cutoff CV of a low-passed saw synth, with or without a
/// container around the synth; with `cabled` false the CV is left out.
fn patch(boxed: bool, cabled: bool) -> RackDoc {
    let mut b = Build::new();
    b.add(MOD, "lfo");
    b.set("lfo", "lfo1_rate", 3.0);
    b.set("lfo", "lfo1_amount", 1.0);
    if boxed {
        b.add(CONTAINER, "box");
        b.inner("box", SYNTH, "synth");
        b.set("box/synth", "filter.cutoff", 700.0);
        if cabled {
            b.connect("box/boundary:cv1", "box/synth:cutoff_cv_in");
            b.connect("lfo:lfo1_cv_out", "box:cv1");
        }
        b.connect("box/synth:out_l", "box/boundary:out_l");
        b.connect("box/synth:out_r", "box/boundary:out_r");
        b.apply(Command::Console(ConsoleCommand::AddChannel {
            source: key("box"),
        }));
        b.song("box");
    } else {
        b.add(SYNTH, "synth");
        b.set("synth", "filter.cutoff", 700.0);
        if cabled {
            b.connect("lfo:lfo1_cv_out", "synth:cutoff_cv_in");
        }
        b.song("synth");
    }
    b.doc.doc().clone()
}

fn bounce(doc: &RackDoc, block: usize, workers: u16) -> Vec<u32> {
    let options = ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    };
    let r = common::render_guarded_in(doc, &default_registry(), &options, None).unwrap();
    r.channels.iter().flatten().map(|x| x.to_bits()).collect()
}

#[test]
fn a_cv_cable_through_a_container_bounces_like_the_flat_patch() {
    let registry = default_registry();
    let boxed = patch(true, true);
    let flat = patch(false, true);
    assert!(boxed.has_containers() && !flat.has_containers());
    // The expansion is the flat cable list of the twin.
    let cables = boxed.expand(&registry).cables;
    assert!(
        cables
            .iter()
            .any(|c| c.from == sref("lfo:lfo1_cv_out") && c.to == sref("box/synth:cutoff_cv_in"))
    );
    let want = bounce(&flat, 256, 1);
    assert!(want.iter().any(|b| f32::from_bits(*b) != 0.0), "it sounds");
    for (block, workers) in [(256, 1), (64, 1), (512, 2)] {
        assert_eq!(bounce(&boxed, block, workers), want, "{block}/{workers}");
    }
}

#[test]
fn the_cv_changes_the_sound_and_an_uncabled_container_matches_its_twin() {
    let with = bounce(&patch(true, true), 256, 1);
    let without = bounce(&patch(true, false), 256, 1);
    assert_ne!(with, without, "the LFO moves the cutoff");
    assert_eq!(without, bounce(&patch(false, false), 256, 1));
}
