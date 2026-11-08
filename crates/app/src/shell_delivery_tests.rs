//! The Delivery panel in the shell: the File and Edit menu rows and their command ids, the panel
//! opening on the right page and closing on Esc and CLOSE, a click on MEASURE rendering the master,
//! SET CEILING as one undo step, the page tabs, and the panel following the window.

use super::*;
use crate::session::NullBackend;
use atomik_core::PPQ;
use atomik_model::{
    Clip, ClipContent, CreateOptions, CreateSource, Note, SongCommand, TARGETS, WorkflowCommand,
};
use atomik_ui::{DeliveryPage, Key, Modifiers, PointerButton};

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

/// A shell whose song has a synth that plays two beats.
fn song_shell() -> Shell<NullBackend> {
    let mut s = shell();
    let core = s.core_mut();
    core.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
        index: usize::MAX,
        source: CreateSource::Type("atomik.poly-synth".into()),
        key: Some(DeviceKey("lead".into())),
        options: CreateOptions::default(),
    }))
    .unwrap();
    core.apply_command(Command::Song(SongCommand::AddClip {
        device: DeviceKey("lead".into()),
        lane: 0,
        clip: Clip {
            id: ClipId(0),
            start: 0,
            length: 4 * PPQ,
            offset: 0,
            loop_length: None,
            muted: false,
            scale: None,
            param_lanes: Vec::new(),
            midi: Default::default(),
            groove: None,
            controllers: Vec::new(),
            name: None,
            color: None,
            grid: None,
            content: ClipContent::Notes(
                vec![Note::new(0, PPQ, 64, 0.8), Note::new(PPQ, PPQ, 67, 0.8)].into(),
            ),
        },
    }))
    .unwrap();
    s
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

fn escape(s: &mut Shell<NullBackend>) {
    s.handle(&UiEvent::Key {
        key: Key::Escape,
        modifiers: Modifiers::NONE,
        pressed: true,
    });
}

/// The label and enabled flag of the row of `command` in the menu named `menu`.
fn row(s: &Shell<NullBackend>, menu: &str, command: AppCommand) -> Option<(String, bool)> {
    s.menu_model()
        .iter()
        .find(|m| m.title == menu)?
        .items()
        .find(|i| i.command == command)
        .map(|i| (i.label.to_string(), i.enabled))
}

#[test]
fn the_menu_rows_exist_and_the_commands_have_ids() {
    let s = shell();
    assert_eq!(
        row(&s, "Analyze", AppCommand::DeliveryTargets),
        Some(("Delivery Targets\u{2026}".to_owned(), true))
    );
    // Peak Surgeon needs an audio clip: its Edit row is off without one.
    assert_eq!(
        row(&s, "Edit", AppCommand::PeakSurgeon),
        Some(("Peak Surgeon\u{2026}".to_owned(), false))
    );
    for (id, command) in [
        ("file.delivery_targets", AppCommand::DeliveryTargets),
        ("sequencer.peak_surgeon", AppCommand::PeakSurgeon),
    ] {
        assert_eq!(command.command_id(), Some((id, None)));
        assert_eq!(AppCommand::from_command(id, None), Some(command));
        assert!(atomik_ui::keymap::Registry::bundled().get(id).is_some());
    }
}

#[test]
fn the_commands_open_the_panel_on_their_page_and_esc_closes_it() {
    let mut s = shell();
    assert!(!s.delivery_open());
    s.execute(AppCommand::DeliveryTargets);
    assert!(s.delivery_open());
    assert_eq!(
        s.delivery_view().unwrap().data().page,
        DeliveryPage::Targets
    );
    escape(&mut s);
    assert!(!s.delivery_open());
    s.execute(AppCommand::PeakSurgeon);
    assert_eq!(s.delivery_view().unwrap().data().page, DeliveryPage::Peaks);
    let close = s.delivery_view().unwrap().close_rect().center();
    click(&mut s, close);
    assert!(!s.delivery_open());
}

#[test]
fn the_tabs_switch_the_page_and_the_rows_choose_a_target() {
    let mut s = shell();
    s.execute(AppCommand::DeliveryTargets);
    let tab = s
        .delivery_view()
        .unwrap()
        .tab_rect(DeliveryPage::Peaks)
        .center();
    click(&mut s, tab);
    assert_eq!(s.delivery_view().unwrap().data().page, DeliveryPage::Peaks);
    let tab = s
        .delivery_view()
        .unwrap()
        .tab_rect(DeliveryPage::Targets)
        .center();
    click(&mut s, tab);
    let third = s.delivery_view().unwrap().row_rect(3).center();
    click(&mut s, third);
    assert_eq!(s.delivery_view().unwrap().data().targets.chosen, 3);
}

#[test]
fn measure_renders_the_master_and_set_ceiling_is_one_undo_step() {
    let mut s = song_shell();
    s.execute(AppCommand::DeliveryTargets);
    let measure = s.delivery_view().unwrap().main_rect().center();
    click(&mut s, measure);
    assert!(s.core().delivery_running());
    s.core_mut().wait_background();
    s.frame();
    let data = s.delivery_view().unwrap().data().targets.clone();
    assert!(
        data.master.as_deref().is_some_and(|m| m.contains("LUFS")),
        "{data:?}"
    );
    assert!(data.rows.iter().all(|r| !r.effect.is_empty()));
    // SET CEILING sets the project's target to the chosen row.
    let depth = s.core().document().undo_depth();
    let set = s.delivery_view().unwrap().action_rect().center();
    click(&mut s, set);
    assert_eq!(
        s.core().doc().delivery.as_ref().map(|d| d.name.as_str()),
        Some(TARGETS[1].name)
    );
    assert_eq!(s.core().document().undo_depth(), depth + 1);
    s.frame();
    assert!(s.delivery_view().unwrap().data().targets.rows[1].current);
}

#[test]
fn the_panel_follows_the_window() {
    let mut s = shell();
    s.execute(AppCommand::DeliveryTargets);
    s.handle(&UiEvent::Resize(Size::new(700.0, 600.0)));
    let p = s.delivery_view().unwrap().panel();
    assert!(p.x1 <= 700.0 && p.y1 <= 600.0, "{p:?}");
    s.handle(&UiEvent::ScaleChanged(2.0));
    assert!(s.delivery_open());
}
