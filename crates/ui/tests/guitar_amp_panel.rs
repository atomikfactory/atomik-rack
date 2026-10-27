//! The guitar amp panel: the speakers and the mic follow the cabinet and mic params, the meters
//! (in, out, gate reduction), the setup selector emits one param change, and every control has a
//! tooltip of its own. The generic effect tests (layout, goldens, bypass) cover the amp through
//! `FxKind::EFFECTS`; the goldens are in `fx_panels.rs`.

mod support;

use atomik_device_sdk::DeviceDescriptor;
use atomik_model::Command;
use atomik_ui::fx::{self, FxKind, update_display};
use atomik_ui::{DisplayList, PointerButton, RackView};
use support::fx::*;
use support::*;

fn desc() -> &'static DeviceDescriptor {
    registry().get("atomik.guitar-amp").unwrap().descriptor
}

fn texts_with(params: &[(&str, f32)]) -> Vec<String> {
    let mut doc = fx_doc(FxKind::GuitarAmp);
    for (k, v) in params {
        doc.devices[0].params.insert((*k).to_string(), *v);
    }
    let reg = registry();
    let mut v = rack_of(&doc);
    v.set_device_meter_values(&key("fx"), &raw_meters(FxKind::GuitarAmp));
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    dl.texts().into_iter().map(str::to_string).collect()
}

fn rack_of(doc: &atomik_model::RackDoc) -> RackView {
    let reg = registry();
    let mut v = RackView::new(atomik_ui::Theme::dark());
    let _ = v.handle(
        &atomik_ui::UiEvent::Resize(atomik_ui::kurbo::Size::new(820.0, 900.0)),
        doc,
        &reg,
    );
    v.sync(doc, &reg);
    v
}

#[test]
fn the_kind_is_the_guitar_amp_with_three_meter_values() {
    assert_eq!(FxKind::of("atomik.guitar-amp"), Some(FxKind::GuitarAmp));
    assert_eq!(FxKind::GuitarAmp.type_id(), "atomik.guitar-amp");
    assert_eq!(FxKind::GuitarAmp.meter_count(), 3);
    assert!(FxKind::EFFECTS.contains(&FxKind::GuitarAmp));
    assert!(!FxKind::AudioTrack.type_id().contains("amp"));
}

#[test]
fn the_speakers_follow_the_cabinet() {
    use atomik_devices::guitar_amp::CAB_NAMES;
    let counts: Vec<usize> = (0..CAB_NAMES.len()).map(fx::amp_speaker_count).collect();
    assert_eq!(counts, [1, 2, 4, 0]);
    for n in [1, 2, 4] {
        let (centres, radius) = fx::amp_speakers(n);
        assert_eq!(centres.len(), n);
        for (i, a) in centres.iter().enumerate() {
            // Inside the unit square, and not over another speaker.
            assert!(a.0 - radius >= 0.0 && a.0 + radius <= 1.0, "{n}: {a:?}");
            assert!(a.1 - radius >= 0.0 && a.1 + radius <= 1.0, "{n}: {a:?}");
            for b in &centres[i + 1..] {
                let d = ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
                assert!(d >= 2.0 * radius, "{n}: {a:?} {b:?}");
            }
        }
    }
    assert_eq!(fx::amp_speakers(0).0.len(), 0);
    // The mic goes from the centre of the cone to near its edge, and stays inside.
    assert_eq!(fx::amp_mic_offset(0.0), 0.0);
    assert!((fx::amp_mic_offset(1.0) - 0.9).abs() < 1e-9);
    assert!(fx::amp_mic_offset(0.5) > fx::amp_mic_offset(0.25));
    assert_eq!(fx::amp_mic_offset(f32::NAN), 0.0);
    assert_eq!(fx::amp_mic_offset(7.0), fx::amp_mic_offset(1.0));
}

#[test]
fn the_visual_names_the_cabinet_and_the_setup_and_labels_the_meters() {
    let t = texts_with(&[("cab", 2.0), ("setup", 2.0)]);
    for s in [
        "AMP",
        "4X12 CLOSED",
        "INDUSTRIAL RHYTHM",
        "IN",
        "OUT",
        "GATE",
    ] {
        assert!(t.iter().any(|x| x == s), "{s:?}: {t:?}");
    }
    let t = texts_with(&[("cab", 0.0), ("setup", 4.0)]);
    assert!(t.iter().any(|x| x == "1X12 OPEN") && t.iter().any(|x| x == "SYNTHWAVE"));
    let t = texts_with(&[("cab", 3.0)]);
    assert!(t.iter().any(|x| x == "DIRECT") && t.iter().any(|x| x == "NO CABINET"));
}

#[test]
fn the_painting_changes_with_the_cabinet_and_the_mic() {
    let paint = |params: &[(&str, f32)]| {
        let mut doc = fx_doc(FxKind::GuitarAmp);
        for (k, v) in params {
            doc.devices[0].params.insert((*k).to_string(), *v);
        }
        let mut v = rack_of(&doc);
        let mut dl = DisplayList::new();
        v.paint(&mut dl, &doc, &registry());
        dl.to_json()
    };
    let base = paint(&[("cab", 2.0), ("mic", 0.2)]);
    assert_ne!(base, paint(&[("cab", 1.0), ("mic", 0.2)]), "other speakers");
    assert_ne!(base, paint(&[("cab", 2.0), ("mic", 0.9)]), "the mic moved");
    assert_eq!(base, paint(&[("cab", 2.0), ("mic", 0.2)]), "deterministic");
}

#[test]
fn the_meters_show_the_levels_and_the_gate_reduction() {
    let mut shown = Vec::new();
    assert!(update_display(
        FxKind::GuitarAmp,
        &[0.5, 0.25, 12.0],
        &mut shown
    ));
    assert_eq!(shown.len(), 3);
    assert!(
        (shown[0] + 6.0).abs() < 0.6,
        "input peak in dB: {}",
        shown[0]
    );
    assert!(
        (shown[1] + 12.0).abs() < 0.6,
        "output peak in dB: {}",
        shown[1]
    );
    assert!(
        (shown[2] - 12.0).abs() < 0.11,
        "gate reduction in dB: {}",
        shown[2]
    );
    // The reduction falls away slowly, the way a gain reduction meter does.
    update_display(FxKind::GuitarAmp, &[0.5, 0.25, 0.0], &mut shown);
    assert!(shown[2] > 10.0 && shown[2] < 12.0, "{}", shown[2]);
    // Silence settles at the floor and stops asking for repaints.
    for _ in 0..400 {
        update_display(FxKind::GuitarAmp, &[0.0, 0.0, 0.0], &mut shown);
    }
    assert!(!update_display(
        FxKind::GuitarAmp,
        &[0.0, 0.0, 0.0],
        &mut shown
    ));
    assert_eq!(shown[2], 0.0);
}

#[test]
fn the_setup_selector_steps_through_the_setups_with_one_param_change() {
    let (doc, reg) = (fx_doc(FxKind::GuitarAmp), registry());
    let mut v = rack_of(&doc);
    let p = v.control_center(&key("fx"), "setup").expect("selector");
    // The document holds setup 2: a click goes to 3, a right click to 1.
    let a = v.handle(&down(p), &doc, &reg);
    assert_eq!(
        commands(&a),
        vec![Command::SetParam {
            key: key("fx"),
            param: "setup".into(),
            value: 3.0,
            merge: None
        }]
    );
    let _ = v.handle(&up(p), &doc, &reg);
    let a = v.handle(&down_with(p, PointerButton::Secondary, 1), &doc, &reg);
    assert_eq!(
        commands(&a),
        vec![Command::SetParam {
            key: key("fx"),
            param: "setup".into(),
            value: 1.0,
            merge: None
        }]
    );
}

#[test]
fn every_control_has_a_short_tooltip_of_its_own() {
    let doc = fx_doc(FxKind::GuitarAmp);
    let v = rack_of(&doc);
    let mut seen = std::collections::BTreeSet::new();
    for p in desc().params.iter().filter(|p| p.key != "bypass") {
        let text = fx::amp_help(p.key).unwrap_or_else(|| panic!("{} has no help", p.key));
        assert!(
            (10..=90).contains(&text.len()),
            "{}: a tooltip is one short line ({})",
            p.key,
            text.len()
        );
        assert!(seen.insert(text), "{}: the text is shared", p.key);
        let at = v.control_center(&key("fx"), p.key).expect(p.key);
        let help = v
            .help_at(at)
            .unwrap_or_else(|| panic!("{} shows no tooltip", p.key));
        assert_eq!(help.text, text, "{}", p.key);
    }
    assert_eq!(fx::amp_help("bypass"), None);
    assert_eq!(fx::amp_help("nope"), None);
}
