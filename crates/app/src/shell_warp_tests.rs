//! `Shell` warp tests: the warp and transpose items of the Edit and Audio menus and their command
//! ids, the commands from the menu, the warp panel (opening from the clip, APPLY, DETECT TEMPO,
//! Esc) and Warp Loop as Bars.

use super::*;
use crate::commands::TransposeStep;
use crate::session::NullBackend;
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_model::{ClipContent, ClipId, WarpMode};
use atomik_ui::{ImportTarget, WarpRow};
use std::sync::atomic::{AtomicU64, Ordering};

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

/// A 2 s mono file with a noise burst every 24 000 frames (a beat at 120 BPM), imported as a clip.
fn imported(s: &mut Shell<NullBackend>) -> ClipId {
    // The warp tests apply and undo warps themselves: the file is imported as is.
    s.core_mut()
        .set_warp_imports(crate::import_warp::WarpImports::Off);
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "atomik-shell-warp-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let data: Vec<f32> = (0..96_000usize)
        .map(|i| {
            let k = (i % 24_000) as u32;
            if k >= 2_000 {
                return 0.0;
            }
            let mut x = k.wrapping_mul(2_654_435_761).wrapping_add(99);
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            ((x >> 8) as f32 / 8_388_608.0 - 1.0) * 0.8 * (-(k as f32) / 400.0).exp()
        })
        .collect();
    let path = dir.join("beats.wav");
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

#[test]
fn the_warp_items_follow_the_selection_and_carry_their_registry_ids() {
    let mut s = shell();
    let id = imported(&mut s);
    s.core_mut().arrangement_mut().set_selected_clips(&[]);
    for (menu, label) in [
        ("Edit", "Quantize Audio"),
        ("Audio", "Warp Audio"),
        ("Audio", "Warp to Song Tempo"),
        ("Audio", "Warp Loop as 2 Bars"),
        ("Audio", "Warp Mode: Tonal"),
        ("Audio", "Transpose: Up a Semitone"),
        ("Audio", "Warp Settings\u{2026}"),
    ] {
        assert!(!item(&s, menu, label).enabled, "{label} needs a clip");
    }
    s.core_mut().arrangement_mut().set_selected_clips(&[id]);
    for (menu, label, command, ident) in [
        (
            "Edit",
            "Quantize Audio",
            AppCommand::AudioQuantize,
            ("sequencer.audio_quantize", None),
        ),
        (
            "Audio",
            "Warp Audio",
            AppCommand::AudioWarp,
            ("sequencer.audio_warp", None),
        ),
        (
            "Audio",
            "Warp Mode: Repitch",
            AppCommand::AudioWarpMode(WarpMode::Repitch),
            ("sequencer.audio_warp_mode", Some("repitch")),
        ),
        (
            "Audio",
            "Warp Loop as 4 Bars",
            AppCommand::AudioWarpBars(4),
            ("sequencer.audio_warp_bars", Some("4")),
        ),
        (
            "Audio",
            "Transpose: Down an Octave",
            AppCommand::AudioTranspose(TransposeStep::OctaveDown),
            ("sequencer.audio_transpose", Some("octave_down")),
        ),
    ] {
        let i = item(&s, menu, label);
        assert!(i.enabled, "{label}");
        assert_eq!(i.command, command);
        assert_eq!(command.command_id(), Some(ident));
        assert_eq!(AppCommand::from_command(ident.0, ident.1), Some(command));
    }
    assert_eq!(
        AppCommand::from_command("sequencer.audio_warp_bars", Some("3")),
        None
    );
}

#[test]
fn warp_mode_and_transpose_from_the_menu_edit_the_clip_and_undo() {
    let mut s = shell();
    let id = imported(&mut s);
    s.core_mut().arrangement_mut().set_selected_clips(&[id]);
    s.set_focus(Pane::Arrangement);
    let before = s.core().doc().clone();
    assert!(s.execute(AppCommand::AudioWarp).is_empty());
    assert!(audio(&s, id).warp.enabled);
    s.execute(AppCommand::AudioWarpMode(WarpMode::Tonal));
    assert_eq!(audio(&s, id).warp.mode, WarpMode::Tonal);
    s.execute(AppCommand::AudioTranspose(TransposeStep::Up));
    s.execute(AppCommand::AudioTranspose(TransposeStep::Up));
    assert_eq!(audio(&s, id).warp.transpose_cents, 200);
    s.execute(AppCommand::AudioTranspose(TransposeStep::Reset));
    assert_eq!(audio(&s, id).warp.transpose_cents, 0);
    s.core_mut().wait_background();
    for _ in 0..5 {
        s.core_mut().undo();
    }
    assert_eq!(s.core().doc(), &before);
}

#[test]
fn the_warp_panel_reads_the_clip_applies_detects_and_closes() {
    let mut s = shell();
    let id = imported(&mut s);
    s.core_mut().arrangement_mut().set_selected_clips(&[id]);
    assert!(!s.warp_open());
    assert!(s.execute(AppCommand::AudioWarpPanel).is_empty());
    assert!(s.warp_open());
    // Warp on and two semitones up, then APPLY.
    let (plus_warp, plus_st, apply) = {
        let v = s.warp_view().unwrap();
        (
            v.plus_rect(WarpRow::Warp).center(),
            v.plus_rect(WarpRow::Transpose).center(),
            v.apply_rect().center(),
        )
    };
    click(&mut s, plus_warp);
    click(&mut s, plus_st);
    click(&mut s, plus_st);
    click(&mut s, apply);
    let w = audio(&s, id).warp;
    assert!(w.enabled && w.transpose_cents == 200, "{w:?}");
    // DETECT TEMPO puts the estimate of the audio (120 BPM here: a click every beat) in the
    // panel; the clip changes only on APPLY.
    s.core_mut().wait_background();
    let detect = s.warp_view().unwrap().detect_rect().center();
    click(&mut s, detect);
    let tenths = s.warp_view().unwrap().values().bpm_tenths;
    assert!((i64::from(tenths) - 1200).abs() <= 6, "{tenths}");
    assert_eq!(audio(&s, id).warp.source_bpm, 120.0);
    // The panel paints, Esc closes it and the values are read from the clip next time.
    let mut dl = atomik_ui::DisplayList::new();
    s.paint(&mut dl);
    assert!(dl.texts().contains(&"WARP"));
    s.handle(&UiEvent::Key {
        key: Key::Escape,
        modifiers: Modifiers::NONE,
        pressed: true,
    });
    assert!(!s.warp_open());
    s.execute(AppCommand::AudioWarpPanel);
    assert_eq!(s.warp_view().unwrap().values().semitones, 2);
    let close = s.warp_view().unwrap().close_rect().center();
    click(&mut s, close);
    assert!(!s.warp_open());
}

#[test]
fn a_loop_as_bars_and_quantize_audio_from_the_menu() {
    let mut s = shell();
    let id = imported(&mut s);
    s.core_mut().arrangement_mut().set_selected_clips(&[id]);
    s.set_focus(Pane::Arrangement);
    // 2 s of audio as 1 bar: 120 BPM, a bar long.
    s.execute(AppCommand::AudioWarpBars(1));
    let w = audio(&s, id).warp;
    assert!(w.enabled && (w.source_bpm - 120.0).abs() < 1e-9);
    assert_eq!(
        s.core().doc().song.clip(id).unwrap().2.length,
        s.core().doc().song.bar_ticks()
    );
    // Quantize Audio uses the quantize options' grid and strength: the hits already sit on the
    // grid, so the markers pin them where they are.
    s.execute(AppCommand::AudioQuantize);
    s.core_mut().wait_background();
    assert!(
        !audio(&s, id).warp.markers.is_empty(),
        "{:?}",
        s.core().last_rejection()
    );
    // The same from its registry id, as a key binding would run it.
    s.core_mut().undo();
    assert!(s.run_command("sequencer.audio_quantize", None).is_some());
    s.core_mut().wait_background();
}

#[test]
fn the_warp_panel_scale_applies_once_and_reset_turns_the_warp_off() {
    let mut s = shell();
    let id = imported(&mut s);
    s.core_mut().arrangement_mut().set_selected_clips(&[id]);
    s.execute(AppCommand::AudioWarp);
    assert!(audio(&s, id).warp.enabled);
    s.execute(AppCommand::AudioWarpPanel);
    // Scale up to 125 %, then APPLY: the source tempo falls to 96, and the panel reads 100 again.
    let (plus, apply, reset) = {
        let v = s.warp_view().unwrap();
        (
            v.plus_rect(WarpRow::Scale).center(),
            v.apply_rect().center(),
            v.reset_rect().center(),
        )
    };
    for _ in 0..25 {
        click(&mut s, plus);
    }
    assert_eq!(s.warp_view().unwrap().values().scale_pct, 125);
    click(&mut s, apply);
    assert!((audio(&s, id).warp.source_bpm - 96.0).abs() < 1e-9);
    assert_eq!(s.warp_view().unwrap().values().scale_pct, 100);
    assert_eq!(s.warp_view().unwrap().values().bpm_tenths, 960);
    // RESET: warp off, the panel follows, one undo step brings it back.
    click(&mut s, reset);
    assert!(!audio(&s, id).warp.enabled);
    assert!(!s.warp_view().unwrap().values().enabled);
    s.core_mut().undo();
    assert!(audio(&s, id).warp.enabled);
}
