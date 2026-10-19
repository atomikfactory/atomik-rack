//! Display-list goldens of the slice editor: an empty slicer, a sliced loop with a selected
//! reversed slice, and the same with files hovering over it. Regenerate with
//! `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test slice_snapshot`.

mod support;

use atomik_model::RackDoc;
use atomik_ui::kurbo::Point;
use atomik_ui::{DisplayList, SliceEditorView, UiEvent};
use std::path::PathBuf;
use support::slicer::*;
use support::*;

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";

fn check(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os(UPDATE_ENV).is_some_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing golden {}; run with {UPDATE_ENV}=1", path.display()))
        .replace("\r\n", "\n");
    if expected != actual {
        let line = expected
            .lines()
            .zip(actual.lines())
            .position(|(a, b)| a != b)
            .unwrap_or(expected.lines().count().min(actual.lines().count()));
        panic!(
            "{name} differs from golden at line {}:\n  golden: {:?}\n  actual: {:?}\n(regenerate with {UPDATE_ENV}=1)",
            line + 1,
            expected.lines().nth(line),
            actual.lines().nth(line)
        );
    }
}

fn paint(v: &mut SliceEditorView, doc: &RackDoc) -> DisplayList {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl
}

#[test]
fn empty_slicer_matches_golden() {
    let (doc, reg) = (slicer_doc(Vec::new()), registry());
    let mut v = editor(&doc, &reg);
    check("slice_editor_empty_1x.json", &paint(&mut v, &doc).to_json());
}

#[test]
fn sliced_loop_with_a_selected_reversed_slice_matches_golden() {
    let (doc, reg) = (slicer_doc(sliced_zones(&MARKERS)), registry());
    let mut v = editor(&doc, &reg);
    v.set_selected_slice(Some(1));
    check(
        "slice_editor_sliced_1x.json",
        &paint(&mut v, &doc).to_json(),
    );
}

#[test]
fn sliced_loop_with_a_drop_hover_matches_golden() {
    let (doc, reg) = (slicer_doc(sliced_zones(&MARKERS)), registry());
    let mut v = editor(&doc, &reg);
    let hover = UiEvent::FileHover {
        pos: Point::new(500.0, 300.0),
        paths: vec!["a.wav".into()],
    };
    let _ = v.handle(&hover, &doc, &reg);
    check("slice_editor_drop_1x.json", &paint(&mut v, &doc).to_json());
}
