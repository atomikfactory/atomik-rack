//! The reverb send wizard through the shell: the Track menu row and its gate, the panel opening,
//! its choices kept between openings, ADD making the send in one undo step and closing, Esc and
//! the refusals that keep it open.

use super::*;
use crate::session::NullBackend;
use atomik_core::PPQ;
use atomik_model::{
    Clip, ClipContent, ClipId, Command, CreateOptions, CreateSource, DeviceKey, Note, SongCommand,
    WorkflowCommand,
};
use atomik_ui::{Key, Modifiers, PointerButton, ReverbSendRow};

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

/// A shell with a synth track `lead` that plays a note on every beat for four bars.
fn with_a_lead(s: &mut Shell<NullBackend>) {
    let core = s.core_mut();
    core.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
        index: usize::MAX,
        source: CreateSource::Type("atomik.poly-synth".into()),
        key: Some(key("lead")),
        options: CreateOptions::default(),
    }))
    .unwrap();
    core.apply_command(Command::Song(SongCommand::AddClip {
        device: key("lead"),
        lane: 0,
        clip: Clip {
            id: ClipId(0),
            start: 0,
            length: 16 * PPQ,
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
                (0..16)
                    .map(|i| Note::new(i * PPQ, PPQ / 2, 60, 0.8))
                    .collect::<Vec<_>>()
                    .into(),
            ),
        },
    }))
    .unwrap();
}

fn row(s: &Shell<NullBackend>) -> (String, bool) {
    let m = s
        .menu_model()
        .into_iter()
        .find(|m| m.title == "Track")
        .unwrap();
    let r = m
        .items()
        .find(|i| i.command == AppCommand::AddReverbSend)
        .expect("Add Reverb Send in the Track menu");
    (r.label.to_string(), r.enabled)
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

fn centre(r: atomik_ui::kurbo::Rect) -> atomik_ui::kurbo::Point {
    atomik_ui::kurbo::Point::new((r.x0 + r.x1) / 2.0, (r.y0 + r.y1) / 2.0)
}

#[test]
fn the_track_menu_row_is_on_with_a_selected_track_and_opens_the_panel() {
    let mut s = shell();
    assert_eq!(row(&s), ("Add Reverb Send\u{2026}".to_owned(), false));
    with_a_lead(&mut s);
    s.core_mut()
        .arrangement_mut()
        .set_selected_track(Some(key("lead")));
    assert!(row(&s).1);
    assert!(!s.reverb_send_open());
    s.execute(AppCommand::AddReverbSend);
    assert!(s.reverb_send_open() && s.modal_open());
    let view = s.reverb_send_view().unwrap();
    assert_eq!(view.value_text(ReverbSendRow::Decay), "auto: 1.0 s");
    // The command id reaches the same panel.
    s.handle(&UiEvent::Key {
        key: Key::Escape,
        modifiers: Modifiers::NONE,
        pressed: true,
    });
    assert!(!s.reverb_send_open());
    s.run_command("sequencer.add_reverb_send", None);
    assert!(s.reverb_send_open());
}

#[test]
fn add_makes_the_send_in_one_undo_step_closes_and_the_choices_are_kept() {
    let mut s = shell();
    with_a_lead(&mut s);
    s.core_mut()
        .arrangement_mut()
        .set_selected_track(Some(key("lead")));
    s.execute(AppCommand::AddReverbSend);
    // Turn the de-esser on and the send pre-fader, then ADD.
    for (row, plus) in [
        (ReverbSendRow::Vocal, true),
        (ReverbSendRow::Position, true),
    ] {
        let v = s.reverb_send_view().unwrap();
        let r = if plus {
            v.plus_rect(row)
        } else {
            v.minus_rect(row)
        };
        click(&mut s, centre(r));
    }
    let add = s.reverb_send_view().unwrap().add_rect();
    let undo_before = s.core().document().undo_label();
    click(&mut s, centre(add));
    assert!(
        !s.reverb_send_open(),
        "the panel closes when the send is made"
    );
    assert_eq!(s.core().document().undo_label(), Some("Add Reverb Send"));
    let doc = s.core().doc();
    assert!(doc.console.returns[0].is_some());
    let de = doc
        .devices
        .iter()
        .filter(|d| d.type_id == "atomik.de-esser")
        .count();
    assert_eq!(de, 1, "the vocal choice reached the model");
    let channel = doc
        .console
        .channel_of(&key("lead"))
        .unwrap()
        .channel
        .clone();
    assert_eq!(
        doc.device(&channel).unwrap().params.get("send1.pre"),
        Some(&1.0)
    );

    // The next opening starts from the same choices.
    s.core_mut().undo();
    assert_eq!(s.core().document().undo_label(), undo_before);
    s.execute(AppCommand::AddReverbSend);
    let v = s.reverb_send_view().unwrap();
    assert!(v.values().vocal && v.values().pre_fader);
}

#[test]
fn a_refusal_keeps_the_panel_open_with_the_reason() {
    let mut s = shell();
    with_a_lead(&mut s);
    s.core_mut()
        .arrangement_mut()
        .set_selected_track(Some(key("lead")));
    s.execute(AppCommand::AddReverbSend);
    // The selection goes away while the panel is open: ADD is off and the panel says so.
    s.core_mut().arrangement_mut().set_selected_track(None);
    s.core_mut().view_mut().set_selection(&[], None);
    s.handle(&UiEvent::PointerMove {
        pos: atomik_ui::kurbo::Point::new(1.0, 1.0),
        modifiers: Modifiers::NONE,
    });
    let add = s.reverb_send_view().unwrap().add_rect();
    click(&mut s, centre(add));
    assert!(s.reverb_send_open());
    assert!(s.core().doc().console.returns[0].is_none());
}
