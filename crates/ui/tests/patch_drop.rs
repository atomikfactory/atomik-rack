//! A patch dropped on a device panel: it loads into that device when its type matches (the
//! panel is outlined while the file hovers), and is inserted below the device otherwise (the
//! slot line shows). From the operating system or the file browser's own drag: both arrive as
//! `FileHover` / `FileDrop`.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{RackDoc, RackSlot};
use atomik_ui::kurbo::Point;
use atomik_ui::{DisplayList, PatchOp, RackView, UiAction, UiEvent};
use std::path::PathBuf;
use support::*;

/// A patch file holding only the stereo delay, in a scratch folder removed on drop.
struct DelayPatch {
    dir: PathBuf,
    path: PathBuf,
}

impl DelayPatch {
    fn new(tag: &str) -> Self {
        let mut doc = reference_doc();
        doc.devices.retain(|d| d.key == key("delay"));
        doc.cables.clear();
        let dir =
            std::env::temp_dir().join(format!("atomik-patch-drop-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Echo.atomikpatch");
        std::fs::write(&path, atomik_model::save_patch(&doc)).unwrap();
        Self { dir, path }
    }

    fn paths(&self) -> Vec<PathBuf> {
        vec![self.path.clone()]
    }
}

impl Drop for DelayPatch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn hover(pos: Point, paths: &[PathBuf]) -> UiEvent {
    UiEvent::FileHover {
        pos,
        paths: paths.to_vec(),
    }
}

fn drop_on(pos: Point, paths: &[PathBuf]) -> UiEvent {
    UiEvent::FileDrop {
        pos,
        paths: paths.to_vec(),
    }
}

fn paint_json(v: &mut RackView, doc: &RackDoc, reg: &Registry) -> String {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, reg);
    dl.to_json()
}

/// A point on the body of device `k` (its middle, away from the patch strip and plate).
fn body(v: &RackView, k: &str) -> Point {
    let r = v.device_rect(&key(k)).unwrap();
    v.rack_to_view(Point::new(r.x0 + 300.0, (r.y0 + r.y1) / 2.0))
}

fn setup() -> (RackDoc, Registry, RackView) {
    let (doc, reg) = (reference_doc(), registry());
    let v = plain_view(&doc, &reg);
    (doc, reg, v)
}

#[test]
fn a_patch_dropped_on_the_body_of_a_device_of_its_type_loads_into_it() {
    let (doc, reg, mut v) = setup();
    let patch = DelayPatch::new("body");
    let paths = patch.paths();
    let to = body(&v, "delay");
    // Not on the patch strip: the strip is a small part of the plate.
    let strip = v
        .patch_strip_center(&key("delay"), PatchOp::Browse)
        .unwrap();
    assert_ne!(to, strip);
    let acts = v.handle(&drop_on(to, &paths), &doc, &reg);
    assert_eq!(
        acts,
        vec![UiAction::ApplyLibraryPatch {
            path: patch.path.clone(),
            device: key("delay"),
        }]
    );
}

#[test]
fn a_patch_dropped_on_a_device_of_another_type_is_inserted_below_it() {
    let (doc, reg, mut v) = setup();
    let patch = DelayPatch::new("other");
    let paths = patch.paths();
    // The synth is index 0 of column 0: the new device goes right below it, whichever half of
    // the panel the pointer is in.
    let r = v.device_rect(&key("synth")).unwrap();
    for y in [r.y0 + 20.0, (r.y0 + r.y1) / 2.0, r.y1 - 20.0] {
        let to = v.rack_to_view(Point::new(r.x0 + 300.0, y));
        let acts = v.handle(&drop_on(to, &paths), &doc, &reg);
        assert_eq!(
            acts,
            vec![UiAction::InsertLibrary {
                path: patch.path.clone(),
                container: None,
                slot: RackSlot {
                    column: 0,
                    index: 1
                },
            }],
            "y {y}"
        );
    }
}

#[test]
fn the_matching_panel_is_outlined_and_the_slot_line_shows_otherwise() {
    let (doc, reg, mut v) = setup();
    let patch = DelayPatch::new("hover");
    let paths = patch.paths();
    let plain = paint_json(&mut v, &doc, &reg);
    // Over the delay: the delay's outline, no line.
    let _ = v.handle(&hover(body(&v, "delay"), &paths), &doc, &reg);
    assert_eq!(v.drop_hover(), Some(&key("delay")));
    let outlined = paint_json(&mut v, &doc, &reg);
    assert_ne!(outlined, plain);
    // Over the synth: no outline, the line instead.
    let _ = v.handle(&hover(body(&v, "synth"), &paths), &doc, &reg);
    assert_eq!(v.drop_hover(), None);
    let line = paint_json(&mut v, &doc, &reg);
    assert_ne!(line, plain);
    assert_ne!(line, outlined);
    // Cancel and drop clear both.
    let _ = v.handle(&UiEvent::FileHoverCancel, &doc, &reg);
    assert_eq!(v.drop_hover(), None);
    assert_eq!(paint_json(&mut v, &doc, &reg), plain);
    let _ = v.handle(&hover(body(&v, "delay"), &paths), &doc, &reg);
    let _ = v.handle(&drop_on(body(&v, "delay"), &paths), &doc, &reg);
    assert_eq!(v.drop_hover(), None);
    assert_eq!(paint_json(&mut v, &doc, &reg), plain);
}

#[test]
fn a_patch_that_cannot_be_read_is_inserted_as_before() {
    let (doc, reg, mut v) = setup();
    let paths = [PathBuf::from("/lib/Patches/Gone.atomikpatch")];
    let _ = v.handle(&hover(body(&v, "delay"), &paths), &doc, &reg);
    assert_eq!(v.drop_hover(), None);
    let acts = v.handle(&drop_on(body(&v, "delay"), &paths), &doc, &reg);
    assert!(
        matches!(acts.as_slice(), [UiAction::InsertLibrary { .. }]),
        "{acts:?}"
    );
}
