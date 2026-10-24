//! `Shell` audio editing tests: the audio items of the Edit and Audio menus, their command ids,
//! Reverse, Normalise and the Strip Silence panel (opening, settings kept between openings,
//! APPLY as one undo step, Esc).

use super::*;
use crate::commands::{GainStep, MenuEntry};
use crate::session::NullBackend;
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_model::{ClipContent, ClipId, FadeCurve};
use atomik_ui::{ImportTarget, SilenceRow};

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
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1000.0, 800.0)));
    s
}

fn click(s: &mut Shell<NullBackend>, p: atomik_ui::kurbo::Point) {
    s.handle(&UiEvent::PointerDown {
        pos: p,
        button: atomik_ui::PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
    s.handle(&UiEvent::PointerUp {
        pos: p,
        button: atomik_ui::PointerButton::Primary,
        modifiers: Modifiers::NONE,
    });
}

/// Three bursts of 0.5 in silence: 48 000 frames, mono.
fn imported(s: &mut Shell<NullBackend>) -> ClipId {
    let dir = std::env::temp_dir().join(format!("atomik-shell-audio-edit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let data: Vec<f32> = (0..48_000)
        .map(|i| {
            let loud = (1_000..9_000).contains(&i)
                || (20_000..30_000).contains(&i)
                || (40_000..46_000).contains(&i);
            if loud { 0.25 } else { 0.0 }
        })
        .collect();
    let path = dir.join("bursts.wav");
    write_wav_f32(&path, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    s.core_mut()
        .import_audio(vec![path], ImportTarget::NewTracks, 0);
    s.core_mut().wait_background();
    let _ = std::fs::remove_dir_all(&dir);
    s.core()
        .doc()
        .song
        .tracks
        .iter()
        .flat_map(|t| &t.lanes)
        .flat_map(|l| &l.clips)
        .find(|c| c.content.is_audio())
        .map(|c| c.id)
        .expect("an audio clip")
}

fn item(s: &Shell<NullBackend>, menu: &str, label: &str) -> crate::commands::MenuItem {
    s.menu_model()
        .iter()
        .find(|m| m.title == menu)
        .unwrap()
        .items()
        .find(|i| i.label == label)
        .cloned()
        .unwrap_or_else(|| panic!("{menu} > {label}"))
}

fn audio(s: &Shell<NullBackend>, id: ClipId) -> atomik_model::AudioClipContent {
    match &s.core().doc().song.clip(id).unwrap().2.content {
        ClipContent::Audio(a) => a.clone(),
        _ => panic!("not audio"),
    }
}

fn clip_count(s: &Shell<NullBackend>) -> usize {
    s.core()
        .doc()
        .song
        .tracks
        .iter()
        .flat_map(|t| &t.lanes)
        .map(|l| l.clips.len())
        .sum()
}

#[test]
fn the_audio_items_follow_the_selection_and_carry_their_registry_ids() {
    let mut s = shell();
    let id = imported(&mut s);
    s.core_mut().arrangement_mut().set_selected_clips(&[]);
    for (menu, label) in [
        ("Edit", "Reverse Audio"),
        ("Edit", "Normalise Audio"),
        ("Edit", "Strip Silence\u{2026}"),
        ("Audio", "Clip Gain: Up 1 dB"),
        ("Audio", "Fades: 5 ms Declick"),
        ("Audio", "Fade Shape: S-curve"),
        ("Audio", "Crossfade: Off"),
    ] {
        assert!(!item(&s, menu, label).enabled, "{label} needs a clip");
    }
    s.core_mut().arrangement_mut().set_selected_clips(&[id]);
    let reverse = item(&s, "Edit", "Reverse Audio");
    assert!(reverse.enabled);
    assert_eq!(reverse.command, AppCommand::AudioReverse);
    assert_eq!(
        reverse.command.command_id(),
        Some(("sequencer.audio_reverse", None))
    );
    assert_eq!(
        item(&s, "Audio", "Fade Shape: S-curve").command,
        AppCommand::AudioFadeShape(FadeCurve::SCurve)
    );
    assert_eq!(
        AppCommand::from_command("sequencer.audio_crossfade", Some("off")),
        Some(AppCommand::AudioCrossfade(None))
    );
    assert_eq!(
        AppCommand::from_command("sequencer.audio_gain", Some("down")),
        Some(AppCommand::AudioGain(GainStep::Down))
    );
    assert!(
        !item(&s, "Audio", "Clear Gain Envelope").enabled,
        "no envelope yet"
    );
}

#[test]
fn reverse_and_the_gain_steps_edit_the_selected_clip_and_undo() {
    let mut s = shell();
    let id = imported(&mut s);
    s.core_mut().arrangement_mut().set_selected_clips(&[id]);
    s.set_focus(Pane::Arrangement);
    let before = s.core().doc().clone();
    assert!(s.execute(AppCommand::AudioReverse).is_empty());
    assert!(audio(&s, id).reverse);
    assert!(s.execute(AppCommand::AudioGain(GainStep::Up)).is_empty());
    assert!((audio(&s, id).gain_db - 1.0).abs() < 1e-6);
    s.execute(AppCommand::AudioFadeShape(FadeCurve::Exponential));
    assert_eq!(audio(&s, id).fade_curve, FadeCurve::Exponential);
    s.execute(AppCommand::AudioCrossfade(None));
    assert_eq!(audio(&s, id).xfade, None);
    for _ in 0..4 {
        s.core_mut().undo();
    }
    assert_eq!(s.core().doc(), &before);
    // The menu item is checked by the selection: reversing again turns it back.
    s.execute(AppCommand::AudioReverse);
    s.execute(AppCommand::AudioReverse);
    assert!(!audio(&s, id).reverse);
}

#[test]
fn normalise_from_the_edit_menu_sets_the_gain() {
    let mut s = shell();
    let id = imported(&mut s);
    s.core_mut().arrangement_mut().set_selected_clips(&[id]);
    assert!(s.execute(AppCommand::AudioNormalise).is_empty());
    // The bursts peak at 0.25: +12.04 dB brings them to full scale.
    let db = audio(&s, id).gain_db;
    assert!((db - 12.0412).abs() < 1e-3, "{db}");
    s.core_mut().undo();
    assert_eq!(audio(&s, id).gain_db, 0.0);
}

#[test]
fn the_strip_silence_panel_opens_keeps_its_settings_and_applies_in_one_step() {
    let mut s = shell();
    let id = imported(&mut s);
    s.core_mut().arrangement_mut().set_selected_clips(&[id]);
    assert!(!s.silence_open());
    assert!(s.execute(AppCommand::AudioStripSilence).is_empty());
    assert!(s.silence_open());
    let view = s.silence_view().unwrap();
    // The selection feeds the panel; APPLY works.
    let apply = view.apply_rect().center();
    // The defaults keep 10 ms around each sound and cut silences from 100 ms: the 1000-frame
    // lead-in and the 2000-frame tail are shorter than 100 ms (4800 frames), the two gaps of
    // 11000 and 10000 frames are not.
    let before = s.core().doc().clone();
    click(&mut s, apply);
    assert_eq!(clip_count(&s), 3, "two silences cut out of one clip");
    assert!(s.silence_open(), "the panel stays for another go");
    s.core_mut().undo();
    assert_eq!(s.core().doc(), &before);
    // A changed value is kept for the next opening.
    let plus = s
        .silence_view()
        .unwrap()
        .plus_rect(SilenceRow::Threshold)
        .center();
    click(&mut s, plus);
    assert_eq!(s.silence_values().threshold_db, -39);
    s.handle(&UiEvent::Key {
        key: Key::Escape,
        modifiers: Modifiers::NONE,
        pressed: true,
    });
    assert!(!s.silence_open());
    s.execute(AppCommand::AudioStripSilence);
    assert_eq!(s.silence_view().unwrap().values().threshold_db, -39);
    // CLOSE closes it too.
    let close = s.silence_view().unwrap().close_rect().center();
    click(&mut s, close);
    assert!(!s.silence_open());
}

#[test]
fn the_panel_cannot_apply_without_an_audio_clip_and_paints() {
    let mut s = shell();
    let _ = imported(&mut s);
    s.core_mut().arrangement_mut().set_selected_clips(&[]);
    s.execute(AppCommand::AudioStripSilence);
    let apply = s.silence_view().unwrap().apply_rect().center();
    let before = s.core().doc().clone();
    click(&mut s, apply);
    assert_eq!(s.core().doc(), &before);
    let mut dl = atomik_ui::DisplayList::new();
    s.paint(&mut dl);
    assert!(dl.texts().contains(&"STRIP SILENCE"));
    let _ = MenuEntry::Separator;
}
