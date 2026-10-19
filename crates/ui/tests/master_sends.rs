//! The master's send level knobs in the console view: `send{k}.level` of the
//! real `atomik.master` descriptor have a knob each, in their own section of the bus column,
//! and move like any other knob. Display-list golden: `ATOMIK_UPDATE_GOLDEN=1 cargo test -p
//! atomik-ui --test master_sends`.

mod support;

use atomik_model::Command;
use atomik_ui::console_layout::{CtrlKind, MASTER_W, SectionId, master_layout};
use atomik_ui::kurbo::{Rect, Vec2};
use atomik_ui::{ConsoleView, DisplayList, Theme, UiAction};
use support::console::CONSOLE_BOUNDS;
use support::starter::*;
use support::*;

fn master_desc() -> atomik_device_sdk::DeviceDescriptor {
    *registry().get("atomik.master").unwrap().descriptor
}

#[test]
fn every_send_bus_has_a_knob_in_the_sends_section() {
    let desc = master_desc();
    for h in [300.0, 380.0, 600.0, 1000.0] {
        let l = master_layout(&desc, h);
        let mut rects: Vec<(usize, Rect)> = Vec::new();
        for n in 1..=8usize {
            let key = format!("send{n}.level");
            let c = l
                .controls
                .iter()
                .find(|c| c.param == key)
                .unwrap_or_else(|| panic!("{key} at {h}"));
            assert_eq!(c.kind, CtrlKind::Knob);
            assert_eq!(c.section, SectionId::Sends);
            assert_eq!(c.label, format!("S{n}"));
            assert!(c.rect.x0 >= 0.0 && c.rect.x1 <= MASTER_W);
            rects.push((n, c.rect));
        }
        for (i, (_, a)) in rects.iter().enumerate() {
            for (_, b) in &rects[i + 1..] {
                let (w, hgt) = (
                    a.x1.min(b.x1) - a.x0.max(b.x0),
                    a.y1.min(b.y1) - a.y0.max(b.y0),
                );
                assert!(w <= 1e-9 || hgt <= 1e-9, "send knobs overlap at {h}");
            }
        }
        assert!(
            l.controls
                .iter()
                .all(|c| !(c.section == SectionId::More && c.param.starts_with("send"))),
            "no send level is listed under MORE"
        );
        // The section does not overlap the other bus column sections.
        let sends = l
            .sections
            .iter()
            .find(|s| s.id == SectionId::Sends)
            .unwrap();
        for s in l.sections.iter().filter(|s| {
            matches!(s.id, SectionId::Bus | SectionId::More)
                || (s.id == SectionId::Monitor && s.rect.x0 < sends.rect.x1)
        }) {
            let (w, hgt) = (
                sends.rect.x1.min(s.rect.x1) - sends.rect.x0.max(s.rect.x0),
                sends.rect.y1.min(s.rect.y1) - sends.rect.y0.max(s.rect.y0),
            );
            assert!(
                w <= 1e-9 || hgt <= 1e-9,
                "{:?} overlaps the sends at {h}",
                s.id
            );
        }
    }
}

#[test]
fn a_send_knob_moves_the_master_param() {
    let reg = registry();
    let d = two_instruments(&reg);
    let doc = d.doc();
    let mut v = ConsoleView::new(Theme::dark());
    v.set_bounds(CONSOLE_BOUNDS);
    v.sync(doc, &reg);
    let at = v.control_center(&key("master"), "send3.level").unwrap();
    let _ = v.handle(&down(at), doc, &reg);
    let out = v.handle(&mv(at - Vec2::new(0.0, 12.0)), doc, &reg);
    let moved: Vec<_> = out
        .iter()
        .filter_map(|a| match a {
            UiAction::Command(Command::SetParam {
                key, param, value, ..
            }) => Some((key.0.clone(), param.clone(), *value)),
            _ => None,
        })
        .collect();
    assert_eq!(moved.len(), 1, "{out:?}");
    assert_eq!(moved[0].0, "master");
    assert_eq!(moved[0].1, "send3.level");
    assert!(moved[0].2 > 0.0, "dragging up raises the send above 0 dB");
}

#[test]
fn golden_console_master_sends() {
    let reg = registry();
    let d = two_instruments(&reg);
    let doc = d.doc();
    let mut v = ConsoleView::new(Theme::dark());
    v.set_bounds(CONSOLE_BOUNDS);
    v.sync(doc, &reg);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &reg);
    check_golden("console_master_sends_1x.json", &dl.to_json());
}

#[test]
fn the_console_source_is_a_pick_list_of_eighteen_entries() {
    let reg = registry();
    let d = two_instruments(&reg);
    let doc = d.doc();
    let mut v = ConsoleView::new(Theme::dark());
    v.set_bounds(CONSOLE_BOUNDS);
    v.sync(doc, &reg);
    let at = v.control_center(&key("master"), "monitor.source").unwrap();
    let out = v.handle(&down(at), doc, &reg);
    assert!(commands(&out).is_empty(), "opening the list sets nothing");
    assert!(v.menu_row_center(17).is_some(), "eighteen rows");
    assert!(v.menu_row_center(18).is_none());
    let row = v.menu_row_center(11).unwrap();
    let out = v.handle(&down(row), doc, &reg);
    assert_eq!(
        commands(&out),
        [Command::SetParam {
            key: key("master"),
            param: "monitor.source".into(),
            value: 11.0,
            merge: None
        }]
    );
}
