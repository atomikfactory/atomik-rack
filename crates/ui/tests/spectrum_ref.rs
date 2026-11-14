//! The second curve of a Spectrum device: the REF button in the corner of its plot, the list it
//! opens (Off, the strips, the reference track), the action a row answers, the curve it draws and
//! its name, in English and in French.
//! Regenerate goldens with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test spectrum_ref`.

mod support;

use atomik_ui::analyser::AnalyserData;
use atomik_ui::fx::FxKind;
use atomik_ui::{CurveSource, DisplayList, Key, Lang, Modifiers, RackView, UiAction, UiEvent};
use std::path::PathBuf;
use support::fx::*;
use support::*;

fn sources() -> Vec<CurveSource> {
    vec![
        CurveSource::Off,
        CurveSource::Strip("Drums".to_owned()),
        CurveSource::Strip("Bass".to_owned()),
        CurveSource::Reference,
    ]
}

/// Trace and a second curve that dips where the trace peaks.
fn with_second(source: usize) -> AnalyserData {
    let mut d = spectrum_data(false);
    d.second_sources = sources();
    d.second_source = source;
    if source > 0 {
        d.second = (0..240)
            .map(|i| -70.0 + 30.0 * (-(((i as f32 / 240.0) - 0.4) * 12.0).powi(2)).exp())
            .collect();
    }
    d
}

fn rack_with(data: &AnalyserData) -> (atomik_model::RackDoc, RackView) {
    let (doc, mut v) = rack(FxKind::Spectrum);
    v.set_device_meter_values(&key("fx"), &raw_meters(FxKind::Spectrum));
    v.set_analyser(&key("fx"), data);
    (doc, v)
}

fn paint(doc: &atomik_model::RackDoc, v: &mut RackView) -> DisplayList {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl
}

fn click(
    doc: &atomik_model::RackDoc,
    v: &mut RackView,
    p: atomik_ui::kurbo::Point,
) -> Vec<UiAction> {
    let reg = registry();
    let mut out = v.handle(&mv(p), doc, &reg);
    out.extend(v.handle(&down(p), doc, &reg));
    out.extend(v.handle(&up(p), doc, &reg));
    out
}

fn check(name: &str, actual: &str, french: bool) {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    if french {
        path = path.join("fr");
    }
    let path = path.join(name);
    if std::env::var_os("ATOMIK_UPDATE_GOLDEN").is_some_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| {
            panic!(
                "missing golden {}; run with ATOMIK_UPDATE_GOLDEN=1",
                path.display()
            )
        })
        .replace("\r\n", "\n");
    assert!(expected == actual, "{name} differs from its golden");
}

#[test]
fn the_ref_button_opens_a_list_of_the_sources_and_a_row_picks_one() {
    let (doc, mut v) = rack_with(&with_second(0));
    let _ = paint(&doc, &mut v);
    let button = v
        .spectrum_ref_button_center(&key("fx"))
        .expect("a REF button");
    assert!(
        click(&doc, &mut v, button)
            .iter()
            .all(|a| !matches!(a, UiAction::SpectrumCurve { .. }))
    );
    assert_eq!(
        v.spectrum_ref_labels(),
        ["Off", "Drums", "Bass", "Reference track"]
    );
    let row = v.spectrum_ref_row_center(2).unwrap();
    let picked: Vec<UiAction> = click(&doc, &mut v, row)
        .into_iter()
        .filter(|a| matches!(a, UiAction::SpectrumCurve { .. }))
        .collect();
    assert_eq!(
        picked,
        [UiAction::SpectrumCurve {
            device: key("fx"),
            index: 2
        }]
    );
    assert!(v.spectrum_ref_labels().is_empty(), "the list closed");
    // A second press on the button closes the list again; Escape closes it too.
    click(&doc, &mut v, button);
    assert!(!v.spectrum_ref_labels().is_empty());
    click(&doc, &mut v, button);
    assert!(v.spectrum_ref_labels().is_empty());
    click(&doc, &mut v, button);
    let esc = UiEvent::Key {
        key: Key::Escape,
        modifiers: Modifiers::NONE,
        pressed: true,
    };
    let _ = v.handle(&esc, &doc, &registry());
    assert!(v.spectrum_ref_labels().is_empty());
}

#[test]
fn a_press_outside_the_list_closes_it_and_picks_nothing() {
    let (doc, mut v) = rack_with(&with_second(1));
    let _ = paint(&doc, &mut v);
    let button = v.spectrum_ref_button_center(&key("fx")).unwrap();
    click(&doc, &mut v, button);
    assert_eq!(v.spectrum_ref_labels().len(), 4);
    let away = atomik_ui::kurbo::Point::new(4.0, 4.0);
    let acts = click(&doc, &mut v, away);
    assert!(
        acts.iter()
            .all(|a| !matches!(a, UiAction::SpectrumCurve { .. }))
    );
    assert!(v.spectrum_ref_labels().is_empty());
}

#[test]
fn a_second_curve_is_drawn_and_named_and_none_changes_the_picture_back() {
    let (doc, mut v) = rack_with(&with_second(0));
    let plain = paint(&doc, &mut v).to_json();
    v.set_analyser(&key("fx"), &with_second(1));
    let with = paint(&doc, &mut v);
    assert_ne!(with.to_json(), plain, "the curve is drawn");
    assert!(with.texts().contains(&"Drums"), "its source is named");
    assert!(with.texts().contains(&"REF"));
    v.set_analyser(&key("fx"), &with_second(0));
    assert_eq!(paint(&doc, &mut v).to_json(), plain);
    // The spectrogram has no level axis for it: the curve and the name are not drawn.
    let mut gram = spectrum_data(false);
    gram.spectrogram_cols = 4;
    gram.spectrogram_rows = 2;
    gram.spectrogram = vec![3; 8];
    gram.second_sources = sources();
    gram.second_source = 1;
    gram.second = with_second(1).second;
    let (mut doc2, mut v2) = rack_with(&gram);
    doc2.devices[0].params.insert("mode".into(), 2.0);
    v2.sync(&doc2, &registry());
    let t = paint(&doc2, &mut v2);
    assert!(!t.texts().contains(&"REF"), "no button over a spectrogram");
}

#[test]
fn the_device_with_a_second_curve_and_its_list_are_goldens_in_english_and_french() {
    let build = || {
        let (doc, mut v) = rack_with(&with_second(1));
        let _ = paint(&doc, &mut v);
        let b = v.spectrum_ref_button_center(&key("fx")).unwrap();
        click(&doc, &mut v, b);
        let dl = paint(&doc, &mut v);
        (dl, v.spectrum_ref_labels())
    };
    let (english, labels) = build();
    check(
        "analyser_spectrum_second_curve_1x.json",
        &english.to_json(),
        false,
    );
    assert_eq!(labels.len(), 4);
    let (french, _) = atomik_ui::i18n::with_language(Lang::French, build);
    check(
        "analyser_spectrum_second_curve_1x.json",
        &french.to_json(),
        true,
    );
    assert!(french.texts().contains(&"Piste de r\u{e9}f\u{e9}rence"));
}
