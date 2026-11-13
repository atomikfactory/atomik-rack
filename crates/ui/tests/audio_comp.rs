//! The audio clip editor's Comp mode, the Keep formants controls and slicing a reversed clip,
//! through the public contract: every emitted command is applied to a real model `Document`.
//! Goldens: Comp mode with three takes and the Keep formants controls, in English and French
//! (`ATOMIK_UPDATE_GOLDEN=1` rewrites them).

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{
    AudioClipContent, ClipContent, ClipId, Command, CompSegment, Document, SongCommand, WarpMarker,
    WarpMode,
};
use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{AudioEditorView, AudioMode, Key as UiKey, Lang, Modifiers, UiAction, UiEvent};
use std::collections::BTreeSet;
use support::audio::*;
use support::song::{BAR, PPQ, press};
use support::starter::check_golden;
use support::*;

/// The first take (take ids follow the song's next clip id).
const TAKE0: ClipId = ClipId(12);
const START: i64 = 8 * BAR;

fn takes_doc() -> Document {
    let reg = registry();
    let mut d = Document::new(audio_doc());
    let take = |id: u64, asset: &str, offset: u64| {
        audio_clip(id, START, 2 * BAR, asset, offset, 0.0, (0, 0))
    };
    d.apply(
        Command::Song(SongCommand::NewTakeGroup {
            device: key("audio-track"),
            lane: 0,
            takes: vec![
                take(0, HASH_A, 0),
                take(0, HASH_A, 24_000),
                take(0, HASH_B, 0),
            ],
        }),
        &reg,
    )
    .unwrap();
    d
}

fn setup() -> (Document, Registry, AudioEditorView) {
    let reg = registry();
    let mut d = takes_doc();
    let mut v = AudioEditorView::new(atomik_ui::Theme::dark());
    v.set_bounds(Rect::new(0.0, 0.0, 1000.0, 520.0));
    v.set_peaks(HASH_A, pyramid(FRAMES_A, 2));
    v.set_peaks(HASH_B, pyramid(FRAMES_B, 1));
    v.open(TAKE0);
    v.sync(d.doc(), &reg);
    let p = v.mode_button_center(AudioMode::Comp);
    feed(&mut v, &mut d, &reg, &[down(p), up(p)]);
    (d, reg, v)
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
        v.sync(d.doc(), reg);
    }
    all
}

fn click(v: &mut AudioEditorView, d: &mut Document, reg: &Registry, p: Point) -> Vec<UiAction> {
    feed(v, d, reg, &[down(p), up(p)])
}

fn comp(d: &Document) -> Vec<CompSegment> {
    d.doc().song.take_groups().next().unwrap().2.comp.clone()
}

/// A drag on take `take`'s lane from tick `a` to tick `b` after the start of the group.
fn drag(
    v: &mut AudioEditorView,
    d: &mut Document,
    reg: &Registry,
    take: usize,
    (a, b): (i64, i64),
) -> Vec<UiAction> {
    let y = v.comp_lane_center(take).unwrap().y;
    let (xa, xb) = (v.tick_x(a), v.tick_x(b));
    feed(
        v,
        d,
        reg,
        &[
            down(Point::new(xa, y)),
            mv(Point::new((xa + xb) / 2.0, y)),
            mv(Point::new(xb, y)),
            up(Point::new(xb, y)),
        ],
    )
}

fn range_cmds(actions: &[UiAction]) -> Vec<(usize, i64, i64, u64)> {
    actions
        .iter()
        .filter_map(|a| match a {
            UiAction::Command(Command::Song(SongCommand::SetCompRange {
                take,
                start,
                end,
                xfade,
                ..
            })) => Some((*take, *start, *end, *xfade)),
            _ => None,
        })
        .collect()
}

#[test]
fn comp_mode_is_available_and_lists_the_takes() {
    assert!(AudioMode::Comp.is_enabled());
    let (_, _, v) = setup();
    assert_eq!(v.mode(), AudioMode::Comp);
    assert_eq!(v.comp_takes(), 3);
    assert!(v.comp_ready());
    assert_eq!(v.comp_xfade_ms(), 10);
    // The newest take is on top.
    assert!(v.comp_lane_center(2).unwrap().y < v.comp_lane_center(0).unwrap().y);
}

#[test]
fn a_click_on_a_take_auditions_it_and_changes_nothing() {
    let (mut d, reg, mut v) = setup();
    let before = comp(&d);
    let p = v.comp_lane_center(1).unwrap();
    let acts = feed(&mut v, &mut d, &reg, &[down(p)]);
    assert!(
        acts.iter()
            .any(|a| matches!(a, UiAction::AuditionSpan { asset, .. } if asset == HASH_A)),
        "{acts:?}"
    );
    let acts = feed(&mut v, &mut d, &reg, &[up(p)]);
    assert!(acts.contains(&UiAction::StopAudition));
    assert!(range_cmds(&acts).is_empty());
    assert_eq!(comp(&d), before);
    assert_eq!(v.comp_take(), Some(1));
}

#[test]
fn a_drag_on_a_take_puts_that_range_in_the_comp_with_a_crossfade_in_one_step() {
    let (mut d, reg, mut v) = setup();
    let depth = d.undo_depth();
    let acts = drag(&mut v, &mut d, &reg, 1, (BAR / 2, BAR));
    let cmds = range_cmds(&acts);
    assert_eq!(cmds.len(), 1, "{acts:?}");
    let (take, start, end, xfade) = cmds[0];
    assert_eq!(take, 1);
    assert!((start - (START + BAR / 2)).abs() <= 8, "{start}");
    assert!((end - (START + BAR)).abs() <= 8, "{end}");
    // 10 ms at 120 BPM is a little over 300 ticks (15360 to the beat).
    assert!((300..=315).contains(&xfade), "{xfade}");
    assert!(acts.contains(&UiAction::StopAudition));
    assert_eq!(d.undo_depth(), depth + 1, "one undo step");
    let c = comp(&d);
    assert!(
        c.iter()
            .any(|s| s.take == 1 && s.start == start && s.end == end)
    );
    assert_eq!(v.comp_selection(), Some((start, end)));
    // The cut where another take meets the new range crossfades.
    assert!(
        c.iter().any(|s| s.start == end && s.xfade == xfade),
        "{c:?}"
    );
}

#[test]
fn the_crossfade_length_is_settable() {
    let (mut d, reg, mut v) = setup();
    assert_eq!(v.comp_xfade_ms(), 10);
    let (down_c, up_c) = v.xfade_button_centers();
    click(&mut v, &mut d, &reg, up_c);
    assert_eq!(v.comp_xfade_ms(), 15);
    click(&mut v, &mut d, &reg, down_c);
    click(&mut v, &mut d, &reg, down_c);
    assert_eq!(v.comp_xfade_ms(), 5);
    let acts = drag(&mut v, &mut d, &reg, 0, (BAR / 4, BAR / 2));
    let (_, _, _, xfade) = range_cmds(&acts)[0];
    assert!((150..=158).contains(&xfade), "5 ms, not {xfade}");
}

#[test]
fn up_and_down_move_the_chosen_range_to_the_take_above_or_below() {
    let (mut d, reg, mut v) = setup();
    drag(&mut v, &mut d, &reg, 1, (BAR / 2, BAR));
    let (a, b) = v.comp_selection().unwrap();
    let depth = d.undo_depth();
    let acts = feed(&mut v, &mut d, &reg, &[press(UiKey::Up, Modifiers::NONE)]);
    let cmds = range_cmds(&acts);
    assert_eq!(cmds.len(), 1);
    assert_eq!((cmds[0].0, cmds[0].1, cmds[0].2), (2, a, b));
    assert_eq!(v.comp_take(), Some(2));
    assert_eq!(d.undo_depth(), depth + 1);
    // It joins the stretches of take 2 that touch it.
    assert!(
        comp(&d)
            .iter()
            .any(|s| s.take == 2 && s.start <= a && s.end >= b)
    );
    // At the top it stays; Down twice reaches the bottom take.
    let acts = feed(&mut v, &mut d, &reg, &[press(UiKey::Up, Modifiers::NONE)]);
    assert!(range_cmds(&acts).is_empty());
    feed(&mut v, &mut d, &reg, &[press(UiKey::Down, Modifiers::NONE)]);
    let acts = feed(&mut v, &mut d, &reg, &[press(UiKey::Down, Modifiers::NONE)]);
    assert_eq!(range_cmds(&acts)[0].0, 0);
    assert_eq!(v.comp_take(), Some(0));
}

#[test]
fn without_a_range_the_keys_move_the_selected_lane_only() {
    let (mut d, reg, mut v) = setup();
    let before = comp(&d);
    let acts = feed(&mut v, &mut d, &reg, &[press(UiKey::Up, Modifiers::NONE)]);
    assert!(acts.is_empty());
    assert_eq!(v.comp_take(), Some(1));
    assert_eq!(comp(&d), before);
}

#[test]
fn the_comp_lane_selects_the_stretch_that_plays_there() {
    let (mut d, reg, mut v) = setup();
    drag(&mut v, &mut d, &reg, 2, (BAR / 2, BAR));
    let (a, b) = comp(&d)
        .iter()
        .find(|s| s.take == 2)
        .map(|s| (s.start, s.end))
        .unwrap();
    let y = v.comp_top_center().y;
    let x = v.tick_x((a + b) / 2 - START);
    feed(
        &mut v,
        &mut d,
        &reg,
        &[down(Point::new(x, y)), up(Point::new(x, y))],
    );
    assert_eq!(v.comp_take(), Some(2));
    assert_eq!(v.comp_selection(), Some((a, b)));
}

#[test]
fn flatten_comp_asks_the_app_to_render_the_group() {
    let (mut d, reg, mut v) = setup();
    let p = v.flatten_comp_button_center();
    let acts = click(&mut v, &mut d, &reg, p);
    let group = d.doc().song.take_groups().next().unwrap().2.id;
    assert!(acts.contains(&UiAction::FlattenComp(group)), "{acts:?}");
}

#[test]
fn escape_during_a_drag_chooses_nothing() {
    let (mut d, reg, mut v) = setup();
    let before = comp(&d);
    let y = v.comp_lane_center(0).unwrap().y;
    let (xa, xb) = (v.tick_x(BAR / 4), v.tick_x(BAR));
    feed(
        &mut v,
        &mut d,
        &reg,
        &[down(Point::new(xa, y)), mv(Point::new(xb, y))],
    );
    let acts = feed(
        &mut v,
        &mut d,
        &reg,
        &[press(UiKey::Escape, Modifiers::NONE)],
    );
    assert!(acts.contains(&UiAction::StopAudition));
    assert!(range_cmds(&acts).is_empty());
    feed(&mut v, &mut d, &reg, &[up(Point::new(xb, y))]);
    assert_eq!(comp(&d), before);
    assert_eq!(v.comp_selection(), None);
}

#[test]
fn a_clip_without_takes_says_so() {
    let reg = registry();
    let mut d = Document::new(audio_doc());
    let mut v = AudioEditorView::new(atomik_ui::Theme::dark());
    v.set_bounds(Rect::new(0.0, 0.0, 1000.0, 520.0));
    v.open(ClipId(10));
    v.sync(d.doc(), &reg);
    let p = v.mode_button_center(AudioMode::Comp);
    feed(&mut v, &mut d, &reg, &[down(p), up(p)]);
    assert_eq!(v.mode(), AudioMode::Comp);
    assert!(!v.comp_ready());
    let mut dl = atomik_ui::DisplayList::new();
    v.paint(&mut dl, d.doc(), &reg);
    assert!(dl.texts().iter().any(|t| t.contains("no takes")));
}

// ---------------------------------------------------------------- Keep formants

fn tonal_setup() -> (Document, Registry, AudioEditorView) {
    let reg = registry();
    let mut d = Document::new(audio_doc());
    let clip = d.doc().song.clip(ClipId(10)).unwrap().2.clone();
    let ClipContent::Audio(a) = &clip.content else {
        unreachable!()
    };
    let mut content = a.clone();
    content.warp.enabled = true;
    content.warp.mode = WarpMode::Tonal;
    content.warp.transpose_cents = 300;
    d.apply(
        Command::Song(SongCommand::SetAudioClip {
            id: clip.id,
            start: clip.start,
            length: clip.length,
            muted: clip.muted,
            content,
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    let mut v = AudioEditorView::new(atomik_ui::Theme::dark());
    v.set_bounds(Rect::new(0.0, 0.0, 1000.0, 500.0));
    v.set_peaks(HASH_A, pyramid(FRAMES_A, 2));
    v.open(ClipId(10));
    v.sync(d.doc(), &reg);
    (d, reg, v)
}

fn warp_of(d: &Document) -> atomik_model::WarpSettings {
    match &d.doc().song.clip(ClipId(10)).unwrap().2.content {
        ClipContent::Audio(a) => a.warp.clone(),
        _ => unreachable!(),
    }
}

#[test]
fn keep_formants_the_shift_and_the_stereo_link_are_one_undo_step_each() {
    let (mut d, reg, mut v) = tonal_setup();
    assert!(!warp_of(&d).keep_formants);
    // The shift does nothing while the formants are not kept.
    let p = v.formant_up_button_center();
    let acts = click(&mut v, &mut d, &reg, p);
    assert!(acts.is_empty());
    let depth = d.undo_depth();
    let p = v.keep_formants_button_center();
    click(&mut v, &mut d, &reg, p);
    assert!(warp_of(&d).keep_formants);
    assert_eq!(d.undo_depth(), depth + 1);
    let p = v.formant_up_button_center();
    click(&mut v, &mut d, &reg, p);
    click(&mut v, &mut d, &reg, p);
    assert_eq!(warp_of(&d).formant_cents, 200);
    assert_eq!(v.formant_semitones(), 2);
    let p = v.formant_down_button_center();
    for _ in 0..20 {
        click(&mut v, &mut d, &reg, p);
    }
    assert_eq!(warp_of(&d).formant_cents, -1200, "limited to an octave");
    let p = v.stereo_link_button_center();
    click(&mut v, &mut d, &reg, p);
    assert!(warp_of(&d).link_stereo);
    d.undo().unwrap();
    assert!(!warp_of(&d).link_stereo);
}

#[test]
fn the_formant_controls_need_a_tonal_clip() {
    let reg = registry();
    let mut d = Document::new(audio_doc());
    let mut v = AudioEditorView::new(atomik_ui::Theme::dark());
    v.set_bounds(Rect::new(0.0, 0.0, 1000.0, 500.0));
    v.open(ClipId(10));
    v.sync(d.doc(), &reg);
    let p = v.keep_formants_button_center();
    let acts = click(&mut v, &mut d, &reg, p);
    assert!(acts.is_empty(), "a Beats clip has no formants to keep");
}

// ---------------------------------------------------------------- reversed clips

#[test]
fn a_reversed_clip_opens_in_slice_mode_and_goes_to_the_slicer() {
    let reg = registry();
    let mut d = Document::new(audio_doc());
    let clip = d.doc().song.clip(ClipId(10)).unwrap().2.clone();
    let ClipContent::Audio(a) = &clip.content else {
        unreachable!()
    };
    let mut content: AudioClipContent = a.clone();
    content.reverse = true;
    content.warp.enabled = true;
    let spt = 60.0 * 48_000.0 / (120.0 * PPQ as f64);
    content.warp.markers = vec![WarpMarker {
        tick: 1000,
        source: (1000.0 * spt).round() as u64,
    }];
    d.apply(
        Command::Song(SongCommand::SetAudioClip {
            id: clip.id,
            start: clip.start,
            length: clip.length,
            muted: clip.muted,
            content,
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    let depth = d.undo_depth();
    let mut v = AudioEditorView::new(atomik_ui::Theme::dark());
    v.set_bounds(Rect::new(0.0, 0.0, 1000.0, 500.0));
    v.set_peaks(HASH_A, pyramid(FRAMES_A, 2));
    v.open(ClipId(10));
    v.sync(d.doc(), &reg);
    // The markers are drawn where the reversed audio plays them: turned around the clip length.
    let m = v.markers().to_vec();
    assert_eq!(m.len(), 3);
    assert_eq!(m[1].tick, clip.length - 1000);
    assert!(m.windows(2).all(|w| w[0].tick < w[1].tick));
    let p = v.mode_button_center(AudioMode::Slice);
    let acts = click(&mut v, &mut d, &reg, p);
    assert_eq!(v.mode(), AudioMode::Slice);
    assert!(
        acts.iter().all(|a| !matches!(a, UiAction::Command(_))),
        "no warp is switched on"
    );
    assert_eq!(d.undo_depth(), depth);
    assert!(v.status().contains("reversed"));
    let p = v.to_slicer_button_center();
    let acts = click(&mut v, &mut d, &reg, p);
    assert!(
        acts.contains(&UiAction::ClipToSlicer(ClipId(10))),
        "{acts:?}"
    );
}

// ---------------------------------------------------------------- goldens

fn paint(v: &mut AudioEditorView, d: &Document, reg: &Registry) -> atomik_ui::DisplayList {
    let mut dl = atomik_ui::DisplayList::new();
    v.paint(&mut dl, d.doc(), reg);
    dl
}

/// Texts the editor draws that are names or values, not words of the program.
fn is_name_or_value(text: &str) -> bool {
    text.contains('+')
        || text.chars().any(|c| c.is_ascii_digit())
        || text.chars().filter(|c| c.is_alphabetic()).count() <= 2
        || text.ends_with(".wav")
        || text.ends_with(".flac")
        || text == "Guitar"
}

fn french(name: &str, make: impl FnOnce() -> atomik_ui::DisplayList) {
    let (list, misses) =
        atomik_ui::i18n::with_misses(|| atomik_ui::i18n::with_language(Lang::French, make));
    check_golden(&format!("fr/{name}"), &list.to_json());
    let missing: BTreeSet<&String> = misses.iter().filter(|m| !is_name_or_value(m)).collect();
    assert!(
        missing.is_empty(),
        "{name}: no French entry for:\n{missing:?}"
    );
}

fn comp_picture() -> atomik_ui::DisplayList {
    let (mut d, reg, mut v) = setup();
    drag(&mut v, &mut d, &reg, 1, (BAR / 2, BAR));
    drag(&mut v, &mut d, &reg, 2, (BAR, BAR + 2 * BAR / 5));
    paint(&mut v, &d, &reg)
}

fn formants_picture() -> atomik_ui::DisplayList {
    let (mut d, reg, mut v) = tonal_setup();
    let p = v.keep_formants_button_center();
    click(&mut v, &mut d, &reg, p);
    let p = v.formant_up_button_center();
    click(&mut v, &mut d, &reg, p);
    click(&mut v, &mut d, &reg, p);
    let p = v.stereo_link_button_center();
    click(&mut v, &mut d, &reg, p);
    paint(&mut v, &d, &reg)
}

#[test]
fn comp_mode_golden() {
    check_golden("audio_editor_comp_1x.json", &comp_picture().to_json());
}

#[test]
fn comp_mode_golden_in_french() {
    french("audio_editor_comp_1x.json", comp_picture);
}

#[test]
fn keep_formants_golden() {
    check_golden(
        "audio_editor_formants_1x.json",
        &formants_picture().to_json(),
    );
}

#[test]
fn keep_formants_golden_in_french() {
    french("audio_editor_formants_1x.json", formants_picture);
}
