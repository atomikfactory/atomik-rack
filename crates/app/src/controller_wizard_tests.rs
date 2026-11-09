use super::*;
use crate::controller_profile::ControlMode;
use atomik_model::{RelativeKind, SourceKind};

fn sweep(w: &mut Wizard, channel: u8, number: u8) -> Progress {
    let mut last = Progress::Idle;
    for v in (0..=127).step_by(9) {
        last = w.feed(&ChannelMessage::cc(channel, number, v));
        if matches!(last, Progress::Accepted(_)) {
            break;
        }
    }
    last
}

fn press(w: &mut Wizard, key: u8) -> Progress {
    w.feed(&ChannelMessage::note_on(0, key, 100))
}

#[test]
fn the_wizard_asks_for_one_control_after_another() {
    let mut w = Wizard::new("In", None);
    assert_eq!(w.stage(), Stage::Control(Role::Knob(1)));
    assert_eq!(w.prompt(), "Move knob 1");
    assert_eq!(w.position(), (0, steps().len()));
    let Progress::Accepted(spec) = sweep(&mut w, 0, 21) else {
        panic!("knob 1 not learned");
    };
    assert_eq!(spec.role, Role::Knob(1));
    assert_eq!(spec.source, SourceKind::Cc(21));
    assert_eq!(spec.mode, ControlMode::Absolute);
    assert_eq!(w.stage(), Stage::Control(Role::Knob(2)));
    assert_eq!(w.prompt(), "Move knob 2");
    assert_eq!(w.count(), 1);
}

#[test]
fn a_knob_that_is_still_turning_does_not_answer_the_next_question() {
    let mut w = Wizard::new("In", None);
    sweep(&mut w, 0, 21);
    // The same knob keeps sending: ignored.
    for v in [3, 40, 80, 120, 10] {
        assert_eq!(w.feed(&ChannelMessage::cc(0, 21, v)), Progress::Idle);
    }
    assert_eq!(w.stage(), Stage::Control(Role::Knob(2)));
    // Another knob answers it.
    assert!(matches!(sweep(&mut w, 0, 22), Progress::Accepted(_)));
    assert_eq!(w.stage(), Stage::Control(Role::Knob(3)));
}

#[test]
fn an_encoder_asks_to_be_turned_both_ways() {
    let mut w = Wizard::new("In", None);
    for v in [1, 1, 2] {
        assert_eq!(w.feed(&ChannelMessage::cc(0, 16, v)), Progress::Hearing);
    }
    assert!(w.hint().is_some());
    assert_eq!(w.stage(), Stage::Control(Role::Knob(1)));
    let Progress::Accepted(spec) = w.feed(&ChannelMessage::cc(0, 16, 127)) else {
        panic!("not learned: {:?}", w.hint());
    };
    assert_eq!(
        spec.mode,
        ControlMode::Relative(RelativeKind::TwosComplement)
    );
}

#[test]
fn skip_leaves_a_control_the_controller_lacks() {
    let mut w = Wizard::new("In", None);
    sweep(&mut w, 0, 21);
    w.skip();
    assert_eq!(w.stage(), Stage::Control(Role::Knob(3)));
    sweep(&mut w, 0, 23);
    assert_eq!(
        w.learned().iter().map(|c| c.role).collect::<Vec<_>>(),
        [Role::Knob(1), Role::Knob(3)]
    );
}

#[test]
fn skip_group_leaves_the_rest_of_the_knobs_or_the_faders_but_one_key() {
    let mut w = Wizard::new("In", None);
    sweep(&mut w, 0, 21);
    w.skip_group();
    assert_eq!(w.stage(), Stage::Control(Role::Fader(1)));
    w.skip_group();
    assert_eq!(w.stage(), Stage::Control(Role::Play));
    w.skip_group();
    assert_eq!(w.stage(), Stage::Control(Role::Stop));
}

#[test]
fn back_takes_the_last_answer_back() {
    let mut w = Wizard::new("In", None);
    sweep(&mut w, 0, 21);
    w.skip();
    assert_eq!(w.stage(), Stage::Control(Role::Knob(3)));
    w.back();
    assert_eq!(w.stage(), Stage::Control(Role::Knob(2)));
    w.back();
    assert_eq!(w.stage(), Stage::Control(Role::Knob(1)));
    assert_eq!(w.count(), 0);
    // The control that was taken back can be learned again.
    assert!(matches!(sweep(&mut w, 0, 21), Progress::Accepted(_)));
    w.back();
    w.back();
    assert_eq!(w.stage(), Stage::Control(Role::Knob(1)));
}

#[test]
fn keys_are_learned_by_pressing_them() {
    let mut w = Wizard::new("In", None);
    w.skip_group();
    // The faders and the master fader are one group.
    w.skip_group();
    assert_eq!(w.prompt(), "Press Play");
    let Progress::Accepted(play) = press(&mut w, 94) else {
        panic!("play not learned");
    };
    assert_eq!(play.role, Role::Play);
    assert_eq!(play.mode, ControlMode::Button);
    // The Stop key is another one: pressing Play again is not heard.
    assert_eq!(press(&mut w, 94), Progress::Idle);
    assert!(matches!(press(&mut w, 93), Progress::Accepted(_)));
    assert_eq!(w.stage(), Stage::Control(Role::Record));
}

#[test]
fn a_controller_with_an_output_is_asked_about_feedback_per_group() {
    let mut w = Wizard::new("In", Some("Out"));
    sweep(&mut w, 0, 21);
    w.skip_group();
    // Faders: one pitch bend fader.
    for v in [0u16, 3000, 8000, 16000] {
        w.feed(&ChannelMessage::pitch_bend(0, v));
    }
    w.skip_group();
    press(&mut w, 94);
    w.finish_now();
    assert_eq!(w.stage(), Stage::Ask(Group::Knobs));
    assert!(w.profile("X").is_none(), "not done yet");
    w.answer(true);
    assert_eq!(w.stage(), Stage::Ask(Group::Faders));
    w.answer(false);
    assert_eq!(w.stage(), Stage::Ask(Group::Keys));
    w.answer(true);
    assert_eq!(w.stage(), Stage::Done);
    let p = w.profile("Desk").unwrap();
    assert_eq!(p.output.as_deref(), Some("Out"));
    let fb = |r| p.control(r).unwrap().feedback;
    assert!(fb(Role::Knob(1)) && !fb(Role::Fader(1)) && fb(Role::Play));
    assert!(p.validate().is_ok());
}

#[test]
fn groups_with_no_control_are_not_asked_about() {
    let mut w = Wizard::new("In", Some("Out"));
    sweep(&mut w, 0, 21);
    w.finish_now();
    assert_eq!(w.stage(), Stage::Ask(Group::Knobs));
    w.answer(false);
    assert_eq!(w.stage(), Stage::Done);
}

#[test]
fn a_controller_without_an_output_is_not_asked() {
    let mut w = Wizard::new("In", None);
    sweep(&mut w, 0, 21);
    w.finish_now();
    assert_eq!(w.stage(), Stage::Done);
    let p = w.profile("Desk").unwrap();
    assert!(p.output.is_none() && p.controls.iter().all(|c| !c.feedback));
}

#[test]
fn nothing_learned_makes_no_profile() {
    let mut w = Wizard::new("In", Some("Out"));
    w.finish_now();
    assert_eq!(w.stage(), Stage::Done);
    assert!(w.profile("Desk").is_none());
}

#[test]
fn back_works_from_the_questions() {
    let mut w = Wizard::new("In", Some("Out"));
    sweep(&mut w, 0, 21);
    w.finish_now();
    assert_eq!(w.stage(), Stage::Ask(Group::Knobs));
    w.back();
    assert_eq!(w.stage(), Stage::Control(Role::Knob(1)));
    assert_eq!(w.count(), 0);
}

#[test]
fn a_whole_controller_is_learned_from_a_message_stream() {
    let mut w = Wizard::new("Desk In", Some("Desk Out"));
    // Eight absolute knobs on controllers 21..=28.
    for n in 0..8u8 {
        assert!(matches!(sweep(&mut w, 0, 21 + n), Progress::Accepted(_)));
    }
    // Eight faders on pitch bend of channels 1..=8.
    for ch in 0..8u8 {
        let mut accepted = false;
        for v in [0u16, 4096, 8192, 12288, 16383] {
            accepted |= matches!(
                w.feed(&ChannelMessage::pitch_bend(ch, v)),
                Progress::Accepted(_)
            );
        }
        assert!(accepted, "fader {}", ch + 1);
    }
    // No master fader; the four transport keys and the two bank keys on notes.
    w.skip();
    for key in 90..96u8 {
        assert!(matches!(press(&mut w, key), Progress::Accepted(_)), "{key}");
    }
    assert!(matches!(w.stage(), Stage::Ask(Group::Knobs)));
    for _ in 0..3 {
        w.answer(true);
    }
    let p = w.profile("Desk Controller").unwrap();
    assert_eq!(p.controls.len(), 8 + 8 + 6);
    assert_eq!(
        p.control(Role::BankRight).unwrap().source,
        SourceKind::Note(95)
    );
    assert!(p.validate().is_ok());
    assert_eq!(
        crate::controller_profile::ControllerProfile::from_json(&p.to_json()).unwrap(),
        p
    );
}
