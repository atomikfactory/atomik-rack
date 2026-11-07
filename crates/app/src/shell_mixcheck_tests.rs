//! Mixer > Check Mix in the shell: the menu row, the panel, CHECK and its result, a fix button as
//! one undo step, the tool button that opens the masking map, and closing with Esc.

use super::*;
use crate::mixcheck_worker::CheckAnalysis;
use crate::session::NullBackend;
use atomik_core::PPQ;
use atomik_model::{
    Clip, ClipContent, ClipId, Command, CreateOptions, CreateSource, DeviceKey, Note, SongCommand,
    WorkflowCommand,
};
use atomik_render::mixcheck::{Finding, Fix, Kind, Tool};
use atomik_ui::{Key, Modifiers, PointerButton};

const BAR: i64 = 4 * PPQ;

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

/// A song with a synth named `Bass` on D1 and one named `Pad`.
fn with_two_synths(s: &mut Shell<NullBackend>) {
    for (k, name, note) in [("bass", "Bass", 26u8), ("pad", "Pad", 52)] {
        let core = s.core_mut();
        core.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
            index: usize::MAX,
            source: CreateSource::Type("atomik.poly-synth".into()),
            key: Some(key(k)),
            options: CreateOptions::default(),
        }))
        .unwrap();
        core.apply_command(Command::Rename {
            key: key(k),
            name: name.into(),
        })
        .unwrap();
        core.apply_command(Command::Song(SongCommand::AddClip {
            device: key(k),
            lane: 0,
            clip: Clip {
                id: ClipId(0),
                start: 0,
                length: 4 * BAR,
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
                content: ClipContent::Notes(vec![Note::new(0, 2 * BAR, note, 0.9)].into()),
            },
        }))
        .unwrap();
    }
    s.core_mut()
        .apply_command(Command::Song(SongCommand::SetSongEnd(2 * BAR)))
        .unwrap();
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

fn finding(kind: Kind, fix: Option<Fix>, tool: Option<Tool>) -> Finding {
    Finding {
        kind,
        track: "Pad".into(),
        channel: None,
        measured: "a number".into(),
        value: 1.0,
        why: "a reason",
        at: None,
        fix,
        tool,
    }
}

#[test]
fn the_mixer_menu_has_check_mix() {
    let s = shell();
    let mixer = s
        .menu_model()
        .into_iter()
        .find(|m| m.title == "Mixer")
        .unwrap();
    let row = mixer
        .items()
        .find(|i| i.command == AppCommand::CheckMix)
        .expect("the row");
    assert_eq!(row.label, "Check Mix\u{2026}");
    assert!(row.enabled);
    assert_eq!(
        AppCommand::CheckMix.command_id(),
        Some(("mixer.check_mix", None))
    );
}

#[test]
fn the_panel_opens_empty_and_esc_closes_it() {
    let mut s = shell();
    assert!(!s.mixcheck_open());
    s.execute(AppCommand::CheckMix);
    assert!(s.mixcheck_open());
    let view = s.mixcheck_view().unwrap();
    assert!(view.data().is_none() && !view.busy());
    s.handle(&UiEvent::Key {
        key: Key::Escape,
        modifiers: Modifiers::NONE,
        pressed: true,
    });
    assert!(!s.mixcheck_open());
}

#[test]
fn checking_a_song_without_channels_says_why_not() {
    let mut s = shell();
    s.execute(AppCommand::CheckMix);
    let run = s.mixcheck_view().unwrap().run_rect().center();
    click(&mut s, run);
    assert!(!s.core().mixcheck_running());
    assert!(s.mixcheck_view().unwrap().data().is_none());
    assert!(
        s.core()
            .last_rejection()
            .is_some_and(|m| m.contains("channels"))
    );
}

#[test]
fn check_lists_the_findings_and_leaves_the_song_alone() {
    let mut s = shell();
    with_two_synths(&mut s);
    let before = s.core().doc().clone();
    s.execute(AppCommand::CheckMix);
    let run = s.mixcheck_view().unwrap().run_rect().center();
    click(&mut s, run);
    assert!(s.core().mixcheck_running());
    s.core_mut().wait_background();
    s.frame();
    let view = s.mixcheck_view().unwrap();
    assert!(!view.busy());
    let data = view.data().expect("a result");
    assert!(
        data.rows
            .iter()
            .any(|r| r.tag == "BASS ROOT" && r.track == "Bass"),
        "{data:?}"
    );
    assert_eq!(*s.core().doc(), before);
}

#[test]
fn a_fix_button_applies_one_undo_step_and_then_says_done() {
    let mut s = shell();
    with_two_synths(&mut s);
    let pad = s
        .core()
        .doc()
        .console
        .channel_of(&key("pad"))
        .unwrap()
        .channel
        .clone();
    s.core_mut().set_mixcheck_for_test(CheckAnalysis {
        from_frame: 0,
        rate: 48_000,
        strips: 3,
        findings: vec![finding(
            Kind::LowMid,
            Some(Fix::CutLowMids {
                channel: pad,
                hz: 252.0,
            }),
            Some(Tool::MaskingMap),
        )],
    });
    let before = s.core().doc().clone();
    s.execute(AppCommand::CheckMix);
    let fix = s.mixcheck_view().unwrap().fix_rect(0).center();
    click(&mut s, fix);
    let eqs = s
        .core()
        .doc()
        .devices
        .iter()
        .filter(|d| d.type_id == "atomik.dynamic-eq")
        .count();
    assert_eq!(eqs, 1);
    assert!(s.mixcheck_open(), "the panel stays for the next fix");
    s.frame();
    assert!(s.mixcheck_view().unwrap().data().unwrap().rows[0].done);
    // A second click does nothing; one undo takes the whole fix back.
    click(&mut s, fix);
    assert_eq!(
        s.core()
            .doc()
            .devices
            .iter()
            .filter(|d| d.type_id == "atomik.dynamic-eq")
            .count(),
        1
    );
    assert!(s.core_mut().undo());
    assert_eq!(*s.core().doc(), before);
}

#[test]
fn the_tool_button_opens_the_masking_map_and_starts_its_analysis() {
    let mut s = shell();
    with_two_synths(&mut s);
    s.core_mut().set_mixcheck_for_test(CheckAnalysis {
        from_frame: 0,
        rate: 48_000,
        strips: 3,
        findings: vec![
            finding(Kind::LowMid, None, Some(Tool::MaskingMap)),
            finding(Kind::Peaks, None, Some(Tool::Clipper)),
        ],
    });
    s.execute(AppCommand::CheckMix);
    // The clipper has no button yet: only the masking map row shows one.
    let rows = s.mixcheck_view().unwrap().data().unwrap().rows.clone();
    assert!(rows[0].tool.is_some() && rows[1].tool.is_none());
    let tool = s.mixcheck_view().unwrap().tool_rect(0).center();
    click(&mut s, tool);
    assert!(!s.mixcheck_open());
    assert!(s.masking_open());
    assert!(s.core().masking_running());
    s.core_mut().wait_background();
}

#[test]
fn the_panel_paints_and_answers_a_tooltip() {
    let mut s = shell();
    s.execute(AppCommand::CheckMix);
    let r = s.mixcheck_view().unwrap().run_rect().center();
    assert!(s.mixcheck_view().unwrap().help_at(r).is_some());
    s.handle(&UiEvent::PointerMove {
        pos: r,
        modifiers: Modifiers::NONE,
    });
    assert!(s.needs_repaint());
}
