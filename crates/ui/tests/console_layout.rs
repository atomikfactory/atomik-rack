//! Console strip and master-section layout maths (headless).

mod support;

use atomik_core::{ParamDescriptor, ParamId, Taper, Unit};
use atomik_device_sdk::DeviceDescriptor;
use atomik_ui::console_layout::{
    CtrlKind, FADER_MAX_H, FADER_MIN_H, Folds, HEADER_H, MASTER_W, PlacedCtrl, STRIP_SECTIONS,
    STRIP_W, SectionId, UPPER_MIN_H, is_hidden_param, master_layout, strip_layout,
};
use atomik_ui::kurbo::Rect;
use support::console::{CHANNEL, CHANNEL_PARAMS, MASTER};

const H: f64 = 700.0;

fn overlaps(a: &Rect, b: &Rect) -> bool {
    a.x0 < b.x1 - 1e-9 && b.x0 < a.x1 - 1e-9 && a.y0 < b.y1 - 1e-9 && b.y0 < a.y1 - 1e-9
}

fn inside(r: &Rect, outer: &Rect) -> bool {
    r.x0 >= outer.x0 - 1e-9
        && r.y0 >= outer.y0 - 1e-9
        && r.x1 <= outer.x1 + 1e-9
        && r.y1 <= outer.y1 + 1e-9
}

fn visible_keys(d: &DeviceDescriptor) -> Vec<&'static str> {
    let mut v: Vec<&str> = d
        .params
        .iter()
        .map(|p| p.key)
        .filter(|k| !is_hidden_param(k))
        .collect();
    v.sort_unstable();
    v
}

fn placed_keys(controls: &[PlacedCtrl]) -> Vec<&'static str> {
    let mut v: Vec<&str> = controls.iter().map(|c| c.param).collect();
    v.sort_unstable();
    v
}

fn assert_no_overlaps(what: &str, rects: &[(String, Rect)]) {
    for (i, (na, a)) in rects.iter().enumerate() {
        for (nb, b) in &rects[i + 1..] {
            assert!(!overlaps(a, b), "{what}: {na} {a:?} overlaps {nb} {b:?}");
        }
    }
}

#[test]
fn hidden_params_are_the_internal_namespace() {
    assert!(is_hidden_param("internal.solo_silence"));
    assert!(!is_hidden_param("solo"));
    assert!(!is_hidden_param("internalish"));
}

#[test]
fn every_visible_channel_param_is_placed_once_and_hidden_ones_never() {
    let l = strip_layout(&CHANNEL, Folds::default(), H);
    assert_eq!(placed_keys(&l.controls), visible_keys(&CHANNEL));
    assert!(l.controls.iter().all(|c| !c.param.starts_with("internal.")));
    // Widget choice follows the strip.
    let kind = |k: &str| l.controls.iter().find(|c| c.param == k).unwrap().kind;
    assert_eq!(kind("fader"), CtrlKind::Fader);
    assert_eq!(kind("mute"), CtrlKind::Button);
    assert_eq!(kind("eq.hf.bell"), CtrlKind::Button);
    assert_eq!(kind("eq.hmf.q"), CtrlKind::Knob);
    assert_eq!(kind("send5.level"), CtrlKind::Slider);
    assert_eq!(kind("pan"), CtrlKind::Knob);
}

#[test]
fn sections_run_top_to_bottom_in_s04_order() {
    let l = strip_layout(&CHANNEL, Folds::default(), H);
    let ids: Vec<SectionId> = l.sections.iter().map(|s| s.id).collect();
    assert_eq!(
        ids,
        [
            SectionId::Input,
            SectionId::Filters,
            SectionId::Dynamics,
            SectionId::Eq,
            SectionId::Inserts,
            SectionId::Sends,
            SectionId::Pan,
            SectionId::Fader,
            SectionId::MuteSolo,
            SectionId::Name,
        ]
    );
    // Each control sits in its own section, below the header.
    for c in &l.controls {
        let s = l.sections.iter().find(|s| s.id == c.section).unwrap();
        assert!(
            inside(&c.rect, &s.rect),
            "{} {:?} not in {:?}",
            c.param,
            c.rect,
            s.id
        );
        assert!(
            c.rect.y0 >= s.header.y1 - 1e-9 || s.id == SectionId::Name,
            "{}",
            c.param
        );
    }
    // Upper sections scroll; pan, fader, mute/solo and the name are pinned to the bottom.
    for s in &l.sections {
        if l.is_pinned(s.id) {
            assert!(s.rect.y0 >= l.pinned_top - 1e-9, "{:?}", s.id);
        }
    }
    let first_pinned = l.sections.iter().position(|s| l.is_pinned(s.id)).unwrap();
    assert!(
        l.sections[..first_pinned]
            .iter()
            .all(|s| !l.is_pinned(s.id))
    );
    assert!((l.sections[first_pinned].rect.y0 - l.pinned_top).abs() < 1e-9);
    assert!(
        l.sections[0].rect.y0.abs() < 1e-9,
        "upper part starts at the top"
    );
    let name = l.sections.last().unwrap();
    assert!((name.rect.y1 - H).abs() < 1e-9);
}

#[test]
fn nothing_overlaps_and_everything_is_inside_the_strip() {
    for folds in [Folds::default(), Folds::all(true)] {
        let l = strip_layout(&CHANNEL, folds, H);
        let strip = Rect::new(0.0, 0.0, STRIP_W, f64::INFINITY);
        for region in [false, true] {
            let mut rects: Vec<(String, Rect)> = l
                .controls
                .iter()
                .filter(|c| l.is_pinned(c.section) == region)
                .map(|c| (c.param.to_string(), c.rect))
                .collect();
            for (n, r, pinned) in [
                ("gr", l.gr_meter, false),
                ("latency", l.latency, true),
                ("meter", l.meter, true),
                ("lcd", l.fader_lcd, true),
            ] {
                if let Some(r) = r
                    && pinned == region
                {
                    rects.push((n.to_string(), r));
                }
            }
            for (n, r) in &rects {
                assert!(inside(r, &strip), "{n} {r:?}");
            }
            assert_no_overlaps("controls", &rects);
            let secs: Vec<(String, Rect)> = l
                .sections
                .iter()
                .filter(|s| l.is_pinned(s.id) == region)
                .map(|s| (format!("{:?}", s.id), s.rect))
                .collect();
            assert_no_overlaps("sections", &secs);
        }
    }
}

#[test]
fn folding_a_section_collapses_it_to_its_header() {
    let open = strip_layout(&CHANNEL, Folds::default(), H);
    let mut folds = Folds::default();
    folds.set(SectionId::Eq, true);
    let shut = strip_layout(&CHANNEL, folds, H);
    let eq_open = open
        .sections
        .iter()
        .find(|s| s.id == SectionId::Eq)
        .unwrap();
    let eq_shut = shut
        .sections
        .iter()
        .find(|s| s.id == SectionId::Eq)
        .unwrap();
    assert!(eq_shut.folded && !eq_open.folded);
    assert!((eq_shut.rect.height() - HEADER_H).abs() < 1e-9);
    let saved = eq_open.rect.height() - HEADER_H;
    assert!(saved > 100.0);
    assert!((open.upper_height - shut.upper_height - saved).abs() < 1e-9);
    assert!(shut.controls.iter().all(|c| c.section != SectionId::Eq));
    // The sections below move up by the same amount.
    let y = |l: &atomik_ui::console_layout::StripLayout, id| {
        l.sections.iter().find(|s| s.id == id).unwrap().rect.y0
    };
    assert!((y(&open, SectionId::Sends) - y(&shut, SectionId::Sends) - saved).abs() < 1e-9);
    // Pinned sections do not move when an upper one folds.
    assert_eq!(open.pinned_top, shut.pinned_top);
}

#[test]
fn folding_pinned_sections_gives_room_to_the_upper_ones() {
    let open = strip_layout(&CHANNEL, Folds::default(), H);
    let mut folds = Folds::default();
    folds.set(SectionId::Fader, true);
    folds.set(SectionId::Pan, true);
    let shut = strip_layout(&CHANNEL, folds, H);
    assert!(shut.pinned_top > open.pinned_top + 100.0);
    assert!(
        shut.controls
            .iter()
            .all(|c| c.param != "fader" && c.param != "pan")
    );
    assert!(shut.meter.is_none());
    // Fold all: every foldable section is a header; the name stays.
    let all = strip_layout(&CHANNEL, Folds::all(true), H);
    let upper = STRIP_SECTIONS
        .iter()
        .filter(|s| !all.is_pinned(**s))
        .count();
    assert!(all.upper_height <= upper as f64 * (HEADER_H + 4.0));
    assert!(all.controls.is_empty(), "{:?}", placed_keys(&all.controls));
    assert!(all.sections.iter().any(|s| s.id == SectionId::Name));
    assert!(Folds::all(true).any_folded() && !Folds::default().any_folded());
}

#[test]
fn the_fader_grows_with_the_pane_within_limits() {
    let fader_h = |h: f64| {
        let l = strip_layout(&CHANNEL, Folds::default(), h);
        l.controls
            .iter()
            .find(|c| c.param == "fader")
            .unwrap()
            .rect
            .height()
    };
    assert!((fader_h(200.0) - FADER_MIN_H).abs() < 1e-9);
    assert!((fader_h(3000.0) - FADER_MAX_H).abs() < 1e-9);
    assert!(fader_h(700.0) > fader_h(500.0));
}

#[test]
fn unknown_params_land_in_a_more_section() {
    static EXTRA: [ParamDescriptor; 3] = [
        ParamDescriptor {
            id: ParamId(0),
            key: "fader",
            name: "Fader",
            min: 0.0,
            max: 1.0,
            default: 0.75,
            taper: Taper::Linear,
            unit: Unit::None,
            discrete: false,
        },
        ParamDescriptor {
            id: ParamId(1),
            key: "tape.drive",
            name: "Drive",
            min: 0.0,
            max: 1.0,
            default: 0.0,
            taper: Taper::Linear,
            unit: Unit::Percent,
            discrete: false,
        },
        ParamDescriptor {
            id: ParamId(2),
            key: "internal.meter_mode",
            name: "Meter mode",
            min: 0.0,
            max: 1.0,
            default: 0.0,
            taper: Taper::Linear,
            unit: Unit::Toggle,
            discrete: true,
        },
    ];
    static D: DeviceDescriptor = DeviceDescriptor {
        params: &EXTRA,
        ..CHANNEL
    };
    let l = strip_layout(&D, Folds::default(), H);
    assert_eq!(placed_keys(&l.controls), ["fader", "tape.drive"]);
    let drive = l.controls.iter().find(|c| c.param == "tape.drive").unwrap();
    assert_eq!(drive.section, SectionId::More);
    // Sections without any params present are left out.
    assert!(l.sections.iter().all(|s| s.id != SectionId::Eq));
    let _ = CHANNEL_PARAMS.len();
}

#[test]
fn master_places_every_visible_param_returns_and_meters() {
    let l = master_layout(&MASTER, H);
    assert_eq!(placed_keys(&l.controls), visible_keys(&MASTER));
    assert_eq!(l.return_pickers.len(), 8);
    for (i, (slot, _)) in l.return_pickers.iter().enumerate() {
        assert_eq!(*slot, i);
    }
    assert!(l.gr_meter.is_some() && l.meter.is_some());
    let area = Rect::new(0.0, 0.0, MASTER_W, H);
    let mut rects: Vec<(String, Rect)> = l
        .controls
        .iter()
        .map(|c| (c.param.to_string(), c.rect))
        .collect();
    rects.extend(
        l.return_pickers
            .iter()
            .map(|(k, r)| (format!("picker{k}"), *r)),
    );
    rects.push(("clear solo".into(), l.clear_solo));
    rects.push(("clear mute".into(), l.clear_mute));
    rects.push(("name".into(), l.name));
    for (n, r) in [("gr", l.gr_meter), ("meter", l.meter), ("lcd", l.fader_lcd)] {
        rects.push((n.into(), r.unwrap()));
    }
    for (n, r) in &rects {
        assert!(inside(r, &area), "{n} {r:?}");
    }
    assert_no_overlaps("master", &rects);
    // Return rows line up: level, mute and pan of slot k share a row with picker k.
    for (k, (_, picker)) in l.return_pickers.iter().enumerate() {
        for what in ["level", "mute", "pan"] {
            let key = format!("return{}.{what}", k + 1);
            let c = l.controls.iter().find(|c| c.param == key).unwrap();
            assert!(
                c.rect.y0 < picker.center().y && c.rect.y1 > picker.center().y,
                "{key}"
            );
        }
    }
    // The master fader lines up with the channel faders.
    let strip = strip_layout(&CHANNEL, Folds::default(), H);
    let f = |cs: &[PlacedCtrl]| cs.iter().find(|c| c.param == "fader").unwrap().rect;
    assert!((f(&l.controls).y0 - f(&strip.controls).y0).abs() < 1e-9);
    assert!((f(&l.controls).y1 - f(&strip.controls).y1).abs() < 1e-9);
}

/// Strip-area height of a console pane `pane` px tall (toolbar and scrollbar excluded).
fn area(pane: f64) -> f64 {
    pane - atomik_ui::console::TOOLBAR_H - atomik_ui::console::SCROLLBAR_H
}

#[test]
fn short_panes_keep_a_usable_scroll_region_and_fader() {
    for pane in [300.0, 380.0, 600.0, 1000.0] {
        let h = area(pane);
        let l = strip_layout(&CHANNEL, Folds::default(), h);
        assert!(
            l.pinned_top >= UPPER_MIN_H - 1e-9,
            "{pane}: upper {}",
            l.pinned_top
        );
        let fader = l.controls.iter().find(|c| c.param == "fader").unwrap();
        assert!(
            fader.rect.height() >= FADER_MIN_H - 1e-9,
            "{pane}: fader {}",
            fader.rect.height()
        );
        // Everything pinned sits below the boundary and inside the strip; nothing overlaps.
        let mut pinned: Vec<(String, Rect)> = l
            .controls
            .iter()
            .filter(|c| l.is_pinned(c.section))
            .map(|c| (c.param.to_string(), c.rect))
            .collect();
        for (n, r) in [
            ("meter", l.meter),
            ("lcd", l.fader_lcd),
            ("latency", l.latency),
        ] {
            pinned.push((n.into(), r.unwrap()));
        }
        for (n, r) in &pinned {
            assert!(
                r.y0 >= l.pinned_top - 1e-9 && r.y1 <= h + 1e-9,
                "{pane}: {n} {r:?}"
            );
        }
        assert_no_overlaps(&format!("pinned at {pane}"), &pinned);
        let upper: Vec<(String, Rect)> = l
            .controls
            .iter()
            .filter(|c| !l.is_pinned(c.section))
            .map(|c| (c.param.to_string(), c.rect))
            .collect();
        assert_no_overlaps(&format!("upper at {pane}"), &upper);
        let secs: Vec<(String, Rect)> = l
            .sections
            .iter()
            .filter(|s| l.is_pinned(s.id))
            .map(|s| (format!("{:?}", s.id), s.rect))
            .collect();
        assert_no_overlaps(&format!("pinned sections at {pane}"), &secs);
        // The master fits the same pane.
        let m = master_layout(&MASTER, h);
        let mf = m.controls.iter().find(|c| c.param == "fader").unwrap();
        assert!(mf.rect.height() >= FADER_MIN_H - 1e-9);
        assert!(
            (mf.rect.y0 - fader.rect.y0).abs() < 1e-9,
            "{pane}: master fader aligns"
        );
        let mut col: Vec<(String, Rect)> = m
            .output
            .iter()
            .map(|(k, r)| (format!("{k:?}"), *r))
            .collect();
        col.push((
            "fader section".into(),
            m.sections
                .iter()
                .find(|s| s.id == SectionId::Fader)
                .unwrap()
                .rect,
        ));
        assert_no_overlaps(&format!("master fader column at {pane}"), &col);
    }
}

#[test]
fn pan_scrolls_with_the_upper_sections_in_short_panes() {
    let short = strip_layout(&CHANNEL, Folds::default(), area(380.0));
    assert!(!short.is_pinned(SectionId::Pan));
    let pan = short
        .sections
        .iter()
        .find(|s| s.id == SectionId::Pan)
        .unwrap();
    let sends = short
        .sections
        .iter()
        .find(|s| s.id == SectionId::Sends)
        .unwrap();
    assert!(pan.rect.y0 > sends.rect.y0, "pan is the last upper section");
    // The fader readout moves into the fader header.
    let fader = short
        .sections
        .iter()
        .find(|s| s.id == SectionId::Fader)
        .unwrap();
    let lcd = short.fader_lcd.unwrap();
    assert!(lcd.y0 >= fader.header.y0 && lcd.y1 <= fader.header.y1);
    let tall = strip_layout(&CHANNEL, Folds::default(), area(1000.0));
    assert!(tall.is_pinned(SectionId::Pan));
    let fader = tall
        .sections
        .iter()
        .find(|s| s.id == SectionId::Fader)
        .unwrap();
    assert!(tall.fader_lcd.unwrap().y0 >= fader.header.y1);
}

#[test]
fn the_latency_readout_sits_in_the_name_plate() {
    let l = strip_layout(&CHANNEL, Folds::all(true), H);
    let r = l.latency.unwrap();
    assert!(r.y0 >= l.name.y0 && r.y1 <= l.name.y1 && r.x1 <= l.name.x1);
    // The latency-adjust knob stays in the inserts section.
    let open = strip_layout(&CHANNEL, Folds::default(), H);
    let adj = open
        .controls
        .iter()
        .find(|c| c.param == "latency.adjust")
        .unwrap();
    assert_eq!(adj.section, SectionId::Inserts);
}

#[test]
fn the_master_fills_the_space_above_its_fader_with_outputs() {
    use atomik_ui::console_layout::OutputItem;
    let l = master_layout(&MASTER, area(600.0));
    let items: Vec<OutputItem> = l.output.iter().map(|(k, _)| *k).collect();
    assert_eq!(
        items,
        [
            OutputItem::PeakLeft,
            OutputItem::PeakRight,
            OutputItem::BusGr,
            OutputItem::Status
        ]
    );
    let out = l
        .sections
        .iter()
        .find(|s| s.id == SectionId::Output)
        .unwrap();
    assert!(out.rect.x0 >= l.fader_x - 1e-9);
    for (_, r) in &l.output {
        assert!(r.y1 <= l.pinned_top + 1e-9);
    }
    // In the shortest pane, rows that do not fit are dropped rather than overlapping.
    let s = master_layout(&MASTER, area(300.0));
    for (_, r) in &s.output {
        assert!(r.y1 <= s.pinned_top + 1e-9);
    }
}

#[test]
fn tall_panes_move_monitoring_above_the_master_fader() {
    let tall = master_layout(&MASTER, area(1000.0));
    let mon = tall
        .sections
        .iter()
        .find(|s| s.id == SectionId::Monitor)
        .unwrap();
    assert!(
        mon.rect.x0 >= tall.fader_x - 1e-9,
        "monitor joins the fader column"
    );
    assert!(mon.rect.y1 <= tall.pinned_top + 1e-9);
    let level = tall
        .controls
        .iter()
        .find(|c| c.param == "monitor.level")
        .unwrap();
    assert!(level.rect.x0 >= tall.fader_x);
    let short = master_layout(&MASTER, area(380.0));
    let mon = short
        .sections
        .iter()
        .find(|s| s.id == SectionId::Monitor)
        .unwrap();
    assert!(
        mon.rect.x1 <= short.fader_x,
        "no room: monitor stays in the bus column"
    );
    // Every visible param is still placed exactly once either way.
    for l in [&tall, &short] {
        assert_eq!(placed_keys(&l.controls), visible_keys(&MASTER));
    }
}
