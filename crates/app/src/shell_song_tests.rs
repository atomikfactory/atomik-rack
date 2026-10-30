//! `Shell` tests (layout): rack on top, divider, transport bar,
//! arrangement or piano roll; pointer routing, focus, transport shortcuts, menus, files.

use super::*;
use crate::app_core::Pane;
use crate::commands::MenuEntry;
use crate::record::InputQuantize;
use crate::session::NullBackend;
use crate::ui_state::UiState;
use atomik_model::{Command, DeviceKey, SongCommand};
use atomik_ui::transport::TRANSPORT_HEIGHT;
use atomik_ui::{DisplayList, Key, Modifiers, PointerButton, TransportAction, TransportButton};

const W: f64 = 1000.0;
const H: f64 = 800.0;

fn shell_at(ui_state_path: Option<PathBuf>) -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(atomik_ui::keymap::Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: None,
            ui_state_path,
            library_path: None,
            library_root: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(W, H)));
    s
}

fn shell() -> Shell<NullBackend> {
    shell_at(None)
}

fn key_event(key: Key, modifiers: Modifiers) -> UiEvent {
    UiEvent::Key {
        key,
        modifiers,
        pressed: true,
    }
}

/// A key press and its release (a held key would be a repeat).
fn tap(s: &mut Shell<NullBackend>, key: Key) {
    s.handle(&press(key.clone()));
    s.handle(&UiEvent::Key {
        key,
        modifiers: Modifiers::NONE,
        pressed: false,
    });
}

fn press(key: Key) -> UiEvent {
    key_event(key, Modifiers::NONE)
}

fn down_n(p: Point, clicks: u8) -> UiEvent {
    UiEvent::PointerDown {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks,
    }
}

fn down(p: Point) -> UiEvent {
    down_n(p, 1)
}

fn up(p: Point) -> UiEvent {
    UiEvent::PointerUp {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    }
}

fn moved(p: Point) -> UiEvent {
    UiEvent::PointerMove {
        pos: p,
        modifiers: Modifiers::NONE,
    }
}

fn click(s: &mut Shell<NullBackend>, p: Point) -> Vec<ShellRequest> {
    let mut r = s.handle(&down(p));
    r.extend(s.handle(&up(p)));
    r
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("atomik-app-shell-song-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn demo(s: &mut Shell<NullBackend>) {
    s.core_mut()
        .open_str(include_str!("../../../examples/projects/demo.atomik"), None)
        .unwrap();
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn the_window_stacks_rack_divider_transport_bar_and_arrangement() {
    let s = shell();
    let l = s.layout();
    assert!(close(l.rack.y0, MENU_BAR_HEIGHT));
    let divider = l.divider.expect("a divider while the arrangement shows");
    assert!(close(divider.y0, l.rack.y1));
    assert!(close(l.transport.y0, divider.y1));
    assert!(close(l.transport.height(), TRANSPORT_HEIGHT));
    let song = l.song.expect("the arrangement area");
    assert!(close(song.y0, l.transport.y1));
    assert!(close(song.y1, H - STATUS_BAR_HEIGHT));
    assert!(close(l.rack.width(), W) && close(song.width(), W));
    // The views are placed there.
    assert_eq!(s.view_size(), l.rack.size());
    assert_eq!(s.core().transport_bar().bounds(), l.transport);
    assert_eq!(s.core().arrangement().bounds(), song);
    // Default split: about half each.
    assert!((l.rack.height() - song.height()).abs() < 2.0);
}

#[test]
fn dragging_the_divider_moves_the_split_and_saves_it_in_the_ui_state() {
    let path = temp("ui-state.json");
    let _ = std::fs::remove_file(&path);
    let mut s = shell_at(Some(path.clone()));
    let d = s.layout().divider.unwrap().center();
    s.handle(&down(d));
    s.handle(&moved(Point::new(d.x, d.y - 100.0)));
    // Captured: moving over the rack does not reach it.
    s.handle(&moved(Point::new(d.x, d.y - 150.0)));
    s.handle(&up(Point::new(d.x, d.y - 150.0)));
    let l = s.layout();
    assert!(close(l.divider.unwrap().center().y, d.y - 150.0));
    let saved = UiState::load(&path);
    assert!((saved.rack_split - s.ui_state().rack_split).abs() < 1e-9);
    assert!(saved.rack_split < 0.45);
    // A new shell with that file starts with the same split.
    let t = shell_at(Some(path.clone()));
    assert!(close(t.layout().rack.height(), l.rack.height()));
    // The split is clamped so both panes stay usable.
    s.handle(&down(l.divider.unwrap().center()));
    s.handle(&moved(Point::new(d.x, 0.0)));
    s.handle(&up(Point::new(d.x, 0.0)));
    assert!(s.layout().rack.height() >= MIN_PANE_HEIGHT - 1e-9);
    std::fs::remove_file(&path).ok();
}

#[test]
fn hiding_the_arrangement_gives_the_rack_the_space_and_keeps_the_transport_bar() {
    let mut s = shell();
    s.execute(AppCommand::ToggleSong);
    let l = s.layout();
    assert!(l.song.is_none() && l.divider.is_none());
    assert!(close(l.transport.y1, H - STATUS_BAR_HEIGHT));
    assert!(close(l.rack.y1, l.transport.y0));
    assert!(!s.menu_state().song_visible);
    s.execute(AppCommand::ToggleSong);
    assert!(s.layout().song.is_some());
}

#[test]
fn transport_bar_buttons_drive_the_transport() {
    let mut s = shell();
    let play = s
        .core()
        .transport_bar()
        .button_center(TransportButton::Play);
    click(&mut s, play);
    assert!(s.core().is_playing());
    let stop = s
        .core()
        .transport_bar()
        .button_center(TransportButton::Stop);
    click(&mut s, stop);
    assert!(!s.core().is_playing());
    let metronome = s
        .core()
        .transport_bar()
        .button_center(TransportButton::Metronome);
    click(&mut s, metronome);
    assert!(s.core().metronome());
    assert!(s.menu_state().metronome);
}

#[test]
fn space_toggles_play_while_the_rack_has_focus_without_a_focused_control() {
    let mut s = shell();
    tap(&mut s, Key::Space);
    assert!(s.core().is_playing());
    tap(&mut s, Key::Space);
    assert!(!s.core().is_playing());
    tap(&mut s, Key::Char('l'));
    assert!(s.core().doc().song.loop_enabled, "L toggles the loop");
    // Ctrl+Space arms recording (here: no armed track, so it is refused with a message).
    s.handle(&key_event(Key::Space, Modifiers::CTRL));
    assert!(!s.core().is_recording());
    assert!(s.core().last_rejection().unwrap().contains("arm"));
}

#[test]
fn ctrl_space_records_on_an_armed_track() {
    let mut s = shell();
    s.core_mut()
        .apply_command(Command::AddDevice {
            index: 0,
            type_id: "atomik.poly-synth".into(),
            key: Some(DeviceKey("bass".into())),
        })
        .unwrap();
    for c in [
        SongCommand::AddTrack {
            device: DeviceKey("bass".into()),
        },
        SongCommand::SetTrackFlags {
            device: DeviceKey("bass".into()),
            muted: false,
            solo: false,
            armed: true,
        },
    ] {
        s.core_mut().apply_command(Command::Song(c)).unwrap();
    }
    s.handle(&key_event(Key::Space, Modifiers::CTRL));
    assert!(s.core().is_recording() && s.core().is_playing());
    assert!(s.menu_state().recording);
    s.execute(AppCommand::Transport(TransportAction::Stop));
    assert!(!s.core().is_recording());
}

#[test]
fn clicking_a_view_focuses_it_and_keys_follow_the_focus() {
    let mut s = shell();
    let song = s.layout().song.unwrap();
    click(&mut s, Point::new(song.x0 + 400.0, song.y1 - 20.0));
    assert_eq!(s.focus(), Pane::Arrangement);
    assert!(s.core().arrangement().focused());
    // The arrangement maps Space itself (same transport shortcut).
    tap(&mut s, Key::Space);
    assert!(s.core().is_playing());
    let rack = s.layout().rack.center();
    click(&mut s, rack);
    assert_eq!(s.focus(), Pane::Rack);
    assert!(!s.core().arrangement().focused());
}

#[test]
fn while_the_tempo_is_typed_every_key_goes_to_the_transport_bar() {
    let mut s = shell();
    let tempo = s.core().transport_bar().tempo_center();
    s.handle(&down_n(tempo, 1));
    s.handle(&up(tempo));
    s.handle(&down_n(tempo, 2));
    s.handle(&up(tempo));
    assert!(s.core().transport_bar().is_editing_tempo());
    for c in ['9', '5'] {
        tap(&mut s, Key::Char(c));
    }
    tap(&mut s, Key::Space);
    assert!(!s.core().is_playing(), "Space does not play while typing");
    tap(&mut s, Key::Enter);
    assert!((s.core().doc().tempo_bpm - 95.0).abs() < 1e-9);
}

#[test]
fn a_signature_typed_in_the_transport_bar_is_one_undo_step() {
    let mut s = shell();
    let meter = s.core().transport_bar().meter_center();
    s.handle(&down_n(meter, 1));
    s.handle(&up(meter));
    s.handle(&down_n(meter, 2));
    s.handle(&up(meter));
    assert!(s.core().transport_bar().is_editing_meter());
    for c in ['7', '/', '8'] {
        tap(&mut s, Key::Char(c));
    }
    tap(&mut s, Key::Space);
    assert!(!s.core().is_playing(), "Space does not play while typing");
    tap(&mut s, Key::Enter);
    let m = &s.core().doc().song.meter;
    assert_eq!((m.len(), m[0].num, m[0].den), (1, 7, 8));
    s.execute(AppCommand::Undo);
    let m = &s.core().doc().song.meter;
    assert_eq!((m.len(), m[0].num, m[0].den), (1, 4, 4));
}

#[test]
fn the_piano_roll_opens_in_place_of_the_arrangement() {
    let mut s = shell();
    demo(&mut s);
    assert!(!s.menu_state().piano_roll);
    s.execute(AppCommand::TogglePianoRoll);
    assert!(s.core().piano_roll_clip().is_some());
    assert!(s.menu_state().piano_roll);
    let song = s.layout().song.unwrap();
    assert_eq!(s.core().piano_roll().bounds(), song);
    // Its pane gets the clicks there now.
    click(&mut s, Point::new(song.x0 + 300.0, song.y0 + 100.0));
    assert_eq!(s.focus(), Pane::PianoRoll);
    s.execute(AppCommand::TogglePianoRoll);
    assert!(s.core().piano_roll_clip().is_none());
    // Without a note clip the roll cannot open: the user is told.
    let mut empty = shell();
    empty.execute(AppCommand::TogglePianoRoll);
    assert!(empty.core().piano_roll_clip().is_none());
    assert!(empty.core().last_rejection().is_some());
}

#[test]
fn each_frame_shows_the_transport_and_paints_every_pane() {
    let mut s = shell();
    demo(&mut s);
    s.frame();
    let mut list = DisplayList::new();
    s.paint(&mut list);
    let texts = list.texts();
    let (bbt, _) = s.core().transport_bar().position_text();
    assert!(texts.iter().any(|t| *t == bbt), "{texts:?}");
    assert!(texts.contains(&"Bass"), "track names: {texts:?}");
    assert!(texts.contains(&"File"));
    assert!(!s.needs_repaint());
}

#[test]
fn file_menu_exports_patches_and_opens_the_e3_2_stress_song() {
    let mut s = shell();
    assert_eq!(
        s.execute(AppCommand::ExportPatch),
        vec![ShellRequest::PickExportFile]
    );
    let path = temp("export.json");
    s.export_to(&path);
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("atomik-patch")
    );
    assert!(s.execute(AppCommand::StressSong).is_empty());
    let song = &s.core().doc().song;
    assert_eq!(song.tracks.len(), 8);
    assert!(song.tracks.iter().all(|t| t.lanes.len() >= 2));
    std::fs::remove_file(&path).ok();
}

#[test]
fn transport_menu_lists_the_transport_and_quantize_choices() {
    let mut s = shell();
    let model = s.menu_model();
    let titles: Vec<_> = model.iter().map(|m| m.title).collect();
    assert_eq!(
        titles,
        [
            "File",
            "Edit",
            "View",
            "Transport",
            "Mixer",
            "Create",
            "Track",
            "Audio"
        ]
    );
    let labels: Vec<_> = model[3].items().map(|i| i.label.as_ref()).collect();
    for want in [
        "Play / Stop",
        "Record",
        "Return to Zero",
        "Loop",
        "Metronome",
        "Input Quantize: Off",
        "Input Quantize: 1/8",
        "Input Quantize: 1/16",
    ] {
        assert!(labels.contains(&want), "{want} missing in {labels:?}");
    }
    assert!(model[3].entries.contains(&MenuEntry::Separator));
    s.execute(AppCommand::Quantize(InputQuantize::Sixteenth));
    assert_eq!(s.core().input_quantize(), InputQuantize::Sixteenth);
    assert_eq!(s.menu_state().quantize, InputQuantize::Sixteenth);
}

/// Presses on `from`, moves to each offset (up is negative) and releases at the last.
fn drag_by(s: &mut Shell<NullBackend>, from: Point, dys: &[f64]) {
    s.handle(&down(from));
    let mut last = from;
    for dy in dys {
        last = Point::new(from.x, from.y + dy);
        s.handle(&moved(last));
    }
    s.handle(&up(last));
}

#[test]
fn dragging_the_tempo_changes_it_live_and_is_one_undo_step() {
    let mut s = shell();
    let before = s.core().doc().tempo_bpm;
    let p = s.core().transport_bar().tempo_center();
    s.handle(&down(p));
    s.handle(&moved(Point::new(p.x, p.y - 10.0)));
    assert!(
        (s.core().doc().tempo_bpm - (before + 2.0)).abs() < 1e-9,
        "applied while the button is still down"
    );
    s.handle(&moved(Point::new(p.x, p.y - 30.0)));
    s.handle(&up(Point::new(p.x, p.y - 30.0)));
    assert!((s.core().doc().tempo_bpm - (before + 6.0)).abs() < 1e-9);
    assert_eq!(s.core().doc().song.tempo[0].bpm, before + 6.0);
    s.execute(AppCommand::Undo);
    assert!(
        (s.core().doc().tempo_bpm - before).abs() < 1e-9,
        "the whole drag undoes at once"
    );
    assert!(!s.core().document().can_undo(), "and it was a single step");
}

#[test]
fn dragging_a_tempo_decimal_or_the_signature_is_one_undo_step_each() {
    let mut s = shell();
    let before = s.core().doc().tempo_bpm;
    let dec = s.core().transport_bar().tempo_decimals_point();
    drag_by(&mut s, dec, &[-5.0, -10.0, -15.0]);
    assert!((s.core().doc().tempo_bpm - (before + 0.03)).abs() < 1e-9);
    let num = s.core().transport_bar().meter_num_point();
    drag_by(&mut s, num, &[-5.0, -10.0, -15.0]);
    let m = &s.core().doc().song.meter;
    assert_eq!((m[0].num, m[0].den), (7, 4));
    let den = s.core().transport_bar().meter_den_point();
    drag_by(&mut s, den, &[-5.0, -10.0]);
    let m = &s.core().doc().song.meter;
    assert_eq!((m[0].num, m[0].den), (7, 16));
    s.execute(AppCommand::Undo);
    let m = &s.core().doc().song.meter;
    assert_eq!((m[0].num, m[0].den), (7, 4));
    s.execute(AppCommand::Undo);
    let m = &s.core().doc().song.meter;
    assert_eq!((m[0].num, m[0].den), (4, 4));
    s.execute(AppCommand::Undo);
    assert!((s.core().doc().tempo_bpm - before).abs() < 1e-9);
    assert!(!s.core().document().can_undo());
}

#[test]
fn the_wheel_over_the_tempo_steps_whole_bpm() {
    let mut s = shell();
    let before = s.core().doc().tempo_bpm;
    let p = s.core().transport_bar().tempo_center();
    s.handle(&UiEvent::Wheel {
        delta: atomik_ui::kurbo::Vec2::new(0.0, 1.0),
        pos: p,
        modifiers: Modifiers::NONE,
    });
    assert!((s.core().doc().tempo_bpm - (before + 1.0)).abs() < 1e-9);
}

#[test]
fn the_recording_input_of_an_armed_audio_track_is_set_from_the_transport_bar() {
    use atomik_ui::RecInputPart;
    let mut s = shell();
    s.core_mut()
        .apply_command(Command::Song(SongCommand::AddAudioTrack { name: None }))
        .unwrap();
    let key = s.core().doc().song.tracks[0].device.clone();
    // Not armed: no section.
    s.frame_at(0.0);
    assert!(!s.core().transport_bar().rec_input_visible());
    s.core_mut()
        .apply_command(Command::Song(SongCommand::SetTrackFlags {
            device: key.clone(),
            muted: false,
            solo: false,
            armed: true,
        }))
        .unwrap();
    s.frame_at(0.1);
    assert!(s.core().transport_bar().rec_input_visible());
    let param =
        |s: &Shell<NullBackend>, p: &str| s.core().doc().param_value(&key, p, s.core().registry());
    let gain = s
        .core()
        .transport_bar()
        .rec_input_point(RecInputPart::Gain)
        .unwrap();
    // 20 px up is four half-decibel steps, applied live, one undo step.
    drag_by(&mut s, gain, &[-5.0, -10.0, -20.0]);
    assert_eq!(param(&s, "rec_gain"), Some(2.0));
    s.execute(AppCommand::Undo);
    assert_eq!(
        param(&s, "rec_gain"),
        Some(0.0),
        "the whole drag undoes at once"
    );
    s.execute(AppCommand::Redo);
    assert_eq!(param(&s, "rec_gain"), Some(2.0));
    let safe = s
        .core()
        .transport_bar()
        .rec_input_point(RecInputPart::Safe)
        .unwrap();
    s.handle(&down(safe));
    s.handle(&up(safe));
    assert_eq!(param(&s, "rec_clip_safe"), Some(1.0));
}
