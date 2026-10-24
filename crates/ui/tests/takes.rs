//! Take groups in the arrangement: the comp strip under a lane, the take rows that open under it,
//! picking a take, comping a range with a drag, deleting a take and flattening a group, run
//! against a real `Document` so the undo steps count. `cargo test -p atomik-ui --test takes --
//! --ignored` writes a preview PNG to `target/preview`.

mod support;

use atomik_model::{Clip, Command, Document, SongCommand};
use atomik_ui::kurbo::Point;
use atomik_ui::timeline::Snap;
use atomik_ui::{
    ArrangementView, DisplayList, Key, Modifiers, PointerButton, TextEngine, UiAction, UiEvent,
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
    assert!(
        actions.is_empty(),
        "opening is view state only: {actions:?}"
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
    d.undo(&reg).unwrap();
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
    d.undo(&reg).unwrap();
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
    d.undo(&reg).unwrap();
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
