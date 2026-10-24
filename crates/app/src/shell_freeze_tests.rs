//! Freeze in the shell: the Track menu items and their state, the registry ids, the header and
//! clip context menus, the status line while a render runs, and a frozen track in the window.

use super::*;
use crate::session::NullBackend;
use atomik_core::PPQ;
use atomik_model::{Clip, ClipContent, ClipId, Command, DeviceKey, Note, SongCommand};
use atomik_ui::PointerButton;
use atomik_ui::keymap::Os;

const SYNTH: &str = "atomik.poly-synth";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

/// A headless shell with a synth `lead` (an eight-bar clip, id 1) and a channel, the arrangement
/// focused.
fn shell() -> Shell<NullBackend> {
    let session = LiveSession::new(NullBackend, AudioSettings::default(), false);
    let mut s = Shell::new(
        AppCore::new(session),
        ShellOptions {
            os: Some(Os::Windows),
            profile: None,
            in_window_menu: true,
            settings_path: None,
            ui_state_path: None,
            library_path: None,
        },
    );
    s.handle(&UiEvent::ScaleChanged(1.0));
    s.handle(&UiEvent::Resize(Size::new(1200.0, 900.0)));
    let core = s.core_mut();
    core.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: SYNTH.to_string(),
        key: Some(key("lead")),
    })
    .unwrap();
    core.apply_command(Command::Song(SongCommand::AddTrack {
        device: key("lead"),
    }))
    .unwrap();
    core.apply_command(Command::Song(SongCommand::AddClip {
        device: key("lead"),
        lane: 0,
        clip: Clip {
            id: ClipId(0),
            start: 0,
            length: 32 * PPQ,
            offset: 0,
            loop_length: None,
            muted: false,
            scale: None,
            controllers: Vec::new(),
            grid: None,
            content: ClipContent::Notes(vec![Note::new(0, PPQ, 60, 0.8)]),
        },
    }))
    .unwrap();
    core.after_view_change();
    s.set_focus(Pane::Arrangement);
    s
}

fn item(s: &Shell<NullBackend>, label: &str) -> (bool, Option<bool>) {
    s.menu_model()
        .iter()
        .flat_map(|m| m.items())
        .find(|i| i.label == label)
        .map(|i| (i.enabled, i.checked))
        .unwrap_or_else(|| panic!("{label}"))
}

fn lead_frozen(s: &Shell<NullBackend>) -> bool {
    s.core()
        .doc()
        .song
        .track(&key("lead"))
        .is_some_and(|t| t.frozen.is_some())
}

#[test]
fn the_track_menu_offers_freeze_and_bounce_by_the_selection() {
    let mut s = shell();
    for label in [
        "Freeze Track",
        "Freeze Track up to Selected Device",
        "Unfreeze Track",
        "Unfreeze Track and Keep Audio",
        "Cancel Freeze",
        "Bounce in Place",
        "Bounce in Place and Mute Originals",
    ] {
        assert!(!item(&s, label).0, "{label} needs a selection");
    }
    assert_eq!(
        item(&s, "Freeze with Tail"),
        (true, Some(true)),
        "the tail is on"
    );
    s.core_mut()
        .arrangement_mut()
        .set_selected_track(Some(key("lead")));
    assert!(item(&s, "Freeze Track").0 && item(&s, "Freeze Track up to Selected Device").0);
    assert!(!item(&s, "Unfreeze Track").0);
    s.core_mut()
        .arrangement_mut()
        .set_selected_clips(&[ClipId(1)]);
    assert!(item(&s, "Bounce in Place").0 && item(&s, "Bounce in Place and Mute Originals").0);
}

#[test]
fn the_freeze_commands_run_from_the_menu_and_the_registry_and_show_their_progress() {
    let mut s = shell();
    s.core_mut()
        .arrangement_mut()
        .set_selected_track(Some(key("lead")));
    s.execute(AppCommand::FreezeTrack);
    assert!(s.core().freeze_busy());
    assert!(item(&s, "Cancel Freeze").0 && !item(&s, "Freeze Track").0);
    s.poll_status();
    assert!(
        s.status_text().contains("Freezing Poly Synth"),
        "{}",
        s.status_text()
    );
    s.core_mut().wait_background();
    s.poll_status();
    assert!(lead_frozen(&s), "{:?}", s.core().last_rejection());
    assert!(!s.status_text().contains("Freezing"), "{}", s.status_text());
    assert!(item(&s, "Unfreeze Track").0 && !item(&s, "Freeze Track").0);

    // The registry ids are the same commands.
    assert_eq!(
        AppCommand::from_command("sequencer.unfreeze_track", None),
        Some(AppCommand::UnfreezeTrack)
    );
    s.execute(AppCommand::UnfreezeTrack);
    assert!(!lead_frozen(&s));
    // The tail switch is a check item.
    s.execute(AppCommand::FreezeTail);
    assert_eq!(item(&s, "Freeze with Tail").1, Some(false));
}

#[test]
fn the_track_header_and_clip_menus_carry_the_freeze_items() {
    let mut s = shell();
    s.core_mut()
        .arrangement_mut()
        .set_selected_track(Some(key("lead")));
    let enable = s.menu_enable(Pane::Arrangement);
    assert!(enable.freeze.can_freeze && !enable.freeze.can_unfreeze);
    let ids = |target| -> Vec<(&'static str, bool)> {
        context_menus::specs(target, &enable)
            .into_iter()
            .filter_map(|spec| match spec {
                atomik_ui::context_menu::Spec::Item {
                    command, enabled, ..
                } => Some((command, enabled)),
                _ => None,
            })
            .collect()
    };
    let header = ids(atomik_ui::ContextTarget::TrackHeader);
    for (id, on) in [
        ("sequencer.freeze_track", true),
        ("sequencer.unfreeze_track", false),
    ] {
        assert_eq!(
            header.iter().find(|(c, _)| *c == id).map(|(_, e)| *e),
            Some(on),
            "{id}"
        );
    }
    assert!(
        ids(atomik_ui::ContextTarget::Clip)
            .iter()
            .any(|(c, _)| *c == "sequencer.bounce_in_place")
    );
}

#[test]
fn a_frozen_track_shows_in_the_window_and_refuses_an_edit_with_a_status_text() {
    let mut s = shell();
    s.core_mut()
        .arrangement_mut()
        .set_selected_track(Some(key("lead")));
    s.execute(AppCommand::FreezeTrack);
    s.core_mut().wait_background();
    assert!(lead_frozen(&s));
    // The arrangement paints the render.
    let mut dl = atomik_ui::DisplayList::new();
    s.paint(&mut dl);
    assert!(dl.to_json().contains("(frozen)"));
    // An edit of the frozen track is refused, and the window says why.
    let before = s.core().doc().clone();
    let result = s.core_mut().apply_command(Command::SetParam {
        key: key("lead"),
        param: "filter.cutoff".into(),
        value: 400.0,
        merge: None,
    });
    assert!(result.is_err());
    assert_eq!(s.core().doc(), &before);
    assert!(s.core().last_rejection().unwrap().contains("unfreeze"));
    // Pressing where its clip is selects nothing.
    let r = s
        .core()
        .arrangement()
        .clip_rect(ClipId(1))
        .expect("the hidden clip keeps its place");
    for down in [true, false] {
        s.handle(&if down {
            UiEvent::PointerDown {
                pos: r.center(),
                button: PointerButton::Primary,
                modifiers: Modifiers::NONE,
                clicks: 1,
            }
        } else {
            UiEvent::PointerUp {
                pos: r.center(),
                button: PointerButton::Primary,
                modifiers: Modifiers::NONE,
            }
        });
    }
    assert!(s.core().arrangement().selected_clips().is_empty());
}
