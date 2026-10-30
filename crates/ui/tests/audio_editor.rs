//! AudioEditorView through its public contract: modes, switching warp on, adding, dragging,
//! deleting, reverting and splitting markers, moving between clips and renaming the asset.
//! Every emitted command is applied to a real model `Document`.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{ClipContent, ClipId, Command, Document, RackDoc, WarpMarker};
use atomik_ui::keymap::CommandOutcome;
use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{AudioEditorView, AudioMode, Key, Modifiers, UiAction, UiEvent};
use support::audio::*;
use support::song::press;
use support::*;

const CLIP: ClipId = ClipId(10);

fn setup() -> (Document, Registry, AudioEditorView) {
    let reg = registry();
    let doc = Document::new(audio_doc());
    let mut v = AudioEditorView::new(atomik_ui::Theme::dark());
    v.set_bounds(Rect::new(0.0, 0.0, 1000.0, 500.0));
    v.set_peaks(HASH_A, pyramid(FRAMES_A, 2));
    v.open(CLIP);
    v.sync(doc.doc(), &reg);
    (doc, reg, v)
}

fn warp_of(d: &Document) -> atomik_model::WarpSettings {
    match &d.doc().song.clip(CLIP).unwrap().2.content {
        ClipContent::Audio(a) => a.warp.clone(),
        _ => unreachable!(),
    }
}

fn markers_of(d: &Document) -> Vec<WarpMarker> {
    warp_of(d).markers
}

fn warp_on(d: &Document) -> bool {
    warp_of(d).enabled
}

fn apply(d: &mut Document, reg: &Registry, actions: &[UiAction]) {
    for a in actions {
        if let UiAction::Command(c) = a {
            d.apply(c.clone(), reg).expect("the command applies");
        }
    }
}

fn feed(
    v: &mut AudioEditorView,
    d: &mut Document,
    reg: &Registry,
    events: &[UiEvent],
) -> Vec<UiAction> {
    let mut all = Vec::new();
    for e in events {
        let a = v.handle(e, d.doc(), reg);
        apply(d, reg, &a);
        all.extend(a);
    }
    all
}

fn dbl(p: Point) -> Vec<UiEvent> {
    vec![
        UiEvent::PointerDown {
            pos: p,
            button: atomik_ui::PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 2,
        },
        up(p),
    ]
}

fn slice_mode(v: &mut AudioEditorView, d: &mut Document, reg: &Registry) {
    let p = v.mode_button_center(AudioMode::Slice);
    feed(v, d, reg, &[down(p), up(p)]);
    v.sync(d.doc(), reg);
}

fn mid_y(v: &AudioEditorView) -> f64 {
    v.wave_rect().center().y
}

fn with_marker(v: &mut AudioEditorView, d: &mut Document, reg: &Registry) -> Point {
    slice_mode(v, d, reg);
    let w = v.wave_rect();
    feed(
        v,
        d,
        reg,
        &dbl(Point::new(w.x0 + w.width() / 2.0, mid_y(v))),
    );
    v.sync(d.doc(), reg);
    Point::new(v.tick_x(v.markers()[1].tick), mid_y(v))
}

#[test]
fn opening_shows_the_clip_and_keeps_the_mode_across_clips() {
    let (d, reg, mut v) = setup();
    assert!(v.is_present());
    assert_eq!(v.mode(), AudioMode::Overview);
    v.set_mode(AudioMode::Vocal);
    assert_eq!(v.mode(), AudioMode::Overview, "a disabled mode is ignored");
    v.set_mode(AudioMode::Slice);
    v.open(ClipId(11));
    v.sync(d.doc(), &reg);
    assert_eq!(v.mode(), AudioMode::Slice);
    // A note clip is not an audio clip.
    v.open(ClipId(1));
    v.sync(d.doc(), &reg);
    assert!(!v.is_present());
}

#[test]
fn entering_slice_mode_switches_warp_on_in_one_undo_step() {
    let (mut d, reg, mut v) = setup();
    assert!(!warp_on(&d));
    slice_mode(&mut v, &mut d, &reg);
    assert_eq!(v.mode(), AudioMode::Slice);
    assert!(warp_on(&d));
    d.undo(&reg).unwrap();
    assert!(!warp_on(&d));
}

#[test]
fn a_double_click_adds_a_marker_pinned_where_the_audio_plays() {
    let (mut d, reg, mut v) = setup();
    slice_mode(&mut v, &mut d, &reg);
    let w = v.wave_rect();
    let p = Point::new(w.x0 + w.width() / 2.0, mid_y(&v));
    feed(&mut v, &mut d, &reg, &dbl(p));
    v.sync(d.doc(), &reg);
    let m = markers_of(&d);
    assert!(m.len() >= 3, "{m:?}");
    assert_eq!(v.markers().len(), m.len());
    let inner = m[1];
    let bare = atomik_model::WarpSettings {
        markers: Vec::new(),
        ..warp_of(&d)
    };
    assert!((bare.source_frames_at(RATE, inner.tick as f64) - inner.source as f64).abs() <= 0.5);
}

#[test]
fn adding_to_a_clip_without_warp_is_one_step() {
    let (mut d, reg, mut v) = setup();
    v.set_mode(AudioMode::Slice);
    let w = v.wave_rect();
    let depth = d.undo_depth();
    let y = mid_y(&v);
    feed(
        &mut v,
        &mut d,
        &reg,
        &dbl(Point::new(w.x0 + w.width() / 3.0, y)),
    );
    assert!(warp_on(&d));
    assert_eq!(d.undo_depth(), depth + 1);
}

#[test]
fn dragging_a_marker_stretches_in_one_merge_group_and_one_undo_step() {
    let (mut d, reg, mut v) = setup();
    let p = with_marker(&mut v, &mut d, &reg);
    let before = markers_of(&d);
    let depth = d.undo_depth();
    let to = Point::new(p.x + 60.0, p.y);
    let actions = feed(
        &mut v,
        &mut d,
        &reg,
        &[
            down(p),
            mv(Point::new(p.x + 20.0, p.y)),
            mv(Point::new(p.x + 40.0, p.y)),
            mv(to),
            up(to),
        ],
    );
    assert!(actions.contains(&UiAction::EndMerge));
    let after = markers_of(&d);
    assert!(after[1].tick > before[1].tick);
    assert_eq!(
        after[1].source, before[1].source,
        "a stretch keeps the frame"
    );
    assert_eq!(d.undo_depth(), depth + 1);
    d.undo(&reg).unwrap();
    assert_eq!(markers_of(&d), before);
}

#[test]
fn a_slide_keeps_the_audio_under_the_marker() {
    let (mut d, reg, mut v) = setup();
    let p = with_marker(&mut v, &mut d, &reg);
    let before = markers_of(&d);
    let alt = if cfg!(target_os = "macos") {
        Modifiers {
            ctrl: true,
            ..Modifiers::NONE
        }
    } else {
        Modifiers {
            alt: true,
            ..Modifiers::NONE
        }
    };
    let to = Point::new(p.x + 50.0, p.y);
    let events = [
        down(p),
        mv_with(Point::new(p.x + 25.0, p.y), alt),
        mv_with(to, alt),
        up(to),
    ];
    feed(&mut v, &mut d, &reg, &events);
    let after = markers_of(&d);
    assert!(after[1].tick > before[1].tick);
    assert!(after[1].source > before[1].source, "{after:?}");
}

#[test]
fn escape_during_a_drag_restores_the_markers() {
    let (mut d, reg, mut v) = setup();
    let p = with_marker(&mut v, &mut d, &reg);
    let before = markers_of(&d);
    feed(
        &mut v,
        &mut d,
        &reg,
        &[
            down(p),
            mv(Point::new(p.x + 20.0, p.y)),
            mv(Point::new(p.x + 40.0, p.y)),
        ],
    );
    assert_ne!(markers_of(&d), before);
    feed(&mut v, &mut d, &reg, &[press(Key::Escape, Modifiers::NONE)]);
    assert_eq!(markers_of(&d), before);
}

#[test]
fn delete_nudge_and_revert() {
    let (mut d, reg, mut v) = setup();
    let p = with_marker(&mut v, &mut d, &reg);
    feed(&mut v, &mut d, &reg, &[down(p), up(p)]);
    assert_eq!(v.selected_marker(), Some(1));
    let before = markers_of(&d)[1].tick;
    let out = v.command("editor.nudge_right_beat", None, d.doc(), &reg);
    let CommandOutcome::Handled(a) = out else {
        panic!("{out:?}")
    };
    apply(&mut d, &reg, &a);
    assert!(markers_of(&d)[1].tick > before);
    v.sync(d.doc(), &reg);
    let CommandOutcome::Handled(a) = v.command("edit.delete", None, d.doc(), &reg) else {
        panic!("delete")
    };
    apply(&mut d, &reg, &a);
    v.sync(d.doc(), &reg);
    assert_eq!(v.markers().len(), 2, "only the two ends are left");
    let c = v.revert_button_center();
    feed(&mut v, &mut d, &reg, &[down(c), up(c)]);
    assert!(markers_of(&d).is_empty());
}

#[test]
fn split_cuts_at_every_marker_in_one_undo_step() {
    let (mut d, reg, mut v) = setup();
    with_marker(&mut v, &mut d, &reg);
    let clips = |d: &Document| d.doc().song.tracks[1].lanes[0].clips.len();
    let (n, depth) = (clips(&d), d.undo_depth());
    let c = v.split_button_center();
    let a = feed(&mut v, &mut d, &reg, &[down(c), up(c)]);
    assert!(
        a.iter()
            .any(|x| matches!(x, UiAction::Command(Command::Batch(_))))
    );
    assert_eq!(clips(&d), n + 1);
    assert_eq!(d.undo_depth(), depth + 1);
    d.undo(&reg).unwrap();
    assert_eq!(clips(&d), n);
}

#[test]
fn to_slicer_asks_the_app_and_needs_markers() {
    let (mut d, reg, mut v) = setup();
    slice_mode(&mut v, &mut d, &reg);
    let c = v.to_slicer_button_center();
    let a = feed(&mut v, &mut d, &reg, &[down(c), up(c)]);
    assert!(!a.contains(&UiAction::ClipToSlicer(CLIP)));
    let w = v.wave_rect();
    let y = mid_y(&v);
    feed(&mut v, &mut d, &reg, &dbl(Point::new(w.x0 + 200.0, y)));
    v.sync(d.doc(), &reg);
    let a = feed(&mut v, &mut d, &reg, &[down(c), up(c)]);
    assert!(a.contains(&UiAction::ClipToSlicer(CLIP)));
}

#[test]
fn arrows_open_the_neighbouring_audio_clips_and_escape_closes() {
    let (mut d, reg, mut v) = setup();
    let a = feed(&mut v, &mut d, &reg, &[press(Key::Down, Modifiers::NONE)]);
    assert_eq!(a, vec![UiAction::OpenAudioEditor(ClipId(11))]);
    let a = feed(&mut v, &mut d, &reg, &[press(Key::Up, Modifiers::NONE)]);
    assert!(a.is_empty(), "the first clip has no previous one");
    let a = feed(&mut v, &mut d, &reg, &[press(Key::Escape, Modifiers::NONE)]);
    assert_eq!(a, vec![UiAction::CloseAudioEditor]);
}

#[test]
fn double_clicking_the_title_renames_the_asset() {
    let (mut d, reg, mut v) = setup();
    let c = v.title_center();
    feed(&mut v, &mut d, &reg, &dbl(c));
    assert!(v.is_text_owner());
    let mut events = Vec::new();
    for _ in 0..20 {
        events.push(press(Key::Backspace, Modifiers::NONE));
    }
    for ch in "Loop".chars() {
        events.push(press(Key::Char(ch), Modifiers::NONE));
    }
    events.push(press(Key::Enter, Modifiers::NONE));
    feed(&mut v, &mut d, &reg, &events);
    assert!(!v.is_text_owner());
    assert_eq!(d.doc().assets[HASH_A].original_name, "Loop");
}

#[test]
fn a_deleted_clip_makes_the_view_not_present() {
    let (d, reg, mut v) = setup();
    let mut doc: RackDoc = d.doc().clone();
    doc.song.tracks[1].lanes[0].clips.retain(|c| c.id != CLIP);
    v.sync(&doc, &reg);
    assert!(!v.is_present());
}

#[test]
fn painting_shows_the_title_the_modes_and_the_marker_count() {
    let (mut d, reg, mut v) = setup();
    with_marker(&mut v, &mut d, &reg);
    let mut dl = atomik_ui::DisplayList::new();
    v.paint(&mut dl, d.doc(), &reg);
    let texts: Vec<String> = dl.texts().into_iter().map(str::to_string).collect();
    for want in ["AUDIO CLIP", "OVERVIEW", "SLICE", "REVERT", "SPLIT"] {
        assert!(texts.iter().any(|t| t == want), "{want}: {texts:?}");
    }
    assert!(texts.iter().any(|t| t.contains("1 marker")), "{texts:?}");
    assert!(texts.iter().any(|t| t.contains("drums.wav")), "{texts:?}");
}

#[test]
fn the_arrangement_opens_an_audio_clip_with_open_clip() {
    let (d, reg, _) = setup();
    let mut arr = audio_arrangement(d.doc(), &reg);
    arr.set_selected_clips(&[CLIP]);
    let out = arr.command("sequencer.open_clip", None, d.doc(), &reg);
    assert!(
        matches!(out, CommandOutcome::Handled(ref a) if a == &[UiAction::OpenAudioEditor(CLIP)]),
        "{out:?}"
    );
}

// ------------------------------------------------------------------ transients and the speaker

/// The first source frame of clip 10 in its asset.
const OFFSET: u64 = RATE as u64;

fn detect_requests(actions: &[UiAction]) -> Vec<(ClipId, u8)> {
    actions
        .iter()
        .filter_map(|a| match a {
            UiAction::DetectTransients { clip, sensitivity } => Some((*clip, *sensitivity)),
            _ => None,
        })
        .collect()
}

/// The x of the asset frame `frame` of clip 10 once warp is on.
fn x_of_frame(v: &AudioEditorView, d: &Document, frame: u64) -> f64 {
    let tick = warp_of(d).ticks_at_source(RATE, (frame - OFFSET) as f64);
    v.tick_x(tick.round() as i64)
}

#[test]
fn slice_mode_and_the_sensitivity_buttons_ask_for_the_transients() {
    let (mut d, reg, mut v) = setup();
    let p = v.mode_button_center(AudioMode::Slice);
    let a = feed(&mut v, &mut d, &reg, &[down(p), up(p)]);
    assert_eq!(detect_requests(&a), vec![(CLIP, 50)]);
    v.sync(d.doc(), &reg);
    let more = v.sensitivity_up_center();
    let a = feed(&mut v, &mut d, &reg, &[down(more), up(more)]);
    assert_eq!(detect_requests(&a), vec![(CLIP, 60)]);
    assert_eq!(v.sensitivity(), 60);
    let less = v.sensitivity_down_center();
    let a = feed(
        &mut v,
        &mut d,
        &reg,
        &[down(less), up(less), down(less), up(less)],
    );
    assert_eq!(detect_requests(&a), vec![(CLIP, 50), (CLIP, 40)]);
    // At the ends the buttons do nothing.
    v.set_sensitivity(100);
    let a = feed(&mut v, &mut d, &reg, &[down(more), up(more)]);
    assert!(detect_requests(&a).is_empty());
    // Overview mode asks for nothing.
    let ov = v.mode_button_center(AudioMode::Overview);
    let a = feed(&mut v, &mut d, &reg, &[down(ov), up(ov)]);
    assert!(detect_requests(&a).is_empty());
}

#[test]
fn a_double_click_near_a_transient_snaps_the_marker_to_it() {
    let (mut d, reg, mut v) = setup();
    slice_mode(&mut v, &mut d, &reg);
    let frame = OFFSET + 30_011;
    v.set_transients(vec![frame, OFFSET + 70_000]);
    let x = x_of_frame(&v, &d, frame);
    let depth = d.undo_depth();
    let y = mid_y(&v);
    feed(&mut v, &mut d, &reg, &dbl(Point::new(x + 3.0, y)));
    assert_eq!(d.undo_depth(), depth + 1);
    let m = markers_of(&d);
    assert!(
        m.iter().any(|m| m.source == 30_011),
        "the marker is on the transient frame: {m:?}"
    );
    // Far from a transient the marker lands where the pointer is, not on a transient.
    v.sync(d.doc(), &reg);
    let far = x_of_frame(&v, &d, OFFSET + 50_000);
    feed(&mut v, &mut d, &reg, &dbl(Point::new(far, y)));
    let m = markers_of(&d);
    assert_eq!(m.len(), 4, "{m:?}");
}

#[test]
fn add_all_puts_a_marker_on_every_transient_in_one_undo_step() {
    let (mut d, reg, mut v) = setup();
    slice_mode(&mut v, &mut d, &reg);
    let c = v.add_all_button_center();
    // Nothing detected yet: a refusal, no command.
    let a = feed(&mut v, &mut d, &reg, &[down(c), up(c)]);
    assert!(commands(&a).is_empty());
    assert!(v.status().contains("No transients"), "{}", v.status());
    v.set_transients(vec![OFFSET + 10_000, OFFSET + 20_000, OFFSET + 40_000]);
    let depth = d.undo_depth();
    let a = feed(&mut v, &mut d, &reg, &[down(c), up(c)]);
    assert_eq!(commands(&a).len(), 1);
    assert_eq!(d.undo_depth(), depth + 1);
    let m = markers_of(&d);
    for want in [10_000, 20_000, 40_000] {
        assert!(m.iter().any(|m| m.source == want), "{want}: {m:?}");
    }
    // The command form is the same; with every transient marked it says so.
    v.sync(d.doc(), &reg);
    let out = v.command("editor.audio_add_transients", None, d.doc(), &reg);
    assert!(
        matches!(out, CommandOutcome::Handled(ref a) if a.is_empty()),
        "{out:?}"
    );
    assert!(v.status().contains("already"), "{}", v.status());
    d.undo(&reg).unwrap();
    assert!(
        markers_of(&d)
            .iter()
            .all(|m| ![10_000, 20_000, 40_000].contains(&m.source))
    );
}

#[test]
fn the_speaker_plays_from_the_press_to_the_end_of_the_clip_and_stops_on_release() {
    let (mut d, reg, mut v) = setup();
    let s = v.speaker_button_center();
    feed(&mut v, &mut d, &reg, &[down(s), up(s)]);
    assert!(v.speaker());
    let w = v.wave_rect();
    let p = Point::new(w.x0 + w.width() / 2.0, mid_y(&v));
    let a = feed(&mut v, &mut d, &reg, &[down(p)]);
    let [
        UiAction::AuditionSpan {
            asset,
            start_frame,
            end_frame,
            reverse,
            gain_db,
        },
    ] = &a[..]
    else {
        panic!("{a:?}")
    };
    assert_eq!(asset, HASH_A);
    assert!(!reverse);
    assert_eq!(*gain_db, -3.0);
    assert!(
        *start_frame > OFFSET && start_frame < end_frame,
        "{start_frame}..{end_frame}"
    );
    assert!(v.is_dragging());
    let a = feed(&mut v, &mut d, &reg, &[up(p)]);
    assert_eq!(a, vec![UiAction::StopAudition]);
    assert!(!v.is_dragging());
    // Nothing was edited, and a double-click does not add a marker while the speaker is on.
    v.set_mode(AudioMode::Slice);
    let depth = d.undo_depth();
    let a = feed(&mut v, &mut d, &reg, &dbl(p));
    assert!(commands(&a).is_empty());
    assert_eq!(d.undo_depth(), depth);
    // Escape during the audition stops it.
    feed(&mut v, &mut d, &reg, &[down(p)]);
    let a = feed(&mut v, &mut d, &reg, &[press(Key::Escape, Modifiers::NONE)]);
    assert_eq!(a, vec![UiAction::StopAudition]);
    // The toggle command switches it off again.
    assert!(matches!(
        v.command("tool.speaker", None, d.doc(), &reg),
        CommandOutcome::Handled(_)
    ));
    assert!(!v.speaker());
}

#[test]
fn the_toolbar_shows_the_transient_and_speaker_controls() {
    let (mut d, reg, mut v) = setup();
    slice_mode(&mut v, &mut d, &reg);
    let mut dl = atomik_ui::DisplayList::new();
    v.paint(&mut dl, d.doc(), &reg);
    let texts: Vec<String> = dl.texts().into_iter().map(str::to_string).collect();
    for want in ["SENS", "50", "ADD ALL", "SPEAKER"] {
        assert!(texts.iter().any(|t| t == want), "{want}: {texts:?}");
    }
}

#[test]
fn the_speaker_tool_in_the_arrangement_auditions_an_audio_clip_while_held() {
    let (d, reg, _) = setup();
    let mut arr = audio_arrangement(d.doc(), &reg);
    arr.set_tool(atomik_ui::Tool::Speaker);
    let r = arr.clip_rect(CLIP).expect("the clip is shown");
    let p = Point::new(r.x0 + r.width() / 2.0, r.y0 + r.height() / 2.0);
    let a = arr.handle(&down(p), d.doc(), &reg);
    let [
        UiAction::AuditionSpan {
            asset,
            start_frame,
            end_frame,
            reverse,
            ..
        },
    ] = &a[..]
    else {
        panic!("{a:?}")
    };
    assert_eq!(asset, HASH_A);
    assert!(!reverse && *start_frame > OFFSET && start_frame < end_frame);
    let a = arr.handle(&up(p), d.doc(), &reg);
    assert_eq!(a, vec![UiAction::StopAudition]);
    // A note clip and an empty lane do not audition, and nothing is selected, drawn or moved.
    let note = arr.clip_rect(ClipId(1)).expect("the note clip");
    let q = Point::new(note.x0 + 10.0, note.y0 + note.height() / 2.0);
    let a = arr.handle(&down(q), d.doc(), &reg);
    assert!(a.is_empty(), "{a:?}");
    assert!(arr.handle(&up(q), d.doc(), &reg).is_empty());
}
