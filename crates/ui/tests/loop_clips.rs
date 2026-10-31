//! Looped clips and clip names and colours in the arrangement: the loop handle in the upper right
//! corner of every clip kind, the seams between repetitions, the loop and colour commands, and
//! the name typed over a clip's label.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{
    AudioClipContent, ClipContent, ClipId, Command, Document, LoopConvert, MetaChange, SongCommand,
    WorkflowCommand,
};
use atomik_ui::keymap::CommandOutcome;
use atomik_ui::kurbo::{Point, Vec2};
use atomik_ui::timeline::Snap;
use atomik_ui::{ArrangementView, DisplayList, Key, Modifiers, UiAction, UiEvent};
use std::path::PathBuf;
use support::audio::*;
use support::song::*;
use support::*;

/// Height of a clip's header strip, and the handle's offset from the clip's top-right corner.
const HEADER: f64 = 12.0;

fn setup() -> (Document, Registry, ArrangementView) {
    let reg = registry();
    let d = Document::new(song_doc());
    let mut v = arrangement(d.doc(), &reg);
    v.set_snap(Snap::Bar);
    (d, reg, v)
}

/// The middle of the loop handle of clip `id`.
fn handle(v: &ArrangementView, id: u64) -> Point {
    let r = v.clip_rect(ClipId(id)).unwrap();
    Point::new(r.x1 - 8.0, r.y0 + HEADER + 2.0 + 5.0)
}

fn bar_px(v: &ArrangementView) -> f64 {
    v.tick_to_x(BAR) - v.tick_to_x(0)
}

fn drive(
    d: &Document,
    reg: &Registry,
    v: &mut ArrangementView,
    events: &[UiEvent],
) -> Vec<UiAction> {
    events
        .iter()
        .flat_map(|e| v.handle(e, d.doc(), reg))
        .collect()
}

/// `(id, start, length, offset, loop)` of every `SetClipGeometry` among `actions`.
fn geometry(actions: &[UiAction]) -> Vec<(ClipId, i64, i64, i64, Option<i64>)> {
    song_commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            SongCommand::SetClipGeometry {
                id,
                start,
                length,
                offset,
                loop_length,
                ..
            } => Some((id, start, length, offset, loop_length)),
            _ => None,
        })
        .collect()
}

fn do_command(
    d: &Document,
    reg: &Registry,
    v: &mut ArrangementView,
    id: &str,
    arg: Option<&str>,
) -> CommandOutcome {
    v.command(id, arg, d.doc(), reg)
}

fn handled(outcome: CommandOutcome) -> Vec<UiAction> {
    match outcome {
        CommandOutcome::Handled(a) => a,
        other => panic!("not handled: {other:?}"),
    }
}

// --------------------------------------------------------------------------- the handle

#[test]
fn dragging_the_handle_of_a_plain_clip_repeats_what_it_shows() {
    let (d, reg, mut v) = setup();
    let bp = bar_px(&v);
    // Clip 1 is four bars from tick 0, no loop.
    let p = handle(&v, 1);
    let a = drive(
        &d,
        &reg,
        &mut v,
        &[down(p), mv(p + Vec2::new(bp * 1.2, 0.0))],
    );
    let g = geometry(&a);
    // The clip grows to bar 5 and loops over the four bars it showed.
    assert_eq!(g.len(), 1);
    assert_eq!(
        (g[0].0, g[0].1, g[0].2, g[0].4),
        (ClipId(1), 0, 5 * BAR, Some(4 * BAR))
    );
    let a = drive(&d, &reg, &mut v, &[up(p)]);
    assert_eq!(a.last(), Some(&UiAction::EndMerge));
    // Dragging back below the length shortens it and leaves no loop.
    let a = drive(
        &d,
        &reg,
        &mut v,
        &[down(p), mv(p - Vec2::new(bp * 2.0, 0.0))],
    );
    let g = geometry(&a);
    assert_eq!((g[0].2, g[0].4), (2 * BAR, None));
    let _ = drive(&d, &reg, &mut v, &[up(p)]);
}

#[test]
fn the_handle_of_a_looped_clip_keeps_its_loop_and_everything_is_one_merge_group() {
    let (d, reg, mut v) = setup();
    let bp = bar_px(&v);
    // Clip 5 is two bars from bar 4 and loops every bar.
    let p = handle(&v, 5);
    let a = drive(
        &d,
        &reg,
        &mut v,
        &[
            down(p),
            mv(p + Vec2::new(bp * 2.2, 0.0)),
            mv(p + Vec2::new(bp * 3.2, 0.0)),
            up(p),
        ],
    );
    let g = geometry(&a);
    assert!(
        g.len() >= 2,
        "the pointer moved twice (and the release restates)"
    );
    assert!(g.iter().all(|c| c.4 == Some(BAR)), "the loop stays one bar");
    assert_eq!(g[0].2, 4 * BAR);
    assert_eq!(a.last(), Some(&UiAction::EndMerge));
    // The merge group is shared by every command of the drag.
    let merges: Vec<_> = song_commands(&a)
        .into_iter()
        .filter_map(|c| match c {
            SongCommand::SetClipGeometry { merge, .. } => merge,
            _ => None,
        })
        .collect();
    assert!(merges.windows(2).all(|w| w[0] == w[1]));
}

#[test]
fn an_audio_clip_gets_a_loop_through_the_handle_but_a_reversed_one_has_none() {
    let reg = registry();
    let doc = audio_doc();
    let d = Document::new(doc);
    let mut v = audio_arrangement(d.doc(), &reg);
    v.set_snap(Snap::Bar);
    let bp = bar_px(&v);
    // Clip 10 is two bars from bar 1.
    let p = handle(&v, 10);
    let a = drive(
        &d,
        &reg,
        &mut v,
        &[down(p), mv(p + Vec2::new(bp * 2.2, 0.0)), up(p)],
    );
    let g = geometry(&a);
    assert_eq!(
        (g[0].0, g[0].1, g[0].2, g[0].3, g[0].4),
        (ClipId(10), BAR, 4 * BAR, 0, Some(2 * BAR))
    );
    // Reversed: no handle, so the same press moves the clip and never sets a loop.
    let mut doc = audio_doc();
    for t in &mut doc.song_mut().tracks {
        for l in &mut t.lanes {
            for c in &mut l.clips {
                if let ClipContent::Audio(a) = &mut c.content {
                    *a = AudioClipContent {
                        reverse: true,
                        ..a.clone()
                    };
                }
            }
        }
    }
    let d = Document::new(doc);
    let mut v = audio_arrangement(d.doc(), &reg);
    v.set_snap(Snap::Bar);
    let p = handle(&v, 10);
    let a = drive(
        &d,
        &reg,
        &mut v,
        &[down(p), mv(p + Vec2::new(bp * 2.2, 0.0)), up(p)],
    );
    assert!(
        song_commands(&a).iter().all(|c| !matches!(
            c,
            SongCommand::SetClipGeometry {
                loop_length: Some(_),
                ..
            }
        )),
        "a reversed clip never loops"
    );
}

#[test]
fn the_handle_is_painted_for_looped_selected_and_hovered_clips_only() {
    let (d, reg, mut v) = setup();
    let paint = |v: &mut ArrangementView| {
        let mut dl = DisplayList::new();
        v.paint(&mut dl, d.doc(), &reg);
        dl.to_json()
    };
    let plain = paint(&mut v);
    // Selecting a plain clip draws its handle.
    v.set_selected_clips(&[ClipId(3)]);
    let selected = paint(&mut v);
    assert_ne!(selected, plain);
    // So does the pointer over it (clip 3 is not selected any more).
    v.set_selected_clips(&[]);
    let r = v.clip_rect(ClipId(3)).unwrap();
    let _ = v.handle(&mv(Point::new(r.center().x, r.center().y)), d.doc(), &reg);
    let hovered = paint(&mut v);
    assert_ne!(hovered, plain);
    // Moving away hides it again.
    let _ = v.handle(&mv(Point::new(2.0, 2.0)), d.doc(), &reg);
    assert_eq!(paint(&mut v), plain);
}

// ------------------------------------------------------------------------------ commands

#[test]
fn convert_loop_and_loop_instance_commands_work_on_the_selection() {
    let (d, reg, mut v) = setup();
    // Nothing selected, or no looped clip: nothing to do.
    assert!(matches!(
        do_command(&d, &reg, &mut v, "sequencer.convert_loop", Some("single")),
        CommandOutcome::NotApplicable(_)
    ));
    v.set_selected_clips(&[ClipId(1)]);
    assert!(matches!(
        do_command(&d, &reg, &mut v, "sequencer.convert_loop", Some("separate")),
        CommandOutcome::NotApplicable(_)
    ));
    // Clip 5 loops.
    v.set_selected_clips(&[ClipId(5)]);
    for (arg, mode) in [
        ("single", LoopConvert::Single),
        ("separate", LoopConvert::Separate),
    ] {
        let a = handled(do_command(
            &d,
            &reg,
            &mut v,
            "sequencer.convert_loop",
            Some(arg),
        ));
        assert_eq!(
            workflow_commands(&a),
            [WorkflowCommand::ConvertLoop {
                ids: vec![ClipId(5)],
                mode
            }]
        );
    }
    assert!(matches!(
        do_command(&d, &reg, &mut v, "sequencer.convert_loop", Some("both")),
        CommandOutcome::Declined
    ));
    // The loop instance is the repetition under the playhead.
    v.set_playhead(4 * BAR + BAR / 2);
    let a = handled(do_command(
        &d,
        &reg,
        &mut v,
        "sequencer.loop_instance",
        None,
    ));
    assert_eq!(
        workflow_commands(&a),
        [WorkflowCommand::DuplicateLoopInstance {
            id: ClipId(5),
            index: 0
        }]
    );
    v.set_playhead(5 * BAR + 1);
    let a = handled(do_command(
        &d,
        &reg,
        &mut v,
        "sequencer.loop_instance",
        None,
    ));
    assert!(matches!(
        workflow_commands(&a)[..],
        [WorkflowCommand::DuplicateLoopInstance { index: 1, .. }]
    ));
    // Outside the clip there is no repetition.
    v.set_playhead(0);
    assert!(matches!(
        do_command(&d, &reg, &mut v, "sequencer.loop_instance", None),
        CommandOutcome::NotApplicable(_)
    ));
}

#[test]
fn clip_colour_sets_the_palette_colour_or_goes_back_to_the_track() {
    let (d, reg, mut v) = setup();
    v.set_selected_clips(&[ClipId(1), ClipId(3)]);
    let a = handled(do_command(
        &d,
        &reg,
        &mut v,
        "sequencer.clip_color",
        Some("green"),
    ));
    assert_eq!(
        song_commands(&a),
        [SongCommand::SetClipMeta {
            ids: vec![ClipId(1), ClipId(3)],
            name: MetaChange::Keep,
            color: MetaChange::Set([0x5a, 0xb8, 0x6a]),
        }]
    );
    let a = handled(do_command(
        &d,
        &reg,
        &mut v,
        "sequencer.clip_color",
        Some("track"),
    ));
    assert!(matches!(
        song_commands(&a)[..],
        [SongCommand::SetClipMeta {
            color: MetaChange::Clear,
            ..
        }]
    ));
    assert!(matches!(
        do_command(&d, &reg, &mut v, "sequencer.clip_color", Some("plaid")),
        CommandOutcome::Declined
    ));
}

#[test]
fn a_double_click_on_the_label_types_the_clip_name_and_enter_commits() {
    let (d, reg, mut v) = setup();
    let r = v.clip_rect(ClipId(1)).unwrap();
    let label = Point::new(r.x0 + 30.0, r.y0 + HEADER / 2.0);
    let _ = drive(
        &d,
        &reg,
        &mut v,
        &[down_mods(label, Modifiers::NONE, 2), up(label)],
    );
    assert!(v.is_text_owner(), "the name is typed");
    assert_eq!(v.org_edit_text(), Some(""));
    for ch in "Verse 1".chars() {
        let key = if ch == ' ' { Key::Space } else { Key::Char(ch) };
        let _ = v.handle(&press(key, Modifiers::NONE), d.doc(), &reg);
    }
    assert_eq!(v.org_edit_text(), Some("Verse 1"));
    let a = v.handle(&press(Key::Enter, Modifiers::NONE), d.doc(), &reg);
    assert_eq!(
        song_commands(&a),
        [SongCommand::SetClipMeta {
            ids: vec![ClipId(1)],
            name: MetaChange::Set("Verse 1".into()),
            color: MetaChange::Keep,
        }]
    );
    assert!(!v.is_text_owner());
    // Escape leaves the name alone, and so does an unchanged text.
    let _ = do_command(&d, &reg, &mut v, "sequencer.rename_clip", None);
    assert!(v.is_text_owner());
    let a = v.handle(&press(Key::Escape, Modifiers::NONE), d.doc(), &reg);
    assert!(song_commands(&a).is_empty());
    // Rename needs exactly one clip.
    v.set_selected_clips(&[ClipId(1), ClipId(3)]);
    assert!(matches!(
        do_command(&d, &reg, &mut v, "sequencer.rename_clip", None),
        CommandOutcome::NotApplicable(_)
    ));
}

#[test]
fn a_named_and_coloured_clip_shows_both() {
    let reg = registry();
    let mut doc = song_doc();
    {
        let song = doc.song_mut();
        let clip = &mut song.tracks[0].lanes[0].clips[0];
        clip.name = Some("Intro riff".into());
        clip.color = Some([0xe0, 0x5a, 0x4f]);
    }
    let named = {
        let v = arrangement(&doc, &reg);
        let mut dl = DisplayList::new();
        let mut v = v;
        v.paint(&mut dl, &doc, &reg);
        dl.to_json()
    };
    assert!(named.contains("Intro riff"));
    let plain = {
        let doc = song_doc();
        let mut v = arrangement(&doc, &reg);
        let mut dl = DisplayList::new();
        v.paint(&mut dl, &doc, &reg);
        dl.to_json()
    };
    assert!(!plain.contains("Intro riff"));
    assert_ne!(named, plain);
}

// ------------------------------------------------------------------------------- golden

fn check(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os("ATOMIK_UPDATE_GOLDEN").is_some_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| {
            panic!(
                "missing golden {}; run with ATOMIK_UPDATE_GOLDEN=1",
                path.display()
            )
        })
        .replace("\r\n", "\n");
    assert!(
        expected == actual,
        "{name} differs from its golden (regenerate with ATOMIK_UPDATE_GOLDEN=1)"
    );
}

#[test]
fn looped_clips_with_their_handle_seams_names_and_colours_match_the_golden() {
    let reg = registry();
    let mut doc = song_doc();
    {
        let song = doc.song_mut();
        // The looped lead clip: named, in a colour of its own.
        let lead = &mut song.tracks[1].lanes[0].clips[0];
        lead.name = Some("Lead loop".into());
        lead.color = Some([0x9a, 0x6a, 0xd0]);
    }
    let mut v = arrangement(&doc, &reg);
    v.set_selected_clips(&[ClipId(5)]);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    check("arrangement_loops_1x.json", &dl.to_json());
}

/// Keeps the model's command type in the imports for the helpers above.
#[test]
fn the_set_clip_meta_command_is_a_song_command() {
    let c = Command::Song(SongCommand::SetClipMeta {
        ids: vec![ClipId(1)],
        name: MetaChange::Clear,
        color: MetaChange::Keep,
    });
    assert!(matches!(c, Command::Song(SongCommand::SetClipMeta { .. })));
}
