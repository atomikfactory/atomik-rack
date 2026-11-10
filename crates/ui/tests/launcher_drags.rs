//! Drags between the arrangement and the launcher: a clip dragged out of the arrangement reports
//! where it was dropped, a slot dragged out of the launcher does the same, files dropped on a slot
//! name it, and the audio columns of the launcher.

mod support;

use atomik_model::{ClipId, Lane, LaneKind, RackDoc};
use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{
    LauncherAction, LauncherView, Modifiers, Theme, UiAction, UiEvent, keymap::CommandOutcome,
};
use support::song::*;
use support::*;

#[test]
fn a_clip_dragged_out_of_the_arrangement_reports_the_drop_and_one_dropped_inside_does_not() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    // The clip with id 2 sits on the lead lane; grab it by its middle.
    let r = v.clip_rect(ClipId(2)).expect("the clip shows");
    let grab = Point::new(r.center().x, r.center().y + 6.0);
    // Drop above the view (where a launcher would be).
    let outside = Point::new(grab.x, -80.0);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down(grab),
            mv(Point::new(grab.x + 30.0, grab.y)),
            mv(outside),
            up(outside),
        ],
    );
    let dragged: Vec<(Vec<ClipId>, Point)> = a
        .iter()
        .filter_map(|a| match a {
            UiAction::ClipsDragged { ids, pos } => Some((ids.clone(), *pos)),
            _ => None,
        })
        .collect();
    assert_eq!(dragged.len(), 1, "{a:?}");
    assert_eq!(dragged[0].1, outside);
    assert!(dragged[0].0.contains(&ClipId(2)));
    // A move that ends inside the view is an ordinary move and reports nothing dragged out.
    let mut v = arrangement(&doc, &reg);
    let inside = Point::new(grab.x + 120.0, grab.y);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down(grab),
            mv(Point::new(grab.x + 30.0, grab.y)),
            mv(inside),
            up(inside),
        ],
    );
    assert!(!a.iter().any(|a| matches!(a, UiAction::ClipsDragged { .. })));
}

fn launcher_doc() -> RackDoc {
    let mut doc = song_doc();
    // An audio track beside the note tracks.
    let mut audio = track(
        "delay",
        vec![Lane {
            kind: LaneKind::Audio,
            name: None,
            muted: false,
            solo: false,
            color: None,
            static_value: None,
            clips: Vec::new(),
            take_groups: Vec::new(),
        }],
    );
    audio.chase = false;
    doc.song_mut().tracks.push(audio);
    doc
}

fn view(doc: &RackDoc) -> LauncherView {
    let mut v = LauncherView::new(Theme::dark());
    v.set_bounds(Rect::new(0.0, 0.0, 760.0, 420.0));
    v.sync(doc);
    v
}

#[test]
fn a_slot_dragged_out_of_the_view_reports_the_drop() {
    let mut doc = launcher_doc();
    {
        let l = &mut doc.song_mut().launcher;
        l.scenes = vec![Default::default()];
        let c = notes_clip(101, 0, BAR, vec![n(0, PPQ, 60, 0.8)]);
        l.slots.insert(
            key("bass"),
            vec![Some(atomik_model::LauncherClip {
                clip: c,
                quantize: None,
            })],
        );
        doc.song_mut().next_clip_id = 200;
    }
    let mut v = view(&doc);
    let from = {
        let r = v.slot_rect(0, 0).unwrap();
        Point::new(r.x1 - 8.0, r.center().y)
    };
    let outside = Point::new(from.x, 900.0);
    let a = run(
        |e| v.handle(e),
        &[
            down(from),
            mv(Point::new(from.x, from.y + 30.0)),
            mv(outside),
            up(outside),
        ],
    );
    assert!(
        a.iter().any(|a| matches!(
            a,
            UiAction::Launcher(LauncherAction::DragOut { device, row: 0, pos })
                if *device == key("bass") && *pos == outside
        )),
        "{a:?}"
    );
}

#[test]
fn files_dropped_on_a_slot_name_it_and_hovering_marks_it() {
    let doc = launcher_doc();
    let mut v = view(&doc);
    let audio_col = v.tracks().iter().position(|d| *d == key("delay")).unwrap();
    let r = v.slot_rect(audio_col, 1).expect("an audio slot shows");
    let at = r.center();
    v.handle(&UiEvent::FileHover {
        pos: at,
        paths: vec!["tone.wav".into()],
    });
    assert!(v.needs_repaint());
    let (device, row, audio) = v.slot_at(at).unwrap();
    assert_eq!((device, row, audio), (key("delay"), 1, true));
    let a = v.handle(&UiEvent::FileDrop {
        pos: at,
        paths: vec!["tone.wav".into(), "other.wav".into()],
    });
    assert!(a.iter().any(|a| matches!(
        a,
        UiAction::Launcher(LauncherAction::DropFile { device, row: 1, path })
            if *device == key("delay") && path.ends_with("tone.wav")
    )));
    // A drop away from the slots names nothing.
    let a = v.handle(&UiEvent::FileDrop {
        pos: Point::new(2_000.0, 2_000.0),
        paths: vec!["tone.wav".into()],
    });
    assert!(a.is_empty());
}

#[test]
fn an_audio_column_has_no_new_clip_and_a_note_column_still_makes_one() {
    let doc = launcher_doc();
    let mut v = view(&doc);
    let audio_col = v.tracks().iter().position(|d| *d == key("delay")).unwrap();
    let at = |v: &LauncherView, col: usize| {
        let r = v.slot_rect(col, 4).unwrap();
        Point::new(r.x1 - 8.0, r.center().y)
    };
    let a = v.handle(&down_mods(at(&v, audio_col), Modifiers::NONE, 2));
    assert!(a.is_empty(), "an audio slot needs a file: {a:?}");
    let a = v.handle(&down_mods(at(&v, 0), Modifiers::NONE, 2));
    assert!(matches!(
        a.first(),
        Some(UiAction::Launcher(LauncherAction::NewClip { row: 4, .. }))
    ));
    // The Edit commands on an empty audio slot are refused like on any empty slot.
    v.set_cursor(audio_col, 4);
    assert!(matches!(
        v.command("edit.delete", None),
        CommandOutcome::NotApplicable(_)
    ));
}
