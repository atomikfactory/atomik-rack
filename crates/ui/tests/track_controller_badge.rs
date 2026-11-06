//! The CC badge on the header of a note track that holds controller lanes of its own
//! (`Track::controllers`): present only then, a click asks for the lanes to open in the piano
//! roll, the name gives it room, and a compact row has none.

mod support;

use atomik_model::controller::{ControllerKind, ControllerLane};
use atomik_model::{AutomationPoint, Curve, Document};
use atomik_ui::keymap::CommandOutcome;
use atomik_ui::kurbo::Point;
use atomik_ui::timeline::Snap;
use atomik_ui::{ArrangementView, Modifiers, PointerButton, TrackButton, UiAction, UiEvent};
use support::song::*;
use support::*;

fn lanes() -> Vec<ControllerLane> {
    vec![ControllerLane::new(
        ControllerKind::PitchBend,
        vec![AutomationPoint {
            tick: 0,
            value: 0.5,
            curve: Curve::Step,
        }],
    )]
}

fn setup(own: Vec<ControllerLane>) -> (Document, atomik_device_sdk::Registry, ArrangementView) {
    let reg = registry();
    let mut doc = song_doc();
    doc.song_mut().tracks[0].controllers = own;
    let d = Document::new(doc);
    let mut v = arrangement(d.doc(), &reg);
    v.set_snap(Snap::Bar);
    (d, reg, v)
}

fn click(
    d: &Document,
    reg: &atomik_device_sdk::Registry,
    v: &mut ArrangementView,
    at: Point,
) -> Vec<UiAction> {
    let down = UiEvent::PointerDown {
        pos: at,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    };
    let up = UiEvent::PointerUp {
        pos: at,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    };
    [down, up]
        .iter()
        .flat_map(|e| v.handle(e, d.doc(), reg))
        .collect()
}

#[test]
fn only_a_track_with_controller_lanes_has_the_badge() {
    let (_d, _reg, v) = setup(lanes());
    assert!(
        v.track_button_center(&key("bass"), TrackButton::Controllers)
            .is_some()
    );
    for other in ["lead", "keys"] {
        assert!(
            v.track_button_center(&key(other), TrackButton::Controllers)
                .is_none(),
            "{other} holds no lanes of its own"
        );
    }
    let (_d, _reg, v) = setup(Vec::new());
    assert!(
        v.track_button_center(&key("bass"), TrackButton::Controllers)
            .is_none()
    );
}

#[test]
fn the_badge_has_a_tooltip_and_sits_left_of_the_keyboard_arrow() {
    let (_d, _reg, v) = setup(lanes());
    let badge = v
        .track_button_center(&key("bass"), TrackButton::Controllers)
        .unwrap();
    let keys = v
        .track_button_center(&key("bass"), TrackButton::Keys)
        .unwrap();
    assert!(badge.x < keys.x - 10.0 && (badge.y - keys.y).abs() < 1.0);
    let help = v.help_at(badge).expect("a tooltip");
    assert!(help.text.contains("controller lanes"), "{}", help.text);
    assert!(help.text.len() <= 100, "{}", help.text.len());
}

#[test]
fn a_click_on_the_badge_asks_to_open_the_track_lanes() {
    let (d, reg, mut v) = setup(lanes());
    let at = v
        .track_button_center(&key("bass"), TrackButton::Controllers)
        .unwrap();
    let actions = click(&d, &reg, &mut v, at);
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, UiAction::OpenTrackControllers(k) if *k == key("bass"))),
        "{actions:?}"
    );
    assert!(
        actions
            .iter()
            .all(|a| !matches!(a, UiAction::Command(_) | UiAction::OpenPianoRoll(_))),
        "the badge edits nothing and opens no clip: {actions:?}"
    );
}

#[test]
fn the_badge_follows_the_document() {
    let (mut d, reg, mut v) = setup(Vec::new());
    assert!(
        v.track_button_center(&key("bass"), TrackButton::Controllers)
            .is_none()
    );
    d.apply(
        atomik_model::Command::Song(atomik_model::SongCommand::SetTrackControllers {
            device: key("bass"),
            lanes: lanes(),
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    v.sync(d.doc(), &reg);
    assert!(
        v.track_button_center(&key("bass"), TrackButton::Controllers)
            .is_some()
    );
    d.undo().unwrap();
    v.sync(d.doc(), &reg);
    assert!(
        v.track_button_center(&key("bass"), TrackButton::Controllers)
            .is_none()
    );
}

#[test]
fn a_compact_row_has_no_badge() {
    let (mut d, reg, mut v) = setup(lanes());
    v.set_selected_track(Some(key("bass")));
    let CommandOutcome::Handled(actions) =
        v.command("sequencer.track_height", Some("small"), d.doc(), &reg)
    else {
        panic!("the height command is handled");
    };
    for a in actions {
        if let UiAction::Command(c) = a {
            d.apply(c, &reg).unwrap();
        }
    }
    v.sync(d.doc(), &reg);
    assert!(
        v.track_button_center(&key("bass"), TrackButton::Controllers)
            .is_none()
    );
}
