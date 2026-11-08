//! Tweaking a param of a device inside a container never moves the selected song track, the
//! armed tracks, the selected mix channel or the track the live keyboard plays.

use super::*;
use crate::sink::RecordingSink;
use atomik_model::{
    ContainerCommand, CreateOptions, CreateSource, DeviceKey, SongCommand, WorkflowCommand,
    empty_doc,
};

const SYNTH: &str = "atomik.poly-synth";
const CHORD: &str = "atomik.key-chord";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn instrument(c: &mut AppCore<RecordingSink>) {
    c.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
        index: usize::MAX,
        source: CreateSource::Type(SYNTH.into()),
        key: None,
        options: CreateOptions::default(),
    }))
    .unwrap();
}

/// Everything a param tweak must leave alone.
#[derive(Debug, PartialEq)]
struct Selection {
    track: Option<DeviceKey>,
    strips: Vec<DeviceKey>,
    armed: Vec<DeviceKey>,
    keyboard: Option<DeviceKey>,
    cell: Option<crate::target::Target>,
}

fn selection(c: &AppCore<RecordingSink>) -> Selection {
    Selection {
        track: c.arrangement().selected_track().cloned(),
        strips: c.console().selected_strips(),
        armed: c
            .doc()
            .song
            .tracks
            .iter()
            .filter(|t| t.armed)
            .map(|t| t.device.clone())
            .collect(),
        keyboard: c.notes_target_device().map(|d| d.key.clone()),
        cell: c.note_target_cell().get(),
    }
}

/// Tracks `poly-synth`, `container` (a chord device and a synth inside) and `poly-synth-2`.
/// `poly-synth` and `playing` are armed, `playing` is selected in the arrangement, the console
/// and (before the rack enters the container) the rack; the rack then shows the container's inside.
fn core(playing: &str) -> AppCore<RecordingSink> {
    let mut c = AppCore::with_registry(RecordingSink::new(), default_registry(), empty_doc(), None);
    instrument(&mut c);
    instrument(&mut c);
    c.select(Some(key("poly-synth-2")));
    assert!(c.combine_selection());
    for (type_id, k) in [(CHORD, "chord"), (SYNTH, "synth")] {
        c.apply_command(Command::Container(ContainerCommand::AddInner {
            container: key("container"),
            index: usize::MAX,
            type_id: type_id.into(),
            key: Some(key(k)),
            auto_split: false,
        }))
        .unwrap();
    }
    instrument(&mut c);
    c.arrangement_mut().set_selected_track(Some(key(playing)));
    c.follow_arrangement_track(&UiEvent::PointerMove {
        pos: Point::ZERO,
        modifiers: atomik_ui::Modifiers::NONE,
    });
    let flags: Vec<Command> = c
        .doc()
        .song
        .tracks
        .iter()
        .map(|t| {
            Command::Song(SongCommand::SetTrackFlags {
                device: t.device.clone(),
                muted: false,
                solo: false,
                armed: t.device.0 == "poly-synth" || t.device.0 == playing,
            })
        })
        .collect();
    c.apply_command(Command::Batch(flags)).unwrap();
    c.view_mut().set_scope(&[key("container")]);
    c.after_view_change();
    c
}

fn tweak(c: &mut AppCore<RecordingSink>, inner: &str, param: &str, value: f32) {
    let device = key(&format!("container/{inner}"));
    c.handle_action(UiAction::Select(device.clone()));
    c.handle_action(UiAction::ParamTouched {
        device: device.clone(),
        param: param.into(),
    });
    c.handle_action(UiAction::Command(Command::SetParam {
        key: device,
        param: param.into(),
        value,
        merge: None,
    }));
    c.frame();
}

#[test]
fn the_fixture_plays_the_selected_armed_track_not_the_first_armed_one() {
    for playing in ["poly-synth-2", "container"] {
        let c = core(playing);
        let s = selection(&c);
        assert_eq!(s.armed.len(), 2, "{s:?}");
        assert_eq!(s.track, Some(key(playing)), "{s:?}");
        assert_eq!(s.keyboard, Some(key(playing)), "{s:?}");
    }
}

#[test]
fn tweaking_a_param_inside_a_container_keeps_every_selection() {
    for playing in ["poly-synth-2", "container"] {
        for follow in [false, true] {
            let mut c = core(playing);
            c.set_follow_selection(follow);
            let before = selection(&c);
            tweak(&mut c, "synth", "filter.cutoff", 900.0);
            assert_eq!(selection(&c), before, "{playing} follow {follow}");
        }
    }
}

#[test]
fn tweaking_the_chord_device_of_a_container_keeps_every_selection() {
    for playing in ["poly-synth-2", "container"] {
        for follow in [false, true] {
            let mut c = core(playing);
            c.set_follow_selection(follow);
            let before = selection(&c);
            tweak(&mut c, "chord", "chord", 2.0);
            assert_eq!(selection(&c), before, "{playing} follow {follow}");
        }
    }
}

#[test]
fn live_notes_keep_going_to_the_playing_track_while_an_inner_device_is_tweaked() {
    for playing in ["poly-synth-2", "container"] {
        let mut c = core(playing);
        let target = c.note_target_cell().get();
        assert!(target.is_some());
        for value in [500.0, 900.0, 1500.0] {
            tweak(&mut c, "synth", "filter.cutoff", value);
            assert_eq!(c.note_target_cell().get(), target, "{playing}");
        }
    }
}

/// An inner device may carry the key of a top-level device (both are named after their type by
/// default): the view-local key must never stand for the top-level one.
#[test]
fn an_inner_device_named_like_a_top_level_device_never_touches_that_device() {
    for playing in ["poly-synth-2", "container"] {
        let mut c = core(playing);
        c.apply_command(Command::Container(ContainerCommand::AddInner {
            container: key("container"),
            index: usize::MAX,
            type_id: SYNTH.into(),
            key: Some(key("poly-synth")),
            auto_split: false,
        }))
        .unwrap();
        c.set_follow_selection(true);
        // A param of the top-level `poly-synth` was touched earlier.
        c.handle_action(UiAction::ParamTouched {
            device: key("poly-synth"),
            param: "filter.cutoff".into(),
        });
        let (before, doc_before) = (selection(&c), c.doc().song.clone());
        c.handle_action(UiAction::Select(key("container/poly-synth")));
        c.after_view_change();
        assert!(
            !c.show_touched_lane(),
            "{playing}: no param touched on the inner device"
        );
        assert_eq!(selection(&c), before, "{playing}");
        assert!(c.doc().song == doc_before, "{playing}: no lane was added");
    }
}
