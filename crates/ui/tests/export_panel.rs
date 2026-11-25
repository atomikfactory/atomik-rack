//! The audio export panel: the options paint and change, the dither row follows the bit depth,
//! the tail row is for songs only, EXPORT and CANCEL emit the right actions with the chosen
//! values, and the progress mode shows the bar and only CANCEL.

mod support;

use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{
    DisplayList, EncoderState, ExportAction, ExportDepth, ExportDither, ExportFile, ExportLoudness,
    ExportQuality, ExportScope, ExportValues, ExportView, ImeEvent, Key, Modifiers, Theme,
    UiAction, UiEvent,
};
use support::*;

const BOUNDS: Rect = Rect::new(0.0, 0.0, 900.0, 600.0);

fn view(scope: ExportScope) -> ExportView {
    let mut v = ExportView::new(Theme::dark());
    v.set_bounds(BOUNDS);
    v.open(scope, "my song.wav", 48_000, ExportValues::default());
    v
}

fn center(v: &ExportView, label: &str) -> Point {
    v.button_rect(label)
        .unwrap_or_else(|| panic!("no button {label}"))
        .center()
}

fn click(v: &mut ExportView, label: &str) -> Vec<ExportAction> {
    let p = center(v, label);
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

fn has(texts: &[String], t: &str) -> bool {
    texts.iter().any(|x| x.contains(t))
}

#[test]
fn the_song_options_paint_every_choice_and_the_file_name() {
    let t = texts(&view(ExportScope::Song));
    for want in [
        "EXPORT SONG AS AUDIO FILE",
        "my song.wav",
        "BIT DEPTH",
        "16-BIT",
        "24-BIT",
        "32-BIT FLOAT",
        "DITHER",
        "TPDF",
        "SAMPLE RATE",
        "PROJECT",
        "44.1 kHz",
        "96 kHz",
        "TAIL",
        "KEEP EFFECT TAIL",
        "EXPORT",
        "CANCEL",
        "Project rate 48 kHz",
    ] {
        assert!(has(&t, want), "{want} in {t:?}");
    }
}

#[test]
fn a_loop_has_no_tail_row() {
    let v = view(ExportScope::Loop);
    let t = texts(&v);
    assert!(has(&t, "EXPORT LOOP AS AUDIO FILE"));
    assert!(!has(&t, "KEEP EFFECT TAIL"));
    assert!(v.button_rect("KEEP EFFECT TAIL").is_none());
    assert!(has(&t, "Exactly from the left to the right locator"));
}

#[test]
fn stems_have_a_folder_a_tail_a_format_and_a_strip_choice() {
    let mut v = view(ExportScope::Stems);
    let t = texts(&v);
    for want in [
        "EXPORT STEMS",
        "FILE FORMAT",
        "WAV",
        "AIFF",
        "TAIL",
        "CHANNELS",
        "AFTER STRIP",
        "DRY SOURCE",
        "One file per strip",
    ] {
        assert!(has(&t, want), "{want} in {t:?}");
    }
    // The strip choice is for stems only; the format is for every scope.
    assert!(view(ExportScope::Song).button_rect("DRY SOURCE").is_none());
    assert!(view(ExportScope::Loop).button_rect("AIFF").is_some());
    assert!(v.button_rect("WHOLE SONG").is_some());
    assert!(view(ExportScope::Song).button_rect("WHOLE SONG").is_none());
    click(&mut v, "AIFF");
    click(&mut v, "DRY SOURCE");
    assert_eq!(v.values().file, atomik_ui::ExportFile::Aiff);
    assert!(v.values().dry);
    click(&mut v, "AFTER STRIP");
    click(&mut v, "WAV");
    assert_eq!(v.values(), ExportValues::default());
    for label in ["AIFF", "DRY SOURCE", "AFTER STRIP", "WAV"] {
        let p = center(&v, label);
        assert!(v.help_at(p).is_some(), "{label}");
    }
    v.show_progress();
    assert!(has(&texts(&v), "EXPORTING STEMS"));
}

#[test]
fn the_buttons_change_the_values() {
    let mut v = view(ExportScope::Song);
    assert_eq!(v.values(), ExportValues::default());
    assert_eq!(v.values().depth, ExportDepth::Pcm24);
    assert!(click(&mut v, "16-BIT").is_empty());
    assert_eq!(v.values().depth, ExportDepth::Pcm16);
    click(&mut v, "SHAPED");
    assert_eq!(v.values().dither, ExportDither::Shaped);
    click(&mut v, "44.1 kHz");
    assert_eq!(v.values().rate, Some(44_100));
    click(&mut v, "PROJECT");
    assert_eq!(v.values().rate, None);
    click(&mut v, "KEEP EFFECT TAIL");
    assert!(!v.values().tail);
    click(&mut v, "32-BIT FLOAT");
    assert_eq!(v.values().depth, ExportDepth::Float32);
}

#[test]
fn the_dither_row_only_works_at_16_bit() {
    let mut v = view(ExportScope::Song);
    // 24-bit (the default): the dither buttons are inert.
    assert_eq!(v.values().dither, ExportDither::Tpdf);
    click(&mut v, "NONE");
    assert_eq!(v.values().dither, ExportDither::Tpdf);
    click(&mut v, "16-BIT");
    click(&mut v, "NONE");
    assert_eq!(v.values().dither, ExportDither::Off);
}

#[test]
fn export_starts_with_the_chosen_values_and_cancel_and_escape_cancel() {
    let mut v = view(ExportScope::Song);
    click(&mut v, "16-BIT");
    click(&mut v, "96 kHz");
    let started = click(&mut v, "EXPORT");
    assert_eq!(
        started,
        vec![ExportAction::Start(ExportValues {
            depth: ExportDepth::Pcm16,
            dither: ExportDither::Tpdf,
            rate: Some(96_000),
            tail: true,
            ..ExportValues::default()
        })]
    );
    assert_eq!(click(&mut v, "CANCEL"), vec![ExportAction::Cancel]);
    assert_eq!(key(&mut v, Key::Escape), vec![ExportAction::Cancel]);
    assert_eq!(
        key(&mut v, Key::Enter),
        vec![ExportAction::Start(v.values())]
    );
}

#[test]
fn the_progress_mode_shows_the_bar_and_only_cancel_works() {
    let mut v = view(ExportScope::Loop);
    v.show_progress();
    assert!(v.in_progress());
    v.set_progress(0.37, "Rendering");
    let t = texts(&v);
    assert!(has(&t, "EXPORTING LOOP"), "{t:?}");
    assert!(has(&t, "37%"), "{t:?}");
    assert!(has(&t, "Rendering"), "{t:?}");
    assert!(v.button_rect("EXPORT").is_none());
    assert!(v.button_rect("16-BIT").is_none());
    assert_eq!(click(&mut v, "CANCEL"), vec![ExportAction::Cancel]);
    // Enter does not start anything while rendering; Escape stops it.
    assert!(key(&mut v, Key::Enter).is_empty());
    assert_eq!(key(&mut v, Key::Escape), vec![ExportAction::Cancel]);
}

#[test]
fn the_panel_is_centred_and_fits_a_small_window() {
    let mut v = view(ExportScope::Song);
    let p = v.panel();
    assert!((p.center().x - 450.0).abs() < 1.0);
    v.set_bounds(Rect::new(0.0, 0.0, 400.0, 300.0));
    assert!(v.panel().width() <= 400.0 - 2.0 * 16.0 + 0.5);
}

#[test]
fn every_button_has_a_tooltip() {
    let mut v = view(ExportScope::Song);
    click(&mut v, "16-BIT");
    for label in [
        "16-BIT",
        "TPDF",
        "PROJECT",
        "44.1 kHz",
        "KEEP EFFECT TAIL",
        "EXPORT",
    ] {
        let p = center(&v, label);
        assert!(v.help_at(p).is_some(), "{label}");
    }
}

#[test]
#[ignore = "writes preview images; run explicitly"]
fn preview_export_png() {
    use support::cpu::CpuCanvas;
    const SCALE: f64 = 1.5;
    let mut text = atomik_ui::TextEngine::new();
    let mut options = view(ExportScope::Song);
    options.set_scale(SCALE);
    let mut progress = view(ExportScope::Loop);
    progress.set_scale(SCALE);
    progress.show_progress();
    progress.set_progress(0.42, "Rendering the loop");
    for (name, v) in [("export_options", &options), ("export_progress", &progress)] {
        let mut canvas = CpuCanvas::new(
            (BOUNDS.width() * SCALE) as u16,
            (360.0 * SCALE) as u16,
            &mut text,
        );
        v.paint(&mut canvas);
        let png = canvas.finish().into_png().expect("png");
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/preview")
            .join(format!("{name}.png"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, png).unwrap();
        println!("wrote {}", path.display());
    }
}

#[test]
fn flac_has_no_float_and_choosing_it_moves_float_to_24_bit() {
    let mut v = view(ExportScope::Song);
    assert!(has(&texts(&v), "FLAC"));
    click(&mut v, "32-BIT FLOAT");
    assert_eq!(v.values().depth, ExportDepth::Float32);
    click(&mut v, "FLAC");
    assert_eq!(v.values().file, atomik_ui::ExportFile::Flac);
    assert_eq!(v.values().depth, ExportDepth::Pcm24);
    // The float button is inert while FLAC is chosen.
    assert!(click(&mut v, "32-BIT FLOAT").is_empty());
    assert_eq!(v.values().depth, ExportDepth::Pcm24);
    click(&mut v, "16-BIT");
    assert_eq!(v.values().depth, ExportDepth::Pcm16);
    // Back to WAV, float works again.
    click(&mut v, "WAV");
    click(&mut v, "32-BIT FLOAT");
    assert_eq!(v.values().depth, ExportDepth::Float32);
    for label in ["FLAC", "32-BIT FLOAT"] {
        let p = center(&v, label);
        assert!(v.help_at(p).is_some(), "{label}");
    }
}

#[test]
fn stems_alone_choose_between_the_whole_song_and_the_locators() {
    for scope in [ExportScope::Song, ExportScope::Loop] {
        assert!(view(scope).button_rect("BETWEEN LOCATORS").is_none());
    }
    let mut v = view(ExportScope::Stems);
    let t = texts(&v);
    for want in ["RANGE", "WHOLE SONG", "BETWEEN LOCATORS"] {
        assert!(has(&t, want), "{want} in {t:?}");
    }
    assert!(!v.values().locators);
    click(&mut v, "BETWEEN LOCATORS");
    assert!(v.values().locators);
    assert!(has(&texts(&v), "exactly between the locators"));
    let started = click(&mut v, "EXPORT");
    assert_eq!(
        started,
        vec![ExportAction::Start(ExportValues {
            locators: true,
            ..ExportValues::default()
        })]
    );
    click(&mut v, "WHOLE SONG");
    assert_eq!(v.values(), ExportValues::default());
    for label in ["WHOLE SONG", "BETWEEN LOCATORS"] {
        let p = center(&v, label);
        assert!(v.help_at(p).is_some(), "{label}");
    }
}

#[test]
fn a_selection_has_its_own_panel_and_stems_can_choose_it() {
    let v = view(ExportScope::Selection);
    let t = texts(&v);
    assert!(has(&t, "EXPORT SELECTION AS AUDIO FILE"), "{t:?}");
    assert!(has(&t, "Exactly the time range selected"), "{t:?}");
    assert!(
        v.button_rect("KEEP EFFECT TAIL").is_none(),
        "no tail for a range"
    );
    assert!(
        v.button_rect("SELECTION").is_none(),
        "only stems have the row"
    );
    assert!(v.button_rect("AIFF").is_some());

    let mut v = view(ExportScope::Stems);
    for want in ["WHOLE SONG", "BETWEEN LOCATORS", "SELECTION"] {
        assert!(v.button_rect(want).is_some(), "{want}");
    }
    // The three buttons of the row do not overlap and stay inside the panel.
    let rects: Vec<_> = ["WHOLE SONG", "BETWEEN LOCATORS", "SELECTION"]
        .iter()
        .map(|l| v.button_rect(l).unwrap())
        .collect();
    for pair in rects.windows(2) {
        assert!(pair[0].x1 < pair[1].x0, "{pair:?}");
    }
    assert!(rects[2].x1 <= v.panel().x1);
    click(&mut v, "SELECTION");
    assert!(v.values().selection && !v.values().locators);
    assert!(has(&texts(&v), "exactly the selected range"));
    assert_eq!(
        click(&mut v, "EXPORT"),
        vec![ExportAction::Start(ExportValues {
            selection: true,
            ..ExportValues::default()
        })]
    );
    // Choosing another range drops the selection.
    click(&mut v, "BETWEEN LOCATORS");
    assert!(v.values().locators && !v.values().selection);
    click(&mut v, "SELECTION");
    click(&mut v, "WHOLE SONG");
    assert_eq!(v.values(), ExportValues::default());
    let p = center(&v, "SELECTION");
    assert!(v.help_at(p).is_some());
}

fn start_values(actions: Vec<ExportAction>) -> ExportValues {
    match actions.as_slice() {
        [ExportAction::Start(v)] => *v,
        other => panic!("expected one Start, got {other:?}"),
    }
}

const MISSING: &str = "MP3, AAC and Opus need FFmpeg: set its folder in Settings > Video.";

#[test]
fn the_compressed_formats_are_always_shown_and_choosable_with_ffmpeg() {
    let mut v = view(ExportScope::Song);
    for label in ["WAV", "AIFF", "FLAC", "MP3", "AAC", "OPUS"] {
        assert!(v.button_rect(label).is_some(), "{label}");
    }
    // No quality buttons for a lossless format.
    assert!(v.button_rect("192 kbps").is_none());
    click(&mut v, "MP3");
    assert_eq!(v.values().file, ExportFile::Mp3);
    for label in [
        "128 kbps", "192 kbps", "256 kbps", "320 kbps", "V0", "V2", "V4",
    ] {
        assert!(v.button_rect(label).is_some(), "{label}");
    }
    click(&mut v, "AAC");
    assert!(
        v.button_rect("V2").is_none(),
        "AAC has no variable bitrate here"
    );
    assert!(v.button_rect("320 kbps").is_some());
    click(&mut v, "OPUS");
    for label in ["64 kbps", "96 kbps", "128 kbps", "160 kbps", "192 kbps"] {
        assert!(v.button_rect(label).is_some(), "{label}");
    }
    assert!(v.button_rect("320 kbps").is_none());
}

#[test]
fn the_quality_buttons_set_the_quality_of_the_format_chosen() {
    let mut v = view(ExportScope::Song);
    click(&mut v, "MP3");
    click(&mut v, "V2");
    assert_eq!(v.values().mp3, ExportQuality::Vbr(2));
    click(&mut v, "320 kbps");
    assert_eq!(v.values().mp3, ExportQuality::Cbr(320));
    click(&mut v, "AAC");
    click(&mut v, "256 kbps");
    click(&mut v, "OPUS");
    click(&mut v, "96 kbps");
    let values = v.values();
    // Each format keeps its own quality while the others are chosen.
    assert_eq!(
        (values.mp3, values.aac, values.opus),
        (ExportQuality::Cbr(320), 256, 96)
    );
    assert_eq!(values.quality(), Some(ExportQuality::Cbr(96)));
    click(&mut v, "WAV");
    assert_eq!(v.values().quality(), None);
    click(&mut v, "AAC");
    let started = start_values(click(&mut v, "EXPORT"));
    assert_eq!((started.file, started.aac), (ExportFile::Aac, 256));
}

#[test]
fn a_compressed_file_has_no_bit_depth_or_dither() {
    let mut v = view(ExportScope::Song);
    click(&mut v, "16-BIT");
    click(&mut v, "TPDF");
    click(&mut v, "MP3");
    let depth = v.values().depth;
    // Both rows are greyed: pressing them changes nothing.
    click(&mut v, "24-BIT");
    click(&mut v, "SHAPED");
    assert_eq!(v.values().depth, depth);
    assert_ne!(v.values().dither, ExportDither::Shaped);
    let text = texts(&v);
    assert!(
        text.iter().any(|t| t.contains("true-peak headroom")),
        "the panel says to leave headroom for lossy encoding: {text:?}"
    );
}

#[test]
fn without_ffmpeg_the_compressed_formats_are_disabled_with_a_line_that_says_what_to_do() {
    let mut v = ExportView::new(Theme::dark());
    v.set_bounds(BOUNDS);
    v.set_encoder(EncoderState::Missing(MISSING.to_owned()));
    v.open(
        ExportScope::Song,
        "my song.wav",
        48_000,
        ExportValues::default(),
    );
    for label in ["MP3", "AAC", "OPUS"] {
        // Shown, but pressing them does nothing.
        assert!(v.button_rect(label).is_some(), "{label}");
        click(&mut v, label);
        assert_eq!(v.values().file, ExportFile::Wav, "{label}");
        let tip = v.help_at(center(&v, label)).unwrap();
        assert_eq!(tip.text, MISSING, "the tooltip says how to set FFmpeg up");
    }
    assert!(has(&texts(&v), MISSING), "{:?}", texts(&v));
    // The lossless formats are untouched.
    click(&mut v, "FLAC");
    assert_eq!(v.values().file, ExportFile::Flac);
    // Finding FFmpeg later enables them and clears the line.
    v.set_encoder(EncoderState::Ready);
    assert!(!has(&texts(&v), MISSING));
    click(&mut v, "OPUS");
    assert_eq!(v.values().file, ExportFile::Opus);
}

#[test]
fn a_remembered_compressed_format_falls_back_to_wav_when_ffmpeg_is_missing() {
    let remembered = ExportValues {
        file: ExportFile::Mp3,
        mp3: ExportQuality::Vbr(0),
        ..ExportValues::default()
    };
    let mut v = ExportView::new(Theme::dark());
    v.set_bounds(BOUNDS);
    v.set_encoder(EncoderState::Checking);
    v.open(ExportScope::Song, "a.wav", 48_000, remembered);
    // While FFmpeg is being looked for the choice stays, and EXPORT waits.
    assert_eq!(v.values().file, ExportFile::Mp3);
    assert!(click(&mut v, "EXPORT").is_empty());
    assert!(key(&mut v, Key::Enter).is_empty());
    v.set_encoder(EncoderState::Ready);
    assert_eq!(
        start_values(click(&mut v, "EXPORT")).mp3,
        ExportQuality::Vbr(0)
    );
    v.set_encoder(EncoderState::Missing(MISSING.to_owned()));
    assert_eq!(v.values().file, ExportFile::Wav);
    assert_eq!(start_values(click(&mut v, "EXPORT")).file, ExportFile::Wav);
}

#[test]
fn stems_can_be_compressed_with_ffmpeg_and_the_buttons_are_greyed_without_it() {
    // A remembered compressed format stays for stems when FFmpeg is there.
    let remembered = ExportValues {
        file: ExportFile::Aac,
        aac: 256,
        ..ExportValues::default()
    };
    let mut v = ExportView::new(Theme::dark());
    v.set_bounds(BOUNDS);
    v.open(ExportScope::Stems, "stems", 48_000, remembered);
    assert_eq!(v.values().file, ExportFile::Aac);
    for label in ["MP3", "AAC", "OPUS"] {
        click(&mut v, label);
        assert!(v.values().file.is_lossy(), "{label}");
    }
    click(&mut v, "MP3");
    click(&mut v, "V2");
    let started = start_values(click(&mut v, "EXPORT"));
    assert_eq!(started.file, ExportFile::Mp3);
    assert_eq!(started.mp3, ExportQuality::Vbr(2));
    // Without FFmpeg they are greyed with the setup line, as for a song.
    v.set_encoder(EncoderState::Missing(MISSING.to_owned()));
    assert_eq!(v.values().file, ExportFile::Wav);
    click(&mut v, "AAC");
    assert_eq!(v.values().file, ExportFile::Wav);
    let tip = v.help_at(center(&v, "AAC")).unwrap();
    assert_eq!(tip.text, MISSING);
    assert!(has(&texts(&v), MISSING));
}

#[test]
fn a_loop_and_a_selection_can_be_compressed_too() {
    for scope in [ExportScope::Loop, ExportScope::Selection] {
        let mut v = view(scope);
        click(&mut v, "OPUS");
        click(&mut v, "160 kbps");
        let started = start_values(click(&mut v, "EXPORT"));
        assert_eq!((started.file, started.opus), (ExportFile::Opus, 160));
    }
}

#[test]
fn every_compressed_button_has_a_short_tooltip() {
    let mut v = view(ExportScope::Song);
    for file in ["MP3", "AAC", "OPUS"] {
        click(&mut v, file);
        let tip = v.help_at(center(&v, file)).unwrap();
        assert!(tip.text.len() <= 90, "{}", tip.text);
        for label in ["64 kbps", "128 kbps", "320 kbps", "V0", "V2", "V4"] {
            if v.button_rect(label).is_some() {
                let tip = v.help_at(center(&v, label)).unwrap();
                assert!(tip.text.len() <= 90, "{label}: {}", tip.text);
            }
        }
        let depth = v.help_at(center(&v, "16-BIT")).unwrap();
        assert!(depth.text.contains("compressed"), "{}", depth.text);
    }
}

#[test]
fn the_panel_with_six_formats_and_a_quality_row_fits_in_its_width() {
    let mut v = view(ExportScope::Song);
    click(&mut v, "MP3");
    let panel = v.panel();
    for label in ["OPUS", "V4", "320 kbps"] {
        let r = v.button_rect(label).unwrap();
        assert!(
            r.x1 <= panel.x1 - 14.0 + 0.5,
            "{label} ends at {} of {}",
            r.x1,
            panel.x1
        );
    }
}

#[test]
fn a_warning_shows_under_the_options_until_a_choice_changes() {
    let mut v = view(ExportScope::Song);
    v.set_warning(Some(
        "song.mp3 already exists: press EXPORT again to replace it".to_owned(),
    ));
    assert!(has(&texts(&v), "already exists"));
    assert!(v.warning().is_some());
    // EXPORT keeps the line: the app decides what a second press means.
    click(&mut v, "EXPORT");
    assert!(v.warning().is_some());
    click(&mut v, "AIFF");
    assert!(v.warning().is_none());
    assert!(!has(&texts(&v), "already exists"));
    // Opening the panel again starts without one.
    v.set_warning(Some("x".to_owned()));
    v.open(ExportScope::Song, "a.wav", 48_000, ExportValues::default());
    assert!(v.warning().is_none());
}

fn loudness_of(v: &ExportView) -> ExportLoudness {
    v.values().loudness
}

/// Opens the CUSTOM box number `which` (0: the loudness row, 1: the ceiling row), types `text`
/// and presses Enter.
fn type_custom(v: &mut ExportView, which: usize, text: &str) {
    let p = v.custom_rect(which == 1).center();
    v.handle(&down(p));
    v.handle(&up(p));
    assert!(v.wants_text(), "the field has the keys");
    v.handle(&UiEvent::Ime(ImeEvent::Commit(text.to_owned())));
    key(v, Key::Enter);
}

#[test]
fn a_custom_loudness_target_is_typed_in_a_box_and_taken_with_enter() {
    for scope in [ExportScope::Song, ExportScope::Stems] {
        let mut v = view(scope);
        assert_eq!(loudness_of(&v), ExportLoudness::Off);
        type_custom(&mut v, 0, "-12.5");
        assert_eq!(loudness_of(&v), ExportLoudness::Custom(-125));
        assert!(!v.wants_text());
        // The box shows the value, lit.
        assert!(has(&texts(&v), "-12.5 LUFS"));
        // A whole number without a sign is negative, a comma is a point.
        type_custom(&mut v, 0, "23");
        assert_eq!(loudness_of(&v), ExportLoudness::Custom(-230));
        type_custom(&mut v, 0, "-18,5");
        assert_eq!(loudness_of(&v), ExportLoudness::Custom(-185));
        // The fixed buttons still work, and replace it.
        click(&mut v, "-14 LUFS");
        assert_eq!(loudness_of(&v), ExportLoudness::Target(-14));
        let started = start_values(click(&mut v, "EXPORT"));
        assert_eq!(started.loudness, ExportLoudness::Target(-14), "{scope:?}");
    }
}

#[test]
fn a_custom_target_out_of_range_or_not_a_number_is_refused_and_escape_drops_it() {
    let mut v = view(ExportScope::Song);
    click(&mut v, "-16 LUFS");
    for bad in ["-3", "-45", "1.2.3", "-", "--"] {
        type_custom(&mut v, 0, bad);
        assert!(v.wants_text(), "{bad:?}: the field stays open");
        assert_eq!(loudness_of(&v), ExportLoudness::Target(-16), "{bad:?}");
        assert!(
            v.warning().is_some_and(|w| w.contains("-40 to -5 LUFS")),
            "{bad:?}"
        );
        // Escape drops the entry and does not close the panel.
        assert!(key(&mut v, Key::Escape).is_empty());
        assert!(!v.wants_text());
    }
    // Typing a good value after a refusal clears the line.
    type_custom(&mut v, 0, "-9.5");
    assert_eq!(loudness_of(&v), ExportLoudness::Custom(-95));
    assert!(v.warning().is_none());
    // A click elsewhere takes a valid number, then does what it was for.
    let at = v.custom_rect(false).center();
    v.handle(&down(at));
    v.handle(&up(at));
    v.handle(&UiEvent::Ime(ImeEvent::Commit("-20".to_owned())));
    let mono = center(&v, "MONO");
    v.handle(&down(mono));
    v.handle(&up(mono));
    assert_eq!(loudness_of(&v), ExportLoudness::Custom(-200));
    assert!(v.values().mono);
}

#[test]
fn the_ceiling_has_two_buttons_and_a_box_and_matters_to_a_target_only() {
    for scope in [ExportScope::Song, ExportScope::Stems] {
        let mut v = view(scope);
        assert!(has(&texts(&v), "CEILING"));
        assert_eq!(v.values().ceiling, -10, "-1 dBTP by default");
        // Greyed while the level is not touched: a press does nothing.
        click(&mut v, "-2 dBTP");
        assert_eq!(v.values().ceiling, -10);
        click(&mut v, "-14 LUFS");
        click(&mut v, "-2 dBTP");
        assert_eq!(v.values().ceiling, -20);
        click(&mut v, "-1 dBTP");
        assert_eq!(v.values().ceiling, -10);
        type_custom(&mut v, 1, "-1,5");
        assert_eq!(v.values().ceiling, -15);
        assert!(has(&texts(&v), "-1.5 dBTP"));
        type_custom(&mut v, 1, "-12");
        assert_eq!(v.values().ceiling, -15, "out of range: kept");
        assert!(v.warning().is_some_and(|w| w.contains("-9 to 0 dBTP")));
        key(&mut v, Key::Escape);
        // A guide has no ceiling.
        click(&mut v, "-9 GUIDE");
        click(&mut v, "-2 dBTP");
        assert_eq!(v.values().ceiling, -15);
        assert!(v.help_at(center(&v, "-2 dBTP")).is_some());
    }
}

#[test]
fn stems_have_a_loudness_a_ceiling_and_a_gain_row_the_mix_has_not_the_gain() {
    let mut v = view(ExportScope::Stems);
    let t = texts(&v);
    for want in [
        "LOUDNESS",
        "CEILING",
        "STEM GAIN",
        "SAME GAIN FOR ALL",
        "EACH STEM",
    ] {
        assert!(has(&t, want), "{want} in {t:?}");
    }
    assert!(!has(&t, "MIX DOWN") && !has(&t, "EXTRA FILE"));
    let song = texts(&view(ExportScope::Song));
    assert!(!has(&song, "STEM GAIN") && has(&song, "LOUDNESS") && has(&song, "CEILING"));
    // One gain for all is the default; the choice is greyed without a loudness.
    assert!(v.values().stems_together);
    click(&mut v, "EACH STEM");
    assert!(
        v.values().stems_together,
        "inert while the level is not touched"
    );
    click(&mut v, "-14 LUFS");
    click(&mut v, "EACH STEM");
    assert!(!v.values().stems_together);
    click(&mut v, "SAME GAIN FOR ALL");
    assert!(v.values().stems_together);
    // The loudness reaches the app with the stems (it used to be dropped for stems); the
    // mix-down and the extra file still do not.
    click(&mut v, "EACH STEM");
    let started = start_values(click(&mut v, "EXPORT"));
    assert_eq!(started.loudness, ExportLoudness::Target(-14));
    assert!(!started.stems_together && !started.mono && !started.also_mp3);
    for label in ["SAME GAIN FOR ALL", "EACH STEM", "CUSTOM"] {
        let tip = v.help_at(center(&v, label)).unwrap();
        assert!(tip.text.len() <= 90, "{label}: {}", tip.text);
    }
}

#[test]
fn the_panel_with_the_new_rows_fits_its_window_and_the_boxes_fit_their_rows() {
    for scope in [ExportScope::Song, ExportScope::Stems] {
        let v = view(scope);
        let panel = v.panel();
        assert!(panel.y1 <= BOUNDS.y1, "{scope:?}: {panel:?}");
        for r in [v.custom_rect(false), v.custom_rect(true)] {
            assert!(r.x1 <= panel.x1 - 14.0 + 0.5, "{r:?} of {panel:?}");
        }
        for label in ["SAME GAIN FOR ALL", "-2 dBTP"] {
            if let Some(r) = v.button_rect(label) {
                assert!(r.x1 <= panel.x1 - 14.0 + 0.5, "{label}");
            }
        }
    }
}
