//! The BYPASS switch of a built-in effect's name strip: which devices have it, the click (one
//! `SetParam`, 1 then 0), its geometry in the strip (every panel height, folded or not), the
//! veil over a bypassed body, and the `bypass` param never being a knob of a panel body.

mod support;

use atomik_core::Unit;
use atomik_device_sdk::{DeviceDescriptor, DeviceKind, RACK_UNIT, is_plugin_type_id};
use atomik_model::{Command, Document};
use atomik_ui::autolayout::{
    BYPASS_KEY, PLATE_X, PanelSpec, auto_layout, from_panel_layout, has_strip_bypass,
};
use atomik_ui::fx::FxKind;
use atomik_ui::fx_layout::fx_layout;
use atomik_ui::kurbo::{Circle, Rect, Shape};
use atomik_ui::paint::{bypass_button_rect, bypass_veil, fold_button_center, plate_rect};
use atomik_ui::{DisplayList, Geom, Op, RackView, Theme, UiAction};
use support::container::{apply, check, paint_rack};
use support::plugin::plugin_rack;
use support::*;

const PLAYERS: [&str; 6] = [
    "atomik.arpeggiator",
    "atomik.step-sequencer",
    "atomik.note-filter",
    "atomik.key-chord",
    "atomik.note-repeater",
    "atomik.note-delay",
];

const EFFECTS: [&str; 10] = [
    "atomik.reverb",
    "atomik.compressor",
    "atomik.eq",
    "atomik.distortion",
    "atomik.modulation",
    "atomik.mastering",
    "atomik.character",
    "atomik.stereo-delay",
    "atomik.spectrum",
    "atomik.phase-meter",
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

fn press(v: &mut RackView, d: &Document) -> Vec<UiAction> {
    let at = v.bypass_button_rect(&key("d")).expect("a switch").center();
    let acts = v.handle(&down(at), d.doc(), &registry());
    let _ = v.handle(&up(at), d.doc(), &registry());
    acts
}

fn bypass_value(d: &Document) -> f32 {
    d.doc()
        .device(&key("d"))
        .and_then(|x| x.params.get(BYPASS_KEY).copied())
        .unwrap_or(0.0)
}

fn set_bypass(value: f32) -> Command {
    Command::SetParam {
        key: key("d"),
        param: BYPASS_KEY.into(),
        value,
        merge: None,
    }
}

/// The panel spec the rack builds for an effect.
fn spec_of(desc: &DeviceDescriptor) -> PanelSpec {
    match FxKind::of(desc.type_id) {
        Some(k) => fx_layout(k, desc),
        None => match desc.type_id {
            "atomik.arpeggiator" => atomik_ui::arp_panel::arp_layout(desc),
            "atomik.step-sequencer" => atomik_ui::seq_panel::seq_layout(desc),
            "atomik.key-chord" => atomik_ui::key_chord_panel::key_chord_layout(desc),
            _ => auto_layout(desc),
        },
    }
}

#[test]
fn only_the_builtin_effects_and_the_players_have_the_switch() {
    let reg = registry();
    let mut with: Vec<&str> = Vec::new();
    for type_id in reg.type_ids() {
        let Some(d) = one_device(type_id) else {
            continue;
        };
        if view_of(&d).bypass_button_rect(&key("d")).is_some() {
            with.push(type_id);
        }
    }
    with.sort_unstable();
    let mut want = EFFECTS.to_vec();
    want.extend(PLAYERS);
    want.sort_unstable();
    assert_eq!(with, want);
    // No instrument, mixer or other utility has one, and no plugin (effect plugins keep the BYPASS
    // button inside their panel).
    for t in reg.type_ids() {
        let desc = reg.descriptor(t).unwrap();
        let expected =
            (desc.kind == DeviceKind::Effect && !is_plugin_type_id(t)) || PLAYERS.contains(&t);
        assert_eq!(has_strip_bypass(desc), expected && want.contains(&t), "{t}");
    }
    let p = plugin_rack();
    let v = view_of(&p);
    for k in ["synth", "fx"] {
        assert!(v.bypass_button_rect(&key(k)).is_none(), "plugin {k}");
    }
}

#[test]
fn a_click_toggles_the_param_with_one_command_and_one_undo_step() {
    for t in EFFECTS {
        let mut d = one_device(t).unwrap();
        let mut v = view_of(&d);
        let cmds = commands(&press(&mut v, &d));
        assert_eq!(cmds, vec![set_bypass(1.0)], "{t}");
        for c in cmds {
            apply(&mut d, c);
        }
        assert_eq!(bypass_value(&d), 1.0, "{t}");
        v.sync(d.doc(), &registry());
        let cmds = commands(&press(&mut v, &d));
        assert_eq!(cmds, vec![set_bypass(0.0)], "{t} back to 0");
        for c in cmds {
            apply(&mut d, c);
        }
        assert_eq!(bypass_value(&d), 0.0, "{t}");
        // Two edits, two undo steps.
        d.undo(&registry()).expect("undo");
        assert_eq!(bypass_value(&d), 1.0, "{t}");
        d.undo(&registry()).expect("undo");
        assert_eq!(bypass_value(&d), 0.0, "{t}");
    }
}

#[test]
fn the_switch_works_on_a_folded_panel() {
    let mut d = one_device("atomik.reverb").unwrap();
    apply(
        &mut d,
        Command::SetFolded {
            key: key("d"),
            folded: true,
        },
    );
    let mut v = view_of(&d);
    assert_eq!(commands(&press(&mut v, &d)), vec![set_bypass(1.0)]);
}

#[test]
fn the_switch_sits_in_the_strip_and_overlaps_nothing_at_every_panel_height() {
    let theme = Theme::dark();
    for u in [1.0, 2.0, 3.0, 4.0, 6.0] {
        let h = u * f64::from(RACK_UNIT);
        // Unfolded: inside the plate, under the (at most two) name lines and the type label,
        // above the fold chevron and the status LED.
        let plate = plate_rect(h, false);
        let b = bypass_button_rect(h, false);
        assert!(
            b.x0 >= plate.x0 && b.x1 <= plate.x1 && b.y0 >= plate.y0 && b.y1 <= plate.y1,
            "{u}U inside the strip: {b:?} / {plate:?}"
        );
        // One unit tall the name is cut to a single line.
        let lines = if u < 2.0 { 1.0 } else { 2.0 };
        let text_bottom = plate.y0 + 19.0 + (f64::from(theme.name_size) + 1.0) * lines - 3.0 + 2.0;
        assert!(b.y0 >= text_bottom, "{u}U: under the labels");
        let fold = Circle::new(fold_button_center(h, false), 8.0).bounding_box();
        assert!(!overlaps(&b, &fold), "{u}U: clear of the chevron");
        let led = Circle::new((plate.x1 - 9.0, plate.y1 - 9.0 + 2.0), 6.0).bounding_box();
        assert!(!overlaps(&b, &led), "{u}U: clear of the LED");
        // Folded: inside the plate, clear of the chevron.
        let fh = h / 2.0;
        let fplate = plate_rect(fh, true);
        let fb = bypass_button_rect(fh, true);
        assert!(
            fb.x0 >= fplate.x0 && fb.x1 <= fplate.x1 && fb.y0 >= fplate.y0 && fb.y1 <= fplate.y1,
            "{u}U folded: {fb:?}"
        );
        let ffold = Circle::new(fold_button_center(fh, true), 8.0).bounding_box();
        assert!(!overlaps(&fb, &ffold), "{u}U folded: clear of the chevron");
        // The veil starts right of the strip.
        assert!(bypass_veil(h).x0 >= PLATE_X.1);
        assert!(!overlaps(&b, &bypass_veil(h)));
    }
}

#[test]
fn every_effect_panel_fits_the_switch() {
    let reg = registry();
    for t in EFFECTS {
        let d = one_device(t).unwrap();
        let v = view_of(&d);
        let b = v.bypass_button_rect(&key("d")).unwrap();
        let fold = v.fold_button_center(&key("d")).unwrap();
        assert!(
            !overlaps(&b, &Circle::new(fold, 7.0).bounding_box()),
            "{t}: clear of the chevron"
        );
        let h = spec_of(reg.descriptor(t).unwrap()).height();
        // The stereo delay is one unit tall: its name is cut to one line above the switch.
        let plate = plate_rect(h, false);
        let lines = if h < 2.0 * f64::from(RACK_UNIT) {
            1.0
        } else {
            2.0
        };
        let text_bottom = plate.y0 + 19.0 + 14.0 * lines - 3.0 + 2.0;
        assert!(bypass_button_rect(h, false).y0 >= text_bottom, "{t}");
    }
}

fn veil_count(dl: &DisplayList, veil: Rect) -> usize {
    dl.ops()
        .iter()
        .filter(|op| matches!(op, Op::Fill { shape: Geom::Rect(r), .. } if *r == veil))
        .count()
}

#[test]
fn the_body_is_veiled_only_while_bypassed_and_the_strip_stays_lit() {
    let reg = registry();
    for t in EFFECTS {
        let mut d = one_device(t).unwrap();
        let mut v = view_of(&d);
        let veil = bypass_veil(spec_of(reg.descriptor(t).unwrap()).height());
        let off = paint_rack(&mut v, d.doc());
        assert_eq!(veil_count(&off, veil), 0, "{t}: no veil when not bypassed");
        assert!(off.texts().contains(&"BYPASS"), "{t}");
        apply(&mut d, set_bypass(1.0));
        v.sync(d.doc(), &reg);
        let on = paint_rack(&mut v, d.doc());
        assert_eq!(veil_count(&on, veil), 1, "{t}: one veil when bypassed");
        assert!(on.texts().contains(&"BYPASS"), "{t}");
        // The lit LED adds a glow circle to the button.
        assert!(on.len() > off.len(), "{t}: lit LED and veil");
        apply(&mut d, set_bypass(0.0));
        v.sync(d.doc(), &reg);
        let back = paint_rack(&mut v, d.doc());
        assert_eq!(veil_count(&back, veil), 0, "{t}: veil gone again");
    }
}

#[test]
fn no_panel_body_places_bypass_as_a_control() {
    let reg = registry();
    for t in EFFECTS {
        let desc = reg.descriptor(t).unwrap();
        let bypass = desc
            .params
            .iter()
            .position(|p| p.key == BYPASS_KEY)
            .unwrap();
        assert_eq!(bypass, desc.params.len() - 1, "{t}: appended last");
        let mut specs = vec![auto_layout(desc), spec_of(desc)];
        if let Some(layout) = atomik_devices::panel_layout(t) {
            specs.push(from_panel_layout(layout, desc));
        }
        for s in specs {
            assert!(s.front.iter().all(|c| c.param != bypass), "{t}");
        }
    }
}

#[test]
fn bypass_stays_in_the_descriptor_for_automation_and_the_param_lists() {
    // The param lists read the descriptor (automation lanes, the plugin param window): the
    // param is there, a normal discrete toggle, like the plugin one.
    let reg = registry();
    for t in EFFECTS {
        let p = reg.descriptor(t).unwrap().param_by_key(BYPASS_KEY).unwrap();
        assert_eq!(p.unit, Unit::Toggle, "{t}");
        assert!(p.discrete, "{t}");
    }
}

#[test]
fn player_panels_show_the_strip_bypass() {
    let reg = registry();
    for t in PLAYERS {
        let mut d = one_device(t).unwrap();
        let mut v = view_of(&d);
        let b = v.bypass_button_rect(&key("d")).expect("a switch");
        let fold = v.fold_button_center(&key("d")).unwrap();
        assert!(!overlaps(&b, &Circle::new(fold, 7.0).bounding_box()), "{t}");
        // One SetParam per click, 1 then 0.
        assert_eq!(commands(&press(&mut v, &d)), vec![set_bypass(1.0)], "{t}");
        // The param is never a control of the panel body.
        let desc = reg.descriptor(t).unwrap();
        let bypass = desc
            .params
            .iter()
            .position(|p| p.key == BYPASS_KEY)
            .unwrap();
        assert_eq!(bypass, desc.params.len() - 1, "{t}: appended last");
        assert!(spec_of(desc).front.iter().all(|c| c.param != bypass), "{t}");
        // The veil covers the body only while bypassed, the strip stays lit.
        let veil = bypass_veil(spec_of(desc).height());
        let off = paint_rack(&mut v, d.doc());
        assert_eq!(veil_count(&off, veil), 0, "{t}: no veil when not bypassed");
        assert!(off.texts().contains(&"BYPASS"), "{t}");
        apply(&mut d, set_bypass(1.0));
        v.sync(d.doc(), &reg);
        let on = paint_rack(&mut v, d.doc());
        assert_eq!(veil_count(&on, veil), 1, "{t}: one veil when bypassed");
        assert!(on.texts().contains(&"BYPASS"), "{t}");
        assert_eq!(commands(&press(&mut v, &d)), vec![set_bypass(0.0)], "{t}");
    }
}

// ------------------------------------------------------------------ goldens

fn golden_of(type_id: &str, bypassed: bool, folded: bool) -> String {
    let mut d = one_device(type_id).unwrap();
    if bypassed {
        apply(&mut d, set_bypass(1.0));
    }
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
    paint_rack(&mut v, d.doc()).to_json()
}

/// Regenerate with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test effect_bypass`.
#[test]
fn goldens_of_the_strip_switch_lit_and_unlit_folded_and_one_unit_tall() {
    check(
        "effect_bypass_reverb_on_1x.json",
        &golden_of("atomik.reverb", true, false),
    );
    check(
        "effect_bypass_reverb_folded_on_1x.json",
        &golden_of("atomik.reverb", true, true),
    );
    check(
        "effect_bypass_reverb_folded_off_1x.json",
        &golden_of("atomik.reverb", false, true),
    );
    // A player's strip carries the same switch.
    check(
        "effect_bypass_arpeggiator_on_1x.json",
        &golden_of("atomik.arpeggiator", true, false),
    );
    // The stereo delay is one unit tall (auto-layout panel).
    check(
        "effect_bypass_delay_off_1x.json",
        &golden_of("atomik.stereo-delay", false, false),
    );
    check(
        "effect_bypass_delay_on_1x.json",
        &golden_of("atomik.stereo-delay", true, false),
    );
}

/// Writes `target/preview/effect_bypass_*.png` (not committed):
/// `cargo test -p atomik-ui --test effect_bypass -- --ignored`.
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
        ("reverb_off", "atomik.reverb", false, false),
        ("reverb_on", "atomik.reverb", true, false),
        ("reverb_folded_on", "atomik.reverb", true, true),
        ("delay_off", "atomik.stereo-delay", false, false),
        ("delay_on", "atomik.stereo-delay", true, false),
        ("compressor_on", "atomik.compressor", true, false),
    ] {
        let mut d = one_device(type_id).unwrap();
        if on {
            apply(&mut d, set_bypass(1.0));
        }
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
        let _ = v.handle(&UiEvent::Resize(Size::new(820.0, 280.0)), d.doc(), &reg);
        let _ = v.handle(&UiEvent::ScaleChanged(scale), d.doc(), &reg);
        let mut canvas = CpuCanvas::new((820.0 * scale) as u16, (280.0 * scale) as u16, &mut text);
        v.paint(&mut canvas, d.doc(), &reg);
        let png = canvas.finish().into_png().expect("png");
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/preview")
            .join(format!("effect_bypass_{name}.png"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, png).unwrap();
        println!("wrote {}", path.display());
    }
}
