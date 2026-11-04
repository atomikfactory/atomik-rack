//! The Track Panel mirrors the chain of a track: players, instrument, inserts, sends and the
//! fader, and every edit it offers is one undo step the model accepts.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{
    Command, CreateOptions, CreateSource, CreateTarget, Document, WorkflowCommand, empty_doc,
    insert_chain, player_stack,
};
use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::track_panel::{Item, TrackPanelView};
use atomik_ui::{Modifiers, PointerButton, Theme, UiAction, UiEvent};
use support::*;

fn wf(c: WorkflowCommand) -> Command {
    Command::Workflow(c)
}

fn typed(t: &str) -> CreateSource {
    CreateSource::Type(t.into())
}

/// A synth `s1` (channel and track) with two players and two inserts.
fn chain_doc(reg: &Registry) -> Document {
    let mut d = Document::new(empty_doc());
    let steps = [
        wf(WorkflowCommand::CreateInstrument {
            index: usize::MAX,
            source: typed("atomik.poly-synth"),
            key: Some(key("s1")),
            options: CreateOptions::default(),
        }),
        wf(WorkflowCommand::CreatePlayer {
            type_id: "atomik.arpeggiator".into(),
            target: CreateTarget::Selected(key("s1")),
            options: CreateOptions::default(),
        }),
        wf(WorkflowCommand::CreatePlayer {
            type_id: "atomik.note-repeater".into(),
            target: CreateTarget::Selected(key("s1")),
            options: CreateOptions::default(),
        }),
    ];
    for c in steps {
        d.apply(c, reg).unwrap();
    }
    for t in ["atomik.stereo-delay", "atomik.reverb"] {
        d.apply(
            wf(WorkflowCommand::CreateEffect {
                source: typed(t),
                target: CreateTarget::Selected(key("s1")),
                key: None,
                options: CreateOptions::default(),
            }),
            reg,
        )
        .unwrap();
    }
    d
}

fn panel(d: &Document, reg: &Registry) -> TrackPanelView {
    let mut v = TrackPanelView::new(Theme::dark());
    v.set_bounds(Rect::new(0.0, 0.0, 236.0, 700.0));
    v.sync(d.doc(), reg, Some(&key("s1")));
    v
}

fn count(v: &TrackPanelView, f: impl Fn(&Item) -> bool) -> usize {
    v.items().iter().filter(|i| f(i)).count()
}

#[test]
fn the_panel_lists_players_instrument_inserts_sends_and_fader() {
    let reg = registry();
    let d = chain_doc(&reg);
    let v = panel(&d, &reg);
    assert_eq!(count(&v, |i| matches!(i, Item::Player { .. })), 2);
    assert_eq!(count(&v, |i| matches!(i, Item::Instrument { .. })), 1);
    let filled = count(&v, |i| matches!(i, Item::Insert { .. }));
    let channel = d
        .doc()
        .console
        .channel_of(&key("s1"))
        .unwrap()
        .channel
        .clone();
    assert_eq!(filled, insert_chain(d.doc(), &reg, &channel).len());
    assert!(filled >= 1);
    assert_eq!(count(&v, |i| matches!(i, Item::EmptyInsert)), 4 - filled);
    assert!(count(&v, |i| matches!(i, Item::Send { .. })) >= 1);
    assert_eq!(count(&v, |i| matches!(i, Item::Fader { .. })), 1);
}

#[test]
fn a_track_that_does_not_exist_shows_nothing() {
    let reg = registry();
    let d = chain_doc(&reg);
    let mut v = panel(&d, &reg);
    v.sync(d.doc(), &reg, Some(&key("nope")));
    assert!(v.items().is_empty());
    v.sync(d.doc(), &reg, None);
    assert!(v.items().is_empty());
}

#[test]
fn every_offered_edit_is_accepted_by_the_model_and_is_one_undo_step() {
    let reg = registry();
    let d = chain_doc(&reg);
    let v = panel(&d, &reg);
    let mut offered = 0;
    for item in v.items() {
        let (Item::Player { controls, .. } | Item::Insert { controls, .. }) = item else {
            continue;
        };
        for c in [&controls.up, &controls.down, &controls.remove]
            .into_iter()
            .flatten()
        {
            offered += 1;
            let mut scratch = Document::new(d.doc().clone());
            let steps = scratch.undo_depth();
            scratch.apply(c.clone(), &reg).unwrap();
            assert_eq!(scratch.undo_depth(), steps + 1, "{c:?}");
        }
    }
    assert!(offered >= 3, "the stack and the chain can be edited");
}

#[test]
fn moving_a_player_down_swaps_it_with_the_one_below() {
    let reg = registry();
    let d = chain_doc(&reg);
    let v = panel(&d, &reg);
    let track = d.doc().song.track(&key("s1")).unwrap();
    let before = player_stack(d.doc(), &reg, track);
    assert_eq!(before.len(), 2);
    let Some(Item::Player { controls, .. }) =
        v.items().iter().find(|i| matches!(i, Item::Player { .. }))
    else {
        panic!("a player row");
    };
    let down = controls.down.clone().expect("the top player can move down");
    let mut d = d;
    d.apply(down, &reg).unwrap();
    let track = d.doc().song.track(&key("s1")).unwrap();
    let after = player_stack(d.doc(), &reg, track);
    assert_eq!(after, vec![before[1].clone(), before[0].clone()]);
}

#[test]
fn bypass_and_send_buttons_emit_param_commands_and_the_fader_drag_is_one_merge() {
    let reg = registry();
    let d = chain_doc(&reg);
    let mut v = panel(&d, &reg);
    // Find the controls by painting-independent geometry: press every row's right edge buttons.
    let rows: Vec<Vec<UiAction>> = (0..700)
        .step_by(4)
        .map(|y| {
            v.handle(&UiEvent::PointerDown {
                pos: Point::new(236.0 - 15.0, f64::from(y)),
                button: PointerButton::Primary,
                modifiers: Modifiers::NONE,
                clicks: 1,
            })
        })
        .collect();
    let params: Vec<String> = rows
        .iter()
        .flatten()
        .filter_map(|a| match a {
            UiAction::Command(Command::SetParam { param, .. }) => Some(param.clone()),
            _ => None,
        })
        .collect();
    assert!(params.iter().any(|p| p.starts_with("send")), "{params:?}");
    assert!(params.iter().any(|p| p == "mute"), "{params:?}");
    // The fader: press on its track, drag, release.
    let mut v = panel(&d, &reg);
    let mut merge = None;
    let mut ended = false;
    for y in (0..700).step_by(2) {
        let down = v.handle(&UiEvent::PointerDown {
            pos: Point::new(120.0, f64::from(y)),
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        });
        if let Some(UiAction::Command(Command::SetParam {
            param, merge: m, ..
        })) = down.first()
            && param == "fader"
        {
            merge = *m;
            let moved = v.handle(&UiEvent::PointerMove {
                pos: Point::new(80.0, f64::from(y)),
                modifiers: Modifiers::NONE,
            });
            assert!(matches!(
                moved.first(),
                Some(UiAction::Command(Command::SetParam { merge: m2, .. })) if *m2 == merge
            ));
            let up = v.handle(&UiEvent::PointerUp {
                pos: Point::new(80.0, f64::from(y)),
                button: PointerButton::Primary,
                modifiers: Modifiers::NONE,
            });
            ended = up.contains(&UiAction::EndMerge);
            break;
        }
    }
    assert!(merge.is_some() && ended);
}

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";

fn check(name: &str, actual: &str) {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
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

#[test]
fn the_track_panel_matches_its_golden() {
    let reg = registry();
    let d = chain_doc(&reg);
    let v = panel(&d, &reg);
    let mut dl = atomik_ui::DisplayList::new();
    v.paint(&mut dl);
    check("track_panel_1x.json", &dl.to_json());
}

#[test]
fn an_empty_track_panel_matches_its_golden() {
    let reg = registry();
    let d = chain_doc(&reg);
    let mut v = panel(&d, &reg);
    v.sync(d.doc(), &reg, None);
    let mut dl = atomik_ui::DisplayList::new();
    v.paint(&mut dl);
    check("track_panel_empty_1x.json", &dl.to_json());
}

// ---- the live groove rows ----

fn press(v: &mut TrackPanelView, x: f64, y: f64) -> Vec<UiAction> {
    v.handle(&UiEvent::PointerDown {
        pos: Point::new(x, y),
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    })
}

fn groove_row(v: &TrackPanelView) -> usize {
    v.items()
        .iter()
        .position(|i| matches!(i, Item::Groove { .. }))
        .expect("a note track has a groove row")
}

/// The y of the middle of item `i` (rows are 22 px, headings 20 px, from y 26).
fn row_y(v: &TrackPanelView, i: usize) -> f64 {
    let mut y = 26.0;
    for item in &v.items()[..i] {
        y += if matches!(item, Item::Heading(_)) {
            20.0
        } else {
            22.0
        };
    }
    y + if matches!(v.items()[i], Item::Heading(_)) {
        10.0
    } else {
        11.0
    }
}

#[test]
fn the_groove_row_steps_through_none_the_built_in_grooves_and_the_songs() {
    let reg = registry();
    let mut d = chain_doc(&reg);
    d.apply(
        Command::Song(atomik_model::SongCommand::SetGroove(atomik_model::Groove {
            name: "Mine".into(),
            step: atomik_model::StepResolution::Sixteenth,
            slots: vec![atomik_model::GrooveSlot::STRAIGHT],
        })),
        &reg,
    )
    .unwrap();
    let mut v = panel(&d, &reg);
    let at = groove_row(&v);
    let Item::Groove { name, prev, next } = v.items()[at].clone() else {
        unreachable!()
    };
    assert_eq!(name, None);
    // Next from none is the first built-in groove; previous wraps to the song's last one.
    let first = atomik_model::groove::builtin_grooves().remove(0).name;
    let applied = |c: &Command| {
        let mut scratch = d.clone();
        scratch.apply(c.clone(), &reg).unwrap();
        scratch.doc().song.tracks[0].groove.clone().map(|g| g.name)
    };
    assert_eq!(applied(&next), Some(first));
    assert_eq!(applied(&prev).as_deref(), Some("Mine"));
    // The buttons at the right edge of the row emit those commands.
    let y = row_y(&v, at);
    assert_eq!(press(&mut v, 236.0 - 15.0, y), [UiAction::Command(next)]);
    assert_eq!(press(&mut v, 236.0 - 35.0, y), [UiAction::Command(prev)]);
}

#[test]
fn the_strength_bar_sends_one_command_when_the_button_is_released() {
    let reg = registry();
    let mut d = chain_doc(&reg);
    d.apply(
        Command::Song(atomik_model::SongCommand::SetTrackGroove {
            device: key("s1"),
            groove: Some(atomik_model::TrackGroove {
                name: "Swing 55%".into(),
                strength: 100,
            }),
        }),
        &reg,
    )
    .unwrap();
    let before = d.undo_depth();
    let mut v = panel(&d, &reg);
    let at = v
        .items()
        .iter()
        .position(|i| matches!(i, Item::GrooveAmount { .. }))
        .expect("the strength row shows with a groove");
    let y = row_y(&v, at);
    // The bar runs from x 62 to 236 - 12 - 26 = 198: its middle is half the strength.
    assert!(
        press(&mut v, 130.0, y).is_empty(),
        "a press only starts the drag"
    );
    let moved = v.handle(&UiEvent::PointerMove {
        pos: Point::new(130.0, y),
        modifiers: Modifiers::NONE,
    });
    assert!(moved.is_empty(), "nothing is sent while dragging");
    let up = v.handle(&UiEvent::PointerUp {
        pos: Point::new(130.0, y),
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    });
    let [UiAction::Command(c)] = up.as_slice() else {
        panic!("one command on release, got {up:?}");
    };
    d.apply(c.clone(), &reg).unwrap();
    let g = d.doc().song.tracks[0].groove.clone().unwrap();
    assert_eq!(g.name, "Swing 55%");
    assert!((48..=52).contains(&g.strength), "{}", g.strength);
    assert_eq!(
        d.undo_depth(),
        before + 1,
        "one undo step for the whole drag"
    );
}
