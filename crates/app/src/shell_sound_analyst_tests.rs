//! The Sound Analyst in the shell: the File menu row and its command id, the panel with its
//! buttons, a dropped or picked file analysed instead of imported, Make Patch as one undo step,
//! Esc and CLOSE stopping a job, and the panel following the window.

use super::*;
use crate::session::NullBackend;
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_ui::{Key, Modifiers, PointerButton};
use std::f64::consts::TAU;

const RATE: u32 = 44_100;

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

/// A temp folder that removes itself.
struct Tmp(PathBuf);

impl Tmp {
    fn new() -> Self {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("atomik-app-sound-shell-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    /// A saw pluck at 220 Hz.
    fn pluck(&self) -> PathBuf {
        let tau = 0.3 / 10f64.ln();
        let samples: Vec<f32> = (0..(1.0 * f64::from(RATE)) as usize)
            .map(|i| {
                let t = i as f64 / f64::from(RATE);
                let sum: f64 = (1..=20)
                    .map(|k| (TAU * 220.0 * k as f64 * t).sin() / k as f64)
                    .sum();
                (0.4 * (t / 0.002).min(1.0) * (-t / tau).exp() * sum) as f32
            })
            .collect();
        let path = self.0.join("pluck.wav");
        write_wav_f32(&path, &AudioData::new(RATE, vec![samples]).unwrap()).unwrap();
        path
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A left click, with every request the shell made.
fn click(s: &mut Shell<NullBackend>, p: atomik_ui::kurbo::Point) -> Vec<ShellRequest> {
    let mut out = s.handle(&UiEvent::PointerDown {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
    out.extend(s.handle(&UiEvent::PointerUp {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    }));
    out
}

fn escape(s: &mut Shell<NullBackend>) {
    s.handle(&UiEvent::Key {
        key: Key::Escape,
        modifiers: Modifiers::NONE,
        pressed: true,
    });
}

fn drop_file(s: &mut Shell<NullBackend>, path: PathBuf) {
    let at = s.layout().rack.center();
    s.handle(&UiEvent::FileDrop {
        pos: at,
        paths: vec![path],
    });
}

#[test]
fn the_file_menu_has_the_row_and_the_command_has_an_id() {
    let s = shell();
    let row = s.menu_model()[0]
        .items()
        .find(|i| i.command == AppCommand::SoundAnalyst)
        .map(|i| (i.label.to_string(), i.enabled));
    assert_eq!(row, Some(("Sound Analyst\u{2026}".to_owned(), true)));
    assert_eq!(
        AppCommand::SoundAnalyst.command_id(),
        Some(("file.sound_analyst", None))
    );
    assert_eq!(
        AppCommand::from_command("file.sound_analyst", None),
        Some(AppCommand::SoundAnalyst)
    );
    assert!(
        atomik_ui::keymap::Registry::bundled()
            .get("file.sound_analyst")
            .is_some()
    );
}

#[test]
fn the_command_opens_the_panel_and_esc_closes_it() {
    let mut s = shell();
    assert!(!s.sound_analyst_open());
    s.execute(AppCommand::SoundAnalyst);
    assert!(s.sound_analyst_open());
    let data = s.sound_analyst_view().unwrap().data();
    assert!(data.file.is_none() && !data.can_make);
    escape(&mut s);
    assert!(!s.sound_analyst_open());
}

#[test]
fn open_file_asks_for_a_file_and_close_closes() {
    let mut s = shell();
    s.execute(AppCommand::SoundAnalyst);
    let open = s.sound_analyst_view().unwrap().open_rect().center();
    assert_eq!(click(&mut s, open), vec![ShellRequest::PickSoundFile]);
    let close = s.sound_analyst_view().unwrap().close_rect().center();
    click(&mut s, close);
    assert!(!s.sound_analyst_open());
}

#[test]
fn a_dropped_file_is_analysed_not_imported_and_make_patch_is_one_undo_step() {
    let tmp = Tmp::new();
    let mut s = shell();
    s.execute(AppCommand::SoundAnalyst);
    drop_file(&mut s, tmp.pluck());
    s.core_mut().wait_background();
    s.frame();
    assert_eq!(
        s.core().doc().song.tracks.len(),
        0,
        "the drop did not import the file as a track"
    );
    let data = s.sound_analyst_view().unwrap().data().clone();
    assert!(data.file.is_some_and(|f| f.starts_with("pluck.wav")));
    assert!(data.can_make && data.matched.is_some());
    // MAKE PATCH adds the track.
    let before = s.core().doc().clone();
    let make = s.sound_analyst_view().unwrap().make_rect().center();
    click(&mut s, make);
    assert_eq!(s.core().doc().song.tracks.len(), 1);
    assert!(s.core().sound_patch_device().is_some());
    assert!(s.sound_analyst_view().unwrap().data().can_match);
    s.core_mut().undo();
    assert_eq!(*s.core().doc(), before, "one undo takes all of it away");
    // The panel is still open on the analysis.
    assert!(s.sound_analyst_open());
}

#[test]
fn a_file_dropped_with_the_panel_closed_still_imports_as_before() {
    let tmp = Tmp::new();
    let mut s = shell();
    drop_file(&mut s, tmp.pluck());
    s.core_mut().wait_background();
    assert_eq!(s.core().doc().song.tracks.len(), 1);
    assert!(!s.sound_analyst_open());
}

#[test]
fn a_midi_file_dropped_on_the_panel_is_not_analysed() {
    let tmp = Tmp::new();
    let mut s = shell();
    s.execute(AppCommand::SoundAnalyst);
    let midi = tmp.0.join("tune.mid");
    std::fs::write(&midi, b"MThd").unwrap();
    drop_file(&mut s, midi);
    assert!(!s.core().sound_running());
    assert!(s.core().sound_analysis().is_none());
    assert_eq!(s.core().doc().song.tracks.len(), 0);
}

#[test]
fn the_analyse_entry_point_opens_the_panel_and_starts() {
    let tmp = Tmp::new();
    let mut s = shell();
    s.analyse_sound_file(&tmp.pluck());
    assert!(s.sound_analyst_open());
    assert!(s.core().sound_running());
    assert!(s.sound_analyst_view().unwrap().data().running.is_some());
    s.core_mut().wait_background();
    s.frame();
    assert!(s.sound_analyst_view().unwrap().data().can_make);
}

#[test]
fn closing_the_panel_stops_a_job_that_runs() {
    let tmp = Tmp::new();
    let mut s = shell();
    s.analyse_sound_file(&tmp.pluck());
    let close = s.sound_analyst_view().unwrap().close_rect().center();
    click(&mut s, close);
    assert!(!s.sound_analyst_open());
    s.core_mut().wait_background();
    assert!(!s.core().sound_running());
}

#[test]
fn the_panel_has_tooltips_and_follows_the_window() {
    let mut s = shell();
    s.execute(AppCommand::SoundAnalyst);
    let view = s.sound_analyst_view().unwrap();
    assert!(view.help_at(view.close_rect().center()).is_some());
    s.handle(&UiEvent::Resize(Size::new(800.0, 700.0)));
    let panel = s.sound_analyst_view().unwrap().panel();
    assert!(panel.x1 <= 800.0 && panel.y1 <= 700.0, "{panel:?}");
}
