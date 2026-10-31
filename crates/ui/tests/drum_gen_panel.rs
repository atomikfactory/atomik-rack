//! The drum generator's rack panel: layout, display-list goldens of both faces and of the live
//! cursor, the step cells (a click toggles, a vertical drag sets the velocity), lane mutes, the
//! pattern slots and buttons, GENERATE and RANDOMISE, the page selector and length stepper, the
//! in-place rename, and the pad names of a cabled drum machine. Every emitted command is applied
//! to a real model `Document`.
//! Regenerate with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test drum_gen_panel`.

mod support;

use atomik_device_sdk::DrumRole;
use atomik_model::{
    Cable, Command, Document, DrumDoc, DrumLaneDoc, DrumPatternDoc, DrumStepDoc, RackDoc,
};
use atomik_ui::drum_gen_panel::{DRUM_GEN_PARAMS, DrumButton, drum_gen_layout, lane_label};
use atomik_ui::kurbo::Point;
use atomik_ui::{DisplayList, Flip, Key, Modifiers, RackView, UiAction, UiEvent};
use std::path::PathBuf;
use support::drum::drum_doc_with;
use support::song::press;
use support::*;

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";
const GEN_TYPE: &str = "atomik.drum-generator";

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

fn hit(velocity: u8, prob: u8) -> DrumStepDoc {
    DrumStepDoc { velocity, prob }
}

/// "Groove": 16 x 1/16, a kick on the beats, a snare on the backbeats, a 50 % ghost, a muted
/// hat lane.
fn groove() -> DrumPatternDoc {
    let mut p = DrumPatternDoc::new("Groove");
    for i in (0..16).step_by(4) {
        p.steps[0][i] = hit(110, 100);
    }
    p.steps[2][4] = hit(100, 100);
    p.steps[2][12] = hit(100, 100);
    p.steps[2][7] = hit(45, 50);
    for i in (0..16).step_by(2) {
        p.steps[6][i] = hit(80, 100);
    }
    p.lanes[6].mute = true;
    p
}

/// A 32-step pattern with a hit on step 20.
fn long() -> DrumPatternDoc {
    let mut p = DrumPatternDoc::new("Long");
    p.length = 32;
    p.steps[1][20] = hit(90, 100);
    p
}

fn drums_doc() -> DrumDoc {
    DrumDoc {
        patterns: vec![groove(), long(), DrumPatternDoc::new("Pattern 3")],
    }
}

/// The reference rack plus the drum machine `drums` (pads named kick_808, snare_soft, ...) and
/// the generator `gen` cabled to it through a note filter `filter`.
fn rack_doc(linked: bool) -> RackDoc {
    let mut doc = drum_doc_with(None);
    let mut g = device("gen", GEN_TYPE, "Drum Generator");
    g.drums = Some(drums_doc());
    g.params.insert("density".into(), 0.25);
    g.params.insert("fill.every".into(), 2.0);
    doc.devices.insert(0, g);
    if linked {
        doc.devices
            .insert(1, device("filter", "atomik.note-filter", "Note Filter"));
        doc.cables.extend([
            Cable {
                from: sref("gen", "notes_out"),
                to: sref("filter", "notes_in"),
            },
            Cable {
                from: sref("filter", "notes_out"),
                to: sref("drums", "notes_in"),
            },
        ]);
        doc.cables.sort();
    }
    doc
}

/// Only the generator.
fn only() -> RackDoc {
    let mut doc = rack_doc(false);
    doc.devices.retain(|d| d.key == key("gen"));
    doc.cables.clear();
    doc.song_mut().tracks.clear();
    doc
}

fn setup(doc: RackDoc) -> (Document, RackView) {
    let reg = registry();
    let v = view(&doc, &reg);
    (Document::new(doc), v)
}

/// Applies the commands among `actions` to the document and syncs the view.
fn apply(d: &mut Document, v: &mut RackView, actions: &[UiAction]) {
    let reg = registry();
    for c in commands(actions) {
        d.apply(c, &reg).unwrap();
    }
    v.sync(d.doc(), &reg);
}

fn click(v: &mut RackView, d: &mut Document, p: Point) -> Vec<UiAction> {
    let reg = registry();
    let mut out = v.handle(&down(p), d.doc(), &reg);
    out.extend(v.handle(&up(p), d.doc(), &reg));
    apply(d, v, &out);
    out
}

fn gen_device(d: &Document) -> &atomik_model::RackDevice {
    d.doc().device(&key("gen")).unwrap()
}

fn drums(d: &Document) -> &DrumDoc {
    gen_device(d).drums.as_ref().unwrap()
}

fn texts_of(v: &mut RackView, d: &Document) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, d.doc(), &registry());
    dl.texts().into_iter().map(str::to_string).collect()
}

fn center(v: &RackView, b: DrumButton) -> Point {
    v.drum_button_center(&key("gen"), b).unwrap()
}

fn cell(v: &RackView, lane: usize, step: usize) -> Point {
    v.drum_cell_center(&key("gen"), lane, step).unwrap()
}

// ------------------------------------------------------------------ goldens

#[test]
fn the_front_panel_matches_golden() {
    let (d, mut v) = setup(only());
    let mut dl = DisplayList::new();
    v.paint(&mut dl, d.doc(), &registry());
    check("drum_gen_panel_front_1x.json", &dl.to_json());
}

#[test]
fn the_back_panel_matches_golden() {
    let (d, mut v) = setup(only());
    v.set_flip(Flip::Back);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, d.doc(), &registry());
    check("drum_gen_panel_back_1x.json", &dl.to_json());
}

#[test]
fn the_live_panel_with_pad_names_and_the_cursor_matches_golden() {
    let (d, mut v) = setup(rack_doc(true));
    // Playing pattern 0, step 6, running: the cursor lights the sixth step of every lane.
    v.set_device_meter_values(&key("gen"), &[0.0, 6.0, 1.0]);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, d.doc(), &registry());
    check("drum_gen_panel_live_1x.json", &dl.to_json());
}

// ------------------------------------------------------------------ layout

#[test]
fn the_layout_is_four_units_with_every_param_placed_once() {
    let reg = registry();
    let desc = reg.get(GEN_TYPE).unwrap().descriptor;
    let spec = drum_gen_layout(desc);
    assert_eq!(spec.height_units, 4);
    let mut keys: Vec<&str> = spec
        .front
        .iter()
        .map(|c| desc.params[c.param].key)
        .collect();
    keys.sort_unstable();
    let mut want: Vec<&str> = DRUM_GEN_PARAMS.to_vec();
    want.sort_unstable();
    assert_eq!(keys, want);
    // Every param but the strip's bypass is a knob of the panel.
    assert_eq!(DRUM_GEN_PARAMS.len() + 1, desc.params.len());
    assert!(desc.params.iter().any(|p| p.key == "bypass"));
    let mut sockets: Vec<usize> = spec.back.iter().map(|s| s.socket).collect();
    sockets.sort_unstable();
    assert_eq!(sockets, (0..desc.sockets.len()).collect::<Vec<_>>());
    let panel = spec.drum_gen.expect("drum generator panel");
    assert_eq!(
        (panel.grid.width() as u32, panel.grid.height() as u32),
        (16 * 14, 16 * 13)
    );
    // The extras and the knobs do not overlap, and everything is inside the panel.
    let height = f64::from(spec.height_units) * 76.0;
    let mut rects: Vec<_> = panel.buttons().into_iter().map(|(_, r)| r).collect();
    rects.push(panel.grid);
    rects.push(panel.name);
    rects.extend(spec.front.iter().map(|c| c.rect));
    for (i, a) in rects.iter().enumerate() {
        assert!(a.y1 < height && a.x1 <= 736.0 && a.x0 >= 116.0, "{a:?}");
        for b in &rects[i + 1..] {
            let inter = a.intersect(*b);
            assert!(
                inter.width() <= 0.0 || inter.height() <= 0.0,
                "{a:?} overlaps {b:?}"
            );
        }
    }
    // Cells map back to their lane and column.
    for (lane, col) in [(0, 0), (15, 15), (7, 3)] {
        assert_eq!(
            panel.cell_at(panel.cell_rect(lane, col).center()),
            Some((lane, col))
        );
    }
    assert_eq!(panel.cell_at(Point::new(0.0, 0.0)), None);
}

// ------------------------------------------------------------------ the cells

#[test]
fn clicking_a_cell_toggles_the_step() {
    let (mut d, mut v) = setup(only());
    // Lane 3 (the clap), step 5 is empty: a click puts a hit at the default velocity.
    let p = cell(&v, 3, 5);
    let out = click(&mut v, &mut d, p);
    assert_eq!(
        commands(&out),
        [Command::SetDrumStep {
            device: key("gen"),
            index: 0,
            lane: 3,
            step: 5,
            velocity: 100,
            prob: 100,
            merge: None,
        }]
    );
    assert_eq!(drums(&d).patterns[0].steps[3][5].velocity, 100);
    // A second click removes it.
    let out = click(&mut v, &mut d, p);
    assert_eq!(
        commands(&out),
        [Command::SetDrumStep {
            device: key("gen"),
            index: 0,
            lane: 3,
            step: 5,
            velocity: 0,
            prob: 100,
            merge: None,
        }]
    );
    assert_eq!(drums(&d).patterns[0].steps[3][5].velocity, 0);
    // A click on a hit with a probability keeps the probability.
    let p = cell(&v, 2, 7);
    let out = click(&mut v, &mut d, p);
    let Command::SetDrumStep { velocity, prob, .. } = commands(&out)[0] else {
        panic!("a step command")
    };
    assert_eq!((velocity, prob), (0, 50));
    d.undo(&registry()).unwrap();
    assert_eq!(drums(&d).patterns[0].steps[2][7].velocity, 45);
}

#[test]
fn a_vertical_drag_sets_the_velocity_in_one_undo_step() {
    let (mut d, mut v) = setup(only());
    let reg = registry();
    let before = d.doc().clone();
    // Press the kick on step 4 (velocity 110) and drag up 20 px, then down 30 px.
    let p = cell(&v, 0, 4);
    let mut out = v.handle(&down(p), d.doc(), &reg);
    assert!(
        commands(&out).is_empty(),
        "nothing happens before the release"
    );
    for dy in [-2.0, -20.0, 30.0] {
        out.extend(v.handle(&mv(Point::new(p.x, p.y + dy)), d.doc(), &reg));
    }
    out.extend(v.handle(&up(Point::new(p.x, p.y + 30.0)), d.doc(), &reg));
    let steps: Vec<(u8, Option<u64>)> = commands(&out)
        .iter()
        .map(|c| match c {
            Command::SetDrumStep {
                velocity, merge, ..
            } => (*velocity, *merge),
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(
        steps.len(),
        2,
        "a move inside the threshold does nothing: {steps:?}"
    );
    assert_eq!(steps[0].0, 127, "20 px up from 110 reaches the top");
    assert_eq!(steps[1].0, 72, "30 px down from 110 is 110 - 38");
    assert!(
        steps[0].1.is_some() && steps[0].1 == steps[1].1,
        "one merge group"
    );
    assert!(out.iter().any(|a| matches!(a, UiAction::EndMerge)));
    apply(&mut d, &mut v, &out);
    assert_eq!(drums(&d).patterns[0].steps[0][4].velocity, 72);
    d.undo(&reg).unwrap();
    assert_eq!(d.doc(), &before, "the drag is one undo step");

    // A drag on an empty cell places a hit that starts at the default velocity.
    v.sync(d.doc(), &reg);
    let p = cell(&v, 5, 9);
    let mut out = v.handle(&down(p), d.doc(), &reg);
    out.extend(v.handle(&mv(Point::new(p.x, p.y - 10.0)), d.doc(), &reg));
    out.extend(v.handle(&up(Point::new(p.x, p.y - 10.0)), d.doc(), &reg));
    let Command::SetDrumStep { velocity, .. } = commands(&out)[0] else {
        panic!("a step command")
    };
    assert_eq!(velocity, 113);
}

#[test]
fn escape_during_a_drag_restores_the_velocity() {
    let (mut d, mut v) = setup(only());
    let reg = registry();
    let p = cell(&v, 0, 4);
    let mut out = v.handle(&down(p), d.doc(), &reg);
    out.extend(v.handle(&mv(Point::new(p.x, p.y + 20.0)), d.doc(), &reg));
    out.extend(v.handle(&press(Key::Escape, Modifiers::NONE), d.doc(), &reg));
    apply(&mut d, &mut v, &out);
    assert_eq!(drums(&d).patterns[0].steps[0][4].velocity, 110);
}

#[test]
fn cells_beyond_the_pattern_length_cannot_be_edited() {
    let (mut d, mut v) = setup(only());
    let reg = registry();
    // Pattern 2 has 32 steps: page 1 (17-32) is editable, page 2 (33-48) is dim.
    let p = center(&v, DrumButton::Slot(1));
    click(&mut v, &mut d, p);
    let p = center(&v, DrumButton::Page(1));
    click(&mut v, &mut d, p);
    assert_eq!(v.drum_page(&key("gen")), Some(1));
    let p = cell(&v, 1, 20);
    let out = click(&mut v, &mut d, p);
    assert_eq!(drums(&d).patterns[1].steps[1][20].velocity, 0, "{out:?}");
    // The 17th step is step 16 and the 33rd is beyond the length.
    let p = center(&v, DrumButton::Page(2));
    click(&mut v, &mut d, p);
    assert_eq!(
        v.drum_page(&key("gen")),
        Some(1),
        "page 3 does not exist at 32 steps"
    );
    let _ = reg;
    let p = center(&v, DrumButton::LenInc);
    click(&mut v, &mut d, p);
    assert_eq!(drums(&d).patterns[1].length, 33);
    let p = center(&v, DrumButton::Page(2));
    click(&mut v, &mut d, p);
    assert_eq!(v.drum_page(&key("gen")), Some(2));
    let p = cell(&v, 0, 32);
    click(&mut v, &mut d, p);
    assert_eq!(drums(&d).patterns[1].steps[0][32].velocity, 100);
    let p = cell(&v, 0, 33);
    let out = click(&mut v, &mut d, p);
    assert!(commands(&out).is_empty(), "step 34 is beyond the length");
}

#[test]
fn the_length_stepper_moves_by_one_and_by_four_with_shift() {
    let (mut d, mut v) = setup(only());
    let reg = registry();
    let p = center(&v, DrumButton::LenDec);
    click(&mut v, &mut d, p);
    assert_eq!(drums(&d).patterns[0].length, 15);
    let mut out = v.handle(
        &UiEvent::PointerDown {
            pos: p,
            button: atomik_ui::PointerButton::Primary,
            modifiers: Modifiers {
                shift: true,
                ..Modifiers::NONE
            },
            clicks: 1,
        },
        d.doc(),
        &reg,
    );
    out.extend(v.handle(&up(p), d.doc(), &reg));
    apply(&mut d, &mut v, &out);
    assert_eq!(drums(&d).patterns[0].length, 11);
    let p = center(&v, DrumButton::LenInc);
    click(&mut v, &mut d, p);
    click(&mut v, &mut d, p);
    assert_eq!(drums(&d).patterns[0].length, 13);
}

// ------------------------------------------------------------------ lanes, slots and buttons

#[test]
fn a_lane_square_mutes_and_unmutes_the_lane() {
    let (mut d, mut v) = setup(only());
    let p = center(&v, DrumButton::Mute(2));
    let out = click(&mut v, &mut d, p);
    assert_eq!(
        commands(&out),
        [Command::SetDrumLane {
            device: key("gen"),
            index: 0,
            lane: 2,
            setting: DrumLaneDoc {
                key: 38,
                role: DrumRole::Snare,
                mute: true,
            },
        }]
    );
    assert!(drums(&d).patterns[0].lanes[2].mute);
    // Lane 6 starts muted.
    let p = center(&v, DrumButton::Mute(6));
    click(&mut v, &mut d, p);
    assert!(!drums(&d).patterns[0].lanes[6].mute);
}

#[test]
fn slots_select_the_edited_pattern_and_a_command_click_plays_it() {
    let (mut d, mut v) = setup(only());
    let reg = registry();
    assert_eq!(v.edited_pattern(&key("gen")), Some(0));
    let p = center(&v, DrumButton::Slot(2));
    let out = click(&mut v, &mut d, p);
    assert!(
        out.iter()
            .any(|a| matches!(a, UiAction::SelectPattern { index: 2, .. }))
    );
    assert_eq!(v.edited_pattern(&key("gen")), Some(2));
    assert!(commands(&out).is_empty(), "selecting changes no document");
    // Ctrl/Cmd plays it too.
    let p = center(&v, DrumButton::Slot(1));
    let mut out = v.handle(
        &UiEvent::PointerDown {
            pos: p,
            button: atomik_ui::PointerButton::Primary,
            modifiers: Modifiers {
                meta: true,
                ..Modifiers::NONE
            },
            clicks: 1,
        },
        d.doc(),
        &reg,
    );
    out.extend(v.handle(&up(p), d.doc(), &reg));
    apply(&mut d, &mut v, &out);
    assert_eq!(gen_device(&d).params.get("pattern"), Some(&2.0));
    // A slot that does not exist does nothing.
    let p = center(&v, DrumButton::Slot(5));
    assert!(commands(&click(&mut v, &mut d, p)).is_empty());
    assert_eq!(v.edited_pattern(&key("gen")), Some(1));
}

#[test]
fn the_pattern_buttons_add_duplicate_remove_copy_paste_and_clear() {
    let (mut d, mut v) = setup(only());
    let before = d.doc().clone();
    let p = center(&v, DrumButton::Add);
    click(&mut v, &mut d, p);
    assert_eq!(drums(&d).patterns.len(), 4);
    assert_eq!(
        v.edited_pattern(&key("gen")),
        Some(3),
        "the new pattern is edited"
    );
    let p = center(&v, DrumButton::Slot(0));
    click(&mut v, &mut d, p);
    let p = center(&v, DrumButton::Duplicate);
    click(&mut v, &mut d, p);
    assert_eq!(drums(&d).patterns.len(), 5);
    assert_eq!(drums(&d).patterns[4].name, "Groove copy");
    assert_eq!(v.edited_pattern(&key("gen")), Some(4));
    let p = center(&v, DrumButton::Remove);
    click(&mut v, &mut d, p);
    assert_eq!(drums(&d).patterns.len(), 4);
    assert_eq!(v.edited_pattern(&key("gen")), Some(3));
    d.undo(&registry()).unwrap();
    d.undo(&registry()).unwrap();
    d.undo(&registry()).unwrap();
    v.sync(d.doc(), &registry());
    assert_eq!(d.doc(), &before);

    // Copy pattern 1 ("Groove"), paste it into pattern 2: the steps and lanes, not the name.
    let p = center(&v, DrumButton::Slot(0));
    click(&mut v, &mut d, p);
    let p = center(&v, DrumButton::Copy);
    click(&mut v, &mut d, p);
    assert_eq!(v.drum_clipboard().unwrap().name, "Groove");
    let p = center(&v, DrumButton::Slot(2));
    click(&mut v, &mut d, p);
    let p = center(&v, DrumButton::Paste);
    click(&mut v, &mut d, p);
    assert_eq!(drums(&d).patterns[2].name, "Pattern 3");
    assert_eq!(drums(&d).patterns[2].steps, drums(&d).patterns[0].steps);
    assert_eq!(drums(&d).patterns[2].lanes, drums(&d).patterns[0].lanes);
    // Clear empties the steps and keeps the name and the lanes.
    let p = center(&v, DrumButton::Clear);
    click(&mut v, &mut d, p);
    let c = &drums(&d).patterns[2];
    assert!(c.steps.iter().flatten().all(|s| s.velocity == 0));
    assert_eq!(c.name, "Pattern 3");
    assert!(c.lanes[6].mute);
}

#[test]
fn generate_and_randomise_ask_the_app_to_write_the_edited_pattern() {
    let (mut d, mut v) = setup(only());
    let p = center(&v, DrumButton::Slot(1));
    click(&mut v, &mut d, p);
    let p = center(&v, DrumButton::Generate);
    let out = click(&mut v, &mut d, p);
    assert!(
        out.iter().any(|a| matches!(
            a,
            UiAction::GenerateDrums {
                index: 1,
                randomise: false,
                ..
            }
        )),
        "{out:?}"
    );
    let p = center(&v, DrumButton::Randomise);
    let out = click(&mut v, &mut d, p);
    assert!(out.iter().any(|a| matches!(
        a,
        UiAction::GenerateDrums {
            index: 1,
            randomise: true,
            ..
        }
    )));
    assert!(commands(&out).is_empty(), "the app writes the pattern");
}

#[test]
fn a_double_click_on_a_slot_renames_the_pattern() {
    let (mut d, mut v) = setup(only());
    let reg = registry();
    let p = center(&v, DrumButton::Slot(0));
    let mut out = v.handle(
        &down_with(p, atomik_ui::PointerButton::Primary, 2),
        d.doc(),
        &reg,
    );
    out.extend(v.handle(&up(p), d.doc(), &reg));
    assert_eq!(v.renaming(&key("gen")), Some("Groove"));
    for key_press in [Key::Backspace, Key::Char('!'), Key::Enter] {
        out.extend(v.handle(&press(key_press, Modifiers::NONE), d.doc(), &reg));
    }
    apply(&mut d, &mut v, &out);
    assert_eq!(drums(&d).patterns[0].name, "Groov!");
    assert_eq!(drums(&d).patterns[0].steps, groove().steps);
}

// ------------------------------------------------------------------ labels and the cursor

#[test]
fn lane_labels_are_pad_names_when_cabled_to_a_drum_machine_else_note_names() {
    // Not cabled: note names.
    let (d, v) = setup(rack_doc(false));
    let labels = v.drum_lane_labels(&key("gen")).unwrap();
    assert_eq!(labels[0], "C2");
    assert_eq!(labels[15], "D#3");
    drop(d);
    // Cabled through a player: the pad names of the kit (pad 1 kick_808, pad 2 snare_soft,
    // the others have no zones and keep their note names).
    let (_, v) = setup(rack_doc(true));
    let labels = v.drum_lane_labels(&key("gen")).unwrap();
    assert_eq!(labels[0], "kick_808");
    assert_eq!(labels[1], "snare_soft");
    assert_eq!(labels[2], "D2");
    // The label helper: only keys 36..=51 have pads.
    let pads = vec!["kick".to_string(), String::new()];
    assert_eq!(lane_label(36, &pads), "kick");
    assert_eq!(lane_label(37, &pads), "C#2");
    assert_eq!(lane_label(60, &pads), "C4");
    assert_eq!(lane_label(10, &pads), "A#-1");
    // The panel draws them.
    let (d, mut v) = setup(rack_doc(true));
    let texts = texts_of(&mut v, &d);
    assert!(texts.iter().any(|t| t == "kick_808"), "{texts:?}");
    assert!(texts.iter().any(|t| t == "snare_soft"));
}

#[test]
fn the_cursor_follows_the_published_step_while_the_edited_pattern_plays() {
    let (d, mut v) = setup(only());
    assert_eq!(v.step_cursor(&key("gen")), None, "no feed yet");
    v.set_device_meter_values(&key("gen"), &[0.0, 6.0, 1.0]);
    assert_eq!(v.step_cursor(&key("gen")), Some(6));
    assert_eq!(v.playing_pattern(&key("gen")), Some(0));
    // A step beyond the length of the playing pattern is no cursor; stopped means none.
    v.set_device_meter_values(&key("gen"), &[0.0, 20.0, 1.0]);
    assert_eq!(v.step_cursor(&key("gen")), None);
    v.set_device_meter_values(&key("gen"), &[1.0, 20.0, 1.0]);
    assert_eq!(v.step_cursor(&key("gen")), Some(20));
    assert_eq!(v.playing_pattern(&key("gen")), Some(1));
    v.set_device_meter_values(&key("gen"), &[1.0, 20.0, 0.0]);
    assert_eq!(v.step_cursor(&key("gen")), None);
    let _ = d;
}

#[test]
fn the_strip_bypass_and_the_panel_agree_on_the_generator() {
    // The generator is a player: the bypass switch is in the strip, not a knob of the panel.
    let reg = registry();
    let desc = reg.get(GEN_TYPE).unwrap().descriptor;
    assert!(atomik_ui::autolayout::has_strip_bypass(desc));
    let spec = drum_gen_layout(desc);
    assert!(
        spec.front
            .iter()
            .all(|c| desc.params[c.param].key != "bypass")
    );
}

// ------------------------------------------------------------------ previews

/// Renders PNG previews of the panel (front, live with pad names, back) with vello_cpu. Run:
/// `cargo test -p atomik-ui --test drum_gen_panel -- --ignored`. Writes
/// `target/drum-gen-preview/*.png` (not committed).
#[test]
#[ignore = "writes preview images; run explicitly"]
fn preview_pngs() {
    use atomik_ui::{TextEngine, Theme};
    use support::cpu::CpuCanvas;
    const SCALE: f64 = 1.5;
    let dir =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/drum-gen-preview");
    std::fs::create_dir_all(&dir).unwrap();
    let reg = registry();
    let mut text = TextEngine::new();
    for (name, doc, flip, live) in [
        ("front", only(), Flip::Front, false),
        ("live", rack_doc(true), Flip::Front, true),
        ("back", only(), Flip::Back, false),
    ] {
        let mut v = RackView::new(Theme::dark());
        let _ = v.handle(
            &UiEvent::Resize(atomik_ui::kurbo::Size::new(820.0, 420.0)),
            &doc,
            &reg,
        );
        let _ = v.handle(&UiEvent::ScaleChanged(SCALE), &doc, &reg);
        if live {
            v.set_device_meter_values(&key("gen"), &[0.0, 6.0, 1.0]);
        }
        v.set_flip(flip);
        let mut canvas = CpuCanvas::new((820.0 * SCALE) as u16, (420.0 * SCALE) as u16, &mut text);
        v.paint(&mut canvas, &doc, &reg);
        std::fs::write(
            dir.join(format!("{name}.png")),
            canvas.finish().into_png().expect("png"),
        )
        .unwrap();
    }
}
