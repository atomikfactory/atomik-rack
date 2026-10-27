//! Audio editing in the arrangement: the gain envelope (pencil, drag, remove), reverse trims,
//! crossfade zones, the audio clip commands and their menu state; the envelope and trim maths;
//! and the strip silence panel.

mod support;

use atomik_model::{
    AudioClipContent, ClipContent, ClipId, Command, FadeCurve, GainPoint, RackDoc, SongCommand,
};
use atomik_ui::audio::{self, TempoMap};
use atomik_ui::keymap::CommandOutcome;
use atomik_ui::kurbo::{Point, Vec2};
use atomik_ui::timeline::Snap;
use atomik_ui::{
    Modifiers, PointerButton, SilenceAction, SilenceRow, SilenceValues, SilenceView, Theme, Tool,
    UiAction, UiEvent,
};
use support::audio::*;
use support::song::*;
use support::*;

fn p(frame: u64, gain_db: f32) -> GainPoint {
    GainPoint { frame, gain_db }
}

/// The content of clip `id` replaced by `f(content)`.
fn edit_clip(doc: &mut RackDoc, id: u64, f: impl Fn(&mut AudioClipContent)) {
    for t in &mut doc.song.tracks {
        for l in &mut t.lanes {
            for c in &mut l.clips {
                if c.id == ClipId(id)
                    && let ClipContent::Audio(a) = &mut c.content
                {
                    f(a);
                }
            }
        }
    }
}

fn audio_cmds(actions: &[UiAction]) -> Vec<SongCommand> {
    song_commands(actions)
        .into_iter()
        .filter(|c| matches!(c, SongCommand::SetAudioClip { .. }))
        .collect()
}

fn last_content(actions: &[UiAction]) -> AudioClipContent {
    match audio_cmds(actions).last() {
        Some(SongCommand::SetAudioClip { content, .. }) => content.clone(),
        other => panic!("no audio clip command: {other:?}"),
    }
}

fn drag_to(
    v: &mut atomik_ui::ArrangementView,
    doc: &RackDoc,
    reg: &atomik_device_sdk::Registry,
    from: Point,
    to: Point,
) -> Vec<UiAction> {
    let mut a = v.handle(&down(from), doc, reg);
    a.extend(v.handle(&mv(Point::new((from.x + to.x) / 2.0, to.y)), doc, reg));
    a.extend(v.handle(&mv(to), doc, reg));
    a.extend(v.handle(&up(to), doc, reg));
    a
}

// ---------------------------------------------------------------- maths

#[test]
fn the_gain_scale_inverts_and_rounds_to_a_tenth() {
    for db in [-60.0f32, -12.0, -3.0, 0.0, 6.5, 12.0] {
        let y = audio::gain_to_y(db, 10.0, 90.0);
        assert!((audio::y_to_gain(y, 10.0, 90.0) - db).abs() < 0.06, "{db}");
    }
    assert_eq!(audio::y_to_gain(-50.0, 10.0, 90.0), audio::GAIN_MAX_DB);
    assert_eq!(audio::y_to_gain(500.0, 10.0, 90.0), audio::GAIN_MIN_DB);
    assert_eq!(audio::y_to_gain(5.0, 10.0, 10.0), 0.0, "an empty scale");
}

#[test]
fn envelope_points_insert_move_and_remove() {
    let list = vec![p(100, -6.0), p(300, 0.0)];
    let (l, i) = audio::envelope_insert(&list, 200, -20.0);
    assert_eq!((i, l.len()), (1, 3));
    assert_eq!(l[1], p(200, -20.0));
    // A point on an existing frame takes the new gain instead of doubling.
    let (l, i) = audio::envelope_insert(&list, 100, 3.0);
    assert_eq!((i, l.len(), l[0]), (0, 2, p(100, 3.0)));
    // Moves stay strictly between the neighbours and inside the gain scale.
    let l = audio::envelope_move(&list, 0, 500, 40.0);
    assert_eq!(l[0], p(299, audio::GAIN_MAX_DB));
    let l = audio::envelope_move(&list, 1, -50, -90.0);
    assert_eq!(l[1], p(101, audio::GAIN_MIN_DB));
    let l = audio::envelope_move(&list, 0, -50, 0.0);
    assert_eq!(l[0].frame, 0);
    assert_eq!(audio::envelope_remove(&list, 0), vec![p(300, 0.0)]);
    assert_eq!(audio::envelope_remove(&list, 9), list);
}

#[test]
fn a_left_trim_keeps_the_envelope_on_the_same_sound() {
    let env = vec![p(0, -20.0), p(1000, 0.0), p(2000, -6.0)];
    // The start moves 500 frames later: every point moves 500 earlier, the value at the new
    // start becomes the first point.
    let cut = audio::envelope_after_trim(&env, 500);
    assert_eq!(cut.len(), 3);
    assert_eq!(cut[1], p(500, 0.0));
    assert_eq!(cut[2], p(1500, -6.0));
    // Earlier: everything moves later and the first value holds in front.
    let longer = audio::envelope_after_trim(&env, -300);
    assert_eq!(longer[0], p(300, -20.0));
    assert_eq!(longer[2], p(2300, -6.0));
    assert!(audio::envelope_after_trim(&[], 40).is_empty());
}

#[test]
fn reversed_clips_trim_at_the_other_ends() {
    let map = TempoMap::default();
    let rate = 48_000;
    // A reversed clip of two beats (48000 frames) from source frame 10000 in an asset of
    // 100000 frames: the left edge cuts the top of the span and keeps the offset.
    let orig = (30_720i64, 30_720, 10_000u64);
    let t = audio::trim_left_reversed(&map, rate, orig, 100_000, 30_720 + 7_680, 240);
    assert_eq!(t.source_offset, 10_000);
    assert_eq!(t.start, 30_720 + 7_680);
    assert_eq!(t.start + t.length, 30_720 + 30_720);
    // It cannot grow past the asset: 90000 frames above the offset at most.
    let t = audio::trim_left_reversed(&map, rate, orig, 100_000, 0, 240);
    let frames = map.frame_at(t.start + t.length, rate) - map.frame_at(t.start, rate);
    assert!((89_990..=90_000).contains(&frames), "{frames}");
    // Never closer than `min_len` to the end.
    let t = audio::trim_left_reversed(&map, rate, orig, 100_000, 10_000_000, 240);
    assert_eq!(t.length, 240);
    // The end edge: cutting 1000 frames moves the offset up by 1000; growing moves it down and
    // stops at frame 0.
    assert_eq!(
        audio::reversed_end_resize(10_000, 48_000, 47_000),
        (11_000, 47_000)
    );
    assert_eq!(
        audio::reversed_end_resize(10_000, 48_000, 49_000),
        (9_000, 49_000)
    );
    assert_eq!(audio::reversed_end_resize(500, 48_000, 60_000), (0, 48_500));
}

// ---------------------------------------------------------------- the envelope on the clip

fn env_doc() -> RackDoc {
    let mut doc = audio_doc();
    edit_clip(&mut doc, 10, |a| {
        a.envelope = vec![p(48_000, -12.0), p(96_000, 0.0)];
    });
    doc
}

#[test]
fn envelope_points_are_drawn_on_the_clip_and_follow_the_frames() {
    let (doc, reg) = (env_doc(), registry());
    let v = audio_arrangement(&doc, &reg);
    let r = v.clip_rect(ClipId(10)).unwrap();
    let a = v.envelope_point_center(ClipId(10), 0).expect("point");
    let b = v.envelope_point_center(ClipId(10), 1).expect("point");
    // 48000 frames after the clip start is 48 px.
    assert!((a.x - (r.x0 + 48.0)).abs() < 1.0, "{a:?} {r:?}");
    assert!((b.x - a.x - 48.0).abs() < 1.0);
    assert!(a.y > b.y, "-12 dB sits below 0 dB");
    assert!(v.envelope_point_center(ClipId(10), 2).is_none());
    assert!(v.envelope_point_center(ClipId(11), 0).is_none());
}

#[test]
fn the_pencil_adds_a_point_and_drags_it() {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    v.set_tool(Tool::Pencil);
    let r = v.clip_rect(ClipId(10)).unwrap();
    let at = Point::new(r.x0 + 40.0, r.center().y);
    let a = v.handle(&down(at), &doc, &reg);
    let cmds = audio_cmds(&a);
    assert_eq!(cmds.len(), 1, "{a:?}");
    let content = last_content(&a);
    assert_eq!(content.envelope.len(), 1);
    // 40 px from the clip start is 40000 frames (no snapping).
    assert!((content.envelope[0].frame as i64 - 40_000).abs() <= 500);
    let to = at + Vec2::new(10.0, -8.0);
    let mut b = v.handle(&mv(Point::new(at.x + 5.0, at.y - 4.0)), &doc, &reg);
    b.extend(v.handle(&mv(to), &doc, &reg));
    b.extend(v.handle(&up(to), &doc, &reg));
    let moved = last_content(&b);
    assert!((moved.envelope[0].frame as i64 - 50_000).abs() <= 500);
    assert!(moved.envelope[0].gain_db > content.envelope[0].gain_db);
    assert_eq!(b.last(), Some(&UiAction::EndMerge));
    // The whole gesture shares one merge id, so it is one undo step.
    let merges: Vec<u64> = audio_cmds(&a)
        .iter()
        .chain(audio_cmds(&b).iter())
        .map(|c| match c {
            SongCommand::SetAudioClip { merge, .. } => merge.unwrap(),
            _ => unreachable!(),
        })
        .collect();
    assert!(merges.windows(2).all(|w| w[0] == w[1]));
}

#[test]
fn a_point_drags_with_the_select_tool_and_stays_between_its_neighbours() {
    let (doc, reg) = (env_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    let a = v.envelope_point_center(ClipId(10), 0).unwrap();
    let b = v.envelope_point_center(ClipId(10), 1).unwrap();
    // Dragging the first point far right stops one frame before the second.
    let acts = drag_to(&mut v, &doc, &reg, a, a + Vec2::new(200.0, 0.0));
    let env = last_content(&acts).envelope;
    assert_eq!(env.len(), 2);
    assert_eq!(env[0].frame, 95_999);
    assert_eq!(env[1], p(96_000, 0.0));
    // Dragging the second point up raises its gain; the clip's other fields are untouched.
    let acts = drag_to(&mut v, &doc, &reg, b, b + Vec2::new(0.0, -10.0));
    let content = last_content(&acts);
    assert!(content.envelope[1].gain_db > 0.0);
    assert!((content.gain_db + 3.0).abs() < 1e-6);
    assert_eq!(content.envelope[0], p(48_000, -12.0));
}

#[test]
fn a_click_on_a_point_without_moving_emits_nothing() {
    let (doc, reg) = (env_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    let a = v.envelope_point_center(ClipId(10), 0).unwrap();
    let mut acts = v.handle(&down(a), &doc, &reg);
    acts.extend(v.handle(&up(a), &doc, &reg));
    assert!(audio_cmds(&acts).is_empty());
}

#[test]
fn a_right_click_or_the_eraser_removes_a_point() {
    let (doc, reg) = (env_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    let a = v.envelope_point_center(ClipId(10), 0).unwrap();
    let acts = v.handle(&down_with(a, PointerButton::Secondary, 1), &doc, &reg);
    assert_eq!(last_content(&acts).envelope, vec![p(96_000, 0.0)]);
    let mut v = audio_arrangement(&doc, &reg);
    v.set_tool(Tool::Eraser);
    let acts = v.handle(&down(a), &doc, &reg);
    assert_eq!(last_content(&acts).envelope, vec![p(96_000, 0.0)]);
}

#[test]
fn escape_during_a_point_drag_restores_the_clip() {
    let (doc, reg) = (env_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    let a = v.envelope_point_center(ClipId(10), 0).unwrap();
    v.handle(&down(a), &doc, &reg);
    v.handle(&mv(a + Vec2::new(5.0, 0.0)), &doc, &reg);
    v.handle(&mv(a + Vec2::new(12.0, -6.0)), &doc, &reg);
    let acts = v.handle(&press(atomik_ui::Key::Escape, Modifiers::NONE), &doc, &reg);
    assert_eq!(
        last_content(&acts).envelope,
        vec![p(48_000, -12.0), p(96_000, 0.0)]
    );
}

// ---------------------------------------------------------------- reverse trims

#[test]
fn trimming_a_reversed_clip_moves_the_offset_with_the_right_edge_only() {
    let mut doc = audio_doc();
    edit_clip(&mut doc, 10, |a| a.reverse = true);
    let reg = registry();
    let mut v = audio_arrangement(&doc, &reg);
    v.set_snap(Snap::Beat);
    let r = v.clip_rect(ClipId(10)).unwrap();
    // The left edge: 24 px (24000 frames) inward keeps the source offset.
    let edge = Point::new(r.x0 + 2.0, r.center().y + 12.0);
    let a = drag_to(&mut v, &doc, &reg, edge, edge + Vec2::new(24.0, 0.0));
    let SongCommand::SetAudioClip {
        start,
        length,
        content,
        ..
    } = audio_cmds(&a).last().unwrap().clone()
    else {
        unreachable!()
    };
    assert_eq!(start, BAR + PPQ);
    assert_eq!(length, 2 * BAR - PPQ);
    assert_eq!(content.source_offset, RATE as u64, "the offset stays");
    assert!(content.reverse);
    // The right edge: one beat (24000 frames) inward raises the offset by that much.
    let right = Point::new(r.x1 - 2.0, r.center().y + 12.0);
    let a = drag_to(&mut v, &doc, &reg, right, right - Vec2::new(24.0, 0.0));
    let SongCommand::SetAudioClip {
        length, content, ..
    } = audio_cmds(&a).last().unwrap().clone()
    else {
        unreachable!()
    };
    assert_eq!(length, 2 * BAR - PPQ);
    assert_eq!(content.source_offset, RATE as u64 + 24_000);
}

#[test]
fn trimming_the_left_edge_shifts_the_envelope_with_the_audio() {
    let (doc, reg) = (env_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    v.set_snap(Snap::Beat);
    let r = v.clip_rect(ClipId(10)).unwrap();
    let edge = Point::new(r.x0 + 2.0, r.center().y + 12.0);
    // 24 px inward: the start moves 24000 frames later.
    let a = drag_to(&mut v, &doc, &reg, edge, edge + Vec2::new(24.0, 0.0));
    let env = last_content(&a).envelope;
    assert_eq!(
        env.first().copied(),
        Some(p(0, atomik_core::gain_to_db(env_at_24000())))
    );
    assert!(env.contains(&p(24_000, -12.0)));
    assert!(env.contains(&p(72_000, 0.0)));
}

/// The amplitude of the test envelope 24000 frames in (before its first point): held.
fn env_at_24000() -> f32 {
    atomik_core::db_to_gain(-12.0)
}

// ---------------------------------------------------------------- crossfade zones

fn overlap_doc(xfade: Option<FadeCurve>) -> RackDoc {
    let mut doc = audio_doc();
    // Clip 11 moves to bar 2 and overlaps the last bar of clip 10 (bars 1..3).
    doc.song.tracks[1].lanes[0].clips[1].start = 2 * BAR;
    edit_clip(&mut doc, 11, |a| a.xfade = xfade);
    doc
}

fn paint_json(v: &mut atomik_ui::ArrangementView, doc: &RackDoc) -> String {
    let mut dl = atomik_ui::DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl.to_json()
}

#[test]
fn a_clip_that_starts_over_an_earlier_one_shows_a_crossfade_zone() {
    let reg = registry();
    let doc = overlap_doc(Some(FadeCurve::EqualPower));
    let mut v = audio_arrangement(&doc, &reg);
    assert_eq!(v.crossfade_zone(ClipId(11)), Some((2 * BAR, 3 * BAR)));
    assert_eq!(v.crossfade_zone(ClipId(10)), None);
    let with = paint_json(&mut v, &doc);
    // Turned off, there is no zone and the picture is different.
    let hard = overlap_doc(None);
    let mut v = audio_arrangement(&hard, &reg);
    assert_eq!(v.crossfade_zone(ClipId(11)), None);
    assert_ne!(paint_json(&mut v, &hard), with);
}

// ---------------------------------------------------------------- the audio commands

fn run(
    v: &mut atomik_ui::ArrangementView,
    doc: &RackDoc,
    reg: &atomik_device_sdk::Registry,
    id: &str,
    arg: Option<&str>,
) -> CommandOutcome {
    v.command(id, arg, doc, reg)
}

fn one_content(out: &CommandOutcome) -> AudioClipContent {
    match out.actions() {
        [UiAction::Command(Command::Song(SongCommand::SetAudioClip { content, merge, .. }))] => {
            assert_eq!(*merge, None);
            content.clone()
        }
        other => panic!("one SetAudioClip expected: {other:?}"),
    }
}

#[test]
fn the_commands_need_a_selected_audio_clip() {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    for (id, arg) in [
        ("sequencer.audio_reverse", None),
        ("sequencer.audio_gain", Some("up")),
        ("sequencer.audio_fade_shape", Some("s_curve")),
        ("sequencer.audio_crossfade", Some("off")),
        ("sequencer.audio_fades", Some("declick")),
        ("sequencer.audio_envelope_clear", None),
    ] {
        assert!(
            matches!(
                run(&mut v, &doc, &reg, id, arg),
                CommandOutcome::NotApplicable(_)
            ),
            "{id}"
        );
    }
    // A note clip is not an audio clip.
    v.set_selected_clips(&[ClipId(1)]);
    assert!(matches!(
        run(&mut v, &doc, &reg, "sequencer.audio_reverse", None),
        CommandOutcome::NotApplicable(_)
    ));
    assert!(!v.audio_menu().can_edit);
    // Normalise and Strip Silence are the app's (they need the samples).
    v.set_selected_clips(&[ClipId(10)]);
    assert_eq!(
        run(&mut v, &doc, &reg, "sequencer.audio_normalise", None),
        CommandOutcome::Declined
    );
}

#[test]
fn reverse_gain_shape_crossfade_fades_and_envelope_edit_the_selected_clip() {
    let (doc, reg) = (env_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    v.set_selected_clips(&[ClipId(10)]);
    let reversed = one_content(&run(&mut v, &doc, &reg, "sequencer.audio_reverse", None));
    assert!(reversed.reverse);
    assert_eq!(reversed.source_offset, u64::from(RATE), "the span stays");
    let up = one_content(&run(&mut v, &doc, &reg, "sequencer.audio_gain", Some("up")));
    assert!((up.gain_db + 2.0).abs() < 1e-6);
    let down = one_content(&run(
        &mut v,
        &doc,
        &reg,
        "sequencer.audio_gain",
        Some("down"),
    ));
    assert!((down.gain_db + 4.0).abs() < 1e-6);
    let reset = one_content(&run(
        &mut v,
        &doc,
        &reg,
        "sequencer.audio_gain",
        Some("reset"),
    ));
    assert_eq!(reset.gain_db, 0.0);
    let shape = one_content(&run(
        &mut v,
        &doc,
        &reg,
        "sequencer.audio_fade_shape",
        Some("exponential"),
    ));
    assert_eq!(shape.fade_curve, FadeCurve::Exponential);
    let x = one_content(&run(
        &mut v,
        &doc,
        &reg,
        "sequencer.audio_crossfade",
        Some("s_curve"),
    ));
    assert_eq!(x.xfade, Some(FadeCurve::SCurve));
    let declick = one_content(&run(
        &mut v,
        &doc,
        &reg,
        "sequencer.audio_fades",
        Some("declick"),
    ));
    assert_eq!(
        (declick.fade_in, declick.fade_out),
        (240, 240),
        "5 ms at 48 kHz"
    );
    let cleared = one_content(&run(
        &mut v,
        &doc,
        &reg,
        "sequencer.audio_fades",
        Some("clear"),
    ));
    assert_eq!((cleared.fade_in, cleared.fade_out), (0, 0));
    let env = one_content(&run(
        &mut v,
        &doc,
        &reg,
        "sequencer.audio_envelope_clear",
        None,
    ));
    assert!(env.envelope.is_empty());
    // A command that changes nothing emits nothing (the test clip crossfades off already).
    let off = run(&mut v, &doc, &reg, "sequencer.audio_crossfade", Some("off"));
    assert!(off.actions().is_empty() && off.is_taken());
}

#[test]
fn several_selected_clips_are_one_batch_and_reverse_toggles_by_the_whole_selection() {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    v.set_selected_clips(&[ClipId(10), ClipId(11)]);
    let out = run(&mut v, &doc, &reg, "sequencer.audio_reverse", None);
    let [UiAction::Command(Command::Batch(cmds))] = out.actions() else {
        panic!("a batch expected: {:?}", out.actions());
    };
    assert_eq!(cmds.len(), 2);
    assert!(cmds.iter().all(|c| matches!(
        c,
        Command::Song(SongCommand::SetAudioClip { content, .. }) if content.reverse
    )));
    // With one of them already reversed, reversing the pair reverses the other one.
    let mut doc = audio_doc();
    edit_clip(&mut doc, 10, |a| a.reverse = true);
    let mut v = audio_arrangement(&doc, &reg);
    v.set_selected_clips(&[ClipId(10), ClipId(11)]);
    assert!(!v.audio_menu().reversed);
    let out = run(&mut v, &doc, &reg, "sequencer.audio_reverse", None);
    assert_eq!(commands(out.actions()).len(), 1, "only clip 11 changes");
    // Both reversed: the item is checked and a second press turns both back.
    edit_clip(&mut doc, 11, |a| a.reverse = true);
    let mut v = audio_arrangement(&doc, &reg);
    v.set_selected_clips(&[ClipId(10), ClipId(11)]);
    assert!(v.audio_menu().reversed);
    let out = run(&mut v, &doc, &reg, "sequencer.audio_reverse", None);
    let [UiAction::Command(Command::Batch(cmds))] = out.actions() else {
        panic!()
    };
    assert!(cmds.iter().all(|c| matches!(
        c,
        Command::Song(SongCommand::SetAudioClip { content, .. }) if !content.reverse
    )));
}

#[test]
fn the_audio_menu_state_follows_the_selection() {
    let (doc, reg) = (env_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    assert_eq!(v.audio_menu(), atomik_ui::arrangement::AudioMenu::default());
    v.set_selected_clips(&[ClipId(11)]);
    let m = v.audio_menu();
    assert!(m.can_edit && !m.reversed && !m.envelope);
    v.set_selected_clips(&[ClipId(10)]);
    assert!(v.audio_menu().envelope);
}

#[test]
fn the_audio_commands_are_in_the_registry() {
    let r = atomik_ui::keymap::Registry::bundled();
    for (id, arg) in [
        ("sequencer.audio_reverse", None),
        ("sequencer.audio_normalise", None),
        ("sequencer.audio_strip_silence", None),
        ("sequencer.audio_envelope_clear", None),
        ("sequencer.audio_gain", Some("up")),
        ("sequencer.audio_gain", Some("reset")),
        ("sequencer.audio_fade_shape", Some("s_curve")),
        ("sequencer.audio_crossfade", Some("off")),
        ("sequencer.audio_crossfade", Some("equal_power")),
        ("sequencer.audio_fades", Some("declick")),
    ] {
        assert!(r.validate(id, arg), "{id} {arg:?}");
        assert!(r.get(id).unwrap().menu, "{id} has a menu item");
    }
    assert!(!r.validate("sequencer.audio_gain", Some("loud")));
    assert!(!r.validate("sequencer.audio_fade_shape", Some("off")));
}

// ---------------------------------------------------------------- the strip silence panel

fn click(v: &mut SilenceView, pos: Point) -> Vec<UiAction> {
    v.handle(&down(pos))
}

#[test]
fn the_panel_steps_its_values_and_clamps_them() {
    let mut v = SilenceView::new(Theme::dark(), SilenceValues::default());
    assert_eq!(v.value_text(SilenceRow::Threshold), "-40 dB");
    assert_eq!(v.value_text(SilenceRow::MinSilence), "100 ms");
    let plus = v.plus_rect(SilenceRow::Threshold).center();
    assert!(click(&mut v, plus).is_empty());
    assert_eq!(v.values().threshold_db, -39);
    let minus = v.minus_rect(SilenceRow::MinSilence).center();
    click(&mut v, minus);
    assert_eq!(v.values().min_silence_ms, 50);
    for _ in 0..30 {
        v.step(SilenceRow::Fade, -1);
    }
    assert_eq!(v.values().fade_ms, 0);
    for _ in 0..200 {
        v.step(SilenceRow::Threshold, 1);
    }
    assert_eq!(v.values().threshold_db, -6);
    for _ in 0..200 {
        v.step(SilenceRow::Threshold, -1);
    }
    assert_eq!(v.values().threshold_db, -96);
    v.step(SilenceRow::PadBefore, 1);
    v.step(SilenceRow::PadAfter, -1);
    assert_eq!((v.values().pad_before_ms, v.values().pad_after_ms), (20, 5));
}

#[test]
fn apply_needs_a_target_and_close_always_works() {
    let mut v = SilenceView::new(Theme::dark(), SilenceValues::default());
    let apply = v.apply_rect().center();
    assert!(click(&mut v, apply).is_empty(), "nothing selected");
    v.set_target("2 audio clips".into(), true);
    assert_eq!(
        click(&mut v, apply),
        vec![UiAction::StripSilence(SilenceAction::Apply)]
    );
    let close = v.close_rect().center();
    assert_eq!(
        click(&mut v, close),
        vec![UiAction::StripSilence(SilenceAction::Close)]
    );
    // The wheel steps the row under it.
    let row = v.row_rect(SilenceRow::Threshold).center();
    v.handle(&UiEvent::Wheel {
        pos: row,
        delta: Vec2::new(0.0, 1.0),
        modifiers: Modifiers::NONE,
    });
    assert_eq!(v.values().threshold_db, -39);
}

#[test]
fn the_panel_paints_and_its_picture_changes_with_the_values() {
    let paint = |v: &SilenceView| {
        let mut dl = atomik_ui::DisplayList::new();
        v.paint(&mut dl);
        dl.to_json()
    };
    let mut v = SilenceView::new(Theme::dark(), SilenceValues::default());
    v.set_target("1 audio clip".into(), true);
    let a = paint(&v);
    assert!(a.contains("STRIP SILENCE") && a.contains("1 audio clip") && a.contains("-40 dB"));
    v.step(SilenceRow::Threshold, 1);
    assert_ne!(paint(&v), a);
}

// ---------------------------------------------------------------- reversed waveform

/// The path fills of a paint that are tall waveform polygons (the clip body, not the chrome).
fn wave_polygons(doc: &RackDoc) -> Vec<atomik_ui::kurbo::BezPath> {
    let reg = registry();
    let mut v = audio_arrangement(doc, &reg);
    let mut dl = atomik_ui::DisplayList::new();
    v.paint(&mut dl, doc, &reg);
    dl.ops()
        .iter()
        .filter_map(|op| match op {
            atomik_ui::Op::Fill {
                shape: atomik_ui::Geom::Path(p),
                ..
            } if p.elements().len() > 100 => Some(p.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_reversed_clip_draws_its_waveform() {
    let forward = wave_polygons(&audio_doc());
    assert!(!forward.is_empty(), "the forward clip has a waveform");
    let mut doc = audio_doc();
    edit_clip(&mut doc, 10, |a| a.reverse = true);
    let reversed = wave_polygons(&doc);
    assert_eq!(reversed.len(), forward.len(), "one polygon per channel too");
    assert_ne!(reversed, forward, "and it is the mirrored audio");
}
