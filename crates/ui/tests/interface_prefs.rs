//! The interface switches of the views: the knob range (how far a drag has to travel to sweep a
//! control) and the value tooltips of the rack. The defaults leave every gesture as it was.

mod support;

use atomik_model::{Command, RackDoc};
use atomik_ui::kurbo::Vec2;
use atomik_ui::widget::{KNOB_PIXELS_PER_RANGE, KnobRange, current_value};
use atomik_ui::{UiAction, UiEvent};
use support::*;

fn set_values(actions: &[UiAction]) -> Vec<f32> {
    commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            Command::SetParam { value, .. } => Some(value),
            _ => None,
        })
        .collect()
}

fn mix_of(doc: &RackDoc, reg: &atomik_device_sdk::Registry) -> f32 {
    let d = &doc.devices[1];
    let desc = reg.get(&d.type_id).unwrap().descriptor;
    current_value(d, desc.param_by_key("mix").unwrap())
}

/// The value a rack knob drag of `px` pixels up reaches with `range` set.
fn rack_drag(range: KnobRange, px: f64) -> f32 {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    v.set_knob_range(range);
    let c = v.control_center(&key("delay"), "mix").unwrap();
    let _ = v.handle(&down(c), &doc, &reg);
    let a = v.handle(&mv(c - Vec2::new(0.0, px)), &doc, &reg);
    set_values(&a)[0]
}

#[test]
fn a_rack_knob_drag_travels_twice_and_four_times_as_far_when_precise() {
    let (doc, reg) = (reference_doc(), registry());
    let start = mix_of(&doc, &reg);
    let normal = rack_drag(KnobRange::Normal, KNOB_PIXELS_PER_RANGE / 4.0);
    let precise = rack_drag(KnobRange::Precise, KNOB_PIXELS_PER_RANGE / 4.0);
    let very = rack_drag(KnobRange::VeryPrecise, KNOB_PIXELS_PER_RANGE / 4.0);
    assert!((normal - (start + 0.25)).abs() < 1e-4, "{normal}");
    assert!((precise - (start + 0.125)).abs() < 1e-4, "{precise}");
    assert!((very - (start + 0.0625)).abs() < 1e-4, "{very}");
}

#[test]
fn the_default_range_is_the_standard_travel() {
    assert_eq!(KnobRange::default(), KnobRange::Normal);
    assert_eq!(KnobRange::Normal.travel(), 1.0);
    // A view nobody configured drags exactly like one set to Normal.
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    let c = v.control_center(&key("delay"), "mix").unwrap();
    let _ = v.handle(&down(c), &doc, &reg);
    let a = v.handle(&mv(c - Vec2::new(0.0, 30.0)), &doc, &reg);
    assert_eq!(set_values(&a)[0], rack_drag(KnobRange::Normal, 30.0));
}

#[test]
fn the_knob_range_ids_round_trip() {
    for r in KnobRange::ALL {
        assert_eq!(KnobRange::from_id(r.id()), Some(r));
    }
    assert_eq!(KnobRange::from_id("fast"), None);
    assert_eq!(KnobRange::Normal.next().next().next(), KnobRange::Normal);
}

#[test]
fn a_console_knob_drag_follows_the_knob_range_too() {
    let value_after = |range: KnobRange| {
        let (doc, reg) = (
            support::console::console_doc(),
            support::console::console_registry(),
        );
        let mut v = support::console::console_view(&doc, &reg);
        v.set_knob_range(range);
        let p = v.control_center(&key("ch-lead"), "input.gain").unwrap();
        let _ = v.handle(&down(p), &doc, &reg);
        let a = v.handle(
            &mv(p - Vec2::new(0.0, KNOB_PIXELS_PER_RANGE / 4.0)),
            &doc,
            &reg,
        );
        commands(&a)
            .into_iter()
            .find_map(|c| match c {
                Command::SetParam { value, .. } => Some(value),
                _ => None,
            })
            .expect("a SetParam")
    };
    // -24..24 dB from 0: a quarter of the standard travel is +12 dB.
    assert!((value_after(KnobRange::Normal) - 12.0).abs() < 1e-3);
    assert!((value_after(KnobRange::Precise) - 6.0).abs() < 1e-3);
    assert!((value_after(KnobRange::VeryPrecise) - 3.0).abs() < 1e-3);
}

#[test]
fn hovering_a_plain_control_shows_its_value_only_when_the_switch_is_on() {
    let (doc, reg) = (reference_doc(), registry());
    let mut v = view(&doc, &reg);
    let at = v.control_center(&key("delay"), "mix").unwrap();
    let _ = v.handle(&mv(at), &doc, &reg);
    assert_eq!(v.tooltip(), None, "off by default");
    v.set_value_tips(true);
    let tip = v.tooltip().expect("a value tooltip over the control");
    assert!(tip.starts_with("Mix"), "{tip}");
    assert!(!tip.contains("automated"), "{tip}");
    v.set_value_tips(false);
    assert_eq!(v.tooltip(), None);
    // The pointer leaving the control clears it again.
    v.set_value_tips(true);
    let _ = v.handle(&UiEvent::PointerLeave, &doc, &reg);
    assert_eq!(v.tooltip(), None);
}
