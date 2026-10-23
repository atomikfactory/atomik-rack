//! The arrangement's registry commands and tools: the
//! shell resolves a key to a command id and offers it to the focused view, so these tests drive
//! `ArrangementView::command` the way the router does.

mod support;

use atomik_model::{ClipId, Command, SongCommand, WorkflowCommand};
use atomik_ui::keymap::{CommandOutcome, Os, Roles};
use atomik_ui::kurbo::Point;
use atomik_ui::timeline::Snap;
use atomik_ui::{
    ArrangementView, DisplayList, Modifiers, Tool, TransportAction, UiAction, UiEvent,
};
use support::song::*;
use support::*;

fn lane_point(v: &ArrangementView, dev: &str, tick: i64) -> Point {
    let r = v.lane_rect(&key(dev), 0).unwrap();
    Point::new(v.tick_to_x(tick), r.center().y + 6.0)
}

fn click(v: &mut ArrangementView, p: Point, m: Modifiers) -> Vec<UiAction> {
    let (doc, reg) = (song_doc(), registry());
    run(
        |e| v.handle(e, &doc, &reg),
        &[down_mods(p, m, 1), up_mods(p, m)],
    )
}

fn up_mods(pos: Point, modifiers: Modifiers) -> UiEvent {
    UiEvent::PointerUp {
        pos,
        button: atomik_ui::PointerButton::Primary,
        modifiers,
    }
}

fn setup() -> (
    ArrangementView,
    atomik_model::RackDoc,
    atomik_device_sdk::Registry,
) {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    v.set_roles(Roles::defaults(Os::Windows));
    (v, doc, reg)
}

fn select(v: &mut ArrangementView, ids: &[u64]) {
    let ids: Vec<ClipId> = ids.iter().map(|i| ClipId(*i)).collect();
    v.set_selected_clips(&ids);
}

// ---------------------------------------------------------------- edit commands

#[test]
fn select_all_duplicate_and_delete_are_commands() {
    let (mut v, doc, reg) = setup();
    assert_eq!(
        v.command("edit.select_all", None, &doc, &reg),
        CommandOutcome::Handled(vec![])
    );
    assert_eq!(v.selected_clips().len(), 5);
    select(&mut v, &[1, 3]);
    let d = v.command("edit.duplicate", None, &doc, &reg);
    assert_eq!(
        d.actions(),
        [UiAction::Command(Command::Workflow(
            WorkflowCommand::DuplicateClips {
                ids: vec![ClipId(1), ClipId(3)]
            }
        ))]
    );
    let del = v.command("edit.delete", None, &doc, &reg);
    assert_eq!(
        del.actions(),
        [UiAction::Command(Command::Workflow(
            WorkflowCommand::RemoveClips {
                ids: vec![ClipId(1), ClipId(3)]
            }
        ))]
    );
}

#[test]
fn duplicate_and_delete_decline_without_a_selection_and_unknown_ids_decline() {
    let (mut v, doc, reg) = setup();
    assert_eq!(
        v.command("edit.duplicate", None, &doc, &reg),
        CommandOutcome::Declined
    );
    assert_eq!(
        v.command("edit.delete", None, &doc, &reg),
        CommandOutcome::Declined
    );
    assert_eq!(
        v.command("editor.quantize", None, &doc, &reg),
        CommandOutcome::Declined,
        "the piano roll's command"
    );
    assert_eq!(
        v.command("file.save", None, &doc, &reg),
        CommandOutcome::Declined
    );
}

#[test]
fn split_at_playhead_splits_the_selection_else_every_clip_under_the_playhead() {
    let (mut v, doc, reg) = setup();
    v.set_playhead(2 * BAR + PPQ);
    select(&mut v, &[1, 3]);
    let s = v.command("sequencer.split_at_playhead", None, &doc, &reg);
    assert_eq!(
        song_commands(s.actions()),
        vec![
            SongCommand::SplitClip {
                id: ClipId(1),
                at: 2 * BAR + PPQ
            },
            SongCommand::SplitClip {
                id: ClipId(3),
                at: 2 * BAR + PPQ
            },
        ]
    );
    select(&mut v, &[]);
    v.set_playhead(BAR + PPQ);
    let s = v.command("sequencer.split_at_playhead", None, &doc, &reg);
    let ids: Vec<ClipId> = song_commands(s.actions())
        .into_iter()
        .map(|c| match c {
            SongCommand::SplitClip { id, at } => {
                assert_eq!(at, BAR + PPQ);
                id
            }
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(ids, vec![ClipId(1), ClipId(4), ClipId(2), ClipId(3)]);
}

#[test]
fn open_clip_opens_the_selected_note_clip_and_declines_otherwise() {
    let (mut v, doc, reg) = setup();
    assert_eq!(
        v.command("sequencer.open_clip", None, &doc, &reg),
        CommandOutcome::Declined,
        "nothing selected: Enter goes on to return to zero"
    );
    select(&mut v, &[4]);
    assert_eq!(
        v.command("sequencer.open_clip", None, &doc, &reg),
        CommandOutcome::Declined,
        "an automation clip has no editor"
    );
    select(&mut v, &[3, 2]);
    assert_eq!(
        v.command("sequencer.open_clip", None, &doc, &reg),
        CommandOutcome::Handled(vec![UiAction::OpenPianoRoll(ClipId(2))]),
        "the first selected note clip in song order"
    );
}

#[test]
fn neighbour_selection_moves_through_the_clips_in_time_order() {
    let (mut v, doc, reg) = setup();
    v.command("sequencer.select_next_clip", None, &doc, &reg);
    assert_eq!(v.selected_clips().len(), 1, "nothing selected: the first");
    let first = v.selected_clips()[0];
    v.command("sequencer.select_next_clip", None, &doc, &reg);
    let second = v.selected_clips()[0];
    assert_ne!(first, second);
    v.command("sequencer.select_prev_clip", None, &doc, &reg);
    assert_eq!(v.selected_clips(), vec![first]);
    // The end of the list stays put.
    for _ in 0..20 {
        v.command("sequencer.select_next_clip", None, &doc, &reg);
    }
    let last = v.selected_clips();
    v.command("sequencer.select_next_clip", None, &doc, &reg);
    assert_eq!(v.selected_clips(), last);
}

#[test]
fn loop_to_selection_and_loop_and_play() {
    let (mut v, doc, reg) = setup();
    assert_eq!(
        v.command("transport.loop_to_selection", None, &doc, &reg),
        CommandOutcome::Declined
    );
    select(&mut v, &[2, 5]);
    let a = v.command("transport.loop_to_selection", None, &doc, &reg);
    assert_eq!(
        song_commands(a.actions()),
        vec![SongCommand::SetLoop {
            range: (BAR, 6 * BAR),
            enabled: true
        }]
    );
    let p = v.command("transport.loop_and_play", None, &doc, &reg);
    assert_eq!(
        song_commands(p.actions()),
        vec![SongCommand::SetLoop {
            range: (BAR, 6 * BAR),
            enabled: true
        }]
    );
    assert!(
        p.actions()
            .contains(&UiAction::Transport(TransportAction::Locate(BAR)))
            && p.actions()
                .contains(&UiAction::Transport(TransportAction::Play)),
        "P starts looped playback from the loop start"
    );
}

// ---------------------------------------------------------------- snap, zoom, follow

#[test]
fn snap_toggle_remembers_the_value_and_set_snap_takes_an_argument() {
    let (mut v, doc, reg) = setup();
    v.set_snap(Snap::Eighth);
    v.command("sequencer.snap_toggle", None, &doc, &reg);
    assert_eq!(v.snap(), Snap::Off);
    v.command("sequencer.snap_toggle", None, &doc, &reg);
    assert_eq!(v.snap(), Snap::Eighth);
    for (arg, want) in [
        ("bar", Snap::Bar),
        ("beat", Snap::Beat),
        ("eighth", Snap::Eighth),
        ("sixteenth", Snap::Sixteenth),
        ("thirtysecond", Snap::ThirtySecond),
        ("sixtyfourth", Snap::SixtyFourth),
        ("quarter_triplet", Snap::QuarterTriplet),
        ("eighth_triplet", Snap::EighthTriplet),
        ("sixteenth_triplet", Snap::SixteenthTriplet),
        ("thirtysecond_triplet", Snap::ThirtySecondTriplet),
        ("grid", Snap::Grid),
        ("off", Snap::Off),
    ] {
        assert!(
            v.command("sequencer.set_snap", Some(arg), &doc, &reg)
                .is_taken()
        );
        assert_eq!(v.snap(), want, "{arg}");
    }
    assert_eq!(
        v.command("sequencer.set_snap", Some("nope"), &doc, &reg),
        CommandOutcome::Declined
    );
}

#[test]
fn zoom_commands_change_the_time_zoom_and_keep_the_anchor_visible() {
    let (mut v, doc, reg) = setup();
    let before = v.px_per_tick();
    assert!(v.command("view.zoom_in", None, &doc, &reg).is_taken());
    assert!(v.px_per_tick() > before);
    assert!(v.command("view.zoom_out", None, &doc, &reg).is_taken());
    assert!((v.px_per_tick() - before).abs() < 1e-12);
    v.command("view.zoom_out", None, &doc, &reg);
    assert!(v.px_per_tick() < before);
}

#[test]
fn zoom_to_selection_toggles_between_the_selection_and_the_whole_song() {
    let (mut v, doc, reg) = setup();
    select(&mut v, &[5]);
    v.command("sequencer.zoom_selection", None, &doc, &reg);
    let (x0, x1) = (v.tick_to_x(4 * BAR), v.tick_to_x(6 * BAR));
    let right = ARR_BOUNDS.x1;
    assert!(x0 >= 208.0 - 1e-6 && x1 <= right + 1e-6);
    assert!(
        x1 - x0 > 0.8 * (right - 208.0),
        "the selection fills the view"
    );
    v.command("sequencer.zoom_selection", None, &doc, &reg);
    let whole = (v.tick_to_x(0), v.tick_to_x(6 * BAR));
    assert!(
        whole.0 >= 208.0 - 1e-6 && whole.1 <= right + 1e-6,
        "the whole song fits"
    );
    assert!(v.px_per_tick() < (x1 - x0) / (2.0 * BAR as f64));
    // Without a selection the key fits the whole song.
    select(&mut v, &[]);
    v.command("sequencer.zoom_selection", None, &doc, &reg);
    assert!(v.tick_to_x(6 * BAR) <= right + 1e-6);
}

#[test]
fn follow_pages_the_view_to_the_playhead_while_playing_until_a_manual_scroll() {
    let (mut v, doc, reg) = setup();
    assert!(!v.follow());
    v.command("sequencer.follow", None, &doc, &reg);
    assert!(v.follow());
    v.set_playing(true);
    v.set_playhead(40 * BAR);
    let x = v.tick_to_x(40 * BAR);
    assert!(
        x > 208.0 && x < ARR_BOUNDS.x1,
        "the playhead is in view: {x}"
    );
    // A manual scroll while playing pauses follow.
    let _ = v.handle(
        &UiEvent::Wheel {
            delta: atomik_ui::kurbo::Vec2::new(0.0, -1.0),
            pos: Point::new(600.0, 100.0),
            modifiers: Modifiers::SHIFT,
        },
        &doc,
        &reg,
    );
    let scroll = v.scroll_ticks();
    v.set_playhead(400 * BAR);
    assert_eq!(v.scroll_ticks(), scroll, "paused");
    // The key resumes it.
    v.command("sequencer.follow", None, &doc, &reg);
    v.set_playhead(500 * BAR);
    assert!(v.tick_to_x(500 * BAR) < ARR_BOUNDS.x1);
    // Stopped: the view stays.
    v.set_playing(false);
    let s = v.scroll_ticks();
    v.set_playhead(900 * BAR);
    assert_eq!(v.scroll_ticks(), s);
}

#[test]
fn alt_with_ctrl_disables_snap_in_a_drag_but_alt_alone_is_the_alternate_tool() {
    let (mut v, doc, reg) = setup();
    let r = v.clip_rect(ClipId(1)).unwrap();
    let p0 = Point::new(r.x0 + 40.0, r.center().y + 6.0);
    let beat_px = PPQ as f64 * v.px_per_tick();
    let drag = |v: &mut ArrangementView, m: Modifiers| {
        v.set_selected_clips(&[]);
        run(
            |e| v.handle(e, &doc, &reg),
            &[
                down(p0),
                mv_with(p0 + atomik_ui::kurbo::Vec2::new(beat_px * 0.37, 0.0), m),
                up(p0),
            ],
        )
    };
    let snapped = drag(&mut v, Modifiers::NONE);
    let starts = |a: &[UiAction]| -> Vec<i64> {
        song_commands(a)
            .into_iter()
            .filter_map(|c| match c {
                SongCommand::SetClipGeometry { start, .. } => Some(start),
                _ => None,
            })
            .collect()
    };
    assert!(starts(&snapped).iter().all(|s| s % (PPQ / 4) == 0));
    let ctrl_alt = Modifiers {
        ctrl: true,
        alt: true,
        ..Modifiers::NONE
    };
    let free = drag(&mut v, ctrl_alt);
    assert!(
        starts(&free).iter().any(|s| s % (PPQ / 4) != 0),
        "Ctrl+Alt: no snap"
    );
    // The legacy roles keep Alt alone as snap off.
    v.set_roles(Roles::legacy(Os::Windows));
    let free = drag(&mut v, alt());
    assert!(starts(&free).iter().any(|s| s % (PPQ / 4) != 0));
}

// ---------------------------------------------------------------- tools

#[test]
fn tool_commands_select_the_tool_and_the_toolbar_shows_it() {
    let (mut v, doc, reg) = setup();
    assert_eq!(v.tool(), Tool::Select);
    for (id, tool) in [
        ("tool.pencil", Tool::Pencil),
        ("tool.eraser", Tool::Eraser),
        ("tool.razor", Tool::Razor),
        ("tool.select", Tool::Select),
    ] {
        assert!(v.command(id, None, &doc, &reg).is_taken());
        assert_eq!(v.tool(), tool);
        assert_eq!(v.shown_tool(), tool);
    }
}

#[test]
fn the_alternate_tool_modifier_switches_the_toolbar_without_a_pointer_move() {
    let (mut v, doc, reg) = setup();
    v.set_tool(Tool::Razor);
    v.paint(&mut DisplayList::new(), &doc, &reg);
    let _ = v.handle(&UiEvent::ModifiersChanged { modifiers: alt() }, &doc, &reg);
    assert_eq!(v.shown_tool(), Tool::Select, "Razor's alternate is Select");
    assert!(v.needs_repaint());
    v.paint(&mut DisplayList::new(), &doc, &reg);
    let _ = v.handle(
        &UiEvent::ModifiersChanged {
            modifiers: Modifiers::SHIFT,
        },
        &doc,
        &reg,
    );
    assert_eq!(v.shown_tool(), Tool::Razor);
}

#[test]
fn the_razor_splits_the_clip_under_the_pointer_at_the_snapped_position() {
    let (mut v, _doc, _reg) = setup();
    v.set_tool(Tool::Razor);
    let p = lane_point(&v, "keys", BAR + PPQ + 100);
    let a = click(&mut v, p, Modifiers::NONE);
    assert_eq!(
        song_commands(&a),
        vec![SongCommand::SplitClip {
            id: ClipId(3),
            at: BAR + PPQ
        }]
    );
    assert!(v.selected_clips().is_empty(), "the razor does not select");
    // Snap off: the split lands where the pointer is (to the tick).
    v.set_snap(Snap::Off);
    let a = click(&mut v, p, Modifiers::NONE);
    let at = v.x_to_tick(p.x);
    assert_eq!(
        song_commands(&a),
        vec![SongCommand::SplitClip { id: ClipId(3), at }]
    );
}

#[test]
fn a_razor_click_in_the_ruler_splits_every_track_at_that_position() {
    let (mut v, _doc, _reg) = setup();
    v.set_tool(Tool::Razor);
    let p = Point::new(v.tick_to_x(BAR + PPQ), v.ruler_label_y());
    let a = click(&mut v, p, Modifiers::NONE);
    let ids: Vec<ClipId> = song_commands(&a)
        .into_iter()
        .map(|c| match c {
            SongCommand::SplitClip { id, at } => {
                assert_eq!(at, BAR + PPQ);
                id
            }
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(ids, vec![ClipId(1), ClipId(4), ClipId(2), ClipId(3)]);
    assert!(
        !a.iter().any(|x| matches!(x, UiAction::Transport(_))),
        "no locate with the razor"
    );
}

#[test]
fn the_razor_does_nothing_on_an_empty_lane_and_the_eraser_removes_the_clip() {
    let (mut v, _doc, _reg) = setup();
    v.set_tool(Tool::Razor);
    let empty = lane_point(&v, "lead", 8 * BAR);
    assert!(click(&mut v, empty, Modifiers::NONE).is_empty());
    v.set_tool(Tool::Eraser);
    let on_clip = lane_point(&v, "keys", BAR);
    let a = click(&mut v, on_clip, Modifiers::NONE);
    assert_eq!(
        song_commands(&a),
        vec![SongCommand::RemoveClip { id: ClipId(3) }]
    );
    assert!(click(&mut v, empty, Modifiers::NONE).is_empty());
}

#[test]
fn the_pencil_makes_a_click_draw_and_leaves_clips_alone() {
    let (mut v, _doc, _reg) = setup();
    v.set_tool(Tool::Pencil);
    let on_clip = lane_point(&v, "keys", BAR);
    assert!(click(&mut v, on_clip, Modifiers::NONE).is_empty());
    assert!(v.selected_clips().is_empty(), "the pencil does not select");
    let empty = lane_point(&v, "lead", 8 * BAR);
    let a = click(&mut v, empty, Modifiers::NONE);
    let added = song_commands(&a);
    assert_eq!(added.len(), 1, "a click draws a one-bar clip");
    match &added[0] {
        SongCommand::AddClip { device, clip, .. } => {
            assert_eq!(device, &key("lead"));
            assert_eq!(clip.length, BAR);
            assert_eq!(clip.start % (PPQ / 4), 0, "snapped");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn select_keeps_the_modeless_gestures_and_the_alternate_role_makes_it_the_pencil() {
    let (mut v, _doc, _reg) = setup();
    // Select: a click on a clip selects it, a click on an empty lane draws nothing.
    let on_clip = lane_point(&v, "keys", BAR);
    assert!(click(&mut v, on_clip, Modifiers::NONE).is_empty());
    assert_eq!(v.selected_clips(), vec![ClipId(3)]);
    let empty = lane_point(&v, "lead", 8 * BAR);
    assert!(click(&mut v, empty, Modifiers::NONE).is_empty());
    // Alt held: the alternate tool is the pencil, so a click on the empty lane draws.
    let a = click(&mut v, empty, alt());
    assert!(matches!(
        song_commands(&a).as_slice(),
        [SongCommand::AddClip { .. }]
    ));
    // ... and the held Alt leaves the stored tool alone.
    assert_eq!(v.tool(), Tool::Select);
    // With the razor, Alt gives Select for one gesture: the click selects, it does not split.
    v.set_tool(Tool::Razor);
    let a = click(&mut v, on_clip, alt());
    assert!(song_commands(&a).is_empty());
    assert_eq!(v.selected_clips(), vec![ClipId(3)]);
}

#[test]
fn a_double_click_on_an_empty_lane_creates_a_clip() {
    let (mut v, doc, reg) = setup();
    let empty = lane_point(&v, "lead", 8 * BAR);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down_mods(empty, Modifiers::NONE, 1),
            up_mods(empty, Modifiers::NONE),
            down_mods(empty, Modifiers::NONE, 2),
            up_mods(empty, Modifiers::NONE),
        ],
    );
    assert!(matches!(
        song_commands(&a).as_slice(),
        [SongCommand::AddClip { .. }]
    ));
}

#[test]
fn the_toolbar_buttons_select_tools_by_click() {
    let (mut v, doc, reg) = setup();
    for tool in [Tool::Razor, Tool::Eraser, Tool::Pencil, Tool::Select] {
        let p = v.tool_button_center(tool);
        let a = run(|e| v.handle(e, &doc, &reg), &[down(p), up(p)]);
        assert!(a.is_empty());
        assert_eq!(v.tool(), tool);
    }
}

// ---------------------------------------------------------------- what the view keeps

/// After the migration the view's own key handler keeps only Escape and Home: every key that
/// is a registry command does nothing here (a remap or a context decides), whatever the
/// modifiers, so an unbound chord cannot act by the back door.
#[test]
fn the_view_key_handler_keeps_escape_and_home_and_drops_every_command_key() {
    use atomik_ui::Key;
    let (mut v, doc, reg) = setup();
    select(&mut v, &[1, 3]);
    let x0 = v.tick_to_x(0);
    let dropped: &[(Key, Modifiers)] = &[
        (Key::Space, Modifiers::NONE),
        (Key::Enter, Modifiers::NONE),
        (Key::Delete, Modifiers::NONE),
        (Key::Backspace, Modifiers::NONE),
        (Key::Char('d'), Modifiers::CTRL),
        (Key::Char('a'), Modifiers::CTRL),
        (Key::Char('z'), Modifiers::CTRL),
        (Key::Char('e'), Modifiers::CTRL),
        (Key::Char('l'), Modifiers::NONE),
        (Key::Char('r'), Modifiers::NONE),
        (Key::Left, Modifiers::NONE),
        (Key::Right, Modifiers::NONE),
    ];
    for (k, m) in dropped {
        let out = v.handle(&press(k.clone(), *m), &doc, &reg);
        assert!(out.is_empty(), "{k:?} {m:?} acted in the view: {out:?}");
        assert_eq!(v.selected_clips().len(), 2, "{k:?} changed the selection");
    }
    assert_eq!(v.tool(), Tool::Select, "R is not a tool key in the view");
    // Home scrolls to the start.
    v.handle(
        &UiEvent::Wheel {
            delta: atomik_ui::kurbo::Vec2::new(-5.0, 0.0),
            pos: Point::new(300.0, 200.0),
            modifiers: Modifiers::NONE,
        },
        &doc,
        &reg,
    );
    assert!((v.tick_to_x(0) - x0).abs() > 1.0, "the wheel scrolled");
    v.handle(&press(Key::Home, Modifiers::NONE), &doc, &reg);
    assert_eq!(v.tick_to_x(0), x0, "Home goes back to the start");
    // Escape clears the selection.
    v.handle(&press(Key::Escape, Modifiers::NONE), &doc, &reg);
    assert!(v.selected_clips().is_empty());
}

// ------------------------------------------------------------------ mute

fn mute_clip_in_doc(doc: &mut atomik_model::RackDoc, id: u64) {
    for t in &mut doc.song.tracks {
        for l in &mut t.lanes {
            for c in &mut l.clips {
                if c.id == ClipId(id) {
                    c.muted = true;
                }
            }
        }
    }
}

fn mute_cmd(ids: &[u64], muted: bool) -> UiAction {
    UiAction::Command(Command::Workflow(WorkflowCommand::SetClipsMuted {
        ids: ids.iter().map(|i| ClipId(*i)).collect(),
        muted,
    }))
}

#[test]
fn toggle_mute_mutes_the_selection_across_clip_kinds() {
    let (mut v, doc, reg) = setup();
    select(&mut v, &[1, 4]); // a note clip and an automation clip
    let s = v.command("sequencer.toggle_mute", None, &doc, &reg);
    assert_eq!(s.actions(), [mute_cmd(&[1, 4], true)]);
}

#[test]
fn toggle_mute_unmutes_only_when_every_selected_clip_is_muted() {
    let (mut v, mut doc, reg) = setup();
    mute_clip_in_doc(&mut doc, 1);
    v.sync(&doc, &reg);
    select(&mut v, &[1, 4]);
    let s = v.command("sequencer.toggle_mute", None, &doc, &reg);
    assert_eq!(
        s.actions(),
        [mute_cmd(&[1, 4], true)],
        "mixed selection mutes all"
    );
    mute_clip_in_doc(&mut doc, 4);
    v.sync(&doc, &reg);
    let s = v.command("sequencer.toggle_mute", None, &doc, &reg);
    assert_eq!(s.actions(), [mute_cmd(&[1, 4], false)]);
}

#[test]
fn toggle_mute_with_nothing_selected_is_a_quiet_no_op() {
    let (mut v, doc, reg) = setup();
    select(&mut v, &[]);
    let s = v.command("sequencer.toggle_mute", None, &doc, &reg);
    assert_eq!(s, CommandOutcome::Handled(vec![]));
}

#[test]
fn the_header_button_toggles_only_its_clip_and_keeps_the_selection() {
    let (mut v, _doc, _reg) = setup();
    select(&mut v, &[1, 3]);
    let p = v
        .mute_button_center(ClipId(2))
        .expect("a wide clip has one");
    let out = click(&mut v, p, Modifiers::NONE);
    assert_eq!(out, [mute_cmd(&[2], true)]);
    assert_eq!(v.selected_clips(), vec![ClipId(1), ClipId(3)]);
    assert!(!v.is_dragging());
}

#[test]
fn the_header_button_of_a_muted_clip_unmutes_it_and_wins_over_the_razor() {
    let (mut v, mut doc, reg) = setup();
    mute_clip_in_doc(&mut doc, 2);
    v.sync(&doc, &reg);
    v.set_tool(Tool::Razor);
    let p = v.mute_button_center(ClipId(2)).unwrap();
    let out = {
        let (d, r) = (doc.clone(), reg);
        run(
            |e| v.handle(e, &d, &r),
            &[
                down_mods(p, Modifiers::NONE, 1),
                up_mods(p, Modifiers::NONE),
            ],
        )
    };
    assert_eq!(out, [mute_cmd(&[2], false)]);
}

#[test]
fn a_clip_narrower_than_three_buttons_has_no_header_button() {
    let (mut v, mut doc, reg) = setup();
    for t in &mut doc.song.tracks {
        for l in &mut t.lanes {
            for c in &mut l.clips {
                if c.id == ClipId(5) {
                    c.length = PPQ / 2;
                    c.loop_length = None;
                }
            }
        }
    }
    v.sync(&doc, &reg);
    assert!(v.mute_button_center(ClipId(5)).is_none());
    assert!(v.mute_button_center(ClipId(1)).is_some());
}
