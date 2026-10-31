//! The level meter in the header of every audio track in the arrangement: the input level
//! after the gain while the track is armed, the playback level otherwise, a peak hold, the dB
//! readout, a clip lamp that latches until the meter is clicked, the layout at every track
//! height, and the repaints (only when what is drawn changes).

mod support;

use atomik_model::RackDoc;
use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{
    ArrangementView, DisplayList, HEIGHT_LARGE, HEIGHT_SMALL, MeterSource, TrackButton, TrackLevel,
    UiAction,
};
use support::audio::*;
use support::song::*;
use support::*;

const GUITAR: &str = "audio-track";
const VOX: &str = "audio-track-2";

fn level(device: &str, source: MeterSource, l: f32, r: f32) -> TrackLevel {
    TrackLevel {
        device: key(device),
        source,
        peaks: [l, r],
        stereo: true,
    }
}

fn input(device: &str, p: f32) -> TrackLevel {
    level(device, MeterSource::Input, p, p)
}

fn playback(device: &str, p: f32) -> TrackLevel {
    level(device, MeterSource::Playback, p, p)
}

/// The audio fixture with the audio tracks `height` tall (`None`: the default).
fn setup(height: Option<u16>) -> (RackDoc, ArrangementView) {
    let (mut doc, reg) = (audio_doc(), registry());
    if let Some(h) = height {
        doc.song_mut().track_heights.insert(key(GUITAR), h);
        doc.song_mut().track_heights.insert(key(VOX), h);
    }
    let v = audio_arrangement(&doc, &reg);
    (doc, v)
}

fn texts(v: &mut ArrangementView, doc: &RackDoc) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl.texts().into_iter().map(str::to_string).collect()
}

fn click(v: &mut ArrangementView, doc: &RackDoc, p: Point) -> Vec<UiAction> {
    let reg = registry();
    run(|e| v.handle(e, doc, &reg), &[down(p), up(p)])
}

fn db(amp: f32) -> f32 {
    20.0 * amp.log10()
}

// ---------------------------------------------------------------- the source

#[test]
fn an_armed_track_shows_its_input_level_and_an_unarmed_one_its_playback() {
    let (doc, mut v) = setup(None);
    v.feed_track_levels(&[input(GUITAR, 0.5), playback(VOX, 0.25)], 0.0);
    let guitar = v.track_meter(&key(GUITAR)).unwrap();
    assert_eq!(guitar.source, MeterSource::Input);
    assert!((guitar.level_db[0] - db(0.5)).abs() < 0.01);
    let vox = v.track_meter(&key(VOX)).unwrap();
    assert_eq!(vox.source, MeterSource::Playback);
    assert!((vox.level_db[0] - db(0.25)).abs() < 0.01);
    // The source follows the arm: the bar starts again from the new source.
    v.feed_track_levels(&[playback(GUITAR, 0.0), playback(VOX, 0.0)], 0.1);
    let guitar = v.track_meter(&key(GUITAR)).unwrap();
    assert_eq!(guitar.source, MeterSource::Playback);
    assert_eq!(
        guitar.level_db,
        [-60.0, -60.0],
        "no input level carried over"
    );
    let _ = doc;
}

#[test]
fn only_audio_tracks_have_a_meter() {
    let (_doc, mut v) = setup(None);
    assert!(v.track_meter(&key("bass")).is_none());
    assert!(v.track_meter_point(&key("bass")).is_none());
    v.feed_track_levels(&[input("bass", 1.0)], 0.0);
    assert!(
        v.track_meter(&key("bass")).is_none(),
        "a note track has none"
    );
    assert!(v.track_meter(&key(GUITAR)).is_some());
    assert!(v.track_meter(&key("nope")).is_none());
}

#[test]
fn the_bars_follow_each_channel_with_a_hold_at_the_console_rates() {
    let (_doc, mut v) = setup(None);
    v.feed_track_levels(&[level(GUITAR, MeterSource::Input, 0.5, 0.25)], 0.0);
    let m = v.track_meter(&key(GUITAR)).unwrap();
    assert!((m.level_db[0] - db(0.5)).abs() < 0.01);
    assert!((m.level_db[1] - db(0.25)).abs() < 0.01);
    assert!(
        (m.hold_db - db(0.5)).abs() < 0.01,
        "the hold is the louder channel"
    );
    v.feed_track_levels(&[input(GUITAR, 0.0)], 0.25);
    v.feed_track_levels(&[input(GUITAR, 0.0)], 0.5);
    let m = v.track_meter(&key(GUITAR)).unwrap();
    assert!(
        (m.level_db[0] - (db(0.5) - 12.0)).abs() < 0.01,
        "24 dB/s for half a second"
    );
    assert!((m.hold_db - db(0.5)).abs() < 0.01, "held");
    for t in [0.75, 1.0, 1.25, 1.5, 1.75, 2.0] {
        v.feed_track_levels(&[input(GUITAR, 0.0)], t);
    }
    assert!(v.track_meter(&key(GUITAR)).unwrap().hold_db < db(0.5) - 0.5);
}

#[test]
fn a_stalled_frame_does_not_jump_and_nan_reads_silence() {
    let (_doc, mut v) = setup(None);
    v.feed_track_levels(&[input(GUITAR, 1.0)], 0.0);
    v.feed_track_levels(&[input(GUITAR, 0.0)], 100.0);
    assert!(v.track_meter(&key(GUITAR)).unwrap().level_db[0] > -7.0);
    v.feed_track_levels(&[input(GUITAR, f32::NAN)], 100.1);
    assert!(v.track_meter(&key(GUITAR)).unwrap().level_db[0].is_finite());
}

// ---------------------------------------------------------------- the readout

#[test]
fn the_readout_is_the_held_peak_in_decibels() {
    let (doc, mut v) = setup(None);
    assert_eq!(v.track_meter(&key(GUITAR)).unwrap().readout, "-inf");
    assert!(texts(&mut v, &doc).iter().any(|t| t == "-inf"));
    v.feed_track_levels(&[input(GUITAR, 0.5), playback(VOX, 2.0)], 0.0);
    assert_eq!(v.track_meter(&key(GUITAR)).unwrap().readout, "-6.0");
    assert_eq!(v.track_meter(&key(VOX)).unwrap().readout, "+6.0");
    let t = texts(&mut v, &doc);
    assert!(t.iter().any(|s| s == "-6.0"), "{t:?}");
    assert!(t.iter().any(|s| s == "+6.0"), "{t:?}");
    v.feed_track_levels(&[input(GUITAR, 1.0), playback(VOX, 0.0)], 0.1);
    assert_eq!(v.track_meter(&key(GUITAR)).unwrap().readout, "0.0");
}

// ---------------------------------------------------------------- the clip lamp

#[test]
fn the_lamp_latches_at_full_scale_and_a_click_on_the_meter_clears_it() {
    let (doc, mut v) = setup(None);
    v.feed_track_levels(&[input(GUITAR, 0.99)], 0.0);
    assert!(!v.track_meter(&key(GUITAR)).unwrap().latched);
    v.feed_track_levels(&[input(GUITAR, 1.0)], 0.1);
    assert!(v.track_meter(&key(GUITAR)).unwrap().latched);
    for i in 0..60 {
        v.feed_track_levels(&[input(GUITAR, 0.0)], 0.2 + f64::from(i) * 0.1);
    }
    assert!(
        v.track_meter(&key(GUITAR)).unwrap().latched,
        "it does not time out"
    );
    let lit = {
        let mut dl = DisplayList::new();
        v.paint(&mut dl, &doc, &registry());
        dl.to_json()
    };
    let p = v.track_meter_point(&key(GUITAR)).unwrap();
    let a = click(&mut v, &doc, p);
    assert!(
        a.is_empty(),
        "clearing the lamp is not a document edit: {a:?}"
    );
    let m = v.track_meter(&key(GUITAR)).unwrap();
    assert!(!m.latched);
    assert_eq!(m.hold_db, -60.0, "the hold is cleared too");
    assert_eq!(m.readout, "-inf");
    let cleared = {
        let mut dl = DisplayList::new();
        v.paint(&mut dl, &doc, &registry());
        dl.to_json()
    };
    assert_ne!(lit, cleared, "the lamp is drawn lit");
    assert!(
        v.selected_track().is_none(),
        "the click is not a track selection"
    );
}

#[test]
fn the_latch_belongs_to_the_track_and_the_source() {
    let (doc, mut v) = setup(None);
    v.feed_track_levels(&[input(GUITAR, 2.0), playback(VOX, 2.0)], 0.0);
    // Clicking one meter clears only that track.
    let p = v.track_meter_point(&key(VOX)).unwrap();
    click(&mut v, &doc, p);
    assert!(v.track_meter(&key(GUITAR)).unwrap().latched);
    assert!(!v.track_meter(&key(VOX)).unwrap().latched);
    // A latched input goes when the track is disarmed, a latched playback survives the arm.
    v.feed_track_levels(&[playback(GUITAR, 0.0), input(VOX, 2.0)], 0.1);
    assert!(!v.track_meter(&key(GUITAR)).unwrap().latched);
    v.feed_track_levels(&[playback(GUITAR, 2.0), playback(VOX, 0.0)], 0.2);
    assert!(v.track_meter(&key(GUITAR)).unwrap().latched);
    v.feed_track_levels(&[input(GUITAR, 0.0), playback(VOX, 0.0)], 0.3);
    assert!(
        !v.track_meter(&key(GUITAR)).unwrap().latched,
        "now the input's own lamp"
    );
    v.feed_track_levels(&[playback(GUITAR, 0.0), playback(VOX, 0.0)], 0.4);
    assert!(
        v.track_meter(&key(GUITAR)).unwrap().latched,
        "the playback lamp is still lit"
    );
}

#[test]
fn a_click_is_reported_so_the_transport_bar_can_follow_and_a_clear_can_be_applied() {
    let (doc, mut v) = setup(None);
    v.feed_track_levels(&[input(GUITAR, 2.0)], 0.0);
    assert!(v.take_meter_clears().is_empty());
    let p = v.track_meter_point(&key(GUITAR)).unwrap();
    click(&mut v, &doc, p);
    assert_eq!(
        v.take_meter_clears(),
        vec![(key(GUITAR), MeterSource::Input)]
    );
    assert!(v.take_meter_clears().is_empty(), "taken once");
    // The other direction: the transport bar's click clears the latch here, silently.
    v.feed_track_levels(&[input(GUITAR, 2.0)], 0.1);
    assert!(v.track_meter(&key(GUITAR)).unwrap().latched);
    v.clear_meter_latch(&key(GUITAR), MeterSource::Input);
    assert!(!v.track_meter(&key(GUITAR)).unwrap().latched);
    assert!(v.take_meter_clears().is_empty(), "not an echo");
    // A clear of the source that is not shown leaves the shown one alone.
    v.feed_track_levels(&[input(GUITAR, 2.0)], 0.2);
    v.clear_meter_latch(&key(GUITAR), MeterSource::Playback);
    assert!(v.track_meter(&key(GUITAR)).unwrap().latched);
}

// ---------------------------------------------------------------- heights

/// Every control of the header of `device`, as a point.
fn controls(v: &ArrangementView, device: &str) -> Vec<(TrackButton, Point)> {
    [
        TrackButton::Expand,
        TrackButton::Mute,
        TrackButton::Solo,
        TrackButton::Arm,
        TrackButton::Input,
        TrackButton::Monitor,
    ]
    .into_iter()
    .filter_map(|b| v.track_button_center(&key(device), b).map(|p| (b, p)))
    .collect()
}

#[test]
fn the_meter_fits_the_header_at_every_height_without_covering_a_control() {
    for height in [
        HEIGHT_SMALL,
        40,
        52,
        atomik_model::DEFAULT_TRACK_HEIGHT,
        HEIGHT_LARGE,
        300,
    ] {
        let (doc, mut v) = setup(Some(height));
        v.feed_track_levels(&[input(GUITAR, 0.5), playback(VOX, 0.5)], 0.0);
        for device in [GUITAR, VOX] {
            let r = v.track_meter_rect(&key(device)).unwrap();
            let row = v.lane_rect(&key(device), 0).unwrap();
            assert!(r.width() >= 20.0 && r.height() >= 16.0, "{height}: {r:?}");
            assert!(
                r.x0 >= 0.0 && r.x1 <= 208.0 && r.y0 >= row.y0 && r.y1 <= row.y1,
                "{height}: {r:?} outside the header of {row:?}"
            );
            for (b, p) in controls(&v, device) {
                assert!(!r.contains(p), "{height}: {b:?} under the meter");
                // The meter's box and the button's own box (about 20 px) do not overlap.
                let button = Rect::from_center_size(p, (18.0, 14.0));
                assert!(
                    r.intersect(button).area() == 0.0,
                    "{height}: {b:?} {button:?} overlaps {r:?}"
                );
            }
        }
        let _ = texts(&mut v, &doc);
    }
}

#[test]
fn a_small_track_keeps_the_bar_and_the_lamp_and_drops_the_readout() {
    let (doc, mut v) = setup(Some(HEIGHT_SMALL));
    v.feed_track_levels(&[input(GUITAR, 0.5)], 0.0);
    let t = texts(&mut v, &doc);
    assert!(!t.iter().any(|s| s == "-6.0" || s == "-inf"), "{t:?}");
    // The meter and its lamp are still there and the lamp still latches and clears.
    v.feed_track_levels(&[input(GUITAR, 1.5)], 0.1);
    assert!(v.track_meter(&key(GUITAR)).unwrap().latched);
    let p = v.track_meter_point(&key(GUITAR)).unwrap();
    click(&mut v, &doc, p);
    assert!(!v.track_meter(&key(GUITAR)).unwrap().latched);
    // A taller track shows the number.
    for h in [atomik_model::DEFAULT_TRACK_HEIGHT, HEIGHT_LARGE] {
        let (doc, mut v) = setup(Some(h));
        v.feed_track_levels(&[input(GUITAR, 0.5)], 0.0);
        assert!(texts(&mut v, &doc).iter().any(|s| s == "-6.0"), "{h}");
    }
}

#[test]
fn the_name_gives_up_room_for_the_meter_on_an_audio_track() {
    let (mut doc, reg) = (audio_doc(), registry());
    doc.devices[1].name = "Guitar Rhythm Left".into();
    let mut v = audio_arrangement(&doc, &reg);
    let t = texts(&mut v, &doc);
    assert!(
        t.iter()
            .any(|s| s.starts_with("Guitar") && s.ends_with('.')),
        "{t:?}"
    );
}

// ---------------------------------------------------------------- repaints and help

#[test]
fn the_view_repaints_only_when_what_is_drawn_changes() {
    let (doc, mut v) = setup(None);
    let _ = texts(&mut v, &doc);
    assert!(!v.needs_repaint());
    v.feed_track_levels(&[playback(GUITAR, 0.0), playback(VOX, 0.0)], 0.0);
    v.feed_track_levels(&[playback(GUITAR, 0.0), playback(VOX, 0.0)], 0.016);
    assert!(!v.needs_repaint(), "silence stays silence");
    v.feed_track_levels(&[playback(GUITAR, 0.5), playback(VOX, 0.0)], 0.032);
    assert!(v.needs_repaint());
    let _ = texts(&mut v, &doc);
    // The same peak again, and a change too small to see: nothing to draw.
    v.feed_track_levels(&[playback(GUITAR, 0.5), playback(VOX, 0.0)], 0.032);
    assert!(!v.needs_repaint());
    v.feed_track_levels(&[playback(GUITAR, 0.5001), playback(VOX, 0.0)], 0.048);
    assert!(!v.needs_repaint(), "a hundredth of a dB is not drawn");
}

#[test]
fn a_meter_scrolled_out_of_view_does_not_repaint_the_view() {
    let (doc, mut v) = setup(Some(400));
    v.set_scroll_y(10_000.0);
    let _ = texts(&mut v, &doc);
    let hidden = [GUITAR, VOX]
        .into_iter()
        .find(|d| {
            let r = v.track_meter_rect(&key(d)).unwrap();
            r.y1 < 60.0 || r.y0 > 420.0
        })
        .expect("one audio track is out of view");
    assert!(!v.needs_repaint());
    v.feed_track_levels(&[playback(hidden, 0.5)], 0.0);
    assert!(!v.needs_repaint(), "nothing visible moved");
}

#[test]
fn the_meter_has_a_tooltip_that_says_what_it_shows() {
    let (_doc, mut v) = setup(None);
    v.feed_track_levels(&[input(GUITAR, 0.1), playback(VOX, 0.1)], 0.0);
    let armed = v
        .help_at(v.track_meter_point(&key(GUITAR)).unwrap())
        .unwrap();
    assert!(
        armed.text.contains("input") && armed.text.contains("click"),
        "{}",
        armed.text
    );
    let idle = v.help_at(v.track_meter_point(&key(VOX)).unwrap()).unwrap();
    assert!(
        idle.text.contains("fader") && idle.text.contains("click"),
        "{}",
        idle.text
    );
}

#[test]
fn feeding_a_track_that_left_drops_its_state() {
    let (_doc, mut v) = setup(None);
    v.feed_track_levels(&[input(GUITAR, 2.0)], 0.0);
    assert!(v.track_meter(&key(GUITAR)).unwrap().latched);
    v.feed_track_levels(&[], 0.1);
    assert!(!v.track_meter(&key(GUITAR)).unwrap().latched);
}
