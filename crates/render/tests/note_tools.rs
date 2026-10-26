//! Bounces of the note tools (players that map or generate notes between a clip and a synth).
//!
//! Each test builds its project in memory and bounces it through the shared renderer, so the
//! engine path (track input, note cable, device batching) is covered, not just the device.

use atomik_core::PPQ;
use atomik_model::{
    Cable, Clip, ClipContent, ClipId, Command, DeviceKey, Document, Note, RackDoc, SocketRef,
    SongCommand, empty_doc,
};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc_in};
use std::num::NonZeroUsize;

const BAR: i64 = 4 * PPQ;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn sref(d: &str, s: &str) -> SocketRef {
    SocketRef {
        device: key(d),
        socket: s.to_string(),
    }
}

struct Builder {
    doc: Document,
}

impl Builder {
    fn new() -> Self {
        Self {
            doc: Document::new(empty_doc()),
        }
    }

    fn apply(&mut self, c: Command) {
        self.doc
            .apply(c.clone(), &default_registry())
            .unwrap_or_else(|e| panic!("{c:?}: {e}"));
    }

    fn add(&mut self, type_id: &str, k: &str) {
        self.apply(Command::AddDevice {
            index: usize::MAX,
            type_id: type_id.into(),
            key: Some(key(k)),
        });
    }

    fn set(&mut self, k: &str, p: &str, v: f32) {
        self.apply(Command::SetParam {
            key: key(k),
            param: p.into(),
            value: v,
            merge: None,
        });
    }

    fn connect(&mut self, from: (&str, &str), to: (&str, &str)) {
        self.apply(Command::Connect(Cable {
            from: sref(from.0, from.1),
            to: sref(to.0, to.1),
        }));
    }

    /// A one-bar clip of `notes` (`start`, `length`, `key`, `velocity`) on the track of `device`.
    fn clip(&mut self, device: &str, notes: &[(i64, i64, u8, f32)]) {
        self.apply(Command::Song(SongCommand::AddTrack {
            device: key(device),
        }));
        let notes: Vec<Note> = notes
            .iter()
            .map(|&(s, l, k, v)| Note::new(s, l, k, v))
            .collect();
        self.apply(Command::Song(SongCommand::AddClip {
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
        }));
        self.apply(Command::Song(SongCommand::SetSongEnd(BAR)));
        self.apply(Command::Song(SongCommand::SetLoop {
            range: (0, BAR),
            enabled: false,
        }));
    }

    fn synth(&mut self) {
        self.add("atomik.poly-synth", "synth");
        self.set("synth", "env.amp.release", 40.0);
    }
}

const NOTES: [(i64, i64, u8, f32); 4] = [
    (0, PPQ, 48, 0.8),
    (PPQ, PPQ, 52, 0.6),
    (PPQ + PPQ / 2, 2 * PPQ, 60, 0.7),
    (2 * PPQ, PPQ, 55, 0.9),
];

fn bounce(doc: &RackDoc, block: usize, workers: u16) -> Vec<u32> {
    let options = ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    };
    let r = render_doc_in(doc, &default_registry(), &options, None).unwrap();
    r.channels.iter().flatten().map(|x| x.to_bits()).collect()
}

fn through_filter(set: &[(&str, f32)]) -> RackDoc {
    let mut b = Builder::new();
    b.synth();
    b.add("atomik.note-filter", "filter");
    for (p, v) in set {
        b.set("filter", p, *v);
    }
    b.connect(("filter", "notes_out"), ("synth", "notes_in"));
    b.clip("filter", &NOTES);
    b.doc.doc().clone()
}

fn straight(notes: &[(i64, i64, u8, f32)]) -> RackDoc {
    let mut b = Builder::new();
    b.synth();
    b.clip("synth", notes);
    b.doc.doc().clone()
}

#[test]
fn note_filter_transpose_bounces_like_the_transposed_clip_into_the_synth() {
    let up: Vec<_> = NOTES
        .iter()
        .map(|&(s, l, k, v)| (s, l, k + 12, v))
        .collect();
    let direct = bounce(&straight(&up), 512, 0);
    assert!(
        direct.iter().any(|b| f32::from_bits(*b).abs() > 1e-3),
        "audible"
    );
    let filtered = through_filter(&[("transpose", 12.0)]);
    assert!(
        bounce(&filtered, 512, 0) == direct,
        "+12 equals the shifted clip"
    );
    // The device is not a no-op: unshifted notes sound different.
    assert!(bounce(&through_filter(&[]), 512, 0) != direct);
}

#[test]
fn note_filter_bounce_is_bit_identical_across_block_sizes_and_workers() {
    let doc = through_filter(&[
        ("transpose", 7.0),
        ("velocity.curve", 0.5),
        ("key.high", 58.0),
    ]);
    let reference = bounce(&doc, 512, 0);
    for (block, workers) in [(64, 0), (333, 0), (512, 2), (1000, 3)] {
        assert!(
            bounce(&doc, block, workers) == reference,
            "block {block}, workers {workers}"
        );
    }
}

#[test]
fn note_filter_key_range_bounces_like_the_clip_without_the_other_notes() {
    let kept: Vec<_> = NOTES.iter().copied().filter(|n| n.2 <= 55).collect();
    let direct = bounce(&straight(&kept), 512, 0);
    let filtered = through_filter(&[("key.high", 55.0)]);
    assert!(bounce(&filtered, 512, 0) == direct);
}
