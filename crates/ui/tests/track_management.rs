//! Track management in the arrangement: dragging track headers (one or several tracks, into and
//! out of folders, copying with the copy role), resizing several tracks together, minimising,
//! deleting a track with its device and colouring a track and its device, run against a real
//! `Document` so the undo steps count.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{Command, Document, SongCommand, TrackPlace, WorkflowCommand};
use atomik_ui::keymap::CommandOutcome;
use atomik_ui::kurbo::Point;
use atomik_ui::timeline::Snap;
use atomik_ui::{ArrangementView, Key, Modifiers, PointerButton, UiAction, UiEvent};
use support::song::*;
use support::*;

/// The song of the other arrangement tests (`bass` with a cutoff lane, `lead`, `keys`) and a
/// fourth track, `pad`, in the order bass, lead, keys, pad.
fn setup() -> (Document, Registry, ArrangementView) {
    let reg = registry();
    let mut d = Document::new(song_doc());
    d.apply(
        Command::Song(SongCommand::AddTrack { device: key("pad") }),
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

fn run(d: &Document, reg: &Registry, v: &mut ArrangementView, events: &[UiEvent]) -> Vec<UiAction> {
    events
        .iter()
        .flat_map(|e| v.handle(e, d.doc(), reg))
        .collect()
}

/// Applies the commands among `actions` and syncs the view.
fn apply(d: &mut Document, reg: &Registry, v: &mut ArrangementView, actions: &[UiAction]) {
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
    v.sync(d.doc(), reg);
}

fn command(
    d: &Document,
    reg: &Registry,
    v: &mut ArrangementView,
    id: &str,
    arg: Option<&str>,
) -> CommandOutcome {
    v.command(id, arg, d.doc(), reg)
}

/// Runs a command that must be handled and applies what it asks for.
fn do_command(
    d: &mut Document,
    reg: &Registry,
    v: &mut ArrangementView,
    id: &str,
    arg: Option<&str>,
) -> Vec<UiAction> {
    let CommandOutcome::Handled(actions) = command(d, reg, v, id, arg) else {
        panic!("{id} must be handled");
    };
    apply(d, reg, v, &actions);
    actions
}

fn order(d: &Document) -> Vec<String> {
    d.doc()
        .song
        .tracks
        .iter()
        .map(|t| t.device.0.clone())
        .collect()
}

fn base_height(d: &Document, k: &str) -> Option<u16> {
    d.doc().song.track_heights.get(&key(k)).copied()
}

fn song_cmds(actions: &[UiAction]) -> Vec<SongCommand> {
    song_commands(actions)
}

/// Selects `bass` and `lead` (neighbours) and groups them: folder 1.
fn grouped() -> (Document, Registry, ArrangementView) {
    let (mut d, reg, mut v) = setup();
    v.set_selected_track(Some(key("bass")));
    v.toggle_selected_track(&key("lead"));
    do_command(&mut d, &reg, &mut v, "sequencer.group_tracks", None);
    (d, reg, v)
}

fn place(to_index: usize, into: Option<u32>) -> TrackPlace {
    TrackPlace { to_index, into }
}

// ------------------------------------------------------------------ dragging tracks

/// Presses the header of `from` and moves to `to` in two steps (the release is separate, so a
/// test can look at the drop line first).
fn drag_header(
    d: &Document,
    reg: &Registry,
    v: &mut ArrangementView,
    from: &str,
    to: Point,
    modifiers: Modifiers,
) {
    let start = v.track_header_center(&key(from)).unwrap();
    let events = [
        down(start),
        mv_with(Point::new(start.x, (start.y + to.y) / 2.0), modifiers),
        mv_with(to, modifiers),
    ];
    let sent = run(d, reg, v, &events);
    assert!(sent.is_empty(), "nothing is sent while dragging: {sent:?}");
}

fn release(d: &Document, reg: &Registry, v: &mut ArrangementView, at: Point) -> Vec<UiAction> {
    run(d, reg, v, &[up(at)])
}

/// The top of the first and the bottom of the last row of a track that has `lanes` lanes.
fn block(v: &ArrangementView, device: &str, lanes: usize) -> (f64, f64) {
    let first = v.lane_rect(&key(device), 0).unwrap();
    let last = v.lane_rect(&key(device), lanes - 1).unwrap();
    (first.y0, last.y1)
}

#[test]
fn dragging_a_header_shows_an_insertion_line_and_the_release_moves_the_track() {
    let (mut d, reg, mut v) = setup();
    let (top, _) = block(&v, "bass", 2);
    let to = Point::new(100.0, top + 4.0);
    drag_header(&d, &reg, &mut v, "pad", to, Modifiers::NONE);
    let drop = v.track_drop().expect("a place to drop at");
    assert_eq!(drop.y, top, "the line is above bass");
    assert_eq!(drop.depth, 0);
    let actions = release(&d, &reg, &mut v, to);
    assert_eq!(
        song_cmds(&actions),
        [SongCommand::MoveTracks {
            devices: vec![key("pad")],
            place: place(0, None),
        }]
    );
    assert!(v.track_drop().is_none(), "the drag is over");
    apply(&mut d, &reg, &mut v, &actions);
    assert_eq!(order(&d), ["pad", "bass", "lead", "keys"]);
    d.undo(&reg).unwrap();
    assert_eq!(order(&d), ["bass", "lead", "keys", "pad"]);
}

#[test]
fn the_lower_part_of_a_track_is_the_place_after_it() {
    let (d, reg, mut v) = setup();
    let (_, bottom) = block(&v, "bass", 2);
    let to = Point::new(100.0, bottom - 3.0);
    drag_header(&d, &reg, &mut v, "pad", to, Modifiers::NONE);
    let drop = v.track_drop().unwrap();
    assert_eq!(drop.y, bottom, "after bass and its automation lane");
    let actions = release(&d, &reg, &mut v, to);
    assert_eq!(
        song_cmds(&actions),
        [SongCommand::MoveTracks {
            devices: vec![key("pad")],
            place: place(1, None),
        }]
    );
}

#[test]
fn several_selected_tracks_drag_together_to_the_end_of_the_list() {
    let (mut d, reg, mut v) = setup();
    v.set_selected_track(Some(key("bass")));
    v.toggle_selected_track(&key("keys"));
    let last = v.lane_rect(&key("pad"), 0).unwrap();
    let to = Point::new(100.0, last.y1 + 30.0);
    drag_header(&d, &reg, &mut v, "keys", to, Modifiers::NONE);
    assert_eq!(v.selected_tracks().len(), 2, "the press kept the selection");
    let actions = release(&d, &reg, &mut v, to);
    assert_eq!(
        song_cmds(&actions),
        [SongCommand::MoveTracks {
            devices: vec![key("bass"), key("keys")],
            place: place(2, None),
        }]
    );
    apply(&mut d, &reg, &mut v, &actions);
    assert_eq!(order(&d), ["lead", "pad", "bass", "keys"]);
    d.undo(&reg).unwrap();
    assert_eq!(order(&d), ["bass", "lead", "keys", "pad"], "one undo step");
}

#[test]
fn a_click_on_one_of_several_selected_tracks_picks_it_on_release() {
    let (d, reg, mut v) = setup();
    v.set_selected_track(Some(key("bass")));
    v.toggle_selected_track(&key("keys"));
    let at = v.track_header_center(&key("keys")).unwrap();
    let actions = run(&d, &reg, &mut v, &[down(at), up(at)]);
    assert!(song_cmds(&actions).is_empty());
    assert_eq!(v.selected_tracks(), [key("keys")]);
}

#[test]
fn dragging_into_a_folder_and_out_of_it_uses_the_folder_aware_places() {
    let (mut d, reg, mut v) = grouped();
    // bass and lead are in folder 1; the folder row is above bass.
    let folder = v.folder_header_center(1).unwrap();
    let (lead_top, lead_bottom) = block(&v, "lead", 1);
    // The lower part of lead: after it, inside the folder.
    let to = Point::new(100.0, lead_bottom - 3.0);
    drag_header(&d, &reg, &mut v, "pad", to, Modifiers::NONE);
    assert_eq!(v.track_drop().unwrap().depth, 1, "the line is indented");
    let actions = release(&d, &reg, &mut v, to);
    assert_eq!(
        song_cmds(&actions),
        [SongCommand::MoveTracks {
            devices: vec![key("pad")],
            place: place(2, Some(1)),
        }]
    );
    apply(&mut d, &reg, &mut v, &actions);
    assert_eq!(order(&d), ["bass", "lead", "pad", "keys"]);
    assert_eq!(d.doc().song.track_group(&key("pad")), Some(1));
    // The upper part of the folder row: before the folder, at the top level.
    let to = Point::new(100.0, folder.y - 10.0);
    drag_header(&d, &reg, &mut v, "pad", to, Modifiers::NONE);
    assert_eq!(v.track_drop().unwrap().depth, 0);
    let actions = release(&d, &reg, &mut v, to);
    apply(&mut d, &reg, &mut v, &actions);
    assert_eq!(order(&d), ["pad", "bass", "lead", "keys"]);
    assert_eq!(d.doc().song.track_group(&key("pad")), None);
    // The upper part of lead is a place inside the folder: a track dropped there joins it.
    let to = Point::new(100.0, lead_top + 3.0);
    drag_header(&d, &reg, &mut v, "keys", to, Modifiers::NONE);
    assert_eq!(v.track_drop().unwrap().depth, 1, "joins the folder there");
    release(&d, &reg, &mut v, to);
}

#[test]
fn escape_cancels_a_track_drag_and_a_place_that_changes_nothing_shows_no_line() {
    let (d, reg, mut v) = setup();
    let (top, _) = block(&v, "bass", 2);
    let to = Point::new(100.0, top + 4.0);
    drag_header(&d, &reg, &mut v, "pad", to, Modifiers::NONE);
    assert!(v.track_drop().is_some());
    let actions = run(&d, &reg, &mut v, &[press(Key::Escape, Modifiers::NONE)]);
    assert!(song_cmds(&actions).is_empty());
    assert!(v.track_drop().is_none());
    assert!(song_cmds(&release(&d, &reg, &mut v, to)).is_empty());
    // Dropping where the track already is changes nothing: no line, no command.
    let (top, _) = block(&v, "pad", 1);
    let to = Point::new(100.0, top + 4.0);
    drag_header(&d, &reg, &mut v, "pad", to, Modifiers::NONE);
    assert!(v.track_drop().is_none());
    assert!(song_cmds(&release(&d, &reg, &mut v, to)).is_empty());
}

#[test]
fn a_track_drag_with_the_copy_role_duplicates_the_tracks_to_the_place() {
    use atomik_ui::keymap::{Os, Roles};
    let (d, reg, mut v) = setup();
    // The same roles on every host, so the copy modifier is the same one everywhere.
    v.set_roles(Roles::defaults(Os::Windows));
    let (top, _) = block(&v, "bass", 2);
    let to = Point::new(100.0, top + 4.0);
    let ctrl = Modifiers {
        ctrl: true,
        ..Modifiers::NONE
    };
    drag_header(&d, &reg, &mut v, "keys", to, ctrl);
    assert!(v.track_drop().is_some());
    let actions = release(&d, &reg, &mut v, to);
    assert_eq!(
        actions,
        [UiAction::Command(Command::Workflow(
            WorkflowCommand::DuplicateTracks {
                devices: vec![key("keys")],
                place: Some(place(0, None)),
            }
        ))]
    );
}

// ------------------------------------------------------------------ resizing and minimising

fn drag_edge(
    d: &Document,
    reg: &Registry,
    v: &mut ArrangementView,
    device: &str,
    dy: f64,
    modifiers: Modifiers,
) -> Vec<UiAction> {
    let start = v.track_edge_center(&key(device)).unwrap();
    let to = Point::new(start.x, start.y + dy);
    run(
        d,
        reg,
        v,
        &[
            down(start),
            mv_with(to, modifiers),
            UiEvent::PointerUp {
                pos: to,
                button: PointerButton::Primary,
                modifiers,
            },
        ],
    )
}

fn set_heights(d: &mut Document, reg: &Registry, v: &mut ArrangementView, h: &[(&str, u16)]) {
    let map = h.iter().map(|(k, h)| (key(k), *h)).collect();
    d.apply(Command::Song(SongCommand::SetTrackHeights(map)), reg)
        .unwrap();
    v.sync(d.doc(), reg);
}

#[test]
fn resizing_a_selected_track_scales_every_selected_track_by_the_same_factor() {
    let (mut d, reg, mut v) = setup();
    set_heights(
        &mut d,
        &reg,
        &mut v,
        &[("bass", 40), ("lead", 60), ("keys", 80), ("pad", 32)],
    );
    v.set_selected_track(Some(key("bass")));
    v.toggle_selected_track(&key("lead"));
    v.toggle_selected_track(&key("keys"));
    // bass 40 -> 60 is a factor of 1.5.
    let actions = drag_edge(&d, &reg, &mut v, "bass", 20.0, Modifiers::NONE);
    apply(&mut d, &reg, &mut v, &actions);
    assert_eq!(base_height(&d, "bass"), Some(60));
    assert_eq!(base_height(&d, "lead"), Some(90));
    assert_eq!(base_height(&d, "keys"), Some(120));
    assert_eq!(base_height(&d, "pad"), Some(32), "not selected");
    // One merge group: one undo gives the heights back.
    d.undo(&reg).unwrap();
    assert_eq!(base_height(&d, "bass"), Some(40));
    assert_eq!(base_height(&d, "lead"), Some(60));
    assert_eq!(base_height(&d, "keys"), Some(80));
}

#[test]
fn resizing_with_alt_gives_every_selected_track_the_same_height() {
    let (mut d, reg, mut v) = setup();
    set_heights(&mut d, &reg, &mut v, &[("bass", 40), ("lead", 60)]);
    v.set_selected_track(Some(key("bass")));
    v.toggle_selected_track(&key("lead"));
    let actions = drag_edge(&d, &reg, &mut v, "bass", 20.0, alt());
    apply(&mut d, &reg, &mut v, &actions);
    assert_eq!(base_height(&d, "bass"), Some(60));
    assert_eq!(base_height(&d, "lead"), Some(60));
    // A drag of a track that is not selected resizes it alone.
    v.set_selected_track(Some(key("lead")));
    let actions = drag_edge(&d, &reg, &mut v, "keys", 10.0, Modifiers::NONE);
    apply(&mut d, &reg, &mut v, &actions);
    assert_eq!(base_height(&d, "keys"), Some(70));
    assert_eq!(base_height(&d, "lead"), Some(60));
}

#[test]
fn minimise_shortens_the_selected_tracks_and_a_second_press_restores_their_heights() {
    let (mut d, reg, mut v) = setup();
    set_heights(&mut d, &reg, &mut v, &[("lead", 120)]);
    v.set_selected_track(Some(key("lead")));
    v.toggle_selected_track(&key("keys"));
    do_command(
        &mut d,
        &reg,
        &mut v,
        "sequencer.track_height",
        Some("minimise"),
    );
    assert_eq!(base_height(&d, "lead"), Some(24));
    assert_eq!(base_height(&d, "keys"), Some(24));
    assert_eq!(base_height(&d, "bass"), None, "not selected");
    assert_eq!(v.track_height(&key("lead")), Some(24.0));
    do_command(
        &mut d,
        &reg,
        &mut v,
        "sequencer.track_height",
        Some("minimise"),
    );
    assert_eq!(base_height(&d, "lead"), Some(120), "what it was");
    assert_eq!(base_height(&d, "keys"), None, "the default again");
}

// ------------------------------------------------------------------ delete and colour

#[test]
fn delete_track_and_device_removes_the_selected_tracks_devices_in_one_step() {
    let (mut d, reg, mut v) = setup();
    assert_eq!(
        command(&d, &reg, &mut v, "sequencer.delete_track_and_device", None),
        CommandOutcome::Declined,
        "nothing selected"
    );
    v.set_selected_track(Some(key("lead")));
    v.toggle_selected_track(&key("pad"));
    let actions = do_command(
        &mut d,
        &reg,
        &mut v,
        "sequencer.delete_track_and_device",
        None,
    );
    assert_eq!(
        actions,
        [UiAction::Command(Command::Workflow(
            WorkflowCommand::DeleteDevices {
                keys: vec![key("lead"), key("pad")]
            }
        ))]
    );
    assert_eq!(order(&d), ["bass", "keys"]);
    assert!(d.doc().device(&key("lead")).is_none());
    assert!(v.selected_tracks().is_empty());
    d.undo(&reg).unwrap();
    assert_eq!(order(&d), ["bass", "lead", "keys", "pad"]);
    assert!(d.doc().device(&key("lead")).is_some());
}

#[test]
fn a_track_colour_also_colours_the_device_and_one_undo_restores_both() {
    let (mut d, reg, mut v) = setup();
    let colour = |d: &Document, k: &str| d.doc().device(&key(k)).unwrap().color;
    let before = ["bass", "lead", "keys"].map(|k| colour(&d, k));
    v.set_selected_track(Some(key("bass")));
    v.toggle_selected_track(&key("keys"));
    do_command(&mut d, &reg, &mut v, "sequencer.track_color", Some("blue"));
    let blue = Some([0x3f, 0x8f, 0xd2]);
    for k in ["bass", "keys"] {
        assert_eq!(d.doc().song.track_color(&key(k)), blue, "{k}");
        assert_eq!(colour(&d, k), blue, "{k}");
    }
    assert_eq!(colour(&d, "lead"), before[1], "not selected");
    d.undo(&reg).unwrap();
    for k in ["bass", "keys"] {
        assert_eq!(d.doc().song.track_color(&key(k)), None, "{k}");
    }
    assert_eq!(["bass", "lead", "keys"].map(|k| colour(&d, k)), before);
    // None clears both.
    do_command(&mut d, &reg, &mut v, "sequencer.track_color", Some("none"));
    assert_eq!(d.doc().song.track_color(&key("bass")), None);
    assert_eq!(colour(&d, "bass"), None);
}

// ------------------------------------------------------------------ folders

#[test]
fn alt_clicking_a_chevron_collapses_every_folder_in_one_step_and_again_expands_them() {
    let (mut d, reg, mut v) = grouped();
    // A second folder around keys and pad.
    v.set_selected_track(Some(key("keys")));
    v.toggle_selected_track(&key("pad"));
    do_command(&mut d, &reg, &mut v, "sequencer.group_tracks", None);
    assert_eq!(d.doc().song.org.groups.len(), 2);
    let toggle = v.folder_toggle_center(1).unwrap();
    let alt_click = |d: &Document, v: &mut ArrangementView, at: Point| {
        run(
            d,
            &reg,
            v,
            &[
                UiEvent::PointerDown {
                    pos: at,
                    button: PointerButton::Primary,
                    modifiers: alt(),
                    clicks: 1,
                },
                up(at),
            ],
        )
    };
    let actions = alt_click(&d, &mut v, toggle);
    apply(&mut d, &reg, &mut v, &actions);
    assert!(d.doc().song.org.groups.iter().all(|g| g.collapsed));
    d.undo(&reg).unwrap();
    assert!(
        d.doc().song.org.groups.iter().all(|g| !g.collapsed),
        "one undo step"
    );
    d.redo(&reg).unwrap();
    v.sync(d.doc(), &reg);
    // Alt on a chevron of a collapsed folder expands them all again.
    let toggle = v.folder_toggle_center(2).unwrap();
    let actions = alt_click(&d, &mut v, toggle);
    apply(&mut d, &reg, &mut v, &actions);
    assert!(d.doc().song.org.groups.iter().all(|g| !g.collapsed));
}
