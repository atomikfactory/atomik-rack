//! The energy lane (View > Energy Lane, View > Refresh Energy Lane) and Edit > Find Repetition in
//! the shell: the menu rows, the saved view option, the measurement that starts when the lane is
//! turned on, the repetition panel with its buttons as one undo step each, and closing with Esc.

use super::*;
use crate::session::NullBackend;
use atomik_core::PPQ;
use atomik_model::song::flatten_track_notes;
use atomik_model::{
    Clip, ClipContent, ClipId, Command, CreateOptions, CreateSource, DeviceKey, Note, SongCommand,
    WorkflowCommand,
};
use atomik_ui::{Key, Modifiers, PointerButton};

const BAR: i64 = 4 * PPQ;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn shell() -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    shell_of(AppCore::new(session))
}

fn shell_of(core: AppCore<LiveSession<NullBackend>>) -> Shell<NullBackend> {
    let mut s = Shell::new(
        core,
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

fn pattern_clip(start: i64) -> Clip {
    Clip {
        id: ClipId(0),
        start,
        length: BAR,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        param_lanes: Vec::new(),
        midi: Default::default(),
        groove: None,
        follow_song_key: true,
        controllers: Vec::new(),
        name: None,
        color: None,
        grid: None,
        content: ClipContent::Notes(
            vec![
                Note::new(0, PPQ / 2, 36, 0.9),
                Note::new(PPQ, PPQ / 2, 38, 0.8),
                Note::new(2 * PPQ, PPQ / 2, 36, 0.9),
                Note::new(3 * PPQ, PPQ / 2, 38, 0.7),
            ]
            .into(),
        ),
    }
}

/// A synth `drums` that plays one pattern in four clips, one per bar.
fn with_a_repeated_pattern(s: &mut Shell<NullBackend>) {
    let core = s.core_mut();
    core.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
        index: usize::MAX,
        source: CreateSource::Type("atomik.poly-synth".into()),
        key: Some(key("drums")),
        options: CreateOptions::default(),
    }))
    .unwrap();
    for bar in 0..4 {
        core.apply_command(Command::Song(SongCommand::AddClip {
            device: key("drums"),
            lane: 0,
            clip: pattern_clip(bar * BAR),
        }))
        .unwrap();
    }
}

fn played(s: &Shell<NullBackend>) -> usize {
    let song = &s.core().doc().song;
    flatten_track_notes(song, song.track(&key("drums")).unwrap()).len()
}

fn click(s: &mut Shell<NullBackend>, p: atomik_ui::kurbo::Point) {
    s.handle(&UiEvent::PointerDown {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    });
    s.handle(&UiEvent::PointerUp {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
    });
}

fn esc(s: &mut Shell<NullBackend>) {
    s.handle(&UiEvent::Key {
        key: Key::Escape,
        modifiers: Modifiers::NONE,
        pressed: true,
    });
}

fn menu_row(s: &Shell<NullBackend>, menu: &str, command: AppCommand) -> (String, bool) {
    let m = s
        .menu_model()
        .into_iter()
        .find(|m| m.title == menu)
        .unwrap();
    let row = m.items().find(|i| i.command == command).expect("the row");
    (row.label.to_string(), row.enabled)
}

#[test]
fn the_menus_have_the_lane_and_the_finder() {
    let s = shell();
    let (label, enabled) = menu_row(&s, "View", AppCommand::ToggleEnergyLane);
    assert_eq!(label, "Energy Lane");
    assert!(enabled);
    let (label, enabled) = menu_row(&s, "View", AppCommand::RefreshEnergyLane);
    assert_eq!(label, "Refresh Energy Lane");
    assert!(!enabled, "nothing to refresh while the lane is off");
    let (label, enabled) = menu_row(&s, "Analyze", AppCommand::FindRepetition);
    assert_eq!(label, "Find Repetition\u{2026}");
    assert!(enabled);
    assert_eq!(
        AppCommand::ToggleEnergyLane.command_id(),
        Some(("view.toggle_energy_lane", None))
    );
    assert_eq!(
        AppCommand::from_command("view.refresh_energy_lane", None),
        Some(AppCommand::RefreshEnergyLane)
    );
    assert_eq!(
        AppCommand::from_command("sequencer.find_repetition", None),
        Some(AppCommand::FindRepetition)
    );
}

#[test]
fn the_lane_is_off_by_default_and_the_command_toggles_it_and_the_saved_state() {
    let mut s = shell();
    assert!(!s.core().arrangement().energy_lane_visible());
    assert!(!s.ui_state().show_energy_lane);
    s.execute(AppCommand::ToggleEnergyLane);
    assert!(s.core().arrangement().energy_lane_visible());
    assert!(s.ui_state().show_energy_lane);
    assert!(menu_row(&s, "View", AppCommand::RefreshEnergyLane).1);
    s.execute(AppCommand::ToggleEnergyLane);
    assert!(!s.core().arrangement().energy_lane_visible());
    assert!(!s.ui_state().show_energy_lane);
}

#[test]
fn turning_the_lane_on_measures_the_song_and_refresh_measures_again() {
    let mut s = shell();
    with_a_repeated_pattern(&mut s);
    s.execute(AppCommand::ToggleEnergyLane);
    assert!(s.core().energy_running(), "no result yet: it measures");
    s.core_mut().wait_background();
    s.frame();
    let (map, stale) = s
        .core()
        .arrangement()
        .energy_map()
        .expect("the lane has it");
    assert!(!map.sections.is_empty());
    assert!(!stale);
    // Turning it off and on again keeps the result: nothing is measured twice.
    s.execute(AppCommand::ToggleEnergyLane);
    s.execute(AppCommand::ToggleEnergyLane);
    assert!(!s.core().energy_running());
    // Refresh measures again.
    s.execute(AppCommand::RefreshEnergyLane);
    assert!(s.core().energy_running());
    s.core_mut().wait_background();
}

#[test]
fn refresh_does_nothing_while_the_lane_is_hidden() {
    let mut s = shell();
    with_a_repeated_pattern(&mut s);
    s.execute(AppCommand::RefreshEnergyLane);
    assert!(!s.core().energy_running());
}

#[test]
fn a_song_that_cannot_be_measured_says_why_in_the_status_line() {
    let mut doc = atomik_model::new_project_doc();
    doc.console.master = None;
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = shell_of(AppCore::with_doc(session, doc, None));
    s.execute(AppCommand::ToggleEnergyLane);
    assert!(!s.core().energy_running());
    assert!(
        s.core()
            .last_rejection()
            .is_some_and(|m| m.contains("master")),
        "{:?}",
        s.core().last_rejection()
    );
}

#[test]
fn find_repetition_opens_the_panel_on_the_ideas_and_esc_closes_it() {
    let mut s = shell();
    with_a_repeated_pattern(&mut s);
    assert!(!s.repetition_open());
    s.execute(AppCommand::FindRepetition);
    assert!(s.repetition_open());
    let data = s
        .repetition_view()
        .unwrap()
        .data()
        .expect("searched")
        .clone();
    assert_eq!(data.rows.len(), 1);
    assert_eq!(data.rows[0].repeats, 4);
    esc(&mut s);
    assert!(!s.repetition_open());
}

#[test]
fn a_song_without_repeats_opens_an_empty_panel() {
    let mut s = shell();
    s.execute(AppCommand::FindRepetition);
    let view = s.repetition_view().unwrap();
    assert!(view.data().is_some_and(|d| d.rows.is_empty()));
}

#[test]
fn a_variation_button_is_one_undo_step_and_then_says_done() {
    let mut s = shell();
    with_a_repeated_pattern(&mut s);
    s.execute(AppCommand::FindRepetition);
    assert_eq!(played(&s), 16);
    let button = s.repetition_view().unwrap().choice_rect(0, 0).center();
    click(&mut s, button);
    assert_eq!(played(&s), 15);
    let view = s.repetition_view().unwrap();
    let row = &view.data().unwrap().rows[0];
    assert!(row.choices[0].done);
    assert!(
        s.core()
            .last_rejection()
            .is_some_and(|m| m.contains("mute the last hit")),
        "{:?}",
        s.core().last_rejection()
    );
    // A second click on the same button changes nothing.
    click(&mut s, button);
    assert_eq!(played(&s), 15);
    // One undo brings the hit back.
    assert!(s.core_mut().undo());
    assert_eq!(played(&s), 16);
}

#[test]
fn a_click_on_the_row_locates_and_find_searches_again() {
    let mut s = shell();
    with_a_repeated_pattern(&mut s);
    s.execute(AppCommand::FindRepetition);
    // Move the playhead away, then click the row away from its buttons.
    s.core_mut()
        .transport_action(atomik_ui::TransportAction::Locate(3 * BAR));
    let row = s.repetition_view().unwrap().row_rect(0);
    click(
        &mut s,
        atomik_ui::kurbo::Point::new(row.x0 + 20.0, row.center().y),
    );
    assert_eq!(s.core().position(), 0);
    // Take the clips away; FIND lists nothing.
    for id in 1..=4u64 {
        s.core_mut()
            .apply_command(Command::Song(SongCommand::RemoveClip { id: ClipId(id) }))
            .unwrap();
    }
    let find = s.repetition_view().unwrap().find_rect().center();
    click(&mut s, find);
    let view = s.repetition_view().unwrap();
    assert!(view.data().unwrap().rows.is_empty());
    assert_eq!(
        view.note(),
        "Nothing repeats three times in a row unchanged"
    );
}
