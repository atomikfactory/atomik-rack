//! Chase from a second note lane, through the real engine: a note on lane 2 that spans the loop
//! start is started again at the loop wrap exactly like the same note on lane 1, and a track
//! with chase off keeps it silent.

use atomik_core::PPQ;
use atomik_model::song::ClipId;
use atomik_model::{
    Clip, ClipContent, Command, CreateOptions, CreateSource, DeviceKey, Document, Note, RackDoc,
    SongCommand, WorkflowCommand, new_project_doc,
};
use atomik_render::project::{ProjectOptions, render_doc};
use atomik_render::{Rendered, default_registry};
use std::num::NonZeroUsize;

const BAR: i64 = 4 * PPQ;
const SYNTH: &str = "atomik.poly-synth";
/// Frames per bar at 120 BPM and 48 kHz.
const BAR_FRAMES: usize = 96_000;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn apply(d: &mut Document, c: SongCommand) {
    let c = Command::Song(c);
    d.apply(c.clone(), &default_registry())
        .unwrap_or_else(|e| panic!("{c:?}: {e}"));
}

/// A synth track whose long note (bars 0 to 3) sits on note lane `lane` of `lanes`, with the loop
/// on bar 2 so the note spans the loop start.
fn project(lanes: usize, lane: usize, chase: bool) -> RackDoc {
    let mut d = Document::new(new_project_doc());
    d.apply(
        Command::Workflow(WorkflowCommand::CreateInstrument {
            index: usize::MAX,
            source: CreateSource::Type(SYNTH.into()),
            key: Some(key("lead")),
            options: CreateOptions::default(),
        }),
        &default_registry(),
    )
    .unwrap();
    apply(
        &mut d,
        SongCommand::SetTrackFlags {
            device: key("lead"),
            muted: false,
            solo: false,
            armed: false,
        },
    );
    apply(
        &mut d,
        SongCommand::SetTrackChase {
            device: key("lead"),
            chase,
        },
    );
    for _ in 1..lanes {
        apply(
            &mut d,
            SongCommand::AddNoteLane {
                device: key("lead"),
                name: None,
            },
        );
    }
    apply(
        &mut d,
        SongCommand::AddClip {
            device: key("lead"),
            lane,
            clip: Clip {
                id: ClipId(0),
                start: 0,
                length: 3 * BAR,
                offset: 0,
                loop_length: None,
                muted: false,
                scale: None,
                controllers: Vec::new(),
                name: None,
                color: None,
                grid: None,
                content: ClipContent::Notes(vec![Note::new(0, 3 * BAR, 69, 0.8)].into()),
            },
        },
    );
    apply(&mut d, SongCommand::SetSongEnd(3 * BAR));
    apply(
        &mut d,
        SongCommand::SetLoop {
            range: (BAR, 2 * BAR),
            enabled: true,
        },
    );
    d.doc().clone()
}

/// Two passes of the loop: the render plays the loop once, wraps and plays it again.
fn bounce_two_passes(doc: &RackDoc, block: usize) -> Rendered {
    let options = ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers: 0,
        loop_count: Some(2),
    };
    render_doc(doc, &default_registry(), &options).unwrap()
}

/// The largest sample of the left channel over the second half of the second pass.
fn late_peak(r: &Rendered) -> f32 {
    let from = 2 * BAR_FRAMES + BAR_FRAMES / 2;
    r.channels[0][from..from + BAR_FRAMES / 4]
        .iter()
        .fold(0.0_f32, |m, x| m.max(x.abs()))
}

#[test]
fn a_note_spanning_the_loop_start_on_the_second_lane_is_chased_at_the_wrap() {
    let on_two = bounce_two_passes(&project(2, 1, true), 512);
    assert!(late_peak(&on_two) > 0.01, "the chased note sounds again");
    // Exactly what the same note does on the only lane, whatever the block size.
    let on_one = bounce_two_passes(&project(1, 0, true), 512);
    assert_eq!(on_two, on_one);
    assert_eq!(bounce_two_passes(&project(2, 1, true), 64), on_one);
}

#[test]
fn a_track_with_chase_off_keeps_the_note_of_the_second_lane_silent_after_the_wrap() {
    let off = bounce_two_passes(&project(2, 1, false), 512);
    assert!(
        late_peak(&off) < 1e-4,
        "no chase, no note: {}",
        late_peak(&off)
    );
}
