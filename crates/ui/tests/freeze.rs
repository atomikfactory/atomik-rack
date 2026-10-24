//! A frozen track in the arrangement: the ice mark in its header, its render drawn in place of
//! its clips (and nothing to grab where the clips were), and the freeze commands in the registry.

mod support;

use atomik_model::{AudioClipContent, ClipId, Frozen, RackDoc};
use atomik_ui::kurbo::Point;
use atomik_ui::{ArrangementView, Modifiers, PointerButton, UiEvent};
use support::audio::*;
use support::song::*;
use support::*;

/// [`audio_doc`] with the synth track `bass` frozen: its render is the stereo asset, two bars
/// from the start, where its note clip 1 is.
fn frozen_doc() -> RackDoc {
    let mut doc = audio_doc();
    doc.song.tracks[0].frozen = Some(Frozen {
        start: 0,
        length: 2 * BAR,
        audio: AudioClipContent::new(HASH_A, 0),
        effects: Vec::new(),
    });
    doc
}

fn paint_json(v: &mut ArrangementView, doc: &RackDoc, reg: &atomik_device_sdk::Registry) -> String {
    let mut dl = atomik_ui::DisplayList::new();
    v.paint(&mut dl, doc, reg);
    dl.to_json()
}

fn press(v: &mut ArrangementView, doc: &RackDoc, reg: &atomik_device_sdk::Registry, at: Point) {
    v.handle(
        &UiEvent::PointerDown {
            pos: at,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        },
        doc,
        reg,
    );
    v.handle(
        &UiEvent::PointerUp {
            pos: at,
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
        },
        doc,
        reg,
    );
}

#[test]
fn a_frozen_track_shows_its_render_with_the_ice_mark() {
    let reg = registry();
    let plain_doc = audio_doc();
    let mut plain = audio_arrangement(&plain_doc, &reg);
    let before = paint_json(&mut plain, &plain_doc, &reg);
    assert!(!before.contains("(frozen)"));

    let doc = frozen_doc();
    let mut v = audio_arrangement(&doc, &reg);
    let after = paint_json(&mut v, &doc, &reg);
    assert!(
        after.contains("Bass (frozen)"),
        "the render is labelled in the lane"
    );
    assert_ne!(before, after);
    // The mark is drawn in the header and in the render's clip, in ice blue.
    let ice = serde_json::to_string(&0.55f32).unwrap();
    assert!(after.contains(&ice), "ice blue is used");
}

#[test]
fn nothing_can_be_grabbed_where_a_frozen_tracks_clips_are() {
    let reg = registry();
    // Unfrozen, a press in the note clip selects it.
    let doc = audio_doc();
    let mut v = audio_arrangement(&doc, &reg);
    let r = v.clip_rect(ClipId(1)).expect("the note clip is laid out");
    press(
        &mut v,
        &doc,
        &reg,
        Point::new(r.center().x, r.center().y + 6.0),
    );
    assert_eq!(v.selected_clips(), vec![ClipId(1)]);

    // Frozen, the same press selects nothing: the clip is hidden behind the render.
    let doc = frozen_doc();
    let mut v = audio_arrangement(&doc, &reg);
    press(
        &mut v,
        &doc,
        &reg,
        Point::new(r.center().x, r.center().y + 6.0),
    );
    assert!(v.selected_clips().is_empty());
}

#[test]
fn the_tracks_a_command_works_on_are_the_selected_ones_or_those_of_the_selected_clips() {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    assert!(v.command_tracks().is_empty());
    v.set_selected_clips(&[ClipId(10), ClipId(11)]);
    assert_eq!(v.command_tracks(), vec![key("audio-track")]);
    v.set_selected_track(Some(key("bass")));
    assert_eq!(v.command_tracks(), vec![key("bass")]);
}

#[test]
fn the_freeze_commands_are_registered_with_menu_items() {
    let r = atomik_ui::keymap::Registry::bundled();
    for id in [
        "sequencer.freeze_track",
        "sequencer.freeze_track_to_device",
        "sequencer.freeze_tail",
        "sequencer.freeze_cancel",
        "sequencer.unfreeze_track",
        "sequencer.unfreeze_track_keep",
        "sequencer.bounce_in_place",
        "sequencer.bounce_in_place_mute",
    ] {
        assert!(r.validate(id, None), "{id}");
        assert!(r.get(id).unwrap().menu, "{id} has a menu item");
    }
}
