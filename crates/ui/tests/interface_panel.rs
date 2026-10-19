//! The Audio I/O panel and the master panel of (UI half):
//! socket state lamps, peak meters and clip lamps, the send rows with the name of their effect,
//! the EDIT buttons, the control-room pick list and the key-in state. Display-list goldens
//! regenerate with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test interface_panel`.

mod support;

use atomik_model::{Cable, Command, DeviceKey, Document, RackDoc, SocketRef};
use atomik_ui::{IoSocketState, RackView, Theme, UiAction, UiEvent};
use support::starter::*;
use support::*;

use IoSocketState::{Connected, Free, Unavailable};

fn fresh(doc: &RackDoc, reg: &atomik_device_sdk::Registry) -> RackView {
    let mut v = RackView::new(Theme::dark());
    let _ = v.handle(&UiEvent::Resize(VIEW), doc, reg);
    v.sync(doc, reg);
    v
}

fn cab(from: (&str, &str), to: (&str, &str)) -> Cable {
    cable(from, to)
}

/// The starter with a sine source on `in_1` and the master key keyed from the second input.
fn io_doc(reg: &atomik_device_sdk::Registry) -> Document {
    let mut d = two_instruments(reg);
    d.apply(
        Command::Connect(cab(("interface", "in_1"), ("synth", "gate_in"))),
        reg,
    )
    .ok();
    d
}

#[test]
fn the_panel_has_one_name_and_a_matching_caption() {
    let reg = registry();
    let d = starter();
    let v = fresh(d.doc(), &reg);
    let json = {
        let mut v = fresh(d.doc(), &reg);
        paint_json(&mut v, d.doc(), &reg)
    };
    assert!(json.contains("Audio I/O"));
    assert!(json.contains("AUDIO I/O"));
    assert!(!json.contains("INTERFACE"), "the old caption is gone");
    assert!(!json.contains("Audio Interface"));
    drop(v);
}

#[test]
fn the_lamps_follow_the_cables_and_the_host_device() {
    let reg = registry();
    let d = starter();
    let doc = d.doc();
    let mut v = fresh(doc, &reg);
    let n = v.io_socket_count();
    assert_eq!(
        n,
        usize::from(doc.input_channels) + usize::from(doc.output_channels)
    );
    // The offline sink has every channel: out_1 and out_2 are cabled by the starter.
    let outs = usize::from(doc.input_channels);
    assert_eq!(v.io_states()[outs], Connected);
    assert_eq!(v.io_states()[outs + 1], Connected);
    assert!(v.io_states()[..outs].iter().all(|s| *s == Free));
    // A device with one output channel: out_2 is cabled but missing.
    v.set_io_host_channels(Some((doc.input_channels, 1)));
    assert_eq!(v.io_states()[outs], Connected);
    assert_eq!(v.io_states()[outs + 1], Unavailable);
    // Back to the offline sink.
    v.set_io_host_channels(None);
    assert_eq!(v.io_states()[outs + 1], Connected);
}

#[test]
fn a_new_cable_changes_the_lamp_at_once() {
    let reg = registry();
    let mut d = two_instruments(&reg);
    let mut v = fresh(d.doc(), &reg);
    assert_eq!(v.io_states()[0], Free);
    d.apply(
        Command::Connect(cab(("interface", "in_1"), ("synth", "gate_in"))),
        &reg,
    )
    .ok();
    let has_cable = d
        .doc()
        .cables
        .iter()
        .any(|c| c.from.device.0 == "interface");
    v.sync(d.doc(), &reg);
    assert_eq!(v.io_states()[0] == Connected, has_cable);
}

#[test]
fn meters_follow_the_feed_and_settle() {
    let reg = registry();
    let d = starter();
    let mut v = fresh(d.doc(), &reg);
    let n = v.io_socket_count();
    let mut peaks = vec![0.0f32; n];
    peaks[0] = 0.5;
    v.set_io_peaks(&peaks);
    assert!((v.io_levels()[0] + 6.0).abs() < 0.3);
    assert!(
        v.io_levels()[1..].iter().all(|l| *l <= -59.9),
        "other sockets stay dark"
    );
    assert!(v.io_clips().iter().all(|c| !c));
    // Silence releases the meter and the view stops asking for repaints.
    for _ in 0..40 {
        v.set_io_peaks(&vec![0.0; n]);
    }
    assert!(v.io_levels().iter().all(|l| *l <= -59.9));
    let _ = paint_json(&mut v, d.doc(), &reg);
    v.set_io_peaks(&vec![0.0; n]);
    assert!(!v.needs_repaint(), "a quiet panel does not repaint");
}

#[test]
fn a_hot_peak_latches_the_clip_lamp_and_a_click_resets_it() {
    let reg = registry();
    let d = starter();
    let doc = d.doc();
    let mut v = fresh(doc, &reg);
    let n = v.io_socket_count();
    let outs = usize::from(doc.input_channels);
    let mut peaks = vec![0.0f32; n];
    peaks[outs] = 1.5;
    peaks[outs + 1] = 0.9;
    v.set_io_peaks(&peaks);
    assert!(v.io_clips()[outs] && !v.io_clips()[outs + 1]);
    assert!(v.io_clips().iter().filter(|c| **c).count() == 1);
    // The lamp stays lit when the signal is gone.
    v.set_io_peaks(&vec![0.0; n]);
    assert!(v.io_clips()[outs]);
    // A click on the lamp of another socket does nothing, on its own lamp resets it.
    let other = v.io_clip_center(outs + 1).unwrap();
    let _ = v.handle(&down(other), doc, &reg);
    let _ = v.handle(&up(other), doc, &reg);
    assert!(v.io_clips()[outs]);
    let at = v.io_clip_center(outs).unwrap();
    let _ = v.handle(&down(at), doc, &reg);
    let _ = v.handle(&up(at), doc, &reg);
    assert!(!v.io_clips()[outs]);
}

#[test]
fn a_click_on_the_plate_resets_every_clip_lamp() {
    let reg = registry();
    let d = starter();
    let doc = d.doc();
    let mut v = fresh(doc, &reg);
    let n = v.io_socket_count();
    v.set_io_peaks(&vec![2.0; n]);
    assert!(v.io_clips().iter().all(|c| *c));
    let at = v.io_header_center().unwrap();
    let _ = v.handle(&down(at), doc, &reg);
    let _ = v.handle(&up(at), doc, &reg);
    assert!(v.io_clips().iter().all(|c| !c));
}

#[test]
fn the_cells_explain_themselves_in_a_tooltip() {
    let reg = registry();
    let d = starter();
    let doc = d.doc();
    let mut v = fresh(doc, &reg);
    v.set_io_host_channels(Some((doc.input_channels, 1)));
    let outs = usize::from(doc.input_channels);
    let at = v.io_clip_center(outs + 1).unwrap();
    let _ = v.handle(&mv(at), doc, &reg);
    let tip = v.tooltip().unwrap();
    assert!(
        tip.contains("Output 2") && tip.contains("no such channel"),
        "{tip}"
    );
    let at = v.io_clip_center(0).unwrap();
    let _ = v.handle(&mv(at), doc, &reg);
    assert!(v.tooltip().unwrap().contains("free"));
    let at = v.io_header_center().unwrap();
    let _ = v.handle(&mv(at), doc, &reg);
    assert!(v.tooltip().unwrap().contains("reset"));
}

#[test]
fn golden_interface_panel_states() {
    let reg = registry();
    let d = starter();
    let doc = d.doc();
    let mut v = fresh(doc, &reg);
    v.set_io_host_channels(Some((doc.input_channels, 1)));
    check_golden(
        "interface_panel_states_1x.json",
        &paint_json(&mut v, doc, &reg),
    );
}

#[test]
fn golden_interface_panel_meters() {
    let reg = registry();
    let d = starter();
    let doc = d.doc();
    let mut v = fresh(doc, &reg);
    let n = v.io_socket_count();
    let outs = usize::from(doc.input_channels);
    let mut peaks = vec![0.0f32; n];
    peaks[0] = 0.25;
    peaks[outs] = 1.5;
    peaks[outs + 1] = 0.7;
    v.set_io_peaks(&peaks);
    check_golden(
        "interface_panel_meters_1x.json",
        &paint_json(&mut v, doc, &reg),
    );
}

// ------------------------------------------------------------------ the master panel

fn master_doc(reg: &atomik_device_sdk::Registry) -> Document {
    let mut d = with_effects(reg);
    // Key the bus compressor from the bass: the cable switches `comp.key` on by itself.
    d.apply(
        Command::Connect(cab(("bass", "out_l"), ("master", "key_in"))),
        reg,
    )
    .ok();
    d
}

#[test]
fn the_master_rows_name_the_effect_on_their_slot() {
    let reg = registry();
    let d = master_doc(&reg);
    let doc = d.doc();
    let mut v = fresh(doc, &reg);
    let json = paint_json(&mut v, doc, &reg);
    assert!(json.contains("Stereo Delay") || json.contains("echo") || json.contains("Delay"));
    assert!(json.contains("free"));
    assert!(json.contains("EDIT"));
}

#[test]
fn edit_selects_the_effect_of_the_slot_and_scrolls_to_it() {
    let reg = registry();
    let d = master_doc(&reg);
    let doc = d.doc();
    let mut v = fresh(doc, &reg);
    v.set_selected(None);
    let at = v.return_edit_center(&key("master"), 0).unwrap();
    let out = v.handle(&down(at), doc, &reg);
    let _ = v.handle(&up(at), doc, &reg);
    assert!(
        out.iter()
            .any(|a| matches!(a, UiAction::Select(k) if *k == key("echo"))),
        "{out:?}"
    );
    assert_eq!(v.selected(), Some(&key("echo")));
    // The effect is on screen after the jump.
    let at = v.plate_center(&key("echo")).unwrap();
    assert!(at.y > 0.0 && at.y < VIEW.height, "{at:?}");
    assert!(v.scroll() > 0.0, "the rack scrolled to the effect");
}

#[test]
fn the_edit_button_names_the_effect_it_selects_in_its_tooltip() {
    let reg = registry();
    let d = master_doc(&reg);
    let doc = d.doc();
    let mut v = fresh(doc, &reg);
    let at = v.return_edit_center(&key("master"), 0).unwrap();
    let _ = v.handle(&mv(at), doc, &reg);
    assert_eq!(v.tooltip().as_deref(), Some("Edit Stereo Delay"));
    // A free slot has no effect to name.
    let at = v.return_edit_center(&key("master"), 4).unwrap();
    let _ = v.handle(&mv(at), doc, &reg);
    assert_eq!(v.tooltip().as_deref(), None);
}

#[test]
fn the_open_pick_list_paints_its_rows_marks_the_current_one_and_follows_the_pointer() {
    let reg = registry();
    let d = master_doc(&reg);
    let doc = d.doc();
    let mut v = fresh(doc, &reg);
    let closed = paint_json(&mut v, doc, &reg);
    let at = v.choice_center(&key("master"), "monitor.source").unwrap();
    let _ = v.handle(&down(at), doc, &reg);
    let _ = v.handle(&up(at), doc, &reg);
    let open = paint_json(&mut v, doc, &reg);
    assert!(open.len() > closed.len(), "the list adds its rows");
    for name in ["Main", "FX returns", "Send 3", "Return 8"] {
        assert!(open.contains(&format!("\"{name}\"")), "{name}");
    }
    assert_eq!(
        open.matches('\u{2022}').count(),
        1,
        "one bullet marks the current source (Main)"
    );
    // Hovering a row paints its highlight; leaving the rows takes it away again.
    let row = v.choice_menu_row_center(5).unwrap();
    let _ = v.handle(&mv(row), doc, &reg);
    let hovered = paint_json(&mut v, doc, &reg);
    assert_ne!(hovered, open, "the hovered row is highlighted");
    let _ = v.handle(&mv(atomik_ui::kurbo::Point::new(2.0, 2.0)), doc, &reg);
    assert_eq!(paint_json(&mut v, doc, &reg), open);
}

#[test]
fn edit_on_a_free_slot_does_nothing() {
    let reg = registry();
    let d = master_doc(&reg);
    let doc = d.doc();
    let mut v = fresh(doc, &reg);
    v.set_selected(None);
    let at = v.return_edit_center(&key("master"), 4).unwrap();
    let out = v.handle(&down(at), doc, &reg);
    let _ = v.handle(&up(at), doc, &reg);
    assert!(out.iter().all(|a| !matches!(a, UiAction::Select(_))));
    assert_eq!(v.selected(), None);
}

#[test]
fn the_source_is_a_pick_list_of_the_descriptor_entries() {
    let reg = registry();
    let d = master_doc(&reg);
    let doc = d.doc();
    let mut v = fresh(doc, &reg);
    let at = v.choice_center(&key("master"), "monitor.source").unwrap();
    let _ = v.handle(&down(at), doc, &reg);
    let _ = v.handle(&up(at), doc, &reg);
    let labels = v.choice_menu_labels();
    assert_eq!(labels.len(), 18);
    assert_eq!(&labels[..3], ["Main", "FX returns", "Send 1"]);
    assert_eq!(labels[17], "Return 8");
    // The list is the descriptor's own, not a copy.
    let desc = reg.get("atomik.master").unwrap().descriptor;
    let p = desc
        .params
        .iter()
        .find(|p| p.key == "monitor.source")
        .unwrap();
    let atomik_core::Unit::Choice(names) = p.unit else {
        panic!("choice")
    };
    assert_eq!(
        labels,
        names.iter().map(|n| n.to_string()).collect::<Vec<_>>()
    );
    // Pick "Return 5" (index 14).
    let row = v.choice_menu_row_center(14).unwrap();
    let out = v.handle(&down(row), doc, &reg);
    let _ = v.handle(&up(row), doc, &reg);
    assert_eq!(
        commands(&out),
        [Command::SetParam {
            key: key("master"),
            param: "monitor.source".into(),
            value: 14.0,
            merge: None
        }]
    );
    assert!(v.choice_menu_labels().is_empty(), "the list closes");
}

#[test]
fn a_press_outside_or_escape_closes_the_pick_list_without_a_change() {
    let reg = registry();
    let d = master_doc(&reg);
    let doc = d.doc();
    let mut v = fresh(doc, &reg);
    let at = v.choice_center(&key("master"), "monitor.source").unwrap();
    let _ = v.handle(&down(at), doc, &reg);
    assert_eq!(v.choice_menu_labels().len(), 18);
    let out = v.handle(
        &UiEvent::Key {
            key: atomik_ui::Key::Escape,
            modifiers: atomik_ui::Modifiers::NONE,
            pressed: true,
        },
        doc,
        &reg,
    );
    assert!(commands(&out).is_empty());
    assert!(v.choice_menu_labels().is_empty());
    // Open again and press on empty rack: closes, no command.
    let _ = v.handle(&down(at), doc, &reg);
    let away = atomik_ui::kurbo::Point::new(2.0, 2.0);
    let out = v.handle(&down(away), doc, &reg);
    assert!(commands(&out).is_empty());
    assert!(v.choice_menu_labels().is_empty());
}

#[test]
fn the_short_choices_still_cycle() {
    let reg = registry();
    let d = master_doc(&reg);
    let doc = d.doc();
    let mut v = fresh(doc, &reg);
    let at = v.choice_center(&key("synth"), "osc1.wave").unwrap();
    let out = v.handle(&down(at), doc, &reg);
    let _ = v.handle(&up(at), doc, &reg);
    assert!(v.choice_menu_labels().is_empty());
    assert_eq!(commands(&out).len(), 1);
}

#[test]
fn golden_master_panel_sends() {
    let reg = registry();
    let d = master_doc(&reg);
    let doc = d.doc();
    let mut v = fresh(doc, &reg);
    check_golden("master_panel_sends_1x.json", &paint_json(&mut v, doc, &reg));
}

#[test]
fn the_key_lamp_follows_the_cable() {
    let reg = registry();
    let uncabled = with_effects(&reg);
    let cabled = master_doc(&reg);
    let a = {
        let mut v = fresh(uncabled.doc(), &reg);
        paint_json(&mut v, uncabled.doc(), &reg)
    };
    let b = {
        let mut v = fresh(cabled.doc(), &reg);
        paint_json(&mut v, cabled.doc(), &reg)
    };
    assert_ne!(a, b);
    assert!(b.contains("from Poly Synth") || b.contains("from bass") || b.contains("from "));
    assert!(!a.contains("from "));
    let _ = DeviceKey("x".into());
    let _ = SocketRef {
        device: DeviceKey("x".into()),
        socket: "y".into(),
    };
    let _ = io_doc;
}
