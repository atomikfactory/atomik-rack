//! Markers and ranges in the app core: an edit is one undo step that never reaches the engine,
//! the commands of the arrangement view run through the core, and a saved project brings the
//! markers back.

use super::*;
use crate::sink::RecordingSink;
use atomik_core::PPQ;
use atomik_model::{Marker, RangeMarker, empty_doc};
use atomik_ui::MarkSel;

const BAR: i64 = 4 * PPQ;

fn core() -> AppCore<RecordingSink> {
    AppCore::with_doc(RecordingSink::default(), empty_doc(), None)
}

fn marker(id: u32, tick: i64, name: &str) -> Marker {
    Marker {
        id,
        tick,
        name: name.into(),
        color: None,
    }
}

fn range(id: u32, start: i64, end: i64, name: &str) -> RangeMarker {
    RangeMarker {
        id,
        start,
        end,
        name: name.into(),
        color: None,
    }
}

#[test]
fn a_marker_edit_is_one_undo_step_and_the_engine_hears_nothing() {
    let mut c = core();
    c.sink_mut().take();
    c.apply_command(Command::Song(SongCommand::SetMarkers(vec![marker(
        1, BAR, "Drop",
    )])))
    .unwrap();
    c.apply_command(Command::Song(SongCommand::SetRanges(vec![range(
        1,
        0,
        2 * BAR,
        "Intro",
    )])))
    .unwrap();
    assert!(
        c.sink_mut().take().is_empty(),
        "markers and ranges are not engine state"
    );
    assert!(c.is_dirty());
    c.undo();
    assert!(c.doc().song.ranges.is_empty());
    assert_eq!(c.doc().song.markers.len(), 1);
    c.undo();
    assert!(c.doc().song.markers.is_empty());
    c.redo();
    c.redo();
    assert_eq!(c.doc().song.markers[0].name, "Drop");
    assert_eq!(c.doc().song.ranges[0].name, "Intro");
    assert!(c.sink_mut().take().is_empty(), "undo and redo neither");
}

#[test]
fn a_dragged_marker_is_one_undo_step() {
    let mut c = core();
    c.apply_command(Command::Song(SongCommand::SetMarkers(vec![marker(
        1, BAR, "A",
    )])))
    .unwrap();
    c.document.end_merge();
    for x in 1..=6 {
        c.handle_action(atomik_ui::UiAction::Command(Command::Song(
            SongCommand::DragMarkers {
                markers: vec![marker(1, BAR + x * 100, "A")],
                merge: 9,
            },
        )));
    }
    c.handle_action(atomik_ui::UiAction::EndMerge);
    assert_eq!(c.doc().song.markers[0].tick, BAR + 600);
    c.undo();
    assert_eq!(c.doc().song.markers[0].tick, BAR);
}

#[test]
fn the_view_commands_edit_the_document_through_the_core() {
    let mut c = core();
    c.transport_action(atomik_ui::TransportAction::Locate(3 * BAR));
    assert!(
        c.pane_command(Pane::Arrangement, "transport.add_marker", None)
            .is_some()
    );
    assert_eq!(c.doc().song.markers, vec![marker(1, 3 * BAR, "Marker 1")]);
    c.arrangement_mut()
        .set_selected_marker(Some(MarkSel::Marker(1)));
    assert!(
        c.pane_command(Pane::Arrangement, "sequencer.marker_color", Some("red"))
            .is_some()
    );
    assert!(c.doc().song.markers[0].color.is_some());
    c.transport_action(atomik_ui::TransportAction::Locate(0));
    assert!(
        c.pane_command(Pane::Arrangement, "transport.next_marker", None)
            .is_some()
    );
    assert_eq!(c.position(), 3 * BAR);
    // The loop range becomes a range marker; deleting the selected range is undoable.
    assert!(
        c.pane_command(Pane::Arrangement, "transport.add_range_from_loop", None)
            .is_some()
    );
    assert_eq!(c.doc().song.ranges.len(), 1);
    assert_eq!(
        (c.doc().song.ranges[0].start, c.doc().song.ranges[0].end),
        c.doc().song.loop_range
    );
    assert!(
        c.pane_command(Pane::Arrangement, "edit.delete", None)
            .is_some()
    );
    assert!(c.doc().song.ranges.is_empty());
    c.undo();
    assert_eq!(c.doc().song.ranges.len(), 1);
}

#[test]
fn the_markers_come_back_from_a_saved_project() {
    let mut c = core();
    c.apply_command(Command::Song(SongCommand::SetMarkers(vec![
        marker(1, BAR, "Drop"),
        marker(2, 5 * BAR, "Break \"two\""),
    ])))
    .unwrap();
    c.apply_command(Command::Song(SongCommand::SetRanges(vec![range(
        1,
        BAR,
        5 * BAR,
        "Verse",
    )])))
    .unwrap();
    let text = atomik_model::save_project(c.doc());
    let mut d = core();
    d.open_str(&text, None).unwrap();
    assert_eq!(d.doc().song.markers, c.doc().song.markers);
    assert_eq!(d.doc().song.ranges, c.doc().song.ranges);
    assert_eq!(atomik_model::save_project(d.doc()), text);
}
