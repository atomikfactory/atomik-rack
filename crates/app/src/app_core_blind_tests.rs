//! The loudness match switch and readout and the blind A/B test through the core.

use super::*;
use crate::sink::{RecordingSink, SinkCall};
use atomik_devices::bypass::MATCH_KEY;
use atomik_model::empty_doc;
use atomik_ui::BlindAction;

const FX: &str = "atomik.gain-tool";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn add(core: &mut AppCore<RecordingSink>, k: &str, type_id: &str) {
    core.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: type_id.into(),
        key: Some(key(k)),
    })
    .unwrap();
}

fn pid(core: &AppCore<RecordingSink>, param: &str) -> atomik_core::ParamId {
    core.registry()
        .get(FX)
        .unwrap()
        .descriptor
        .param_by_key(param)
        .unwrap()
        .id
}

/// The `(param, value)` calls on `bypass` and the match switch since the last look.
fn calls(core: &mut AppCore<RecordingSink>) -> Vec<(&'static str, f32)> {
    let (b, m) = (pid(core, "bypass"), pid(core, MATCH_KEY));
    core.sink_mut()
        .take()
        .into_iter()
        .filter_map(|c| match c {
            SinkCall::Param { param, plain, .. } if param == b => Some(("bypass", plain)),
            SinkCall::Param { param, plain, .. } if param == m => Some(("match", plain)),
            _ => None,
        })
        .collect()
}

fn core_with_fx() -> AppCore<RecordingSink> {
    let mut core = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    add(&mut core, "fx", FX);
    core
}

#[test]
fn toggling_the_match_is_a_listening_aid_not_an_undo_step_and_says_so() {
    let mut core = core_with_fx();
    let depth = core.document().undo_depth();
    core.toggle_match(&[key("fx")]);
    assert!(core.match_on(&key("fx")));
    assert!(core.has_match(&key("fx")));
    assert!(
        core.last_rejection()
            .is_some_and(|m| m.contains("match on"))
    );
    assert_eq!(calls(&mut core), vec![("match", 1.0)]);
    // Sent once, then again only after a new engine.
    core.frame();
    assert!(calls(&mut core).is_empty());
    core.toggle_match(&[key("fx")]);
    assert!(!core.match_on(&key("fx")));
    assert_eq!(calls(&mut core), vec![("match", 0.0)]);
    assert_eq!(core.document().undo_depth(), depth, "nothing to undo");
    // Nothing of it is in the document.
    assert!(core.doc().resolve(&key("fx")).unwrap().params.is_empty());
}

#[test]
fn a_device_without_the_match_is_refused() {
    let mut core = core_with_fx();
    add(&mut core, "syn", "atomik.poly-synth");
    assert!(!core.has_match(&key("syn")));
    core.toggle_match(&[key("syn")]);
    assert!(core.last_rejection().is_some_and(|m| m.contains("effect")));
    assert!(core.blind_start_seeded(&key("syn"), 4, 1).is_err());
    assert!(!core.blind_open());
}

#[test]
fn the_readout_follows_the_published_gain_and_clears_when_the_match_goes_off() {
    let mut core = core_with_fx();
    core.toggle_match(&[key("fx")]);
    let id = core.device_id(&key("fx")).unwrap();
    core.sink_mut()
        .device_values
        .insert(id, (vec![-6.04, 4242.0], 1));
    core.frame();
    assert_eq!(core.view().match_gain(&key("fx")), Some(-6.0));
    core.toggle_match(&[key("fx")]);
    core.frame();
    assert_eq!(core.view().match_gain(&key("fx")), None);
}

#[test]
fn a_blind_test_drives_the_engine_only_and_hands_the_device_back() {
    let mut core = core_with_fx();
    let undo_before = core.document().undo_depth();
    core.blind_start_seeded(&key("fx"), 3, 11).unwrap();
    assert!(core.blind_open());
    let info = core.blind_info().unwrap();
    assert_eq!((info.trial, info.trials, info.can_pick), (1, 3, false));
    core.sink_mut().take();
    for _ in 0..3 {
        core.blind_listen(0);
        let x = calls(&mut core);
        core.blind_listen(1);
        let y = calls(&mut core);
        let bypass = |c: &[(&str, f32)]| c.iter().find(|(k, _)| *k == "bypass").unwrap().1;
        assert_ne!(bypass(&x), bypass(&y), "one is the effect, one is bypassed");
        assert!(core.blind_info().unwrap().can_pick);
        core.blind_pick(0);
    }
    // Over: the engine has the document's own values again, and the result is shown.
    let back = calls(&mut core);
    assert!(back.contains(&("bypass", 0.0)), "{back:?}");
    let info = core.blind_info().unwrap();
    assert!(info.result.as_ref().is_some_and(|r| r.len() == 4));
    assert_eq!(
        core.document().undo_depth(),
        undo_before,
        "no undo step, no change"
    );
    assert!(!core.match_on(&key("fx")));
}

#[test]
fn closing_a_test_midway_restores_the_device_and_the_match_choice_is_the_tests_own() {
    let mut core = core_with_fx();
    core.toggle_match(&[key("fx")]);
    core.blind_start_seeded(&key("fx"), 5, 3).unwrap();
    assert!(
        core.blind_info().unwrap().match_on,
        "starts from the device's setting"
    );
    core.blind_action(BlindAction::ToggleMatch);
    core.blind_action(BlindAction::Listen(0));
    let on = calls(&mut core);
    assert!(on.contains(&("match", 0.0)), "{on:?}");
    core.blind_action(BlindAction::Close);
    assert!(!core.blind_open());
    let back = calls(&mut core);
    assert!(
        back.contains(&("match", 1.0)),
        "the document says on: {back:?}"
    );
    assert!(core.match_on(&key("fx")), "the document never changed");
}

#[test]
fn removing_the_device_ends_the_test() {
    let mut core = core_with_fx();
    core.blind_start_seeded(&key("fx"), 3, 3).unwrap();
    core.apply_command(Command::RemoveDevice { key: key("fx") })
        .unwrap();
    core.frame();
    assert!(!core.blind_open());
}
