//! The Learn strip of a plugin window: the plugin's name, the LEARN AUTOMATION and LEARN MIDI
//! toggles (one lit at most), the status line and the `×`; a click asks the app to toggle or
//! disarm; each state's display list matches its golden, in English here and in French in
//! `french_goldens.rs`.

mod support;

use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{
    DisplayList, DockStripHit, PluginDockRequest, PluginDockUi, PluginLearnMode,
    PluginLearnRequest, PluginLearnState, PluginLearnStatus, PluginLearnStrip, Theme, UiAction,
};
use support::*;

const BOUNDS: Rect = Rect::new(0.0, 0.0, 720.0, atomik_ui::LEARN_STRIP_HEIGHT);

fn state(mode: Option<PluginLearnMode>, status: PluginLearnStatus) -> PluginLearnState {
    PluginLearnState {
        device: key("lead"),
        plugin: "Test Synth".into(),
        mode,
        status,
    }
}

fn strip(s: PluginLearnState) -> PluginLearnStrip {
    let mut v = PluginLearnStrip::new(Theme::dark());
    v.set_bounds(BOUNDS);
    v.set_state(s);
    v
}

fn texts(v: &PluginLearnStrip) -> Vec<String> {
    let mut dl = DisplayList::new();
    v.paint(&mut dl);
    dl.texts().into_iter().map(str::to_string).collect()
}

fn click(v: &mut PluginLearnStrip, p: Point) -> Vec<UiAction> {
    let mut out = v.handle(&down(p));
    out.extend(v.handle(&up(p)));
    out
}

/// The four states the goldens show.
fn states() -> Vec<(&'static str, PluginLearnState)> {
    vec![
        ("idle", state(None, PluginLearnStatus::Waiting)),
        (
            "automation",
            state(
                Some(PluginLearnMode::Automation),
                PluginLearnStatus::LaneShown("Cutoff".into()),
            ),
        ),
        (
            "midi",
            state(
                Some(PluginLearnMode::Midi),
                PluginLearnStatus::MoveHardware("Cutoff".into()),
            ),
        ),
        (
            "mapped",
            state(
                Some(PluginLearnMode::Midi),
                PluginLearnStatus::Mapped {
                    param: "Cutoff".into(),
                    source: "CC 21 ch 1".into(),
                },
            ),
        ),
    ]
}

#[test]
fn the_strip_shows_the_plugin_the_two_toggles_the_status_and_the_close() {
    let t = texts(&strip(state(None, PluginLearnStatus::Waiting)));
    for want in ["Test Synth", "LEARN AUTOMATION", "LEARN MIDI", "\u{d7}"] {
        assert!(t.iter().any(|s| s == want), "missing {want:?} in {t:?}");
    }
    assert!(
        t.iter().any(|s| s.starts_with("Pick LEARN AUTOMATION")),
        "{t:?}"
    );
    let t = texts(&strip(state(
        Some(PluginLearnMode::Midi),
        PluginLearnStatus::Mapped {
            param: "Cutoff".into(),
            source: "CC 21 ch 1".into(),
        },
    )));
    assert!(t.iter().any(|s| s == "Cutoff \u{2190} CC 21 ch 1"), "{t:?}");
    let t = texts(&strip(state(
        Some(PluginLearnMode::Automation),
        PluginLearnStatus::NotAutomatable,
    )));
    assert!(t.iter().any(|s| s == "This control is not automatable"));
}

#[test]
fn the_toggles_and_the_close_ask_the_app() {
    let mut v = strip(state(None, PluginLearnStatus::Waiting));
    let auto = v.automation_rect().center();
    let midi = v.midi_rect().center();
    let close = v.close_rect().center();
    let ask = |request| UiAction::PluginLearn {
        device: key("lead"),
        request,
    };
    assert_eq!(
        click(&mut v, auto),
        vec![ask(PluginLearnRequest::Toggle(PluginLearnMode::Automation))]
    );
    assert_eq!(
        click(&mut v, midi),
        vec![ask(PluginLearnRequest::Toggle(PluginLearnMode::Midi))]
    );
    assert_eq!(click(&mut v, close), vec![ask(PluginLearnRequest::Off)]);
    // The plugin's name and the status line are not buttons.
    assert!(click(&mut v, Point::new(20.0, 14.0)).is_empty());
    assert!(click(&mut v, Point::new(500.0, 14.0)).is_empty());
    assert!(v.help_at(auto).is_some() && v.help_at(close).is_some());
}

#[test]
fn the_parts_never_overlap_and_fit_the_strip() {
    let v = strip(state(None, PluginLearnStatus::Waiting));
    let (a, m, c) = (v.automation_rect(), v.midi_rect(), v.close_rect());
    assert!(a.x1 <= m.x0 && m.x1 < c.x0);
    for r in [a, m, c] {
        assert!(r.y0 >= BOUNDS.y0 && r.y1 <= BOUNDS.y1, "{r:?}");
    }
}

#[test]
fn a_new_state_repaints_and_the_same_one_does_not() {
    let mut v = strip(state(None, PluginLearnStatus::Waiting));
    v.painted();
    v.set_state(state(None, PluginLearnStatus::Waiting));
    assert!(!v.needs_repaint());
    v.set_state(state(
        Some(PluginLearnMode::Automation),
        PluginLearnStatus::Waiting,
    ));
    assert!(v.needs_repaint());
}

#[test]
fn the_strip_in_each_state_matches_its_golden() {
    for (name, s) in states() {
        let v = strip(s);
        let mut dl = DisplayList::new();
        v.paint(&mut dl);
        starter::check_golden(&format!("plugin_learn_strip_{name}_1x.json"), &dl.to_json());
    }
}

fn docked(dock: PluginDockUi) -> PluginLearnStrip {
    let mut v = strip(state(None, PluginLearnStatus::Waiting));
    v.set_dock(dock);
    v
}

fn dock_actions(actions: &[UiAction]) -> Vec<PluginDockRequest> {
    actions
        .iter()
        .filter_map(|a| match a {
            UiAction::PluginDock { request, device } => {
                assert_eq!(device, &key("lead"));
                Some(*request)
            }
            _ => None,
        })
        .collect()
}

#[test]
fn an_editor_in_its_own_window_offers_dock_in_app_where_the_system_can_and_nothing_where_not() {
    let t = texts(&docked(PluginDockUi::CanDock));
    assert!(t.iter().any(|s| s == "DOCK IN APP"), "{t:?}");
    assert!(!t.iter().any(|s| s == "POP OUT" || s == "CLOSE"), "{t:?}");
    let mut v = docked(PluginDockUi::CanDock);
    let p = v.dock_rect().expect("the button").center();
    assert_eq!(dock_actions(&click(&mut v, p)), [PluginDockRequest::Dock]);
    let hidden = docked(PluginDockUi::Hidden);
    assert!(hidden.dock_rect().is_none() && hidden.shut_rect().is_none());
    assert!(!texts(&hidden).iter().any(|s| s == "DOCK IN APP"));
}

#[test]
fn a_frame_in_the_app_window_has_pop_out_close_and_a_grip_and_its_buttons_act() {
    let mut v = docked(PluginDockUi::Docked);
    let t = texts(&v);
    for want in ["POP OUT", "CLOSE", "Test Synth"] {
        assert!(t.iter().any(|s| s == want), "missing {want:?} in {t:?}");
    }
    let (pop, shut) = (v.dock_rect().unwrap(), v.shut_rect().unwrap());
    assert!(
        pop.x1 <= shut.x0 && shut.x1 <= v.close_rect().x0,
        "no overlap"
    );
    assert_eq!(
        dock_actions(&click(&mut v, pop.center())),
        [PluginDockRequest::PopOut]
    );
    assert_eq!(
        dock_actions(&click(&mut v, shut.center())),
        [PluginDockRequest::Close]
    );
    // The Learn toggles still work in the frame.
    let auto = v.automation_rect().center();
    assert!(
        click(&mut v, auto)
            .iter()
            .any(|a| matches!(a, UiAction::PluginLearn { .. }))
    );
    assert!(
        v.automation_rect().x0 >= v.grip_rect().unwrap().x1,
        "the grip leads"
    );
}

#[test]
fn the_title_bar_tells_buttons_the_grip_and_the_bar_apart() {
    let v = docked(PluginDockUi::Docked);
    assert_eq!(v.hit(v.dock_rect().unwrap().center()), DockStripHit::Button);
    assert_eq!(v.hit(v.automation_rect().center()), DockStripHit::Button);
    assert_eq!(v.hit(v.grip_rect().unwrap().center()), DockStripHit::Grip);
    assert_eq!(
        v.hit(Point::new(v.midi_rect().x1 + 4.0, 14.0)),
        DockStripHit::Bar
    );
    // An editor with a window of its own has no grip to drag.
    let own = docked(PluginDockUi::CanDock);
    assert!(own.grip_rect().is_none());
    assert_eq!(
        own.hit(own.grip_rect().unwrap_or_default().center()),
        DockStripHit::Bar
    );
}

#[test]
fn a_plugin_that_stopped_is_said_so_in_its_frame() {
    let v = docked(PluginDockUi::Stopped);
    let t = texts(&v);
    assert!(t.iter().any(|s| s == "The plugin stopped"), "{t:?}");
    assert!(t.iter().any(|s| s == "CLOSE"), "{t:?}");
    assert!(v.grip_rect().is_none(), "a stopped frame is not resized");
}
