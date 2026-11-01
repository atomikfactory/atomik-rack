//! The CPU readout on the name plates: shown from the loads the app sets, a container takes the
//! sum of what is inside it, nothing is drawn when it is off, and a figure that does not change
//! whole percent does not repaint. Goldens regenerate with
//! `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test rack_cpu_readout`.

mod support;

use atomik_model::DeviceKey;
use atomik_ui::RackView;
use std::collections::BTreeMap;
use support::starter::*;
use support::*;

fn loads(pairs: &[(&str, f32)]) -> BTreeMap<DeviceKey, f32> {
    pairs.iter().map(|(k, v)| (key(k), *v)).collect()
}

fn unfolded_view() -> (atomik_model::Document, atomik_device_sdk::Registry) {
    let reg = registry();
    let mut d = two_instruments(&reg);
    for k in ["synth", "bass", "channel"] {
        run(
            &mut d,
            &reg,
            [atomik_model::Command::SetFolded {
                key: key(k),
                folded: false,
            }],
        );
    }
    (d, reg)
}

#[test]
fn plates_show_the_loads_and_nothing_when_off() {
    let (d, reg) = unfolded_view();
    let doc = d.doc();
    let mut v: RackView = view(doc, &reg);
    let off = paint_json(&mut v, doc, &reg);
    v.set_cpu_loads(Some(&loads(&[("synth", 12.0), ("channel", 80.0)])));
    assert_eq!(v.cpu_load(&key("synth")), Some(12.0));
    assert_eq!(v.cpu_load(&key("bass")), None, "no reading, no figure");
    let on = paint_json(&mut v, doc, &reg);
    assert_ne!(on, off);
    check_golden("rack_cpu_readout_1x.json", &on);
    v.set_cpu_loads(None);
    assert_eq!(
        paint_json(&mut v, doc, &reg),
        off,
        "off draws exactly as before"
    );
}

#[test]
fn a_folded_strip_shows_its_figure_past_the_plate() {
    let reg = registry();
    let d = two_instruments(&reg);
    let doc = d.doc();
    let mut v = view(doc, &reg);
    let off = paint_json(&mut v, doc, &reg);
    v.set_cpu_loads(Some(&loads(&[("synth", 35.0)])));
    let on = paint_json(&mut v, doc, &reg);
    assert_ne!(on, off);
    check_golden("rack_cpu_readout_folded_1x.json", &on);
}

#[test]
fn the_figure_follows_the_load_and_repaints_only_per_whole_percent() {
    let (d, reg) = unfolded_view();
    let doc = d.doc();
    let mut v = view(doc, &reg);
    v.set_cpu_loads(Some(&loads(&[("synth", 12.0)])));
    let first = paint_json(&mut v, doc, &reg);
    assert!(!v.needs_repaint(), "painted");
    v.set_cpu_loads(Some(&loads(&[("synth", 12.2)])));
    assert!(!v.needs_repaint(), "12.2 shows as 12 too");
    v.set_cpu_loads(Some(&loads(&[("synth", 13.0)])));
    assert!(v.needs_repaint());
    assert_ne!(paint_json(&mut v, doc, &reg), first);
}
