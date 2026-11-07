//! The mix channel panel: the compact rack panel of a console channel or output bus. Display-list
//! goldens regenerate with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test mix_panel`.

mod support;

use atomik_device_sdk::{DeviceDescriptor, Registry};
use atomik_model::{
    Command, ConsoleCommand, Document, InsertEffect, OutputDest, bypass_inserts_commands,
    inserts_bypassed,
};
use atomik_ui::autolayout::{CONTENT_X, MARGIN_BOTTOM, MARGIN_TOP};
use atomik_ui::kurbo::{Point, Rect, Size};
use atomik_ui::mix_panel::{
    self, BACK_PAIRS, FRONT_PARAMS, MixButton, geometry, is_mix_type, mix_layout,
};
use atomik_ui::{DisplayList, Flip, PointerButton, RackView, Theme, UiEvent};
use support::starter::*;
use support::*;

fn descriptor(reg: &Registry, type_id: &str) -> DeviceDescriptor {
    *reg.get(type_id).expect("registered").descriptor
}

fn cc(c: ConsoleCommand) -> Command {
    Command::Console(c)
}

fn unfold(d: &mut Document, reg: &Registry, k: &str) {
    run(
        d,
        reg,
        [Command::SetFolded {
            key: key(k),
            folded: false,
        }],
    );
}

/// The starter with two synths, the channel of the first one unfolded and two insert effects on
/// it (a reverb and a delay), and a bus called Drums (key `bus`) unfolded.
fn mixed(reg: &Registry) -> Document {
    let mut d = two_instruments(reg);
    unfold(&mut d, reg, "channel");
    for t in ["atomik.reverb", "atomik.stereo-delay"] {
        let slot = atomik_model::insert_chain(d.doc(), reg, &key("channel")).len();
        run(
            &mut d,
            reg,
            [cc(ConsoleCommand::SetInsert {
                channel: key("channel"),
                slot,
                effect: Some(InsertEffect::New { type_id: t.into() }),
            })],
        );
    }
    run(
        &mut d,
        reg,
        [cc(ConsoleCommand::CreateBus {
            name: "Drums".into(),
            dest: OutputDest::Master,
        })],
    );
    let bus = d.doc().console.buses[0].bus.clone();
    unfold(&mut d, reg, &bus.0);
    d
}

fn tall_view(doc: &atomik_model::RackDoc, reg: &Registry) -> RackView {
    let mut v = RackView::new(Theme::dark());
    let _ = v.handle(&UiEvent::Resize(Size::new(1000.0, 2400.0)), doc, reg);
    v.sync(doc, reg);
    v
}

/// A click at `at`: the actions of the press and of the release.
fn click(
    v: &mut RackView,
    at: Point,
    doc: &atomik_model::RackDoc,
    reg: &Registry,
) -> Vec<atomik_ui::UiAction> {
    let mut out = v.handle(&down(at), doc, reg);
    out.extend(v.handle(&up(at), doc, reg));
    out
}

fn overlaps(a: &Rect, b: &Rect) -> bool {
    a.x0 < b.x1 && b.x0 < a.x1 && a.y0 < b.y1 && b.y0 < a.y1
}

// ------------------------------------------------------------------ layout

#[test]
fn the_panel_is_for_channels_and_buses_only() {
    assert!(is_mix_type("atomik.channel") && is_mix_type("atomik.bus"));
    assert!(!is_mix_type("atomik.master") && !is_mix_type("atomik.poly-synth"));
}

#[test]
fn every_control_is_placed_once_and_nothing_overlaps() {
    let reg = registry();
    for type_id in ["atomik.channel", "atomik.bus"] {
        let desc = descriptor(&reg, type_id);
        let spec = mix_layout(&desc);
        assert_eq!(spec.height_units, 1, "{type_id}: one rack unit");
        let mut keys: Vec<&str> = spec
            .front
            .iter()
            .map(|c| desc.params[c.param].key)
            .collect();
        keys.sort_unstable();
        let mut want = FRONT_PARAMS.to_vec();
        want.sort_unstable();
        assert_eq!(keys, want, "{type_id}: MUTE, SOLO, fader and PAN once each");

        let g = geometry();
        let mut parts: Vec<(String, Rect)> = spec
            .front
            .iter()
            .filter(|c| desc.params[c.param].key != "fader")
            .map(|c| (desc.params[c.param].key.to_string(), c.rect))
            .collect();
        parts.push(("fader".into(), g.fader));
        parts.push(("bypass".into(), g.bypass));
        parts.push(("show".into(), g.show));
        parts.push(("meter l".into(), g.meter[0]));
        parts.push(("meter r".into(), g.meter[1]));
        parts.push(("output".into(), g.output));
        let panel = Rect::new(
            CONTENT_X.0,
            MARGIN_TOP,
            CONTENT_X.1,
            spec.height() - MARGIN_BOTTOM,
        );
        for (name, r) in &parts {
            assert!(
                r.x0 >= panel.x0 && r.x1 <= panel.x1 && r.y0 >= panel.y0 && r.y1 <= panel.y1,
                "{type_id}: {name} {r:?} leaves the panel {panel:?}"
            );
        }
        for (i, (a, ra)) in parts.iter().enumerate() {
            for (b, rb) in &parts[i + 1..] {
                assert!(!overlaps(ra, rb), "{type_id}: {a} overlaps {b}");
            }
        }
        // The section titles run left to right without touching.
        for pair in spec.front_sections.windows(2) {
            assert!(pair[0].rect.x1 <= pair[1].rect.x0, "{type_id}: titles");
        }
        let fader = spec
            .front
            .iter()
            .find(|c| desc.params[c.param].key == "fader")
            .unwrap();
        assert_eq!(fader.rect, g.fader, "the fader control is the track cell");
    }
}

#[test]
fn the_back_has_exactly_the_four_socket_pairs() {
    let reg = registry();
    let channel = descriptor(&reg, "atomik.channel");
    let bus = descriptor(&reg, "atomik.bus");
    let keys = |desc: &DeviceDescriptor| -> Vec<&'static str> {
        mix_layout(desc)
            .back
            .iter()
            .map(|s| desc.sockets[s.socket].key)
            .collect()
    };
    assert_eq!(
        keys(&channel),
        [
            "in_l",
            "in_r",
            "insert_send_l",
            "insert_send_r",
            "insert_return_l",
            "insert_return_r",
            "out_l",
            "out_r"
        ]
    );
    assert_eq!(
        keys(&bus),
        [
            "bus_l",
            "bus_r",
            "insert_send_l",
            "insert_send_r",
            "insert_return_l",
            "insert_return_r",
            "out_l",
            "out_r"
        ],
        "a bus takes the summing input"
    );
    let titles: Vec<String> = mix_layout(&channel)
        .back_sections
        .iter()
        .map(|s| s.label.clone())
        .collect();
    assert_eq!(titles, BACK_PAIRS.map(|(t, ..)| t.to_string()));
    // No cell touches another, and every title has room.
    let spec = mix_layout(&channel);
    for (i, a) in spec.back.iter().enumerate() {
        for b in &spec.back[i + 1..] {
            assert!(!overlaps(&a.rect, &b.rect));
        }
    }
    let theme = Theme::dark();
    for (s, next) in spec
        .back_sections
        .iter()
        .zip(spec.back_sections.iter().skip(1))
    {
        let width = atomik_ui::paint::estimate_width(&s.label, theme.section_size) + 8.0;
        assert!(s.rect.x0 + width <= next.rect.x0, "{} fits", s.label);
    }
}

#[test]
fn the_rack_draws_only_those_jacks_and_every_other_socket_keeps_working() {
    let reg = registry();
    let d = mixed(&reg);
    let doc = d.doc();
    let mut v = tall_view(doc, &reg);
    v.set_flip(Flip::Back);
    let bus = doc.console.buses[0].bus.clone();
    for k in [
        "in_l",
        "in_r",
        "insert_send_l",
        "insert_send_r",
        "insert_return_l",
        "insert_return_r",
        "out_l",
        "out_r",
    ] {
        assert!(
            v.socket_center(&sref("channel", k)).is_some(),
            "channel {k}"
        );
    }
    for k in [
        "key_in",
        "send1_l",
        "send8_r",
        "gr_cv_out",
        "fader_cv_in",
        "pan_cv_in",
        "send1_level_cv_in",
        "parallel_out_l",
        "follow_in",
    ] {
        assert!(
            v.socket_center(&sref("channel", k)).is_none(),
            "channel {k} is not drawn"
        );
        assert!(
            reg.get("atomik.channel")
                .unwrap()
                .descriptor
                .socket_by_key(k)
                .is_some(),
            "channel {k} stays in the model"
        );
    }
    assert!(v.socket_center(&sref(&bus.0, "bus_l")).is_some());
    assert!(v.socket_center(&sref(&bus.0, "follow_out")).is_none());
    // The hidden sends are still cabled to the master.
    assert!(
        doc.cables
            .iter()
            .any(|c| c.from.device == key("channel") && c.from.socket == "send1_l")
    );
}

// ------------------------------------------------------------------ visibility

#[test]
fn channels_buses_and_the_master_are_always_in_the_rack() {
    let reg = registry();
    let d = mixed(&reg);
    let doc = d.doc();
    let v = tall_view(doc, &reg);
    let bus = doc.console.buses[0].bus.clone();
    for k in [key("master"), key("channel"), key("channel-2"), bus.clone()] {
        assert!(v.device_rect(&k).is_some(), "{k} is shown");
    }
    assert_eq!(v.mix_panel_keys(), [key("channel"), key("channel-2"), bus]);
}

#[test]
fn a_folded_strip_is_a_slim_line_and_an_unfolded_one_is_one_rack_unit() {
    let reg = registry();
    let d = mixed(&reg);
    let v = tall_view(d.doc(), &reg);
    let h = |k: &str| v.device_rect(&key(k)).unwrap().height();
    assert!(
        (h("channel") - 76.0).abs() < 1e-9,
        "one unit: {}",
        h("channel")
    );
    assert!(h("channel-2") < 50.0, "folded: {}", h("channel-2"));
    assert!(v.shows_mix_panels());
}

// ------------------------------------------------------------------ same params as the console

/// The command of the first `SetParam` in `actions` for `param`.
fn set_param(actions: &[atomik_ui::UiAction], param: &str) -> Option<(f32, Option<u64>)> {
    commands(actions).into_iter().find_map(|c| match c {
        Command::SetParam {
            param: p,
            value,
            merge,
            ..
        } if p == param => Some((value, merge)),
        _ => None,
    })
}

#[test]
fn mute_and_solo_set_the_params_the_console_strip_sets() {
    let reg = registry();
    let d = mixed(&reg);
    let doc = d.doc().clone();
    let mut v = tall_view(&doc, &reg);
    for param in ["mute", "solo"] {
        let at = v.control_center(&key("channel"), param).unwrap();
        let out = click(&mut v, at, &doc, &reg);
        assert_eq!(
            commands(&out),
            [Command::SetParam {
                key: key("channel"),
                param: param.to_string(),
                value: 1.0,
                merge: None
            }],
            "{param}"
        );
    }
}

#[test]
fn the_fader_drags_along_its_track_and_resets_on_a_double_click() {
    let reg = registry();
    let d = mixed(&reg);
    let doc = d.doc().clone();
    let mut v = tall_view(&doc, &reg);
    let at = v.control_center(&key("channel"), "fader").unwrap();
    let g = geometry();
    let travel = g.travel.1 - g.travel.0;
    // A drag to the right raises the fader; one full track length sweeps the range.
    let _ = v.handle(&down(at), &doc, &reg);
    let a = v.handle(&mv(Point::new(at.x - travel, at.y)), &doc, &reg);
    let (low, merge) = set_param(&a, "fader").expect("moved");
    assert!(low < 1e-6, "all the way left: {low}");
    assert!(merge.is_some(), "a drag is one merged undo step");
    let b = v.handle(
        &mv(Point::new(at.x - travel + travel / 2.0, at.y + 30.0)),
        &doc,
        &reg,
    );
    let (mid, _) = set_param(&b, "fader").expect("moved");
    assert!((mid - 0.5).abs() < 0.02, "half a track right: {mid}");
    let _ = v.handle(&up(Point::new(at.x, at.y)), &doc, &reg);
    // A vertical move alone changes nothing.
    let _ = v.handle(&down(at), &doc, &reg);
    let c = v.handle(&mv(Point::new(at.x, at.y - 40.0)), &doc, &reg);
    assert!(set_param(&c, "fader").is_none());
    let _ = v.handle(&up(Point::new(at.x, at.y - 40.0)), &doc, &reg);
    // A double-click resets to unity.
    let mut out = v.handle(&down_with(at, PointerButton::Primary, 2), &doc, &reg);
    out.extend(v.handle(&up(at), &doc, &reg));
    let all = commands(&out);
    assert!(
        all.iter()
            .any(|c| matches!(c, Command::SetParam { param, value, .. }
            if param == "fader" && (*value - atomik_dsp::FADER_UNITY_POS).abs() < 1e-6)),
        "{all:?}"
    );
}

#[test]
fn the_panel_follows_edits_made_anywhere_and_the_fader_reads_in_db() {
    let reg = registry();
    let mut d = mixed(&reg);
    let doc = d.doc().clone();
    let mut v = tall_view(&doc, &reg);
    let before = paint_json(&mut v, &doc, &reg);
    assert!(before.contains("\"+0.0\""), "unity reads +0.0");
    let revisions = v.panel_revisions();
    // The console strip (or anything else) moves the fader and the mute.
    run(
        &mut d,
        &reg,
        [
            Command::SetParam {
                key: key("channel"),
                param: "fader".into(),
                value: 0.5,
                merge: None,
            },
            Command::SetParam {
                key: key("channel"),
                param: "mute".into(),
                value: 1.0,
                merge: None,
            },
        ],
    );
    let doc = d.doc().clone();
    v.sync(&doc, &reg);
    assert_ne!(v.panel_revisions(), revisions, "the panel is re-recorded");
    let after = paint_json(&mut v, &doc, &reg);
    assert_ne!(before, after);
    let db = atomik_dsp::fader_db(0.5);
    assert!(after.contains(&format!("\"{db:+.1}\"")), "{db}");
}

#[test]
fn bypass_insert_fx_is_one_undo_step_and_the_console_switch() {
    let reg = registry();
    let mut d = mixed(&reg);
    let doc = d.doc().clone();
    let mut v = tall_view(&doc, &reg);
    assert_eq!(inserts_bypassed(&doc, &reg, &key("channel")), Some(false));
    let at = v
        .mix_button_center(&key("channel"), MixButton::BypassInserts)
        .unwrap();
    let out = click(&mut v, at, &doc, &reg);
    let cmds = commands(&out);
    // The console's B-all switch builds the same batch.
    let want = bypass_inserts_commands(&doc, &reg, &key("channel"), true);
    assert_eq!(want.len(), 2, "two insert effects");
    assert_eq!(cmds, [Command::Batch(want)]);
    let steps = d.undo_depth();
    d.apply(cmds[0].clone(), &reg).unwrap();
    assert_eq!(d.undo_depth(), steps + 1, "one undo step");
    assert_eq!(inserts_bypassed(d.doc(), &reg, &key("channel")), Some(true));
    // The panel lights up, and a second press restores them all.
    let doc2 = d.doc().clone();
    v.sync(&doc2, &reg);
    let lit = paint_json(&mut v, &doc2, &reg);
    assert!(lit.contains("BYPASS INSERT FX"));
    let out = click(&mut v, at, &doc2, &reg);
    d.apply(commands(&out)[0].clone(), &reg).unwrap();
    assert_eq!(
        inserts_bypassed(d.doc(), &reg, &key("channel")),
        Some(false)
    );
    d.undo().unwrap();
    assert_eq!(inserts_bypassed(d.doc(), &reg, &key("channel")), Some(true));
}

#[test]
fn a_strip_without_inserts_toggles_its_own_insert_bypass_param() {
    let reg = registry();
    let mut d = two_instruments(&reg);
    unfold(&mut d, &reg, "channel-2");
    let doc = d.doc().clone();
    let mut v = tall_view(&doc, &reg);
    assert_eq!(inserts_bypassed(&doc, &reg, &key("channel-2")), None);
    let at = v
        .mix_button_center(&key("channel-2"), MixButton::BypassInserts)
        .unwrap();
    let out = click(&mut v, at, &doc, &reg);
    assert_eq!(
        commands(&out),
        [Command::SetParam {
            key: key("channel-2"),
            param: "insert.bypass".into(),
            value: 1.0,
            merge: None
        }]
    );
}

// ------------------------------------------------------------------ show insert fx

#[test]
fn show_insert_fx_is_view_state_that_starts_on_and_is_pruned_with_the_device() {
    let reg = registry();
    let d = mixed(&reg);
    let doc = d.doc().clone();
    let mut v = tall_view(&doc, &reg);
    assert!(v.inserts_shown(&key("channel")), "starts shown");
    let at = v
        .mix_button_center(&key("channel"), MixButton::ShowInserts)
        .unwrap();
    let revisions = v.panel_revisions();
    let out = click(&mut v, at, &doc, &reg);
    assert!(commands(&out).is_empty(), "state only: no document command");
    assert!(!v.inserts_shown(&key("channel")));
    assert!(v.inserts_shown(&key("channel-2")), "per device");
    assert_ne!(v.panel_revisions(), revisions, "the switch is redrawn");
    let _ = click(&mut v, at, &doc, &reg);
    assert!(v.inserts_shown(&key("channel")));
    assert!(!v.set_inserts_shown(&key("channel"), true), "no change");
    // A hidden strip that leaves the document takes its state along.
    assert!(v.set_inserts_shown(&key("channel-2"), false));
    let held = v.device_state_len();
    let mut doc3 = doc.clone();
    doc3.devices.retain(|x| x.key != key("channel-2"));
    doc3.console
        .channels
        .retain(|c| c.channel != key("channel-2"));
    doc3.cables
        .retain(|c| c.from.device != key("channel-2") && c.to.device != key("channel-2"));
    v.sync(&doc3, &reg);
    assert!(v.device_state_len() < held);
    assert!(
        v.inserts_shown(&key("channel-2"))
            || !doc3.devices.iter().any(|x| x.key == key("channel-2"))
    );
}

// ------------------------------------------------------------------ output display and meter

#[test]
fn the_audio_output_display_names_where_the_strip_plays() {
    assert_eq!(mix_panel::output_words("Output: Master"), "Master Section");
    assert_eq!(mix_panel::output_words("Output: Bus Drums"), "Bus Drums");
    assert_eq!(
        mix_panel::output_words("Output: Audio I/O 3+4"),
        "Audio I/O 3+4"
    );
    assert_eq!(mix_panel::output_words("Output: none"), "none");
    let reg = registry();
    let d = mixed(&reg);
    let doc = d.doc();
    let mut v = tall_view(doc, &reg);
    let json = paint_json(&mut v, doc, &reg);
    assert!(
        json.contains("\"Master Section\""),
        "the panel says where it plays"
    );
    // Only the folded strip (`channel-2`) keeps the label on its plate: the unfolded channel and
    // the bus say it in their panels.
    assert_eq!(
        json.matches("Output: Master").count(),
        1,
        "the plate label stays on the folded strip only"
    );
}

#[test]
fn the_meter_repaints_only_when_a_bar_moves_and_only_while_in_view() {
    let reg = registry();
    let d = mixed(&reg);
    let doc = d.doc();
    let mut v = tall_view(doc, &reg);
    let _ = paint_json(&mut v, doc, &reg);
    assert!(!v.needs_repaint());
    v.set_mix_meters(&key("channel"), [-20.0, -30.0]);
    assert!(v.needs_repaint(), "a bar moved");
    let _ = paint_json(&mut v, doc, &reg);
    v.set_mix_meters(&key("channel"), [-20.1, -30.1]);
    assert!(!v.needs_repaint(), "inside the same half dB step");
    v.set_mix_meters(&key("channel"), [-20.0, f32::NAN]);
    assert!(v.needs_repaint());
    let _ = paint_json(&mut v, doc, &reg);
    // A device that is no strip takes nothing.
    v.set_mix_meters(&key("synth"), [-3.0, -3.0]);
    v.set_mix_meters(&key("nobody"), [-3.0, -3.0]);
    assert!(!v.needs_repaint());
    // A folded strip stores the reading without a repaint.
    v.set_mix_meters(&key("channel-2"), [-3.0, -3.0]);
    assert!(!v.needs_repaint());
    assert_eq!(mix_panel::quantise_meter(-20.2), -20.0);
    assert_eq!(mix_panel::quantise_meter(f32::NEG_INFINITY), -60.0);
}

#[test]
fn fader_text_reads_unity_as_zero_db() {
    assert_eq!(mix_panel::fader_text(atomik_dsp::FADER_UNITY_POS), "+0.0");
    assert_eq!(mix_panel::fader_text(0.0), "-inf");
    assert!((mix_panel::fader_at(geometry().travel.0)).abs() < 1e-12);
    assert!((mix_panel::fader_at(geometry().travel.1) - 1.0).abs() < 1e-12);
}

// ------------------------------------------------------------------ goldens

fn golden_at(scale: f64) -> String {
    let reg = registry();
    let d = mixed(&reg);
    let doc = d.doc();
    let mut v = RackView::new(Theme::dark());
    let _ = v.handle(&UiEvent::Resize(Size::new(1000.0, 1400.0)), doc, &reg);
    let _ = v.handle(&UiEvent::ScaleChanged(scale), doc, &reg);
    v.sync(doc, &reg);
    v.set_mix_meters(&key("channel"), [-14.0, -9.0]);
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &reg);
    v.set_flip(Flip::Back);
    v.paint(&mut dl, doc, &reg);
    dl.to_json()
}

#[test]
fn the_mix_channel_panel_matches_its_goldens() {
    check_golden("rack_mix_channel_1x.json", &golden_at(1.0));
    check_golden("rack_mix_channel_2x.json", &golden_at(2.0));
}

// ------------------------------------------------------------------ name plate

fn press_key(v: &mut RackView, doc: &atomik_model::RackDoc, reg: &Registry, k: atomik_ui::Key) {
    let _ = v.handle(
        &UiEvent::Key {
            key: k,
            modifiers: atomik_ui::Modifiers::NONE,
            pressed: true,
        },
        doc,
        reg,
    );
}

#[test]
fn the_plate_follows_the_source_instruments_name_and_a_rename_renames_it() {
    let reg = registry();
    let d = mixed(&reg);
    let doc = d.doc().clone();
    let mut v = tall_view(&doc, &reg);
    let synth = doc.device(&key("synth")).unwrap().name.clone();
    let p = v.name_center(&key("channel")).unwrap();
    let _ = v.handle(&down_with(p, PointerButton::Primary, 1), &doc, &reg);
    let _ = v.handle(&down_with(p, PointerButton::Primary, 2), &doc, &reg);
    assert_eq!(
        v.renaming_device(&key("channel")),
        Some(synth.as_str()),
        "the field holds the name the plate shows: the instrument's"
    );
    press_key(&mut v, &doc, &reg, atomik_ui::Key::Char('2'));
    let out = v.handle(
        &UiEvent::Key {
            key: atomik_ui::Key::Enter,
            modifiers: atomik_ui::Modifiers::NONE,
            pressed: true,
        },
        &doc,
        &reg,
    );
    assert_eq!(
        commands(&out),
        [Command::Rename {
            key: key("synth"),
            name: format!("{synth}2")
        }]
    );
    // A bus has no source: its plate and a rename are its own.
    let bus = doc.console.buses[0].bus.clone();
    let p = v.name_center(&bus).unwrap();
    let _ = v.handle(&down_with(p, PointerButton::Primary, 1), &doc, &reg);
    let _ = v.handle(&down_with(p, PointerButton::Primary, 2), &doc, &reg);
    assert_eq!(v.renaming_device(&bus), Some("Drums"));
}

#[test]
fn every_jack_of_the_back_has_a_short_label_that_tells_the_side() {
    let reg = registry();
    for type_id in ["atomik.channel", "atomik.bus"] {
        let desc = descriptor(&reg, type_id);
        for s in &mix_layout(&desc).back {
            let key = desc.sockets[s.socket].key;
            let label = mix_panel::socket_label(key).expect(key);
            assert!(
                label.ends_with(" L") || label.ends_with(" R"),
                "{key}: {label}"
            );
            assert!(label.len() <= 8, "{label} fits a 50 px cell");
        }
    }
    assert_eq!(mix_panel::socket_label("send1_l"), None);
}
