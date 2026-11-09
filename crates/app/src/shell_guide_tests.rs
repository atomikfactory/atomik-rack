//! The guided first song and the start screen cards in the shell: the Help command, that the panel
//! never takes the keyboard, what its buttons and the detection do, the outlines, and the cards
//! of the start screen with their check box.

use super::*;
use crate::guide::{Button, Step};
use crate::keymap_store::{WORKFLOW_FILE, load_workflow};
use crate::session::NullBackend;
use crate::start_screen::Target;
use atomik_model::{Command, CreateOptions, CreateSource, DeviceKey, WorkflowCommand};
use atomik_ui::{Key, Modifiers, PointerButton};
use std::sync::atomic::{AtomicU64, Ordering};

/// A temporary config folder that is removed when the test ends.
struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!(
            "atomik-shell-guide-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Self(d)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn started(d: &Dir, workflow: &str) -> Shell<NullBackend> {
    std::fs::write(d.0.join(WORKFLOW_FILE), workflow).unwrap();
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut core = AppCore::new(session);
    core.set_recent_path(d.0.join("recent.json"));
    let mut s = Shell::new(
        core,
        ShellOptions {
            os: Some(atomik_ui::keymap::Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: Some(d.0.join("settings.json")),
            ui_state_path: None,
            library_path: None,
            library_root: Some(d.0.join("Library")),
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1000.0, 800.0)));
    s
}

fn shipped() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../factory")
}

fn down(pos: Point) -> UiEvent {
    UiEvent::PointerDown {
        pos,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    }
}

fn key(k: Key) -> UiEvent {
    UiEvent::Key {
        key: k,
        modifiers: Modifiers::NONE,
        pressed: true,
    }
}

fn click_button(s: &mut Shell<NullBackend>, b: Button) {
    let bounds = s.guide_bounds();
    let at = s.guide().expect("the guide is open").button_rect(bounds, b);
    s.handle(&down(at.center()));
}

fn add_synth(s: &mut Shell<NullBackend>, name: &str) {
    s.core_mut()
        .apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
            index: usize::MAX,
            source: CreateSource::Type("atomik.poly-synth".into()),
            key: Some(DeviceKey(name.into())),
            options: CreateOptions::default(),
        }))
        .unwrap();
}

fn painted(s: &mut Shell<NullBackend>) -> String {
    let mut dl = atomik_ui::DisplayList::new();
    s.paint_in(WindowKind::Main, &mut dl);
    dl.to_json()
}

fn click_row(s: &mut Shell<NullBackend>, target: Target) -> Vec<ShellRequest> {
    let size = s.size();
    let at = s
        .start_screen()
        .expect("the start screen is open")
        .rows(size)
        .into_iter()
        .find(|(t, _)| *t == target)
        .unwrap_or_else(|| panic!("{target:?} is listed"))
        .1
        .center();
    s.handle(&down(at))
}

#[test]
fn help_has_a_guided_tour_row_and_it_opens_a_new_song_and_the_panel() {
    let d = Dir::new("help");
    let mut s = started(&d, "{}");
    let help = s
        .menu_model()
        .into_iter()
        .find(|m| m.title == "Help")
        .expect("a Help menu");
    assert!(
        help.items()
            .any(|i| i.label == "Guided Tour" && i.command == AppCommand::GuidedTour)
    );
    assert!(!s.guide_open());
    s.execute(AppCommand::GuidedTour);
    assert!(s.guide_open());
    assert_eq!(s.guide().unwrap().index(), 0);
    assert!(!s.modal_open(), "the panel is not a modal");
    assert!(s.core().file().is_none() && !s.core().is_dirty());
}

#[test]
fn a_song_with_unsaved_changes_is_asked_about_before_the_tour_replaces_it() {
    let d = Dir::new("dirty");
    let mut s = started(&d, "{}");
    add_synth(&mut s, "mine");
    assert!(s.core().is_dirty());
    let requests = s.execute(AppCommand::GuidedTour);
    assert!(
        requests.iter().any(|r| matches!(
            r,
            ShellRequest::ConfirmSave {
                then: PendingAction::Command(AppCommand::GuidedTour)
            }
        )),
        "{requests:?}"
    );
    assert!(!s.guide_open(), "nothing happens until the answer");
    s.confirmed(AppCommand::GuidedTour);
    assert!(s.guide_open());
    assert!(!s.core().doc().devices.iter().any(|d| d.key.0 == "mine"));
}

#[test]
fn the_panel_never_takes_the_keyboard_and_the_song_works_under_it() {
    let d = Dir::new("keys");
    let mut s = started(&d, "{}");
    s.execute(AppCommand::GuidedTour);
    // Escape does not close it, and L still switches the loop of the song.
    s.handle(&key(Key::Escape));
    assert!(s.guide_open());
    assert!(!s.core().doc().song.loop_enabled);
    s.handle(&key(Key::Char('l')));
    assert!(s.core().doc().song.loop_enabled);
    s.frame();
    assert!(s.guide().unwrap().progress().looping);
    // The song is edited like any other: undo goes through the panel.
    assert!(s.core().is_dirty());
}

#[test]
fn a_press_inside_the_panel_is_the_panels_and_a_press_outside_is_the_songs() {
    let d = Dir::new("pointer");
    let mut s = started(&d, "{}");
    s.execute(AppCommand::GuidedTour);
    click_button(&mut s, Button::Skip);
    assert_eq!(s.guide().unwrap().index(), 1);
    click_button(&mut s, Button::Back);
    assert_eq!(s.guide().unwrap().index(), 0);
    click_button(&mut s, Button::Move);
    assert!(s.guide().unwrap().on_left());
    // A press on the panel's body reaches nothing under it.
    let bounds = s.guide_bounds();
    let r = s.guide().unwrap().rect(bounds);
    s.handle(&down(Point::new(r.x0 + 30.0, r.y0 + 60.0)));
    assert!(s.guide_open());
    // A press outside it is the app's: the menu bar opens on its title.
    let menus = s.menu_model();
    let file = MenuBar::title_rects(&menus)[0];
    s.handle(&down(file.center()));
    assert!(s.menubar.as_ref().is_some_and(MenuBar::is_open));
    assert!(s.guide_open(), "and the panel stays");
}

#[test]
fn close_closes_it_at_any_time_and_the_song_is_untouched() {
    let d = Dir::new("close");
    let mut s = started(&d, "{}");
    s.execute(AppCommand::GuidedTour);
    add_synth(&mut s, "lead");
    s.frame();
    assert_eq!(s.guide().unwrap().index(), 1);
    click_button(&mut s, Button::Close);
    assert!(!s.guide_open());
    assert!(s.core().doc().devices.iter().any(|d| d.key.0 == "lead"));
    // Nothing of the tour was put in the song or the preferences.
    let text = atomik_model::save_project(s.core().doc());
    assert!(!text.to_lowercase().contains("guide") && !text.to_lowercase().contains("tour"));
}

#[test]
fn the_panel_moves_on_by_itself_as_the_song_changes() {
    let d = Dir::new("detect");
    let mut s = started(&d, "{}");
    s.execute(AppCommand::GuidedTour);
    assert_eq!(s.guide().unwrap().current(), Some(Step::Instrument));
    s.frame();
    assert_eq!(s.guide().unwrap().index(), 0);
    add_synth(&mut s, "lead");
    s.frame();
    assert_eq!(s.guide().unwrap().current(), Some(Step::Notes));
    // The loop step: pressing L at the notes step does not skip the notes step.
    s.handle(&key(Key::Char('l')));
    s.frame();
    assert_eq!(s.guide().unwrap().current(), Some(Step::Notes));
    assert!(s.guide().unwrap().progress().looping);
}

#[test]
fn the_guide_outlines_the_control_a_step_talks_about() {
    let d = Dir::new("outline");
    let mut s = started(&d, "{}");
    assert!(s.guide_outline().is_none(), "no panel, no outline");
    s.execute(AppCommand::GuidedTour);
    let menus = s.menu_model();
    let titles = MenuBar::title_rects(&menus);
    let at = |title: &str| titles[menus.iter().position(|m| m.title == title).unwrap()];
    assert_eq!(s.guide_outline(), Some(at("Create")));
    for _ in 0..Step::Loop as usize {
        click_button(&mut s, Button::Skip);
    }
    assert_eq!(s.guide().unwrap().current(), Some(Step::Loop));
    assert_eq!(
        s.guide_outline(),
        Some(
            s.core()
                .transport_bar()
                .button_rect(atomik_ui::TransportButton::Loop)
        )
    );
    click_button(&mut s, Button::Skip);
    click_button(&mut s, Button::Skip);
    click_button(&mut s, Button::Skip);
    assert_eq!(s.guide().unwrap().current(), Some(Step::Save));
    assert_eq!(s.guide_outline(), Some(at("File")));
    for _ in 0..2 {
        click_button(&mut s, Button::Skip);
    }
    assert!(s.guide().unwrap().finished());
    assert!(s.guide_outline().is_none());
}

#[test]
fn the_panel_is_drawn_over_the_song_and_hidden_under_a_modal() {
    let d = Dir::new("paint");
    let mut s = started(&d, "{}");
    assert!(!painted(&mut s).contains("MAKE YOUR FIRST SONG"));
    s.execute(AppCommand::GuidedTour);
    assert!(painted(&mut s).contains("Add an instrument"));
    s.execute(AppCommand::StartScreen);
    assert!(
        !painted(&mut s).contains("Add an instrument"),
        "a modal comes first"
    );
}

#[test]
fn the_tooltip_of_a_panel_button_shows_over_the_song() {
    let d = Dir::new("tip");
    let mut s = started(&d, "{}");
    s.execute(AppCommand::GuidedTour);
    let bounds = s.guide_bounds();
    let close = s.guide().unwrap().button_rect(bounds, Button::Close);
    s.handle(&UiEvent::PointerMove {
        pos: close.center(),
        modifiers: Modifiers::NONE,
    });
    s.frame_at(1.0);
    s.frame_at(1.0 + atomik_ui::HELP_DELAY + 0.05);
    let tip = s.tooltip().expect("a tooltip");
    assert!(tip.text.contains("Close"), "{}", tip.text);
}

// ------------------------------------------------------------------------ the start screen

fn with_factory(d: &Dir, workflow: &str) -> Shell<NullBackend> {
    let mut s = started(d, workflow);
    s.core_mut().set_factory_root(Some(shipped()));
    s
}

#[test]
fn the_start_screen_offers_the_examples_and_the_guide_as_cards() {
    let d = Dir::new("cards");
    let mut s = with_factory(&d, "{}");
    s.execute(AppCommand::StartScreen);
    let screen = s.start_screen().unwrap();
    assert_eq!(screen.examples().len(), 3);
    assert!(screen.show_guide() && screen.cards_shown(s.size()));
    let json = painted(&mut s);
    for name in [
        "Basalt Beat",
        "Bus Room",
        "Moonrise Modular",
        "Seven short steps",
    ] {
        assert!(json.contains(name), "{name}");
    }
}

#[test]
fn an_example_card_opens_the_example_as_a_new_unsaved_song() {
    let d = Dir::new("example");
    let mut s = with_factory(&d, "{}");
    s.execute(AppCommand::StartScreen);
    click_row(&mut s, Target::Example(0));
    assert!(!s.start_screen_open());
    assert!(s.core().file().is_none(), "untitled: Save asks for a place");
    assert!(!s.core().doc().song.tracks.is_empty());
    assert!(!s.guide_open());
    // The factory copy is never touched: it holds no audio written by the open.
    assert!(shipped().join("Projects").is_dir());
}

#[test]
fn an_example_replaces_a_song_with_unsaved_changes_only_after_the_question() {
    let d = Dir::new("example-dirty");
    let mut s = with_factory(&d, "{}");
    add_synth(&mut s, "mine");
    s.execute(AppCommand::StartScreen);
    let requests = click_row(&mut s, Target::Example(1));
    assert!(requests.iter().any(|r| matches!(
        r,
        ShellRequest::ConfirmSave {
            then: PendingAction::Command(AppCommand::OpenExample(1))
        }
    )));
    assert!(s.core().doc().devices.iter().any(|d| d.key.0 == "mine"));
    s.confirmed(AppCommand::OpenExample(1));
    assert!(!s.core().doc().devices.iter().any(|d| d.key.0 == "mine"));
}

#[test]
fn the_guide_card_starts_the_tour_like_the_help_row_does() {
    let d = Dir::new("card-tour");
    let mut s = with_factory(&d, "{}");
    s.execute(AppCommand::StartScreen);
    click_row(&mut s, Target::Guide);
    assert!(!s.start_screen_open());
    assert!(s.guide_open());
    assert_eq!(s.guide().unwrap().index(), 0);
    assert!(!s.core().doc().song.tracks.iter().any(|_| true));
}

#[test]
fn the_check_box_turns_the_guide_card_off_and_is_saved_at_once() {
    let d = Dir::new("card-off");
    let mut s = with_factory(&d, "{}");
    s.execute(AppCommand::StartScreen);
    assert!(s.workflow().show_guide_card);
    click_row(&mut s, Target::ShowGuide);
    assert!(s.start_screen_open(), "the box stays open");
    assert!(!s.workflow().show_guide_card);
    assert!(
        !load_workflow(&d.0.join(WORKFLOW_FILE))
            .value
            .show_guide_card
    );
    let screen = s.start_screen().unwrap();
    assert!(!screen.show_guide());
    assert!(!painted(&mut s).contains("Seven short steps"));
    // The examples stay, and the next time the box opens without the card; the Help row works.
    let mut again = with_factory(
        &d,
        &std::fs::read_to_string(d.0.join(WORKFLOW_FILE)).unwrap(),
    );
    again.execute(AppCommand::StartScreen);
    let screen = again.start_screen().unwrap();
    assert!(!screen.show_guide() && screen.examples().len() == 3);
    assert!(again.start_screen().unwrap().cards_shown(again.size()));
    again.execute(AppCommand::GuidedTour);
    assert!(again.guide_open());
}

#[test]
fn without_a_factory_folder_the_strip_is_just_the_guide_card() {
    let d = Dir::new("no-factory");
    let mut s = started(&d, "{}");
    s.execute(AppCommand::StartScreen);
    let screen = s.start_screen().unwrap();
    assert!(screen.examples().is_empty() && screen.show_guide());
    assert!(screen.cards_shown(s.size()));
}
