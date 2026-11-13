//! The audio clip editor's Vocal mode through its public contract: the analysis request, the
//! selection, pitch and time drags, Correct, Flatten and Reset, and the picture (goldens). Every
//! emitted command is applied to a real model `Document`.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::harmony::Key;
use atomik_model::theory::ScaleKind;
use atomik_model::{ClipContent, ClipId, Command, Document, PitchEdit, SongCommand};
use atomik_ui::keymap::CommandOutcome;
use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{AudioEditorView, AudioMode, Key as UiKey, Modifiers, UiAction, UiEvent};
use support::audio::*;
use support::song::{down_mods, press};
use support::starter::check_golden;
use support::*;

const CLIP: ClipId = ClipId(10);

/// The editor on clip 10 in Vocal mode with the three notes of [`vocal_notes`] analysed.
fn setup() -> (Document, Registry, AudioEditorView) {
    let reg = registry();
    let mut doc = audio_doc();
    doc.song_mut().harmony.key = Some(Key::new(0, ScaleKind::Major));
    let mut d = Document::new(doc);
    let mut v = AudioEditorView::new(atomik_ui::Theme::dark());
    v.set_bounds(Rect::new(0.0, 0.0, 1000.0, 500.0));
    v.set_peaks(HASH_A, pyramid(FRAMES_A, 2));
    v.open(CLIP);
    v.sync(d.doc(), &reg);
    let p = v.mode_button_center(AudioMode::Vocal);
    let asked = feed(&mut v, &mut d, &reg, &[down(p), up(p)]);
    assert!(asked.contains(&UiAction::AnalyseVocal {
        clip: CLIP,
        sensitivity: 50
    }));
    v.set_vocal_notes(CLIP, vocal_notes());
    v.sync(d.doc(), &reg);
    (d, reg, v)
}

fn edits_of(d: &Document) -> Vec<PitchEdit> {
    match &d.doc().song.clip(CLIP).unwrap().2.content {
        ClipContent::Audio(a) => a.pitch_edits.clone(),
        _ => unreachable!(),
    }
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

fn command(v: &mut AudioEditorView, d: &mut Document, reg: &Registry, id: &str) -> Vec<UiAction> {
    match v.command(id, None, d.doc(), reg) {
        CommandOutcome::Handled(a) => {
            apply(d, reg, &a);
            v.sync(d.doc(), reg);
            a
        }
        other => panic!("{id}: {other:?}"),
    }
}

fn click(v: &mut AudioEditorView, d: &mut Document, reg: &Registry, p: Point) -> Vec<UiAction> {
    feed(v, d, reg, &[down(p), up(p)])
}

fn merges(actions: &[UiAction]) -> Vec<Option<u64>> {
    actions
        .iter()
        .filter_map(|a| match a {
            UiAction::Command(Command::Song(SongCommand::SetPitchEdits { merge, .. })) => {
                Some(*merge)
            }
            _ => None,
        })
        .collect()
}

#[test]
fn vocal_mode_is_available() {
    assert!(AudioMode::Vocal.is_enabled());
    let (_, _, v) = setup();
    assert_eq!(v.mode(), AudioMode::Vocal);
    assert!(v.vocal_ready());
    assert_eq!(v.vocal_notes().len(), 3);
}

#[test]
fn a_click_selects_shift_adds_and_a_band_selects() {
    let (mut d, reg, mut v) = setup();
    let a = v.vocal_note_center(0).unwrap();
    let b = v.vocal_note_center(1).unwrap();
    click(&mut v, &mut d, &reg, a);
    assert_eq!(v.vocal_selection(), vec![0]);
    let shift = Modifiers {
        shift: true,
        ..Modifiers::NONE
    };
    feed(&mut v, &mut d, &reg, &[down_mods(b, shift, 1), up(b)]);
    assert_eq!(v.vocal_selection(), vec![0, 1]);
    // A band over the empty grid around the last two notes.
    let w = v.wave_rect();
    let c = v.vocal_note_center(2).unwrap();
    let from = Point::new(b.x - 20.0, w.y0 + 2.0);
    let to = Point::new(c.x + 20.0, w.y1 - 2.0);
    feed(&mut v, &mut d, &reg, &[down(from), mv(to), up(to)]);
    assert_eq!(v.vocal_selection(), vec![1, 2]);
    // Select all.
    command(&mut v, &mut d, &reg, "edit.select_all");
    assert_eq!(v.vocal_selection(), vec![0, 1, 2]);
    // Esc clears the selection first.
    feed(
        &mut v,
        &mut d,
        &reg,
        &[press(UiKey::Escape, Modifiers::NONE)],
    );
    assert!(v.vocal_selection().is_empty());
}

#[test]
fn a_pitch_drag_moves_the_selection_in_semitones_in_one_undo_step() {
    let (mut d, reg, mut v) = setup();
    let before = d.doc().clone();
    let a = v.vocal_note_center(0).unwrap();
    let row = v.vocal_row_height();
    let mid = Point::new(a.x, a.y - 1.2 * row);
    let to = Point::new(a.x, a.y - 2.2 * row);
    let acts = feed(&mut v, &mut d, &reg, &[down(a), mv(mid), mv(to), up(to)]);
    let m = merges(&acts);
    assert!(!m.is_empty() && m.iter().all(|x| x.is_some() && *x == m[0]));
    assert!(acts.contains(&UiAction::EndMerge));
    let e = edits_of(&d);
    assert_eq!(e.len(), 1);
    assert_eq!(
        (e[0].src_start, e[0].src_end, e[0].shift_cents),
        (60_000, 100_000, 200)
    );
    d.undo().unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn the_alternate_tool_drags_in_cents_and_escape_restores() {
    let (mut d, reg, mut v) = setup();
    let a = v.vocal_note_center(1).unwrap();
    let row = v.vocal_row_height();
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
    let to = Point::new(a.x, a.y - 0.37 * row - 4.0);
    feed(&mut v, &mut d, &reg, &[down(a), mv_with(to, alt)]);
    let cents = edits_of(&d)[0].shift_cents;
    assert!(cents % 100 != 0 && cents > 0, "{cents}");
    let acts = feed(
        &mut v,
        &mut d,
        &reg,
        &[press(UiKey::Escape, Modifiers::NONE)],
    );
    assert!(acts.contains(&UiAction::EndMerge));
    assert!(edits_of(&d).is_empty());
}

#[test]
fn correct_brings_notes_to_the_semitone_or_the_scale_in_one_undo_step() {
    let (mut d, reg, mut v) = setup();
    command(&mut v, &mut d, &reg, "edit.select_all");
    command(&mut v, &mut d, &reg, "editor.vocal_correct");
    let shifts: Vec<i32> = edits_of(&d).iter().map(|e| e.shift_cents).collect();
    assert_eq!(shifts, vec![20, -30, -10]);
    d.undo().unwrap();
    assert!(edits_of(&d).is_empty());
    // With SCALE on and the song in C major, the sharp E stays an E and the D a D; a note
    // between the scale's notes goes to the nearest of them.
    v.set_vocal_notes(CLIP, vec![vocal_note(60_000, 100_000, 61.4, 0.0)]);
    v.sync(d.doc(), &reg);
    let at = v.scale_button_center();
    click(&mut v, &mut d, &reg, at);
    assert!(v.vocal_scale());
    command(&mut v, &mut d, &reg, "edit.select_all");
    command(&mut v, &mut d, &reg, "editor.vocal_correct");
    assert_eq!(edits_of(&d)[0].shift_cents, 60);
}

#[test]
fn flatten_and_reset_are_one_undo_step_each() {
    let (mut d, reg, mut v) = setup();
    // Nothing selected: Flatten says why.
    assert!(matches!(
        v.command("editor.vocal_flatten", None, d.doc(), &reg),
        CommandOutcome::NotApplicable(_)
    ));
    command(&mut v, &mut d, &reg, "edit.select_all");
    v.set_vocal_flatten(60);
    command(&mut v, &mut d, &reg, "editor.vocal_flatten");
    let e = edits_of(&d);
    assert_eq!(e.len(), 3);
    assert!(e.iter().all(|e| e.drift == 60 && e.shift_cents == 0));
    let flat = d.doc().clone();
    // Reset of one note, then of all.
    let at = v.vocal_note_center(1).unwrap();
    click(&mut v, &mut d, &reg, at);
    command(&mut v, &mut d, &reg, "editor.vocal_reset");
    assert_eq!(edits_of(&d).len(), 2);
    d.undo().unwrap();
    assert_eq!(d.doc(), &flat);
    feed(
        &mut v,
        &mut d,
        &reg,
        &[press(UiKey::Escape, Modifiers::NONE)],
    );
    command(&mut v, &mut d, &reg, "editor.vocal_reset");
    assert!(edits_of(&d).is_empty());
}

#[test]
fn a_time_drag_pins_markers_around_the_note_in_one_undo_step() {
    let (mut d, reg, mut v) = setup();
    let before = d.doc().clone();
    let a = v.vocal_note_center(1).unwrap();
    let to = Point::new(a.x + 40.0, a.y);
    let acts = feed(
        &mut v,
        &mut d,
        &reg,
        &[down(a), mv(Point::new(a.x + 10.0, a.y)), mv(to), up(to)],
    );
    assert!(acts.contains(&UiAction::EndMerge));
    let ClipContent::Audio(c) = &d.doc().song.clip(CLIP).unwrap().2.content else {
        unreachable!()
    };
    assert!(c.warp.enabled, "warp is switched on");
    // The two markers sit on the note's frames (from the clip's offset of 48 000).
    let sources: Vec<u64> = c.warp.markers.iter().map(|m| m.source).collect();
    assert!(
        sources.contains(&62_000) && sources.contains(&102_000),
        "{sources:?}"
    );
    d.undo().unwrap();
    assert_eq!(d.doc(), &before);
}

#[test]
fn the_footer_shows_the_progress_the_inspector_and_an_unpitched_clip() {
    let (mut d, reg, mut v) = setup();
    let texts = |v: &mut AudioEditorView, d: &Document| {
        let mut dl = atomik_ui::DisplayList::new();
        v.paint(&mut dl, d.doc(), &reg);
        dl.texts()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    let at = v.vocal_note_center(1).unwrap();
    click(&mut v, &mut d, &reg, at);
    let t = texts(&mut v, &d);
    assert!(
        t.iter().any(|x| x.contains("E3") && x.contains("+30 ct")),
        "{t:?}"
    );
    v.set_vocal_progress(Some(0.4));
    assert!(texts(&mut v, &d).iter().any(|x| x == "Finding the notes…"));
    let acts = feed(
        &mut v,
        &mut d,
        &reg,
        &[press(UiKey::Escape, Modifiers::NONE)],
    );
    assert!(acts.contains(&UiAction::CancelVocalAnalysis));
    v.set_vocal_notes(CLIP, Vec::new());
    assert!(
        texts(&mut v, &d)
            .iter()
            .any(|x| x == "No pitched notes found in this clip")
    );
}

#[test]
fn vocal_mode_golden() {
    let (mut d, reg, mut v) = setup();
    // One note corrected and selected, the scale on.
    let at = v.vocal_note_center(1).unwrap();
    click(&mut v, &mut d, &reg, at);
    command(&mut v, &mut d, &reg, "editor.vocal_correct");
    let at = v.scale_button_center();
    click(&mut v, &mut d, &reg, at);
    let mut dl = atomik_ui::DisplayList::new();
    v.paint(&mut dl, d.doc(), &reg);
    check_golden("audio_editor_vocal_1x.json", &dl.to_json());
}
