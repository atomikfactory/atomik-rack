//! Drum fixtures: a rack with one drum machine `drums` (pad 1 kick, pad 2 snare with two
//! velocity layers, pad 5 a missing-name asset), a note track with one clip, and a synced
//! step editor over it.

use super::song::{notes_clip, track};
use super::*;
use atomik_device_sdk::Registry;
use atomik_model::{
    AssetMeta, Clip, ClipId, Command, Document, Lane, LaneKind, Note, RackDoc, SongCommand,
    StepGrid, StepResolution, ZoneDoc, pad_key,
};
use atomik_ui::kurbo::Rect;
use atomik_ui::{StepEditorView, Theme, UiAction};

pub const KICK: &str = "d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1d1";
pub const SNARE: &str = "d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2d2";
pub const SNARE_HARD: &str = "d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3d3";
pub const FRAMES: u64 = 24_000;
pub const PPQ: i64 = 15_360;
pub const S16: i64 = PPQ / 4;
pub const S32: i64 = PPQ / 8;
pub const EDITOR_BOUNDS: Rect = Rect::new(0.0, 0.0, 1000.0, 560.0);
pub const CLIP: ClipId = ClipId(7);

fn meta(name: &str) -> AssetMeta {
    AssetMeta {
        channels: 1,
        sample_rate: 48_000,
        frames: FRAMES,
        original_name: name.to_string(),
    }
}

/// A zone of one asset on `pad`'s key, over velocities `vel`.
pub fn pad_zone(asset: &str, pad: u8, vel: (u8, u8)) -> ZoneDoc {
    let mut z = ZoneDoc::new(asset, FRAMES);
    z.key_lo = pad_key(pad);
    z.key_hi = pad_key(pad);
    z.root_key = pad_key(pad);
    (z.vel_lo, z.vel_hi) = vel;
    z
}

pub fn drum_zones() -> Vec<ZoneDoc> {
    vec![
        pad_zone(KICK, 1, (1, 127)),
        pad_zone(SNARE, 2, (1, 79)),
        pad_zone(SNARE_HARD, 2, (80, 127)),
    ]
}

pub fn grid(res: StepResolution, steps: u16, swing: u8) -> StepGrid {
    StepGrid {
        resolution: res,
        steps,
        swing,
    }
}

pub fn step_note(step: i64, pad: u8, velocity: f32) -> Note {
    Note {
        tick: step * S16,
        length: S16,
        key: pad_key(pad),
        velocity,
    }
}

/// The reference rack plus drum machine `drums` (assets, zones) with a note track holding
/// `clip` (id 7).
pub fn drum_doc_with(clip: Option<Clip>) -> RackDoc {
    let mut doc = reference_doc();
    let mut d = device("drums", "atomik.drum-machine", "Drum Machine");
    d.zones = drum_zones();
    doc.devices.insert(0, d);
    doc.assets.insert(KICK.into(), meta("kick_808.wav"));
    doc.assets.insert(SNARE.into(), meta("snare_soft.flac"));
    doc.assets
        .insert(SNARE_HARD.into(), meta("snare_hard.flac"));
    let clips = clip.into_iter().collect();
    doc.song.tracks.push(track(
        "drums",
        vec![Lane {
            kind: LaneKind::Notes,
            clips,
        }],
    ));
    doc.song.next_clip_id = 100;
    doc
}

/// A one-bar clip with a 16 x 1/16 grid and a four-on-the-floor kick, backbeat snare.
pub fn beat_clip() -> Clip {
    let mut notes = Vec::new();
    for k in [0, 4, 8, 12] {
        notes.push(step_note(k, 1, 80.0 / 127.0));
    }
    for k in [4, 12] {
        notes.push(step_note(k, 2, 120.0 / 127.0));
    }
    for k in [2, 6, 10, 14] {
        notes.push(step_note(k, 6, 40.0 / 127.0));
    }
    notes.sort_by_key(|n| (n.tick, n.key));
    let mut c = notes_clip(7, 0, 16 * S16, notes);
    c.grid = Some(grid(StepResolution::Sixteenth, 16, 0));
    c
}

pub fn drum_doc() -> RackDoc {
    drum_doc_with(Some(beat_clip()))
}

pub fn editor(doc: &RackDoc, reg: &Registry, clip: Option<ClipId>) -> StepEditorView {
    let mut v = StepEditorView::new(Theme::dark());
    v.set_bounds(EDITOR_BOUNDS);
    v.open(key("drums"), clip);
    v.sync(doc, reg);
    v
}

/// Applies every command among `actions` to `document`, asserting the model accepts it, and
/// ends merges on `EndMerge`. Returns the merge ids of the content / grid commands seen.
pub fn apply_all(
    document: &mut Document,
    reg: &Registry,
    actions: &[UiAction],
) -> Vec<Option<u64>> {
    let mut merges = Vec::new();
    for a in actions {
        match a {
            UiAction::Command(c) => {
                match c {
                    Command::Song(SongCommand::SetClipContent { merge, .. })
                    | Command::Song(SongCommand::SetClipGrid { merge, .. }) => merges.push(*merge),
                    _ => {}
                }
                document
                    .apply(c.clone(), reg)
                    .unwrap_or_else(|e| panic!("model rejected {c:?}: {e}"));
            }
            UiAction::EndMerge => document.end_merge(),
            _ => {}
        }
    }
    merges
}

/// The clip with id 7 in a document.
pub fn clip_of(doc: &RackDoc) -> Clip {
    doc.song
        .tracks
        .iter()
        .flat_map(|t| t.lanes.iter())
        .flat_map(|l| l.clips.iter())
        .find(|c| c.id == CLIP)
        .cloned()
        .expect("clip 7")
}

/// The `(key, velocity, on)` triples of the audition actions among `actions`.
pub fn auditions(actions: &[UiAction]) -> Vec<(u8, u8, bool)> {
    actions
        .iter()
        .filter_map(|a| match a {
            UiAction::AuditionVelocity {
                device,
                key,
                velocity,
                on,
            } => {
                assert_eq!(device, &super::key("drums"));
                Some((*key, *velocity, *on))
            }
            _ => None,
        })
        .collect()
}

/// Every `SetClipContent` note list among `actions`, with its merge id.
pub fn note_lists(actions: &[UiAction]) -> Vec<(Vec<Note>, Option<u64>)> {
    actions
        .iter()
        .filter_map(|a| match a {
            UiAction::Command(Command::Song(SongCommand::SetClipContent {
                content: atomik_model::ClipContent::Notes(n),
                merge,
                ..
            })) => Some((n.clone(), *merge)),
            _ => None,
        })
        .collect()
}
