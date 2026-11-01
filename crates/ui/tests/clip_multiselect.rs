//! The arrangement's multi-selection and lane drags: Shift and Ctrl clicks across tracks, the
//! rubber band, and dragging clips onto other lanes (move, Ctrl = copy), run against a real
//! document so the undo steps count.

mod support;

use atomik_model::{ClipId, Command, Document, SongCommand, WorkflowCommand};
use atomik_ui::Tool;
use atomik_ui::kurbo::{Point, Vec2};
use atomik_ui::timeline::Snap;
use atomik_ui::{ArrangementView, Modifiers, PointerButton, UiAction, UiEvent};
use support::song::*;
use support::*;

fn setup() -> (
    ArrangementView,
    atomik_model::RackDoc,
    atomik_device_sdk::Registry,
) {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    v.set_snap(Snap::Bar);
    (v, doc, reg)
}

fn ev_down(at: Point, modifiers: Modifiers) -> UiEvent {
    UiEvent::PointerDown {
        pos: at,
        button: PointerButton::Primary,
        modifiers,
        clicks: 1,
    }
}

fn ev_up(at: Point, modifiers: Modifiers) -> UiEvent {
    UiEvent::PointerUp {
        pos: at,
        button: PointerButton::Primary,
        modifiers,
    }
}

fn ctrl() -> Modifiers {
    Modifiers {
        ctrl: true,
        ..Modifiers::NONE
    }
}

fn run_events(
    v: &mut ArrangementView,
    doc: &atomik_model::RackDoc,
    reg: &atomik_device_sdk::Registry,
    events: &[UiEvent],
) -> Vec<UiAction> {
    events.iter().flat_map(|e| v.handle(e, doc, reg)).collect()
}

fn click(
    v: &mut ArrangementView,
    doc: &atomik_model::RackDoc,
    reg: &atomik_device_sdk::Registry,
    id: u64,
    modifiers: Modifiers,
) {
    let at = grab(v, id);
    run_events(v, doc, reg, &[ev_down(at, modifiers), ev_up(at, modifiers)]);
}

/// A point inside a clip's body, away from its edges and header.
fn grab(v: &ArrangementView, id: u64) -> Point {
    let r = v.clip_rect(ClipId(id)).unwrap();
    Point::new(r.x0 + 30.0, r.center().y + 6.0)
}

/// Applies the commands among `actions` to `d` the way the app does.
fn apply(d: &mut Document, reg: &atomik_device_sdk::Registry, actions: &[UiAction]) {
    for a in actions {
        match a {
            UiAction::Command(c) => {
                d.apply(c.clone(), reg)
                    .unwrap_or_else(|e| panic!("{c:?}: {e}"));
            }
            UiAction::EndMerge => d.end_merge(),
            _ => {}
        }
    }
}

/// A `MoveClips`: `(clip, track, start)` per clip, the copy flag and the merge id.
type Moved = (Vec<(u64, String, i64)>, bool, Option<u64>);

fn move_cmds(actions: &[UiAction]) -> Vec<Moved> {
    commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            Command::Workflow(WorkflowCommand::MoveClips { moves, copy, merge }) => Some((
                moves
                    .into_iter()
                    .map(|m| (m.id.0, m.track.0, m.start))
                    .collect(),
                copy,
                merge,
            )),
            _ => None,
        })
        .collect()
}

// ------------------------------------------------------------------ click selection

#[test]
fn shift_and_ctrl_clicks_build_a_selection_across_tracks() {
    let (mut v, doc, reg) = setup();
    click(&mut v, &doc, &reg, 1, Modifiers::NONE);
    click(&mut v, &doc, &reg, 2, Modifiers::SHIFT);
    click(&mut v, &doc, &reg, 3, ctrl());
    assert_eq!(v.selected_clips(), vec![ClipId(1), ClipId(2), ClipId(3)]);
    click(&mut v, &doc, &reg, 2, ctrl());
    assert_eq!(
        v.selected_clips(),
        vec![ClipId(1), ClipId(3)],
        "Ctrl toggles off"
    );
    click(&mut v, &doc, &reg, 5, Modifiers::NONE);
    assert_eq!(
        v.selected_clips(),
        vec![ClipId(5)],
        "a plain click selects only"
    );
}

// ------------------------------------------------------------------ rubber band

#[test]
fn shift_dragging_on_a_lane_selects_the_clips_inside_and_adds_to_the_selection() {
    let (mut v, doc, reg) = setup();
    let keys = v.lane_rect(&key("keys"), 0).unwrap();
    let lead = v.clip_rect(ClipId(2)).unwrap();
    let keys_clip = v.clip_rect(ClipId(3)).unwrap();
    // From the empty part of the keys lane (right of its clip) up across the lead clip.
    let start = Point::new(keys_clip.x1 + 40.0, keys.center().y);
    let end = Point::new(lead.x0 + 10.0, lead.center().y);
    click(&mut v, &doc, &reg, 5, Modifiers::NONE);
    let acts = run_events(
        &mut v,
        &doc,
        &reg,
        &[
            ev_down(start, Modifiers::SHIFT),
            UiEvent::PointerMove {
                pos: end,
                modifiers: Modifiers::SHIFT,
            },
            ev_up(end, Modifiers::SHIFT),
        ],
    );
    assert!(
        commands(&acts).is_empty(),
        "selecting edits nothing: {acts:?}"
    );
    assert_eq!(v.selected_clips(), vec![ClipId(2), ClipId(3), ClipId(5)]);
}

#[test]
fn the_pencil_draws_by_dragging_on_a_lane_and_ctrl_drag_selects_without_drawing() {
    let (mut v, doc, reg) = setup();
    v.set_tool(Tool::Pencil);
    let keys = v.lane_rect(&key("keys"), 0).unwrap();
    let a = Point::new(keys.x0 + 600.0, keys.center().y);
    let b = a + Vec2::new(120.0, 0.0);
    let drawn = run_events(
        &mut v,
        &doc,
        &reg,
        &[
            ev_down(a, Modifiers::NONE),
            UiEvent::PointerMove {
                pos: b,
                modifiers: Modifiers::NONE,
            },
            ev_up(b, Modifiers::NONE),
        ],
    );
    assert!(
        commands(&drawn)
            .iter()
            .any(|c| matches!(c, Command::Song(SongCommand::AddClip { .. }))),
        "{drawn:?}"
    );
    v.set_tool(Tool::Select);
    let selected = run_events(
        &mut v,
        &doc,
        &reg,
        &[
            ev_down(a, ctrl()),
            UiEvent::PointerMove {
                pos: b,
                modifiers: ctrl(),
            },
            ev_up(b, ctrl()),
        ],
    );
    assert!(commands(&selected).is_empty(), "{selected:?}");
}

#[test]
fn dragging_below_the_last_track_selects_with_a_plain_drag_and_a_plain_click_clears() {
    let (mut v, doc, reg) = setup();
    let keys = v.lane_rect(&key("keys"), 0).unwrap();
    let bass = v.clip_rect(ClipId(1)).unwrap();
    let below = Point::new(keys.x0 + 700.0, 400.0);
    assert!(below.y > keys.y1 + 30.0, "the point is below the rows");
    let end = Point::new(bass.x0 + 20.0, bass.center().y);
    let acts = run_events(
        &mut v,
        &doc,
        &reg,
        &[
            ev_down(below, Modifiers::NONE),
            UiEvent::PointerMove {
                pos: end,
                modifiers: Modifiers::NONE,
            },
            ev_up(end, Modifiers::NONE),
        ],
    );
    assert!(commands(&acts).is_empty(), "{acts:?}");
    let sel = v.selected_clips();
    assert!(
        sel.contains(&ClipId(1)) && sel.contains(&ClipId(3)),
        "{sel:?}"
    );
    run_events(
        &mut v,
        &doc,
        &reg,
        &[
            ev_down(below, Modifiers::NONE),
            ev_up(below, Modifiers::NONE),
        ],
    );
    assert!(v.selected_clips().is_empty());
}

fn drag(
    v: &mut ArrangementView,
    doc: &atomik_model::RackDoc,
    reg: &atomik_device_sdk::Registry,
    from: Point,
    to: Point,
    m: Modifiers,
) -> Vec<UiAction> {
    run_events(
        v,
        doc,
        reg,
        &[
            ev_down(from, m),
            UiEvent::PointerMove {
                pos: to,
                modifiers: m,
            },
            ev_up(to, m),
        ],
    )
}

fn no_add_clip(acts: &[UiAction]) -> bool {
    !commands(acts)
        .iter()
        .any(|c| matches!(c, Command::Song(SongCommand::AddClip { .. })))
}

#[test]
fn the_select_tool_drags_a_band_across_lanes_and_creates_no_clip() {
    let (mut v, doc, reg) = setup();
    assert_eq!(v.tool(), Tool::Select);
    let keys = v.lane_rect(&key("keys"), 0).unwrap();
    let lead = v.clip_rect(ClipId(2)).unwrap();
    let keys_clip = v.clip_rect(ClipId(3)).unwrap();
    let start = Point::new(keys_clip.x1 + 40.0, keys.center().y);
    let end = Point::new(lead.x0 + 10.0, lead.center().y);
    let acts = drag(&mut v, &doc, &reg, start, end, Modifiers::NONE);
    assert!(commands(&acts).is_empty(), "{acts:?}");
    let sel = v.selected_clips();
    assert!(
        sel.contains(&ClipId(2)) && sel.contains(&ClipId(3)),
        "{sel:?}"
    );
    // A second plain band replaces the selection.
    let keys = v.lane_rect(&key("keys"), 0).unwrap();
    let from = Point::new(keys.x0 + 700.0, keys.center().y);
    let acts = drag(
        &mut v,
        &doc,
        &reg,
        from,
        Point::new(from.x + 30.0, from.y + 2.0),
        Modifiers::NONE,
    );
    assert!(commands(&acts).is_empty(), "{acts:?}");
    assert!(
        v.selected_clips().is_empty(),
        "a band over nothing selects nothing"
    );
}

#[test]
fn shift_and_ctrl_add_to_the_selection_like_the_piano_roll() {
    for m in [Modifiers::SHIFT, ctrl()] {
        let (mut v, doc, reg) = setup();
        let keys = v.lane_rect(&key("keys"), 0).unwrap();
        let keys_clip = v.clip_rect(ClipId(3)).unwrap();
        click(&mut v, &doc, &reg, 5, Modifiers::NONE);
        let a = Point::new(keys_clip.x1 + 40.0, keys.center().y);
        let acts = drag(
            &mut v,
            &doc,
            &reg,
            a,
            Point::new(a.x - 20.0 - keys_clip.width(), a.y),
            m,
        );
        assert!(no_add_clip(&acts) && commands(&acts).is_empty(), "{acts:?}");
        let sel = v.selected_clips();
        assert!(
            sel.contains(&ClipId(5)) && sel.contains(&ClipId(3)),
            "{m:?} {sel:?}"
        );
    }
}

#[test]
fn a_click_on_an_empty_lane_with_select_clears_the_selection_and_creates_nothing() {
    let (mut v, doc, reg) = setup();
    click(&mut v, &doc, &reg, 3, Modifiers::NONE);
    assert!(!v.selected_clips().is_empty());
    let keys = v.lane_rect(&key("keys"), 0).unwrap();
    let at = Point::new(keys.x0 + 700.0, keys.center().y);
    let acts = run_events(
        &mut v,
        &doc,
        &reg,
        &[ev_down(at, Modifiers::NONE), ev_up(at, Modifiers::NONE)],
    );
    assert!(commands(&acts).is_empty(), "{acts:?}");
    assert!(v.selected_clips().is_empty());
}

#[test]
fn the_alternate_tool_role_and_a_double_click_still_create_clips_with_select() {
    let (mut v, doc, reg) = setup();
    let keys = v.lane_rect(&key("keys"), 0).unwrap();
    let a = Point::new(keys.x0 + 600.0, keys.center().y);
    let alt = Modifiers {
        alt: true,
        ..Modifiers::NONE
    };
    let acts = drag(&mut v, &doc, &reg, a, a + Vec2::new(120.0, 0.0), alt);
    assert!(!no_add_clip(&acts), "Alt switches to the pencil: {acts:?}");
    let dbl = run_events(
        &mut v,
        &doc,
        &reg,
        &[
            UiEvent::PointerDown {
                pos: a,
                button: PointerButton::Primary,
                modifiers: Modifiers::NONE,
                clicks: 2,
            },
            ev_up(a, Modifiers::NONE),
        ],
    );
    assert!(!no_add_clip(&dbl), "{dbl:?}");
}

// ------------------------------------------------------------------ lane drags

#[test]
fn dragging_a_clip_onto_another_track_moves_it_there_as_one_undo_step() {
    let (mut v, doc, reg) = setup();
    let mut d = Document::new(doc.clone());
    let before = d.doc().clone();
    let from = grab(&v, 2);
    let target = v.lane_rect(&key("keys"), 0).unwrap();
    let to = Point::new(from.x + 96.0, target.center().y);
    let mut acts = run_events(
        &mut v,
        &doc,
        &reg,
        &[
            ev_down(from, Modifiers::NONE),
            UiEvent::PointerMove {
                pos: Point::new(from.x + 48.0, from.y),
                modifiers: Modifiers::NONE,
            },
            UiEvent::PointerMove {
                pos: to,
                modifiers: Modifiers::NONE,
            },
            ev_up(to, Modifiers::NONE),
        ],
    );
    let moves = move_cmds(&acts);
    assert_eq!(moves.len(), 1, "{acts:?}");
    let (list, copy, merge) = &moves[0];
    assert!(!copy);
    assert_eq!(
        list,
        &[(2, "keys".to_owned(), 2 * BAR)],
        "time follows the drag"
    );
    assert!(merge.is_some(), "joins the geometry drags of the gesture");
    assert_eq!(acts.last(), Some(&UiAction::EndMerge));
    apply(&mut d, &reg, &acts);
    let keys = d.doc().song.track(&key("keys")).unwrap();
    assert!(keys.lanes[0].clips.iter().any(|c| c.id == ClipId(2)));
    assert!(
        d.doc().song.track(&key("lead")).unwrap().lanes[0]
            .clips
            .is_empty()
    );
    d.undo(&reg).expect("a step");
    assert_eq!(d.doc(), &before, "the gesture is one undo step");
    acts.clear();
}

#[test]
fn ctrl_dragging_drops_a_copy_and_leaves_the_clip_alone_meanwhile() {
    let (mut v, doc, reg) = setup();
    let mut d = Document::new(doc.clone());
    let before = d.doc().clone();
    let from = grab(&v, 2);
    let target = v.lane_rect(&key("keys"), 0).unwrap();
    let to = Point::new(from.x, target.center().y);
    let moving = run_events(
        &mut v,
        &doc,
        &reg,
        &[
            ev_down(from, ctrl()),
            UiEvent::PointerMove {
                pos: to,
                modifiers: ctrl(),
            },
        ],
    );
    assert!(
        commands(&moving).is_empty(),
        "nothing moves before the drop: {moving:?}"
    );
    let drop = v.handle(&ev_up(to, ctrl()), &doc, &reg);
    let moves = move_cmds(&drop);
    assert_eq!(moves.len(), 1, "{drop:?}");
    assert!(moves[0].1, "a copy");
    assert_eq!(moves[0].2, None);
    apply(&mut d, &reg, &drop);
    assert_eq!(
        d.doc().song.track(&key("lead")).unwrap().lanes[0]
            .clips
            .len(),
        1
    );
    assert_eq!(
        d.doc().song.track(&key("keys")).unwrap().lanes[0]
            .clips
            .len(),
        2
    );
    d.undo(&reg).expect("a step");
    assert_eq!(d.doc(), &before);
}

#[test]
fn ctrl_click_without_a_drag_still_toggles_the_selection() {
    let (mut v, doc, reg) = setup();
    click(&mut v, &doc, &reg, 2, Modifiers::NONE);
    click(&mut v, &doc, &reg, 3, ctrl());
    assert_eq!(v.selected_clips(), vec![ClipId(2), ClipId(3)]);
    click(&mut v, &doc, &reg, 3, ctrl());
    assert_eq!(v.selected_clips(), vec![ClipId(2)]);
}

#[test]
fn a_clip_does_not_drop_on_a_lane_it_does_not_fit() {
    let (mut v, doc, reg) = setup();
    // The automation clip (bass cutoff lane) has no counterpart on the lead track.
    let from = {
        let r = v.clip_rect(ClipId(4)).unwrap();
        Point::new(r.x0 + 30.0, r.center().y + 6.0)
    };
    let lead = v.lane_rect(&key("lead"), 0).unwrap();
    let to = Point::new(from.x, lead.center().y);
    let acts = run_events(
        &mut v,
        &doc,
        &reg,
        &[
            ev_down(from, Modifiers::NONE),
            UiEvent::PointerMove {
                pos: to,
                modifiers: Modifiers::NONE,
            },
            ev_up(to, Modifiers::NONE),
        ],
    );
    assert!(move_cmds(&acts).is_empty(), "{acts:?}");
}

#[test]
fn a_group_keeps_its_relative_tracks_and_offsets_across_lanes() {
    let (mut v, doc, reg) = setup();
    let mut d = Document::new(doc.clone());
    let before = d.doc().clone();
    click(&mut v, &doc, &reg, 2, Modifiers::NONE);
    click(&mut v, &doc, &reg, 3, Modifiers::SHIFT);
    // Lead (track 1) and keys (track 2) go up one track: to bass and lead.
    let from = grab(&v, 3);
    let bass = v.lane_rect(&key("lead"), 0).unwrap();
    let to = Point::new(from.x + 96.0, bass.center().y);
    let acts = run_events(
        &mut v,
        &doc,
        &reg,
        &[
            ev_down(from, Modifiers::NONE),
            UiEvent::PointerMove {
                pos: Point::new(from.x + 48.0, from.y),
                modifiers: Modifiers::NONE,
            },
            UiEvent::PointerMove {
                pos: to,
                modifiers: Modifiers::NONE,
            },
            ev_up(to, Modifiers::NONE),
        ],
    );
    let moves = move_cmds(&acts);
    assert_eq!(moves.len(), 1, "{acts:?}");
    let list = &moves[0].0;
    assert_eq!(list[0], (2, "bass".to_owned(), 2 * BAR));
    assert_eq!(list[1], (3, "lead".to_owned(), BAR));
    apply(&mut d, &reg, &acts);
    d.undo(&reg).expect("a step");
    assert_eq!(d.doc(), &before, "the whole group is one undo step");
}

#[test]
fn a_remapped_copy_drag_role_decides_which_modifier_copies() {
    use atomik_ui::keymap::{Mods, Os, Role, Roles};
    let (mut v, doc, reg) = setup();
    v.set_roles(
        Roles::defaults(Os::Windows).with(Role::CopyDrag, Mods::parse("Ctrl+Shift").unwrap()),
    );
    let from = grab(&v, 2);
    let target = v.lane_rect(&key("keys"), 0).unwrap();
    let to = Point::new(from.x, target.center().y);
    let drag = |v: &mut ArrangementView, m: Modifiers| {
        let events = [
            ev_down(from, m),
            UiEvent::PointerMove {
                pos: to,
                modifiers: m,
            },
            ev_up(to, m),
        ];
        move_cmds(&run_events(v, &doc, &reg, &events))
    };
    let both = Modifiers {
        ctrl: true,
        shift: true,
        ..Modifiers::NONE
    };
    let copied = drag(&mut v, both);
    assert_eq!(copied.len(), 1);
    assert!(copied[0].1, "Ctrl+Shift copies once remapped");
    let (mut v, doc, reg) = setup();
    v.set_roles(
        Roles::defaults(Os::Windows).with(Role::CopyDrag, Mods::parse("Ctrl+Shift").unwrap()),
    );
    let events = [
        ev_down(from, ctrl()),
        UiEvent::PointerMove {
            pos: to,
            modifiers: ctrl(),
        },
        ev_up(to, ctrl()),
    ];
    let moved = move_cmds(&run_events(&mut v, &doc, &reg, &events));
    assert!(
        moved.iter().all(|m| !m.1),
        "Ctrl no longer copies: {moved:?}"
    );
}
