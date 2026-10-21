//! Mute in the editors: the step editor's note mute (cell under the pointer), goldens of muted
//! clips and notes, and a PNG preview (`cargo test -p atomik-ui --test mute_ui -- --ignored`
//! writes `target/preview/mute_*.png`). Regenerate goldens with `ATOMIK_UPDATE_GOLDEN=1`.

mod support;

use atomik_model::{ClipContent, ClipId, Note, RackDoc, SongCommand};
use atomik_ui::keymap::CommandOutcome;
use atomik_ui::kurbo::Rect;
use atomik_ui::{
    ArrangementView, DisplayList, Modifiers, PianoRollView, StepEditorView, TextEngine, UiAction,
    UiEvent,
};
use std::path::PathBuf;
use support::audio::*;
use support::cpu::CpuCanvas;
use support::drum::{CLIP, EDITOR_BOUNDS, drum_doc, editor};
use support::song::*;
use support::*;

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";

fn check(name: &str, actual: &str) {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os(UPDATE_ENV).is_some_and(|v| v == "1") {
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing golden {}; run with {UPDATE_ENV}=1", path.display()))
        .replace("\r\n", "\n");
    assert!(
        expected == actual,
        "{name} differs from golden (regenerate with {UPDATE_ENV}=1)"
    );
}

fn mute_notes(doc: &mut RackDoc, clip: u64, which: &[usize]) {
    for t in &mut doc.song.tracks {
        for l in &mut t.lanes {
            for c in &mut l.clips {
                if let ClipContent::Notes(ns) = &mut c.content
                    && c.id == ClipId(clip)
                {
                    for i in which {
                        ns[*i].muted = true;
                    }
                }
            }
        }
    }
}

fn mute_clips(doc: &mut RackDoc, ids: &[u64]) {
    for t in &mut doc.song.tracks {
        for l in &mut t.lanes {
            for c in &mut l.clips {
                if ids.contains(&c.id.0) {
                    c.muted = true;
                }
            }
        }
    }
}

fn muted_arrangement() -> (ArrangementView, RackDoc) {
    let mut doc = audio_doc();
    mute_clips(&mut doc, &[1, 10]);
    let reg = registry();
    (audio_arrangement(&doc, &reg), doc)
}

fn muted_roll() -> (PianoRollView, RackDoc) {
    let mut doc = song_doc();
    mute_notes(&mut doc, 1, &[1, 3]);
    let reg = registry();
    let mut v = piano_roll(&doc, &reg);
    v.set_selected_notes(&[1]);
    (v, doc)
}

fn muted_step() -> (StepEditorView, RackDoc) {
    let mut doc = drum_doc();
    for t in &mut doc.song.tracks {
        for l in &mut t.lanes {
            for c in &mut l.clips {
                if let ClipContent::Notes(ns) = &mut c.content {
                    for n in ns.iter_mut().filter(|n| n.key == atomik_model::pad_key(1)) {
                        n.muted = n.tick > 0;
                    }
                }
            }
        }
    }
    let v = editor(&doc, &registry(), Some(CLIP));
    (v, doc)
}

#[test]
fn muted_clips_match_golden() {
    let (mut v, doc) = muted_arrangement();
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &registry());
    check("mute_arrangement_1x.json", &dl.to_json());
}

#[test]
fn muted_notes_in_the_piano_roll_match_golden() {
    let (mut v, doc) = muted_roll();
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &registry());
    check("mute_piano_roll_1x.json", &dl.to_json());
}

#[test]
fn muted_cells_in_the_step_editor_match_golden() {
    let (mut v, doc) = muted_step();
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &registry());
    check("mute_step_editor_1x.json", &dl.to_json());
}

fn hover(v: &mut StepEditorView, doc: &RackDoc, pad: u8, step: u16) {
    let p = v.cell_center(pad, step);
    let _ = v.handle(
        &UiEvent::PointerMove {
            pos: p,
            modifiers: Modifiers::NONE,
        },
        doc,
        &registry(),
    );
}

#[test]
fn step_editor_m_toggles_the_notes_of_the_hovered_cell_in_one_command() {
    let doc = drum_doc();
    let reg = registry();
    let mut v = editor(&doc, &reg, Some(CLIP));
    hover(&mut v, &doc, 1, 4); // an on cell (kick on step 4)
    assert!(!v.cell_muted(1, 4));
    let out = v.command("editor.toggle_note_mute", None, &doc, &reg);
    let CommandOutcome::Handled(actions) = out else {
        panic!("handled")
    };
    let sets: Vec<Vec<Note>> = song_commands(&actions)
        .into_iter()
        .filter_map(|c| match c {
            SongCommand::SetClipContent {
                content: ClipContent::Notes(n),
                ..
            } => Some(n),
            _ => None,
        })
        .collect();
    assert_eq!(sets.len(), 1);
    let before = v.notes();
    for (a, b) in sets[0].iter().zip(before) {
        let in_cell = a.key == b.key && a.key == atomik_model::pad_key(1) && a.tick == 4 * 3840;
        assert_eq!(a.muted, in_cell, "only the hovered cell's note flips");
    }
}

#[test]
fn step_editor_m_is_quiet_off_a_cell_and_declines_other_commands() {
    let doc = drum_doc();
    let reg = registry();
    let mut v = editor(&doc, &reg, Some(CLIP));
    hover(&mut v, &doc, 1, 1); // an empty cell
    assert_eq!(
        v.command("editor.toggle_note_mute", None, &doc, &reg),
        CommandOutcome::Handled(vec![])
    );
    assert_eq!(
        v.command("editor.quantize", None, &doc, &reg),
        CommandOutcome::Declined
    );
}

#[test]
fn a_muted_cell_reads_as_muted_and_a_fully_muted_cell_unmutes() {
    let (mut v, doc) = muted_step();
    let reg = registry();
    assert!(v.cell_muted(1, 4) && !v.cell_muted(1, 0));
    hover(&mut v, &doc, 1, 4);
    let CommandOutcome::Handled(actions) = v.command("editor.toggle_note_mute", None, &doc, &reg)
    else {
        panic!()
    };
    assert!(matches!(actions.as_slice(), [UiAction::Command(_)]));
}

fn png(name: &str, f: impl FnOnce(&mut dyn atomik_ui::Canvas), b: Rect) {
    const SCALE: f64 = 1.5;
    let mut text = TextEngine::new();
    let mut canvas = CpuCanvas::new(
        (b.width() * SCALE) as u16,
        (b.height() * SCALE) as u16,
        &mut text,
    );
    f(&mut canvas);
    let png = canvas.finish().into_png().expect("png");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/preview")
        .join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, png).unwrap();
    println!("wrote {}", path.display());
}

#[test]
#[ignore = "writes preview images; run explicitly"]
fn preview_mute_png() {
    let reg = registry();
    let (mut a, doc) = muted_arrangement();
    a.set_scale(1.5);
    png(
        "mute_arrangement.png",
        |c| a.paint(c, &doc, &reg),
        ARR_BOUNDS,
    );
    let (mut r, doc) = muted_roll();
    r.set_scale(1.5);
    png(
        "mute_piano_roll.png",
        |c| r.paint(c, &doc, &reg),
        ROLL_BOUNDS,
    );
    let (mut s, doc) = muted_step();
    s.set_scale(1.5);
    png(
        "mute_step_editor.png",
        |c| s.paint(c, &doc, &reg),
        EDITOR_BOUNDS,
    );
}
