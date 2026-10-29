//! The clip inspector through the shell: a real drag on Fade In in the transport bar changes the
//! selected clips, and the keys go to the typed field.

use super::*;
use crate::session::NullBackend;
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_model::{AudioClipContent, ClipContent, ClipId};
use atomik_ui::kurbo::Point;
use atomik_ui::{ImportTarget, InspectorField};

fn shell() -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(atomik_ui::keymap::Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: None,
            ui_state_path: None,
            library_path: None,
            library_root: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1600.0, 800.0)));
    s
}

fn imported(s: &mut Shell<NullBackend>) -> Vec<ClipId> {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "atomik-shell-inspector-{}-{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let paths: Vec<_> = ["a.wav", "b.wav"]
        .iter()
        .map(|n| {
            let path = dir.join(n);
            let data: Vec<f32> = (0..96_000).map(|i| (i % 100) as f32 / 200.0).collect();
            write_wav_f32(&path, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
            path
        })
        .collect();
    s.core_mut().import_audio(paths, ImportTarget::NewTracks, 0);
    s.core_mut().wait_background();
    let _ = std::fs::remove_dir_all(&dir);
    s.core()
        .doc()
        .song
        .tracks
        .iter()
        .flat_map(|t| t.lanes.iter().flat_map(|l| l.clips.iter().map(|c| c.id)))
        .collect()
}

fn audio(s: &Shell<NullBackend>, id: ClipId) -> AudioClipContent {
    match &s.core().doc().song.clip(id).unwrap().2.content {
        ClipContent::Audio(a) => a.clone(),
        _ => panic!("not audio"),
    }
}

fn down(s: &mut Shell<NullBackend>, p: Point, clicks: u8) {
    s.handle(&UiEvent::PointerDown {
        pos: p,
        button: atomik_ui::PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks,
    });
}

fn mv(s: &mut Shell<NullBackend>, p: Point) {
    s.handle(&UiEvent::PointerMove {
        pos: p,
        modifiers: Modifiers::NONE,
    });
}

fn up(s: &mut Shell<NullBackend>, p: Point) {
    s.handle(&UiEvent::PointerUp {
        pos: p,
        button: atomik_ui::PointerButton::Primary,
        modifiers: Modifiers::NONE,
    });
}

fn tap(s: &mut Shell<NullBackend>, key: Key) {
    for pressed in [true, false] {
        s.handle(&UiEvent::Key {
            key: key.clone(),
            modifiers: Modifiers::NONE,
            pressed,
        });
    }
}

#[test]
fn a_drag_on_fade_in_changes_every_selected_clip_and_undoes_in_one_step() {
    let mut s = shell();
    let ids = imported(&mut s);
    s.core_mut().arrangement_mut().set_selected_clips(&ids);
    s.core_mut().after_view_change();
    let before = s.core().doc().clone();
    let p = s
        .core()
        .transport_bar()
        .inspector_point(InspectorField::FadeIn)
        .expect("the inspector shows at this width");
    assert!(s.layout().transport.contains(p));
    down(&mut s, p, 1);
    for dy in [10.0, 20.0, 30.0] {
        mv(&mut s, Point::new(p.x, p.y - dy));
    }
    up(&mut s, Point::new(p.x, p.y - 30.0));
    for id in &ids {
        assert_eq!(audio(&s, *id).fade_in, 60 * 48, "{id:?}");
    }
    s.execute(AppCommand::Undo);
    assert_eq!(s.core().doc(), &before);
}

#[test]
fn keys_go_to_the_typed_inspector_field() {
    let mut s = shell();
    let ids = imported(&mut s);
    s.core_mut().arrangement_mut().set_selected_clips(&ids[..1]);
    s.core_mut().after_view_change();
    let p = s
        .core()
        .transport_bar()
        .inspector_point(InspectorField::FadeOut)
        .unwrap();
    down(&mut s, p, 1);
    up(&mut s, p);
    down(&mut s, p, 2);
    up(&mut s, p);
    assert!(s.core().transport_bar().is_editing_inspector());
    for c in ['2', '0', '0'] {
        tap(&mut s, Key::Char(c));
    }
    tap(&mut s, Key::Space);
    assert!(!s.core().is_playing(), "Space does not play while typing");
    tap(&mut s, Key::Enter);
    assert_eq!(audio(&s, ids[0]).fade_out, 200 * 48);
}

#[test]
fn the_inspector_follows_the_selection() {
    let mut s = shell();
    let ids = imported(&mut s);
    assert_eq!(ids.len(), 2);
    s.core_mut().arrangement_mut().set_selected_clips(&[]);
    s.core_mut().after_view_change();
    assert!(s.core().transport_bar().inspector_fields().is_empty());
    s.core_mut().arrangement_mut().set_selected_clips(&ids[1..]);
    s.core_mut().after_view_change();
    assert!(!s.core().transport_bar().inspector_fields().is_empty());
}

fn start_of(s: &Shell<NullBackend>, id: ClipId) -> i64 {
    s.core().doc().song.clip(id).unwrap().2.start
}

#[test]
fn the_arrange_inspector_toggles_edits_two_clips_in_one_undo_step_and_takes_the_keys() {
    use atomik_ui::ClipField;
    let mut s = shell();
    let ids = imported(&mut s);
    assert!(!s.core().arrangement().inspector_visible());
    assert!(!s.menu_state().inspector);
    s.execute(AppCommand::ToggleInspector);
    assert!(s.core().arrangement().inspector_visible());
    assert!(s.ui_state().show_inspector && s.menu_state().inspector);
    s.core_mut().arrangement_mut().set_selected_clips(&ids);
    s.core_mut().after_view_change();
    let before = s.core().doc().clone();
    let starts: Vec<i64> = ids.iter().map(|i| start_of(&s, *i)).collect();
    // Double-click the tick part of Position and type +240.
    let p = s
        .core()
        .arrangement()
        .inspector_part_point(ClipField::Position, 3)
        .unwrap();
    down(&mut s, p, 2);
    up(&mut s, p);
    assert!(s.core().arrangement().is_text_owner());
    for c in ['+', '2', '4', '0'] {
        tap(&mut s, Key::Char(c));
    }
    tap(&mut s, Key::Space);
    assert!(!s.core().is_playing(), "Space does not play while typing");
    tap(&mut s, Key::Enter);
    for (id, start) in ids.iter().zip(&starts) {
        assert_eq!(start_of(&s, *id), start + 240, "{id:?}");
    }
    s.execute(AppCommand::Undo);
    assert_eq!(s.core().doc(), &before, "one undo step for both clips");
    s.execute(AppCommand::ToggleInspector);
    assert!(!s.core().arrangement().inspector_visible() && !s.ui_state().show_inspector);
}

#[test]
fn the_note_strip_shows_with_the_same_switch_and_edits_two_notes_in_one_undo_step() {
    use atomik_core::PPQ;
    use atomik_model::{Clip, Command, DeviceKey, Note, SongCommand};
    use atomik_ui::NoteField;
    let mut s = shell();
    let dev = DeviceKey("a".into());
    s.core_mut()
        .apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".into(),
            key: Some(dev.clone()),
        })
        .unwrap();
    let song = |s: &mut Shell<NullBackend>, c: SongCommand| {
        s.core_mut().apply_command(Command::Song(c)).unwrap();
    };
    song(
        &mut s,
        SongCommand::AddTrack {
            device: dev.clone(),
        },
    );
    song(
        &mut s,
        SongCommand::AddClip {
            device: dev,
            lane: 0,
            clip: Clip {
                id: ClipId(0),
                start: 0,
                length: 4 * PPQ,
                offset: 0,
                loop_length: None,
                muted: false,
                scale: None,
                controllers: Vec::new(),
                grid: None,
                content: ClipContent::Notes(vec![
                    Note::new(0, PPQ, 60, 0.8),
                    Note::new(PPQ, PPQ, 64, 0.8),
                ]),
            },
        },
    );
    s.core_mut().open_piano_roll(ClipId(1));
    s.execute(AppCommand::ToggleInspector);
    assert!(s.core().piano_roll().inspector_visible());
    assert!(
        s.core().arrangement().inspector_visible(),
        "one switch for both strips"
    );
    s.core_mut().piano_roll_mut().set_selected_notes(&[0, 1]);
    s.core_mut().after_view_change();
    let before = s.core().doc().clone();
    let p = s
        .core()
        .piano_roll()
        .inspector_part_point(NoteField::Pitch, 0)
        .expect("the strip fits in the pane");
    down(&mut s, p, 2);
    up(&mut s, p);
    assert!(s.core().piano_roll().is_text_owner());
    for c in ['+', '2'] {
        tap(&mut s, Key::Char(c));
    }
    tap(&mut s, Key::Space);
    assert!(!s.core().is_playing(), "Space does not play while typing");
    tap(&mut s, Key::Enter);
    let keys: Vec<u8> = match &s.core().doc().song.clip(ClipId(1)).unwrap().2.content {
        ClipContent::Notes(n) => n.iter().map(|n| n.key).collect(),
        _ => panic!("not notes"),
    };
    assert_eq!(keys, vec![62, 66]);
    s.execute(AppCommand::Undo);
    assert_eq!(s.core().doc(), &before, "one undo step for both notes");
}
