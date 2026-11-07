//! The energy lane of the arrangement: off by default, one row under the ruler that pushes the
//! lanes down when shown (and under the tempo lane when both are), a section's name and value and
//! its flag are drawn, a stale map says so in the header, a running measurement shows its
//! progress, and a press in the lane changes nothing.

mod support;

use atomik_model::energy_map::{
    EnergyMap, Flag, Measure, Role, Section, SectionEnergy, energy as energy_of,
};
use atomik_ui::arrangement::ENERGY_LANE_HEIGHT;
use atomik_ui::kurbo::Point;
use atomik_ui::tempo_lane::TEMPO_LANE_HEIGHT;
use atomik_ui::{ArrangementView, DisplayList, UiAction};
use std::sync::Arc;
use support::song::*;
use support::*;

fn section(name: &str, from_bar: i64, to_bar: i64, role: Role, level: f32) -> SectionEnergy {
    let measure = Measure {
        loudness_db: level,
        brightness_hz: 1200.0,
        width: 0.4,
        tracks: 3,
    };
    SectionEnergy {
        section: Section {
            name: name.into(),
            role,
            start: from_bar * BAR,
            end: to_bar * BAR,
        },
        energy: energy_of(&measure, 4),
        measure,
    }
}

fn map(flagged: bool) -> Arc<EnergyMap> {
    Arc::new(EnergyMap {
        sections: vec![
            section("Build", 0, 2, Role::Build, -12.0),
            section("Drop", 2, 4, Role::Drop, -24.0),
        ],
        flags: if flagged {
            vec![Flag::BuildLouderThanDrop { build: 0, drop: 1 }]
        } else {
            Vec::new()
        },
    })
}

fn view() -> ArrangementView {
    let (doc, reg) = (song_doc(), registry());
    arrangement(&doc, &reg)
}

fn texts(v: &mut ArrangementView) -> Vec<String> {
    let (doc, reg) = (song_doc(), registry());
    let mut dl = DisplayList::new();
    v.paint(&mut dl, &doc, &reg);
    dl.texts().into_iter().map(str::to_string).collect()
}

#[test]
fn the_lane_is_hidden_by_default_and_pushes_the_lanes_down_when_shown() {
    let mut v = view();
    assert!(!v.energy_lane_visible());
    assert_eq!(v.energy_lane_rect().height(), 0.0);
    let before = v.lane_rect(&key("bass"), 0).unwrap();
    v.set_energy_lane(true);
    assert!(v.energy_lane_visible());
    assert_eq!(v.energy_lane_rect().height(), ENERGY_LANE_HEIGHT);
    let after = v.lane_rect(&key("bass"), 0).unwrap();
    assert!((after.y0 - before.y0 - ENERGY_LANE_HEIGHT).abs() < 1e-9);
    v.set_energy_lane(false);
    let back = v.lane_rect(&key("bass"), 0).unwrap();
    assert!((back.y0 - before.y0).abs() < 1e-9);
}

#[test]
fn the_lane_sits_under_the_tempo_lane_when_both_are_shown() {
    let mut v = view();
    v.set_tempo_lane(true);
    let tempo = v.tempo_lane_rect();
    v.set_energy_lane(true);
    assert_eq!(
        v.tempo_lane_rect(),
        tempo,
        "the tempo lane stays where it was"
    );
    let lane = v.energy_lane_rect();
    assert!((lane.y0 - tempo.y1).abs() < 1e-9);
    assert!((lane.height() - ENERGY_LANE_HEIGHT).abs() < 1e-9);
    let first = v.lane_rect(&key("bass"), 0).unwrap();
    assert!((first.y0 - lane.y1).abs() < 1.0);
    assert!((tempo.height() - TEMPO_LANE_HEIGHT).abs() < 1e-9);
}

#[test]
fn sections_are_drawn_with_their_name_value_and_flag() {
    let mut v = view();
    v.set_energy_lane(true);
    v.set_energy_map(Some(map(true)), false);
    let t = texts(&mut v);
    assert!(t.iter().any(|s| s == "ENERGY"), "{t:?}");
    assert!(t.iter().any(|s| s.starts_with("Build 0.")), "{t:?}");
    assert!(t.iter().any(|s| s.starts_with("Drop 0.")), "{t:?}");
    assert!(t.iter().any(|s| s == "! build > drop"), "{t:?}");
    assert!(t.iter().any(|s| s == "1 flag"), "{t:?}");
    let (m, stale) = v.energy_map().unwrap();
    assert_eq!((m.sections.len(), stale), (2, false));
}

#[test]
fn the_header_says_what_state_the_lane_is_in() {
    let mut v = view();
    v.set_energy_lane(true);
    assert!(texts(&mut v).iter().any(|s| s == "not measured"));
    v.set_energy_map(Some(map(false)), false);
    assert!(texts(&mut v).iter().any(|s| s == "2 sections"));
    v.set_energy_map(Some(map(false)), true);
    assert!(texts(&mut v).iter().any(|s| s == "out of date"));
    v.set_energy_progress(Some(0.4));
    assert!(texts(&mut v).iter().any(|s| s == "measuring 40%"));
    v.set_energy_progress(None);
    v.set_energy_note(Some("no master".into()));
    assert!(texts(&mut v).iter().any(|s| s == "no master"));
    v.set_energy_map(None, false);
    assert!(v.energy_map().is_none());
}

#[test]
fn nothing_is_drawn_while_the_lane_is_hidden() {
    let mut v = view();
    v.set_energy_map(Some(map(true)), false);
    let t = texts(&mut v);
    assert!(!t.iter().any(|s| s == "ENERGY"));
    assert!(!t.iter().any(|s| s.starts_with("Build 0.")));
}

#[test]
fn a_press_in_the_lane_changes_nothing() {
    let (doc, reg) = (song_doc(), registry());
    let mut v = arrangement(&doc, &reg);
    v.set_energy_lane(true);
    v.set_energy_map(Some(map(true)), false);
    let rect = v.energy_lane_rect();
    let p = Point::new(rect.x0 + 60.0, rect.center().y);
    let actions = run(|e| v.handle(e, &doc, &reg), &[down(p), up(p)]);
    let touched: Vec<_> = actions
        .iter()
        .filter(|a| !matches!(a, UiAction::EndMerge))
        .collect();
    assert!(touched.is_empty(), "{touched:?}");
}
