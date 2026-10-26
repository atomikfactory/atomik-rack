//! The clip inspector in the app core: the transport bar shows the arrangement's selection, and
//! an edit of several selected clips (a drag on Fade In, a typed transposition) is one undo step.

use super::*;
use crate::sink::RecordingSink;
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_model::{AudioClipContent, ClipContent, empty_doc};
use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{ImportTarget, InspectorField, Key, Modifiers, PointerButton};

fn core_with_two_clips() -> (AppCore<RecordingSink>, Vec<ClipId>) {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "atomik-app-inspector-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let paths: Vec<PathBuf> = ["a.wav", "b.wav"]
        .iter()
        .map(|n| {
            let path = dir.join(n);
            let data: Vec<f32> = (0..96_000).map(|i| (i % 100) as f32 / 200.0).collect();
            write_wav_f32(&path, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
            path
        })
        .collect();
    let mut c = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    c.sink_mut().take();
    c.import_audio(paths, ImportTarget::NewTracks, 0);
    c.wait_background();
    let _ = std::fs::remove_dir_all(&dir);
    let ids: Vec<ClipId> = c
        .doc()
        .song
        .tracks
        .iter()
        .flat_map(|t| t.lanes.iter().flat_map(|l| l.clips.iter().map(|c| c.id)))
        .collect();
    assert_eq!(ids.len(), 2);
    c.transport_bar_mut()
        .set_bounds(Rect::new(0.0, 0.0, 1600.0, 44.0));
    (c, ids)
}

fn audio(c: &AppCore<RecordingSink>, id: ClipId) -> AudioClipContent {
    match &c.doc().song.clip(id).unwrap().2.content {
        ClipContent::Audio(a) => a.clone(),
        _ => panic!("not audio"),
    }
}

fn pointer(c: &mut AppCore<RecordingSink>, event: UiEvent) {
    c.handle_pane_event(Pane::Transport, &event);
}

fn down(p: Point, clicks: u8) -> UiEvent {
    UiEvent::PointerDown {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks,
    }
}

fn up(p: Point) -> UiEvent {
    UiEvent::PointerUp {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    }
}

fn mv(p: Point) -> UiEvent {
    UiEvent::PointerMove {
        pos: p,
        modifiers: Modifiers::NONE,
    }
}

fn key(k: Key) -> UiEvent {
    UiEvent::Key {
        key: k,
        modifiers: Modifiers::NONE,
        pressed: true,
    }
}

#[test]
fn the_inspector_follows_the_arrangement_selection() {
    let (mut c, ids) = core_with_two_clips();
    c.arrangement_mut().set_selected_clips(&[]);
    c.after_view_change();
    assert!(c.transport_bar().inspector_fields().is_empty());
    c.arrangement_mut().set_selected_clips(&ids[..1]);
    c.after_view_change();
    assert_eq!(c.transport_bar().inspector_fields().len(), 6);
}

#[test]
fn a_fade_drag_on_two_clips_is_one_undo_step() {
    let (mut c, ids) = core_with_two_clips();
    c.arrangement_mut().set_selected_clips(&ids);
    c.after_view_change();
    let before = c.doc().clone();
    let p = c
        .transport_bar()
        .inspector_point(InspectorField::FadeIn)
        .unwrap();
    pointer(&mut c, down(p, 1));
    // Several moves, as a real drag: 8 steps of 10 ms in all.
    for dy in [5.0, 15.0, 25.0, 40.0] {
        pointer(&mut c, mv(Point::new(p.x, p.y - dy)));
    }
    pointer(&mut c, up(Point::new(p.x, p.y - 40.0)));
    for id in &ids {
        assert_eq!(audio(&c, *id).fade_in, 80 * 48, "{id:?}");
    }
    assert!(c.undo());
    assert_eq!(c.doc(), &before, "one undo reverts both clips");
}

#[test]
fn a_typed_transposition_on_two_clips_is_one_undo_step() {
    let (mut c, ids) = core_with_two_clips();
    c.arrangement_mut().set_selected_clips(&ids);
    c.after_view_change();
    let before = c.doc().clone();
    let p = c
        .transport_bar()
        .inspector_point(InspectorField::TransposeWhole)
        .unwrap();
    pointer(&mut c, down(p, 1));
    pointer(&mut c, up(p));
    pointer(&mut c, down(p, 2));
    pointer(&mut c, up(p));
    assert!(c.transport_bar().is_editing_inspector());
    for ch in "3.5".chars() {
        pointer(&mut c, key(Key::Char(ch)));
    }
    pointer(&mut c, key(Key::Enter));
    for id in &ids {
        assert_eq!(audio(&c, *id).warp.transpose_cents, 350);
    }
    assert!(c.undo());
    assert_eq!(c.doc(), &before);
}

#[test]
fn undo_updates_what_the_inspector_shows() {
    let (mut c, ids) = core_with_two_clips();
    c.arrangement_mut().set_selected_clips(&ids);
    c.after_view_change();
    let p = c
        .transport_bar()
        .inspector_point(InspectorField::Reverse)
        .unwrap();
    pointer(&mut c, down(p, 1));
    pointer(&mut c, up(p));
    assert!(ids.iter().all(|id| audio(&c, *id).reverse));
    // Transposition is refused now (reversed clips), with a reason.
    let t = c
        .transport_bar()
        .inspector_point(InspectorField::TransposeWhole)
        .unwrap();
    let help = c.help_in_pane(Pane::Transport, t).unwrap();
    assert!(help.text.contains("reversed"), "{}", help.text);
    assert!(c.undo());
    c.after_view_change();
    let help = c.help_in_pane(Pane::Transport, t).unwrap();
    assert!(!help.text.contains("reversed"), "{}", help.text);
}
