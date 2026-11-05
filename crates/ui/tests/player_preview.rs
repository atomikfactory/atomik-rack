//! The play (preview) button on the name strip of every note player: it is in the same place on
//! all of them, clear of the bypass switch, the fold chevron, the patch strip, the status LED and
//! the name; a click selects the player and asks the app for the preview (one `PreviewPlayer`);
//! the lamp shows what the app answered; no other device has it.
//! Regenerate the goldens with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test
//! player_preview`.
#![allow(clippy::float_cmp)] // Justification: the strip rects are exact sums of constants.

mod support;

use atomik_device_sdk::RACK_UNIT;
use atomik_model::{Command, Document};
use atomik_ui::autolayout::BYPASS_KEY;
use atomik_ui::kurbo::{Circle, Rect, Shape};
use atomik_ui::paint::{
    bypass_button_rect, fold_button_center, patch_strip, plate_rect, player_bypass_rect,
    preview_button_rect,
};
use atomik_ui::{RackView, Theme, UiAction};
use std::path::PathBuf;
use support::container::{apply, paint_rack};
use support::*;

const PLAYERS: [&str; 8] = [
    "atomik.arpeggiator",
    "atomik.cv-sequencer",
    "atomik.drum-generator",
    "atomik.step-sequencer",
    "atomik.note-filter",
    "atomik.key-chord",
    "atomik.note-repeater",
    "atomik.note-delay",
];

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";

fn one_device(type_id: &str) -> Document {
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
    .unwrap();
    d
}

fn view_of(d: &Document) -> RackView {
    plain_view(d.doc(), &registry())
}

fn overlaps(a: &Rect, b: &Rect) -> bool {
    a.x0 < b.x1 && b.x0 < a.x1 && a.y0 < b.y1 && b.y0 < a.y1
}

fn click(v: &mut RackView, d: &Document, at: atomik_ui::kurbo::Point) -> Vec<UiAction> {
    let acts = v.handle(&down(at), d.doc(), &registry());
    let _ = v.handle(&up(at), d.doc(), &registry());
    acts
}

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
    assert!(
        expected == actual,
        "{name} differs (regenerate with {UPDATE_ENV}=1)"
    );
}

#[test]
fn every_player_has_the_button_in_the_same_place_and_nothing_else_does() {
    let reg = registry();
    let mut places = Vec::new();
    for t in PLAYERS {
        let d = one_device(t);
        let v = view_of(&d);
        let r = v.preview_button_rect(&key("d")).expect(t);
        let origin = v.device_rect(&key("d")).unwrap();
        places.push((r.x0 - origin.x0, r.width(), r.height()));
        // Down to the bypass switch beside it, which still works as one.
        let b = v.bypass_button_rect(&key("d")).unwrap();
        assert!(!overlaps(&r, &b), "{t}: clear of the bypass switch");
        assert!(r.y0 == b.y0 && r.y1 == b.y1, "{t}: one row");
        assert!(b.width() >= 44.0, "{t}: the switch keeps its label room");
        let fold = v.fold_button_center(&key("d")).unwrap();
        assert!(!overlaps(&r, &Circle::new(fold, 8.0).bounding_box()), "{t}");
    }
    assert!(places.windows(2).all(|w| w[0] == w[1]), "{places:?}");
    // Effects, instruments and the container do not have it.
    for t in ["atomik.reverb", "atomik.poly-synth", "atomik.container"] {
        let d = one_device(t);
        assert!(view_of(&d).preview_button_rect(&key("d")).is_none(), "{t}");
    }
    let _ = reg;
}

#[test]
fn the_button_fits_the_strip_at_every_player_height_folded_or_not() {
    let theme = Theme::dark();
    for u in [1.0, 2.0, 3.0, 4.0, 5.0, 6.0] {
        let h = u * f64::from(RACK_UNIT);
        let plate = plate_rect(h, false);
        let p = preview_button_rect(h, false);
        let b = player_bypass_rect(h, false);
        assert!(
            p.x0 >= plate.x0 && b.x1 <= plate.x1 && p.y0 >= plate.y0 && p.y1 <= plate.y1,
            "{u}U inside the strip"
        );
        assert!(!overlaps(&p, &b), "{u}U: beside the switch");
        // The two together take the room the switch of an effect takes.
        assert_eq!(
            (p.x0, p.y0, p.y1),
            (bypass_button_rect(h, false).x0, b.y0, b.y1)
        );
        assert_eq!(b.x1, bypass_button_rect(h, false).x1);
        // Under the name and the type label, above the chevron, the patch strip and the LED.
        let lines = if u < 2.0 { 1.0 } else { 2.0 };
        let text_bottom = plate.y0 + 19.0 + (f64::from(theme.name_size) + 1.0) * lines - 3.0 + 2.0;
        assert!(p.y0 >= text_bottom, "{u}U: under the labels");
        let fold = Circle::new(fold_button_center(h, false), 8.0).bounding_box();
        assert!(!overlaps(&p, &fold), "{u}U: clear of the chevron");
        let strip = patch_strip(h);
        for part in [strip.prev, strip.name, strip.next, strip.save] {
            assert!(!overlaps(&p, &part), "{u}U: clear of the patch strip");
        }
        let led = Circle::new((plate.x1 - 9.0, plate.y1 - 9.0 + 2.0), 6.0).bounding_box();
        assert!(!overlaps(&p, &led), "{u}U: clear of the LED");
        // Folded: just before the switch, inside the plate, clear of the chevron.
        let fh = h / 2.0;
        let fplate = plate_rect(fh, true);
        let fp = preview_button_rect(fh, true);
        let fb = player_bypass_rect(fh, true);
        assert!(
            fp.x0 >= fplate.x0 && fp.x1 <= fb.x0 && fb.x1 <= fplate.x1,
            "{u}U folded"
        );
        assert!(fp.y0 >= fplate.y0 && fp.y1 <= fplate.y1, "{u}U folded");
        let ffold = Circle::new(fold_button_center(fh, true), 8.0).bounding_box();
        assert!(!overlaps(&fp, &ffold), "{u}U folded: clear of the chevron");
    }
}

#[test]
fn pressing_the_button_selects_the_player_and_asks_for_the_preview_once() {
    for t in PLAYERS {
        let d = one_device(t);
        let mut v = view_of(&d);
        let at = v.preview_button_rect(&key("d")).unwrap().center();
        let acts = click(&mut v, &d, at);
        let previews: Vec<_> = acts
            .iter()
            .filter(|a| matches!(a, UiAction::PreviewPlayer(k) if *k == key("d")))
            .collect();
        assert_eq!(previews.len(), 1, "{t}: {acts:?}");
        assert!(
            acts.iter().all(|a| !matches!(a, UiAction::Command(_))),
            "{t}: the preview is not an edit"
        );
        assert_eq!(v.selected(), Some(&key("d")), "{t}: the player is selected");
        // The same press is a toggle for the app: it asks again on the next click.
        let again = click(&mut v, &d, at);
        assert!(
            again
                .iter()
                .any(|a| matches!(a, UiAction::PreviewPlayer(k) if *k == key("d"))),
            "{t}"
        );
    }
}

#[test]
fn the_bypass_switch_beside_the_button_still_toggles_the_param() {
    for t in PLAYERS {
        let d = one_device(t);
        let mut v = view_of(&d);
        let at = v.bypass_button_rect(&key("d")).unwrap().center();
        let acts = click(&mut v, &d, at);
        assert!(
            acts.iter().any(|a| matches!(
                a,
                UiAction::Command(Command::SetParam { param, value, .. })
                    if param == BYPASS_KEY && *value == 1.0
            )),
            "{t}: {acts:?}"
        );
        assert!(
            !acts.iter().any(|a| matches!(a, UiAction::PreviewPlayer(_))),
            "{t}"
        );
    }
}

#[test]
fn the_lamp_follows_the_app_and_the_picture_changes_only_for_the_previewing_player() {
    let mut d = one_device("atomik.arpeggiator");
    let mut v = view_of(&d);
    let off = paint_rack(&mut v, d.doc());
    v.set_previewing(Some(key("d")));
    assert_eq!(v.previewing(), Some(&key("d")));
    let on = paint_rack(&mut v, d.doc());
    assert_ne!(off.to_json(), on.to_json(), "the button reads as pressed");
    assert!(on.texts().contains(&"BYPASS"));
    // Another device being named changes nothing for this one.
    v.set_previewing(Some(key("other")));
    assert_eq!(paint_rack(&mut v, d.doc()).to_json(), off.to_json());
    v.set_previewing(None);
    assert_eq!(paint_rack(&mut v, d.doc()).to_json(), off.to_json());
    // Folded, the button is drawn too.
    apply(
        &mut d,
        Command::SetFolded {
            key: key("d"),
            folded: true,
        },
    );
    v.sync(d.doc(), &registry());
    let folded_off = paint_rack(&mut v, d.doc());
    v.set_previewing(Some(key("d")));
    assert_ne!(folded_off.to_json(), paint_rack(&mut v, d.doc()).to_json());
}

#[test]
fn the_button_has_a_tooltip() {
    let d = one_device("atomik.step-sequencer");
    let mut v = view_of(&d);
    let at = v.preview_button_rect(&key("d")).unwrap().center();
    assert!(v.help_at(at).is_some(), "the help line explains the button");
    let _ = v.handle(&mv(at), d.doc(), &registry());
    let tip = v.tooltip().expect("a tooltip");
    assert!(tip.contains("Preview"), "{tip}");
    v.set_previewing(Some(key("d")));
    let _ = v.handle(
        &mv(at + atomik_ui::kurbo::Vec2::new(1.0, 0.0)),
        d.doc(),
        &registry(),
    );
    assert!(v.tooltip().is_some_and(|t| t.contains("stop")));
}

#[test]
fn goldens_of_the_button_idle_lit_and_folded() {
    let golden = |t: &str, previewing: bool, folded: bool| {
        let mut d = one_device(t);
        if folded {
            apply(
                &mut d,
                Command::SetFolded {
                    key: key("d"),
                    folded: true,
                },
            );
        }
        let mut v = view_of(&d);
        if previewing {
            v.set_previewing(Some(key("d")));
        }
        paint_rack(&mut v, d.doc()).to_json()
    };
    check(
        "player_preview_note_delay_idle_1x.json",
        &golden("atomik.note-delay", false, false),
    );
    check(
        "player_preview_note_delay_on_1x.json",
        &golden("atomik.note-delay", true, false),
    );
    check(
        "player_preview_arpeggiator_on_1x.json",
        &golden("atomik.arpeggiator", true, false),
    );
    check(
        "player_preview_step_sequencer_folded_on_1x.json",
        &golden("atomik.step-sequencer", true, true),
    );
    check(
        "player_preview_step_sequencer_folded_idle_1x.json",
        &golden("atomik.step-sequencer", false, true),
    );
}

/// Writes `target/preview/player_preview_*.png` (not committed):
/// `cargo test -p atomik-ui --test player_preview -- --ignored`.
#[test]
#[ignore = "writes preview images; run explicitly"]
fn preview_pngs() {
    use atomik_ui::kurbo::Size;
    use atomik_ui::{TextEngine, UiEvent};
    use support::cpu::CpuCanvas;
    let reg = registry();
    let mut text = TextEngine::new();
    let scale = 2.0;
    for (name, type_id, on, folded) in [
        ("delay_idle", "atomik.note-delay", false, false),
        ("delay_on", "atomik.note-delay", true, false),
        ("arp_on", "atomik.arpeggiator", true, false),
        ("seq_idle", "atomik.step-sequencer", false, false),
        ("seq_folded_on", "atomik.step-sequencer", true, true),
        ("filter_idle", "atomik.note-filter", false, false),
    ] {
        let mut d = one_device(type_id);
        if folded {
            apply(
                &mut d,
                Command::SetFolded {
                    key: key("d"),
                    folded: true,
                },
            );
        }
        let mut v = view_of(&d);
        if on {
            v.set_previewing(Some(key("d")));
        }
        let _ = v.handle(&UiEvent::Resize(Size::new(820.0, 420.0)), d.doc(), &reg);
        let _ = v.handle(&UiEvent::ScaleChanged(scale), d.doc(), &reg);
        let mut canvas = CpuCanvas::new((820.0 * scale) as u16, (420.0 * scale) as u16, &mut text);
        v.paint(&mut canvas, d.doc(), &reg);
        let png = canvas.finish().into_png().expect("png");
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/preview")
            .join(format!("player_preview_{name}.png"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, png).unwrap();
        println!("wrote {}", path.display());
    }
}
