//! The audio track panel's tuner: the layout, the TUNER button, the texts of every state, the
//! needle maths and the goldens of the display. Regenerate goldens with
//! `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test tuner_panel`.

mod support;

use atomik_dsp::tuner::{Note, Reading};
use atomik_ui::analyser::AnalyserData;
use atomik_ui::autolayout::auto_layout;
use atomik_ui::fx::FxKind;
use atomik_ui::fx_layout::{audio_track_layout, fx_layout};
use atomik_ui::paint::{name_rect, plate_rect, tuner_button_rect};
use atomik_ui::tuner_panel::{self, geometry, needle_x};
use atomik_ui::{DisplayList, Flip, RackView, UiAction};
use std::path::PathBuf;
use support::fx::*;
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
        "{name} differs from its golden (regenerate with {UPDATE_ENV}=1)"
    );
}

fn reading(key: i32, cents: f32, hz: f32, held: bool) -> Reading {
    Reading {
        hz,
        note: Note { key, cents },
        held,
    }
}

fn shown(r: Option<Reading>) -> AnalyserData {
    AnalyserData {
        tuner_on: true,
        tuner: r,
        ..AnalyserData::default()
    }
}

fn paint(doc: &atomik_model::RackDoc, v: &mut RackView) -> DisplayList {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl
}

fn texts_of(data: &AnalyserData) -> Vec<String> {
    let (doc, mut v) = rack(FxKind::AudioTrack);
    v.set_analyser(&key("fx"), data);
    paint(&doc, &mut v)
        .texts()
        .into_iter()
        .map(str::to_string)
        .collect()
}

fn has(t: &[String], s: &str) -> bool {
    t.iter().any(|x| x == s)
}

#[test]
fn the_audio_track_is_an_fx_kind_with_a_tuner_panel() {
    assert_eq!(FxKind::of("atomik.audio-track"), Some(FxKind::AudioTrack));
    assert_eq!(FxKind::AudioTrack.meter_count(), 0);
    assert!(!FxKind::EFFECTS.contains(&FxKind::AudioTrack));
    let d = registry().get("atomik.audio-track").unwrap().descriptor;
    let spec = fx_layout(FxKind::AudioTrack, d);
    let p = spec.fx.expect("a visual");
    assert_eq!(p.kind, FxKind::AudioTrack);
    // The TUNER button is on the name plate, not in the display.
    assert!(p.button.is_none());
    assert!(p.visual.contains_rect(p.plot));
    assert!(
        p.plot.width() > 300.0 && p.plot.height() > 60.0,
        "{:?}",
        p.plot
    );
    // Every visible param is placed once; the app's `internal.*` one is not.
    let mut placed: Vec<&str> = spec.front.iter().map(|c| d.params[c.param].key).collect();
    placed.sort_unstable();
    assert_eq!(
        placed,
        [
            "monitor",
            "rec_clip_safe",
            "rec_gain",
            "tuner_mute",
            "tuner_ref"
        ]
    );
    // Every socket is on the back, as in the generic layout.
    assert_eq!(spec.back.len(), auto_layout(d).back.len());
    assert!(!spec.front.is_empty() && spec.height_units >= 2);
}

#[test]
fn a_hidden_tuner_collapses_the_display_and_its_settings() {
    let d = registry().get("atomik.audio-track").unwrap().descriptor;
    let shown = audio_track_layout(d, true);
    let hidden = audio_track_layout(d, false);
    assert!(shown.fx.is_some() && hidden.fx.is_none());
    assert!(
        hidden.height_units < shown.height_units,
        "the panel is shorter"
    );
    let keys = |s: &atomik_ui::autolayout::PanelSpec| {
        let mut k: Vec<&str> = s.front.iter().map(|c| d.params[c.param].key).collect();
        k.sort_unstable();
        k
    };
    assert_eq!(keys(&hidden), ["monitor", "rec_clip_safe", "rec_gain"]);
    assert_eq!(keys(&shown), keys(&fx_layout(FxKind::AudioTrack, d)));
    // The back is the same whichever way the front is.
    assert_eq!(hidden.back.len(), shown.back.len());
    assert!(hidden.front_sections.iter().all(|s| s.label != "TUNER"));
}

#[test]
fn the_scale_geometry_fits_the_display() {
    let d = registry().get("atomik.audio-track").unwrap().descriptor;
    let p = fx_layout(FxKind::AudioTrack, d).fx.unwrap();
    let g = geometry(p.plot);
    for r in [g.note, g.scale, g.readout] {
        assert!(p.plot.contains_rect(r), "{r:?} in {:?}", p.plot);
    }
    assert!(g.scale.width() > 150.0, "{:?}", g.scale);
    assert!(g.note.x1 < g.scale.x0 && g.scale.y1 < g.readout.y1);
    assert!(p.plot.contains(g.lamp));
}

#[test]
fn the_needle_runs_from_minus_fifty_to_plus_fifty_cents() {
    let s = vello::kurbo::Rect::new(100.0, 0.0, 300.0, 20.0);
    assert_eq!(needle_x(s, 0.0), 200.0);
    assert_eq!(needle_x(s, -50.0), 100.0);
    assert_eq!(needle_x(s, 50.0), 300.0);
    assert_eq!(needle_x(s, 25.0), 250.0);
    // Past the ends it stays on the scale.
    assert_eq!(needle_x(s, 80.0), 300.0);
    assert_eq!(needle_x(s, -80.0), 100.0);
}

#[test]
fn the_texts_of_a_reading() {
    assert_eq!(tuner_panel::note_text(40), "E2");
    assert_eq!(tuner_panel::note_text(61), "C#4");
    assert_eq!(tuner_panel::note_text(23), "B0");
    assert_eq!(tuner_panel::hz_text(82.407), "82.41 Hz");
    assert_eq!(tuner_panel::hz_text(1318.51), "1318.5 Hz");
    assert_eq!(tuner_panel::cents_text(3.24), "+3.2 ct");
    assert_eq!(tuner_panel::cents_text(-12.0), "-12.0 ct");
    assert_eq!(tuner_panel::cents_text(0.01), "0.0 ct");
    assert_eq!(tuner_panel::cents_text(-0.01), "0.0 ct");
    assert_eq!(tuner_panel::reference_text(440.0), "A4 = 440 Hz");
    assert_eq!(tuner_panel::reference_text(432.0), "A4 = 432 Hz");
}

#[test]
fn the_status_follows_the_state() {
    use tuner_panel::status_text;
    assert_eq!(status_text(false, None), "TUNER OFF");
    assert_eq!(
        status_text(false, Some(&reading(40, 0.0, 82.41, false))),
        "TUNER OFF"
    );
    assert_eq!(status_text(true, None), "PLAY A NOTE");
    assert_eq!(
        status_text(true, Some(&reading(40, 1.5, 82.5, false))),
        "IN TUNE"
    );
    assert_eq!(
        status_text(true, Some(&reading(40, -3.0, 82.2, false))),
        "IN TUNE"
    );
    assert_eq!(
        status_text(true, Some(&reading(40, -3.1, 82.2, false))),
        "FLAT"
    );
    assert_eq!(
        status_text(true, Some(&reading(40, 12.0, 83.0, false))),
        "SHARP"
    );
    assert_eq!(
        status_text(true, Some(&reading(40, 0.0, 82.4, true))),
        "HOLD"
    );
}

#[test]
fn the_needle_is_green_amber_or_red() {
    let g = tuner_panel::needle_color(0.0);
    assert_eq!(tuner_panel::needle_color(2.9), g);
    assert_eq!(tuner_panel::needle_color(-3.0), g);
    let amber = tuner_panel::needle_color(8.0);
    assert_ne!(amber, g);
    assert_eq!(tuner_panel::needle_color(-15.0), amber);
    let red = tuner_panel::needle_color(30.0);
    assert!(red != g && red != amber);
    assert_eq!(tuner_panel::needle_color(-49.0), red);
}

#[test]
fn a_hidden_tuner_shows_only_the_button() {
    let t = texts_of(&AnalyserData::default());
    assert!(has(&t, "TUNER"), "the button: {t:?}");
    for s in [
        "TUNER OFF",
        "PLAY A NOTE",
        "Press TUNER to tune the input",
        "A4 = 440 Hz",
        "A4 Hz",
        "Mute",
    ] {
        assert!(!has(&t, s), "{s:?}: {t:?}");
    }
}

#[test]
fn a_shown_tuner_without_a_note_asks_for_one() {
    let t = texts_of(&shown(None));
    for s in ["PLAY A NOTE", "--", "-- Hz", "A4 = 440 Hz"] {
        assert!(has(&t, s), "{s:?}: {t:?}");
    }
}

#[test]
fn a_reading_shows_the_note_the_frequency_and_the_cents() {
    let t = texts_of(&shown(Some(reading(40, 3.24, 82.57, false))));
    for s in ["E", "2", "82.57 Hz", "+3.2 ct", "SHARP", "-50", "0", "+50"] {
        assert!(has(&t, s), "{s:?}: {t:?}");
    }
    let t = texts_of(&shown(Some(reading(61, -1.0, 277.0, false))));
    for s in ["C#", "4", "IN TUNE", "-1.0 ct"] {
        assert!(has(&t, s), "{s:?}: {t:?}");
    }
    let t = texts_of(&shown(Some(reading(69, -20.0, 434.9, true))));
    assert!(has(&t, "HOLD") && has(&t, "A"), "{t:?}");
    // Hidden: the stored reading is not shown.
    let mut hidden = shown(Some(reading(69, 0.0, 440.0, false)));
    hidden.tuner_on = false;
    let t = texts_of(&hidden);
    assert!(!has(&t, "IN TUNE") && !has(&t, "E"), "{t:?}");
}

#[test]
fn the_reference_pitch_is_the_tracks_param() {
    let (mut doc, mut v) = rack(FxKind::AudioTrack);
    doc.devices[0].params.insert("tuner_ref".into(), 432.0);
    v.set_analyser(&key("fx"), &shown(None));
    v.sync(&doc, &registry());
    let t: Vec<String> = paint(&doc, &mut v)
        .texts()
        .into_iter()
        .map(str::to_string)
        .collect();
    assert!(has(&t, "A4 = 432 Hz"), "{t:?}");
}

#[test]
fn a_shifted_reference_from_the_app_replaces_the_tracks_own_in_the_readout() {
    let (mut doc, mut v) = rack(FxKind::AudioTrack);
    doc.devices[0].params.insert("tuner_ref".into(), 432.0);
    let mut data = shown(None);
    data.tuner_ref_hz = 432.0 * 2f32.powf(100.0 / 1200.0);
    v.set_analyser(&key("fx"), &data);
    v.sync(&doc, &registry());
    let t: Vec<String> = paint(&doc, &mut v)
        .texts()
        .into_iter()
        .map(str::to_string)
        .collect();
    assert!(has(&t, "A4 = 458 Hz"), "{t:?}");
    assert!(!has(&t, "A4 = 432 Hz"), "{t:?}");
}

#[test]
fn the_button_sits_on_the_plate_under_the_name() {
    let (_, v) = rack(FxKind::AudioTrack);
    let b = v.tuner_button_center(&key("fx")).expect("the button");
    let dev = v.device_rect(&key("fx")).unwrap();
    let want = v.rack_to_view(tuner_button_rect().center() + dev.origin().to_vec2());
    assert_eq!(b, want);
    let plate = plate_rect(dev.height(), false);
    let b = b - dev.origin().to_vec2();
    let name = name_rect(dev.height(), false, true);
    assert!(
        plate.contains(b) && b.x < atomik_ui::autolayout::CONTENT_X.0,
        "{b:?} {plate:?}"
    );
    assert!(b.y > name.y0 + 16.0, "under the name: {b:?} {name:?}");
}

#[test]
fn the_tuner_is_off_and_the_panel_short_until_the_button_is_pressed() {
    let (doc, mut v) = rack(FxKind::AudioTrack);
    let reg = registry();
    let off = v.device_rect(&key("fx")).unwrap().height();
    assert!(!v.analyser_data(&key("fx")).unwrap().tuner_on);
    let b = v.tuner_button_center(&key("fx")).unwrap();
    let _ = v.handle(&down(b), &doc, &reg);
    let _ = v.handle(&up(b), &doc, &reg);
    let on = v.device_rect(&key("fx")).unwrap().height();
    assert!(on > off, "the display opens: {off} then {on}");
    // The button does not move when the panel grows: a second click toggles it back.
    assert_eq!(v.tuner_button_center(&key("fx")), Some(b));
    let t: Vec<String> = paint(&doc, &mut v)
        .texts()
        .into_iter()
        .map(str::to_string)
        .collect();
    assert!(has(&t, "PLAY A NOTE") && has(&t, "A4 Hz"), "{t:?}");
    let _ = v.handle(&down(b), &doc, &reg);
    let _ = v.handle(&up(b), &doc, &reg);
    assert_eq!(v.device_rect(&key("fx")).unwrap().height(), off);
}

#[test]
fn the_tuner_button_toggles_and_reports() {
    let (doc, mut v) = rack(FxKind::AudioTrack);
    let reg = registry();
    let b = v.tuner_button_center(&key("fx")).expect("the button");
    let acts = v.handle(&down(b), &doc, &reg);
    let _ = v.handle(&up(b), &doc, &reg);
    assert!(
        acts.contains(&UiAction::SetTuner {
            device: key("fx"),
            on: true
        }),
        "{acts:?}"
    );
    assert!(v.analyser_data(&key("fx")).unwrap().tuner_on);
    assert!(has(
        &paint(&doc, &mut v)
            .texts()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>(),
        "PLAY A NOTE"
    ));
    let acts = v.handle(&down(b), &doc, &reg);
    let _ = v.handle(&up(b), &doc, &reg);
    assert!(
        acts.contains(&UiAction::SetTuner {
            device: key("fx"),
            on: false
        }),
        "{acts:?}"
    );
    assert!(!v.analyser_data(&key("fx")).unwrap().tuner_on);
    // Hiding drops the note on the display.
    v.set_analyser(&key("fx"), &shown(Some(reading(40, 0.0, 82.41, false))));
    let acts = v.handle(&down(b), &doc, &reg);
    let _ = v.handle(&up(b), &doc, &reg);
    assert!(acts.contains(&UiAction::SetTuner {
        device: key("fx"),
        on: false
    }));
    assert_eq!(v.analyser_data(&key("fx")).unwrap().tuner, None);
    // The back face has no button.
    v.set_flip(Flip::Back);
    assert!(v.tuner_button_center(&key("fx")).is_none());
}

#[test]
fn the_feed_repaints_only_on_news() {
    let (doc, mut v) = rack(FxKind::AudioTrack);
    let data = shown(Some(reading(40, 1.0, 82.5, false)));
    v.set_analyser(&key("fx"), &data);
    let _ = paint(&doc, &mut v);
    v.set_analyser(&key("fx"), &data);
    assert!(!v.needs_repaint(), "no news, no repaint");
    let mut other = data.clone();
    other.tuner = Some(reading(40, 2.0, 82.6, false));
    v.set_analyser(&key("fx"), &other);
    assert!(v.needs_repaint());
}

#[test]
fn other_devices_ignore_the_tuner_feed() {
    let (_, mut v) = rack(FxKind::Compressor);
    v.set_analyser(&key("fx"), &shown(None));
    assert!(v.analyser_data(&key("fx")).is_none());
}

#[test]
fn the_button_has_a_tooltip() {
    let (_, v) = rack(FxKind::AudioTrack);
    let help = v
        .help_at(v.tuner_button_center(&key("fx")).unwrap())
        .expect("a tooltip");
    assert!(help.text.contains("tuner"), "{}", help.text);
}

fn record(data: &AnalyserData, flip: Flip) -> String {
    let (doc, mut v) = rack(FxKind::AudioTrack);
    v.set_analyser(&key("fx"), data);
    v.set_flip(flip);
    paint(&doc, &mut v).to_json()
}

#[test]
fn goldens_of_the_tuner_states() {
    check(
        "tuner_off_1x.json",
        &record(&AnalyserData::default(), Flip::Front),
    );
    check("tuner_idle_1x.json", &record(&shown(None), Flip::Front));
    check(
        "tuner_flat_1x.json",
        &record(&shown(Some(reading(40, -18.4, 81.5, false))), Flip::Front),
    );
    check(
        "tuner_in_tune_1x.json",
        &record(&shown(Some(reading(45, 0.8, 110.05, false))), Flip::Front),
    );
    check(
        "tuner_held_1x.json",
        &record(&shown(Some(reading(52, 6.0, 165.5, true))), Flip::Front),
    );
    check(
        "tuner_back_1x.json",
        &record(&AnalyserData::default(), Flip::Back),
    );
}
