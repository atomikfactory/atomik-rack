//! Display-list goldens of the step editor: the empty state, a 16-step beat with the
//! three velocity levels, a 32 x 1/32 grid with swing, and a clip with off-grid notes.
//! Regenerate with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test step_snapshot`.

mod support;

use atomik_model::{ClipContent, Note, RackDoc, StepResolution, pad_key};
use atomik_ui::{DisplayList, StepEditorView};
use std::path::PathBuf;
use support::drum::*;
use support::song::notes_clip;
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

fn paint(v: &mut StepEditorView, doc: &RackDoc) -> DisplayList {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl
}

fn record(doc: RackDoc, clip: Option<atomik_model::ClipId>) -> DisplayList {
    let reg = registry();
    let mut v = editor(&doc, &reg, clip);
    paint(&mut v, &doc)
}

fn long_swung_doc() -> RackDoc {
    let mut notes = Vec::new();
    for k in (0..32).step_by(4) {
        notes.push(Note::new(k * S32, S32, pad_key(1), 120.0 / 127.0));
    }
    for k in (2..32).step_by(4) {
        notes.push(Note::new(k * S32, S32, pad_key(6), 0.35));
    }
    for k in 0..32 {
        if k % 2 == 1 {
            notes.push(Note::new(
                k * S32,
                S32,
                pad_key(7),
                0.1 + 0.9 * (k as f32 / 31.0),
            ));
        }
    }
    notes.sort_by_key(|n| (n.tick, n.key));
    let mut clip = notes_clip(7, 0, 32 * S32, notes);
    clip.grid = Some(grid(StepResolution::ThirtySecond, 32, 40));
    drum_doc_with(Some(clip))
}

fn off_grid_doc() -> RackDoc {
    let mut clip = beat_clip();
    if let ClipContent::Notes(n) = &mut clip.content {
        n.push(Note::new(0, 3840, 60, 0.6));
        n.push(Note::new(20 * S16, S16, pad_key(1), 0.6));
        n.push(Note::new(5 * S16 + 900, 400, pad_key(3), 0.7));
        n.sort_by_key(|n| (n.tick, n.key));
    }
    drum_doc_with(Some(clip))
}

#[test]
fn empty_state_matches_golden() {
    check(
        "step_editor_empty_1x.json",
        &record(drum_doc_with(None), None).to_json(),
    );
}

#[test]
fn a_beat_with_three_velocity_levels_matches_golden() {
    check(
        "step_editor_beat_1x.json",
        &record(drum_doc(), Some(CLIP)).to_json(),
    );
}

#[test]
fn a_32_step_1_32_grid_with_swing_matches_golden() {
    check(
        "step_editor_swing_1x.json",
        &record(long_swung_doc(), Some(CLIP)).to_json(),
    );
}

#[test]
fn off_grid_notes_match_golden() {
    check(
        "step_editor_offgrid_1x.json",
        &record(off_grid_doc(), Some(CLIP)).to_json(),
    );
}

#[test]
fn recording_is_deterministic() {
    let a = record(long_swung_doc(), Some(CLIP)).to_json();
    let b = record(long_swung_doc(), Some(CLIP)).to_json();
    assert_eq!(a, b);
}
