//! Fixtures for the tab editor: a song with one note clip (id 1) on a track whose name picks the
//! instrument, with or without a tab sidecar.

use super::song::{notes_clip, track};
use super::*;
use atomik_model::{
    ClipId, Command, Document, Lane, MidiData, Note, RackDoc, SongCommand, TabData,
};
use atomik_ui::kurbo::Rect;
use atomik_ui::{TabEditorView, Theme, UiAction};
use std::sync::Arc;

pub const PPQ: i64 = 15_360;
pub const TAB_BOUNDS: Rect = Rect::new(0.0, 0.0, 1000.0, 420.0);

/// A song with one track named `name` (a poly synth) and one note clip of two bars.
pub fn tab_doc(name: &str, notes: Vec<Note>, tab: Option<TabData>) -> RackDoc {
    let mut doc = reference_doc();
    doc.devices = vec![device("gtr", "atomik.poly-synth", name)];
    doc.cables.clear();
    let mut clip = notes_clip(1, 0, 8 * PPQ, notes);
    clip.midi = Arc::new(MidiData {
        tab,
        ..MidiData::default()
    });
    doc.song_mut().tracks = vec![track("gtr", vec![Lane::notes().with_clips(vec![clip])])];
    doc.song_mut().next_clip_id = 2;
    doc
}

/// A tab editor open on clip 1.
pub fn tab_editor(doc: &RackDoc, reg: &atomik_device_sdk::Registry) -> TabEditorView {
    let mut v = TabEditorView::new(Theme::dark());
    v.set_bounds(TAB_BOUNDS);
    v.open(ClipId(1));
    v.sync(doc, reg);
    v
}

/// Applies the commands among `actions` (ending merges on `EndMerge`); returns the merge id of
/// each tab edit (a batch of content and data).
pub fn apply_tab(
    document: &mut Document,
    reg: &atomik_device_sdk::Registry,
    actions: &[UiAction],
) -> Vec<Option<u64>> {
    let mut merges = Vec::new();
    for a in actions {
        match a {
            UiAction::Command(c) => {
                if let Command::Batch(items) = c {
                    for i in items {
                        if let Command::Song(SongCommand::SetClipMidi { merge, .. }) = i {
                            merges.push(*merge);
                        }
                    }
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

/// The clip with id 1.
pub fn clip1(doc: &RackDoc) -> atomik_model::Clip {
    doc.song.clip(ClipId(1)).expect("clip 1").2.clone()
}
