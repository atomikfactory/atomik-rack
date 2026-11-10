//! The Preset row of the audio export panel: the drop-down fills the fields, a touched field
//! marks the preset "(changed)", a preset of stems turns the panel into a stems export, presets
//! that need FFmpeg are disabled without it, and a name typed for "Save Preset..." is handed to
//! the app. The mix-down, loudness and extra-file rows set their values.

mod support;

use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{
    DisplayList, EncoderState, ExportAction, ExportDepth, ExportDither, ExportFile, ExportLoudness,
    ExportQuality, ExportScope, ExportValues, ExportView, ImeEvent, Key, Modifiers, PresetChoice,
    Theme, UiAction, UiEvent,
};
use support::*;

const BOUNDS: Rect = Rect::new(0.0, 0.0, 900.0, 600.0);

fn streaming() -> ExportValues {
    ExportValues {
        loudness: ExportLoudness::Target(-14),
        also_mp3: true,
        ..ExportValues::default()
    }
}

fn podcast() -> ExportValues {
    ExportValues {
        file: ExportFile::Mp3,
        mp3: ExportQuality::Cbr(128),
        mono: true,
        loudness: ExportLoudness::Target(-16),
        ..ExportValues::default()
    }
}

fn stems() -> ExportValues {
    ExportValues {
        dither: ExportDither::Off,
        ..ExportValues::default()
    }
}

fn presets() -> Vec<PresetChoice> {
    let choice = |name: &str, stems: bool, values, user| PresetChoice {
        name: name.to_owned(),
        stems,
        values,
        user,
    };
    vec![
        choice("Streaming", false, streaming(), false),
        choice("Podcast", false, podcast(), false),
        choice("Stems for mixing", true, stems(), false),
        choice("My master", false, ExportValues::default(), true),
    ]
}

fn view(scope: ExportScope) -> ExportView {
    let mut v = ExportView::new(Theme::dark());
    v.set_bounds(BOUNDS);
    v.open(scope, "my song.wav", 48_000, ExportValues::default());
    v.set_presets(presets());
    v
}

fn center(v: &ExportView, label: &str) -> Point {
    v.button_rect(label)
        .unwrap_or_else(|| panic!("no button {label}"))
        .center()
}

fn click(v: &mut ExportView, label: &str) -> Vec<ExportAction> {
    let p = center(v, label);
    click_at(v, p)
}

fn click_at(v: &mut ExportView, p: Point) -> Vec<ExportAction> {
    let mut out = v.handle(&down(p));
    out.extend(v.handle(&up(p)));
    out.into_iter()
        .filter_map(|a| match a {
            UiAction::Export(e) => Some(e),
            _ => None,
        })
        .collect()
}

fn key(v: &mut ExportView, key: Key) -> Vec<ExportAction> {
    v.handle(&UiEvent::Key {
        key,
        modifiers: Modifiers::default(),
        pressed: true,
    })
    .into_iter()
    .filter_map(|a| match a {
        UiAction::Export(e) => Some(e),
        _ => None,
    })
    .collect()
}

fn texts(v: &ExportView) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    dl.texts().into_iter().map(str::to_string).collect()
}

/// Opens the list and clicks the row of the preset called `name`.
fn choose(v: &mut ExportView, name: &str) {
    let open = v.preset_label();
    click(v, &open);
    assert!(v.list_open(), "the list opens");
    let index = v.presets().iter().position(|p| p.name == name).unwrap();
    // The rows sit under the drop-down, 20 px each after 4 px of padding, in a 240 px list.
    let button = v.button_rect(&open).unwrap();
    let p = Point::new(
        button.x0 + 30.0,
        button.y1 + 2.0 + 4.0 + 20.0 * index as f64 + 10.0,
    );
    click_at(v, p);
}

#[test]
fn the_panel_shows_the_preset_row_and_its_buttons() {
    let v = view(ExportScope::Song);
    let t = texts(&v);
    for want in [
        "PRESET",
        "(NONE)",
        "SAVE PRESET...",
        "DELETE PRESET",
        "MIX DOWN",
        "STEREO",
        "MONO",
        "LOUDNESS",
        "OFF",
        "-16 LUFS",
        "-14 LUFS",
        "-9 GUIDE",
        "EXTRA FILE",
        "ONE FILE",
        "MP3 320 kbps",
    ] {
        assert!(t.iter().any(|x| x == want), "{want} in {t:?}");
    }
    // No preset is shown, so there is nothing to delete.
    assert!(v.preset().is_none() && !v.preset_is_user());
}

#[test]
fn the_panel_text_and_geometry_match_the_golden() {
    let mut out = String::new();
    for (name, scope) in [
        ("song", ExportScope::Song),
        ("loop", ExportScope::Loop),
        ("stems", ExportScope::Stems),
    ] {
        let mut v = view(scope);
        out.push_str(&format!("== {name}\n"));
        let p = v.panel();
        out.push_str(&format!(
            "panel {:.1} {:.1} {:.1} {:.1}\n",
            p.x0, p.y0, p.x1, p.y1
        ));
        out.push_str(&texts(&v).join("\n"));
        out.push('\n');
        for label in [
            "(NONE)",
            "SAVE PRESET...",
            "DELETE PRESET",
            "STEREO",
            "-14 LUFS",
            "MP3 320 kbps",
        ] {
            if let Some(r) = v.button_rect(label) {
                out.push_str(&format!(
                    "{label}: {:.1} {:.1} {:.1} {:.1}\n",
                    r.x0, r.y0, r.x1, r.y1
                ));
            }
        }
        choose(&mut v, "Streaming");
        out.push_str(&format!("after Streaming: {}\n", v.preset_label()));
    }
    starter::check_golden("export_presets_panel.txt", &out);
}

#[test]
fn choosing_a_preset_fills_the_fields_and_a_touched_field_marks_it_changed() {
    let mut v = view(ExportScope::Song);
    choose(&mut v, "Podcast");
    assert_eq!(v.preset(), Some("Podcast"));
    assert_eq!(v.values(), podcast());
    assert_eq!(v.preset_label(), "Podcast");
    assert!(!v.preset_changed());
    assert!(!v.list_open());

    click(&mut v, "192 kbps");
    assert!(v.preset_changed());
    assert_eq!(v.preset_label(), "Podcast (changed)");
    // The drop-down shows the marker.
    assert!(texts(&v).iter().any(|t| t == "Podcast (changed)"));

    // Putting the field back removes the marker.
    click(&mut v, "128 kbps");
    assert!(!v.preset_changed());
    assert_eq!(v.preset_label(), "Podcast");

    // The loudness, mix-down and extra file rows count as fields too.
    click(&mut v, "STEREO");
    assert_eq!(v.preset_label(), "Podcast (changed)");
    click(&mut v, "MONO");
    click(&mut v, "-14 LUFS");
    assert_eq!(v.preset_label(), "Podcast (changed)");
    click(&mut v, "-16 LUFS");
    assert_eq!(v.preset_label(), "Podcast");
}

#[test]
fn a_preset_of_stems_makes_a_stems_export_and_a_mix_preset_brings_the_scope_back() {
    let mut v = view(ExportScope::Loop);
    choose(&mut v, "Stems for mixing");
    assert_eq!(v.scope(), ExportScope::Stems);
    assert_eq!(v.values().dither, ExportDither::Off);
    assert!(texts(&v).iter().any(|t| t == "EXPORT STEMS"));
    assert!(v.button_rect("MONO").is_none(), "stems have no mix-down");
    assert!(!v.preset_changed());
    choose(&mut v, "Streaming");
    assert_eq!(v.scope(), ExportScope::Loop);
    assert_eq!(v.values(), streaming());
}

#[test]
fn the_row_buttons_set_the_mix_down_the_loudness_and_the_extra_file() {
    let mut v = view(ExportScope::Song);
    click(&mut v, "MONO");
    click(&mut v, "-14 LUFS");
    click(&mut v, "MP3 320 kbps");
    let want = ExportValues {
        mono: true,
        loudness: ExportLoudness::Target(-14),
        also_mp3: true,
        ..ExportValues::default()
    };
    assert_eq!(v.values(), want);
    click(&mut v, "-9 GUIDE");
    assert_eq!(v.values().loudness, ExportLoudness::Guide(-9));
    click(&mut v, "OFF");
    assert_eq!(v.values().loudness, ExportLoudness::Off);
    click(&mut v, "ONE FILE");
    assert!(!v.values().also_mp3);
    // EXPORT hands the values on.
    click(&mut v, "MONO");
    let started = click(&mut v, "EXPORT");
    assert!(matches!(started[..], [ExportAction::Start(_)]));
}

#[test]
fn stems_and_a_compressed_file_never_start_with_the_extra_file_or_the_loudness() {
    let mut v = view(ExportScope::Song);
    click(&mut v, "-14 LUFS");
    click(&mut v, "MP3 320 kbps");
    click(&mut v, "MP3");
    // The extra MP3 is for a lossless file: the button is greyed and the value does not start.
    let started = click(&mut v, "EXPORT");
    let [ExportAction::Start(values)] = started[..] else {
        panic!("{started:?}");
    };
    assert!(!values.also_mp3 && values.file == ExportFile::Mp3);

    let mut v = view(ExportScope::Song);
    choose(&mut v, "Streaming");
    choose(&mut v, "Stems for mixing");
    let started = click(&mut v, "EXPORT");
    let [ExportAction::Start(values)] = started[..] else {
        panic!("{started:?}");
    };
    assert_eq!(values.loudness, ExportLoudness::Off);
    assert!(!values.also_mp3 && !values.mono);
}

#[test]
fn without_ffmpeg_the_presets_that_need_it_are_disabled() {
    let mut v = view(ExportScope::Song);
    v.set_encoder(EncoderState::Missing("needs FFmpeg".to_owned()));
    let by_name = |v: &ExportView, name: &str| {
        v.presets()
            .iter()
            .find(|p| p.name == name)
            .cloned()
            .unwrap()
    };
    assert!(
        !v.preset_usable(&by_name(&v, "Streaming")),
        "it writes an MP3"
    );
    assert!(!v.preset_usable(&by_name(&v, "Podcast")));
    assert!(v.preset_usable(&by_name(&v, "Stems for mixing")));
    assert!(v.preset_usable(&by_name(&v, "My master")));
    assert!(!v.choose_preset("Podcast"));
    assert!(v.preset().is_none());
    assert!(v.choose_preset("My master"));

    // The open list says why a row is disabled.
    let open = v.preset_label();
    click(&mut v, &open);
    let t = texts(&v);
    assert!(t.iter().any(|x| x == "Streaming (Needs FFmpeg)"), "{t:?}");
    assert!(t.iter().any(|x| x == "Podcast (Needs FFmpeg)"));
    assert!(t.iter().any(|x| x == "Stems for mixing"));
}

#[test]
fn the_list_closes_on_escape_and_on_a_press_elsewhere() {
    let mut v = view(ExportScope::Song);
    let open = v.preset_label();
    click(&mut v, &open);
    assert!(v.list_open());
    // Escape closes the list, not the panel.
    assert!(key(&mut v, Key::Escape).is_empty());
    assert!(!v.list_open());
    click(&mut v, &open);
    click_at(&mut v, Point::new(5.0, 5.0));
    assert!(!v.list_open());
    // The drop-down toggles.
    click(&mut v, &open);
    click(&mut v, &open);
    assert!(!v.list_open());
}

#[test]
fn only_a_user_preset_can_be_deleted() {
    let mut v = view(ExportScope::Song);
    choose(&mut v, "Streaming");
    assert!(
        click(&mut v, "DELETE PRESET").is_empty(),
        "built-in: greyed"
    );
    choose(&mut v, "My master");
    assert!(v.preset_is_user());
    assert_eq!(
        click(&mut v, "DELETE PRESET"),
        vec![ExportAction::DeletePreset]
    );
}

#[test]
fn save_preset_takes_a_name_and_hands_it_to_the_app() {
    let mut v = view(ExportScope::Song);
    click(&mut v, "-14 LUFS");
    assert!(click(&mut v, "SAVE PRESET...").is_empty());
    assert!(v.is_naming() && v.wants_text());
    assert_eq!(v.preset_name_entry(), Some(""));
    // A blank name cannot be saved.
    assert!(key(&mut v, Key::Enter).is_empty());
    v.handle(&UiEvent::Ime(ImeEvent::Commit("Loud one".to_owned())));
    assert_eq!(v.preset_name_entry(), Some("Loud one"));
    assert_eq!(key(&mut v, Key::Enter), vec![ExportAction::SavePreset]);
    // The app saved it: the panel shows it, unchanged.
    let mut list = presets();
    list.push(PresetChoice {
        name: "Loud one".to_owned(),
        stems: false,
        values: v.values(),
        user: true,
    });
    v.set_presets(list);
    v.preset_saved("Loud one");
    assert!(!v.is_naming());
    assert_eq!(v.preset_label(), "Loud one");
    assert!(v.preset_is_user());
    assert!(!v.preset_changed());
}

#[test]
fn escape_leaves_the_name_entry_not_the_panel() {
    let mut v = view(ExportScope::Song);
    click(&mut v, "SAVE PRESET...");
    assert!(key(&mut v, Key::Escape).is_empty());
    assert!(!v.is_naming());
    // A second Escape is the panel's own: cancel.
    assert_eq!(key(&mut v, Key::Escape), vec![ExportAction::Cancel]);
    click(&mut v, "SAVE PRESET...");
    assert!(click(&mut v, "CANCEL").is_empty());
    assert!(!v.is_naming());
}

#[test]
fn saving_over_a_user_preset_starts_with_its_name_in_the_field() {
    let mut v = view(ExportScope::Song);
    choose(&mut v, "My master");
    click(&mut v, "SAVE PRESET...");
    assert_eq!(v.preset_name_entry(), Some("My master"));
}

#[test]
fn every_preset_button_has_a_short_tooltip() {
    let mut v = view(ExportScope::Song);
    for label in [
        "(NONE)",
        "SAVE PRESET...",
        "DELETE PRESET",
        "STEREO",
        "MONO",
        "OFF",
        "-14 LUFS",
        "-9 GUIDE",
        "ONE FILE",
        "MP3 320 kbps",
    ] {
        let p = center(&v, label);
        let help = v.help_at(p).unwrap_or_else(|| panic!("{label}"));
        assert!(help.text.chars().count() <= 90, "{label}: {}", help.text);
    }
    click(&mut v, "SAVE PRESET...");
    for label in ["SAVE", "CANCEL"] {
        let p = center(&v, label);
        assert!(v.help_at(p).is_some(), "{label}");
    }
}

#[test]
fn the_defaults_keep_the_old_behaviour() {
    let v = ExportValues::default();
    assert!(!v.mono && !v.also_mp3 && v.loudness == ExportLoudness::Off);
    assert_eq!(v.depth, ExportDepth::Pcm24);
}

#[test]
fn the_ten_rows_fit_in_a_small_window() {
    let mut v = view(ExportScope::Song);
    v.set_bounds(Rect::new(0.0, 0.0, 800.0, 520.0));
    let p = v.panel();
    assert!(p.y1 <= 520.0, "{p:?}");
}
