//! The Launcher view: its tracks and scenes, the play triangle and stop squares, double clicks,
//! drags between slots, the keyboard, the right-click list, renaming, and goldens of the view
//! with playing, queued and empty slots at 1x and 2x.

mod support;

use atomik_model::{
    ClipId, Command, LaunchQuantize, LauncherClip, LauncherCommand, MetaChange, RackDoc, Scene,
    SongCommand,
};
use atomik_ui::keymap::CommandOutcome;
use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{
    DisplayList, Key, LauncherAction, LauncherFeed, LauncherView, Modifiers, Theme, TrackLaunch,
    UiAction, UiEvent,
};
use support::song::*;
use support::*;

const BOUNDS: Rect = Rect::new(0.0, 0.0, 760.0, 420.0);
const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";

fn slot_clip(id: u64, name: Option<&str>, color: Option<[u8; 3]>) -> LauncherClip {
    let mut clip = notes_clip(
        id,
        0,
        BAR,
        vec![
            n(0, PPQ / 2, 60, 0.9),
            n(PPQ, PPQ / 2, 64, 0.7),
            n(2 * PPQ, PPQ, 67, 0.8),
        ],
    );
    clip.name = name.map(str::to_owned);
    clip.color = color;
    LauncherClip {
        clip,
        quantize: None,
    }
}

/// A song with four note tracks and slots: bass has clips in scenes 1 and 2, lead in scene 1,
/// keys in scene 3.
fn doc_with_slots() -> RackDoc {
    let mut doc = song_doc();
    let l = &mut doc.song_mut().launcher;
    l.scenes = vec![
        Scene {
            name: "Intro".into(),
            color: None,
            tempo: None,
        },
        Scene {
            name: String::new(),
            color: Some([0x9a, 0x4f, 0x4f]),
            tempo: Some(128.0),
        },
        Scene::default(),
    ];
    let mut put = |track: &str, row: usize, clip: LauncherClip| {
        let list = l.slots.entry(key(track)).or_default();
        if list.len() <= row {
            list.resize(row + 1, None);
        }
        list[row] = Some(clip);
    };
    put("bass", 0, slot_clip(101, Some("Bass A"), None));
    put(
        "bass",
        1,
        slot_clip(102, Some("Bass B"), Some([0xc8, 0x7c, 0xe0])),
    );
    put("lead", 0, slot_clip(103, None, None));
    put("keys", 2, slot_clip(104, Some("Keys long name here"), None));
    doc.song_mut().next_clip_id = 200;
    doc
}

fn view(doc: &RackDoc) -> LauncherView {
    let mut v = LauncherView::new(Theme::dark());
    v.set_bounds(BOUNDS);
    v.sync(doc);
    v
}

fn play_of(v: &LauncherView, col: usize, row: usize) -> Point {
    v.play_rect(col, row).expect("the slot shows").center()
}

fn slot_of(v: &LauncherView, col: usize, row: usize) -> Point {
    let r = v.slot_rect(col, row).expect("the slot shows");
    Point::new(r.x1 - 8.0, r.center().y)
}

fn launcher_cmds(actions: &[UiAction]) -> Vec<LauncherCommand> {
    song_commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            SongCommand::Launcher(l) => Some(l),
            _ => None,
        })
        .collect()
}

fn requests(actions: &[UiAction]) -> Vec<LauncherAction> {
    actions
        .iter()
        .filter_map(|a| match a {
            UiAction::Launcher(l) => Some(l.clone()),
            _ => None,
        })
        .collect()
}

fn feed(device: &str, state: TrackLaunch) -> LauncherFeed {
    let mut f = LauncherFeed::default();
    f.tracks.insert(key(device), state);
    f
}

#[test]
fn the_columns_are_the_note_tracks_in_order_and_a_song_always_has_rows_to_fill() {
    let doc = doc_with_slots();
    let v = view(&doc);
    let tracks = v.tracks();
    assert!(tracks.len() >= 3, "{tracks:?}");
    assert_eq!(tracks[0], key("bass"));
    assert!(v.slot_rect(0, 0).is_some() && v.slot_rect(0, 5).is_some());
    assert!(
        v.slot_rect(tracks.len(), 0).is_none(),
        "no column past the tracks"
    );
    // An empty song still shows empty scenes to fill.
    let mut empty = song_doc();
    empty.song_mut().launcher = Default::default();
    let v = view(&empty);
    assert!(v.slot_rect(0, 6).is_some());
}

#[test]
fn the_triangle_launches_and_a_scene_button_launches_the_row() {
    let doc = doc_with_slots();
    let mut v = view(&doc);
    let a = v.handle(&down(play_of(&v, 0, 0)));
    assert_eq!(
        requests(&a),
        vec![LauncherAction::Launch {
            device: key("bass"),
            row: 0
        }]
    );
    // An empty slot's triangle does nothing while its track plays nothing.
    assert!(v.handle(&down(play_of(&v, 0, 4))).is_empty());
    let s = v.scene_launch_rect(1).unwrap().center();
    assert_eq!(
        requests(&v.handle(&down(s))),
        vec![LauncherAction::LaunchScene(1)]
    );
}

#[test]
fn the_stop_square_of_an_empty_slot_and_the_stop_row_stop_a_track() {
    let doc = doc_with_slots();
    let mut v = view(&doc);
    v.set_feed(feed(
        "bass",
        TrackLaunch {
            playing: Some(0),
            ..TrackLaunch::default()
        },
    ));
    let a = v.handle(&down(play_of(&v, 0, 4)));
    assert_eq!(requests(&a), vec![LauncherAction::Stop(key("bass"))]);
    let a = v.handle(&down(v.stop_rect(1).unwrap().center()));
    assert_eq!(requests(&a), vec![LauncherAction::Stop(key("lead"))]);
    let a = v.handle(&down(v.stop_all_rect().center()));
    assert_eq!(requests(&a), vec![LauncherAction::StopAll]);
    let a = v.handle(&down(v.back_rect().center()));
    assert_eq!(requests(&a), vec![LauncherAction::Back]);
}

#[test]
fn the_toolbar_switches_record_stop_empty_and_the_quantize() {
    let doc = doc_with_slots();
    let mut v = view(&doc);
    let a = v.handle(&down(v.record_rect().center()));
    assert_eq!(requests(&a), vec![LauncherAction::ToggleRecord]);
    let a = v.handle(&down(v.stop_empty_rect().center()));
    assert_eq!(requests(&a), vec![LauncherAction::ToggleStopEmpty]);
    // The song's quantize is 1 bar: the next value is 2 bars; Shift goes back.
    let a = v.handle(&down(v.quantize_rect().center()));
    assert_eq!(
        requests(&a),
        vec![LauncherAction::SetQuantize(LaunchQuantize::TwoBars)]
    );
    let a = v.handle(&down_mods(v.quantize_rect().center(), Modifiers::SHIFT, 1));
    assert_eq!(
        requests(&a),
        vec![LauncherAction::SetQuantize(LaunchQuantize::Half)]
    );
}

#[test]
fn a_double_click_opens_a_clip_or_makes_one() {
    let doc = doc_with_slots();
    let mut v = view(&doc);
    let a = v.handle(&down_mods(slot_of(&v, 0, 0), Modifiers::NONE, 2));
    assert_eq!(requests(&a), vec![LauncherAction::OpenClip(ClipId(101))]);
    let a = v.handle(&down_mods(slot_of(&v, 1, 5), Modifiers::NONE, 2));
    assert_eq!(
        requests(&a),
        vec![LauncherAction::NewClip {
            device: key("lead"),
            row: 5
        }]
    );
    assert_eq!(v.cursor(), (1, 5));
}

#[test]
fn a_drag_moves_a_slot_and_ctrl_copies_it() {
    let doc = doc_with_slots();
    let mut v = view(&doc);
    let (from, to) = (slot_of(&v, 0, 0), slot_of(&v, 1, 3));
    let a = run(
        |e| v.handle(e),
        &[
            down(from),
            mv(Point::new(from.x + 30.0, from.y + 30.0)),
            mv(to),
            up(to),
        ],
    );
    assert_eq!(
        launcher_cmds(&a),
        vec![LauncherCommand::MoveSlot {
            from: (key("bass"), 0),
            to: (key("lead"), 3),
            copy: false
        }]
    );
    let ctrl = Modifiers {
        ctrl: true,
        meta: cfg!(target_os = "macos"),
        ..Modifiers::NONE
    };
    let ctrl = if cfg!(target_os = "macos") {
        Modifiers {
            ctrl: false,
            meta: true,
            ..Modifiers::NONE
        }
    } else {
        ctrl
    };
    let a = run(|e| v.handle(e), &[down_mods(from, ctrl, 1), mv(to), up(to)]);
    assert!(matches!(
        launcher_cmds(&a)[..],
        [LauncherCommand::MoveSlot { copy: true, .. }]
    ));
    // A click that does not move, and a drop on its own slot, edit nothing.
    let a = run(|e| v.handle(e), &[down(from), up(from)]);
    assert!(launcher_cmds(&a).is_empty());
    let a = run(
        |e| v.handle(e),
        &[down(from), mv(Point::new(from.x + 20.0, from.y)), up(from)],
    );
    assert!(launcher_cmds(&a).is_empty());
}

#[test]
fn the_arrows_move_the_cursor_and_enter_launches_the_slot_or_the_scene() {
    let doc = doc_with_slots();
    let mut v = view(&doc);
    v.handle(&press(Key::Down, Modifiers::NONE));
    v.handle(&press(Key::Right, Modifiers::NONE));
    assert_eq!(v.cursor(), (1, 1));
    // Lead has nothing in scene 2; bass does in scene 2.
    v.handle(&press(Key::Left, Modifiers::NONE));
    let a = v.handle(&press(Key::Enter, Modifiers::NONE));
    assert_eq!(
        requests(&a),
        vec![LauncherAction::Launch {
            device: key("bass"),
            row: 1
        }]
    );
    let a = v.handle(&press(Key::Enter, Modifiers::SHIFT));
    assert_eq!(requests(&a), vec![LauncherAction::LaunchScene(1)]);
    // The cursor stops at the edges.
    for _ in 0..20 {
        v.handle(&press(Key::Left, Modifiers::NONE));
        v.handle(&press(Key::Up, Modifiers::NONE));
    }
    assert_eq!(v.cursor(), (0, 0));
    v.handle(&press(Key::End, Modifiers::NONE));
    assert_eq!(v.cursor().0, v.tracks().len() - 1);
    // An empty slot's Enter does nothing.
    v.set_cursor(0, 6);
    assert!(v.handle(&press(Key::Enter, Modifiers::NONE)).is_empty());
}

#[test]
fn the_edit_commands_act_on_the_slot_under_the_cursor() {
    let doc = doc_with_slots();
    let mut v = view(&doc);
    let out = |o: CommandOutcome| match o {
        CommandOutcome::Handled(a) => a,
        other => panic!("{other:?}"),
    };
    let a = out(v.command("edit.delete", None));
    assert_eq!(
        launcher_cmds(&a),
        vec![LauncherCommand::ClearSlots {
            slots: vec![(key("bass"), 0)]
        }]
    );
    assert!(out(v.command("edit.copy", None)).is_empty());
    v.set_cursor(1, 4);
    let a = out(v.command("edit.paste", None));
    assert!(matches!(
        launcher_cmds(&a)[..],
        [LauncherCommand::SetSlot { row: 4, .. }]
    ));
    v.set_cursor(0, 1);
    let a = out(v.command("edit.duplicate", None));
    assert_eq!(
        launcher_cmds(&a),
        vec![LauncherCommand::MoveSlot {
            from: (key("bass"), 1),
            to: (key("bass"), 2),
            copy: true
        }]
    );
    let a = out(v.command("edit.cut", None));
    assert_eq!(launcher_cmds(&a).len(), 1);
    // An empty slot has nothing to cut, copy, delete or duplicate; paste is the only way in.
    v.set_cursor(0, 7);
    for id in ["edit.cut", "edit.copy", "edit.delete", "edit.duplicate"] {
        assert!(matches!(
            v.command(id, None),
            CommandOutcome::NotApplicable(_)
        ));
    }
    assert!(matches!(
        v.command("edit.select_all", None),
        CommandOutcome::Declined
    ));
}

#[test]
fn the_right_click_list_edits_a_slot_and_a_scene() {
    let doc = doc_with_slots();
    let mut v = view(&doc);
    // A slot: Colour > Red.
    v.handle(&right_down(slot_of(&v, 0, 0)));
    assert_eq!(
        v.menu_labels(),
        [
            "Rename",
            "Colour",
            "Loop Length",
            "Quantize",
            "Duplicate",
            "Delete"
        ]
    );
    let click = |v: &mut LauncherView, i: usize| {
        let at = v.menu_row_rect(i).expect("row").center();
        v.handle(&down(at))
    };
    assert!(click(&mut v, 1).is_empty());
    assert_eq!(v.menu_labels()[0], "Track colour");
    let red = v.menu_labels().iter().position(|l| l == "red").unwrap();
    let a = click(&mut v, red);
    let cmds = launcher_cmds(&a);
    assert!(matches!(
        &cmds[..],
        [LauncherCommand::EditSlot {
            color: MetaChange::Set(_),
            row: 0,
            ..
        }]
    ));
    // Loop length: 2 bars.
    v.handle(&right_down(slot_of(&v, 0, 0)));
    click(&mut v, 2);
    assert_eq!(v.menu_labels()[1], "2 bars");
    let a = click(&mut v, 1);
    assert!(matches!(
        &launcher_cmds(&a)[..],
        [LauncherCommand::EditSlot { length: Some(l), .. }] if *l == 2 * BAR
    ));
    // Quantize: this slot follows the song, or its own value.
    v.handle(&right_down(slot_of(&v, 0, 0)));
    click(&mut v, 3);
    assert_eq!(v.menu_labels()[0], "Song setting");
    let a = click(&mut v, 3);
    assert!(matches!(
        &launcher_cmds(&a)[..],
        [LauncherCommand::EditSlot {
            quantize: Some(Some(_)),
            ..
        }]
    ));
    // Delete and Duplicate.
    v.handle(&right_down(slot_of(&v, 0, 0)));
    let a = click(&mut v, 5);
    assert!(matches!(
        &launcher_cmds(&a)[..],
        [LauncherCommand::ClearSlots { .. }]
    ));
    v.handle(&right_down(slot_of(&v, 0, 0)));
    let a = click(&mut v, 4);
    assert!(matches!(
        &launcher_cmds(&a)[..],
        [LauncherCommand::MoveSlot { copy: true, .. }]
    ));
    // A scene.
    v.handle(&right_down(v.scene_rect(1).unwrap().center()));
    assert_eq!(
        v.menu_labels(),
        ["Rename", "Insert Scene", "Duplicate Scene", "Delete Scene"]
    );
    let a = click(&mut v, 3);
    assert_eq!(
        launcher_cmds(&a),
        vec![LauncherCommand::DeleteScene { at: 1 }]
    );
    // A click outside closes the list; an empty slot has no list.
    v.handle(&right_down(slot_of(&v, 0, 0)));
    v.handle(&down(Point::new(1.0, 1.0)));
    assert!(v.menu_labels().is_empty());
    v.handle(&right_down(slot_of(&v, 0, 6)));
    assert!(v.menu_labels().is_empty());
}

#[test]
fn renaming_a_slot_and_a_scene_types_into_the_view() {
    let doc = doc_with_slots();
    let mut v = view(&doc);
    assert!(!v.is_text_owner());
    // Slot: Rename from the list.
    v.handle(&right_down(slot_of(&v, 0, 0)));
    let at = v.menu_row_rect(0).unwrap().center();
    v.handle(&down(at));
    assert!(v.is_text_owner());
    for ch in "xy".chars() {
        v.handle(&press(Key::Char(ch), Modifiers::NONE));
    }
    let a = v.handle(&press(Key::Enter, Modifiers::NONE));
    assert!(!v.is_text_owner());
    let cmds = launcher_cmds(&a);
    assert!(matches!(
        &cmds[..],
        [LauncherCommand::EditSlot { name: MetaChange::Set(n), row: 0, .. }] if n.ends_with("xy")
    ));
    // Scene: double click, replace the text, Escape cancels.
    v.handle(&down_mods(
        v.scene_rect(2).unwrap().center(),
        Modifiers::NONE,
        2,
    ));
    assert!(v.is_text_owner());
    v.handle(&press(Key::Char('q'), Modifiers::NONE));
    let a = v.handle(&press(Key::Escape, Modifiers::NONE));
    assert!(a.is_empty() && !v.is_text_owner());
    // Enter with the text cleared goes back to "Scene N".
    v.handle(&down_mods(
        v.scene_rect(0).unwrap().center(),
        Modifiers::NONE,
        2,
    ));
    v.handle(&press(Key::Backspace, Modifiers::NONE));
    for _ in 0..10 {
        v.handle(&press(Key::Backspace, Modifiers::NONE));
    }
    let a = v.handle(&press(Key::Enter, Modifiers::NONE));
    assert_eq!(
        launcher_cmds(&a),
        vec![LauncherCommand::RenameScene { at: 0, name: None }]
    );
}

#[test]
fn the_wheel_scrolls_rows_and_the_cursor_scrolls_into_view() {
    let mut doc = song_doc();
    doc.song_mut().launcher = Default::default();
    let mut v = view(&doc);
    assert!(v.slot_rect(0, 0).is_some());
    v.set_cursor(0, 7);
    assert!(v.slot_rect(0, 6).is_some());
    for _ in 0..5 {
        v.handle(&UiEvent::Wheel {
            delta: atomik_ui::kurbo::Vec2::new(0.0, -1.0),
            pos: Point::new(100.0, 200.0),
            modifiers: Modifiers::NONE,
        });
    }
    assert!(v.slot_rect(0, 0).is_none(), "scrolled out");
}

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

fn golden_view(scale: f64) -> LauncherView {
    let doc = doc_with_slots();
    let mut v = view(&doc);
    v.set_scale(scale);
    v.set_focused(true);
    // Bass plays scene 1 (half way), lead waits to launch scene 1, keys waits to stop.
    let mut f = LauncherFeed::default();
    f.tracks.insert(
        key("bass"),
        TrackLaunch {
            playing: Some(0),
            queued: Some(1),
            queued_stop: false,
            phase: 0.5,
        },
    );
    f.tracks.insert(
        key("lead"),
        TrackLaunch {
            playing: None,
            queued: Some(0),
            queued_stop: false,
            phase: 0.0,
        },
    );
    f.tracks.insert(
        key("keys"),
        TrackLaunch {
            playing: Some(2),
            queued: None,
            queued_stop: true,
            phase: 0.2,
        },
    );
    f.record = true;
    f.blink = true;
    v.set_feed(f);
    v.set_cursor(1, 1);
    v
}

#[test]
fn the_view_with_playing_queued_and_empty_slots_matches_its_golden_at_1x() {
    let v = golden_view(1.0);
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    check("launcher_view_1x.json", &dl.to_json());
}

#[test]
fn the_view_in_french_matches_its_golden() {
    let (list, misses) = atomik_ui::i18n::with_misses(|| {
        atomik_ui::i18n::with_language(atomik_ui::Lang::French, || {
            let v = golden_view(1.0);
            let mut dl = DisplayList::new();
            v.paint(&mut dl);
            dl
        })
    });
    check("fr/launcher_view_1x.json", &list.to_json());
    // Only the names the test gave the tracks and the scenes are left in English.
    let words: Vec<_> = misses
        .iter()
        .filter(|m| {
            !["Bass", "Lead", "Keys", "Intro"]
                .iter()
                .any(|n| m.starts_with(n))
                && !m.chars().any(|c| c.is_ascii_digit())
        })
        .collect();
    assert!(words.is_empty(), "no French entry: {words:?}");
}

#[test]
fn the_view_matches_its_golden_at_2x() {
    let v = golden_view(2.0);
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    check("launcher_view_2x.json", &dl.to_json());
}

#[test]
fn the_view_follows_the_theme_and_paints_without_tracks() {
    let mut doc = song_doc();
    doc.song_mut().tracks.clear();
    let mut v = LauncherView::new(Theme::light());
    v.set_bounds(BOUNDS);
    v.sync(&doc);
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    assert!(dl.to_json().contains("No note tracks yet"));
    assert!(!v.needs_repaint());
    v.set_theme(Theme::dark());
    assert!(v.needs_repaint());
    let _ = Command::SetRackSort(atomik_model::RackSort::Free);
}

#[test]
fn the_launch_commands_are_the_enter_keys_of_the_launcher_context() {
    let doc = doc_with_slots();
    let mut v = view(&doc);
    let handled = |o: CommandOutcome| match o {
        CommandOutcome::Handled(a) => a,
        other => panic!("{other:?}"),
    };
    let a = handled(v.command("launcher.launch", None));
    assert_eq!(
        requests(&a),
        vec![LauncherAction::Launch {
            device: key("bass"),
            row: 0
        }]
    );
    let a = handled(v.command("launcher.launch_scene", None));
    assert_eq!(requests(&a), vec![LauncherAction::LaunchScene(0)]);
    // An empty slot of a track that plays nothing has nothing to launch.
    v.set_cursor(0, 6);
    assert!(matches!(
        v.command("launcher.launch", None),
        CommandOutcome::NotApplicable(_)
    ));
    // On a playing track the empty slot's Enter stops the track.
    v.set_feed(feed(
        "bass",
        TrackLaunch {
            playing: Some(0),
            ..TrackLaunch::default()
        },
    ));
    let a = handled(v.command("launcher.launch", None));
    assert_eq!(requests(&a), vec![LauncherAction::Stop(key("bass"))]);
}
