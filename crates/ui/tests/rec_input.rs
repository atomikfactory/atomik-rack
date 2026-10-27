//! The recording input in the transport bar: the gain (drag, wheel, double-click), the SAFE
//! switch, the level meter with its hold and its latching clip lamp, the tooltips, and the
//! room it takes from the clip inspector and the narrow bar.

mod support;

use atomik_model::{Command, DeviceKey, RackDoc};
use atomik_ui::kurbo::{Point, Rect, Vec2};
use atomik_ui::{
    DisplayList, InspectorSource, Modifiers, RecInput, RecInputPart, TransportBar, UiAction,
    UiEvent,
};
use support::audio::*;
use support::song::*;
use support::*;

const GAIN: &str = "rec_gain";
const SAFE: &str = "rec_clip_safe";

fn input(gain_db: f32, clip_safe: bool) -> RecInput {
    RecInput {
        device: key("audio-track-1"),
        name: "Guitar".into(),
        gain_db,
        clip_safe,
    }
}

/// A bar `width` wide showing the input of `audio-track-1`.
fn bar(width: f64, gain_db: f32, clip_safe: bool) -> (TransportBar, RackDoc) {
    let doc = audio_doc();
    let mut b = TransportBar::new(atomik_ui::Theme::dark());
    b.set_bounds(Rect::new(0.0, 0.0, width, 44.0));
    b.sync(&doc, &registry());
    b.set_rec_input(Some(input(gain_db, clip_safe)));
    (b, doc)
}

fn handle(bar: &mut TransportBar, doc: &RackDoc, events: &[UiEvent]) -> Vec<UiAction> {
    let reg = registry();
    run(|e| bar.handle(e, doc, &reg), events)
}

fn texts(bar: &mut TransportBar, doc: &RackDoc) -> Vec<String> {
    let mut dl = DisplayList::new();
    bar.paint(&mut dl, doc, &registry());
    dl.texts().into_iter().map(str::to_string).collect()
}

fn wheel(pos: Point, dy: f64, modifiers: Modifiers) -> UiEvent {
    UiEvent::Wheel {
        pos,
        delta: Vec2::new(0.0, dy),
        modifiers,
    }
}

fn above(p: Point, px: f64) -> Point {
    Point::new(p.x, p.y - px)
}

fn shift() -> Modifiers {
    Modifiers {
        shift: true,
        ..Modifiers::NONE
    }
}

/// `(param, value, merge)` of every `SetParam` among the actions.
fn params(actions: &[UiAction]) -> Vec<(String, f32, Option<u64>)> {
    commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            Command::SetParam {
                key: device,
                param,
                value,
                merge,
            } => {
                assert_eq!(device, key("audio-track-1"));
                Some((param, value, merge))
            }
            _ => None,
        })
        .collect()
}

fn peaks(p: f32) -> Vec<(DeviceKey, f32)> {
    vec![(key("audio-track-1"), p)]
}

// ---------------------------------------------------------------- display

#[test]
fn the_section_is_drawn_only_with_a_track_and_room() {
    let doc = audio_doc();
    let mut none = TransportBar::new(atomik_ui::Theme::dark());
    none.set_bounds(Rect::new(0.0, 0.0, 1500.0, 44.0));
    assert!(!none.rec_input_visible());
    assert!(none.rec_input_point(RecInputPart::Gain).is_none());
    assert!(!texts(&mut none, &doc).iter().any(|s| s == "SAFE"));
    let (wide, _) = bar(1500.0, 0.0, false);
    assert!(wide.rec_input_visible());
    // Too narrow for the readouts and the section: it is not drawn rather than overlapping.
    let (narrow, _) = bar(600.0, 0.0, false);
    assert!(!narrow.rec_input_visible());
    // Nothing armed any more: gone again.
    let (mut b, _) = bar(1500.0, 0.0, false);
    b.set_rec_input(None);
    assert!(!b.rec_input_visible());
}

#[test]
fn it_shows_the_gain_the_switch_and_the_track_name() {
    let (mut b, doc) = bar(1500.0, 6.5, false);
    let t = texts(&mut b, &doc);
    for want in ["INPUT GAIN", "+6.5 dB", "SAFE", "GUITAR"] {
        assert!(t.iter().any(|s| s == want), "{want} in {t:?}");
    }
    let (mut b, doc) = bar(1500.0, -60.0, true);
    assert!(texts(&mut b, &doc).iter().any(|s| s == "OFF"));
    let (mut b, doc) = bar(1500.0, -12.0, true);
    assert!(texts(&mut b, &doc).iter().any(|s| s == "-12.0 dB"));
}

#[test]
fn the_clip_inspector_gives_way_and_the_two_never_overlap() {
    let doc = audio_doc();
    let reg = registry();
    let mut arr = audio_arrangement(&doc, &reg);
    arr.set_selected_clips(&[atomik_model::ClipId(10)]);
    for w in [1500.0, 1300.0, 1200.0, 1100.0, 1000.0] {
        let mut b = TransportBar::new(atomik_ui::Theme::dark());
        b.set_bounds(Rect::new(0.0, 0.0, w, 44.0));
        b.sync(&doc, &reg);
        b.set_inspector(arr.audio_inspector());
        b.set_rec_input(Some(input(0.0, false)));
        let Some(gain) = b.rec_input_point(RecInputPart::Gain) else {
            continue;
        };
        for f in b.inspector_fields() {
            let p = b.inspector_point(f).unwrap();
            assert!(p.x < gain.x - 40.0, "{w}: {f:?} at {p:?} over the section");
        }
    }
    // Without the section the inspector keeps all its fields at 1100 px; with it, fewer.
    let mut plain = TransportBar::new(atomik_ui::Theme::dark());
    plain.set_bounds(Rect::new(0.0, 0.0, 1100.0, 44.0));
    plain.sync(&doc, &reg);
    plain.set_inspector(arr.audio_inspector());
    let with = {
        let mut b = TransportBar::new(atomik_ui::Theme::dark());
        b.set_bounds(Rect::new(0.0, 0.0, 1100.0, 44.0));
        b.sync(&doc, &reg);
        b.set_inspector(arr.audio_inspector());
        b.set_rec_input(Some(input(0.0, false)));
        b.inspector_fields().len()
    };
    assert!(with < plain.inspector_fields().len());
    let _ = InspectorSource::None;
}

// ---------------------------------------------------------------- the gain

#[test]
fn a_drag_steps_half_a_decibel_per_five_pixels_as_one_merge_group() {
    let (mut b, doc) = bar(1500.0, 0.0, false);
    let p = b.rec_input_point(RecInputPart::Gain).unwrap();
    // 20 px up is four steps of 0.5 dB.
    let a = handle(&mut b, &doc, &[down(p), mv(above(p, 20.0)), up(p)]);
    let set = params(&a);
    assert_eq!(set.len(), 1, "{a:?}");
    assert_eq!((set[0].0.as_str(), set[0].1), (GAIN, 2.0));
    assert!(set[0].2.is_some(), "a drag merges");
    assert_eq!(a.last(), Some(&UiAction::EndMerge));
    // Moving on in steps sends every new value in the same merge group.
    let a = handle(
        &mut b,
        &doc,
        &[
            down(p),
            mv(above(p, 5.0)),
            mv(above(p, 10.0)),
            mv(above(p, 5.0)),
            up(p),
        ],
    );
    let set = params(&a);
    assert_eq!(set.iter().map(|s| s.1).collect::<Vec<_>>(), [0.5, 1.0, 0.5]);
    assert!(set.iter().all(|s| s.2 == set[0].2 && s.2.is_some()));
}

#[test]
fn shift_makes_the_step_a_tenth_and_the_range_ends_at_off_and_plus_24() {
    let (mut b, doc) = bar(1500.0, 0.0, false);
    let p = b.rec_input_point(RecInputPart::Gain).unwrap();
    let a = handle(
        &mut b,
        &doc,
        &[down(p), mv_with(above(p, 15.0), shift()), up(p)],
    );
    assert_eq!(params(&a).last().unwrap().1, 0.3);
    let a = handle(&mut b, &doc, &[down(p), mv(above(p, 5000.0)), up(p)]);
    assert_eq!(params(&a).last().unwrap().1, 24.0);
    let a = handle(
        &mut b,
        &doc,
        &[down(p), mv(Point::new(p.x, p.y + 5000.0)), up(p)],
    );
    assert_eq!(params(&a).last().unwrap().1, -60.0);
}

#[test]
fn a_press_that_does_not_move_sends_nothing() {
    let (mut b, doc) = bar(1500.0, 3.0, false);
    let p = b.rec_input_point(RecInputPart::Gain).unwrap();
    let a = handle(&mut b, &doc, &[down(p), up(p)]);
    assert!(a.is_empty(), "{a:?}");
    assert!(!b.is_dragging_rec_input());
}

#[test]
fn the_wheel_is_one_step_a_notch_and_stops_at_the_ends() {
    let (mut b, doc) = bar(1500.0, 0.0, false);
    let p = b.rec_input_point(RecInputPart::Gain).unwrap();
    let a = handle(&mut b, &doc, &[wheel(p, 1.0, Modifiers::NONE)]);
    assert_eq!(params(&a), [(GAIN.to_owned(), 0.5, None)]);
    let a = handle(&mut b, &doc, &[wheel(p, -1.0, shift())]);
    assert_eq!(params(&a), [(GAIN.to_owned(), -0.1, None)]);
    let (mut top, doc) = bar(1500.0, 24.0, false);
    let p = top.rec_input_point(RecInputPart::Gain).unwrap();
    assert!(handle(&mut top, &doc, &[wheel(p, 1.0, Modifiers::NONE)]).is_empty());
    let (mut bottom, doc) = bar(1500.0, -60.0, false);
    let p = bottom.rec_input_point(RecInputPart::Gain).unwrap();
    assert!(handle(&mut bottom, &doc, &[wheel(p, -1.0, Modifiers::NONE)]).is_empty());
}

#[test]
fn a_double_click_sets_zero_decibels() {
    let (mut b, doc) = bar(1500.0, 9.5, false);
    let p = b.rec_input_point(RecInputPart::Gain).unwrap();
    let a = handle(
        &mut b,
        &doc,
        &[
            down(p),
            up(p),
            down_with(p, atomik_ui::PointerButton::Primary, 2),
            up(p),
        ],
    );
    assert_eq!(params(&a), [(GAIN.to_owned(), 0.0, None)]);
    let (mut zero, doc) = bar(1500.0, 0.0, false);
    let p = zero.rec_input_point(RecInputPart::Gain).unwrap();
    let a = handle(
        &mut zero,
        &doc,
        &[down_with(p, atomik_ui::PointerButton::Primary, 2)],
    );
    assert!(a.is_empty());
}

// ---------------------------------------------------------------- safe

#[test]
fn the_safe_button_toggles_clip_safe() {
    let (mut off, doc) = bar(1500.0, 0.0, false);
    let p = off.rec_input_point(RecInputPart::Safe).unwrap();
    let a = handle(&mut off, &doc, &[down(p), up(p)]);
    assert_eq!(params(&a), [(SAFE.to_owned(), 1.0, None)]);
    let (mut on, doc) = bar(1500.0, 0.0, true);
    let a = handle(&mut on, &doc, &[down(p), up(p)]);
    assert_eq!(params(&a), [(SAFE.to_owned(), 0.0, None)]);
}

// ---------------------------------------------------------------- the meter

#[test]
fn the_meter_follows_the_peak_with_a_hold() {
    let (mut b, _) = bar(1500.0, 0.0, false);
    assert_eq!(b.rec_level_db().0, -60.0);
    b.feed_rec_peaks(&peaks(0.5), 0.0);
    let (level, hold) = b.rec_level_db();
    assert!((level + 6.02).abs() < 0.01 && (hold + 6.02).abs() < 0.01);
    // Quiet again: the bar falls at the console's rate, the hold stays for its hold time.
    b.feed_rec_peaks(&peaks(0.0), 0.25);
    b.feed_rec_peaks(&peaks(0.0), 0.5);
    let (level, hold) = b.rec_level_db();
    assert!(
        (level + 18.02).abs() < 0.01,
        "24 dB/s for half a second: {level}"
    );
    assert!((hold + 6.02).abs() < 0.01, "held: {hold}");
    for t in [0.75, 1.0, 1.25, 1.5, 1.75, 2.0] {
        b.feed_rec_peaks(&peaks(0.0), t);
    }
    assert!(b.rec_level_db().1 < -6.5, "the hold falls after 1.5 s");
}

#[test]
fn the_clip_lamp_latches_until_the_meter_is_clicked() {
    let (mut b, doc) = bar(1500.0, 0.0, false);
    b.feed_rec_peaks(&peaks(0.99), 0.0);
    assert!(!b.rec_clip_latched());
    b.feed_rec_peaks(&peaks(1.0), 0.1);
    assert!(b.rec_clip_latched(), "full scale latches");
    for i in 0..60 {
        b.feed_rec_peaks(&peaks(0.0), 0.2 + f64::from(i) * 0.1);
    }
    assert!(b.rec_clip_latched(), "it does not time out");
    let p = b.rec_input_point(RecInputPart::Meter).unwrap();
    let a = handle(&mut b, &doc, &[down(p), up(p)]);
    assert!(a.is_empty(), "clearing the lamp is not a document edit");
    assert!(!b.rec_clip_latched());
    assert_eq!(b.rec_level_db().1, -60.0, "the hold is cleared too");
}

#[test]
fn the_latch_belongs_to_the_track_and_goes_when_it_is_disarmed() {
    let (mut b, _) = bar(1500.0, 0.0, false);
    let other = key("audio-track-2");
    b.feed_rec_peaks(&[(key("audio-track-1"), 2.0), (other.clone(), 0.1)], 0.0);
    assert!(b.rec_clip_latched());
    // Showing the other track: its own lamp, unlit.
    b.set_rec_input(Some(RecInput {
        device: other.clone(),
        name: "Vox".into(),
        gain_db: 0.0,
        clip_safe: false,
    }));
    assert!(!b.rec_clip_latched());
    // Back: still lit.
    b.set_rec_input(Some(input(0.0, false)));
    assert!(b.rec_clip_latched());
    // Disarmed (not in the list any more) and armed again: clean.
    b.feed_rec_peaks(&[(other, 0.1)], 1.0);
    b.feed_rec_peaks(&[(key("audio-track-1"), 0.1)], 2.0);
    assert!(!b.rec_clip_latched());
}

#[test]
fn a_stalled_frame_does_not_make_the_meter_jump_and_nan_reads_silence() {
    let (mut b, _) = bar(1500.0, 0.0, false);
    b.feed_rec_peaks(&peaks(1.0), 0.0);
    b.feed_rec_peaks(&peaks(0.0), 100.0);
    let (level, _) = b.rec_level_db();
    assert!(
        level > -7.0,
        "one frame is at most a quarter second: {level}"
    );
    b.feed_rec_peaks(&peaks(f32::NAN), 100.1);
    assert!(b.rec_level_db().0.is_finite());
}

#[test]
fn the_meter_repaints_only_when_what_is_drawn_changes() {
    let (mut b, doc) = bar(1500.0, 0.0, false);
    let _ = texts(&mut b, &doc);
    assert!(!b.needs_repaint());
    b.feed_rec_peaks(&peaks(0.0), 0.0);
    b.feed_rec_peaks(&peaks(0.0), 0.016);
    assert!(!b.needs_repaint(), "silence stays silence");
    b.feed_rec_peaks(&peaks(0.5), 0.032);
    assert!(b.needs_repaint());
}

// ---------------------------------------------------------------- tooltips

#[test]
fn every_part_has_a_tooltip_and_the_gain_names_the_track() {
    let (b, _) = bar(1500.0, 0.0, false);
    for part in [RecInputPart::Gain, RecInputPart::Safe, RecInputPart::Meter] {
        let p = b.rec_input_point(part).unwrap();
        let h = b.help_at(p).unwrap_or_else(|| panic!("{part:?}"));
        assert!(!h.text.is_empty(), "{part:?}");
    }
    let gain = b
        .help_at(b.rec_input_point(RecInputPart::Gain).unwrap())
        .unwrap();
    assert!(
        gain.text.contains("Guitar") && gain.text.contains("monitor"),
        "{}",
        gain.text
    );
    let safe = b
        .help_at(b.rec_input_point(RecInputPart::Safe).unwrap())
        .unwrap();
    assert!(safe.text.contains("0 dBFS"), "{}", safe.text);
    let meter = b
        .help_at(b.rec_input_point(RecInputPart::Meter).unwrap())
        .unwrap();
    assert!(meter.text.contains("click"), "{}", meter.text);
}

// ---------------------------------------------------------------- the latch shared with the arrangement

#[test]
fn a_click_on_the_meter_is_reported_and_a_clear_from_outside_is_silent() {
    let (mut b, doc) = bar(1500.0, 0.0, false);
    b.feed_rec_peaks(&peaks(2.0), 0.0);
    assert!(b.take_rec_clears().is_empty());
    let p = b.rec_input_point(RecInputPart::Meter).unwrap();
    handle(&mut b, &doc, &[down(p), up(p)]);
    assert!(!b.rec_clip_latched());
    assert_eq!(b.take_rec_clears(), vec![key("audio-track-1")]);
    assert!(b.take_rec_clears().is_empty(), "taken once");
    // The arrangement header's meter was clicked: the latch goes, nothing is reported back.
    b.feed_rec_peaks(&peaks(2.0), 0.1);
    assert!(b.rec_clip_latched());
    b.clear_rec_latch(&key("audio-track-1"));
    assert!(!b.rec_clip_latched());
    assert_eq!(b.rec_level_db().1, -60.0, "the hold goes with it");
    assert!(b.take_rec_clears().is_empty(), "not an echo");
    b.clear_rec_latch(&key("nope"));
}
