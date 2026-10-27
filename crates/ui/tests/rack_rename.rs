//! A device's name is typed in place on its name plate: a double-click on the name opens the
//! editor (Enter commits `Command::Rename`, Escape cancels, a press elsewhere commits, a blank
//! name asks for the default), the rest of the panel keeps its double-click, and it works
//! inside a container too. Golden `rack_rename_editing.json`.
//! Regenerate with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test rack_rename`.

mod support;

use atomik_model::{Command, RackDoc};
use atomik_ui::kurbo::Point;
use atomik_ui::{Key, Modifiers, PointerButton, RackView, UiAction, UiEvent};
use support::container::*;
use support::*;

fn press_key(v: &mut RackView, doc: &RackDoc, key: Key) -> Vec<UiAction> {
    v.handle(
        &UiEvent::Key {
            key,
            modifiers: Modifiers::NONE,
            pressed: true,
        },
        doc,
        &registry(),
    )
}

fn type_text(v: &mut RackView, doc: &RackDoc, text: &str) {
    for c in text.chars() {
        let key = if c == ' ' { Key::Space } else { Key::Char(c) };
        let _ = press_key(v, doc, key);
    }
}

/// Double-clicks the name of `dev` the way a mouse does: a click, then the second press.
fn double_click_name(v: &mut RackView, doc: &RackDoc, dev: &str) -> Vec<UiAction> {
    let p = v.name_center(&key(dev)).expect("a name");
    let mut out = press_with(v, doc, p, PointerButton::Primary, 1, Modifiers::NONE);
    out.extend(press_with(
        v,
        doc,
        p,
        PointerButton::Primary,
        2,
        Modifiers::NONE,
    ));
    out
}

/// Erases the name being typed.
fn clear_text(v: &mut RackView, doc: &RackDoc) {
    for _ in 0..40 {
        let _ = press_key(v, doc, Key::Backspace);
    }
}

fn rename(dev: &str, name: &str) -> Command {
    Command::Rename {
        key: key(dev),
        name: name.into(),
    }
}

#[test]
fn a_double_click_on_the_name_opens_the_editor_with_the_current_name() {
    let doc = reference_doc();
    let mut v = plain_view(&doc, &registry());
    assert!(!v.is_text_owner());
    let out = double_click_name(&mut v, &doc, "delay");
    assert_eq!(commands(&out), vec![], "opening the editor changes nothing");
    assert_eq!(v.renaming_device(&key("delay")), Some("Stereo Delay"));
    assert_eq!(v.renaming_device(&key("synth")), None);
    assert!(
        v.is_text_owner(),
        "the shell routes plain keys to the field"
    );
}

#[test]
fn typing_and_enter_rename_the_device() {
    let doc = reference_doc();
    let mut v = plain_view(&doc, &registry());
    let _ = double_click_name(&mut v, &doc, "delay");
    clear_text(&mut v, &doc);
    type_text(&mut v, &doc, "Slap Echo");
    assert_eq!(v.renaming_device(&key("delay")), Some("Slap Echo"));
    let out = press_key(&mut v, &doc, Key::Enter);
    assert_eq!(commands(&out), vec![rename("delay", "Slap Echo")]);
    assert_eq!(v.renaming_device(&key("delay")), None);
    assert!(!v.is_text_owner());
}

#[test]
fn escape_cancels_without_a_command() {
    let doc = reference_doc();
    let mut v = plain_view(&doc, &registry());
    let _ = double_click_name(&mut v, &doc, "synth");
    type_text(&mut v, &doc, "xyz");
    let out = press_key(&mut v, &doc, Key::Escape);
    assert_eq!(commands(&out), vec![]);
    assert_eq!(v.renaming_device(&key("synth")), None);
    assert!(!v.is_text_owner());
}

#[test]
fn a_press_elsewhere_commits_but_a_press_on_the_field_keeps_editing() {
    let doc = reference_doc();
    let mut v = plain_view(&doc, &registry());
    let _ = double_click_name(&mut v, &doc, "delay");
    type_text(&mut v, &doc, "!");
    let on_field = v.name_center(&key("delay")).unwrap();
    let out = click(&mut v, &doc, on_field);
    assert_eq!(commands(&out), vec![]);
    assert_eq!(v.renaming_device(&key("delay")), Some("Stereo Delay!"));
    let elsewhere = v.plate_center(&key("mixer")).unwrap();
    let out = click(&mut v, &doc, elsewhere);
    assert_eq!(commands(&out), vec![rename("delay", "Stereo Delay!")]);
    assert_eq!(v.renaming_device(&key("delay")), None);
}

#[test]
fn an_unchanged_name_sends_nothing_and_a_blank_one_asks_for_the_default() {
    let doc = reference_doc();
    let mut v = plain_view(&doc, &registry());
    let _ = double_click_name(&mut v, &doc, "delay");
    let out = press_key(&mut v, &doc, Key::Enter);
    assert_eq!(commands(&out), vec![], "same name: no undo step");
    let _ = double_click_name(&mut v, &doc, "delay");
    clear_text(&mut v, &doc);
    let out = press_key(&mut v, &doc, Key::Enter);
    assert_eq!(commands(&out), vec![rename("delay", "")]);
}

#[test]
fn the_model_accepts_what_the_editor_sends() {
    let d = layered();
    let mut v = plain_view(d.doc(), &registry());
    let mut model = d;
    let _ = double_click_name(&mut v, model.doc(), "lead");
    clear_text(&mut v, model.doc());
    type_text(&mut v, model.doc(), "Bass");
    for c in commands(&press_key(&mut v, model.doc(), Key::Enter)) {
        model.apply(c, &registry()).expect("accepted");
    }
    assert_eq!(model.doc().device(&key("lead")).unwrap().name, "Bass");
}

#[test]
fn a_double_click_elsewhere_on_the_panel_keeps_its_own_behaviour() {
    let mut doc = reference_doc();
    doc.devices[0].params.insert("filter.cutoff".into(), 2400.0);
    let mut v = plain_view(&doc, &registry());
    // A knob still resets to its default on a double-click.
    let knob = v.control_center(&key("synth"), "filter.cutoff").unwrap();
    let out = press_with(
        &mut v,
        &doc,
        knob,
        PointerButton::Primary,
        2,
        Modifiers::NONE,
    );
    assert!(
        matches!(commands(&out).as_slice(), [Command::SetParam { param, .. }] if param == "filter.cutoff"),
        "{out:?}"
    );
    assert_eq!(v.renaming_device(&key("synth")), None);
    // The fold button and the bare plate do not open the editor either.
    let fold = v.fold_button_center(&key("synth")).unwrap();
    let _ = press_with(
        &mut v,
        &doc,
        fold,
        PointerButton::Primary,
        2,
        Modifiers::NONE,
    );
    let r = v.device_rect(&key("delay")).unwrap();
    let low = v.rack_to_view(Point::new(r.x0 + 30.0, r.y1 - 3.0));
    let _ = press_with(
        &mut v,
        &doc,
        low,
        PointerButton::Primary,
        2,
        Modifiers::NONE,
    );
    assert!(!v.is_text_owner());
}

#[test]
fn a_device_inside_a_container_is_renamed_with_its_path_key() {
    let d = layered();
    let mut v = scoped_view(d.doc(), &["layer"]);
    let _ = double_click_name(&mut v, d.doc(), "pad");
    assert_eq!(
        v.renaming_device(&key("pad")).map(str::to_owned),
        d.doc().resolve(&key("layer/pad")).map(|x| x.name.clone())
    );
    type_text(&mut v, d.doc(), "2");
    let out = press_key(&mut v, d.doc(), Key::Enter);
    let Some(Command::Rename { key: k, .. }) = commands(&out).pop() else {
        panic!("{out:?}");
    };
    assert_eq!(k, key("layer/pad"));
}

#[test]
fn leaving_the_container_drops_an_open_editor() {
    let d = layered();
    let mut v = scoped_view(d.doc(), &["layer"]);
    let _ = double_click_name(&mut v, d.doc(), "pad");
    v.set_scope(&path(&[]));
    v.sync(d.doc(), &registry());
    assert!(!v.is_text_owner());
}

#[test]
fn the_name_has_a_tooltip_and_the_editor_matches_golden() {
    let doc = reference_doc();
    let mut v = plain_view(&doc, &registry());
    let p = v.name_center(&key("delay")).unwrap();
    assert_eq!(v.help_at(p).unwrap().text, "Double-click to rename");
    let _ = double_click_name(&mut v, &doc, "delay");
    type_text(&mut v, &doc, " 2");
    check(
        "rack_rename_editing.json",
        &paint_rack(&mut v, &doc).to_json(),
    );
}
