//! The channel strip upgrades in the console, against the real `atomik.channel` descriptor: the
//! PARALLEL section (enable, threshold, ratio and mix; attack, release and makeup are left to the
//! rack's channel panel), the input trim and polarity controls, and the section's place in the
//! strip.

mod support;

use atomik_core::ParamDescriptor;
use atomik_device_sdk::Registry;
use atomik_model::{Command, RackDoc};
use atomik_ui::console_layout::{CtrlKind, Folds, SectionId, is_hidden_param, strip_layout};
use atomik_ui::kurbo::Point;
use atomik_ui::{ConsoleView, UiAction};
use support::console::*;
use support::*;

fn view(doc: &RackDoc, reg: &Registry) -> ConsoleView {
    console_view(doc, reg)
}

fn click(v: &mut ConsoleView, doc: &RackDoc, reg: &Registry, p: Point) -> Vec<UiAction> {
    let mut a = v.handle(&down(p), doc, reg);
    a.extend(v.handle(&up(p), doc, reg));
    a
}

fn param_of(p: &'static ParamDescriptor) -> &'static str {
    p.key
}

#[test]
fn the_parallel_section_follows_the_dynamics_and_holds_four_controls() {
    let desc = &atomik_devices::channel::DESCRIPTOR;
    let l = strip_layout(desc, Folds::default(), 900.0);
    let ids: Vec<SectionId> = l.sections.iter().map(|s| s.id).collect();
    let at = |id| ids.iter().position(|s| *s == id).unwrap();
    assert_eq!(at(SectionId::Parallel), at(SectionId::Dynamics) + 1);
    assert_eq!(at(SectionId::Eq), at(SectionId::Parallel) + 1);
    let par: Vec<(&str, CtrlKind)> = l
        .controls
        .iter()
        .filter(|c| c.section == SectionId::Parallel)
        .map(|c| (c.param, c.kind))
        .collect();
    assert_eq!(
        par,
        [
            ("pcomp.on", CtrlKind::Button),
            ("pcomp.threshold", CtrlKind::Knob),
            ("pcomp.ratio", CtrlKind::Knob),
            ("pcomp.mix", CtrlKind::Knob),
        ]
    );
    assert_eq!(
        l.sections
            .iter()
            .find(|s| s.id == SectionId::Parallel)
            .map(|s| SectionId::title(s.id)),
        Some("PARALLEL")
    );
}

#[test]
fn the_detail_params_are_in_the_rack_only_and_every_other_param_is_placed_once() {
    let desc = &atomik_devices::channel::DESCRIPTOR;
    let l = strip_layout(desc, Folds::default(), 900.0);
    let placed: Vec<&str> = l.controls.iter().map(|c| c.param).collect();
    for rack_only in ["pcomp.attack", "pcomp.release", "pcomp.makeup"] {
        assert!(
            !placed.contains(&rack_only),
            "{rack_only} stays in the rack"
        );
        assert!(
            desc.param_by_key(rack_only).is_some(),
            "{rack_only} is a param of the strip"
        );
    }
    for p in desc.params {
        if is_hidden_param(p.key)
            || p.key.starts_with("pcomp.a")
            || ["pcomp.release", "pcomp.makeup"].contains(&p.key)
        {
            continue;
        }
        assert_eq!(
            placed.iter().filter(|k| **k == param_of(p)).count(),
            1,
            "{} is placed once",
            p.key
        );
    }
    // The input trim and the polarity switch are the first controls of the strip.
    assert_eq!(&placed[..2], ["input.gain", "input.phase"]);
    let kind = |k: &str| l.controls.iter().find(|c| c.param == k).unwrap().kind;
    assert_eq!(kind("input.gain"), CtrlKind::Knob);
    assert_eq!(kind("input.phase"), CtrlKind::Button);
}

#[test]
fn the_parallel_button_and_the_polarity_switch_set_their_params_from_the_console() {
    let (doc, reg) = (console_doc(), console_registry_real_channel());
    let ch = key("ch-bass");
    let mut v = view(&doc, &reg);
    for (param, want) in [("input.phase", 1.0f32), ("pcomp.on", 1.0)] {
        assert!(v.scroll_to_control(&ch, param), "{param} is shown");
        let p = v.control_center(&ch, param).expect(param);
        let sent: Vec<Command> = commands(&click(&mut v, &doc, &reg, p));
        assert_eq!(
            sent,
            [Command::SetParam {
                key: ch.clone(),
                param: param.into(),
                value: want,
                merge: None,
            }]
        );
    }
}

#[test]
fn the_parallel_mix_knob_drags_through_its_range_with_one_merge_id() {
    let (doc, reg) = (console_doc(), console_registry_real_channel());
    let ch = key("ch-bass");
    let mut v = view(&doc, &reg);
    assert!(v.scroll_to_control(&ch, "pcomp.mix"));
    let p = v.control_center(&ch, "pcomp.mix").unwrap();
    let mut actions = v.handle(&down(p), &doc, &reg);
    actions.extend(v.handle(&mv(Point::new(p.x, p.y - 30.0)), &doc, &reg));
    actions.extend(v.handle(&up(Point::new(p.x, p.y - 30.0)), &doc, &reg));
    let sent = commands(&actions);
    assert!(!sent.is_empty());
    let merges: Vec<Option<u64>> = sent
        .iter()
        .filter_map(|c| match c {
            Command::SetParam { param, merge, .. } if param == "pcomp.mix" => Some(*merge),
            _ => None,
        })
        .collect();
    assert!(merges.iter().all(|m| m.is_some() && *m == merges[0]));
    let top = sent.iter().rev().find_map(|c| match c {
        Command::SetParam { value, .. } => Some(*value),
        _ => None,
    });
    assert!(top.unwrap() > 0.5, "dragging up raises the mix");
}
