//! The film export panel: the options paint and change, the quality and bitrate rows follow the
//! picture and sound choices, the film audio button follows the film, EXPORT and CANCEL emit the
//! right actions with the chosen values, EXPORT waits for FFmpeg, and the progress mode shows the
//! bar and only CANCEL.

mod support;

use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{
    DisplayList, EncoderState, FilmAction, FilmCover, FilmPictureChoice, FilmQualityChoice,
    FilmSoundChoice, FilmValues, FilmView, Key, Modifiers, Theme, UiAction, UiEvent,
};
use support::*;

const BOUNDS: Rect = Rect::new(0.0, 0.0, 900.0, 600.0);

fn view() -> FilmView {
    let mut v = FilmView::new(Theme::dark());
    v.set_bounds(BOUNDS);
    v.open("my song.mp4", true, FilmValues::default());
    v
}

fn center(v: &FilmView, label: &str) -> Point {
    v.button_rect(label)
        .unwrap_or_else(|| panic!("no button {label}"))
        .center()
}

fn click(v: &mut FilmView, label: &str) -> Vec<FilmAction> {
    let p = center(v, label);
    let mut out = v.handle(&down(p));
    out.extend(v.handle(&up(p)));
    out.into_iter()
        .filter_map(|a| match a {
            UiAction::Film(f) => Some(f),
            _ => None,
        })
        .collect()
}

fn key(v: &mut FilmView, key: Key) -> Vec<FilmAction> {
    v.handle(&UiEvent::Key {
        key,
        modifiers: Modifiers::default(),
        pressed: true,
    })
    .into_iter()
    .filter_map(|a| match a {
        UiAction::Film(f) => Some(f),
        _ => None,
    })
    .collect()
}

fn texts(v: &FilmView) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    dl.texts().into_iter().map(str::to_string).collect()
}

fn has(texts: &[String], t: &str) -> bool {
    texts.iter().any(|x| x.contains(t))
}

#[test]
fn the_options_paint_every_choice_and_the_file_name() {
    let t = texts(&view());
    for want in [
        "EXPORT FILM WITH MIX",
        "my song.mp4",
        "COVER",
        "FILM'S SPAN",
        "WHOLE SONG",
        "PICTURE",
        "AUTO",
        "COPY",
        "RE-ENCODE",
        "QUALITY",
        "STANDARD",
        "SOUND",
        "AAC (MP4)",
        "PCM (MOV)",
        "BITRATE",
        "192 kbps",
        "FILM AUDIO",
        "KEEP AS SECOND TRACK",
        "EXPORT",
        "CANCEL",
        "keyframes",
    ] {
        assert!(has(&t, want), "{want} in {t:?}");
    }
}

#[test]
fn the_dialog_says_that_a_copy_cuts_at_keyframes_and_reencoding_is_exact() {
    let mut v = view();
    assert!(has(&texts(&v), "AUTO copies only from the film's start"));
    click(&mut v, "COPY");
    let t = texts(&v);
    assert!(has(&t, "A copy cuts at keyframes"), "{t:?}");
    assert!(has(&t, "choose RE-ENCODE for an exact cut"), "{t:?}");
    click(&mut v, "RE-ENCODE");
    assert!(has(&texts(&v), "cuts the picture exactly"));
}

#[test]
fn buttons_change_the_values_and_export_emits_them() {
    let mut v = view();
    assert!(click(&mut v, "WHOLE SONG").is_empty());
    assert!(click(&mut v, "RE-ENCODE").is_empty());
    assert!(click(&mut v, "HIGH").is_empty());
    assert!(click(&mut v, "PCM (MOV)").is_empty());
    assert!(click(&mut v, "KEEP AS SECOND TRACK").is_empty());
    let want = FilmValues {
        cover: FilmCover::Song,
        picture: FilmPictureChoice::Encode,
        quality: FilmQualityChoice::High,
        sound: FilmSoundChoice::Pcm,
        aac: 192,
        own_audio: true,
    };
    assert_eq!(v.values(), want);
    assert_eq!(click(&mut v, "EXPORT"), vec![FilmAction::Start(want)]);
    // Enter does the same.
    assert_eq!(key(&mut v, Key::Enter), vec![FilmAction::Start(want)]);
}

#[test]
fn the_aac_bitrate_is_chosen_and_follows_the_sound() {
    let mut v = view();
    click(&mut v, "256 kbps");
    assert_eq!(v.values().aac, 256);
    click(&mut v, "PCM (MOV)");
    // PCM has no bitrate: the buttons are inert and keep the last one for later.
    assert!(click(&mut v, "128 kbps").is_empty());
    assert_eq!(v.values().aac, 256);
    click(&mut v, "AAC (MP4)");
    assert_eq!(v.values().aac, 256);
    assert_eq!(FilmSoundChoice::Aac.extension(), "mp4");
    assert_eq!(FilmSoundChoice::Pcm.extension(), "mov");
}

#[test]
fn the_quality_is_inert_while_the_picture_is_copied() {
    let mut v = view();
    click(&mut v, "COPY");
    click(&mut v, "SMALL");
    assert_eq!(v.values().quality, FilmQualityChoice::Standard);
    click(&mut v, "AUTO");
    click(&mut v, "SMALL");
    assert_eq!(v.values().quality, FilmQualityChoice::Small);
}

#[test]
fn a_film_without_sound_cannot_keep_a_second_track() {
    let mut v = FilmView::new(Theme::dark());
    v.set_bounds(BOUNDS);
    v.open(
        "x.mp4",
        false,
        FilmValues {
            own_audio: true,
            ..FilmValues::default()
        },
    );
    assert!(!v.values().own_audio, "the remembered choice is dropped");
    assert!(click(&mut v, "KEEP AS SECOND TRACK").is_empty());
    assert!(!v.values().own_audio);
    let r = v.button_rect("KEEP AS SECOND TRACK").unwrap();
    let help = v.help_at(r.center()).unwrap();
    assert!(help.text.contains("no sound"), "{}", help.text);
}

#[test]
fn export_waits_for_ffmpeg_and_says_why_it_cannot_start() {
    let mut v = view();
    v.set_encoder(EncoderState::Checking);
    assert!(click(&mut v, "EXPORT").is_empty());
    assert!(key(&mut v, Key::Enter).is_empty());
    assert!(has(&texts(&v), "Looking for FFmpeg"));
    let why = "The film needs FFmpeg: set its folder in Settings > Video.";
    v.set_encoder(EncoderState::Missing(why.to_owned()));
    assert!(click(&mut v, "EXPORT").is_empty());
    assert!(has(&texts(&v), why));
    let r = v.button_rect("EXPORT").unwrap();
    assert_eq!(v.help_at(r.center()).unwrap().text, why);
    v.set_encoder(EncoderState::Ready);
    assert_eq!(click(&mut v, "EXPORT").len(), 1);
}

#[test]
fn a_warning_shows_until_a_choice_changes() {
    let mut v = view();
    v.set_warning(Some(
        "song.mp3 already exists: press EXPORT again".to_owned(),
    ));
    assert!(has(&texts(&v), "already exists"));
    assert!(v.warning().is_some());
    // A press on EXPORT keeps the line (the app decides what it means).
    click(&mut v, "EXPORT");
    assert!(v.warning().is_some());
    click(&mut v, "SMALL");
    assert!(v.warning().is_none());
    assert!(!has(&texts(&v), "already exists"));
}

#[test]
fn cancel_and_escape_close_the_options() {
    let mut v = view();
    assert_eq!(click(&mut v, "CANCEL"), vec![FilmAction::Cancel]);
    assert_eq!(key(&mut v, Key::Escape), vec![FilmAction::Cancel]);
}

#[test]
fn the_progress_mode_shows_a_bar_and_only_cancel() {
    let mut v = view();
    v.show_progress();
    assert!(v.in_progress());
    v.set_progress(0.5, "Rendering the mix");
    let t = texts(&v);
    assert!(has(&t, "EXPORTING FILM"), "{t:?}");
    assert!(has(&t, "Rendering the mix"), "{t:?}");
    assert!(has(&t, "50%"), "{t:?}");
    assert!(v.button_rect("EXPORT").is_none());
    assert!(v.button_rect("RE-ENCODE").is_none());
    assert_eq!(click(&mut v, "CANCEL"), vec![FilmAction::Cancel]);
    assert!(key(&mut v, Key::Enter).is_empty());
    let bar = v.bar_rect();
    assert!(v.panel().contains(Point::new(bar.x0, bar.y0)));
}

#[test]
fn every_button_has_a_short_tooltip() {
    let v = view();
    for label in [
        "FILM'S SPAN",
        "WHOLE SONG",
        "AUTO",
        "COPY",
        "RE-ENCODE",
        "HIGH",
        "STANDARD",
        "SMALL",
        "AAC (MP4)",
        "PCM (MOV)",
        "128 kbps",
        "KEEP AS SECOND TRACK",
        "EXPORT",
        "CANCEL",
    ] {
        let r = v.button_rect(label).unwrap();
        let help = v.help_at(r.center()).unwrap();
        assert!(
            !help.text.is_empty() && help.text.len() <= 90,
            "{label}: {}",
            help.text
        );
    }
}

#[test]
fn the_panel_fits_a_small_window_and_buttons_do_not_overlap() {
    let mut v = view();
    v.set_bounds(Rect::new(0.0, 0.0, 560.0, 420.0));
    let panel = v.panel();
    assert!(
        panel.x0 >= 0.0 && panel.x1 <= 560.0 && panel.y1 <= 420.0,
        "{panel:?}"
    );
    let labels = [
        "FILM'S SPAN",
        "WHOLE SONG",
        "AUTO",
        "COPY",
        "RE-ENCODE",
        "HIGH",
        "STANDARD",
        "SMALL",
        "AAC (MP4)",
        "PCM (MOV)",
        "128 kbps",
        "192 kbps",
        "256 kbps",
        "320 kbps",
        "KEEP AS SECOND TRACK",
        "EXPORT",
        "CANCEL",
    ];
    let rects: Vec<Rect> = labels.iter().map(|l| v.button_rect(l).unwrap()).collect();
    for (i, a) in rects.iter().enumerate() {
        assert!(
            panel.contains(Point::new(a.x1 - 0.5, a.y1 - 0.5)),
            "{}",
            labels[i]
        );
        for (j, b) in rects.iter().enumerate().skip(i + 1) {
            let overlap = a.intersect(*b);
            assert!(
                overlap.width() <= 0.0 || overlap.height() <= 0.0,
                "{} overlaps {}",
                labels[i],
                labels[j]
            );
        }
    }
}
