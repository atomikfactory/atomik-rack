//! The piano roll in its track mode: the controller lanes a note track holds outside its clips.
//! The roll shows the ruler and the controller area over the whole song, edits go to the track
//! (`SetTrackControllers`), and nothing of the note editor is left to emit a note command.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::controller::{ControllerKind, ControllerLane};
use atomik_model::{AutomationPoint, Command, Curve, DeviceKey, Document, SongCommand};
use atomik_ui::controller_lane::{Chip, PRESETS};
use atomik_ui::keymap::CommandOutcome;
use atomik_ui::kurbo::Point;
use atomik_ui::timeline::Snap;
use atomik_ui::{DisplayList, PianoRollView, UiAction};
use support::song::*;
use support::*;

const BEND: ControllerKind = ControllerKind::PitchBend;
const MOD: ControllerKind = ControllerKind::Cc(1);

fn pt(tick: i64, value: f32, curve: Curve) -> AutomationPoint {
    AutomationPoint { tick, value, curve }
}

fn bass() -> DeviceKey {
    DeviceKey("bass".into())
}

fn setup(lanes: Vec<ControllerLane>) -> (PianoRollView, Document, Registry) {
    let reg = registry();
    let mut doc = song_doc();
    doc.song_mut().tracks[0].controllers = lanes;
    let d = Document::new(doc);
    let mut v = PianoRollView::new(atomik_ui::Theme::dark());
    v.set_bounds(ROLL_BOUNDS);
    v.open_track(bass());
    v.sync(d.doc(), &reg);
    v.set_snap(Snap::Beat);
    (v, d, reg)
}

fn own(d: &Document) -> Vec<ControllerLane> {
    d.doc().song.tracks[0].controllers.clone()
}

fn points(d: &Document, kind: ControllerKind) -> Vec<AutomationPoint> {
    own(d)
        .into_iter()
        .find(|l| l.kind == kind)
        .map(|l| l.points)
        .unwrap_or_default()
}

fn apply(v: &mut PianoRollView, d: &mut Document, reg: &Registry, actions: &[UiAction]) {
    for c in song_commands(actions) {
        d.apply(Command::Song(c), reg).unwrap();
    }
    v.sync(d.doc(), reg);
}

fn gesture(
    v: &mut PianoRollView,
    d: &mut Document,
    reg: &Registry,
    events: &[atomik_ui::UiEvent],
) -> Vec<UiAction> {
    let mut all = Vec::new();
    for e in events {
        let a = v.handle(e, d.doc(), reg);
        apply(v, d, reg, &a);
        all.extend(a);
    }
    all
}

fn at(v: &PianoRollView, kind: ControllerKind, tick: i64, value: f32) -> Point {
    v.controller_point(kind, tick, value)
}

#[test]
fn the_track_mode_gives_the_controller_area_everything_under_the_ruler() {
    let (v, _d, _reg) = setup(vec![]);
    assert_eq!(v.track(), Some(&bass()));
    assert_eq!(v.clip(), None, "no clip is open");
    assert!(v.controllers_open());
    assert!(!v.controllers_folded());
    let area = v.controller_area();
    assert_eq!(area.y1, v.bounds().y1);
    assert!(area.height() > 300.0, "{area:?}");
    // The grid, keys and velocity lane have no room: the area starts at the ruler's foot.
    assert_eq!(v.velocity_rect().y1, area.y0);
}

#[test]
fn the_area_cannot_be_hidden_folded_or_switched_to_messages() {
    let (mut v, d, reg) = setup(vec![]);
    v.set_controllers_open(false);
    v.set_controllers_folded(true);
    v.set_messages_open(true);
    assert!(v.controllers_open());
    assert!(!v.controllers_folded());
    assert!(!v.messages_open());
    assert_eq!(
        v.command("editor.toggle_controllers", None, d.doc(), &reg),
        CommandOutcome::Handled(Vec::new())
    );
    assert!(v.controllers_open());
}

#[test]
fn a_press_in_the_lane_adds_a_point_to_the_track_and_the_drag_is_one_undo_step() {
    let (mut v, mut d, reg) = setup(vec![]);
    let before = d.doc().clone();
    let a = at(&v, BEND, 0, 0.0);
    let b = at(&v, BEND, 2 * PPQ, 0.5);
    let c = at(&v, BEND, 4 * PPQ, -0.5);
    let actions = gesture(&mut v, &mut d, &reg, &[down(a), mv(b), mv(c), up(c)]);
    let sent = song_commands(&actions);
    assert!(!sent.is_empty());
    let mut merges = Vec::new();
    for s in &sent {
        match s {
            SongCommand::SetTrackControllers { device, merge, .. } => {
                assert_eq!(device, &bass());
                merges.push(*merge);
            }
            other => panic!("only track controller commands, got {other:?}"),
        }
    }
    assert!(merges.iter().all(|m| m.is_some() && *m == merges[0]));
    assert!(points(&d, BEND).len() >= 3, "{:?}", points(&d, BEND));
    // The bass clip's own lanes are untouched.
    assert!(
        d.doc().song.tracks[0].lanes[0].clips[0]
            .controllers
            .is_empty()
    );
    d.undo().unwrap();
    assert_eq!(d.doc(), &before, "the stroke undoes at once");
}

#[test]
fn the_lane_chips_choose_the_controller_and_each_lane_is_its_own_list() {
    let (mut v, mut d, reg) = setup(vec![]);
    let p = v.controller_chip_center(Chip::Preset(2));
    v.handle(&down(p), d.doc(), &reg);
    v.handle(&up(p), d.doc(), &reg);
    assert_eq!(v.controller_kind(), PRESETS[2]);
    assert_eq!(v.controller_kind(), MOD);
    let a = at(&v, MOD, PPQ, 0.5);
    gesture(&mut v, &mut d, &reg, &[down(a), up(a)]);
    let p = v.controller_chip_center(Chip::Preset(0));
    v.handle(&down(p), d.doc(), &reg);
    v.handle(&up(p), d.doc(), &reg);
    let a = at(&v, BEND, 2 * PPQ, 0.25);
    gesture(&mut v, &mut d, &reg, &[down(a), up(a)]);
    let kinds: Vec<_> = own(&d).iter().map(|l| l.kind).collect();
    assert_eq!(kinds, [BEND, MOD]);
}

#[test]
fn a_point_is_deleted_with_the_key_and_the_lanes_follow_the_document() {
    let lane = ControllerLane::new(
        BEND,
        vec![pt(0, 0.0, Curve::Step), pt(PPQ, 0.5, Curve::Step)],
    );
    let (mut v, mut d, reg) = setup(vec![lane]);
    let p = at(&v, BEND, PPQ, 0.5);
    gesture(&mut v, &mut d, &reg, &[down(p), up(p)]);
    assert_eq!(v.selected_controller_points(), [1]);
    assert!(v.controller_focus());
    let outcome = v.command("edit.delete", None, d.doc(), &reg);
    let CommandOutcome::Handled(actions) = outcome else {
        panic!("{outcome:?}");
    };
    apply(&mut v, &mut d, &reg, &actions);
    assert_eq!(points(&d, BEND).len(), 1);
    // An undo outside the roll shows in the roll at the next sync.
    d.undo().unwrap();
    v.sync(d.doc(), &reg);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, d.doc(), &reg);
    assert_eq!(points(&d, BEND).len(), 2);
}

#[test]
fn nothing_of_the_note_editor_sends_a_note_command() {
    let (mut v, mut d, reg) = setup(vec![]);
    let b = v.bounds();
    // The quantize buttons are not there, and the scale bar and the old grid area are inert.
    let quant = Point::new(b.x1 - 90.0, b.y0 + 14.0);
    let scale = Point::new(b.x0 + 70.0, b.y0 + 28.0 + 10.0);
    for p in [quant, scale] {
        let actions = gesture(&mut v, &mut d, &reg, &[down(p), up(p)]);
        assert!(actions.is_empty(), "{p:?}: {actions:?}");
    }
    // Keys that edit notes find nothing.
    for id in ["edit.duplicate", "editor.legato", "edit.select_all"] {
        let out = v.command(id, None, d.doc(), &reg);
        if let CommandOutcome::Handled(a) = out {
            assert!(song_commands(&a).is_empty(), "{id}");
        }
    }
}

#[test]
fn the_ruler_still_locates() {
    let (mut v, mut d, reg) = setup(vec![]);
    let p = Point::new(v.tick_to_x(2 * BAR), v.ruler_y());
    let actions = gesture(&mut v, &mut d, &reg, &[down(p), up(p)]);
    assert!(
        actions.iter().any(|a| matches!(
            a,
            UiAction::Transport(atomik_ui::TransportAction::Locate(_))
        )),
        "{actions:?}"
    );
}

#[test]
fn the_close_button_closes_and_a_gone_track_leaves_an_empty_roll() {
    let (mut v, mut d, reg) = setup(vec![]);
    let close = v.close_button_center();
    let actions = gesture(&mut v, &mut d, &reg, &[down(close), up(close)]);
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, UiAction::ClosePianoRoll))
    );
    // The track goes away: the roll paints its empty state.
    let mut doc = d.doc().clone();
    doc.song_mut().tracks.remove(0);
    v.sync(&doc, &reg);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    assert!(dl.texts().contains(&"No clip open"), "{:?}", dl.texts());
}

#[test]
fn the_track_roll_paints_its_own_title_and_no_note_editor() {
    let (mut v, d, reg) = setup(vec![ControllerLane::new(
        MOD,
        vec![pt(0, 0.5, Curve::Step)],
    )]);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, d.doc(), &reg);
    let texts = dl.texts();
    assert!(texts.contains(&"TRACK CONTROLLERS"), "{texts:?}");
    assert!(!texts.contains(&"PIANO ROLL"), "{texts:?}");
    assert!(!texts.contains(&"QUANT"), "{texts:?}");
    for want in ["CTRL", "BEND"] {
        assert!(texts.contains(&want), "{want} in {texts:?}");
    }
}

#[test]
fn opening_a_clip_leaves_the_track_mode() {
    let (mut v, d, reg) = setup(vec![]);
    v.open(atomik_model::ClipId(1));
    v.sync(d.doc(), &reg);
    assert_eq!(v.track(), None);
    assert_eq!(v.clip(), Some(atomik_model::ClipId(1)));
    v.open_track(bass());
    v.close();
    assert_eq!(v.track(), None);
}
