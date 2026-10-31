//! The second cluster of the transport bar: held Rewind and Forward, tap tempo, the position,
//! time and locator readouts you can type into, the click-level slider, Q Rec with its grid,
//! and the room it all takes at 1000 px and wide.

mod support;

use atomik_model::{RackDoc, SongCommand};
use atomik_ui::kurbo::{Point, Rect, Vec2};
use atomik_ui::quantize_panel::{GRIDS, QuantizeAction};
use atomik_ui::{
    ClusterPart, DisplayList, Key, Modifiers, REPEAT_DELAY, REPEAT_EVERY, RecInput, RecInputPart,
    TransportAction, TransportBar, TransportState, UiAction, UiEvent,
};
use support::audio::*;
use support::song::*;
use support::*;

fn bar_of(width: f64, doc: &RackDoc) -> TransportBar {
    let mut b = TransportBar::new(atomik_ui::Theme::dark());
    b.set_bounds(Rect::new(0.0, 0.0, width, 44.0));
    b.sync(doc, &registry());
    b
}

fn with_rec(width: f64) -> (TransportBar, RackDoc) {
    let doc = audio_doc();
    let mut b = bar_of(width, &doc);
    b.set_rec_input(Some(RecInput {
        device: key("audio-track-1"),
        name: "Guitar".into(),
        gain_db: 0.0,
        clip_safe: false,
    }));
    (b, doc)
}

fn handle(b: &mut TransportBar, doc: &RackDoc, events: &[UiEvent]) -> Vec<UiAction> {
    let reg = registry();
    run(|e| b.handle(e, doc, &reg), events)
}

fn click(b: &mut TransportBar, doc: &RackDoc, p: Point) -> Vec<UiAction> {
    handle(b, doc, &[down(p), up(p)])
}

fn double_click(b: &mut TransportBar, doc: &RackDoc, at: impl Fn(&TransportBar) -> Point) {
    let p = at(b);
    handle(b, doc, &[down_mods(p, Modifiers::NONE, 2), up(p)]);
}

fn key_event(key: Key) -> UiEvent {
    UiEvent::Key {
        key,
        modifiers: Modifiers::NONE,
        pressed: true,
    }
}

fn typed(b: &mut TransportBar, doc: &RackDoc, text: &str) -> Vec<UiAction> {
    let mut events: Vec<UiEvent> = text.chars().map(|c| key_event(Key::Char(c))).collect();
    events.push(key_event(Key::Enter));
    handle(b, doc, &events)
}

fn wheel(pos: Point, dy: f64) -> UiEvent {
    UiEvent::Wheel {
        pos,
        delta: Vec2::new(0.0, dy),
        modifiers: Modifiers::NONE,
    }
}

const ALL_PARTS: [ClusterPart; 9] = [
    ClusterPart::Rewind,
    ClusterPart::Forward,
    ClusterPart::Tap,
    ClusterPart::GoLeft,
    ClusterPart::GoRight,
    ClusterPart::Click,
    ClusterPart::QRec,
    ClusterPart::Grid,
    ClusterPart::Quantize,
];

/// Every rectangle the cluster shows, with a name.
fn rects(b: &TransportBar) -> Vec<(String, Rect)> {
    let mut v: Vec<(String, Rect)> = ALL_PARTS
        .iter()
        .chain([&ClusterPart::More])
        .filter_map(|p| b.cluster_rect(*p).map(|r| (format!("{p:?}"), r)))
        .collect();
    for right in [false, true] {
        if let Some(r) = b.locator_rect(right) {
            v.push((format!("locator {right}"), r));
        }
    }
    v
}

fn assert_no_overlap(b: &TransportBar, width: f64) {
    let r = rects(b);
    let meter_end = b.meter_center().x + 23.0;
    // The input strip starts 42 px left of the centre of its gain box.
    let right_limit = b
        .rec_input_point(RecInputPart::Gain)
        .map_or(width, |g| g.x - 42.0);
    for (i, (na, a)) in r.iter().enumerate() {
        assert!(a.x0 >= meter_end, "{na} starts inside the readouts: {a:?}");
        assert!(a.x1 <= right_limit, "{na} runs into the input strip: {a:?}");
        assert!(a.x1 <= width, "{na} is outside the bar");
        for (nb, c) in &r[i + 1..] {
            let inter = a.intersect(*c);
            assert!(
                inter.width() <= 0.0 || inter.height() <= 0.0,
                "{na} {a:?} overlaps {nb} {c:?}"
            );
        }
    }
}

#[test]
fn nothing_overlaps_at_1000_px_and_wide() {
    for width in [1000.0, 1100.0, 1300.0, 1600.0, 1920.0] {
        let (b, _) = with_rec(width);
        assert_no_overlap(&b, width);
        let plain = bar_of(width, &song_doc());
        assert_no_overlap(&plain, width);
    }
    // The strip is still visible at 1000 px, and the overflow button is how the rest is reached.
    let (b, _) = with_rec(1000.0);
    assert!(b.rec_input_visible());
    assert!(b.cluster_rect(ClusterPart::More).is_some());
    // Wide: every control, no overflow.
    let (b, _) = with_rec(1920.0);
    for p in ALL_PARTS {
        assert!(b.cluster_rect(p).is_some(), "{p:?}");
    }
    assert!(b.cluster_rect(ClusterPart::More).is_none());
    assert!(b.locator_rect(false).is_some() && b.locator_rect(true).is_some());
}

#[test]
fn the_overflow_button_opens_the_transport_menu() {
    let (mut b, doc) = with_rec(1000.0);
    let p = b.cluster_rect(ClusterPart::More).unwrap().center();
    let a = click(&mut b, &doc, p);
    assert!(matches!(
        a.as_slice(),
        [UiAction::ContextMenu {
            target: atomik_ui::ContextTarget::TransportMore,
            ..
        }]
    ));
}

#[test]
fn buttons_emit_their_actions() {
    let doc = song_doc();
    let mut b = bar_of(1920.0, &doc);
    for (part, want) in [
        (ClusterPart::Tap, TransportAction::TapTempo),
        (ClusterPart::GoLeft, TransportAction::GoLocator(false)),
        (ClusterPart::GoRight, TransportAction::GoLocator(true)),
        (ClusterPart::Forward, TransportAction::Bars(1)),
        (ClusterPart::Rewind, TransportAction::Bars(-1)),
    ] {
        let p = b.cluster_rect(part).unwrap().center();
        let a = click(&mut b, &doc, p);
        assert_eq!(a, vec![UiAction::Transport(want)], "{part:?}");
    }
    let p = b.cluster_rect(ClusterPart::Quantize).unwrap().center();
    assert_eq!(
        click(&mut b, &doc, p),
        vec![UiAction::Quantize(QuantizeAction::ApplyLast)]
    );
}

#[test]
fn a_held_rewind_repeats_and_speeds_up_on_an_injected_clock() {
    let doc = song_doc();
    let mut b = bar_of(1920.0, &doc);
    let p = b.cluster_rect(ClusterPart::Rewind).unwrap().center();
    let first = handle(&mut b, &doc, &[down(p)]);
    assert_eq!(first, vec![UiAction::Transport(TransportAction::Bars(-1))]);
    assert!(b.is_repeating());
    // The first tick only notes the press; nothing before the delay.
    assert!(b.tick(100.0).is_empty());
    assert!(b.tick(100.0 + REPEAT_DELAY - 0.01).is_empty());
    let mut bars = 0;
    let mut steps = Vec::new();
    let mut t = 100.0 + REPEAT_DELAY;
    while bars > -100 {
        for a in b.tick(t) {
            match a {
                UiAction::Transport(TransportAction::Bars(n)) => {
                    assert!(n < 0);
                    bars += n;
                    steps.push(-n);
                }
                other => panic!("{other:?}"),
            }
        }
        t += REPEAT_EVERY;
    }
    assert_eq!(steps[0], 1, "it starts with single bars");
    assert!(steps.windows(2).all(|w| w[1] >= w[0]), "{steps:?}");
    assert!(steps.iter().any(|s| *s > 1), "it speeds up: {steps:?}");
    // Letting go stops it.
    handle(&mut b, &doc, &[up(p)]);
    assert!(!b.is_repeating());
    assert!(b.tick(t + 5.0).is_empty());
}

#[test]
fn a_held_button_does_not_repeat_while_following_a_clock() {
    let doc = song_doc();
    let mut b = bar_of(1920.0, &doc);
    b.set_state(TransportState {
        following: true,
        ..TransportState::default()
    });
    let p = b.cluster_rect(ClusterPart::Forward).unwrap().center();
    handle(&mut b, &doc, &[down(p)]);
    assert!(!b.is_repeating());
    assert!(b.tick(1.0).is_empty() && b.tick(3.0).is_empty());
}

#[test]
fn typing_a_position_or_a_time_moves_the_playhead_exactly() {
    let doc = song_doc();
    let mut b = bar_of(1920.0, &doc);
    double_click(&mut b, &doc, |b| b.position_rect().center());
    assert!(b.is_editing_readout());
    let a = typed(&mut b, &doc, "5.1.0");
    assert_eq!(
        a,
        vec![UiAction::Transport(TransportAction::Locate(4 * BAR))]
    );
    assert!(!b.is_editing_readout());
    double_click(&mut b, &doc, |b| b.time_rect().center());
    let a = typed(&mut b, &doc, "00:12.500");
    // 120 BPM: 2 beats a second.
    assert_eq!(
        a,
        vec![UiAction::Transport(TransportAction::Locate(25 * PPQ))]
    );
    // Escape drops the text.
    double_click(&mut b, &doc, |b| b.time_rect().center());
    handle(&mut b, &doc, &[key_event(Key::Escape)]);
    assert!(!b.is_editing_readout());
}

#[test]
fn editing_a_locator_sets_the_loop_in_one_command() {
    let mut doc = song_doc();
    doc.song.loop_range = (0, 4 * BAR);
    doc.song.loop_enabled = true;
    let mut b = bar_of(1920.0, &doc);
    double_click(&mut b, &doc, |b| b.locator_rect(false).unwrap().center());
    let a = typed(&mut b, &doc, "2.1.0");
    assert_eq!(
        song_commands(&a),
        vec![SongCommand::SetLoop {
            range: (BAR, 4 * BAR),
            enabled: true
        }]
    );
    // A value that would cross the other locator is refused.
    double_click(&mut b, &doc, |b| b.locator_rect(true).unwrap().center());
    assert!(typed(&mut b, &doc, "1.1.0").is_empty());
    double_click(&mut b, &doc, |b| b.locator_rect(true).unwrap().center());
    let a = typed(&mut b, &doc, "9");
    assert_eq!(
        song_commands(&a),
        vec![SongCommand::SetLoop {
            range: (0, 8 * BAR),
            enabled: true
        }]
    );
    let mut dl = DisplayList::new();
    b.paint(&mut dl, &doc, &registry());
    assert!(dl.texts().contains(&"1.1.00000"));
}

#[test]
fn the_click_slider_drags_scrolls_and_resets() {
    let doc = song_doc();
    let mut b = bar_of(1920.0, &doc);
    let r = b.cluster_rect(ClusterPart::Click).unwrap();
    let mid = r.center();
    let a = handle(&mut b, &doc, &[down(mid)]);
    let UiAction::SetMetronomeGain(g) = a[0] else {
        panic!("{a:?}");
    };
    assert!((g - 0.5).abs() < 0.06, "{g}");
    assert!(b.is_dragging());
    let a = handle(&mut b, &doc, &[mv(Point::new(r.x1 + 40.0, mid.y)), up(mid)]);
    assert_eq!(a, vec![UiAction::SetMetronomeGain(1.0)]);
    assert!(!b.is_dragging());
    let a = handle(&mut b, &doc, &[wheel(mid, -1.0)]);
    assert_eq!(a.len(), 1);
    let a = handle(&mut b, &doc, &[down_mods(mid, Modifiers::NONE, 2), up(mid)]);
    assert_eq!(a, vec![UiAction::SetMetronomeGain(0.5)]);
}

#[test]
fn q_rec_toggles_and_the_grid_picks_any_quantize_panel_grid() {
    let doc = song_doc();
    let mut b = bar_of(1920.0, &doc);
    let q = b.cluster_rect(ClusterPart::QRec).unwrap().center();
    // Off, 1/16 shown: the switch turns it on at 1/16.
    let a = click(&mut b, &doc, q);
    assert_eq!(a, vec![UiAction::SetInputQuantize(Some(GRIDS[3].1))]);
    let on = |grid: usize| TransportState {
        quantize: Some(GRIDS[grid].1),
        ..TransportState::default()
    };
    b.set_state(on(3));
    assert_eq!(b.q_grid_index(), 3);
    // On: scrolling the grid changes it (1/16 triplet is the next one).
    let g = b.cluster_rect(ClusterPart::Grid).unwrap().center();
    let a = handle(&mut b, &doc, &[wheel(g, -1.0)]);
    assert_eq!(a.len(), 1);
    assert!(matches!(a[0], UiAction::SetInputQuantize(Some(_))));
    b.set_state(on(4));
    // Clicking the selector steps to the next grid.
    let a = click(&mut b, &doc, g);
    assert_eq!(a, vec![UiAction::SetInputQuantize(Some(GRIDS[5].1))]);
    // On: the switch turns it off, and the grid is remembered for next time.
    b.set_state(on(5));
    assert_eq!(
        click(&mut b, &doc, q),
        vec![UiAction::SetInputQuantize(None)]
    );
    b.set_state(TransportState::default());
    assert_eq!(b.q_grid_index(), 5);
    // Off: changing the grid sends nothing.
    assert!(click(&mut b, &doc, g).is_empty());
}

#[test]
fn the_cluster_shows_its_labels_and_tooltips() {
    let doc = song_doc();
    let mut b = bar_of(1920.0, &doc);
    let mut dl = DisplayList::new();
    b.paint(&mut dl, &doc, &registry());
    let t = dl.texts();
    for want in ["TAP", "CLICK", "GRID", "QUANT", "L", "R", "1/16"] {
        assert!(t.contains(&want), "{want} in {t:?}");
    }
    let r = b.cluster_rect(ClusterPart::Tap).unwrap();
    let h = b.help_at(r.center()).unwrap();
    assert_eq!(h.command, Some("transport.tap_tempo"));
}
