//! The master section panel in the rack: the compact front (fader, meter, the two insert
//! buttons and eight send rows), the back (sends, returns, insert loop, outputs), the master's
//! insert area (the channel gestures) and the shared send effects grouped under it as "Send N".
//! Goldens: `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test master_section`.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::{
    Command, ConsoleCommand, CreateOptions, CreateSource, CreateTarget, DeviceKey, Document,
    InsertEffect, WorkflowCommand, insert_chain, send_chain,
};
use atomik_ui::kurbo::{Point, Rect, Size};
use atomik_ui::{Flip, RackView, Theme, UiAction, UiEvent};
use support::starter::*;
use support::*;

fn cc(c: ConsoleCommand) -> Command {
    Command::Console(c)
}

fn create(target: CreateTarget, t: &str) -> Command {
    Command::Workflow(WorkflowCommand::CreateEffect {
        source: CreateSource::Type(t.into()),
        target,
        key: None,
        options: CreateOptions::default(),
    })
}

/// Two synths, two inserts on the master and three send effects (the second with a second
/// effect in series).
fn sectioned(reg: &Registry) -> Document {
    let mut d = two_instruments(reg);
    for (slot, t) in ["atomik.eq", "atomik.compressor"].into_iter().enumerate() {
        run(
            &mut d,
            reg,
            [cc(ConsoleCommand::AddInsert {
                channel: key("master"),
                slot,
                effect: InsertEffect::New { type_id: t.into() },
            })],
        );
    }
    for t in ["atomik.reverb", "atomik.stereo-delay", "atomik.reverb"] {
        run(&mut d, reg, [create(CreateTarget::Master, t)]);
    }
    let second = send_chain(d.doc(), reg, 1)[0].clone();
    run(
        &mut d,
        reg,
        [create(CreateTarget::Selected(second), "atomik.distortion")],
    );
    d
}

fn tall(doc: &atomik_model::RackDoc, reg: &Registry) -> RackView {
    let mut v = RackView::new(Theme::dark());
    let _ = v.handle(&UiEvent::Resize(Size::new(1000.0, 7000.0)), doc, reg);
    v.sync(doc, reg);
    v
}

fn rect(v: &RackView, k: &DeviceKey) -> Rect {
    v.device_rect(k).unwrap_or_else(|| panic!("{k} is drawn"))
}

fn click(
    v: &mut RackView,
    at: Point,
    doc: &atomik_model::RackDoc,
    reg: &Registry,
) -> Vec<UiAction> {
    let mut out = v.handle(&down(at), doc, reg);
    out.extend(v.handle(&up(at), doc, reg));
    out
}

fn drag(
    v: &mut RackView,
    doc: &atomik_model::RackDoc,
    reg: &Registry,
    a: Point,
    b: Point,
) -> Vec<UiAction> {
    let mut out = v.handle(&down(a), doc, reg);
    out.extend(v.handle(&mv(Point::new(a.x, a.y + 12.0)), doc, reg));
    out.extend(v.handle(&mv(b), doc, reg));
    out.extend(v.handle(&up(b), doc, reg));
    out
}

fn master_desc(reg: &Registry) -> atomik_device_sdk::DeviceDescriptor {
    *reg.get("atomik.master").unwrap().descriptor
}

fn texts_of(v: &mut RackView, doc: &atomik_model::RackDoc, reg: &Registry) -> Vec<String> {
    let mut list = atomik_ui::DisplayList::new();
    v.paint(&mut list, doc, reg);
    list.texts().into_iter().map(str::to_owned).collect()
}

// ------------------------------------------------------------------ panel

#[test]
fn the_front_places_the_fader_and_the_eight_return_levels_only() {
    let reg = registry();
    let desc = master_desc(&reg);
    let spec = atomik_ui::master_panel::master_layout(&desc);
    assert_eq!(spec.height_units, 4);
    let mut keys: Vec<&str> = spec
        .front
        .iter()
        .map(|c| desc.params[c.param].key)
        .collect();
    assert_eq!(keys.remove(0), "fader");
    let want: Vec<String> = (1..=8).map(|k| format!("return{k}.level")).collect();
    assert_eq!(keys, want.iter().map(String::as_str).collect::<Vec<_>>());
    // The bus compressor and the control room stay on the mixer.
    assert!(!keys.contains(&"comp.on") && !keys.contains(&"monitor.source"));
    assert_eq!(spec.front_sections.last().unwrap().label, "SEND EFFECTS");
}

#[test]
fn the_back_draws_sends_returns_insert_loop_and_outputs_and_nothing_else() {
    let reg = registry();
    let desc = master_desc(&reg);
    let spec = atomik_ui::master_panel::master_layout(&desc);
    let mut got: Vec<&str> = spec
        .back
        .iter()
        .map(|b| desc.sockets[b.socket].key)
        .collect();
    got.sort_unstable();
    let mut want: Vec<String> = Vec::new();
    for k in 1..=8 {
        for side in ["l", "r"] {
            want.push(format!("fx_send{k}_{side}"));
            want.push(format!("return{k}_{side}"));
        }
    }
    for k in [
        "insert_send_l",
        "insert_send_r",
        "insert_return_l",
        "insert_return_r",
        "main_l",
        "main_r",
        "monitor_l",
        "monitor_r",
    ] {
        want.push(k.to_string());
    }
    want.sort_unstable();
    assert_eq!(got, want.iter().map(String::as_str).collect::<Vec<_>>());
    let titles: Vec<&str> = spec
        .back_sections
        .iter()
        .map(|s| s.label.as_str())
        .collect();
    for t in [
        "INSERT FX TO DEVICE",
        "INSERT FX FROM DEVICE",
        "MASTER OUT",
        "CONTROL ROOM OUT",
    ] {
        assert!(titles.contains(&t), "{t}");
    }
    for (i, a) in spec.back.iter().enumerate() {
        for b in &spec.back[i + 1..] {
            let (w, h) = (
                a.rect.x1.min(b.rect.x1) - a.rect.x0.max(b.rect.x0),
                a.rect.y1.min(b.rect.y1) - a.rect.y0.max(b.rect.y0),
            );
            assert!(w <= 1e-9 || h <= 1e-9, "jack cells overlap");
        }
    }
}

#[test]
fn only_drawn_sockets_exist_on_the_back_of_the_view() {
    let reg = registry();
    let d = sectioned(&reg);
    let mut v = tall(d.doc(), &reg);
    v.set_flip(Flip::Back);
    for k in [
        "fx_send3_l",
        "return8_r",
        "insert_send_l",
        "main_r",
        "monitor_l",
    ] {
        assert!(v.socket_center(&sref("master", k)).is_some(), "{k}");
    }
    for k in ["bus_l", "send_bus1_l", "key_in", "gr_cv_out", "fader_cv_in"] {
        assert!(v.socket_center(&sref("master", k)).is_none(), "{k}");
    }
}

#[test]
fn the_send_rows_name_the_effects_and_edit_selects_one() {
    let reg = registry();
    let d = sectioned(&reg);
    let doc = d.doc();
    let mut v = tall(doc, &reg);
    let m = key("master");
    let effect = doc.console.returns[2].clone().unwrap();
    let at = v.return_edit_center(&m, 2).expect("an EDIT button");
    let out = click(&mut v, at, doc, &reg);
    assert!(
        out.iter()
            .any(|a| matches!(a, UiAction::Select(k) if *k == effect)),
        "{out:?}"
    );
    assert_eq!(v.selected(), Some(&effect));
    // A free slot has an EDIT button that selects nothing.
    let at = v.return_edit_center(&m, 6).unwrap();
    let out = click(&mut v, at, doc, &reg);
    assert!(
        !out.iter().any(|a| matches!(a, UiAction::Select(_))),
        "{out:?}"
    );
    let texts = texts_of(&mut v, doc, &reg);
    assert!(texts.iter().any(|t| t == "free"));
    assert!(texts.iter().any(|t| t == "Stereo Delay"), "{texts:?}");
}

#[test]
fn a_return_slider_drives_the_return_level_param() {
    let reg = registry();
    let d = sectioned(&reg);
    let doc = d.doc();
    let mut v = tall(doc, &reg);
    let at = v.control_center(&key("master"), "return2.level").unwrap();
    let _ = v.handle(&down(at), doc, &reg);
    let out = v.handle(&mv(at - atomik_ui::kurbo::Vec2::new(40.0, 0.0)), doc, &reg);
    let moved: Vec<_> = commands(&out)
        .into_iter()
        .filter_map(|c| match c {
            Command::SetParam {
                key, param, value, ..
            } => Some((key.0, param, value)),
            _ => None,
        })
        .collect();
    assert_eq!(moved.len(), 1, "{out:?}");
    assert_eq!(
        (moved[0].0.as_str(), moved[0].1.as_str()),
        ("master", "return2.level")
    );
    assert!(
        moved[0].2 < 0.0,
        "dragging left lowers the level: {}",
        moved[0].2
    );
}

#[test]
fn the_insert_buttons_are_the_mix_panels() {
    let reg = registry();
    let d = sectioned(&reg);
    let doc = d.doc();
    let mut v = tall(doc, &reg);
    let m = key("master");
    // BYPASS INSERT FX bypasses every master insert in one batch.
    let at = v
        .mix_button_center(&m, atomik_ui::mix_panel::MixButton::BypassInserts)
        .unwrap();
    let out = click(&mut v, at, doc, &reg);
    match &commands(&out)[..] {
        [Command::Batch(list)] => assert_eq!(list.len(), 2),
        other => panic!("{other:?}"),
    }
    // SHOW INSERT FX hides the master's inserts, not the send groups.
    let at = v
        .mix_button_center(&m, atomik_ui::mix_panel::MixButton::ShowInserts)
        .unwrap();
    click(&mut v, at, doc, &reg);
    v.sync(doc, &reg);
    let areas = v.insert_areas();
    assert!(!areas.iter().find(|a| a.owner == m).unwrap().shown);
    assert_eq!(v.send_groups().len(), 3);
    assert!(v.device_rect(&insert_chain(doc, &reg, &m)[0]).is_none());
}

// ------------------------------------------------------------------ stack

#[test]
fn inserts_then_send_groups_are_stacked_under_the_master_in_order() {
    let reg = registry();
    let d = sectioned(&reg);
    let doc = d.doc();
    let v = tall(doc, &reg);
    let m = key("master");
    let inserts = insert_chain(doc, &reg, &m);
    assert_eq!(inserts.len(), 2);
    let master = rect(&v, &m);
    assert_eq!(rect(&v, &inserts[0]).y0, master.y1);
    assert_eq!(rect(&v, &inserts[1]).y0, rect(&v, &inserts[0]).y1);
    let groups = v.send_groups();
    assert_eq!(groups.iter().map(|g| g.slot).collect::<Vec<_>>(), [0, 1, 2]);
    assert_eq!(groups[1].members.len(), 2, "two effects in series");
    let area = v.insert_areas().into_iter().find(|a| a.owner == m).unwrap();
    assert!(groups[0].header.y0 >= area.row.y1 - 1e-9);
    let mut last = groups[0].header.y0;
    for g in &groups {
        assert!(g.header.y0 >= last - 1e-9);
        let first = rect(&v, &g.members[0]);
        assert_eq!(
            first.y0, g.header.y1,
            "the header is right above the effect"
        );
        last = rect(&v, g.members.last().unwrap()).y1;
    }
    let mut vv = tall(doc, &reg);
    let texts = texts_of(&mut vv, doc, &reg);
    for n in 1..=3 {
        assert!(texts.iter().any(|t| *t == format!("Send {n}")), "Send {n}");
    }
}

#[test]
fn a_lone_send_group_sits_under_the_empty_master_area() {
    let reg = registry();
    let mut d = two_instruments(&reg);
    run(
        &mut d,
        &reg,
        [create(CreateTarget::Master, "atomik.reverb")],
    );
    let v = tall(d.doc(), &reg);
    let g = v.send_groups();
    assert_eq!(g.len(), 1);
    assert_eq!(g[0].slot, 0);
}

#[test]
fn a_device_dragged_into_the_masters_area_becomes_an_insert() {
    let reg = registry();
    let mut d = sectioned(&reg);
    run(&mut d, &reg, [add_end("atomik.eq", "loose")]);
    let doc = d.doc();
    let mut v = tall(doc, &reg);
    let a = v.plate_center(&key("loose")).unwrap();
    let b = v.insert_row_center(&key("master")).unwrap();
    let out = drag(&mut v, doc, &reg, a, b);
    match &commands(&out)[..] {
        [
            Command::Console(ConsoleCommand::AddInsert {
                channel,
                slot,
                effect,
            }),
        ] => {
            assert_eq!(*channel, key("master"));
            assert_eq!(*slot, 2);
            assert_eq!(*effect, InsertEffect::Existing(key("loose")));
        }
        other => panic!("{other:?} {:?}", texts_of(&mut v, doc, &reg)),
    }
}

#[test]
fn a_send_effect_dragged_into_the_masters_area_is_refused_with_the_reason() {
    let reg = registry();
    let d = sectioned(&reg);
    let doc = d.doc();
    let mut v = tall(doc, &reg);
    let send = send_chain(doc, &reg, 0)[0].clone();
    let a = v.plate_center(&send).unwrap();
    let b = v.insert_row_center(&key("master")).unwrap();
    let out = drag(&mut v, doc, &reg, a, b);
    assert!(commands(&out).is_empty(), "{out:?}");
    let texts = texts_of(&mut v, doc, &reg);
    assert!(texts.iter().any(|t| t.contains("return")), "{texts:?}");
}

#[test]
fn an_insert_dragged_down_the_masters_area_reorders_the_chain() {
    let reg = registry();
    let d = sectioned(&reg);
    let doc = d.doc();
    let mut v = tall(doc, &reg);
    let m = key("master");
    let inserts = insert_chain(doc, &reg, &m);
    let a = v.plate_center(&inserts[0]).unwrap();
    let b = v.insert_row_center(&m).unwrap();
    let out = drag(&mut v, doc, &reg, a, b);
    assert_eq!(
        commands(&out),
        [Command::Console(ConsoleCommand::MoveInsert {
            channel: m,
            from: 0,
            to: 1
        })]
    );
}

#[test]
fn a_click_on_the_masters_drop_row_makes_the_next_effect_an_insert() {
    let reg = registry();
    let d = sectioned(&reg);
    let doc = d.doc();
    let mut v = tall(doc, &reg);
    let m = key("master");
    assert_eq!(v.effect_create_target(), CreateTarget::Nothing);
    let at = v.insert_row_center(&m).unwrap();
    click(&mut v, at, doc, &reg);
    assert_eq!(v.selected(), Some(&m));
    assert_eq!(v.effect_create_target(), CreateTarget::MasterInserts);
    // Selecting the master again from elsewhere is a send.
    v.set_selected(Some(m.clone()));
    assert_eq!(v.effect_create_target(), CreateTarget::Selected(m));
}

#[test]
fn the_master_panel_matches_its_goldens() {
    let reg = registry();
    let d = sectioned(&reg);
    let doc = d.doc();
    for (name, flip) in [("front", Flip::Front), ("back", Flip::Back)] {
        let mut v = tall(doc, &reg);
        v.set_flip(flip);
        check_golden(
            &format!("master_section_{name}.json"),
            &paint_json(&mut v, doc, &reg),
        );
    }
}

/// Writes `target/preview/master_section_preview.png`: the master section, front and back.
/// Run: `cargo test -p atomik-ui --test master_section -- --ignored`.
#[test]
#[ignore = "writes a preview image; run explicitly"]
fn master_section_preview_png() {
    use atomik_ui::kurbo::Affine;
    use atomik_ui::{Canvas, TextEngine};
    let reg = registry();
    let d = sectioned(&reg);
    let doc = d.doc();
    let scale = 1.2;
    let size = Size::new(900.0, 1300.0);
    let mut text = TextEngine::new();
    let mut canvas = support::cpu::CpuCanvas::new(
        (size.width * 2.0 * scale) as u16,
        (size.height * scale) as u16,
        &mut text,
    );
    for (i, flip) in [Flip::Front, Flip::Back].into_iter().enumerate() {
        let mut v = RackView::new(Theme::dark());
        let _ = v.handle(&UiEvent::Resize(size), doc, &reg);
        let _ = v.handle(&UiEvent::ScaleChanged(scale), doc, &reg);
        v.sync(doc, &reg);
        v.set_flip(flip);
        canvas.push_transform(Affine::translate((size.width * scale * i as f64, 0.0)));
        v.paint(&mut canvas, doc, &reg);
        canvas.pop_transform();
    }
    let png = canvas.finish().into_png().expect("png");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/preview/master_section_preview.png");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, png).unwrap();
}
