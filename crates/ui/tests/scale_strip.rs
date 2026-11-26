//! The scale strip of the CV Quantizer panel: its name tag cycles through the device's own scale,
//! the song key and the scales saved in the song; NEW and EDIT ask for the scale picker.

mod support;

use atomik_model::{Command, RackDoc};
use atomik_ui::event::{Modifiers, PointerButton};
use atomik_ui::kurbo::Point;
use atomik_ui::scale_strip::QUANTIZER_TYPE;
use atomik_ui::{DisplayList, RackView, UiAction, UiEvent};
use support::*;

fn quantizer(params: &[(&str, f32)]) -> (RackDoc, RackView) {
    let mut doc = reference_doc();
    let mut d = device("q", QUANTIZER_TYPE, "CV Quantizer");
    for (k, v) in params {
        d.params.insert((*k).to_string(), *v);
    }
    doc.devices = vec![d];
    doc.cables.clear();
    let reg = registry();
    let v = view(&doc, &reg);
    (doc, v)
}

fn click(v: &mut RackView, doc: &RackDoc, p: Point) -> Vec<UiAction> {
    let reg = registry();
    let mut out = v.handle(&down(p), doc, &reg);
    out.extend(v.handle(&up(p), doc, &reg));
    out
}

fn right_click(v: &mut RackView, doc: &RackDoc, p: Point) -> Vec<UiAction> {
    let reg = registry();
    let e = UiEvent::PointerDown {
        pos: p,
        button: PointerButton::Secondary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    };
    v.handle(&e, doc, &reg)
}

fn pickers(actions: &[UiAction]) -> Vec<&UiAction> {
    actions
        .iter()
        .filter(|a| matches!(a, UiAction::SongScalePicker { .. }))
        .collect()
}

fn source_params(actions: &[UiAction]) -> Vec<(String, f32)> {
    let cmds = commands(actions);
    let [Command::Batch(items)] = cmds.as_slice() else {
        panic!("one batch: {actions:?}");
    };
    items
        .iter()
        .map(|c| match c {
            Command::SetParam { param, value, .. } => (param.clone(), *value),
            other => panic!("{other:?}"),
        })
        .collect()
}

fn texts(v: &mut RackView, doc: &RackDoc) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl.texts().into_iter().map(str::to_string).collect()
}

fn scales() -> Vec<(u8, &'static str)> {
    vec![(1, "Pent"), (4, "Triad")]
}

#[test]
fn the_tag_names_the_source_and_the_panel_has_no_loose_source_params() {
    let (doc, mut v) = quantizer(&[]);
    let t = texts(&mut v, &doc);
    assert!(t.iter().any(|x| x == "OWN"), "{t:?}");
    for s in ["NEW", "EDIT"] {
        assert!(t.iter().any(|x| x == s), "{s}: {t:?}");
    }
    assert!(
        !t.iter().any(|x| x.to_lowercase().contains("song scale")),
        "{t:?}"
    );
    v.set_song_scales(scales().into_iter());
    let (doc, mut v2) = quantizer(&[("scale.source", 2.0), ("scale.id", 4.0)]);
    v2.set_song_scales(scales().into_iter());
    assert!(texts(&mut v2, &doc).iter().any(|x| x == "Triad"));
    let (doc, mut v3) = quantizer(&[("scale.source", 1.0)]);
    assert!(texts(&mut v3, &doc).iter().any(|x| x == "KEY"));
}

#[test]
fn a_click_on_the_tag_goes_to_the_next_source_and_a_right_click_to_the_one_before() {
    let (doc, mut v) = quantizer(&[]);
    v.set_song_scales(scales().into_iter());
    let tag = v.scale_strip_center(&key("q"), 0).expect("tag");
    assert_eq!(
        source_params(&click(&mut v, &doc, tag)),
        [("scale.source".into(), 1.0), ("scale.id".into(), 0.0)]
    );
    assert_eq!(
        source_params(&right_click(&mut v, &doc, tag)),
        [("scale.source".into(), 2.0), ("scale.id".into(), 4.0)],
        "from OWN backwards: the last song scale"
    );
    let (doc, mut v) = quantizer(&[("scale.source", 1.0)]);
    v.set_song_scales(scales().into_iter());
    let tag = v.scale_strip_center(&key("q"), 0).unwrap();
    assert_eq!(
        source_params(&click(&mut v, &doc, tag)),
        [("scale.source".into(), 2.0), ("scale.id".into(), 1.0)]
    );
}

#[test]
fn new_asks_for_a_picker_on_the_own_notes_and_edit_on_the_chosen_scale() {
    let (doc, mut v) = quantizer(&[]);
    v.set_song_scales(scales().into_iter());
    let new = v.scale_strip_center(&key("q"), 1).unwrap();
    let a = click(&mut v, &doc, new);
    let a = pickers(&a);
    // C major, the device's default scale.
    assert!(
        matches!(
            a.as_slice(),
            [UiAction::SongScalePicker {
                id: None,
                own: 0b1010_1011_0101,
                ..
            }]
        ),
        "{a:?}"
    );
    // EDIT does nothing while the device uses no song scale.
    let edit = v.scale_strip_center(&key("q"), 2).unwrap();
    assert!(pickers(&click(&mut v, &doc, edit)).is_empty());
    let (doc, mut v) = quantizer(&[("scale.source", 2.0), ("scale.id", 4.0)]);
    v.set_song_scales(scales().into_iter());
    let edit = v.scale_strip_center(&key("q"), 2).unwrap();
    let a = click(&mut v, &doc, edit);
    let a = pickers(&a);
    assert!(
        matches!(
            a.as_slice(),
            [UiAction::SongScalePicker { id: Some(4), .. }]
        ),
        "{a:?}"
    );
}

#[test]
fn a_device_with_a_custom_scale_offers_its_toggles_as_the_new_scale() {
    let mut params = vec![("scale.custom", 1.0)];
    for pc in 0..12 {
        params.push((
            Box::leak(format!("scale.note.{pc}").into_boxed_str()) as &str,
            if pc == 1 || pc == 6 { 1.0 } else { 0.0 },
        ));
    }
    let (doc, mut v) = quantizer(&params);
    let new = v.scale_strip_center(&key("q"), 1).unwrap();
    let a = click(&mut v, &doc, new);
    let a = pickers(&a);
    assert!(
        matches!(a.as_slice(), [UiAction::SongScalePicker { id: None, own, .. }] if *own == 0b100_0010),
        "{a:?}"
    );
}
