//! Audio clips in the arrangement: import mapping from file drops,
//! the add-audio-track row, waveform painting, clip handles, and the audio track header.

mod support;

use atomik_model::{ClipId, SongCommand};
use atomik_ui::kurbo::Point;
use atomik_ui::timeline::{Snap, snap_round};
use atomik_ui::{ImportTarget, Modifiers, UiAction, UiEvent};
use std::path::PathBuf;
use support::audio::*;
use support::song::*;
use support::*;

fn paths(n: usize) -> Vec<PathBuf> {
    (0..n)
        .map(|i| PathBuf::from(format!("take{i}.wav")))
        .collect()
}

fn drop_at(pos: Point, n: usize) -> UiEvent {
    UiEvent::FileDrop {
        pos,
        paths: paths(n),
    }
}

fn hover_at(pos: Point, n: usize) -> UiEvent {
    UiEvent::FileHover {
        pos,
        paths: paths(n),
    }
}

/// A point inside the lane of `device` at logical x.
fn in_lane(v: &atomik_ui::ArrangementView, device: &str, x: f64) -> Point {
    let r = v.lane_rect(&key(device), 0).unwrap();
    Point::new(x, r.center().y)
}

// ---------------------------------------------------------------- drop mapping

#[test]
fn a_drop_on_an_audio_lane_targets_that_track_at_the_snapped_tick() {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    v.set_snap(Snap::Beat);
    let pos = in_lane(&v, "audio-track-2", 330.0);
    let tick = snap_round(v.x_to_tick(330.0), Some(PPQ));
    let a = v.handle(&drop_at(pos, 1), &doc, &reg);
    assert_eq!(
        a,
        vec![UiAction::ImportAudio {
            paths: paths(1),
            target: ImportTarget::ExistingTrack(key("audio-track-2")),
            tick,
        }]
    );
    assert!(tick > 0 && tick % PPQ == 0);
}

#[test]
fn several_files_on_one_track_are_one_action() {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    v.set_snap(Snap::Off);
    let pos = in_lane(&v, "audio-track", 700.0);
    let a = v.handle(&drop_at(pos, 3), &doc, &reg);
    let tick = v.x_to_tick(700.0);
    assert_eq!(
        a,
        vec![UiAction::ImportAudio {
            paths: paths(3),
            target: ImportTarget::ExistingTrack(key("audio-track")),
            tick,
        }]
    );
}

#[test]
fn a_drop_elsewhere_in_the_track_area_creates_new_tracks() {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    v.set_snap(Snap::Bar);
    let bar_x = v.tick_to_x(2 * BAR);
    // On a note lane, on the add-track row and below every row.
    let note_lane = in_lane(&v, "bass", bar_x + 3.0);
    let add_row = {
        let c = v.add_audio_track_center().unwrap();
        Point::new(bar_x + 3.0, c.y)
    };
    let below = Point::new(bar_x + 3.0, 395.0);
    for pos in [note_lane, add_row, below] {
        let a = v.handle(&drop_at(pos, 2), &doc, &reg);
        assert_eq!(
            a,
            vec![UiAction::ImportAudio {
                paths: paths(2),
                target: ImportTarget::NewTracks,
                tick: 2 * BAR,
            }],
            "{pos:?}"
        );
    }
}

#[test]
fn a_drop_on_the_track_header_uses_the_left_edge_of_the_timeline() {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    v.set_snap(Snap::Bar);
    v.set_scroll_ticks(3.0 * BAR as f64);
    let r = v.lane_rect(&key("audio-track-2"), 0).unwrap();
    let a = v.handle(&drop_at(Point::new(40.0, r.center().y), 1), &doc, &reg);
    assert_eq!(
        a,
        vec![UiAction::ImportAudio {
            paths: paths(1),
            target: ImportTarget::ExistingTrack(key("audio-track-2")),
            tick: 3 * BAR,
        }]
    );
}

#[test]
fn drops_outside_the_lanes_emit_nothing() {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    let lane = in_lane(&v, "audio-track", 400.0);
    for pos in [
        Point::new(400.0, 10.0),    // ruler
        Point::new(40.0, 10.0),     // corner
        Point::new(1500.0, lane.y), // right of the view
        Point::new(400.0, 900.0),   // below the view
        Point::new(-5.0, lane.y),
    ] {
        assert!(v.handle(&drop_at(pos, 1), &doc, &reg).is_empty(), "{pos:?}");
    }
    assert!(
        v.handle(&drop_at(lane, 0), &doc, &reg).is_empty(),
        "no files"
    );
}

#[test]
fn hover_shows_an_indicator_until_it_is_cancelled_or_dropped() {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    v.set_snap(Snap::Beat);
    let plain = {
        let mut dl = atomik_ui::DisplayList::new();
        v.paint(&mut dl, &doc, &reg);
        dl.to_json()
    };
    assert!(!v.needs_repaint());
    let lane = in_lane(&v, "audio-track-2", 420.0);
    assert!(v.handle(&hover_at(lane, 1), &doc, &reg).is_empty());
    assert!(v.needs_repaint());
    assert_eq!(
        v.drop_hover(),
        Some((
            ImportTarget::ExistingTrack(key("audio-track-2")),
            snap_round(v.x_to_tick(420.0), Some(PPQ))
        ))
    );
    let hovered = {
        let mut dl = atomik_ui::DisplayList::new();
        v.paint(&mut dl, &doc, &reg);
        dl.to_json()
    };
    assert_ne!(hovered, plain, "the indicator is painted");
    // Moving to a note lane switches to the new-track indicator.
    let note = in_lane(&v, "bass", 420.0);
    let _ = v.handle(&hover_at(note, 2), &doc, &reg);
    assert_eq!(v.drop_hover().map(|h| h.0), Some(ImportTarget::NewTracks));
    // Cancel removes it again.
    let _ = v.handle(&UiEvent::FileHoverCancel, &doc, &reg);
    assert_eq!(v.drop_hover(), None);
    let after = {
        let mut dl = atomik_ui::DisplayList::new();
        v.paint(&mut dl, &doc, &reg);
        dl.to_json()
    };
    assert_eq!(after, plain);
    // A drop also removes the indicator; a hover outside the lanes shows none.
    let _ = v.handle(&hover_at(lane, 1), &doc, &reg);
    let _ = v.handle(&drop_at(lane, 1), &doc, &reg);
    assert_eq!(v.drop_hover(), None);
    let _ = v.handle(&hover_at(Point::new(400.0, 10.0), 1), &doc, &reg);
    assert_eq!(v.drop_hover(), None);
}

// ---------------------------------------------------------------- add audio track

#[test]
fn the_add_audio_track_row_emits_add_audio_track() {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    let p = v.add_audio_track_center().unwrap();
    let a = run(|e| v.handle(e, &doc, &reg), &[down(p), up(p)]);
    assert_eq!(
        song_commands(&a),
        vec![SongCommand::AddAudioTrack { name: None }]
    );
}

#[test]
fn the_import_dialog_action_exists() {
    assert_eq!(
        UiAction::ImportAudioDialog,
        UiAction::ImportAudioDialog.clone()
    );
}

// ---------------------------------------------------------------- peaks cache

#[test]
fn peaks_can_be_set_and_cleared() {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    let paint = |v: &mut atomik_ui::ArrangementView| {
        let mut dl = atomik_ui::DisplayList::new();
        v.paint(&mut dl, &doc, &reg);
        dl.to_json()
    };
    let with = paint(&mut v);
    assert!(!v.needs_repaint());
    v.clear_peaks(HASH_A);
    assert!(v.needs_repaint());
    let without = paint(&mut v);
    assert_ne!(with, without);
    v.set_peaks(HASH_A, pyramid(FRAMES_A, 2));
    assert_eq!(paint(&mut v), with);
    let _ = ClipId(0);
}

// ---------------------------------------------------------------- handles and trims

/// 1 px = 1000 frames at the default zoom and 120 BPM (96000 frames per bar, 96 px per bar).
const FRAMES_PER_PX: f64 = 1000.0;
/// Source offset of clip 10 in frames.
const OFFSET_10: u64 = 48_000;
/// Clip 10 lasts two bars: 192000 frames.
const CLIP_FRAMES_10: u64 = 2 * FRAMES_PER_BAR;

fn content10(
    source_offset: u64,
    gain_db: f32,
    fade_in: u64,
    fade_out: u64,
) -> atomik_model::AudioClipContent {
    atomik_model::AudioClipContent {
        asset: HASH_A.into(),
        source_offset,
        gain_db,
        fade_in,
        fade_out,
        fade_curve: atomik_model::FadeCurve::Linear,
        reverse: false,
        envelope: Vec::new(),
        xfade: None,
        warp: Default::default(),
    }
}

fn set_audio(
    id: u64,
    start: i64,
    length: i64,
    content: atomik_model::AudioClipContent,
    merge: u64,
) -> SongCommand {
    SongCommand::SetAudioClip {
        id: ClipId(id),
        start,
        length,
        muted: false,
        content,
        merge: Some(merge),
    }
}

/// The audio clip commands among `actions`, with the merge id of each.
fn audio_cmds(actions: &[UiAction]) -> Vec<SongCommand> {
    song_commands(actions)
        .into_iter()
        .filter(|c| matches!(c, SongCommand::SetAudioClip { .. }))
        .collect()
}

fn merge_of(c: &SongCommand) -> u64 {
    match c {
        SongCommand::SetAudioClip { merge, .. } => merge.expect("merge id"),
        other => panic!("not an audio clip command: {other:?}"),
    }
}

fn last_content(actions: &[UiAction]) -> atomik_model::AudioClipContent {
    match audio_cmds(actions).last() {
        Some(SongCommand::SetAudioClip { content, .. }) => content.clone(),
        other => panic!("no audio clip command: {other:?}"),
    }
}

fn drag_to(
    v: &mut atomik_ui::ArrangementView,
    doc: &atomik_model::RackDoc,
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

fn handle_pos(v: &atomik_ui::ArrangementView, id: u64, h: atomik_ui::AudioHandle) -> Point {
    v.audio_handle_center(ClipId(id), h).expect("handle")
}

#[test]
fn handle_positions_follow_fades_and_gain() {
    use atomik_ui::AudioHandle::*;
    let (doc, reg) = (audio_doc(), registry());
    let v = audio_arrangement(&doc, &reg);
    let r = v.clip_rect(ClipId(10)).unwrap();
    // 24000 frames = 24 px from the left; 48000 frames = 48 px from the right.
    let fi = handle_pos(&v, 10, FadeIn);
    let fo = handle_pos(&v, 10, FadeOut);
    assert!((fi.x - (r.x0 + 24.0)).abs() < 0.01, "{fi:?}");
    assert!((fo.x - (r.x1 - 48.0)).abs() < 0.01, "{fo:?}");
    assert!(
        fi.y < r.y0 + 12.0 && fo.y < r.y0 + 12.0,
        "in the header strip"
    );
    let g = handle_pos(&v, 10, Gain);
    assert!((g.x - (r.x0 + r.x1) / 2.0).abs() < 0.01);
    // -3 dB sits a little below the top of the +12 .. -60 dB scale.
    let (top, bottom) = (r.y0 + 12.0 + 4.0, r.y1 - 4.0);
    let want = top + (12.0 + 3.0) / 72.0 * (bottom - top);
    assert!((g.y - want).abs() < 0.01, "{g:?} want y {want}");
    // Handles exist only on audio clips, and the gain handle needs some width.
    assert!(v.audio_handle_center(ClipId(1), Gain).is_none());
    assert!(v.audio_handle_center(ClipId(99), Gain).is_none());
    let mut small = audio_arrangement(&doc, &reg);
    small.set_px_per_tick(0.0015625 / 4.0);
    assert!(small.audio_handle_center(ClipId(10), Gain).is_none());
    assert!(small.audio_handle_center(ClipId(10), FadeIn).is_some());
}

#[test]
fn dragging_the_fade_in_handle_changes_fade_in_frames_without_snapping() {
    use atomik_ui::AudioHandle::FadeIn;
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    v.set_snap(Snap::Bar);
    let from = handle_pos(&v, 10, FadeIn);
    let a = drag_to(
        &mut v,
        &doc,
        &reg,
        from,
        from + atomik_ui::kurbo::Vec2::new(10.3, 0.0),
    );
    let cmds = audio_cmds(&a);
    assert_eq!(cmds.len(), 2, "one per move: {cmds:?}");
    let merge = merge_of(&cmds[0]);
    assert!(merge >= 1 << 40, "ids start above the rack view's");
    assert!(cmds.iter().all(|c| merge_of(c) == merge));
    let want = 24_000 + (10.3 * FRAMES_PER_PX).round() as u64;
    let SongCommand::SetAudioClip {
        id,
        start,
        length,
        muted,
        content,
        ..
    } = cmds.last().unwrap().clone()
    else {
        unreachable!()
    };
    assert_eq!(
        (id, start, length, muted),
        (ClipId(10), BAR, 2 * BAR, false)
    );
    assert!(
        content.fade_in.abs_diff(want) <= 2,
        "{} vs {want}",
        content.fade_in
    );
    assert_eq!(
        (content.fade_out, content.gain_db, content.source_offset),
        (48_000, -3.0, OFFSET_10)
    );
    assert_eq!(a.last(), Some(&UiAction::EndMerge));
    assert_eq!(v.selected_clips(), vec![ClipId(10)]);
}

#[test]
fn fades_clamp_to_the_clip_and_to_each_other() {
    use atomik_ui::AudioHandle::{FadeIn, FadeOut};
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    let far = atomik_ui::kurbo::Vec2::new(900.0, 0.0);
    let from = handle_pos(&v, 10, FadeIn);
    let a = drag_to(&mut v, &doc, &reg, from, from + far);
    assert_eq!(
        last_content(&a).fade_in,
        CLIP_FRAMES_10 - 48_000,
        "up to the fade-out"
    );
    let a = drag_to(&mut v, &doc, &reg, from, from - far);
    assert_eq!(last_content(&a).fade_in, 0);
    let from = handle_pos(&v, 10, FadeOut);
    let a = drag_to(&mut v, &doc, &reg, from, from - far);
    assert_eq!(last_content(&a).fade_out, CLIP_FRAMES_10 - 24_000);
    let a = drag_to(&mut v, &doc, &reg, from, from + far);
    assert_eq!(last_content(&a).fade_out, 0);
    // Dragging the out handle left by 20 px lengthens it by 20000 frames.
    let a = drag_to(
        &mut v,
        &doc,
        &reg,
        from,
        from - atomik_ui::kurbo::Vec2::new(20.0, 0.0),
    );
    assert_eq!(last_content(&a).fade_out, 68_000);
}

#[test]
fn dragging_the_gain_handle_changes_gain_with_shift_for_fine_control() {
    use atomik_ui::AudioHandle::Gain;
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    let from = handle_pos(&v, 10, Gain);
    // The scale is 34 px tall for 72 dB. 5 px up: -3 + 10.588 = 7.6 dB (0.1 dB steps).
    let a = drag_to(
        &mut v,
        &doc,
        &reg,
        from,
        from - atomik_ui::kurbo::Vec2::new(0.0, 5.0),
    );
    assert!(
        (last_content(&a).gain_db - 7.6).abs() < 1e-4,
        "{:?}",
        last_content(&a)
    );
    assert_eq!(a.last(), Some(&UiAction::EndMerge));
    // With Shift: a tenth of the speed and 0.01 dB steps.
    let mut a = v.handle(&down(from), &doc, &reg);
    let to = from - atomik_ui::kurbo::Vec2::new(0.0, 5.0);
    a.extend(v.handle(&mv_with(to, Modifiers::SHIFT), &doc, &reg));
    a.extend(v.handle(&up(to), &doc, &reg));
    assert!((last_content(&a).gain_db - (-3.0 + 1.0588)).abs() < 0.01);
    // Clamped to -60 .. +12 dB.
    let a = drag_to(
        &mut v,
        &doc,
        &reg,
        from,
        from + atomik_ui::kurbo::Vec2::new(0.0, 300.0),
    );
    assert_eq!(last_content(&a).gain_db, -60.0);
    let a = drag_to(
        &mut v,
        &doc,
        &reg,
        from,
        from - atomik_ui::kurbo::Vec2::new(0.0, 300.0),
    );
    assert_eq!(last_content(&a).gain_db, 12.0);
}

#[test]
fn a_click_on_a_handle_without_moving_emits_nothing() {
    use atomik_ui::AudioHandle::Gain;
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    let p = handle_pos(&v, 10, Gain);
    let a = run(|e| v.handle(e, &doc, &reg), &[down(p), up(p)]);
    assert!(a.is_empty(), "{a:?}");
    assert!(!v.is_dragging());
}

#[test]
fn the_right_edge_trims_the_length_on_the_grid_and_clamps_the_fades() {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    v.set_snap(Snap::Bar);
    let r = v.clip_rect(ClipId(10)).unwrap();
    let edge = Point::new(r.x1 - 2.0, r.center().y + 12.0);
    let a = drag_to(
        &mut v,
        &doc,
        &reg,
        edge,
        edge + atomik_ui::kurbo::Vec2::new(96.0, 0.0),
    );
    assert_eq!(
        audio_cmds(&a).last(),
        Some(&set_audio(
            10,
            BAR,
            3 * BAR,
            content10(OFFSET_10, -3.0, 24_000, 48_000),
            merge_of(&audio_cmds(&a)[0])
        ))
    );
    assert_eq!(a.last(), Some(&UiAction::EndMerge));
    // Trimmed to the minimum with snapping off (960 ticks = 1500 frames): the fades shrink.
    v.set_snap(Snap::Off);
    let a = drag_to(
        &mut v,
        &doc,
        &reg,
        edge,
        edge - atomik_ui::kurbo::Vec2::new(400.0, 0.0),
    );
    let SongCommand::SetAudioClip {
        start,
        length,
        content,
        ..
    } = audio_cmds(&a).last().unwrap().clone()
    else {
        unreachable!()
    };
    assert_eq!((start, length), (BAR, PPQ / 16));
    assert_eq!((content.fade_in, content.fade_out), (1500, 0));
    assert_eq!(content.source_offset, OFFSET_10, "the offset does not move");
}

#[test]
fn the_left_edge_trims_start_and_offset_together() {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    v.set_snap(Snap::Beat);
    let r = v.clip_rect(ClipId(10)).unwrap();
    let edge = Point::new(r.x0 + 2.0, r.center().y + 12.0);
    let px = |dx: f64| atomik_ui::kurbo::Vec2::new(dx, 0.0);
    // Half a bar (48 px = 1 s = 48000 frames) earlier uncovers the whole second before the
    // offset: the start moves to tick 30720 and the offset to frame 0.
    let a = drag_to(&mut v, &doc, &reg, edge, edge - px(48.0));
    let merge = merge_of(&audio_cmds(&a)[0]);
    assert_eq!(
        audio_cmds(&a).last(),
        Some(&set_audio(
            10,
            30_720,
            2 * BAR + 30_720,
            content10(0, -3.0, 24_000, 48_000),
            merge
        ))
    );
    assert_eq!(a.last(), Some(&UiAction::EndMerge));
    // Further left stops at frame 0 of the asset.
    let a = drag_to(&mut v, &doc, &reg, edge, edge - px(90.0));
    let SongCommand::SetAudioClip { start, content, .. } = audio_cmds(&a).last().unwrap().clone()
    else {
        unreachable!()
    };
    assert_eq!((start, content.source_offset), (30_720, 0));
    // Inward by half a bar: the offset advances by the same second.
    let a = drag_to(&mut v, &doc, &reg, edge, edge + px(48.0));
    let SongCommand::SetAudioClip {
        start,
        length,
        content,
        ..
    } = audio_cmds(&a).last().unwrap().clone()
    else {
        unreachable!()
    };
    assert_eq!((start, length), (BAR + 30_720, 2 * BAR - 30_720));
    assert_eq!(content.source_offset, OFFSET_10 + 48_000);
}

#[test]
fn trimming_never_goes_before_tick_zero_or_past_the_asset_end() {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    v.set_snap(Snap::Off);
    // Clip 11 (mono, 144000 frames) starts at bar 5 with offset 24000.
    let r = v.clip_rect(ClipId(11)).unwrap();
    let edge = Point::new(r.x0 + 2.0, r.center().y + 12.0);
    let a = drag_to(
        &mut v,
        &doc,
        &reg,
        edge,
        edge - atomik_ui::kurbo::Vec2::new(300.0, 0.0),
    );
    let SongCommand::SetAudioClip {
        start,
        length,
        content,
        ..
    } = audio_cmds(&a).last().unwrap().clone()
    else {
        unreachable!()
    };
    // 24000 frames = 15360 ticks earlier is the earliest start.
    assert_eq!((start, content.source_offset), (4 * BAR - PPQ, 0));
    assert_eq!(start + length, 6 * BAR, "the end stays");
    // Trimming the start towards the end stops where the offset reaches the asset end:
    // 144000 - 24000 = 120000 frames = 76800 ticks after the start.
    let a = drag_to(
        &mut v,
        &doc,
        &reg,
        edge,
        edge + atomik_ui::kurbo::Vec2::new(300.0, 0.0),
    );
    let SongCommand::SetAudioClip { start, content, .. } = audio_cmds(&a).last().unwrap().clone()
    else {
        unreachable!()
    };
    assert_eq!(start, 4 * BAR + 5 * PPQ);
    assert_eq!(content.source_offset, FRAMES_B);
}

#[test]
fn the_body_moves_the_clip_keeping_its_content() {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    v.set_snap(Snap::Beat);
    let r = v.clip_rect(ClipId(10)).unwrap();
    let grab = Point::new(r.x0 + 40.0, r.y1 - 8.0);
    let a = drag_to(
        &mut v,
        &doc,
        &reg,
        grab,
        grab + atomik_ui::kurbo::Vec2::new(48.0, 0.0),
    );
    let cmds = song_commands(&a);
    assert!(
        cmds.iter()
            .all(|c| matches!(c, SongCommand::SetAudioClip { .. })),
        "audio clips move with SetAudioClip: {cmds:?}"
    );
    let merge = merge_of(&cmds[0]);
    assert_eq!(
        cmds.last(),
        Some(&set_audio(
            10,
            BAR + 30_720,
            2 * BAR,
            content10(OFFSET_10, -3.0, 24_000, 48_000),
            merge
        ))
    );
    assert_eq!(a.last(), Some(&UiAction::EndMerge));
}

#[test]
fn moving_a_mixed_selection_uses_the_right_command_per_clip() {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    v.set_snap(Snap::Bar);
    v.set_selected_clips(&[ClipId(1), ClipId(10)]);
    let r = v.clip_rect(ClipId(10)).unwrap();
    let grab = Point::new(r.x0 + 40.0, r.y1 - 8.0);
    let a = drag_to(
        &mut v,
        &doc,
        &reg,
        grab,
        grab + atomik_ui::kurbo::Vec2::new(96.0, 0.0),
    );
    let cmds = song_commands(&a);
    assert!(
        cmds.iter()
            .any(|c| matches!(c, SongCommand::SetClipGeometry { id, .. } if *id == ClipId(1)))
    );
    assert!(cmds.iter().any(|c| matches!(c, SongCommand::SetAudioClip { id, start, .. } if *id == ClipId(10) && *start == 2 * BAR)));
}

#[test]
fn escape_restores_the_clip_during_a_handle_drag() {
    use atomik_ui::AudioHandle::FadeIn;
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    let from = handle_pos(&v, 10, FadeIn);
    let mut a = v.handle(&down(from), &doc, &reg);
    a.extend(v.handle(
        &mv(from + atomik_ui::kurbo::Vec2::new(30.0, 0.0)),
        &doc,
        &reg,
    ));
    a.extend(v.handle(&press(atomik_ui::Key::Escape, Modifiers::NONE), &doc, &reg));
    let cmds = audio_cmds(&a);
    assert_eq!(cmds.len(), 2);
    assert_eq!(last_content(&a), content10(OFFSET_10, -3.0, 24_000, 48_000));
    assert_eq!(merge_of(&cmds[0]), merge_of(&cmds[1]));
    assert_eq!(a.last(), Some(&UiAction::EndMerge));
    assert!(!v.is_dragging());
    // The released button after the cancel emits nothing more.
    assert!(v.handle(&up(from), &doc, &reg).is_empty());
}

// ---------------------------------------------------------------- audio track header

fn header_texts(v: &mut atomik_ui::ArrangementView, doc: &atomik_model::RackDoc) -> Vec<String> {
    let mut dl = atomik_ui::DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl.texts().into_iter().map(str::to_string).collect()
}

fn click(
    v: &mut atomik_ui::ArrangementView,
    doc: &atomik_model::RackDoc,
    reg: &atomik_device_sdk::Registry,
    p: Point,
) -> Vec<UiAction> {
    run(|e| v.handle(e, doc, reg), &[down(p), up(p)])
}

fn commands_text(actions: &[UiAction]) -> Vec<String> {
    // An input change is a single `Command::Batch` (one undo step).
    let all = commands(actions);
    assert!(all.len() <= 1, "one command per input change: {all:?}");
    all.into_iter()
        .flat_map(|c| match c {
            atomik_model::Command::Batch(cs) => cs,
            other => panic!("expected a cable batch, got {other:?}"),
        })
        .map(|c| match c {
            atomik_model::Command::Disconnect { to } => format!("-{}", to.socket),
            atomik_model::Command::Connect(c) => {
                format!("{}>{}", c.from.socket, c.to.socket)
            }
            other => panic!("unexpected {other:?}"),
        })
        .collect()
}

#[test]
fn the_arm_button_works_for_audio_tracks() {
    use atomik_ui::TrackButton::Arm;
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    let p = v.track_button_center(&key("audio-track"), Arm).unwrap();
    let a = click(&mut v, &doc, &reg, p);
    assert_eq!(
        song_commands(&a),
        vec![SongCommand::SetTrackFlags {
            device: key("audio-track"),
            muted: false,
            solo: false,
            armed: true
        }]
    );
}

#[test]
fn audio_tracks_have_input_and_monitor_buttons_instead_of_add_lane() {
    use atomik_ui::TrackButton::*;
    let (doc, reg) = (audio_doc(), registry());
    let v = audio_arrangement(&doc, &reg);
    assert!(v.track_button_center(&key("audio-track"), Input).is_some());
    assert!(
        v.track_button_center(&key("audio-track"), Monitor)
            .is_some()
    );
    assert!(
        v.track_button_center(&key("audio-track"), AddLane)
            .is_none()
    );
    assert!(v.track_button_center(&key("bass"), AddLane).is_some());
    assert!(v.track_button_center(&key("bass"), Input).is_none());
    assert!(v.track_button_center(&key("bass"), Monitor).is_none());
    // The controls do not overlap.
    let rects: Vec<Point> = [Mute, Solo, Arm, Input, Monitor]
        .iter()
        .map(|b| v.track_button_center(&key("audio-track"), *b).unwrap())
        .collect();
    assert!(
        rects.windows(2).all(|w| w[1].x - w[0].x > 20.0),
        "{rects:?}"
    );
}

#[test]
fn the_input_menu_lists_off_mono_and_stereo_pairs() {
    use atomik_ui::TrackButton::Input;
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    let p = v.track_button_center(&key("audio-track"), Input).unwrap();
    assert!(click(&mut v, &doc, &reg, p).is_empty());
    assert_eq!(
        v.menu_labels(),
        ["Off", "In 1", "In 2", "In 3", "In 4", "In 1+2", "In 3+4"]
    );
    assert_eq!(v.menu_current(), Some(5), "In 1+2 is cabled");
    // Escape closes it without a command.
    let a = v.handle(&press(atomik_ui::Key::Escape, Modifiers::NONE), &doc, &reg);
    assert!(a.is_empty() && v.menu_labels().is_empty());
    // An empty track offers the same list with Off current.
    let p = v.track_button_center(&key("audio-track-2"), Input).unwrap();
    let _ = click(&mut v, &doc, &reg, p);
    assert_eq!(v.menu_current(), Some(0));
}

#[test]
fn the_input_menu_with_no_inputs_offers_only_off() {
    use atomik_ui::TrackButton::Input;
    let (mut doc, reg) = (audio_doc(), registry());
    doc.input_channels = 0;
    doc.cables.clear();
    let mut v = audio_arrangement(&doc, &reg);
    let p = v.track_button_center(&key("audio-track"), Input).unwrap();
    let _ = click(&mut v, &doc, &reg, p);
    assert_eq!(v.menu_labels(), ["Off"]);
}

fn pick_input(device: &str, label: &str) -> Vec<String> {
    use atomik_ui::TrackButton::Input;
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    let p = v.track_button_center(&key(device), Input).unwrap();
    let _ = click(&mut v, &doc, &reg, p);
    let i = v.menu_labels().iter().position(|l| l == label).unwrap();
    let row = v.param_menu_row_center(i).unwrap();
    let a = click(&mut v, &doc, &reg, row);
    assert!(v.menu_labels().is_empty(), "the menu closes");
    assert!(
        !a.contains(&UiAction::EndMerge) && a.iter().all(|x| matches!(x, UiAction::Command(_))),
        "{a:?}"
    );
    commands_text(&a)
}

#[test]
fn choosing_an_input_emits_the_cable_commands_replacing_the_old_ones() {
    // audio-track is on 1+2.
    assert_eq!(
        pick_input("audio-track", "In 3+4"),
        ["-in_l", "-in_r", "in_3>in_l", "in_4>in_r"]
    );
    assert_eq!(
        pick_input("audio-track", "In 3"),
        ["-in_l", "-in_r", "in_3>in_l", "in_3>in_r"]
    );
    assert_eq!(pick_input("audio-track", "In 1"), ["-in_r", "in_1>in_r"]);
    assert_eq!(pick_input("audio-track", "Off"), ["-in_l", "-in_r"]);
    assert!(pick_input("audio-track", "In 1+2").is_empty(), "no change");
    // audio-track-2 has no cables.
    assert_eq!(
        pick_input("audio-track-2", "In 2"),
        ["in_2>in_l", "in_2>in_r"]
    );
    assert_eq!(
        pick_input("audio-track-2", "In 1+2"),
        ["in_1>in_l", "in_2>in_r"]
    );
    assert!(pick_input("audio-track-2", "Off").is_empty());
}

#[test]
fn a_click_outside_the_input_menu_closes_it_silently() {
    use atomik_ui::TrackButton::Input;
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    let p = v.track_button_center(&key("audio-track"), Input).unwrap();
    let _ = click(&mut v, &doc, &reg, p);
    let a = click(&mut v, &doc, &reg, Point::new(900.0, 300.0));
    assert!(a.is_empty() && v.menu_labels().is_empty());
}

#[test]
fn the_header_shows_the_selection_derived_from_the_cables() {
    let (mut doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    v.set_snap(Snap::Beat);
    let texts = header_texts(&mut v, &doc);
    assert!(texts.iter().any(|t| t == "In 1+2"), "{texts:?}");
    assert!(texts.iter().any(|t| t == "Off"), "{texts:?}");
    // Re-cabling in the rack updates the header although the song did not change.
    doc.cables = vec![
        cable(("interface", "in_3"), ("audio-track", "in_l")),
        cable(("interface", "in_3"), ("audio-track", "in_r")),
    ];
    doc.cables.sort();
    v.sync(&doc, &reg);
    assert!(v.needs_repaint());
    let texts = header_texts(&mut v, &doc);
    assert!(texts.iter().any(|t| t == "In 3"), "{texts:?}");
    assert!(!texts.iter().any(|t| t == "In 1+2"));
}

#[test]
fn the_monitor_button_cycles_off_auto_on() {
    use atomik_ui::TrackButton::Monitor;
    let (mut doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    let set = |value: f32| {
        vec![atomik_model::Command::SetParam {
            key: key("audio-track"),
            param: "monitor".into(),
            value,
            merge: None,
        }]
    };
    // The default is auto (1); the header says so.
    assert!(header_texts(&mut v, &doc).iter().any(|t| t == "AUTO"));
    let p = v.track_button_center(&key("audio-track"), Monitor).unwrap();
    assert_eq!(commands(&click(&mut v, &doc, &reg, p)), set(2.0));
    for (stored, label, next) in [(2.0, "ON", 0.0), (0.0, "OFF", 1.0)] {
        doc.devices[1].params.insert("monitor".into(), stored);
        v.sync(&doc, &reg);
        assert!(header_texts(&mut v, &doc).iter().any(|t| t == label));
        assert_eq!(
            commands(&click(&mut v, &doc, &reg, p)),
            set(next),
            "{label}"
        );
    }
}

// ---------------------------------------------------------------- menus, hover, cancel

fn paint_json(v: &mut atomik_ui::ArrangementView, doc: &atomik_model::RackDoc) -> String {
    let mut dl = atomik_ui::DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl.to_json()
}

#[test]
fn the_open_input_menu_is_painted_and_marks_the_current_choice() {
    use atomik_ui::TrackButton::Input;
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    let closed = paint_json(&mut v, &doc);
    let p = v.track_button_center(&key("audio-track"), Input).unwrap();
    let _ = click(&mut v, &doc, &reg, p);
    let open = paint_json(&mut v, &doc);
    assert_ne!(open, closed);
    let mut dl = atomik_ui::DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    let texts = dl.texts();
    for label in ["Off", "In 1", "In 4", "In 1+2", "In 3+4"] {
        assert!(texts.contains(&label), "{label} in {texts:?}");
    }
    // Hovering a row lights it.
    let row = v.param_menu_row_center(6).unwrap();
    let _ = v.handle(&mv(row), &doc, &reg);
    assert_ne!(paint_json(&mut v, &doc), open);
}

#[test]
fn hovering_handles_and_the_add_row_lights_them() {
    use atomik_ui::AudioHandle::*;
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    let rest = paint_json(&mut v, &doc);
    let mut seen = vec![rest.clone()];
    for h in [FadeIn, FadeOut, Gain] {
        let p = handle_pos(&v, 10, h);
        let _ = v.handle(&mv(p), &doc, &reg);
        let lit = paint_json(&mut v, &doc);
        assert!(!seen.contains(&lit), "{h:?} is lit differently");
        seen.push(lit);
    }
    let _ = v.handle(&mv(Point::new(900.0, 300.0)), &doc, &reg);
    let p = v.add_audio_track_center().unwrap();
    let _ = v.handle(&mv(p), &doc, &reg);
    assert!(!seen.contains(&paint_json(&mut v, &doc)));
    // Leaving the view clears the hover.
    let _ = v.handle(&UiEvent::PointerLeave, &doc, &reg);
    assert_eq!(paint_json(&mut v, &doc), rest);
}

#[test]
fn escape_restores_moves_and_trims_of_audio_clips() {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    v.set_snap(Snap::Beat);
    let r = v.clip_rect(ClipId(10)).unwrap();
    let original = content10(OFFSET_10, -3.0, 24_000, 48_000);
    let esc = press(atomik_ui::Key::Escape, Modifiers::NONE);
    let grabs = [
        Point::new(r.x0 + 40.0, r.y1 - 8.0),         // body: move
        Point::new(r.x1 - 2.0, r.center().y + 12.0), // right edge: resize
        Point::new(r.x0 + 2.0, r.center().y + 12.0), // left edge: trim
    ];
    for grab in grabs {
        let mut a = v.handle(&down(grab), &doc, &reg);
        a.extend(v.handle(
            &mv(grab + atomik_ui::kurbo::Vec2::new(-40.0, 0.0)),
            &doc,
            &reg,
        ));
        a.extend(v.handle(&esc, &doc, &reg));
        assert_eq!(
            audio_cmds(&a).last(),
            Some(&set_audio(
                10,
                BAR,
                2 * BAR,
                original.clone(),
                merge_of(&audio_cmds(&a)[0])
            )),
            "{grab:?}"
        );
        assert_eq!(a.last(), Some(&UiAction::EndMerge));
        assert!(v.handle(&up(grab), &doc, &reg).is_empty());
    }
}

#[test]
fn a_clip_with_a_missing_asset_still_trims_freely_at_the_right() {
    let (mut doc, reg) = (audio_doc(), registry());
    doc.assets.remove(HASH_B);
    let mut v = audio_arrangement(&doc, &reg);
    v.set_snap(Snap::Off);
    let r = v.clip_rect(ClipId(11)).unwrap();
    let edge = Point::new(r.x0 + 2.0, r.center().y + 12.0);
    // Without pool data nothing limits the offset except frame 0 (24000 frames earlier).
    let a = drag_to(
        &mut v,
        &doc,
        &reg,
        edge,
        edge - atomik_ui::kurbo::Vec2::new(300.0, 0.0),
    );
    let SongCommand::SetAudioClip { content, .. } = audio_cmds(&a).last().unwrap().clone() else {
        unreachable!()
    };
    assert_eq!(content.source_offset, 0);
}

#[test]
fn toggle_chase_flips_the_selected_note_track_and_declines_for_audio_and_no_track() {
    use atomik_ui::keymap::CommandOutcome;
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    let run =
        |v: &mut atomik_ui::ArrangementView| v.command("sequencer.toggle_chase", None, &doc, &reg);
    assert_eq!(run(&mut v), CommandOutcome::Declined, "no track selected");
    assert_eq!(v.chase_menu(), (false, false));
    v.set_selected_track(Some(key("bass")));
    assert_eq!(v.chase_menu(), (true, true));
    let CommandOutcome::Handled(a) = run(&mut v) else {
        panic!("a note track toggles");
    };
    assert_eq!(
        song_commands(&a),
        vec![SongCommand::SetTrackChase {
            device: key("bass"),
            chase: false
        }]
    );
    v.set_selected_track(Some(key("audio-track")));
    assert_eq!(v.chase_menu(), (false, true), "an audio track has no notes");
    assert_eq!(run(&mut v), CommandOutcome::Declined);
}

// ---------------------------------------------------------------- recording a console strip

/// [`audio_doc`] with a console: one channel ("Guitar mix") fed by the guitar track, a bus
/// "Drums" and a master.
fn console_audio_doc() -> atomik_model::RackDoc {
    use atomik_model::console::{ConsoleBus, ConsoleChannel};
    let mut doc = audio_doc();
    doc.devices
        .push(device("channel", "atomik.channel", "Guitar mix"));
    doc.devices.push(device("bus", "atomik.bus", "Drums"));
    doc.devices.push(device("master", "atomik.master", "Main"));
    doc.console.channels.push(ConsoleChannel {
        channel: key("channel"),
        source: Some(key("audio-track")),
    });
    doc.console.buses.push(ConsoleBus {
        bus: key("bus"),
        name: "Drums".into(),
    });
    doc.console.master = Some(key("master"));
    doc
}

fn pick_from(doc: &atomik_model::RackDoc, device_key: &str, label: &str) -> Vec<String> {
    use atomik_ui::TrackButton::Input;
    let reg = registry();
    let mut v = audio_arrangement(doc, &reg);
    let p = v.track_button_center(&key(device_key), Input).unwrap();
    let _ = click(&mut v, doc, &reg, p);
    let i = v.menu_labels().iter().position(|l| l == label).unwrap();
    let row = v.param_menu_row_center(i).unwrap();
    let a = click(&mut v, doc, &reg, row);
    commands(&a)
        .into_iter()
        .flat_map(|c| match c {
            atomik_model::Command::Batch(cs) => cs,
            other => panic!("expected a batch, got {other:?}"),
        })
        .map(|c| match c {
            atomik_model::Command::Disconnect { to } => format!("-{}", to.socket),
            atomik_model::Command::Connect(c) => format!("{}>{}", c.from.socket, c.to.socket),
            atomik_model::Command::SetParam { param, value, .. } => format!("{param}={value}"),
            other => panic!("unexpected {other:?}"),
        })
        .collect()
}

#[test]
fn the_input_menu_offers_every_strip_after_the_inputs_and_picking_one_sets_the_source() {
    use atomik_ui::TrackButton::Input;
    let (doc, reg) = (console_audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    let p = v.track_button_center(&key("audio-track"), Input).unwrap();
    let _ = click(&mut v, &doc, &reg, p);
    assert_eq!(
        v.menu_labels(),
        [
            "Off",
            "In 1",
            "In 2",
            "In 3",
            "In 4",
            "In 1+2",
            "In 3+4",
            "Channel: Guitar mix",
            "Bus: Drums",
            "Master: Main"
        ]
    );
    assert_eq!(
        v.menu_current(),
        Some(5),
        "the inputs are still what it records"
    );
    assert_eq!(
        pick_from(&doc, "audio-track-2", "Channel: Guitar mix"),
        ["rec_source=1"]
    );
    assert_eq!(
        pick_from(&doc, "audio-track-2", "Bus: Drums"),
        ["rec_source=1001"]
    );
    assert_eq!(
        pick_from(&doc, "audio-track-2", "Master: Main"),
        ["rec_source=2000"]
    );
}

#[test]
fn a_track_that_records_a_strip_shows_it_and_going_back_to_an_input_clears_the_source() {
    use atomik_ui::TrackButton::Input;
    let mut doc = console_audio_doc();
    doc.devices[2].params.insert("rec_source".into(), 1001.0);
    let reg = registry();
    let mut v = audio_arrangement(&doc, &reg);
    assert!(
        header_texts(&mut v, &doc)
            .iter()
            .any(|t| t.contains("Bus: Drums")),
        "the header names the strip"
    );
    let p = v.track_button_center(&key("audio-track-2"), Input).unwrap();
    let _ = click(&mut v, &doc, &reg, p);
    let at = v.menu_labels().iter().position(|l| l == "Bus: Drums");
    assert_eq!(v.menu_current(), at, "the strip is the checked row");
    // Picking the strip again changes nothing; an input puts the source back to the inputs
    // first, in the same undo step.
    assert!(pick_from(&doc, "audio-track-2", "Bus: Drums").is_empty());
    assert_eq!(
        pick_from(&doc, "audio-track-2", "In 2"),
        ["rec_source=0", "in_2>in_l", "in_2>in_r"]
    );
}
