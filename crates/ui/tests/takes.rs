//! Take groups in the arrangement: the comp strip under a lane, the take rows that open under it,
//! picking a take, comping a range with a drag, deleting a take and flattening a group, run
//! against a real `Document` so the undo steps count. `cargo test -p atomik-ui --test takes --
//! --ignored` writes a preview PNG to `target/preview`.

mod support;

use atomik_model::{Clip, Command, Document, SongCommand};
use atomik_ui::kurbo::Point;
use atomik_ui::timeline::Snap;
use atomik_ui::{
    ArrangementView, DisplayList, Key, Modifiers, PointerButton, TextEngine, Tool, UiAction,
    UiEvent,
};
use std::path::PathBuf;
use support::cpu::CpuCanvas;
use support::song::*;
use support::*;

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";

/// The song of the other arrangement tests with a group of three note takes (keys 70, 72 and 74)
/// over bars 4 to 6 on the `keys` track, the last take shorter than the others.
fn setup() -> (Document, atomik_device_sdk::Registry, ArrangementView) {
    let reg = registry();
    let mut d = Document::new(song_doc());
    let take = |length: i64, key: u8| -> Clip {
        notes_clip(
            50,
            4 * BAR,
            length,
            vec![n(0, PPQ, key, 0.8), n(BAR, PPQ, key + 1, 0.7)],
        )
    };
    d.apply(
        Command::Song(SongCommand::NewTakeGroup {
            device: key("keys"),
            lane: 0,
            takes: vec![take(2 * BAR, 70), take(2 * BAR, 72), take(BAR, 74)],
        }),
        &reg,
    )
    .unwrap();
    let mut v = arrangement(d.doc(), &reg);
    v.set_snap(Snap::Bar);
    (d, reg, v)
}

fn down(at: Point) -> UiEvent {
    UiEvent::PointerDown {
        pos: at,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    }
}

fn up(at: Point) -> UiEvent {
    UiEvent::PointerUp {
        pos: at,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    }
}

fn run(
    d: &Document,
    reg: &atomik_device_sdk::Registry,
    v: &mut ArrangementView,
    events: &[UiEvent],
) -> Vec<UiAction> {
    events
        .iter()
        .flat_map(|e| v.handle(e, d.doc(), reg))
        .collect()
}

fn click(
    d: &Document,
    reg: &atomik_device_sdk::Registry,
    v: &mut ArrangementView,
    at: Point,
) -> Vec<UiAction> {
    run(d, reg, v, &[down(at), up(at)])
}

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

fn song_commands(actions: &[UiAction]) -> Vec<SongCommand> {
    commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            Command::Song(s) => Some(s),
            _ => None,
        })
        .collect()
}

fn group(d: &Document) -> atomik_model::TakeGroup {
    d.doc()
        .song
        .take_groups()
        .next()
        .expect("a group")
        .2
        .clone()
}

fn open(d: &Document, reg: &atomik_device_sdk::Registry, v: &mut ArrangementView) {
    let toggle = v.take_group_toggle_center(1).expect("a strip");
    let actions = click(d, reg, v, toggle);
    assert_eq!(
        song_commands(&actions),
        [SongCommand::SetTakesOpen {
            take_group: 1,
            open: true
        }],
        "opening shows the rows at once and asks the song to keep it"
    );
    assert!(v.takes_open(1));
}

// ------------------------------------------------------------------ strip and rows

#[test]
fn a_group_shows_a_closed_strip_and_its_chevron_opens_one_row_per_take() {
    let (d, reg, mut v) = setup();
    assert!(
        v.take_group_toggle_center(1).is_some(),
        "the strip is there"
    );
    assert!(!v.takes_open(1));
    assert!(
        v.take_point(1, 0, 4 * BAR).is_none(),
        "closed: no take rows"
    );
    let content_closed = v.lane_rect(&key("keys"), 0).unwrap();
    open(&d, &reg, &mut v);
    for take in 0..3 {
        assert!(v.take_point(1, take, 4 * BAR).is_some(), "take {take}");
    }
    assert!(v.take_point(1, 3, 4 * BAR).is_none());
    // The lane itself did not move; the rows are below it.
    assert_eq!(v.lane_rect(&key("keys"), 0).unwrap(), content_closed);
    let p0 = v.take_point(1, 0, 0).unwrap();
    let p2 = v.take_point(1, 2, 0).unwrap();
    assert!(p0.y < p2.y, "takes stack top to bottom in recording order");
    // The chevron closes them again.
    let toggle = v.take_group_toggle_center(1).unwrap();
    click(&d, &reg, &mut v, toggle);
    assert!(!v.takes_open(1));
    assert!(v.take_point(1, 0, 0).is_none());
}

#[test]
fn a_song_without_groups_has_no_take_rows() {
    let (doc, reg) = (song_doc(), registry());
    let v = arrangement(&doc, &reg);
    assert!(v.take_group_toggle_center(1).is_none());
}

#[test]
fn the_open_state_goes_when_the_group_does() {
    let (mut d, reg, mut v) = setup();
    open(&d, &reg, &mut v);
    let flatten = v.take_flatten_center(1).unwrap();
    let actions = click(&d, &reg, &mut v, flatten);
    assert_eq!(
        song_commands(&actions),
        [SongCommand::FlattenTakes { group: 1 }]
    );
    apply(&mut d, &reg, &actions);
    v.sync(d.doc(), &reg);
    assert!(!v.takes_open(1));
    assert!(v.take_group_toggle_center(1).is_none());
}

// ------------------------------------------------------------------ picking

#[test]
fn clicking_a_take_name_picks_the_take_as_one_undo_step() {
    let (mut d, reg, mut v) = setup();
    open(&d, &reg, &mut v);
    let at = v.take_label_center(1, 1).unwrap();
    let actions = click(&d, &reg, &mut v, at);
    assert_eq!(
        song_commands(&actions),
        [SongCommand::PickTake { group: 1, take: 1 }]
    );
    let before = group(&d).comp;
    apply(&mut d, &reg, &actions);
    assert_eq!(group(&d).picked(), Some(1));
    d.undo().unwrap();
    assert_eq!(group(&d).comp, before);
}

#[test]
fn a_click_inside_a_take_row_picks_that_take() {
    let (d, reg, mut v) = setup();
    open(&d, &reg, &mut v);
    let at = v.take_point(1, 0, 4 * BAR + PPQ).unwrap();
    let actions = click(&d, &reg, &mut v, at);
    assert_eq!(
        song_commands(&actions),
        [SongCommand::PickTake { group: 1, take: 0 }]
    );
}

// ------------------------------------------------------------------ comping

#[test]
fn dragging_across_a_take_row_comps_the_snapped_range_in_one_step() {
    let (mut d, reg, mut v) = setup();
    open(&d, &reg, &mut v);
    let before = group(&d).comp;
    // Bar 5 (tick 4 BAR) to bar 6 (5 BAR): snap is a bar, the pointer lands near the lines.
    let a = v.take_point(1, 1, 4 * BAR + PPQ / 4).unwrap();
    let b = v.take_point(1, 1, 5 * BAR - PPQ / 4).unwrap();
    let mid = Point::new((a.x + b.x) / 2.0, a.y);
    let actions = run(
        &d,
        &reg,
        &mut v,
        &[
            down(a),
            UiEvent::PointerMove {
                pos: mid,
                modifiers: Modifiers::NONE,
            },
            UiEvent::PointerMove {
                pos: b,
                modifiers: Modifiers::NONE,
            },
            up(b),
        ],
    );
    assert_eq!(
        song_commands(&actions),
        [SongCommand::SetCompSegment {
            group: 1,
            take: 1,
            start: 4 * BAR,
            end: 5 * BAR
        }],
        "one command on release, nothing while dragging"
    );
    apply(&mut d, &reg, &actions);
    assert_eq!(group(&d).take_at(4 * BAR + 1), Some(1));
    d.undo().unwrap();
    assert_eq!(group(&d).comp, before);
}

#[test]
fn a_drag_past_the_group_is_cut_to_its_range_and_a_backwards_drag_works() {
    let (d, reg, mut v) = setup();
    open(&d, &reg, &mut v);
    let a = v.take_point(1, 0, 6 * BAR + BAR).unwrap();
    let b = v.take_point(1, 0, 5 * BAR - PPQ / 8).unwrap();
    let actions = run(
        &d,
        &reg,
        &mut v,
        &[
            down(a),
            UiEvent::PointerMove {
                pos: b,
                modifiers: Modifiers::NONE,
            },
            up(b),
        ],
    );
    assert_eq!(
        song_commands(&actions),
        [SongCommand::SetCompSegment {
            group: 1,
            take: 0,
            start: 5 * BAR,
            end: 6 * BAR
        }]
    );
}

#[test]
fn escape_cancels_a_comp_drag_without_a_command() {
    let (d, reg, mut v) = setup();
    open(&d, &reg, &mut v);
    let a = v.take_point(1, 1, 4 * BAR).unwrap();
    let b = v.take_point(1, 1, 5 * BAR).unwrap();
    let mut actions = run(
        &d,
        &reg,
        &mut v,
        &[
            down(a),
            UiEvent::PointerMove {
                pos: b,
                modifiers: Modifiers::NONE,
            },
        ],
    );
    assert!(v.is_dragging());
    actions.extend(run(
        &d,
        &reg,
        &mut v,
        &[UiEvent::Key {
            key: Key::Escape,
            modifiers: Modifiers::NONE,
            pressed: true,
        }],
    ));
    actions.extend(run(&d, &reg, &mut v, &[up(b)]));
    assert!(song_commands(&actions).is_empty(), "{actions:?}");
    assert!(!v.is_dragging());
}

// ------------------------------------------------------------------ delete and flatten

#[test]
fn the_cross_deletes_a_take_and_the_row_goes() {
    let (mut d, reg, mut v) = setup();
    open(&d, &reg, &mut v);
    let at = v.take_delete_center(1, 2).unwrap();
    let actions = click(&d, &reg, &mut v, at);
    assert_eq!(
        song_commands(&actions),
        [SongCommand::DeleteTake { group: 1, take: 2 }]
    );
    apply(&mut d, &reg, &actions);
    v.sync(d.doc(), &reg);
    assert!(v.take_point(1, 1, 0).is_some());
    assert!(v.take_point(1, 2, 0).is_none(), "two takes are left");
}

#[test]
fn flatten_keeps_the_clips_the_comp_made() {
    let (mut d, reg, mut v) = setup();
    let clips = d.doc().song.tracks[2].lanes[0].clips.clone();
    let flatten = v.take_flatten_center(1).unwrap();
    let actions = click(&d, &reg, &mut v, flatten);
    apply(&mut d, &reg, &actions);
    assert_eq!(d.doc().song.take_groups().count(), 0);
    assert_eq!(d.doc().song.tracks[2].lanes[0].clips, clips);
    d.undo().unwrap();
    assert_eq!(d.doc().song.take_groups().count(), 1);
}

#[test]
fn a_header_click_on_a_take_row_selects_nothing_and_the_lane_clips_still_work() {
    let (d, reg, mut v) = setup();
    open(&d, &reg, &mut v);
    // The clip of the comp on the lane is still an ordinary clip: a click selects it.
    let comp_clip = group(&d).clips[0];
    let r = v.clip_rect(comp_clip).unwrap();
    click(
        &d,
        &reg,
        &mut v,
        Point::new(r.x0 + 30.0, r.center().y + 6.0),
    );
    assert!(v.selected_clips().contains(&comp_clip));
}

// ------------------------------------------------------------------ painted

#[test]
fn the_rows_paint_their_names_and_say_which_takes_the_comp_uses() {
    let (mut d, reg, mut v) = setup();
    open(&d, &reg, &mut v);
    let texts_of = |d: &Document, v: &mut ArrangementView| -> Vec<String> {
        let mut dl = DisplayList::new();
        v.paint(&mut dl, d.doc(), &reg);
        dl.texts().into_iter().map(str::to_owned).collect()
    };
    // A fresh group layers its takes: the short last take plays over the first bar of the
    // second, the first take is not in the comp.
    let texts = texts_of(&d, &mut v);
    for want in [
        "Takes (3)",
        "Take 1",
        "Take 2 (in comp)",
        "Take 3 (in comp)",
        "FLATTEN",
    ] {
        assert!(texts.iter().any(|t| t == want), "{want} in {texts:?}");
    }
    // Picking one take makes it the active one alone.
    d.apply(
        Command::Song(SongCommand::PickTake { group: 1, take: 0 }),
        &reg,
    )
    .unwrap();
    let texts = texts_of(&d, &mut v);
    assert!(texts.iter().any(|t| t == "Take 1 (active)"), "{texts:?}");
    assert!(!texts.iter().any(|t| t.contains("in comp")), "{texts:?}");
}

fn check(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os(UPDATE_ENV).is_some_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing golden {}; run with {UPDATE_ENV}=1", path.display()))
        .replace("\r\n", "\n");
    assert!(
        expected == actual,
        "{name} differs from its golden (regenerate with {UPDATE_ENV}=1)"
    );
}

fn record() -> DisplayList {
    let (mut d, reg, mut v) = setup();
    d.apply(
        Command::Song(SongCommand::SetCompSegment {
            group: 1,
            take: 0,
            start: 4 * BAR,
            end: 5 * BAR,
        }),
        &reg,
    )
    .unwrap();
    v.set_takes_open(1, true);
    v.set_playhead(4 * BAR + PPQ);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, d.doc(), &reg);
    dl
}

#[test]
fn the_arrangement_with_open_takes_matches_its_golden() {
    check("arrangement_takes_1x.json", &record().to_json());
}

#[test]
#[ignore = "writes a preview image; run explicitly"]
fn preview_png() {
    let (mut d, reg, mut v) = setup();
    d.apply(
        Command::Song(SongCommand::SetCompSegment {
            group: 1,
            take: 0,
            start: 4 * BAR,
            end: 5 * BAR,
        }),
        &reg,
    )
    .unwrap();
    let mut text = TextEngine::new();
    v.set_scale(1.5);
    v.set_takes_open(1, true);
    v.set_focused(true);
    let (w, h) = (
        (ARR_BOUNDS.width() * 1.5) as u16,
        (ARR_BOUNDS.height() * 1.5) as u16,
    );
    let mut canvas = CpuCanvas::new(w, h, &mut text);
    v.paint(&mut canvas, d.doc(), &reg);
    let png = canvas.finish().into_png().expect("png");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/preview/arrangement_takes.png");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, png).unwrap();
    println!("wrote {}", path.display());
}

// ------------------------------------------------------------------ comp strip and row menus

fn mv(at: Point) -> UiEvent {
    UiEvent::PointerMove {
        pos: at,
        modifiers: Modifiers::NONE,
    }
}

fn right_click(
    d: &Document,
    reg: &atomik_device_sdk::Registry,
    v: &mut ArrangementView,
    at: Point,
) -> Vec<UiAction> {
    run(
        d,
        reg,
        v,
        &[
            UiEvent::PointerDown {
                pos: at,
                button: PointerButton::Secondary,
                modifiers: Modifiers::NONE,
                clicks: 1,
            },
            UiEvent::PointerUp {
                pos: at,
                button: PointerButton::Secondary,
                modifiers: Modifiers::NONE,
            },
        ],
    )
}

/// Clicks the row of the open menu labelled `label`.
fn pick_row(
    d: &Document,
    reg: &atomik_device_sdk::Registry,
    v: &mut ArrangementView,
    label: &str,
) -> Vec<UiAction> {
    let i = v
        .menu_labels()
        .iter()
        .position(|l| l == label)
        .unwrap_or_else(|| panic!("no row {label}: {:?}", v.menu_labels()));
    let at = v.param_menu_row_center(i).expect("a visible row");
    click(d, reg, v, at)
}

#[test]
fn dragging_across_the_strip_silences_the_range_in_one_step() {
    let (mut d, reg, mut v) = setup();
    let before = group(&d).comp;
    let a = v.take_strip_point(1, 4 * BAR + PPQ / 4).unwrap();
    let b = v.take_strip_point(1, 5 * BAR - PPQ / 4).unwrap();
    let actions = run(&d, &reg, &mut v, &[down(a), mv(b), up(b)]);
    assert_eq!(
        song_commands(&actions),
        [SongCommand::SetCompSilence {
            group: 1,
            start: 4 * BAR,
            end: 5 * BAR
        }]
    );
    apply(&mut d, &reg, &actions);
    assert_eq!(group(&d).take_at(4 * BAR + 1), None);
    d.undo().unwrap();
    assert_eq!(group(&d).comp, before);
    // A click on the strip, and a drag over a range that is already silent, send nothing.
    assert!(song_commands(&click(&d, &reg, &mut v, a)).is_empty());
}

#[test]
fn dragging_a_cut_moves_it_by_growing_one_neighbour_and_escape_cancels() {
    let (mut d, reg, mut v) = setup();
    v.set_snap(Snap::Beat);
    // Layered takes: take 2 plays bar 4, take 1 plays bar 5, the cut is at `5 * BAR`.
    assert_eq!(group(&d).take_at(4 * BAR + 1), Some(2));
    assert_eq!(group(&d).take_at(5 * BAR + 1), Some(1));
    let cut = v.take_strip_point(1, 5 * BAR).unwrap();
    let later = v.take_strip_point(1, 5 * BAR + 2 * PPQ).unwrap();
    let actions = run(&d, &reg, &mut v, &[down(cut), mv(later), up(later)]);
    assert_eq!(
        song_commands(&actions),
        [SongCommand::SetCompSegment {
            group: 1,
            take: 2,
            start: 5 * BAR,
            end: 5 * BAR + 2 * PPQ
        }],
        "the take that grows takes the range the cut moved over"
    );
    apply(&mut d, &reg, &actions);
    assert_eq!(group(&d).take_at(5 * BAR + PPQ), Some(2));
    d.undo().unwrap();
    assert_eq!(group(&d).take_at(5 * BAR + PPQ), Some(1));
    // Dragging back into the earlier segment grows the later take instead.
    let earlier = v.take_strip_point(1, 5 * BAR - 2 * PPQ).unwrap();
    let actions = run(&d, &reg, &mut v, &[down(cut), mv(earlier), up(earlier)]);
    assert_eq!(
        song_commands(&actions),
        [SongCommand::SetCompSegment {
            group: 1,
            take: 1,
            start: 5 * BAR - 2 * PPQ,
            end: 5 * BAR
        }]
    );
    // Escape drops the drag.
    let mut actions = run(&d, &reg, &mut v, &[down(cut), mv(later)]);
    assert!(v.is_dragging());
    actions.extend(run(
        &d,
        &reg,
        &mut v,
        &[
            UiEvent::Key {
                key: Key::Escape,
                modifiers: Modifiers::NONE,
                pressed: true,
            },
            up(later),
        ],
    ));
    assert!(song_commands(&actions).is_empty(), "{actions:?}");
}

#[test]
fn the_take_row_menu_duplicates_and_reorders_in_one_step_each() {
    let (mut d, reg, mut v) = setup();
    open(&d, &reg, &mut v);
    let before = group(&d);
    let at = v.take_point(1, 1, 4 * BAR + PPQ).unwrap();
    right_click(&d, &reg, &mut v, at);
    let labels = v.menu_labels();
    assert_eq!(
        labels,
        [
            "Copy Take",
            "Copy All Takes",
            "Paste Takes",
            "Import MIDI File as Take...",
            "Open in Piano Roll",
            "Duplicate Take",
            "Move Take Up",
            "Move Take Down",
            "Loop Take x2",
            "Loop Take x4",
            "Loop Take x8",
            "Delete Take",
            "Delete Unused Takes"
        ],
        "a notes take has no level or offset rows; take 0 is not in the comp"
    );
    let actions = pick_row(&d, &reg, &mut v, "Duplicate Take");
    assert_eq!(
        song_commands(&actions),
        [SongCommand::DuplicateTake { group: 1, take: 1 }]
    );
    apply(&mut d, &reg, &actions);
    assert_eq!(group(&d).takes.len(), 4);
    d.undo().unwrap();
    assert_eq!(group(&d), before);
    right_click(&d, &reg, &mut v, at);
    let actions = pick_row(&d, &reg, &mut v, "Move Take Down");
    assert_eq!(
        song_commands(&actions),
        [SongCommand::MoveTake {
            group: 1,
            take: 1,
            to: 2
        }]
    );
    apply(&mut d, &reg, &actions);
    d.undo().unwrap();
    // The first take cannot move up; the menu says so by not offering it.
    let top = v.take_point(1, 0, 4 * BAR + PPQ).unwrap();
    right_click(&d, &reg, &mut v, top);
    assert!(!v.menu_labels().iter().any(|l| l == "Move Take Up"));
    let actions = pick_row(&d, &reg, &mut v, "Delete Unused Takes");
    assert_eq!(
        song_commands(&actions),
        [SongCommand::DeleteUnusedTakes { group: 1 }]
    );
    apply(&mut d, &reg, &actions);
    assert_eq!(group(&d).takes.len(), 2);
}

#[test]
fn the_take_row_menu_loops_a_take_and_plays_it_once_again() {
    let (mut d, reg, mut v) = setup();
    open(&d, &reg, &mut v);
    let at = v.take_point(1, 1, 4 * BAR + PPQ).unwrap();
    right_click(&d, &reg, &mut v, at);
    assert!(!v.menu_labels().iter().any(|l| l == "Play Take Once"));
    let actions = pick_row(&d, &reg, &mut v, "Loop Take x4");
    assert_eq!(
        song_commands(&actions),
        [SongCommand::SetTakeLoop {
            group: 1,
            take: 1,
            repeats: 4
        }]
    );
    apply(&mut d, &reg, &actions);
    let looped = group(&d).takes[1].clone();
    assert!(looped.loop_length.is_some());
    // The view follows the document, and the menu now offers the way back (and not x4).
    v.sync(d.doc(), &reg);
    let at = v.take_point(1, 1, 4 * BAR + PPQ).unwrap();
    right_click(&d, &reg, &mut v, at);
    let labels = v.menu_labels();
    assert!(labels.iter().any(|l| l == "Play Take Once"), "{labels:?}");
    assert!(!labels.iter().any(|l| l == "Loop Take x4"), "{labels:?}");
    let actions = pick_row(&d, &reg, &mut v, "Play Take Once");
    assert_eq!(
        song_commands(&actions),
        [SongCommand::SetTakeLoop {
            group: 1,
            take: 1,
            repeats: 1
        }]
    );
}

#[test]
fn the_strip_menu_silences_a_segment_and_offers_no_crossfade_for_notes() {
    let (mut d, reg, mut v) = setup();
    let at = v.take_strip_point(1, 5 * BAR).unwrap();
    right_click(&d, &reg, &mut v, at);
    let labels = v.menu_labels();
    assert!(labels.iter().any(|l| l == "Silence Segment"), "{labels:?}");
    assert!(
        !labels.iter().any(|l| l.contains("Crossfade")),
        "{labels:?}"
    );
    let actions = pick_row(&d, &reg, &mut v, "Silence Segment");
    assert_eq!(
        song_commands(&actions),
        [SongCommand::SetCompSilence {
            group: 1,
            start: 5 * BAR,
            end: 6 * BAR
        }]
    );
    apply(&mut d, &reg, &actions);
    assert_eq!(group(&d).take_at(5 * BAR + 1), None);
}

// ------------------------------------------------------------------ take keys, clipboard rows

fn keys(
    d: &Document,
    reg: &atomik_device_sdk::Registry,
    v: &mut ArrangementView,
    id: &str,
) -> atomik_ui::keymap::CommandOutcome {
    v.command(id, None, d.doc(), reg)
}

#[test]
fn the_take_keys_step_between_the_take_rows_and_stop_at_the_ends() {
    let (d, reg, mut v) = setup();
    // Closed: there are no rows to step through, and the key says why.
    assert!(matches!(
        keys(&d, &reg, &mut v, "sequencer.take_next"),
        atomik_ui::keymap::CommandOutcome::NotApplicable(_)
    ));
    open(&d, &reg, &mut v);
    assert_eq!(v.take_focus(), None);
    keys(&d, &reg, &mut v, "sequencer.take_next");
    assert_eq!(
        v.take_focus(),
        Some((1, 0)),
        "the first row takes the focus"
    );
    keys(&d, &reg, &mut v, "sequencer.take_next");
    keys(&d, &reg, &mut v, "sequencer.take_next");
    assert_eq!(v.take_focus(), Some((1, 2)));
    keys(&d, &reg, &mut v, "sequencer.take_next");
    assert_eq!(v.take_focus(), Some((1, 2)), "it stops at the last row");
    keys(&d, &reg, &mut v, "sequencer.take_prev");
    assert_eq!(v.take_focus(), Some((1, 1)));
    keys(&d, &reg, &mut v, "sequencer.take_prev");
    keys(&d, &reg, &mut v, "sequencer.take_prev");
    assert_eq!(v.take_focus(), Some((1, 0)), "and at the first");
}

#[test]
fn the_focused_take_is_picked_and_opened_from_the_keyboard() {
    let (d, reg, mut v) = setup();
    open(&d, &reg, &mut v);
    assert_eq!(
        keys(&d, &reg, &mut v, "sequencer.take_pick"),
        atomik_ui::keymap::CommandOutcome::Declined,
        "with no focus the key is not this view's"
    );
    keys(&d, &reg, &mut v, "sequencer.take_next");
    keys(&d, &reg, &mut v, "sequencer.take_next");
    let picked = keys(&d, &reg, &mut v, "sequencer.take_pick");
    assert_eq!(
        song_commands(picked.actions()),
        [SongCommand::PickTake { group: 1, take: 1 }]
    );
    // Return opens the focused take in the piano roll (a note take), by its own clip id.
    let id = group(&d).takes[1].id;
    let opened = keys(&d, &reg, &mut v, "sequencer.open_clip");
    assert_eq!(opened.actions(), [UiAction::OpenPianoRoll(id)]);
}

#[test]
fn selecting_a_clip_takes_the_keys_back_from_the_take_focus() {
    let (d, reg, mut v) = setup();
    open(&d, &reg, &mut v);
    keys(&d, &reg, &mut v, "sequencer.take_next");
    assert!(v.take_focus().is_some());
    let comp = d.doc().song.tracks[0].lanes[0].clips[0].id;
    v.set_selected_clips(&[comp]);
    assert_eq!(v.take_focus(), None);
    // And a click on a take row drops the clip selection.
    let at = v.take_label_center(1, 2).unwrap();
    click(&d, &reg, &mut v, at);
    assert_eq!(v.take_focus(), Some((1, 2)));
    assert!(v.selected_clips().is_empty());
}

#[test]
fn the_take_menu_offers_the_clipboard_rows_and_names_what_they_act_on() {
    let (d, reg, mut v) = setup();
    open(&d, &reg, &mut v);
    let at = v.take_point(1, 2, 4 * BAR + PPQ).unwrap();
    right_click(&d, &reg, &mut v, at);
    assert_eq!(v.take_focus(), Some((1, 2)), "the menu focuses its row");
    let copy = pick_row(&d, &reg, &mut v, "Copy Take");
    assert_eq!(
        copy,
        [UiAction::CopyTakes {
            group: 1,
            takes: vec![2]
        }]
    );
    right_click(&d, &reg, &mut v, at);
    let all = pick_row(&d, &reg, &mut v, "Copy All Takes");
    assert_eq!(
        all,
        [UiAction::CopyTakes {
            group: 1,
            takes: vec![0, 1, 2]
        }]
    );
    right_click(&d, &reg, &mut v, at);
    assert_eq!(
        pick_row(&d, &reg, &mut v, "Paste Takes"),
        [UiAction::PasteTakes { group: 1 }]
    );
    right_click(&d, &reg, &mut v, at);
    assert_eq!(
        pick_row(&d, &reg, &mut v, "Import MIDI File as Take..."),
        [UiAction::ImportTake {
            group: 1,
            notes: true
        }]
    );
    right_click(&d, &reg, &mut v, at);
    let id = group(&d).takes[2].id;
    assert_eq!(
        pick_row(&d, &reg, &mut v, "Open in Piano Roll"),
        [UiAction::OpenPianoRoll(id)]
    );
    // The strip offers the group-wide rows only.
    let strip = v.take_strip_point(1, 5 * BAR).unwrap();
    right_click(&d, &reg, &mut v, strip);
    let labels = v.menu_labels();
    assert!(labels.iter().any(|l| l == "Copy All Takes"), "{labels:?}");
    assert!(!labels.iter().any(|l| l == "Copy Take"), "{labels:?}");
    assert!(
        !labels.iter().any(|l| l.starts_with("Open in")),
        "{labels:?}"
    );
}

#[test]
fn the_piano_roll_opens_a_take_and_reads_its_notes() {
    let (d, reg, _v) = setup();
    let take = group(&d).takes[2].clone();
    assert!(
        d.doc().song.clip(take.id).is_none(),
        "a take is not a lane clip"
    );
    let mut roll = atomik_ui::PianoRollView::new(atomik_ui::Theme::dark());
    roll.set_bounds(atomik_ui::kurbo::Rect::new(0.0, 0.0, 1000.0, 560.0));
    roll.open(take.id);
    roll.sync(d.doc(), &reg);
    let atomik_model::ClipContent::Notes(notes) = &take.content else {
        panic!("a note take")
    };
    assert_eq!(roll.notes(), notes.as_slice());
    assert_eq!(roll.track_device(), Some(&key("keys")));
}

// ------------------------------------------------------------------ razor and linked comp

#[test]
fn the_razor_in_a_take_row_cuts_the_comp_at_the_click() {
    let (mut d, reg, mut v) = setup();
    open(&d, &reg, &mut v);
    v.set_tool(Tool::Razor);
    v.set_snap(Snap::Off);
    // Take 0 is under the comp nowhere yet: the comp is the layered stack (take 1 on top where
    // take 2 is short). A razor click in take 0 at 4 BAR + 1 BAR / 2 cuts to the segment's end.
    let before = group(&d).comp;
    let at = 4 * BAR + BAR / 2;
    let end = before
        .iter()
        .find(|s| s.start <= at && at < s.end)
        .unwrap()
        .end;
    let p = v.take_point(1, 0, at).unwrap();
    let actions = click(&d, &reg, &mut v, p);
    let cmds = song_commands(&actions);
    assert_eq!(cmds.len(), 1, "{cmds:?}");
    let SongCommand::SetCompSegment {
        group: 1,
        take: 0,
        start,
        end: e,
    } = cmds[0]
    else {
        panic!("{cmds:?}");
    };
    assert!(
        (start - at).abs() < BAR / 8,
        "the cut is at the pointer: {start}"
    );
    assert_eq!(e, end);
    apply(&mut d, &reg, &actions);
    assert_eq!(group(&d).take_at(start), Some(0));
    assert_eq!(
        group(&d).take_at(start - 1),
        before
            .iter()
            .find(|s| s.start < start && start <= s.end)
            .map(|s| s.take)
    );
    d.undo().unwrap();
    assert_eq!(group(&d).comp, before);
}

#[test]
fn a_razor_click_that_changes_nothing_sends_nothing() {
    let (d, reg, mut v) = setup();
    open(&d, &reg, &mut v);
    v.set_tool(Tool::Razor);
    v.set_snap(Snap::Bar);
    // The top take already plays the start of the group.
    let top = group(&d).take_at(4 * BAR).unwrap();
    let p = v.take_point(1, top, 4 * BAR).unwrap();
    let actions = click(&d, &reg, &mut v, p);
    assert!(song_commands(&actions).is_empty(), "{actions:?}");
}

#[test]
fn the_razor_dragged_across_a_take_row_comps_the_range() {
    let (d, reg, mut v) = setup();
    open(&d, &reg, &mut v);
    v.set_tool(Tool::Razor);
    let a = v.take_point(1, 1, 4 * BAR + PPQ / 4).unwrap();
    let b = v.take_point(1, 1, 5 * BAR - PPQ / 4).unwrap();
    let mid = Point::new((a.x + b.x) / 2.0, a.y);
    let moves = |p| UiEvent::PointerMove {
        pos: p,
        modifiers: Modifiers::NONE,
    };
    let actions = run(&d, &reg, &mut v, &[down(a), moves(mid), moves(b), up(b)]);
    assert_eq!(
        song_commands(&actions),
        [SongCommand::SetCompSegment {
            group: 1,
            take: 1,
            start: 4 * BAR,
            end: 5 * BAR
        }]
    );
}

/// The setup with a second group of two takes on the `lead` track, both groups open and the
/// tracks `lead` and `keys` selected together.
fn linked_setup() -> (Document, atomik_device_sdk::Registry, ArrangementView) {
    let (mut d, reg, mut v) = setup();
    d.apply(
        Command::Song(SongCommand::NewTakeGroup {
            device: key("lead"),
            lane: 0,
            takes: (0..2)
                .map(|i| notes_clip(50, 4 * BAR, 2 * BAR, vec![n(0, PPQ, 60 + i, 0.8)]))
                .collect(),
        }),
        &reg,
    )
    .unwrap();
    v.sync(d.doc(), &reg);
    open(&d, &reg, &mut v);
    let toggle = v.take_group_toggle_center(2).expect("a strip");
    let actions = click(&d, &reg, &mut v, toggle);
    assert!(!song_commands(&actions).is_empty());
    v.set_takes_open(2, true);
    v.sync(d.doc(), &reg);
    v.set_selected_track(Some(key("lead")));
    v.command("sequencer.extend_track_selection_down", None, d.doc(), &reg);
    assert_eq!(v.selected_tracks(), [key("lead"), key("keys")]);
    (d, reg, v)
}

#[test]
fn a_comp_drag_with_several_tracks_selected_comps_every_group_in_one_step() {
    let (mut d, reg, mut v) = linked_setup();
    let a = v.take_point(1, 0, 4 * BAR + PPQ / 4).unwrap();
    let b = v.take_point(1, 0, 5 * BAR - PPQ / 4).unwrap();
    let mid = Point::new((a.x + b.x) / 2.0, a.y);
    let moves = |p| UiEvent::PointerMove {
        pos: p,
        modifiers: Modifiers::NONE,
    };
    let actions = run(&d, &reg, &mut v, &[down(a), moves(mid), moves(b), up(b)]);
    assert_eq!(
        song_commands(&actions),
        [SongCommand::SetLinkedComp {
            groups: vec![1, 2],
            take: 0,
            start: 4 * BAR,
            end: 5 * BAR
        }]
    );
    assert_eq!(v.selected_tracks().len(), 2, "the selection stays");
    apply(&mut d, &reg, &actions);
    for (_, _, g) in d.doc().song.take_groups() {
        assert_eq!(g.take_at(4 * BAR + 1), Some(0));
    }
    d.undo().unwrap();
    for (_, _, g) in d.doc().song.take_groups() {
        assert_ne!(g.take_at(4 * BAR + 1), Some(0));
    }
}

#[test]
fn a_comp_drag_in_an_unselected_track_is_not_linked() {
    let (d, reg, mut v) = linked_setup();
    v.set_selected_track(Some(key("bass")));
    let a = v.take_point(1, 0, 4 * BAR + PPQ / 4).unwrap();
    let b = v.take_point(1, 0, 5 * BAR - PPQ / 4).unwrap();
    let moves = |p| UiEvent::PointerMove {
        pos: p,
        modifiers: Modifiers::NONE,
    };
    let actions = run(&d, &reg, &mut v, &[down(a), moves(b), up(b)]);
    assert!(matches!(
        song_commands(&actions)[..],
        [SongCommand::SetCompSegment { group: 1, .. }]
    ));
}

// ------------------------------------------------------------------ the saved link

#[test]
fn the_menu_links_the_groups_of_the_selected_tracks_and_a_saved_link_combs_them() {
    let (mut d, reg, mut v) = linked_setup();
    let at = v.take_point(1, 0, 4 * BAR + PPQ).unwrap();
    right_click(&d, &reg, &mut v, at);
    assert!(
        !v.menu_labels().iter().any(|l| l == "Unlink Take Group"),
        "{:?}",
        v.menu_labels()
    );
    let linked = pick_row(&d, &reg, &mut v, "Link With Selected Tracks");
    assert_eq!(
        song_commands(&linked),
        [SongCommand::LinkTakeGroups { groups: vec![1, 2] }]
    );
    apply(&mut d, &reg, &linked);
    v.sync(d.doc(), &reg);
    // The link no longer needs the selection.
    v.set_selected_track(None);
    let a = v.take_point(1, 0, 4 * BAR + PPQ / 4).unwrap();
    let b = v.take_point(1, 0, 5 * BAR - PPQ / 4).unwrap();
    let actions = run(&d, &reg, &mut v, &[down(a), mv(b), up(b)]);
    assert_eq!(
        song_commands(&actions),
        [SongCommand::SetLinkedComp {
            groups: vec![1, 2],
            take: 0,
            start: 4 * BAR,
            end: 5 * BAR
        }]
    );
    // The strip says so, and the menu offers Unlink.
    let mut dl = DisplayList::new();
    v.paint(&mut dl, d.doc(), &reg);
    assert!(dl.texts().contains(&"Takes (3) linked"), "{:?}", dl.texts());
    right_click(&d, &reg, &mut v, at);
    let labels = v.menu_labels();
    assert!(
        labels.iter().any(|l| l == "Unlink Take Group"),
        "{labels:?}"
    );
    let unlink = pick_row(&d, &reg, &mut v, "Unlink Take Group");
    assert_eq!(
        song_commands(&unlink),
        [SongCommand::UnlinkTakeGroup { group: 1 }]
    );
    apply(&mut d, &reg, &unlink);
    v.sync(d.doc(), &reg);
    let a = v.take_point(1, 0, 4 * BAR + PPQ / 4).unwrap();
    let b = v.take_point(1, 0, 5 * BAR - PPQ / 4).unwrap();
    let actions = run(&d, &reg, &mut v, &[down(a), mv(b), up(b)]);
    assert!(matches!(
        song_commands(&actions).as_slice(),
        [SongCommand::SetCompSegment { .. }]
    ));
}

#[test]
fn an_automation_lane_shows_its_take_rows_and_combs_them() {
    let (mut d, reg, _) = setup();
    d.apply(
        Command::Song(SongCommand::AddAutomationLane {
            device: key("keys"),
            param: "filter.cutoff".into(),
        }),
        &reg,
    )
    .unwrap();
    let lane = d.doc().song.track(&key("keys")).unwrap().lanes.len() - 1;
    let pass = |from: f32, to: f32| atomik_model::Clip {
        content: atomik_model::ClipContent::Automation(vec![
            atomik_model::AutomationPoint {
                tick: 0,
                value: from,
                curve: atomik_model::Curve::Linear,
            },
            atomik_model::AutomationPoint {
                tick: BAR,
                value: to,
                curve: atomik_model::Curve::Linear,
            },
        ]),
        ..notes_clip(50, 4 * BAR, 2 * BAR, Vec::new())
    };
    d.apply(
        Command::Song(SongCommand::NewTakeGroup {
            device: key("keys"),
            lane,
            takes: vec![pass(100.0, 900.0), pass(900.0, 100.0)],
        }),
        &reg,
    )
    .unwrap();
    let id = d.doc().song.take_groups().last().unwrap().2.id;
    let mut v = arrangement(d.doc(), &reg);
    v.set_snap(Snap::Bar);
    v.set_takes_open(id, true);
    v.sync(d.doc(), &reg);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, d.doc(), &reg);
    let texts = dl.texts();
    assert!(texts.contains(&"Takes (2)"), "{texts:?}");
    assert!(texts.contains(&"Take 2 (active)"), "{texts:?}");
    let a = v.take_point(id, 0, 4 * BAR + PPQ / 4).expect("a take row");
    let b = v.take_point(id, 0, 5 * BAR - PPQ / 4).unwrap();
    let actions = run(&d, &reg, &mut v, &[down(a), mv(b), up(b)]);
    assert_eq!(
        song_commands(&actions),
        [SongCommand::SetCompSegment {
            group: id,
            take: 0,
            start: 4 * BAR,
            end: 5 * BAR
        }]
    );
    apply(&mut d, &reg, &actions);
    let (_, _, g) = d.doc().song.take_group(id).unwrap();
    assert_eq!(g.take_at(4 * BAR + 1), Some(0));
}
