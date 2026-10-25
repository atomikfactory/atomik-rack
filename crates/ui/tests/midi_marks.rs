//! MIDI marks in the rack and the console: the right click on a control that opens its MIDI
//! menu, the dot on a mapped control, the outline while MIDI Learn is on and the warning outline
//! on the armed control. Nothing is painted extra without marks.

mod support;

use atomik_ui::kurbo::Point;
use atomik_ui::{ConsoleView, DisplayList, Geom, MidiMarks, Op, PointerButton, RackView, UiAction};
use support::console::*;
use support::*;

fn right_click_rack(v: &mut RackView, doc: &atomik_model::RackDoc, p: Point) -> Vec<UiAction> {
    v.handle(&down_with(p, PointerButton::Secondary, 1), doc, &registry())
}

fn right_click_console(
    v: &mut ConsoleView,
    doc: &atomik_model::RackDoc,
    reg: &atomik_device_sdk::Registry,
    p: Point,
) -> Vec<UiAction> {
    v.handle(&down_with(p, PointerButton::Secondary, 1), doc, reg)
}

fn paint_rack_view(v: &mut RackView, doc: &atomik_model::RackDoc) -> DisplayList {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl
}

fn paint_console(
    v: &mut ConsoleView,
    doc: &atomik_model::RackDoc,
    reg: &atomik_device_sdk::Registry,
) -> DisplayList {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, reg);
    dl
}

/// The number of circles filled in a display list.
fn circles(dl: &DisplayList) -> usize {
    dl.ops()
        .iter()
        .filter(|op| {
            matches!(
                op,
                Op::Fill {
                    shape: Geom::Circle(_),
                    ..
                }
            )
        })
        .count()
}

fn marks(device: &str, params: &[&str]) -> MidiMarks {
    MidiMarks {
        mapped: params
            .iter()
            .map(|p| (key(device), (*p).to_owned()))
            .collect(),
        ..MidiMarks::default()
    }
}

// ---- The right click ---------------------------------------------------------------------------

#[test]
fn a_right_click_on_a_rack_knob_opens_its_midi_menu() {
    let doc = reference_doc();
    let mut v = plain_view(&doc, &registry());
    let at = v.control_center(&key("synth"), "filter.cutoff").unwrap();
    let actions = right_click_rack(&mut v, &doc, at);
    let menu: Vec<_> = actions
        .iter()
        .filter_map(|a| match a {
            UiAction::ControlMenu { device, param, .. } => Some((device.clone(), param.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(menu, [(key("synth"), "filter.cutoff".to_owned())]);
    assert!(commands(&actions).is_empty(), "nothing changed");
    // A left click does not open it.
    let left = v.handle(&down(at), &doc, &registry());
    assert!(
        !left
            .iter()
            .any(|a| matches!(a, UiAction::ControlMenu { .. }))
    );
}

#[test]
fn a_right_click_on_a_rack_choice_still_steps_it_back() {
    let doc = reference_doc();
    let mut v = plain_view(&doc, &registry());
    let at = v.control_center(&key("synth"), "osc2.wave").unwrap();
    let actions = right_click_rack(&mut v, &doc, at);
    assert!(
        !actions
            .iter()
            .any(|a| matches!(a, UiAction::ControlMenu { .. }))
    );
    assert_eq!(commands(&actions).len(), 1, "the choice stepped back");
}

#[test]
fn a_right_click_on_a_console_fader_pan_and_button_opens_the_menu() {
    let (doc, reg) = (console_doc(), console_registry());
    let mut v = console_view(&doc, &reg);
    for param in ["fader", "pan", "mute"] {
        let at = v.control_center(&key("ch-bass"), param).unwrap();
        let actions = right_click_console(&mut v, &doc, &reg, at);
        assert!(
            actions.iter().any(|a| matches!(a,
                UiAction::ControlMenu { device, param: p, .. }
                    if *device == key("ch-bass") && p == param)),
            "{param}: {actions:?}"
        );
        assert!(commands(&actions).is_empty(), "{param}");
    }
}

// ---- The marks ---------------------------------------------------------------------------------

#[test]
fn a_rack_with_no_marks_paints_exactly_what_it_did() {
    let doc = reference_doc();
    let mut plain = plain_view(&doc, &registry());
    let base = paint_rack_view(&mut plain, &doc);
    let mut set = plain_view(&doc, &registry());
    set.set_midi_marks(MidiMarks::default());
    assert_eq!(paint_rack_view(&mut set, &doc).ops(), base.ops());
}

#[test]
fn a_mapped_rack_control_gets_a_dot_and_learning_outlines_it() {
    let doc = reference_doc();
    let mut v = plain_view(&doc, &registry());
    let base = circles(&paint_rack_view(&mut v, &doc));
    v.set_midi_marks(marks("synth", &["filter.cutoff", "osc2.wave"]));
    let dots = circles(&paint_rack_view(&mut v, &doc));
    assert_eq!(
        dots,
        base + 4,
        "a dot and its backing for each of the two controls"
    );
    // Marking a control that is not on the front adds nothing.
    v.set_midi_marks(marks("synth", &["no.such.param"]));
    assert_eq!(circles(&paint_rack_view(&mut v, &doc)), base);
    // Learning outlines the mapped controls (strokes), the armed one in the warning colour.
    let strokes = |dl: &DisplayList| {
        dl.ops()
            .iter()
            .filter(|o| matches!(o, Op::Stroke { .. }))
            .count()
    };
    v.set_midi_marks(MidiMarks::default());
    let none = strokes(&paint_rack_view(&mut v, &doc));
    let mut learning = marks("synth", &["filter.cutoff"]);
    learning.learning = true;
    v.set_midi_marks(learning.clone());
    assert_eq!(strokes(&paint_rack_view(&mut v, &doc)), none + 1);
    learning.armed = Some((key("synth"), "osc2.wave".to_owned()));
    v.set_midi_marks(learning);
    assert_eq!(strokes(&paint_rack_view(&mut v, &doc)), none + 2);
    // A new set of marks repaints, the same set does not.
    let mut again = marks("synth", &["filter.cutoff"]);
    again.learning = true;
    v.set_midi_marks(again.clone());
    assert!(v.needs_repaint());
    paint_rack_view(&mut v, &doc);
    v.set_midi_marks(again);
    assert!(!v.needs_repaint());
}

#[test]
fn the_console_marks_mapped_strip_controls_in_both_halves_of_a_strip() {
    let (doc, reg) = (console_doc(), console_registry());
    let mut v = console_view(&doc, &reg);
    let base = circles(&paint_console(&mut v, &doc, &reg));
    // `fader` and `mute` are pinned, `eq.lf.gain` scrolls with the upper sections.
    v.set_midi_marks(marks("ch-bass", &["fader", "mute", "eq.lf.gain"]));
    let dots = circles(&paint_console(&mut v, &doc, &reg));
    assert!(dots >= base + 4, "{dots} against {base}");
    let mut off = console_view(&doc, &reg);
    off.set_midi_marks(MidiMarks::default());
    assert_eq!(
        paint_console(&mut off, &doc, &reg).ops(),
        paint_console(&mut console_view(&doc, &reg), &doc, &reg).ops()
    );
    // A control of another strip is not marked on this one.
    let other = marks("ch-lead", &["fader"]);
    let mut v2 = console_view(&doc, &reg);
    v2.set_midi_marks(other);
    let lead = circles(&paint_console(&mut v2, &doc, &reg));
    assert_eq!(lead, base + 2, "one dot with its backing");
}

#[test]
fn a_mapped_vca_strip_is_marked() {
    use atomik_model::VcaGroup;
    let reg = console_registry();
    let mut doc = bus_doc();
    doc.console.vcas = vec![VcaGroup {
        id: 1,
        name: "Drums".into(),
        color: None,
        level_db: 0.0,
        mute: false,
        solo: false,
        members: vec![key("ch-bass")],
    }];
    let mut v = console_view(&doc, &reg);
    v.set_bounds(atomik_ui::kurbo::Rect::new(0.0, 0.0, 1600.0, 760.0));
    v.sync(&doc, &reg);
    let base = circles(&paint_console(&mut v, &doc, &reg));
    v.set_midi_marks(MidiMarks {
        vcas: [1].into(),
        ..MidiMarks::default()
    });
    assert_eq!(circles(&paint_console(&mut v, &doc, &reg)), base + 2);
    v.set_midi_marks(MidiMarks {
        vcas: [2].into(),
        ..MidiMarks::default()
    });
    assert_eq!(circles(&paint_console(&mut v, &doc, &reg)), base);
}
