//! RackView behaviour through its public contract.

mod support;

use atomik_device_sdk::{RACK_UNIT, RACK_WIDTH};
use atomik_model::{Command, RackDoc};
use atomik_ui::kurbo::{Point, Vec2};
use atomik_ui::widget::{KNOB_PIXELS_PER_RANGE, current_value};
use atomik_ui::{
    CableVisibility, DisplayList, Flip, Key, Modifiers, PointerButton, RackView, SocketHighlight,
    UiAction, UiEvent,
};
use support::*;

fn param(doc: &RackDoc, reg: &atomik_device_sdk::Registry, dev: usize, key: &str) -> f32 {
    let d = &doc.devices[dev];
    let desc = reg.get(&d.type_id).unwrap().descriptor;
    current_value(d, desc.param_by_key(key).unwrap())
}

fn set_param(actions: &[UiAction]) -> Vec<(String, f32, Option<u64>)> {
    commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            Command::SetParam {
                param,
                value,
                merge,
                ..
            } => Some((param, value, merge)),
            _ => None,
        })
        .collect()
}

fn key_event(key: Key, modifiers: Modifiers) -> UiEvent {
    UiEvent::Key {
        key,
        modifiers,
        pressed: true,
    }
}

fn back_view() -> (RackDoc, atomik_device_sdk::Registry, RackView) {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    v.set_flip(Flip::Back);
    (doc, reg, v)
}

// ---------------------------------------------------------------- layout, scroll, zoom

#[test]
fn devices_stack_in_rack_order_below_the_interface() {
    let (doc, reg) = (reference_doc(), registry());
    let v = view(&doc, &reg);
    let iface = v.device_rect(&key("interface")).unwrap();
    assert!((iface.height() - f64::from(RACK_UNIT)).abs() < 1e-9);
    let mut y = iface.y1;
    for d in &doc.devices {
        let r = v.device_rect(&d.key).unwrap();
        assert!((r.y0 - y).abs() < 1e-9, "{} starts at {}", d.key.0, r.y0);
        assert!((r.width() - f64::from(RACK_WIDTH)).abs() < 1e-9);
        let units = r.height() / f64::from(RACK_UNIT);
        assert!((units - units.round()).abs() < 1e-9 && units >= 1.0);
        y = r.y1;
    }
    assert!((v.content_height() - y).abs() < 1e-9);
}

#[test]
fn folded_devices_are_half_a_unit() {
    let (mut doc, reg) = (reference_doc(), registry());
    doc.devices[0].folded = true;
    let v = view(&doc, &reg);
    let r = v.device_rect(&key("synth")).unwrap();
    assert!((r.height() - f64::from(RACK_UNIT) / 2.0).abs() < 1e-9);
    assert!(v.control_center(&key("synth"), "filter.cutoff").is_none());
}

#[test]
fn zoom_is_clamped_and_wheel_zoom_keeps_the_anchor_fixed() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    v.set_zoom(5.0);
    assert!((v.zoom() - 2.0).abs() < 1e-9);
    v.set_zoom(0.1);
    assert!((v.zoom() - 0.5).abs() < 1e-9);
    v.set_zoom(1.0);
    // Shorter than the content, so the scroll range allows keeping the anchor in place.
    let _ = v.handle(&UiEvent::Resize((1000.0, 300.0).into()), &doc, &reg);
    v.set_scroll(100.0);
    let anchor = Point::new(500.0, 150.0);
    let before = v.view_to_rack(anchor);
    let _ = v.handle(
        &UiEvent::Wheel {
            delta: Vec2::new(0.0, 2.0),
            pos: anchor,
            modifiers: Modifiers::CTRL,
        },
        &doc,
        &reg,
    );
    assert!(v.zoom() > 1.0);
    let after = v.view_to_rack(anchor);
    assert!((before - after).hypot() < 0.5, "{before:?} vs {after:?}");
}

#[test]
fn scroll_is_clamped_to_content() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    let _ = v.handle(&UiEvent::Resize((1000.0, 200.0).into()), &doc, &reg);
    v.set_scroll(-50.0);
    assert!(v.scroll().abs() < 1e-9);
    v.set_scroll(1e9);
    let max = v.scroll();
    assert!(max > 0.0 && max < v.content_height() * v.zoom() + 100.0);
    let _ = v.handle(
        &UiEvent::Wheel {
            delta: Vec2::new(0.0, 1.0),
            pos: Point::new(10.0, 10.0),
            modifiers: Modifiers::NONE,
        },
        &doc,
        &reg,
    );
    assert!(v.scroll() < max);
}

#[test]
fn hidpi_scale_is_applied_by_paint() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    let _ = v.handle(&UiEvent::ScaleChanged(2.0), &doc, &reg);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    let first_transform = dl.ops().iter().find_map(|op| match op {
        atomik_ui::Op::PushTransform(t) => Some(*t),
        _ => None,
    });
    assert_eq!(first_transform.unwrap().as_coeffs()[0], 2.0);
    assert_eq!(v.physical_size(), (2000, 2400));
}

// ---------------------------------------------------------------- knobs and widgets

#[test]
fn knob_drag_emits_merged_set_params_and_end_merge() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    let c = v.control_center(&key("delay"), "mix").unwrap();
    let start = param(&doc, &reg, 1, "mix");
    assert!(v.handle(&down(c), &doc, &reg).is_empty());
    let a = v.handle(
        &mv(c - Vec2::new(0.0, KNOB_PIXELS_PER_RANGE / 4.0)),
        &doc,
        &reg,
    );
    let s = set_param(&a);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].0, "mix");
    assert!((s[0].1 - (start + 0.25)).abs() < 1e-4);
    let merge = s[0].2.expect("drags merge");
    let b = v.handle(
        &mv(c - Vec2::new(0.0, KNOB_PIXELS_PER_RANGE / 2.0)),
        &doc,
        &reg,
    );
    assert_eq!(set_param(&b)[0].2, Some(merge));
    let u = v.handle(&up(c), &doc, &reg);
    assert!(u.contains(&UiAction::EndMerge));
    // A second drag gets a fresh merge id.
    let _ = v.handle(&down(c), &doc, &reg);
    let a2 = v.handle(&mv(c - Vec2::new(0.0, 10.0)), &doc, &reg);
    assert_ne!(set_param(&a2)[0].2, Some(merge));
}

#[test]
fn shift_drag_is_ten_times_finer() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    let c = v.control_center(&key("delay"), "mix").unwrap();
    let start = param(&doc, &reg, 1, "mix");
    let _ = v.handle(&down(c), &doc, &reg);
    let a = v.handle(
        &mv_with(
            c - Vec2::new(0.0, KNOB_PIXELS_PER_RANGE / 2.0),
            Modifiers::SHIFT,
        ),
        &doc,
        &reg,
    );
    assert!((set_param(&a)[0].1 - (start + 0.05)).abs() < 1e-4);
}

#[test]
fn knob_values_are_clamped_by_the_descriptor() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    let c = v.control_center(&key("delay"), "feedback").unwrap();
    let _ = v.handle(&down(c), &doc, &reg);
    let a = v.handle(&mv(c - Vec2::new(0.0, 5000.0)), &doc, &reg);
    assert!((set_param(&a)[0].1 - 0.95).abs() < 1e-6);
    let b = v.handle(&mv(c + Vec2::new(0.0, 5000.0)), &doc, &reg);
    assert!(set_param(&b)[0].1.abs() < 1e-6);
}

#[test]
fn zoom_does_not_change_drag_sensitivity() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    let _ = v.handle(&UiEvent::Resize((2000.0, 2400.0).into()), &doc, &reg);
    v.set_zoom(2.0);
    v.set_scroll(0.0);
    let c = v.control_center(&key("synth"), "lfo.rate").unwrap();
    let d = &reg.get("atomik.poly-synth").unwrap().descriptor;
    let p = d.param_by_key("lfo.rate").unwrap();
    let start = p.to_normalized(param(&doc, &reg, 0, "lfo.rate"));
    let _ = v.handle(&down(c), &doc, &reg);
    let a = v.handle(&mv(c - Vec2::new(0.0, 20.0)), &doc, &reg);
    let got = p.to_normalized(set_param(&a)[0].1);
    assert!((got - (start + 0.1)).abs() < 1e-3, "{got} vs {start}");
}

#[test]
fn double_click_resets_to_default() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    let c = v.control_center(&key("delay"), "feedback").unwrap();
    let a = v.handle(&down_with(c, PointerButton::Primary, 2), &doc, &reg);
    assert_eq!(set_param(&a), vec![("feedback".to_string(), 0.4, None)]);
}

#[test]
fn wheel_steps_a_knob() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    let c = v.control_center(&key("delay"), "mix").unwrap();
    let a = v.handle(
        &UiEvent::Wheel {
            delta: Vec2::new(0.0, 1.0),
            pos: c,
            modifiers: Modifiers::NONE,
        },
        &doc,
        &reg,
    );
    let s = set_param(&a);
    assert!((s[0].1 - 0.32).abs() < 1e-4, "{s:?}");
}

#[test]
fn toggle_and_choice_clicks() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    let t = v.control_center(&key("delay"), "ping_pong").unwrap();
    let a = v.handle(&down(t), &doc, &reg);
    assert_eq!(set_param(&a), vec![("ping_pong".to_string(), 0.0, None)]);
    let _ = v.handle(&up(t), &doc, &reg);
    let ch = v.control_center(&key("synth"), "osc2.wave").unwrap();
    let a = v.handle(&down(ch), &doc, &reg);
    assert_eq!(set_param(&a), vec![("osc2.wave".to_string(), 2.0, None)]);
    let _ = v.handle(&up(ch), &doc, &reg);
    let b = v.handle(&down_with(ch, PointerButton::Secondary, 1), &doc, &reg);
    assert_eq!(set_param(&b), vec![("osc2.wave".to_string(), 0.0, None)]);
}

#[test]
fn faders_drag_vertically() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    let c = v.control_center(&key("mixer"), "ch1.level").unwrap();
    let _ = v.handle(&down(c), &doc, &reg);
    let a = v.handle(&mv(c - Vec2::new(0.0, 10.0)), &doc, &reg);
    let s = set_param(&a);
    assert_eq!(s[0].0, "ch1.level");
    assert!(s[0].1 > -6.0);
}

#[test]
fn tooltip_shows_value_while_dragging() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    let c = v.control_center(&key("delay"), "mix").unwrap();
    let _ = v.handle(&down(c), &doc, &reg);
    let _ = v.handle(&mv(c - Vec2::new(0.0, 20.0)), &doc, &reg);
    assert_eq!(v.tooltip().as_deref(), Some("Mix  40 %"));
    let _ = v.handle(&up(c), &doc, &reg);
    assert_eq!(v.tooltip(), None);
}

// ---------------------------------------------------------------- cables

#[test]
fn sockets_are_only_interactive_on_the_back() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    assert!(v.socket_center(&sref("synth", "out_l")).is_none());
    v.set_flip(Flip::Back);
    assert!(v.socket_center(&sref("synth", "out_l")).is_some());
    assert!(v.control_center(&key("synth"), "filter.cutoff").is_none());
    assert!(v.socket_center(&sref("interface", "out_2")).is_some());
}

#[test]
fn dragging_a_cable_highlights_compatible_sockets_and_connects_on_drop() {
    let (doc, reg, mut v) = back_view();
    let from = v.socket_center(&sref("synth", "out_r")).unwrap();
    let to = v.socket_center(&sref("mixer", "ch2_in_l")).unwrap();
    assert!(v.handle(&down(from), &doc, &reg).is_empty());
    let _ = v.handle(&mv(to + Vec2::new(30.0, 0.0)), &doc, &reg);
    assert_eq!(
        v.socket_highlight(&sref("mixer", "ch2_in_l")),
        SocketHighlight::Compatible
    );
    // Wrong kind, wrong direction, same device.
    for s in [
        ("mixer", "ch2_level_cv_in"),
        ("delay", "out_l"),
        ("synth", "notes_in"),
    ] {
        assert_eq!(
            v.socket_highlight(&sref(s.0, s.1)),
            SocketHighlight::Incompatible,
            "{s:?}"
        );
    }
    assert_eq!(
        v.socket_highlight(&sref("synth", "out_r")),
        SocketHighlight::Source
    );
    let _ = v.handle(&mv(to), &doc, &reg);
    let a = v.handle(&up(to), &doc, &reg);
    assert_eq!(
        commands(&a),
        vec![Command::Connect(cable(
            ("synth", "out_r"),
            ("mixer", "ch2_in_l")
        ))]
    );
    assert_eq!(
        v.socket_highlight(&sref("mixer", "ch2_in_l")),
        SocketHighlight::Idle
    );
}

#[test]
fn input_to_output_drag_connects_in_the_right_direction() {
    let (doc, reg, mut v) = back_view();
    let from = v.socket_center(&sref("mixer", "ch3_in_l")).unwrap();
    let to = v.socket_center(&sref("synth", "out_r")).unwrap();
    let _ = v.handle(&down(from), &doc, &reg);
    let _ = v.handle(&mv(to), &doc, &reg);
    let a = v.handle(&up(to), &doc, &reg);
    assert_eq!(
        commands(&a),
        vec![Command::Connect(cable(
            ("synth", "out_r"),
            ("mixer", "ch3_in_l")
        ))]
    );
}

#[test]
fn dropping_on_an_incompatible_socket_or_nowhere_cancels() {
    let (doc, reg, mut v) = back_view();
    let from = v.socket_center(&sref("synth", "out_r")).unwrap();
    let bad = v.socket_center(&sref("mixer", "ch2_level_cv_in")).unwrap();
    let _ = v.handle(&down(from), &doc, &reg);
    let _ = v.handle(&mv(bad), &doc, &reg);
    assert!(commands(&v.handle(&up(bad), &doc, &reg)).is_empty());
    let _ = v.handle(&down(from), &doc, &reg);
    let nowhere = Point::new(5.0, 5.0);
    let _ = v.handle(&mv(nowhere), &doc, &reg);
    assert!(commands(&v.handle(&up(nowhere), &doc, &reg)).is_empty());
}

#[test]
fn escape_cancels_a_cable_drag() {
    let (doc, reg, mut v) = back_view();
    let from = v.socket_center(&sref("synth", "out_r")).unwrap();
    let to = v.socket_center(&sref("mixer", "ch2_in_l")).unwrap();
    let _ = v.handle(&down(from), &doc, &reg);
    let _ = v.handle(&mv(to), &doc, &reg);
    let _ = v.handle(&key_event(Key::Escape, Modifiers::NONE), &doc, &reg);
    assert_eq!(
        v.socket_highlight(&sref("mixer", "ch2_in_l")),
        SocketHighlight::Idle
    );
    assert!(commands(&v.handle(&up(to), &doc, &reg)).is_empty());
}

#[test]
fn dragging_from_a_connected_input_detaches_and_redrags() {
    let (doc, reg, mut v) = back_view();
    let input = v.socket_center(&sref("delay", "in_l")).unwrap();
    let a = v.handle(&down(input), &doc, &reg);
    assert_eq!(
        commands(&a),
        vec![Command::Disconnect {
            to: sref("delay", "in_l")
        }]
    );
    // The loose end now belongs to synth.out_l.
    assert_eq!(
        v.socket_highlight(&sref("synth", "out_l")),
        SocketHighlight::Source
    );
    let to = v.socket_center(&sref("mixer", "ch2_in_r")).unwrap();
    let _ = v.handle(&mv(to), &doc, &reg);
    let b = v.handle(&up(to), &doc, &reg);
    assert_eq!(
        commands(&b),
        vec![Command::Connect(cable(
            ("synth", "out_l"),
            ("mixer", "ch2_in_r")
        ))]
    );
}

#[test]
fn dropping_on_an_occupied_input_replaces_its_cable() {
    let (doc, reg, mut v) = back_view();
    let from = v.socket_center(&sref("synth", "out_r")).unwrap();
    let to = v.socket_center(&sref("delay", "in_l")).unwrap();
    let _ = v.handle(&down(from), &doc, &reg);
    let _ = v.handle(&mv(to), &doc, &reg);
    let a = v.handle(&up(to), &doc, &reg);
    assert_eq!(
        commands(&a),
        vec![
            Command::Disconnect {
                to: sref("delay", "in_l")
            },
            Command::Connect(cable(("synth", "out_r"), ("delay", "in_l"))),
        ]
    );
}

#[test]
fn cable_visibility_modes() {
    let (doc, reg, mut v) = back_view();
    assert_eq!(v.visible_cables(&doc).len(), 4);
    v.set_cable_visibility(CableVisibility::Hide);
    assert!(v.visible_cables(&doc).is_empty());
    v.set_cable_visibility(CableVisibility::HoverOnly);
    assert!(v.visible_cables(&doc).is_empty());
    let mixer = v.device_rect(&key("mixer")).unwrap();
    let inside = v.rack_to_view(Point::new(mixer.x0 + 400.0, mixer.y0 + 3.0));
    let _ = v.handle(&mv(inside), &doc, &reg);
    let shown = v.visible_cables(&doc);
    assert_eq!(shown.len(), 2, "{shown:?}");
    v.set_cable_visibility(CableVisibility::Show);
    v.set_flip(Flip::Front);
    assert!(v.visible_cables(&doc).is_empty());
}

#[test]
fn hovering_a_cable_highlights_it() {
    let (doc, reg, mut v) = back_view();
    let c = cable(("synth", "lfo_cv_out"), ("delay", "time_cv_in"));
    let mid = v.cable_midpoint(&c).unwrap();
    let _ = v.handle(&mv(mid), &doc, &reg);
    assert_eq!(v.hovered_cable(), Some(&c));
    assert_eq!(v.socket_highlight(&c.from), SocketHighlight::Hovered);
    assert_eq!(v.socket_highlight(&c.to), SocketHighlight::Hovered);
}

#[test]
fn cables_sag_proportionally_to_horizontal_distance() {
    let near = atomik_ui::cables::cable_curve(Point::new(0.0, 0.0), Point::new(50.0, 0.0));
    let far = atomik_ui::cables::cable_curve(Point::new(0.0, 0.0), Point::new(500.0, 0.0));
    let sag = |c: atomik_ui::kurbo::CubicBez| c.p1.y.max(c.p2.y);
    assert!(sag(near) > 0.0);
    assert!(sag(far) > sag(near) * 3.0);
}

// ---------------------------------------------------------------- devices, keys, browser

#[test]
fn clicking_a_device_selects_it_and_delete_removes_it() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    let p = v.plate_center(&key("delay")).unwrap();
    let a = v.handle(&down(p), &doc, &reg);
    assert!(a.contains(&UiAction::Select(key("delay"))));
    let _ = v.handle(&up(p), &doc, &reg);
    assert_eq!(v.selected(), Some(&key("delay")));
    // Delete is the `edit.delete` command the shell resolves; the key itself does nothing here.
    let d = v.command("edit.delete", None, &doc, &reg);
    assert_eq!(
        commands(d.actions()),
        vec![Command::Workflow(
            atomik_model::WorkflowCommand::DeleteDevices {
                keys: vec![key("delay")]
            }
        )],
        "auto-routing is on: the removal keeps the audio path"
    );
    let k = v.handle(&key_event(Key::Delete, Modifiers::NONE), &doc, &reg);
    assert!(commands(&k).is_empty());
}

#[test]
fn fold_button_toggles_fold() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    let f = v.fold_button_center(&key("mixer")).unwrap();
    let a = v.handle(&down(f), &doc, &reg);
    assert_eq!(
        commands(&a),
        vec![Command::SetFolded {
            key: key("mixer"),
            folded: true
        }]
    );
}

#[test]
fn dragging_a_plate_reorders_devices() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    let p = v.plate_center(&key("mixer")).unwrap();
    let synth = v.device_rect(&key("synth")).unwrap();
    let target = v.rack_to_view(Point::new(60.0, synth.y0 + 5.0));
    let _ = v.handle(&down(p), &doc, &reg);
    let _ = v.handle(&mv(target), &doc, &reg);
    let a = v.handle(&up(target), &doc, &reg);
    assert_eq!(
        commands(&a),
        vec![Command::Workflow(
            atomik_model::WorkflowCommand::MoveDevicesToSlot {
                keys: vec![key("mixer")],
                slot: atomik_model::RackSlot {
                    column: 0,
                    index: 0
                }
            }
        )]
    );
}

#[test]
fn undo_redo_are_commands_the_view_does_not_handle_as_keys() {
    // Undo and redo are registry commands (Global); the shell resolves the chord, so a remap
    // applies and the view never acts on Ctrl+Z by itself.
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    let z = |m| key_event(Key::Char('z'), m);
    let ctrl_shift = Modifiers {
        shift: true,
        ..Modifiers::CTRL
    };
    for e in [
        z(Modifiers::CTRL),
        key_event(Key::Char('Z'), ctrl_shift),
        key_event(Key::Char('y'), Modifiers::CTRL),
        z(Modifiers::NONE),
    ] {
        assert!(v.handle(&e, &doc, &reg).is_empty());
    }
}

#[test]
fn keyboard_focus_and_arrow_keys_adjust_controls() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    let _ = v.handle(&key_event(Key::Tab, Modifiers::NONE), &doc, &reg);
    let (dev, param) = v.focused_control().unwrap();
    assert_eq!(dev, key("synth"));
    assert_eq!(param, "osc1.wave");
    let _ = v.handle(&key_event(Key::Tab, Modifiers::NONE), &doc, &reg);
    let (_, param) = v.focused_control().unwrap();
    let a = v.handle(&key_event(Key::Up, Modifiers::NONE), &doc, &reg);
    let s = set_param(&a);
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].0, param);
}

#[test]
fn browser_filters_and_double_click_adds_below_selection() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    v.set_browser_visible(true);
    // Two more with the console devices shown (the default).
    assert_eq!(v.browser_rows(&reg).len(), 18);
    v.set_show_console_devices(false);
    assert_eq!(v.browser_rows(&reg).len(), 16); // arpeggiator, line mixer, poly synth, sampler, drum machine, loop slicer, delay, reverb, compressor, eq, distortion, mastering, modulation, step sequencer, container, character
    for ch in "dela".chars() {
        let _ = v.handle(&key_event(Key::Char(ch), Modifiers::NONE), &doc, &reg);
    }
    assert_eq!(v.browser_rows(&reg), vec!["atomik.stereo-delay"]);
    let _ = v.handle(&key_event(Key::Backspace, Modifiers::NONE), &doc, &reg);
    let _ = v.handle(&key_event(Key::Backspace, Modifiers::NONE), &doc, &reg);
    let _ = v.handle(&key_event(Key::Backspace, Modifiers::NONE), &doc, &reg);
    let _ = v.handle(&key_event(Key::Backspace, Modifiers::NONE), &doc, &reg);
    assert_eq!(v.browser_rows(&reg).len(), 16); // as above (16 browsable devices, the container included)
    // Select the synth, then double-click the mixer row.
    let p = v.plate_center(&key("synth")).unwrap();
    let _ = v.handle(&down(p), &doc, &reg);
    let _ = v.handle(&up(p), &doc, &reg);
    let row = v.browser_row_center(3).unwrap(); // atomik.drum-machine (rows sort by name, the players last: character, compressor, distortion, drum machine, ..., arpeggiator, step sequencer)
    let a = v.handle(&down_with(row, PointerButton::Primary, 2), &doc, &reg);
    assert_eq!(
        commands(&a),
        vec![Command::Workflow(
            atomik_model::WorkflowCommand::CreateInstrument {
                index: 1,
                source: atomik_model::CreateSource::Type("atomik.drum-machine".into()),
                key: None,
                options: atomik_model::CreateOptions::default(),
            }
        )]
    );
}

#[test]
fn the_view_says_when_it_owns_text_input() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    assert!(!v.is_text_owner(), "nothing is being typed into");
    v.set_browser_visible(true);
    assert!(v.is_text_owner(), "the browser filter is focused");
    let _ = v.handle(&key_event(Key::Escape, Modifiers::NONE), &doc, &reg);
    assert!(!v.is_text_owner(), "Esc gives the filter up");
    v.set_browser_visible(false);
    assert!(!v.is_text_owner());
}

#[test]
fn double_click_on_empty_rack_opens_the_browser() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    let below = v.rack_to_view(Point::new(300.0, v.content_height() + 20.0));
    let a = v.handle(&down_with(below, PointerButton::Primary, 2), &doc, &reg);
    assert!(a.contains(&UiAction::OpenBrowser));
    assert!(v.browser_visible());
}

#[test]
fn needs_repaint_is_cleared_by_paint_and_set_by_changes() {
    let (mut doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    assert!(v.needs_repaint());
    v.paint(&mut DisplayList::new(), &doc, &reg);
    assert!(!v.needs_repaint());
    v.sync(&doc, &reg);
    assert!(!v.needs_repaint(), "an unchanged doc does not repaint");
    doc.devices[1].params.insert("mix".into(), 0.9);
    v.sync(&doc, &reg);
    assert!(v.needs_repaint());
    v.paint(&mut DisplayList::new(), &doc, &reg);
    v.set_flip(Flip::Back);
    assert!(v.needs_repaint());
}

#[test]
fn rejections_are_shown_until_the_next_click() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    v.notify_rejected("cable rejected: cycle");
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    assert!(dl.texts().contains(&"cable rejected: cycle"));
    let _ = v.handle(&down(Point::new(5.0, 5.0)), &doc, &reg);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    assert!(!dl.texts().contains(&"cable rejected: cycle"));
}

#[test]
fn only_changed_devices_are_re_recorded() {
    let (mut doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    v.paint(&mut DisplayList::new(), &doc, &reg);
    let before = v.panel_revisions();
    doc.devices[1].params.insert("mix".into(), 0.9);
    v.sync(&doc, &reg);
    v.paint(&mut DisplayList::new(), &doc, &reg);
    let after = v.panel_revisions();
    assert_eq!(before.len(), after.len());
    let changed: Vec<_> = before
        .iter()
        .zip(&after)
        .filter(|(a, b)| a != b)
        .map(|(a, _)| a.0.0.clone())
        .collect();
    assert_eq!(changed, vec!["delay".to_string()]);
}

// ---------------------------------------------------------------- host inputs

#[test]
fn interface_shows_host_inputs_beside_the_outputs() {
    let reg = registry();
    let mut doc = reference_doc();
    doc.input_channels = 2;
    let mut v = view(&doc, &reg);
    v.set_flip(Flip::Back);
    v.sync(&doc, &reg);
    assert!(v.socket_center(&sref("interface", "in_1")).is_some());
    assert!(v.socket_center(&sref("interface", "in_2")).is_some());
    assert!(v.socket_center(&sref("interface", "in_3")).is_none());
    assert!(v.socket_center(&sref("interface", "out_2")).is_some());
    // The input count follows the document.
    doc.input_channels = 3;
    v.sync(&doc, &reg);
    assert!(v.socket_center(&sref("interface", "in_3")).is_some());
    doc.input_channels = 0;
    v.sync(&doc, &reg);
    assert!(v.socket_center(&sref("interface", "in_1")).is_none());
}

#[test]
fn a_cable_can_be_dragged_from_a_host_input() {
    let reg = registry();
    let mut doc = reference_doc();
    doc.input_channels = 2;
    let mut v = view(&doc, &reg);
    v.set_flip(Flip::Back);
    v.sync(&doc, &reg);
    let from = v.socket_center(&sref("interface", "in_1")).unwrap();
    let to = v.socket_center(&sref("mixer", "ch2_in_l")).unwrap();
    let mut actions = v.handle(&down(from), &doc, &reg);
    actions.extend(v.handle(&mv(to + Vec2::new(30.0, 0.0)), &doc, &reg));
    actions.extend(v.handle(&mv(to), &doc, &reg));
    actions.extend(v.handle(&up(to), &doc, &reg));
    assert_eq!(
        commands(&actions),
        vec![Command::Connect(cable(
            ("interface", "in_1"),
            ("mixer", "ch2_in_l")
        ))]
    );
}
