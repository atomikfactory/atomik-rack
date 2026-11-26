//! The named scales of the song in a bounce: two CV Quantizers that name one scale follow it, an
//! edit of the scale reaches both, and a quantizer whose scale the song lacks keeps its own.
//!
//! Seeded noise is held at a steady rate, quantized twice (the second one an octave up) and the
//! two results are added into the level of a synth that plays one long note, so the bounce depends
//! on what both quantizers do. The same rack with the scale written into each quantizer as its
//! own custom scale is the reference.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::PPQ;
use atomik_model::{
    Cable, Clip, ClipContent, ClipId, Command, DeviceKey, Document, Harmony, Note, RackDoc,
    SocketRef, SongCommand, SongScale, empty_doc,
};
use atomik_render::default_registry;
use atomik_render::project::ProjectOptions;
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

/// The scales of the tests: a pentatonic and a triad.
const PENTATONIC: u16 = 0b10_1001_0101;
const TRIAD: u16 = 0b1001_0001;

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

    /// One long note on the track of `device`, one bar.
    fn note(&mut self, device: &str) {
        self.apply(Command::Song(SongCommand::AddTrack {
            device: key(device),
        }));
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
                param_lanes: Vec::new(),
                midi: Default::default(),
                groove: None,
                follow_song_key: true,
                controllers: Vec::new(),
                name: None,
                color: None,
                grid: None,
                content: ClipContent::Notes(vec![Note::new(0, BAR, 52, 0.9)].into()),
            },
        }));
        self.apply(Command::Song(SongCommand::SetSongEnd(BAR)));
        self.apply(Command::Song(SongCommand::SetLoop {
            range: (0, BAR),
            enabled: false,
        }));
    }
}

fn scale_set(mask: u16) -> Harmony {
    Harmony::default()
        .with_scale(SongScale::new(1, "Test", mask))
        .unwrap()
}

/// The rack: noise held 16 times a second, two quantizers, their sum as the synth's level.
/// `shared` is how the quantizers get their scale: `Some(id)` names the song scale, `None` gives
/// each one `own` as its custom scale.
fn rack(shared: Option<u8>, own: u16) -> Builder {
    let mut b = Builder::new();
    b.add("atomik.poly-synth", "synth");
    b.set("synth", "env.amp.release", 40.0);
    b.add("atomik.sample-hold", "hold");
    b.set("hold", "clock", 1.0);
    b.set("hold", "rate", 16.0);
    b.add("atomik.cv-quantizer", "qa");
    b.add("atomik.cv-quantizer", "qb");
    b.set("qb", "transpose", 12.0);
    for q in ["qa", "qb"] {
        match shared {
            Some(id) => {
                b.set(q, "scale.source", 2.0);
                b.set(q, "scale.id", f32::from(id));
            }
            None => {
                b.set(q, "scale.custom", 1.0);
                for pc in 0..12 {
                    let on = own & (1 << pc) != 0;
                    b.set(q, &format!("scale.note.{pc}"), if on { 1.0 } else { 0.0 });
                }
            }
        }
    }
    b.add("atomik.cv-math", "sum");
    b.connect(("hold", "cv_out"), ("qa", "pitch_cv_in"));
    b.connect(("hold", "cv_out"), ("qb", "pitch_cv_in"));
    b.connect(("qa", "pitch_cv_out"), ("sum", "a_in"));
    b.connect(("qb", "pitch_cv_out"), ("sum", "b_in"));
    b.connect(("sum", "out"), ("synth", "level_cv_in"));
    b.note("synth");
    b
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

fn with_scale(mask: u16) -> RackDoc {
    let mut b = rack(Some(1), 0);
    b.apply(Command::Song(SongCommand::SetHarmony(scale_set(mask))));
    b.doc.doc().clone()
}

fn with_own_scale(mask: u16) -> RackDoc {
    rack(None, mask).doc.doc().clone()
}

#[test]
fn two_quantizers_that_name_a_song_scale_play_as_if_each_had_that_scale_of_its_own() {
    for mask in [PENTATONIC, TRIAD] {
        let shared = bounce(&with_scale(mask), 512, 0);
        assert!(
            shared.iter().any(|b| f32::from_bits(*b).abs() > 1e-3),
            "silent"
        );
        assert!(
            shared == bounce(&with_own_scale(mask), 512, 0),
            "mask {mask:#b}: shared scale and own scale differ"
        );
    }
    assert!(
        bounce(&with_scale(PENTATONIC), 512, 0) != bounce(&with_scale(TRIAD), 512, 0),
        "the scale changes what plays"
    );
}

#[test]
fn editing_the_song_scale_reaches_every_quantizer_that_names_it() {
    let mut b = rack(Some(1), 0);
    b.apply(Command::Song(SongCommand::SetHarmony(scale_set(
        PENTATONIC,
    ))));
    let before = bounce(b.doc.doc(), 512, 0);
    // One edit of the scale, one command.
    let edited = b
        .doc
        .doc()
        .song
        .harmony
        .with_scale(SongScale::new(1, "Test", TRIAD))
        .unwrap();
    b.apply(Command::Song(SongCommand::SetHarmony(edited)));
    let after = bounce(b.doc.doc(), 512, 0);
    assert!(before != after, "the edit changes the bounce");
    assert!(
        after == bounce(&with_own_scale(TRIAD), 512, 0),
        "both quantizers follow the edit"
    );
    // One undo takes it back.
    b.doc.undo().unwrap();
    assert!(bounce(b.doc.doc(), 512, 0) == before);
}

#[test]
fn a_scale_the_song_lacks_leaves_the_quantizer_on_its_own_and_blocks_do_not_matter() {
    // The song has no scale 7: the quantizers fall back to their own custom scale.
    let mut b = rack(None, PENTATONIC);
    for q in ["qa", "qb"] {
        b.set(q, "scale.source", 2.0);
        b.set(q, "scale.id", 7.0);
    }
    b.apply(Command::Song(SongCommand::SetHarmony(scale_set(TRIAD))));
    let doc = b.doc.doc().clone();
    let reference = bounce(&doc, 512, 0);
    assert!(reference == bounce(&with_own_scale(PENTATONIC), 512, 0));
    let named = with_scale(TRIAD);
    let shared = bounce(&named, 512, 0);
    for (block, workers) in [(64, 0), (333, 0), (1000, 3)] {
        assert!(
            bounce(&doc, block, workers) == reference,
            "block {block}, workers {workers}"
        );
        assert!(
            bounce(&named, block, workers) == shared,
            "block {block}, workers {workers}"
        );
    }
}
