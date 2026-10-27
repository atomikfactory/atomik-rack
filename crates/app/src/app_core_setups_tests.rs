//! Choosing a guitar amp setup: one batch of param changes, one undo step, free tweaking after.

use super::*;
use crate::sink::RecordingSink;
use atomik_devices::guitar_amp::setups::{SETUP_NAMES, SETUPS};
use atomik_model::{Command, DeviceKey};

const AMP: &str = "atomik.guitar-amp";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn amp_core() -> AppCore<RecordingSink> {
    let mut c = AppCore::new(RecordingSink::new());
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: AMP.into(),
        key: Some(key("amp")),
    })
    .unwrap();
    c
}

fn set<S: EngineSink>(c: &mut AppCore<S>, param: &str, value: f32) {
    c.apply_command(Command::SetParam {
        key: key("amp"),
        param: param.into(),
        value,
        merge: None,
    })
    .unwrap();
}

/// The value of `param`: the stored one, else the descriptor's default.
fn value<S: EngineSink>(c: &AppCore<S>, param: &str) -> f32 {
    let dev = c.doc().resolve(&key("amp")).unwrap();
    dev.params.get(param).copied().unwrap_or_else(|| {
        atomik_devices::guitar_amp::DESCRIPTOR
            .param_by_key(param)
            .unwrap()
            .default
    })
}

fn snapshot<S: EngineSink>(c: &AppCore<S>) -> Vec<(String, f32)> {
    atomik_devices::guitar_amp::DESCRIPTOR
        .params
        .iter()
        .map(|p| (p.key.to_string(), value(c, p.key)))
        .collect()
}

#[test]
fn choosing_a_setup_sets_every_param_of_it() {
    for (i, s) in SETUPS.iter().enumerate() {
        let mut c = amp_core();
        set(&mut c, "setup", i as f32);
        assert_eq!(value(&c, "setup"), i as f32, "{}", SETUP_NAMES[i]);
        for (param, want) in s.values {
            assert_eq!(value(&c, param), want, "{}: {param}", s.name);
        }
    }
}

#[test]
fn a_setup_is_one_undo_step_and_redoes_whole() {
    let mut c = amp_core();
    // Some tweaks first, so undo has something to come back to.
    set(&mut c, "gain", 7.0);
    set(&mut c, "bypass", 1.0);
    let before = snapshot(&c);
    set(&mut c, "setup", 2.0);
    let after = snapshot(&c);
    assert_ne!(before, after);
    assert!(c.undo(), "one undo");
    assert_eq!(
        snapshot(&c),
        before,
        "everything is back, the bypass and the tweak included"
    );
    assert!(c.redo());
    assert_eq!(snapshot(&c), after);
    // The bypass is not part of a setup.
    assert_eq!(value(&c, "bypass"), 1.0);
    // The undo before the setup is the tweak, not a part of the setup.
    assert!(c.undo());
    assert!(c.undo());
    assert_eq!(value(&c, "bypass"), 0.0);
    assert_eq!(value(&c, "gain"), 7.0);
}

#[test]
fn tweaks_after_a_setup_are_free_and_do_not_reload_it() {
    let mut c = amp_core();
    set(&mut c, "setup", 3.0);
    set(&mut c, "gain", 1.5);
    set(&mut c, "delay.time", 222.0);
    assert_eq!(value(&c, "gain"), 1.5);
    assert_eq!(value(&c, "delay.time"), 222.0);
    assert_eq!(
        value(&c, "setup"),
        3.0,
        "the selector keeps showing the last setup"
    );
    assert!(c.undo());
    assert_eq!(value(&c, "delay.time"), 380.0);
    assert_eq!(
        value(&c, "gain"),
        1.5,
        "undo steps back one tweak at a time"
    );
    assert!(c.undo());
    assert_eq!(value(&c, "gain"), 8.0);
    // Choosing the same setup again restores its values.
    set(&mut c, "gain", 0.5);
    set(&mut c, "setup", 3.0);
    assert_eq!(value(&c, "gain"), 8.0);
}

#[test]
fn the_ui_selector_goes_through_the_same_path() {
    let mut c = amp_core();
    c.handle_action(UiAction::Command(Command::SetParam {
        key: key("amp"),
        param: "setup".into(),
        value: 4.0,
        merge: Some(77),
    }));
    assert_eq!(value(&c, "setup"), 4.0);
    assert_eq!(value(&c, "chorus.on"), 1.0);
    assert_eq!(value(&c, "delay.sync"), 7.0);
    assert!(c.undo());
    assert_eq!(snapshot(&c), snapshot(&amp_core()));
}

#[test]
fn other_params_and_other_devices_are_left_alone() {
    let mut c = amp_core();
    set(&mut c, "bass", 8.0);
    assert_eq!(value(&c, "treble"), 5.5, "one param changed one param");
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.distortion".into(),
        key: Some(key("dist")),
    })
    .unwrap();
    set_other(&mut c);
    assert!(c.undo());
    assert_eq!(value(&c, "bass"), 8.0);
}

fn set_other<S: EngineSink>(c: &mut AppCore<S>) {
    c.apply_command(Command::SetParam {
        key: key("dist"),
        param: "drive".into(),
        value: 20.0,
        merge: None,
    })
    .unwrap();
    assert_eq!(
        c.doc().resolve(&key("dist")).unwrap().params.get("drive"),
        Some(&20.0)
    );
}
