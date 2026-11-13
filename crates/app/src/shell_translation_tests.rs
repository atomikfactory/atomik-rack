//! The translation lens, "Will the bass read?" and Split Sub in the shell: the menu rows and their
//! checks, the transport bar's more menu, the lens in the status line, the panel with its
//! measurement and its Esc, and the Split Sub rows of the Edit menu and of the patch pop-up.

use super::*;
use crate::context_menus::{MenuEnable, patch_entries, specs};
use crate::session::NullBackend;
use atomik_core::PPQ;
use atomik_model::{
    Clip, ClipContent, ClipId, Command, CreateOptions, CreateSource, DeviceKey, Note, SongCommand,
    WorkflowCommand,
};
use atomik_ui::context_menu::Spec;
use atomik_ui::{ContextTarget, Key, Modifiers, PointerButton};

const BAR: i64 = 4 * PPQ;
const BASS: &str = "atomik.mono-bass";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

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

/// A shell with a Mono Bass track `bass` that plays a note.
fn with_a_bass(s: &mut Shell<NullBackend>) {
    let core = s.core_mut();
    core.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
        index: usize::MAX,
        source: CreateSource::Type(BASS.into()),
        key: Some(key("bass")),
        options: CreateOptions::default(),
    }))
    .unwrap();
    for (p, v) in [
        ("osc1.wave", 0.0f32),
        ("osc.mix", 0.0),
        ("filter.type", 0.0),
        ("filter.resonance", 0.0),
        ("filter.drive", 0.0),
        ("filter.cutoff", 20_000.0),
        ("filter.env_amount", 0.0),
        ("env.amp.attack", 5.0),
        ("env.amp.sustain", 1.0),
    ] {
        core.apply_command(Command::SetParam {
            key: key("bass"),
            param: p.into(),
            value: v,
            merge: None,
        })
        .unwrap();
    }
    core.apply_command(Command::Song(SongCommand::AddClip {
        device: key("bass"),
        lane: 0,
        clip: Clip {
            id: ClipId(0),
            start: 0,
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
            content: ClipContent::Notes(vec![Note::new(0, BAR - PPQ / 2, 36, 0.9)].into()),
        },
    }))
    .unwrap();
    core.apply_command(Command::Song(SongCommand::SetSongEnd(BAR)))
        .unwrap();
}

fn menu_row(
    s: &Shell<NullBackend>,
    menu: &str,
    command: AppCommand,
) -> (String, bool, Option<bool>) {
    let m = s
        .menu_model()
        .into_iter()
        .find(|m| m.title == menu)
        .unwrap();
    let row = m.items().find(|i| i.command == command).expect("the row");
    (row.label.to_string(), row.enabled, row.checked)
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

#[test]
fn the_mixer_menu_has_the_lens_switches_with_checks() {
    let mut s = shell();
    let (label, enabled, checked) = menu_row(&s, "Analyze", AppCommand::LensHeadphones);
    assert_eq!(
        (label.as_str(), enabled, checked),
        ("Headphones", true, Some(true))
    );
    let (_, _, small) = menu_row(&s, "Analyze", AppCommand::LensSmallSpeaker);
    assert_eq!(small, Some(false));
    let (label, enabled, _) = menu_row(&s, "Analyze", AppCommand::BassRead);
    assert_eq!(label, "Will the Bass Read?\u{2026}");
    assert!(enabled);
    s.execute(AppCommand::LensSmallSpeaker);
    s.execute(AppCommand::LensMono);
    assert_eq!(
        menu_row(&s, "Analyze", AppCommand::LensSmallSpeaker).2,
        Some(true)
    );
    assert_eq!(
        menu_row(&s, "Analyze", AppCommand::LensClubSub).2,
        Some(false)
    );
    assert_eq!(menu_row(&s, "Analyze", AppCommand::LensMono).2, Some(true));
    assert_eq!(
        menu_row(&s, "Analyze", AppCommand::LensHeadphones).2,
        Some(false)
    );
    s.execute(AppCommand::LensHeadphones);
    assert_eq!(
        menu_row(&s, "Analyze", AppCommand::LensHeadphones).2,
        Some(true)
    );
    assert_eq!(menu_row(&s, "Analyze", AppCommand::LensMono).2, Some(false));
}

#[test]
fn the_commands_have_ids_and_come_back_from_them() {
    for (id, command) in [
        ("mixer.lens_headphones", AppCommand::LensHeadphones),
        ("mixer.lens_small_speaker", AppCommand::LensSmallSpeaker),
        ("mixer.lens_club_sub", AppCommand::LensClubSub),
        ("mixer.lens_mono", AppCommand::LensMono),
        ("mixer.bass_read", AppCommand::BassRead),
        ("rack.split_sub", AppCommand::SplitSub),
    ] {
        assert_eq!(command.command_id(), Some((id, None)));
        assert_eq!(AppCommand::from_command(id, None), Some(command));
        assert!(
            atomik_ui::keymap::Registry::bundled().get(id).is_some(),
            "{id} is in the registry"
        );
    }
}

#[test]
fn the_lens_shows_in_the_status_line_while_it_is_on() {
    let mut s = shell();
    s.poll_status();
    assert!(!s.status_text().contains("LENS"));
    s.execute(AppCommand::LensClubSub);
    s.poll_status();
    assert!(
        s.status_text().contains("LENS CLUB SUB"),
        "{}",
        s.status_text()
    );
    s.execute(AppCommand::LensMono);
    s.poll_status();
    assert!(s.status_text().contains("LENS CLUB SUB + MONO"));
    s.execute(AppCommand::LensHeadphones);
    s.poll_status();
    assert!(!s.status_text().contains("LENS"));
}

#[test]
fn the_transport_more_menu_lists_the_switches_checked_as_they_are() {
    let rows = |e: &MenuEnable| -> Vec<(&'static str, Option<&'static str>, Option<bool>)> {
        specs(ContextTarget::TransportMore, e)
            .into_iter()
            .filter_map(|spec| match spec {
                Spec::Item {
                    command,
                    label,
                    checked,
                    ..
                } if command.starts_with("mixer.lens_") => Some((command, label, checked)),
                _ => None,
            })
            .collect()
    };
    let off = rows(&MenuEnable {
        lens_off: true,
        ..MenuEnable::default()
    });
    assert_eq!(
        off,
        [
            (
                "mixer.lens_small_speaker",
                Some("Lens: Small Speaker"),
                Some(false)
            ),
            ("mixer.lens_club_sub", Some("Lens: Club Sub"), Some(false)),
            ("mixer.lens_mono", Some("Lens: Mono"), Some(false)),
            (
                "mixer.lens_headphones",
                Some("Lens: Headphones"),
                Some(true)
            ),
        ]
    );
    let on = rows(&MenuEnable {
        lens_small: true,
        lens_mono: true,
        ..MenuEnable::default()
    });
    assert_eq!(on[0].2, Some(true));
    assert_eq!(on[2].2, Some(true));
    assert_eq!(on[3].2, Some(false));
}

#[test]
fn the_shell_builds_the_transport_menu_state_from_the_lens() {
    let mut s = shell();
    s.execute(AppCommand::LensSmallSpeaker);
    let enable = s.menu_enable(Pane::Arrangement);
    assert!(enable.lens_small && !enable.lens_club && !enable.lens_off);
    s.execute(AppCommand::LensMono);
    assert!(s.menu_enable(Pane::Arrangement).lens_mono);
}

#[test]
fn the_panel_measures_the_selected_track_and_shows_the_verdict() {
    let mut s = shell();
    with_a_bass(&mut s);
    s.core_mut()
        .arrangement_mut()
        .set_selected_track(Some(key("bass")));
    assert!(!s.bass_read_open());
    s.execute(AppCommand::BassRead);
    assert!(s.bass_read_open());
    let view = s.bass_read_view().expect("open");
    assert_eq!(view.data().sources.len(), 2);
    assert_eq!(view.data().chosen, 1, "the selected track's channel");
    let measure = view.measure_rect().center();
    click(&mut s, measure);
    s.core_mut().wait_background();
    s.frame();
    let data = s.bass_read_view().expect("open").data().clone();
    let row = data.reading.expect("a reading");
    assert!(row.share > 0.08, "{row:?}");
    assert_eq!(row.level, atomik_ui::BassLevel::Reads);
    assert!(row.verdict.starts_with("Yes"));
    // A click on the first row chooses the master.
    let first = s.bass_read_view().unwrap().row_rect(0).center();
    click(&mut s, first);
    assert_eq!(s.bass_read_view().unwrap().data().chosen, 0);
    // Esc closes the panel.
    s.handle(&UiEvent::Key {
        key: Key::Escape,
        modifiers: Modifiers::NONE,
        pressed: true,
    });
    assert!(!s.bass_read_open());
}

#[test]
fn closing_the_panel_stops_a_reading_that_runs() {
    let mut s = shell();
    with_a_bass(&mut s);
    s.execute(AppCommand::BassRead);
    let measure = s.bass_read_view().unwrap().measure_rect().center();
    click(&mut s, measure);
    let close = s.bass_read_view().unwrap().close_rect().center();
    click(&mut s, close);
    assert!(!s.bass_read_open());
    s.core_mut().wait_background();
    assert!(s.core().bass_reading().is_none() || !s.core().bass_read_running());
}

#[test]
fn the_panel_has_tooltips_and_follows_the_window() {
    let mut s = shell();
    s.execute(AppCommand::BassRead);
    let view = s.bass_read_view().unwrap();
    let close = view.close_rect().center();
    assert!(view.help_at(close).is_some());
    s.handle(&UiEvent::Resize(Size::new(700.0, 560.0)));
    let panel = s.bass_read_view().unwrap().panel();
    assert!(panel.x1 <= 700.0 && panel.y1 <= 560.0);
}

#[test]
fn split_sub_is_in_the_edit_menu_for_a_selected_mono_bass() {
    let mut s = shell();
    let (label, enabled, _) = menu_row(&s, "Edit", AppCommand::SplitSub);
    assert_eq!(label, "Split Sub");
    assert!(!enabled, "nothing to split");
    with_a_bass(&mut s);
    s.core_mut()
        .view_mut()
        .set_selection(&[key("bass")], Some(&key("bass")));
    assert!(menu_row(&s, "Edit", AppCommand::SplitSub).1);
    s.execute(AppCommand::SplitSub);
    assert!(s.core().doc().devices.iter().any(|d| d.inner.is_some()));
    assert!(s.core().doc().device(&key("bass")).is_none());
    assert_eq!(s.core().document().undo_label(), Some("Split Sub"));
    s.core_mut().undo();
    assert!(s.core().doc().device(&key("bass")).is_some());
}

#[test]
fn the_patch_pop_up_of_a_mono_bass_offers_split_sub_after_clean_up() {
    let items = vec![(
        "Deep".to_owned(),
        std::path::PathBuf::from("deep.atomikpatch"),
    )];
    let commands = |clean: bool, split: bool| -> Vec<String> {
        patch_entries(&items, Some(0), clean, split)
            .into_iter()
            .filter_map(|e| match e {
                atomik_ui::context_menu::Entry::Item(i) => Some(i.command),
                _ => None,
            })
            .collect()
    };
    assert_eq!(
        commands(true, true),
        ["rack.clean_up_bass", "rack.split_sub", "rack.patch_pick"]
    );
    assert_eq!(
        commands(true, false),
        ["rack.clean_up_bass", "rack.patch_pick"]
    );
    assert_eq!(commands(false, false), ["rack.patch_pick"]);
    assert_eq!(
        patch_entries(&[], None, true, true).len(),
        2,
        "no separator without patches"
    );
}
