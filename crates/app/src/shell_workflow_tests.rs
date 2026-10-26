//! The daily keys end to end: key scripts
//! through the router and a headless `Shell`, the OS a parameter (never `cfg`). The app default
//! profile is `standard`; `legacy` stays selectable.

use super::*;
use crate::keymap_store::{WORKFLOW_FILE, load_workflow};
use crate::session::NullBackend;
use atomik_model::{Command, DeviceKey, SongCommand};
use atomik_ui::browser::CreateKind;
use atomik_ui::keymap::script::event_for;
use atomik_ui::keymap::{BindingDef, RemovedDef};
use atomik_ui::timeline::Snap;
use atomik_ui::{PointerButton, Tool, TransportAction};

const BAR: i64 = 4 * 15_360;

fn shell_with(os: Os, profile: Option<Profile>, settings: Option<PathBuf>) -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(os),
            profile,
            in_window_menu: true,
            settings_path: settings,
            ui_state_path: None,
            library_path: None,
            library_root: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1100.0, 800.0)));
    s
}

/// A shell on the app default profile (no profile given).
fn shell_for(os: Os) -> Shell<NullBackend> {
    shell_with(os, None, None)
}

fn shell() -> Shell<NullBackend> {
    shell_for(Os::Windows)
}

fn demo(s: &mut Shell<NullBackend>) {
    s.core_mut()
        .open_str(include_str!("../../../examples/projects/demo.atomik"), None)
        .unwrap();
}

struct Keys<'a> {
    shell: &'a mut Shell<NullBackend>,
    os: Os,
}

impl Keys<'_> {
    fn event(&self, chord: &str, pressed: bool) -> UiEvent {
        let (key, modifiers) = event_for(self.os, chord).unwrap();
        UiEvent::Key {
            key,
            modifiers,
            pressed,
        }
    }

    fn tap(&mut self, chord: &str) -> Vec<ShellRequest> {
        let e = self.event(chord, true);
        let r = self.shell.handle(&e);
        let e = self.event(chord, false);
        self.shell.handle(&e);
        r
    }

    fn taps(&mut self, chords: &[&str]) {
        for c in chords {
            self.tap(c);
        }
    }
}

fn keys(s: &mut Shell<NullBackend>) -> Keys<'_> {
    Keys {
        shell: s,
        os: Os::Windows,
    }
}

fn keys_on(s: &mut Shell<NullBackend>, os: Os) -> Keys<'_> {
    Keys { shell: s, os }
}

fn arm_synth(s: &mut Shell<NullBackend>) {
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
}

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("atomik-wk-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn click_at(s: &mut Shell<NullBackend>, p: Point, modifiers: Modifiers) {
    for e in [
        UiEvent::PointerDown {
            pos: p,
            button: PointerButton::Primary,
            modifiers,
            clicks: 1,
        },
        UiEvent::PointerUp {
            pos: p,
            button: PointerButton::Primary,
            modifiers,
        },
    ] {
        s.handle(&e);
    }
}

// ---------------------------------------------------------------- the default profile

#[test]
fn the_app_runs_the_reference_keys_and_legacy_stays_selectable() {
    assert_eq!(APP_DEFAULT_PROFILE, Profile::Standard);
    let text = |s: &Shell<NullBackend>, id: &str| s.keymap().shortcut_text(id, None);
    let s = shell_for(Os::Windows);
    assert_eq!(
        text(&s, "file.import_audio").as_deref(),
        Some("Ctrl+Shift+I")
    );
    assert_eq!(text(&s, "create.instrument").as_deref(), Some("Ctrl+I"));
    assert_eq!(text(&s, "transport.record").as_deref(), Some("Ctrl+Enter"));
    let s = shell_for(Os::Mac);
    assert_eq!(text(&s, "file.import_audio").as_deref(), Some("Cmd+Opt+I"));
    assert_eq!(text(&s, "create.instrument").as_deref(), Some("Cmd+I"));
    let s = shell_with(Os::Windows, Some(Profile::Legacy), None);
    assert_eq!(text(&s, "file.import_audio").as_deref(), Some("Ctrl+I"));
    assert_eq!(text(&s, "transport.record").as_deref(), Some("Ctrl+Space"));
}

#[test]
fn the_menus_show_the_new_chords() {
    let s = shell();
    let menus = s.menu_model();
    let item = |title: &str, label: &str| {
        menus
            .iter()
            .find(|m| m.title == title)
            .unwrap()
            .items()
            .find(|i| i.label.starts_with(label))
            .unwrap_or_else(|| panic!("{title} > {label}"))
            .shortcut
            .clone()
    };
    assert_eq!(
        item("File", "Import Audio").as_deref(),
        Some("Ctrl+Shift+I")
    );
    assert_eq!(item("Transport", "Record").as_deref(), Some("Ctrl+Enter"));
    assert_eq!(item("View", "Rack").as_deref(), Some("F6"));
    assert_eq!(item("View", "Sequencer Only").as_deref(), Some("F7"));
    assert_eq!(item("View", "Clip Editor").as_deref(), Some("F8"));
    assert_eq!(
        item("Create", "Create Instrument").as_deref(),
        Some("Ctrl+I")
    );
    assert_eq!(item("Create", "Create Effect").as_deref(), Some("Ctrl+E"));
    assert_eq!(
        item("Create", "Create Audio Track").as_deref(),
        Some("Ctrl+T")
    );
    assert_eq!(
        item("Create", "Create Mixer Channel").as_deref(),
        Some("Ctrl+Shift+M")
    );
}

// ---------------------------------------------------------------- transport keys

#[test]
fn space_plays_and_stops_numpad_enter_plays_and_stop_keys_stop_then_rewind() {
    let mut s = shell();
    keys(&mut s).tap("Space");
    assert!(s.core().is_playing());
    keys(&mut s).tap("Space");
    assert!(!s.core().is_playing());
    keys(&mut s).tap("NumpadEnter");
    keys(&mut s).tap("NumpadEnter");
    assert!(s.core().is_playing(), "Numpad Enter only plays");
    s.core_mut()
        .transport_action(TransportAction::Locate(3 * BAR));
    keys(&mut s).tap("Shift+Return");
    assert!(!s.core().is_playing());
    assert_eq!(
        s.core().position(),
        3 * BAR,
        "the first stop keeps the position"
    );
    keys(&mut s).tap("Shift+Return");
    assert_eq!(
        s.core().position(),
        0,
        "a second press returns to the start"
    );
    keys(&mut s).tap("NumpadEnter");
    keys(&mut s).tap("Numpad0");
    assert!(!s.core().is_playing());
}

#[test]
fn control_return_records_with_its_secondaries_and_plain_r_is_the_razor() {
    for chord in ["Cmd+Return", "Cmd+Space", "NumpadMul"] {
        let mut s = shell();
        arm_synth(&mut s);
        keys(&mut s).tap(chord);
        assert!(s.core().is_recording() && s.core().is_playing(), "{chord}");
        keys(&mut s).tap(chord);
        assert!(!s.core().is_recording(), "{chord} stops the take");
    }
    // R no longer records: with the arrangement focused it selects the razor tool.
    let mut s = shell();
    arm_synth(&mut s);
    s.set_focus(Pane::Arrangement);
    keys(&mut s).tap("R");
    assert!(!s.core().is_recording());
    assert_eq!(s.core().arrangement().tool(), Tool::Razor);
}

#[test]
fn metronome_loop_start_and_tempo_keys() {
    let mut s = shell();
    for chord in ["C", "Numpad9"] {
        keys(&mut s).tap(chord);
    }
    assert!(!s.core().metronome(), "two toggles");
    keys(&mut s).tap("C");
    assert!(s.core().metronome());
    keys(&mut s).tap("L");
    assert!(s.core().doc().song.loop_enabled);
    keys(&mut s).tap("NumpadDiv");
    assert!(!s.core().doc().song.loop_enabled);
    let bpm = s.core().doc().tempo_bpm;
    keys(&mut s).tap("NumpadAdd");
    assert!((s.core().doc().tempo_bpm - (bpm + 1.0)).abs() < 1e-9);
    keys(&mut s).taps(&["NumpadSub", "NumpadSub"]);
    assert!((s.core().doc().tempo_bpm - (bpm - 1.0)).abs() < 1e-9);
    s.core_mut()
        .transport_action(TransportAction::Locate(2 * BAR));
    keys(&mut s).tap("NumpadDecimal");
    assert_eq!(s.core().position(), 0);
}

#[test]
fn numpad_bar_moves_and_locators() {
    let mut s = shell();
    keys(&mut s).tap("Numpad5");
    assert_eq!(s.core().position(), BAR);
    keys(&mut s).tap("Numpad8");
    assert_eq!(s.core().position(), 2 * BAR);
    keys(&mut s).tap("Numpad4");
    assert_eq!(s.core().position(), BAR);
    keys(&mut s).tap("Numpad7");
    assert_eq!(s.core().position(), 0);
    keys(&mut s).tap("Numpad7");
    assert_eq!(s.core().position(), 0, "never before the start");
    // Mid-bar: forward goes to the next bar line, back to the start of the bar.
    s.core_mut()
        .transport_action(TransportAction::Locate(BAR + 100));
    keys(&mut s).tap("Numpad4");
    assert_eq!(s.core().position(), BAR);
    s.core_mut()
        .transport_action(TransportAction::Locate(BAR + 100));
    keys(&mut s).tap("Numpad5");
    assert_eq!(s.core().position(), 2 * BAR);
    let loop_range = (2 * BAR, 6 * BAR);
    s.core_mut()
        .apply_command(Command::Song(SongCommand::SetLoop {
            range: loop_range,
            enabled: true,
        }))
        .unwrap();
    keys(&mut s).tap("Numpad2");
    assert_eq!(s.core().position(), 6 * BAR);
    keys(&mut s).tap("Numpad1");
    assert_eq!(s.core().position(), 2 * BAR);
    keys(&mut s).tap("Alt+Right");
    assert_eq!(s.core().position(), 6 * BAR);
    keys(&mut s).tap("Alt+Left");
    assert_eq!(s.core().position(), 2 * BAR);
}

#[test]
fn the_transport_keys_work_with_every_pane_focused() {
    let mut s = shell();
    demo(&mut s);
    s.execute(AppCommand::TogglePianoRoll);
    for pane in [Pane::Rack, Pane::Arrangement, Pane::PianoRoll] {
        s.set_focus(pane);
        keys(&mut s).tap("Space");
        assert!(s.core().is_playing(), "{pane:?}");
        keys(&mut s).tap("Space");
        assert!(!s.core().is_playing(), "{pane:?}");
    }
}

// ---------------------------------------------------------------- tools, snap, zoom, follow

#[test]
fn q_w_e_r_select_the_tools_of_the_focused_view() {
    let mut s = shell();
    demo(&mut s);
    s.set_focus(Pane::Arrangement);
    for (chord, tool) in [
        ("W", Tool::Pencil),
        ("E", Tool::Eraser),
        ("R", Tool::Razor),
        ("Q", Tool::Select),
    ] {
        keys(&mut s).tap(chord);
        assert_eq!(s.core().arrangement().tool(), tool, "{chord}");
    }
    s.execute(AppCommand::TogglePianoRoll);
    s.set_focus(Pane::PianoRoll);
    keys(&mut s).tap("R");
    assert_eq!(s.core().piano_roll().tool(), Tool::Razor);
    assert_eq!(
        s.core().arrangement().tool(),
        Tool::Select,
        "each view has its own tool"
    );
    // In the rack the letters are not tools (and not record either).
    s.set_focus(Pane::Rack);
    keys(&mut s).tap("R");
    assert_eq!(s.core().piano_roll().tool(), Tool::Razor);
    assert!(!s.core().is_recording());
}

#[test]
fn the_razor_tool_splits_a_clip_and_a_ruler_click_splits_every_track() {
    let mut s = shell();
    demo(&mut s);
    s.set_focus(Pane::Arrangement);
    keys(&mut s).tap("R");
    let a = s.core().arrangement();
    let at = 2 * BAR;
    // The lane of the bass track, at bar 3.
    let lane = a.lane_rect(&DeviceKey("bass".into()), 0).unwrap();
    let p = Point::new(a.tick_to_x(at), lane.center().y + 6.0);
    let clips_before = s.core().doc().song.tracks[0].lanes[0].clips.len();
    click_at(&mut s, p, Modifiers::NONE);
    assert_eq!(
        s.core().doc().song.tracks[0].lanes[0].clips.len(),
        clips_before + 1,
        "the razor split the clip under the pointer"
    );
    // A ruler click at bar 6 splits the clips of both tracks.
    let a = s.core().arrangement();
    let ruler = Point::new(a.tick_to_x(6 * BAR), a.ruler_label_y());
    let total = |s: &Shell<NullBackend>| -> usize {
        s.core()
            .doc()
            .song
            .tracks
            .iter()
            .flat_map(|t| t.lanes.iter())
            .map(|l| l.clips.len())
            .sum()
    };
    let before = total(&s);
    click_at(&mut s, ruler, Modifiers::NONE);
    assert!(total(&s) >= before + 2, "{} -> {}", before, total(&s));
}

#[test]
fn s_toggles_snap_and_remembers_the_value() {
    let mut s = shell();
    demo(&mut s);
    s.set_focus(Pane::Arrangement);
    s.core_mut().arrangement_mut().set_snap(Snap::Eighth);
    keys(&mut s).tap("S");
    assert_eq!(s.core().arrangement().snap(), Snap::Off);
    keys(&mut s).tap("S");
    assert_eq!(s.core().arrangement().snap(), Snap::Eighth);
    s.execute(AppCommand::TogglePianoRoll);
    s.set_focus(Pane::PianoRoll);
    keys(&mut s).tap("S");
    assert_eq!(s.core().piano_roll().snap(), Snap::Off);
    assert_eq!(s.core().arrangement().snap(), Snap::Eighth);
}

#[test]
fn h_and_g_zoom_time_in_the_sequencer_and_the_rack_zoom_stays_on_ctrl_equals() {
    let mut s = shell();
    demo(&mut s);
    s.set_focus(Pane::Arrangement);
    let z = s.core().arrangement().px_per_tick();
    keys(&mut s).tap("H");
    assert!(s.core().arrangement().px_per_tick() > z);
    keys(&mut s).tap("G");
    assert!((s.core().arrangement().px_per_tick() - z).abs() < 1e-12);
    keys(&mut s).tap("Cmd+=");
    assert!(
        s.core().arrangement().px_per_tick() > z,
        "Ctrl+= is time zoom with the sequencer focused"
    );
    let rack_zoom = s.core().view().zoom();
    s.set_focus(Pane::Rack);
    keys(&mut s).tap("Cmd+=");
    assert!(
        s.core().view().zoom() > rack_zoom,
        "and rack zoom with the rack focused"
    );
    let z = s.core().arrangement().px_per_tick();
    keys(&mut s).tap("H");
    assert_eq!(
        s.core().arrangement().px_per_tick(),
        z,
        "H is not a rack key"
    );
}

#[test]
fn shift_h_g_z_zoom_the_track_heights_in_the_sequencer_and_the_keys_in_the_roll() {
    for os in [Os::Windows, Os::Mac, Os::Linux] {
        let mut s = shell_for(os);
        demo(&mut s);
        s.set_focus(Pane::Arrangement);
        let (time, keys0) = (
            s.core().arrangement().px_per_tick(),
            s.core().piano_roll().key_height(),
        );
        keys_on(&mut s, os).tap("Shift+H");
        assert!(s.core().arrangement().track_zoom() > 1.0, "{os:?}");
        keys_on(&mut s, os).tap("Shift+G");
        assert!(
            (s.core().arrangement().track_zoom() - 1.0).abs() < 1e-9,
            "{os:?}"
        );
        keys_on(&mut s, os).tap("Shift+H");
        keys_on(&mut s, os).tap("Shift+Z");
        assert_eq!(s.core().arrangement().track_zoom(), 1.0, "reset");
        assert_eq!(
            s.core().arrangement().px_per_tick(),
            time,
            "not the time zoom"
        );
        assert_eq!(
            s.core().piano_roll().key_height(),
            keys0,
            "not the key zoom"
        );
        s.execute(AppCommand::TogglePianoRoll);
        s.set_focus(Pane::PianoRoll);
        keys_on(&mut s, os).tap("Shift+H");
        assert!(s.core().piano_roll().key_height() > keys0, "{os:?}");
        keys_on(&mut s, os).tap("Shift+G");
        keys_on(&mut s, os).tap("Shift+G");
        assert!(s.core().piano_roll().key_height() < keys0, "{os:?}");
        keys_on(&mut s, os).tap("Shift+Z");
        assert_eq!(s.core().piano_roll().key_height(), keys0);
        assert_eq!(
            s.core().arrangement().track_zoom(),
            1.0,
            "not the track zoom"
        );
    }
}

#[test]
fn the_vertical_zooms_are_saved_in_the_window_state_and_restored_at_start() {
    let dir = std::env::temp_dir().join(format!("atomik-zoom-ui-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("ui-state.json");
    let _ = std::fs::remove_file(&path);
    let make = |path: &PathBuf| {
        let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
        let mut s = Shell::new(
            AppCore::new(session),
            ShellOptions {
                os: Some(Os::Windows),
                profile: None,
                in_window_menu: true,
                settings_path: None,
                ui_state_path: Some(path.clone()),
                library_path: None,
                library_root: None,
            },
        );
        s.handle(&UiEvent::Resize(Size::new(1100.0, 800.0)));
        s
    };
    let mut s = make(&path);
    demo(&mut s);
    assert_eq!(s.ui_state().track_zoom, 1.0);
    s.set_focus(Pane::Arrangement);
    keys(&mut s).tap("Shift+H");
    keys(&mut s).tap("Shift+H");
    let zoom = s.core().arrangement().track_zoom();
    assert!(zoom > 1.5);
    assert_eq!(s.ui_state().track_zoom, zoom);
    s.execute(AppCommand::TogglePianoRoll);
    s.set_focus(Pane::PianoRoll);
    keys(&mut s).tap("Shift+H");
    let kh = s.core().piano_roll().key_height();
    let saved = atomik_ui_state(&path);
    assert!(
        (saved.track_zoom - zoom).abs() < 1e-12,
        "{saved:?} vs {zoom} {kh}"
    );
    assert!((saved.piano_roll_key_height - kh).abs() < 1e-12);
    // A new shell reads them back.
    let s2 = make(&path);
    assert!((s2.core().arrangement().track_zoom() - zoom).abs() < 1e-12);
    assert!((s2.core().piano_roll().key_height() - kh).abs() < 1e-12);
    let _ = std::fs::remove_dir_all(&dir);
}

fn atomik_ui_state(path: &Path) -> crate::ui_state::UiState {
    crate::ui_state::UiState::load(path)
}

#[test]
fn z_zooms_to_the_selection_and_f_follows_the_playhead() {
    let mut s = shell();
    demo(&mut s);
    s.set_focus(Pane::Arrangement);
    let ids = s.core().arrangement().selected_clips();
    assert!(ids.is_empty());
    s.core_mut()
        .arrangement_mut()
        .set_selected_clips(&[atomik_model::ClipId(3)]);
    keys(&mut s).tap("Z");
    let a = s.core().arrangement();
    let r = a.clip_rect(atomik_model::ClipId(3)).unwrap();
    assert!(
        r.width() > 0.7 * (a.bounds().width() - 208.0),
        "the clip fills the view"
    );
    keys(&mut s).tap("Z");
    let a = s.core().arrangement();
    let r2 = a.clip_rect(atomik_model::ClipId(3)).unwrap();
    assert!(r2.width() < r.width(), "back to the whole song");
    keys(&mut s).tap("F");
    assert!(s.core().arrangement().follow());
    keys(&mut s).tap("F");
    assert!(!s.core().arrangement().follow());
}

#[test]
fn alt_x_splits_at_the_playhead_and_ctrl_k_quantizes_and_ctrl_arrows_transpose() {
    let mut s = shell();
    demo(&mut s);
    s.core_mut().transport_action(TransportAction::Locate(BAR));
    s.set_focus(Pane::Arrangement);
    s.core_mut()
        .arrangement_mut()
        .set_selected_clips(&[atomik_model::ClipId(1)]);
    let before = s.core().doc().song.tracks[0].lanes[0].clips.len();
    keys(&mut s).tap("Alt+X");
    assert_eq!(
        s.core().doc().song.tracks[0].lanes[0].clips.len(),
        before + 1
    );
    // Ctrl+E is create effect now, not split.
    keys(&mut s).tap("Escape");
    // The piano roll: Ctrl+Up transposes the selected notes, Shift makes it an octave.
    s.core_mut().open_piano_roll(atomik_model::ClipId(3));
    s.set_focus(Pane::PianoRoll);
    s.core_mut().piano_roll_mut().set_selected_notes(&[0]);
    let key_of = |s: &Shell<NullBackend>| -> u8 {
        let clip = s
            .core()
            .doc()
            .song
            .clip(atomik_model::ClipId(3))
            .unwrap()
            .2
            .clone();
        match clip.content {
            atomik_model::ClipContent::Notes(n) => n[0].key,
            _ => panic!(),
        }
    };
    let k0 = key_of(&s);
    keys(&mut s).tap("Cmd+Up");
    assert_eq!(key_of(&s), k0 + 1);
    keys(&mut s).tap("Cmd+Shift+Up");
    assert_eq!(key_of(&s), k0 + 13);
    keys(&mut s).tap("Cmd+Down");
    assert_eq!(key_of(&s), k0 + 12);
    // Plain Up does not transpose any more.
    keys(&mut s).tap("Up");
    assert_eq!(key_of(&s), k0 + 12);
    // Ctrl+K quantizes: shift one note off the grid, then quantize.
    s.core_mut().piano_roll_mut().set_snap(Snap::Beat);
    keys(&mut s).tap("Cmd+K");
    let tick_of = |s: &Shell<NullBackend>| -> i64 {
        let clip = s
            .core()
            .doc()
            .song
            .clip(atomik_model::ClipId(3))
            .unwrap()
            .2
            .clone();
        match clip.content {
            atomik_model::ClipContent::Notes(n) => n.iter().map(|x| x.tick).max().unwrap(),
            _ => panic!(),
        }
    };
    assert_eq!(tick_of(&s) % 15_360, 0, "every note is on the beat");
}

// ---------------------------------------------------------------- Enter, F8 and the panes

#[test]
fn enter_opens_the_selected_clip_and_returns_to_zero_otherwise() {
    let mut s = shell();
    demo(&mut s);
    s.core_mut()
        .transport_action(TransportAction::Locate(2 * BAR));
    s.set_focus(Pane::Arrangement);
    keys(&mut s).tap("Return");
    assert!(s.core().piano_roll_clip().is_none());
    assert_eq!(s.core().position(), 0, "nothing selected: return to zero");
    s.core_mut()
        .arrangement_mut()
        .set_selected_clips(&[atomik_model::ClipId(3)]);
    keys(&mut s).tap("Return");
    assert_eq!(s.core().piano_roll_clip(), Some(atomik_model::ClipId(3)));
    assert_eq!(s.focus(), Pane::PianoRoll);
    // In the open roll Enter goes on to return to zero.
    s.core_mut().transport_action(TransportAction::Locate(BAR));
    keys(&mut s).tap("Return");
    assert_eq!(s.core().position(), 0);
}

#[test]
fn f8_opens_the_selected_clip_and_closes_the_open_editor() {
    let mut s = shell();
    demo(&mut s);
    s.core_mut()
        .arrangement_mut()
        .set_selected_clips(&[atomik_model::ClipId(3)]);
    s.set_focus(Pane::Arrangement);
    keys(&mut s).tap("F8");
    assert_eq!(s.core().piano_roll_clip(), Some(atomik_model::ClipId(3)));
    keys(&mut s).tap("F8");
    assert!(s.core().piano_roll_clip().is_none());
    keys(&mut s).tap("Shift+Tab");
    assert!(
        s.core().piano_roll_clip().is_some(),
        "Shift+Tab is the same command"
    );
    // Without any clip it says so.
    let mut empty = shell();
    keys(&mut empty).tap("F8");
    assert!(empty.core().piano_roll_clip().is_none());
    assert!(empty.core().last_rejection().is_some());
}

#[test]
fn the_pane_keys_toggle_the_browser_console_rack_and_sequencer_only_layout() {
    let mut s = shell();
    keys(&mut s).tap("F3");
    assert!(s.core().view().browser_visible());
    keys(&mut s).tap("F3");
    assert!(!s.core().view().browser_visible());
    keys(&mut s).tap("Cmd+F");
    assert!(s.core().view().browser_visible(), "Ctrl+F is the alias");
    keys(&mut s).tap("Cmd+B");
    assert!(
        !s.core().view().browser_visible(),
        "Ctrl+B stays a secondary"
    );
    keys(&mut s).tap("F5");
    assert!(s.menu_state().console);
    keys(&mut s).tap("F5");
    assert!(!s.menu_state().console);
    // F6 hides the rack: the song area takes the space; again restores it.
    let rack_h = s.layout().rack.height();
    assert!(rack_h > 100.0);
    keys(&mut s).tap("F6");
    assert_eq!(s.layout().rack.height(), 0.0);
    assert!(s.layout().song.unwrap().height() > 300.0);
    keys(&mut s).tap("F6");
    assert!((s.layout().rack.height() - rack_h).abs() < 1e-6);
    // F7: sequencer only (no rack, no console); again restores the previous layout.
    keys(&mut s).tap("F5");
    assert!(s.menu_state().console);
    keys(&mut s).tap("F7");
    assert_eq!(s.layout().rack.height(), 0.0);
    assert!(!s.menu_state().console);
    assert_eq!(s.lower_pane(), Pane::Arrangement);
    keys(&mut s).tap("F7");
    assert!((s.layout().rack.height() - rack_h).abs() < 1e-6);
    assert!(s.menu_state().console, "the console is back");
}

#[test]
fn f7_from_a_hidden_rack_brings_the_rack_back_and_the_menu_follows_the_layout() {
    let mut s = shell();
    assert!(s.menu_state().rack && !s.menu_state().sequencer_only);
    keys(&mut s).tap("F6");
    assert!(!s.menu_state().rack && s.menu_state().sequencer_only);
    keys(&mut s).tap("F7");
    assert!(s.menu_state().rack && !s.menu_state().sequencer_only);
    assert!(s.layout().rack.height() > 0.0);
}

#[test]
fn the_rack_flag_is_saved_in_workflow_json_and_restored() {
    let dir = temp_dir("rackflag");
    let settings = dir.join("settings.json");
    {
        let mut s = shell_with(Os::Windows, None, Some(settings.clone()));
        keys(&mut s).tap("F6");
        assert_eq!(s.layout().rack.height(), 0.0);
    }
    let saved = load_workflow(&dir.join(WORKFLOW_FILE)).value;
    assert!(!saved.rack_visible);
    let s = shell_with(Os::Windows, None, Some(settings));
    assert_eq!(s.layout().rack.height(), 0.0, "restored at start");
}

#[test]
fn k_cycles_the_cables_and_the_browser_search_key_opens_the_browser() {
    let mut s = shell();
    let before = s.core().view().cable_visibility();
    keys(&mut s).tap("K");
    let a = s.core().view().cable_visibility();
    keys(&mut s).tap("K");
    let b = s.core().view().cable_visibility();
    keys(&mut s).tap("K");
    assert_eq!(
        s.core().view().cable_visibility(),
        before,
        "three presses cycle back"
    );
    assert!(a != before && b != a);
    keys(&mut s).tap("Cmd+Shift+F");
    assert!(s.core().view().browser_visible() && s.core().view().is_text_owner());
}

#[test]
fn macos_pane_secondaries_use_command_option_and_the_physical_digit() {
    let mut s = shell_for(Os::Mac);
    keys_on(&mut s, Os::Mac).tap("Cmd+Alt+Digit6");
    assert_eq!(s.layout().rack.height(), 0.0, "Cmd+Opt+6 is F6");
    keys_on(&mut s, Os::Mac).tap("Cmd+Alt+Digit6");
    assert!(s.layout().rack.height() > 0.0);
    keys_on(&mut s, Os::Mac).tap("Cmd+Alt+Digit5");
    assert!(s.menu_state().console);
    keys_on(&mut s, Os::Mac).tap("Cmd+Alt+Digit3");
    assert!(s.core().view().browser_visible());
    // Ctrl is not Cmd on macOS.
    let mut s = shell_for(Os::Mac);
    keys_on(&mut s, Os::Mac).tap("Ctrl+I");
    assert_eq!(s.core().view().browser_create(), None);
    keys_on(&mut s, Os::Mac).tap("Cmd+I");
    assert_eq!(
        s.core().view().browser_create(),
        Some(CreateKind::Instrument)
    );
}

// ---------------------------------------------------------------- create keys

#[test]
fn ctrl_i_opens_the_create_browser_and_enter_adds_the_device() {
    let mut s = shell();
    keys(&mut s).tap("Cmd+I");
    assert_eq!(
        s.core().view().browser_create(),
        Some(CreateKind::Instrument)
    );
    assert!(s.core().view().is_text_owner());
    for c in ["P", "O", "L", "Y"] {
        keys(&mut s).tap(c);
    }
    assert!(!s.core().is_playing() && !s.core().is_recording());
    let before = s.core().doc().devices.len();
    keys(&mut s).tap("Return");
    let doc = s.core().doc();
    assert!(doc.devices.len() > before);
    assert!(doc.devices.iter().any(|d| d.type_id == "atomik.poly-synth"));
    assert!(
        !s.core().view().browser_visible(),
        "created: the browser closes"
    );
}

#[test]
fn ctrl_e_creates_an_effect_escape_cancels_and_insert_is_a_second_binding() {
    let mut s = shell();
    keys(&mut s).tap("Cmd+E");
    assert_eq!(s.core().view().browser_create(), Some(CreateKind::Effect));
    for c in ["D", "E", "L", "A", "Y"] {
        keys(&mut s).tap(c);
    }
    let before = s.core().doc().devices.len();
    keys(&mut s).tap("Escape");
    assert!(!s.core().view().browser_visible());
    assert_eq!(s.core().doc().devices.len(), before, "Esc creates nothing");
    keys(&mut s).tap("Insert");
    assert_eq!(
        s.core().view().browser_create(),
        Some(CreateKind::Instrument)
    );
    // Create keeps the browser open when the preference says so.
    let mut s = shell();
    s.workflow.create_keeps_browser_open = true;
    s.apply_workflow_to_views();
    keys(&mut s).tap("Cmd+E");
    for c in ["D", "E", "L", "A", "Y"] {
        keys(&mut s).tap(c);
    }
    keys(&mut s).tap("Return");
    assert!(s.core().view().browser_visible());
}

#[test]
fn create_keys_show_a_hidden_rack_and_focus_the_filter() {
    let mut s = shell();
    s.set_focus(Pane::Arrangement);
    keys(&mut s).tap("F6");
    assert_eq!(s.layout().rack.height(), 0.0);
    keys(&mut s).tap("Cmd+I");
    assert!(
        s.layout().rack.height() > 0.0,
        "the create browser needs the rack"
    );
    assert_eq!(s.focus(), Pane::Rack);
    assert!(s.core().view().is_text_owner());
}

#[test]
fn ctrl_t_adds_an_audio_track_and_ctrl_shift_m_opens_the_channel_picker() {
    let mut s = shell();
    let tracks = s.core().doc().song.tracks.len();
    keys(&mut s).tap("Cmd+T");
    assert_eq!(s.core().doc().song.tracks.len(), tracks + 1);
    assert!(s.core().doc().song.tracks.last().unwrap().is_audio());
    keys(&mut s).tap("Cmd+Z");
    assert_eq!(s.core().doc().song.tracks.len(), tracks, "one undo step");
    assert!(!s.menu_state().console);
    keys(&mut s).tap("Cmd+Shift+M");
    assert!(
        s.menu_state().console,
        "the console shows with its add-channel picker"
    );
}

// ---------------------------------------------------------------- remaps really apply

fn remap(s: &mut Shell<NullBackend>, bindings: Vec<BindingDef>, removed: Vec<RemovedDef>) {
    let user = UserKeymap {
        bindings,
        removed,
        ..UserKeymap::default()
    };
    s.set_user_keymap(user, false);
}

#[test]
fn a_context_binding_of_a_view_command_shadows_a_global_binding_of_the_same_chord() {
    let mut s = shell();
    demo(&mut s);
    // C is the global metronome toggle; bind it to select all in the sequencer.
    remap(
        &mut s,
        vec![BindingDef {
            command: "edit.select_all".into(),
            arg: None,
            context: Context::Sequencer,
            chords: vec!["C".into()],
            os: None,
        }],
        vec![],
    );
    s.set_focus(Pane::Arrangement);
    keys(&mut s).tap("C");
    assert!(
        s.core().arrangement().selected_clips().len() >= 2,
        "select all ran"
    );
    assert!(!s.core().metronome(), "the global binding did not");
    s.set_focus(Pane::Rack);
    keys(&mut s).tap("C");
    assert!(
        s.core().metronome(),
        "in the rack the global binding still runs"
    );
}

#[test]
fn an_unbound_chord_does_nothing_in_a_view_that_used_to_handle_it_itself() {
    let mut s = shell();
    demo(&mut s);
    remap(
        &mut s,
        vec![],
        vec![
            RemovedDef {
                command: "transport.play_stop".into(),
                arg: None,
                context: None,
                chord: "Space".into(),
            },
            RemovedDef {
                command: "edit.delete".into(),
                arg: None,
                context: None,
                chord: "Delete".into(),
            },
        ],
    );
    s.set_focus(Pane::Arrangement);
    s.core_mut()
        .arrangement_mut()
        .set_selected_clips(&[atomik_model::ClipId(1)]);
    let clips = s.core().doc().song.tracks[0].lanes[0].clips.len();
    keys(&mut s).tap("Space");
    keys(&mut s).tap("Delete");
    assert!(!s.core().is_playing(), "Space is unbound");
    assert_eq!(
        s.core().doc().song.tracks[0].lanes[0].clips.len(),
        clips,
        "Delete is unbound"
    );
    keys(&mut s).tap("Backspace");
    assert!(
        s.core().doc().song.tracks[0].lanes[0].clips.len() < clips,
        "Backspace is still bound to delete"
    );
    s.core_mut().open_piano_roll(atomik_model::ClipId(1));
    s.set_focus(Pane::PianoRoll);
    keys(&mut s).tap("Space");
    assert!(!s.core().is_playing());
}

#[test]
fn the_legacy_profile_restores_the_pre_4w_keys() {
    let mut s = shell_with(Os::Windows, Some(Profile::Legacy), None);
    demo(&mut s);
    s.set_focus(Pane::Arrangement);
    // R is record (refused: no armed track), Q is not a tool.
    keys(&mut s).tap("R");
    assert!(s.core().last_rejection().is_some_and(|m| m.contains("arm")));
    assert_eq!(s.core().arrangement().tool(), Tool::Select);
    keys(&mut s).tap("W");
    assert_eq!(s.core().arrangement().tool(), Tool::Select, "no tool keys");
    // Ctrl+E splits at the playhead.
    s.core_mut().transport_action(TransportAction::Locate(BAR));
    s.core_mut()
        .arrangement_mut()
        .set_selected_clips(&[atomik_model::ClipId(1)]);
    let n = s.core().doc().song.tracks[0].lanes[0].clips.len();
    keys(&mut s).tap("Cmd+E");
    assert_eq!(s.core().doc().song.tracks[0].lanes[0].clips.len(), n + 1);
    // Ctrl+I imports audio.
    assert_eq!(
        keys(&mut s).tap("Cmd+I"),
        vec![ShellRequest::PickImportFiles]
    );
    // Enter returns to zero even with a clip selected.
    keys(&mut s).tap("Return");
    assert_eq!(s.core().position(), 0);
    assert!(s.core().piano_roll_clip().is_none());
    // The legacy roles keep Alt as snap off.
    assert!(s.roles().is_active(
        atomik_ui::keymap::Role::DisableSnap,
        alt(),
        atomik_ui::keymap::GestureClass::Hold
    ));
}

fn alt() -> Modifiers {
    Modifiers {
        alt: true,
        ..Modifiers::NONE
    }
}

#[test]
fn the_views_get_the_roles_of_the_profile_and_the_follow_preference() {
    let mut s = shell();
    assert!(!s.core().arrangement().follow());
    s.workflow.follow_song = true;
    s.apply_workflow_to_views();
    assert!(s.core().arrangement().follow() && s.core().piano_roll().follow());
    // Alt held: the toolbar shows the alternate tool through ModifiersChanged.
    s.handle(&UiEvent::ModifiersChanged { modifiers: alt() });
    assert_eq!(s.core().arrangement().shown_tool(), Tool::Pencil);
    s.handle(&UiEvent::ModifiersChanged {
        modifiers: Modifiers::NONE,
    });
    assert_eq!(s.core().arrangement().shown_tool(), Tool::Select);
    // The legacy profile has no alternate tool.
    let mut l = shell_with(Os::Windows, Some(Profile::Legacy), None);
    l.handle(&UiEvent::ModifiersChanged { modifiers: alt() });
    assert_eq!(l.core().arrangement().shown_tool(), Tool::Select);
}

#[test]
fn focus_loss_clears_a_held_alternate_tool() {
    let mut s = shell();
    s.handle(&UiEvent::ModifiersChanged { modifiers: alt() });
    assert_eq!(s.core().arrangement().shown_tool(), Tool::Pencil);
    s.focus_lost();
    assert_eq!(s.core().arrangement().shown_tool(), Tool::Select);
}

// ---------------------------------------------------------------- the one-time notice

#[test]
fn the_notice_of_moved_keys_shows_once_and_is_kept_in_workflow_json() {
    let dir = temp_dir("notice");
    let settings = dir.join("settings.json");
    let s = shell_with(Os::Windows, None, Some(settings.clone()));
    let text = s.core().last_rejection().unwrap_or_default().to_string();
    assert!(
        text.contains("Ctrl+Enter") && text.contains("Alt+X"),
        "{text}"
    );
    assert!(
        text.contains("Ctrl+Shift+I") && text.contains("Ctrl+I"),
        "{text}"
    );
    assert!(
        text.contains("razor") && text.to_lowercase().contains("legacy"),
        "{text}"
    );
    assert!(
        load_workflow(&dir.join(WORKFLOW_FILE))
            .value
            .keys_notice_seen
    );
    // The second start shows nothing.
    let s = shell_with(Os::Windows, None, Some(settings.clone()));
    assert_eq!(s.core().last_rejection(), None);
    // macOS names Cmd and Opt.
    let dir2 = temp_dir("notice-mac");
    let s = shell_with(Os::Mac, None, Some(dir2.join("settings.json")));
    let text = s.core().last_rejection().unwrap_or_default().to_string();
    assert!(
        text.contains("Cmd+Enter") && text.contains("Opt+X"),
        "{text}"
    );
}

#[test]
fn no_notice_for_the_legacy_profile_or_without_a_settings_folder() {
    let dir = temp_dir("notice-legacy");
    let s = shell_with(
        Os::Windows,
        Some(Profile::Legacy),
        Some(dir.join("settings.json")),
    );
    assert_eq!(s.core().last_rejection(), None);
    assert!(
        !load_workflow(&dir.join(WORKFLOW_FILE))
            .value
            .keys_notice_seen
    );
    let s = shell_for(Os::Windows);
    assert_eq!(s.core().last_rejection(), None, "nothing to remember it in");
}

// ---------------------------------------------------------------- additions

/// Auto-route acts on the selected rack devices only: a key press with no device
/// selected, or in a time view, changes no document, selection or transport state. (The
/// creation chain itself: `shell_create_tests.rs`.)
#[test]
fn auto_route_does_nothing_without_a_selected_device_or_in_a_time_view() {
    let mut s = shell();
    demo(&mut s);
    remap(
        &mut s,
        vec![BindingDef {
            command: "rack.auto_route".into(),
            arg: None,
            context: Context::Rack,
            chords: vec!["Alt+Y".into()],
            os: None,
        }],
        vec![],
    );
    s.core_mut()
        .arrangement_mut()
        .set_selected_clips(&[atomik_model::ClipId(1)]);
    let doc = s.core().doc().clone();
    let selected = s.core().arrangement().selected_clips();
    for pane in [Pane::Arrangement, Pane::PianoRoll, Pane::Rack] {
        s.set_focus(pane);
        keys(&mut s).tap("Alt+Y");
        assert_eq!(s.core().doc(), &doc, "{pane:?} changed the document");
        assert!(!s.core().is_playing(), "{pane:?}");
    }
    assert_eq!(s.core().arrangement().selected_clips(), selected);
}

/// Escape (the end of a gesture, the close of a popup) and text entry are what the views keep
/// after the migration: a rack browser filter still takes typed letters that are tool keys
/// elsewhere, and Escape closes the create browser, while the same letters in a time view are
/// commands.
#[test]
fn escape_and_text_entry_stay_in_the_views_after_the_migration() {
    let mut s = shell();
    demo(&mut s);
    s.set_focus(Pane::Rack);
    keys(&mut s).tap("Cmd+I");
    assert!(s.core().view().browser_visible());
    for c in ["R", "W", "E", "Q", "S", "Z"] {
        keys(&mut s).tap(c);
    }
    assert_eq!(
        s.core().arrangement().tool(),
        Tool::Select,
        "the letters went to the filter, not to the tool commands"
    );
    assert!(s.core().view().browser_visible());
    keys(&mut s).tap("Escape");
    assert!(!s.core().view().browser_visible());
    s.set_focus(Pane::Arrangement);
    keys(&mut s).tap("R");
    assert_eq!(s.core().arrangement().tool(), Tool::Razor);
}

/// The Create and View menus show the chords of this OS: Command and Option on macOS (text,
/// never glyphs), Ctrl and Insert-free primaries on Linux.
#[test]
fn the_create_and_view_menus_show_the_chords_of_each_os() {
    for (os, create_instrument, import, record, editor) in [
        (Os::Mac, "Cmd+I", "Cmd+Opt+I", "Cmd+Enter", "F8"),
        (Os::Linux, "Ctrl+I", "Ctrl+Shift+I", "Ctrl+Enter", "F8"),
        (Os::Windows, "Ctrl+I", "Ctrl+Shift+I", "Ctrl+Enter", "F8"),
    ] {
        let s = shell_for(os);
        let menus = s.menu_model();
        let item = |title: &str, label: &str| {
            menus
                .iter()
                .find(|m| m.title == title)
                .unwrap()
                .items()
                .find(|i| i.label.starts_with(label))
                .unwrap_or_else(|| panic!("{title} > {label}"))
                .shortcut
                .clone()
        };
        assert_eq!(
            item("Create", "Create Instrument").as_deref(),
            Some(create_instrument),
            "{os:?}"
        );
        assert_eq!(
            item("Create", "Create Effect").as_deref(),
            Some(create_instrument.replace('I', "E").as_str()),
            "{os:?}"
        );
        assert_eq!(item("File", "Import Audio").as_deref(), Some(import));
        assert_eq!(item("Transport", "Record").as_deref(), Some(record));
        assert_eq!(item("View", "Clip Editor").as_deref(), Some(editor));
        assert_eq!(item("View", "Rack").as_deref(), Some("F6"));
    }
}

/// Mutation survivors: F8 opens the first selected *note* clip, skipping an
/// automation clip that comes earlier in the selection.
#[test]
fn f8_skips_selected_clips_that_are_not_note_clips() {
    let mut s = shell();
    demo(&mut s);
    s.core_mut()
        .arrangement_mut()
        .set_selected_clips(&[atomik_model::ClipId(2), atomik_model::ClipId(3)]);
    s.set_focus(Pane::Arrangement);
    keys(&mut s).tap("F8");
    assert_eq!(s.core().piano_roll_clip(), Some(atomik_model::ClipId(3)));
    keys(&mut s).tap("F8");
    // Only an automation clip selected: no note clip, so the default editor or a message.
    s.core_mut()
        .arrangement_mut()
        .set_selected_clips(&[atomik_model::ClipId(2)]);
    keys(&mut s).tap("F8");
    assert_ne!(
        s.core().piano_roll_clip(),
        Some(atomik_model::ClipId(2)),
        "an automation clip has no piano roll"
    );
}

/// Mutation survivors: the numpad tempo keys stop at the limits of the model and do
/// not add an undo step when nothing changes.
#[test]
fn the_tempo_keys_stop_at_the_limits() {
    let mut s = shell();
    demo(&mut s);
    let set = |s: &mut Shell<NullBackend>, bpm: f64| {
        let mut map = s.core().doc().song.tempo.clone();
        map[0].bpm = bpm;
        s.core_mut()
            .apply_command(Command::Song(SongCommand::SetTempoMap(map)))
            .unwrap();
    };
    set(&mut s, atomik_model::TEMPO_MAX - 0.5);
    keys(&mut s).tap("NumpadAdd");
    assert_eq!(s.core().doc().song.tempo[0].bpm, atomik_model::TEMPO_MAX);
    keys(&mut s).tap("NumpadAdd");
    assert_eq!(s.core().doc().song.tempo[0].bpm, atomik_model::TEMPO_MAX);
    assert!(s.core().last_rejection().is_none(), "no refused command");
    set(&mut s, atomik_model::TEMPO_MIN + 0.5);
    keys(&mut s).tap("NumpadSub");
    assert_eq!(s.core().doc().song.tempo[0].bpm, atomik_model::TEMPO_MIN);
    keys(&mut s).tap("NumpadSub");
    assert_eq!(s.core().doc().song.tempo[0].bpm, atomik_model::TEMPO_MIN);
}

/// Mutation survivors: F6 never hides the song area, even when it was switched off
/// before: the space the rack gave up is not left empty.
#[test]
fn f6_shows_the_song_area_when_it_was_switched_off() {
    let mut s = shell();
    s.execute(AppCommand::ToggleSong);
    assert!(s.layout().song.is_none(), "the song area is off");
    keys(&mut s).tap("F6");
    assert_eq!(s.layout().rack.height(), 0.0);
    assert!(
        s.layout().song.is_some_and(|r| r.height() > 300.0),
        "the song area took the space"
    );
}
