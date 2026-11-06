//! Selecting the master in the console: its settings copy and paste, and nothing else follows
//! the selection to the master.

use super::*;
use crate::commands::SettingsSection;
use crate::sink::RecordingSink;
use atomik_model::{CreateOptions, CreateSource, DeviceKey, WorkflowCommand, empty_doc};
use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{Modifiers, PointerButton};

fn core() -> AppCore<RecordingSink> {
    let mut c = AppCore::with_registry(RecordingSink::new(), default_registry(), empty_doc(), None);
    for _ in 0..2 {
        c.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
            index: usize::MAX,
            source: CreateSource::Type("atomik.poly-synth".into()),
            key: None,
            options: CreateOptions::default(),
        }))
        .unwrap();
    }
    c
}

fn master(c: &AppCore<RecordingSink>) -> DeviceKey {
    c.doc().console.master.clone().expect("master")
}

fn set(c: &mut AppCore<RecordingSink>, key: &DeviceKey, param: &str, value: f32) {
    c.apply_command(Command::SetParam {
        key: key.clone(),
        param: param.into(),
        value,
        merge: None,
    })
    .unwrap();
}

fn value(c: &AppCore<RecordingSink>, key: &DeviceKey, param: &str) -> Option<f32> {
    c.doc().device(key).unwrap().params.get(param).copied()
}

fn click(c: &mut AppCore<RecordingSink>, p: Point) {
    for event in [
        UiEvent::PointerDown {
            pos: p,
            button: PointerButton::Primary,
            modifiers: Modifiers::default(),
            clicks: 1,
        },
        UiEvent::PointerUp {
            pos: p,
            button: PointerButton::Primary,
            modifiers: Modifiers::default(),
        },
    ] {
        c.handle_pane_event(Pane::Console, &event);
    }
}

fn shown(c: &mut AppCore<RecordingSink>) {
    let doc = c.doc().clone();
    c.console_mut()
        .set_bounds(Rect::new(0.0, 0.0, 1100.0, 760.0));
    c.console_mut().sync(&doc, &default_registry());
}

#[test]
fn a_click_on_the_master_name_plate_selects_it_and_a_strip_click_takes_it_out_again() {
    let mut c = core();
    shown(&mut c);
    let strip = c.doc().console.channels[0].channel.clone();
    assert!(!c.console().master_selected());
    let p = c.console().master_name_center().expect("master shown");
    click(&mut c, p);
    assert!(c.console().master_selected());
    assert!(c.console().selected_strips().is_empty());
    let p = c.console().name_center(&strip).expect("strip shown");
    click(&mut c, p);
    assert!(!c.console().master_selected());
    assert_eq!(c.console().selected_strips(), vec![strip]);
    // Back to the master: the strip selection is cleared.
    let p = c.console().master_name_center().unwrap();
    click(&mut c, p);
    assert!(c.console().master_selected() && c.console().selected_strips().is_empty());
}

#[test]
fn the_master_copies_and_pastes_its_settings_in_one_undo_step() {
    let mut c = core();
    shown(&mut c);
    let m = master(&c);
    set(&mut c, &m, "fader", 0.4);
    set(&mut c, &m, "comp.threshold", -18.0);
    let p = c.console().master_name_center().unwrap();
    click(&mut c, p);
    assert!(c.strip_menu(Pane::Console).can_copy_settings);
    assert!(c.copy_strip_settings(Pane::Console, SettingsSection::All));
    assert!(c.copied_settings().unwrap().master);
    set(&mut c, &m, "fader", 0.9);
    set(&mut c, &m, "comp.threshold", -3.0);
    assert!(c.strip_menu(Pane::Console).can_paste_settings);
    let depth = c.document().undo_depth();
    assert!(c.paste_strip_settings(Pane::Console));
    assert_eq!(c.document().undo_depth(), depth + 1);
    assert_eq!(value(&c, &m, "fader"), Some(0.4));
    assert_eq!(value(&c, &m, "comp.threshold"), Some(-18.0));
    assert!(c.undo());
    assert_eq!(value(&c, &m, "fader"), Some(0.9));
}

#[test]
fn master_settings_do_not_paste_onto_a_channel_and_channel_settings_not_onto_the_master() {
    let mut c = core();
    shown(&mut c);
    let strip = c.doc().console.channels[0].channel.clone();
    let m = master(&c);
    // Master copy onto a selected channel.
    let p = c.console().master_name_center().unwrap();
    click(&mut c, p);
    assert!(c.copy_strip_settings(Pane::Console, SettingsSection::All));
    let p = c.console().name_center(&strip).unwrap();
    click(&mut c, p);
    let depth = c.document().undo_depth();
    assert!(!c.paste_strip_settings(Pane::Console));
    assert!(c.last_rejection().unwrap().contains("master"));
    assert_eq!(c.document().undo_depth(), depth);
    // Channel copy onto the selected master.
    assert!(c.copy_strip_settings(Pane::Console, SettingsSection::All));
    assert!(!c.copied_settings().unwrap().master);
    let p = c.console().master_name_center().unwrap();
    click(&mut c, p);
    assert!(!c.paste_strip_settings(Pane::Console));
    assert_eq!(c.document().undo_depth(), depth);
    assert_eq!(value(&c, &m, "fader"), None);
}

#[test]
fn the_master_selection_is_not_a_routing_or_group_selection() {
    let mut c = core();
    shown(&mut c);
    let p = c.console().master_name_center().unwrap();
    click(&mut c, p);
    let m = master(&c);
    assert!(!c.strips_for_vca(Pane::Console).contains(&m));
    assert!(!c.strips_to_route(true).contains(&m));
}
