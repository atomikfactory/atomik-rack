//! The patch strip on the foot of a device's name plate: which devices have it (every factory
//! device but the console devices), where it sits against the other plate widgets, and what a
//! press on each part asks for.

mod support;

use atomik_device_sdk::RACK_UNIT;
use atomik_model::console::is_console_type;
use atomik_model::{Command, Document};
use atomik_ui::kurbo::{Circle, Rect, Shape};
use atomik_ui::paint::{
    bypass_button_rect, fold_button_center, has_patch_strip, patch_strip, plate_rect,
};
use atomik_ui::{PatchOp, RackView, UiAction};
use support::*;

const OPS: [PatchOp; 4] = [
    PatchOp::Previous,
    PatchOp::Browse,
    PatchOp::Next,
    PatchOp::Save,
];

fn one_device(type_id: &str) -> Option<Document> {
    let mut doc = reference_doc();
    doc.devices.clear();
    doc.cables.clear();
    let mut d = Document::new(doc);
    d.apply(
        Command::AddDevice {
            index: 0,
            type_id: type_id.to_string(),
            key: Some(key("d")),
        },
        &registry(),
    )
    .ok()?;
    Some(d)
}

fn view_of(d: &Document) -> RackView {
    plain_view(d.doc(), &registry())
}

fn overlaps(a: &Rect, b: &Rect) -> bool {
    a.x0 < b.x1 && b.x0 < a.x1 && a.y0 < b.y1 && b.y0 < a.y1
}

#[test]
fn every_factory_device_has_the_strip_except_the_console_devices() {
    let reg = registry();
    let mut checked = 0;
    for type_id in reg.type_ids().collect::<Vec<_>>() {
        if type_id == atomik_model::SPLIT_TYPE {
            continue;
        }
        let Some(d) = one_device(type_id) else {
            continue;
        };
        let v = view_of(&d);
        for op in OPS {
            let centre = v.patch_strip_center(&key("d"), op);
            assert_eq!(
                centre.is_some(),
                !is_console_type(type_id),
                "{type_id} {op:?}"
            );
        }
        if !is_console_type(type_id) {
            checked += 1;
        }
    }
    assert!(
        checked >= 20,
        "the registry has the factory devices: {checked}"
    );
}

#[test]
fn the_strip_fits_the_plate_and_leaves_the_other_widgets_alone() {
    // A plate one unit tall is the tightest; taller ones only add room above.
    for units in 1..=4u32 {
        let height = f64::from(RACK_UNIT) * f64::from(units);
        let strip = patch_strip(height);
        let plate = plate_rect(height, false);
        let parts = [strip.prev, strip.name, strip.next, strip.save];
        for (i, a) in parts.iter().enumerate() {
            assert!(
                a.x0 >= plate.x0 && a.x1 <= plate.x1 && a.y0 >= plate.y0 && a.y1 <= plate.y1,
                "part {i} inside the plate at {units} units"
            );
            for b in &parts[i + 1..] {
                assert!(!overlaps(a, b), "the parts do not overlap");
            }
            assert!(
                !overlaps(a, &bypass_button_rect(height, false)),
                "clear of BYPASS"
            );
            let chevron = Circle::new(fold_button_center(height, false), 7.0).bounding_box();
            assert!(!overlaps(a, &chevron), "clear of the fold chevron");
        }
    }
}

#[test]
fn a_folded_device_and_the_interface_have_no_strip() {
    let d = one_device("atomik.stereo-delay").unwrap();
    let mut doc = d.doc().clone();
    doc.devices[0].folded = true;
    let v = plain_view(&doc, &registry());
    assert!(v.patch_strip_center(&key("d"), PatchOp::Browse).is_none());
    assert!(!has_patch_strip(None, false));
}

#[test]
fn a_press_on_each_part_asks_for_its_operation_and_selects_the_device() {
    let d = one_device("atomik.stereo-delay").unwrap();
    let reg = registry();
    let mut v = view_of(&d);
    for op in OPS {
        let at = v.patch_strip_center(&key("d"), op).unwrap();
        let acts = v.handle(&down(at), d.doc(), &reg);
        let _ = v.handle(&up(at), d.doc(), &reg);
        assert!(
            acts.contains(&UiAction::DevicePatch {
                device: key("d"),
                op
            }),
            "{op:?}: {acts:?}"
        );
        if op == PatchOp::Previous {
            assert!(
                acts.iter()
                    .any(|a| matches!(a, UiAction::Select(_) | UiAction::SelectMany { .. })),
                "the first press selects the device"
            );
        }
        assert!(commands(&acts).is_empty(), "a press edits nothing itself");
    }
}

#[test]
fn the_patch_name_shows_on_the_plate_and_changes_what_is_drawn() {
    let d = one_device("atomik.stereo-delay").unwrap();
    let reg = registry();
    let mut named = d.doc().clone();
    named.devices[0].patch_name = "Warm Pad".into();
    let plain = paint_text(&mut view_of(&d), d.doc(), &reg);
    let mut v = plain_view(d.doc(), &reg);
    v.sync(&named, &reg);
    let shown = paint_text(&mut v, &named, &reg);
    assert!(shown.iter().any(|t| t.starts_with("Warm")), "{shown:?}");
    assert!(!plain.iter().any(|t| t.starts_with("Warm")));
}

fn paint_text(
    v: &mut RackView,
    doc: &atomik_model::RackDoc,
    reg: &atomik_device_sdk::Registry,
) -> Vec<String> {
    let mut list = atomik_ui::DisplayList::new();
    v.paint(&mut list, doc, reg);
    list.texts().iter().map(|t| (*t).to_string()).collect()
}

#[test]
fn a_patch_dropped_on_the_strip_is_for_that_device_and_elsewhere_inserts() {
    let d = one_device("atomik.stereo-delay").unwrap();
    let reg = registry();
    let mut v = view_of(&d);
    let path = std::path::PathBuf::from("/lib/Patches/Echo.atomikpatch");
    let paths = std::slice::from_ref(&path);
    let at = v.patch_strip_center(&key("d"), PatchOp::Browse).unwrap();
    let acts = v.handle(
        &atomik_ui::UiEvent::FileDrop {
            pos: at,
            paths: paths.to_vec(),
        },
        d.doc(),
        &reg,
    );
    assert_eq!(
        acts,
        vec![UiAction::ApplyLibraryPatch {
            path: path.clone(),
            device: key("d"),
        }]
    );
    // A container item on the strip, and a patch on the body, are inserted as before.
    let item = std::path::PathBuf::from("/lib/Containers/Box.atomikcontainer");
    let acts = v.handle(
        &atomik_ui::UiEvent::FileDrop {
            pos: at,
            paths: vec![item],
        },
        d.doc(),
        &reg,
    );
    assert!(
        matches!(acts.as_slice(), [UiAction::InsertLibrary { .. }]),
        "{acts:?}"
    );
}

#[test]
fn the_strip_is_not_pressable_on_the_back() {
    let d = one_device("atomik.stereo-delay").unwrap();
    let reg = registry();
    let mut v = view_of(&d);
    let at = v.patch_strip_center(&key("d"), PatchOp::Next).unwrap();
    v.set_flip(atomik_ui::Flip::Back);
    v.sync(d.doc(), &reg);
    let acts = v.handle(&down(at), d.doc(), &reg);
    assert!(
        !acts
            .iter()
            .any(|a| matches!(a, UiAction::DevicePatch { .. })),
        "{acts:?}"
    );
}
