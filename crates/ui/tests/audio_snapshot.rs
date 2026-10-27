//! Display-list checks of audio clips: goldens of an audio lane with two
//! clips (one with peaks, one without) showing fades and gain, and of a drop-hover indicator,
//! plus structural assertions on the waveform, the asset end and the fades. Regenerate the
//! goldens with `ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-ui --test audio_snapshot`.

mod support;

use atomik_model::{AudioClipContent, ClipContent, ClipId, RackDoc, TempoEvent};
use atomik_ui::canvas::Geom;
use atomik_ui::kurbo::{BezPath, Point, Rect, Shape};
use atomik_ui::peniko::{Brush, Color};
use atomik_ui::timeline::Snap;
use atomik_ui::{ArrangementView, DisplayList, Op, SeqTheme, Theme, UiEvent};
use std::path::PathBuf;
use support::audio::*;
use support::song::*;
use support::*;

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";

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
    if expected != actual {
        let line = expected
            .lines()
            .zip(actual.lines())
            .position(|(a, b)| a != b)
            .unwrap_or(expected.lines().count().min(actual.lines().count()));
        panic!(
            "{name} differs from golden at line {}:\n  golden: {:?}\n  actual: {:?}\n(regenerate with {UPDATE_ENV}=1)",
            line + 1,
            expected.lines().nth(line),
            actual.lines().nth(line)
        );
    }
}

fn paint(v: &mut ArrangementView, doc: &RackDoc) -> DisplayList {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl
}

fn record_audio_lane() -> DisplayList {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    v.set_snap(Snap::Beat);
    v.set_playhead(2 * BAR + PPQ);
    v.set_selected_clips(&[ClipId(10)]);
    paint(&mut v, &doc)
}

fn record_drop_hover() -> DisplayList {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    v.set_snap(Snap::Beat);
    let lane = v.lane_rect(&key("audio-track-2"), 0).unwrap();
    let pos = Point::new(v.tick_to_x(2 * BAR + PPQ / 2), lane.center().y);
    let _ = v.handle(
        &UiEvent::FileHover {
            pos,
            paths: vec![PathBuf::from("a.wav")],
        },
        &doc,
        &reg,
    );
    paint(&mut v, &doc)
}

#[test]
fn audio_lane_matches_golden() {
    check("audio_arrangement_1x.json", &record_audio_lane().to_json());
}

#[test]
fn drop_hover_matches_golden() {
    check("audio_drop_hover_1x.json", &record_drop_hover().to_json());
}

#[test]
fn audio_recordings_are_deterministic() {
    assert_eq!(record_audio_lane().to_json(), record_audio_lane().to_json());
    assert_eq!(record_drop_hover().to_json(), record_drop_hover().to_json());
    assert_eq!(
        record_track_meters().to_json(),
        record_track_meters().to_json()
    );
}

// ---------------------------------------------------------------- structure

fn solid(color: Color) -> Brush {
    Brush::Solid(color)
}

fn bbox(p: &BezPath) -> Rect {
    p.bounding_box()
}

/// Bounding boxes of the filled paths of a display list.
fn path_fills(dl: &DisplayList) -> Vec<(Rect, Brush)> {
    dl.ops()
        .iter()
        .filter_map(|op| match op {
            Op::Fill {
                shape: Geom::Path(p),
                brush,
            } => Some((bbox(p), brush.clone())),
            _ => None,
        })
        .collect()
}

fn rect_fills(dl: &DisplayList, color: Color) -> Vec<Rect> {
    dl.ops()
        .iter()
        .filter_map(|op| match op {
            Op::Fill {
                shape: Geom::Rect(r),
                brush,
            } if *brush == solid(color) => Some(*r),
            _ => None,
        })
        .collect()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.01
}

fn with_peaks_for_b(doc: &RackDoc, reg: &atomik_device_sdk::Registry) -> ArrangementView {
    let mut v = audio_arrangement(doc, reg);
    v.set_peaks(HASH_B, pyramid(FRAMES_B, 1));
    v
}

fn set_gain(doc: &mut RackDoc, id: u64, db: f32) {
    for t in &mut doc.song.tracks {
        for l in &mut t.lanes {
            for c in &mut l.clips {
                if c.id == ClipId(id)
                    && let ClipContent::Audio(AudioClipContent { gain_db, .. }) = &mut c.content
                {
                    *gain_db = db;
                }
            }
        }
    }
}

#[test]
fn a_stereo_clip_draws_two_stacked_waveforms_inside_its_clip() {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    let clip = v.clip_rect(ClipId(10)).unwrap();
    let dl = paint(&mut v, &doc);
    let waves: Vec<Rect> = path_fills(&dl)
        .into_iter()
        .map(|p| p.0)
        .filter(|b| b.width() > 100.0)
        .collect();
    assert_eq!(waves.len(), 2, "one path per channel: {waves:?}");
    for w in &waves {
        assert!(
            w.x0 >= clip.x0 - 1.0 && w.x1 <= clip.x1 + 1.0,
            "{w:?} in {clip:?}"
        );
        assert!(w.y0 >= clip.y0 && w.y1 <= clip.y1);
    }
    let (top, bottom) = if waves[0].y0 < waves[1].y0 {
        (waves[0], waves[1])
    } else {
        (waves[1], waves[0])
    };
    assert!(top.y1 <= bottom.y0 + 1.0, "channel 0 above channel 1");
}

#[test]
fn a_mono_clip_draws_one_waveform() {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = with_peaks_for_b(&doc, &reg);
    let dl = paint(&mut v, &doc);
    let clip = v.clip_rect(ClipId(11)).unwrap();
    let waves = path_fills(&dl)
        .into_iter()
        .filter(|(b, _)| b.x0 >= clip.x0 - 1.0 && b.x1 <= clip.x1 + 1.0 && b.width() > 60.0)
        .count();
    assert_eq!(waves, 1);
}

#[test]
fn the_waveform_stops_at_the_asset_end_and_the_rest_is_empty() {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = with_peaks_for_b(&doc, &reg);
    let dl = paint(&mut v, &doc);
    let outside = SeqTheme::from_theme(&Theme::dark()).outside;
    // Clip 11: 2.5 s of the mono asset remain after the 0.5 s offset = 5 beats at 120 BPM.
    let end_x = v.tick_to_x(4 * BAR + 5 * PPQ);
    let clip = v.clip_rect(ClipId(11)).unwrap();
    let empties = rect_fills(&dl, outside);
    let empty = empties
        .iter()
        .find(|r| close(r.x0, end_x))
        .unwrap_or_else(|| panic!("no empty part at {end_x}: {empties:?}"));
    assert!(close(empty.x1, clip.x1));
    // Clip 10 lies fully inside its asset.
    assert_eq!(empties.len(), 1, "only clip 11 runs past its asset");
    let wave = path_fills(&dl)
        .into_iter()
        .map(|p| p.0)
        .find(|b| b.x0 >= clip.x0 - 1.0 && b.x1 <= clip.x1 + 1.0 && b.width() > 60.0)
        .unwrap();
    assert!(wave.x1 <= end_x + 1.0 && wave.x1 > end_x - 2.0, "{wave:?}");
}

#[test]
fn the_asset_end_follows_the_tempo_map() {
    let (mut doc, reg) = (audio_doc(), registry());
    // Half speed from bar 3 on: clip 11 starts at bar 5, 2.5 s are now 2.5 beats.
    doc.song.tempo.push(TempoEvent::step(2 * BAR, 60.0));
    let mut v = with_peaks_for_b(&doc, &reg);
    let dl = paint(&mut v, &doc);
    let outside = SeqTheme::from_theme(&Theme::dark()).outside;
    let end_x = v.tick_to_x(4 * BAR + 5 * PPQ / 2);
    let empties = rect_fills(&dl, outside);
    assert!(
        empties.iter().any(|r| close(r.x0, end_x)),
        "expected {end_x}, got {empties:?}"
    );
}

#[test]
fn gain_scales_the_displayed_waveform() {
    let reg = registry();
    let height = |db: f32| {
        let mut doc = audio_doc();
        set_gain(&mut doc, 10, db);
        let mut v = audio_arrangement(&doc, &reg);
        let dl = paint(&mut v, &doc);
        path_fills(&dl)
            .into_iter()
            .map(|p| p.0)
            .filter(|b| b.width() > 100.0)
            .map(|b| b.height())
            .fold(0.0_f64, f64::max)
    };
    let (loud, quiet) = (height(0.0), height(-12.0));
    assert!(loud > 4.0, "{loud}");
    let ratio = quiet / loud;
    assert!((0.2..0.3).contains(&ratio), "-12 dB is a quarter: {ratio}");
}

#[test]
fn clips_without_peaks_show_the_file_name() {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    let dl = paint(&mut v, &doc);
    let texts = dl.texts();
    assert!(texts.contains(&"vox.flac"), "{texts:?}");
    assert!(!texts.contains(&"drums.wav"), "clip 10 has a waveform");
    v.clear_peaks(HASH_A);
    assert!(paint(&mut v, &doc).texts().contains(&"drums.wav"));
}

#[test]
fn a_clip_of_a_missing_asset_says_so() {
    let (mut doc, reg) = (audio_doc(), registry());
    doc.assets.remove(HASH_B);
    let mut v = audio_arrangement(&doc, &reg);
    assert!(paint(&mut v, &doc).texts().contains(&"missing asset"));
}

#[test]
fn fades_are_shaded_with_their_length() {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    let dl = paint(&mut v, &doc);
    let shade = SeqTheme::from_theme(&Theme::dark()).fade_shade;
    let fades: Vec<f64> = path_fills(&dl)
        .into_iter()
        .filter(|(_, b)| *b == solid(shade))
        .map(|(b, _)| b.width())
        .collect();
    // 24000 frames = 0.5 s = one beat = 24 px; 48000 frames = 48 px at 96 px per bar.
    assert_eq!(fades.len(), 2, "{fades:?}");
    assert!(fades.iter().any(|w| close(*w, 24.0)), "{fades:?}");
    assert!(fades.iter().any(|w| close(*w, 48.0)), "{fades:?}");
}

#[test]
fn audio_clips_use_their_own_body_colour() {
    let (doc, reg) = (audio_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    let dl = paint(&mut v, &doc);
    let st = SeqTheme::from_theme(&Theme::dark());
    let accent = Theme::dark().accent_for_type("atomik.audio-track");
    let want = Brush::Solid(st.audio_body(accent, false));
    let found = dl
        .ops()
        .iter()
        .any(|op| matches!(op, Op::Fill { shape: Geom::RoundedRect(_), brush } if *brush == want));
    assert!(found, "audio body colour not used");
}

/// The audio track headers with their meters at work: the first track (default height) reads
/// its input with a clipped left peak, the second (small, one line) its playback.
fn record_track_meters() -> DisplayList {
    use atomik_ui::{MeterSource, TrackLevel};
    let (mut doc, reg) = (audio_doc(), registry());
    doc.song
        .track_heights
        .insert(key("audio-track-2"), atomik_ui::HEIGHT_SMALL);
    let mut v = audio_arrangement(&doc, &reg);
    let level = |device: &str, source, peaks| TrackLevel {
        device: key(device),
        source,
        peaks,
        stereo: true,
    };
    v.feed_track_levels(
        &[
            level("audio-track", MeterSource::Input, [1.2, 0.25]),
            level("audio-track-2", MeterSource::Playback, [0.5, 0.1]),
        ],
        0.0,
    );
    paint(&mut v, &doc)
}

#[test]
fn track_meters_match_golden() {
    check(
        "audio_track_meters_1x.json",
        &record_track_meters().to_json(),
    );
}
