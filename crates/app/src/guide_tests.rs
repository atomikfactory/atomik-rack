//! The guided first song: what each step waits for, how the panel moves between steps, its
//! buttons, and that it only ever takes the pointer inside its own rectangle.

use super::*;
use atomik_ui::kurbo::Vec2;
use atomik_ui::{DisplayList, Key, Modifiers};

const BOUNDS: Rect = Rect::new(0.0, 22.0, 1000.0, 778.0);

fn done_through(n: usize) -> Progress {
    // The progress of a song that has done the first `n` steps.
    Progress {
        instrument: n > 0,
        notes: n > 1,
        looping: n > 2,
        drums: n > 3,
        level: n > 4,
        effect: n > 4,
        saved: n > 5,
        exported: n > 6,
    }
}

fn down(p: Point) -> UiEvent {
    UiEvent::PointerDown {
        pos: p,
        button: PointerButton::Primary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    }
}

fn centre(g: &Guide, b: Button) -> Point {
    g.button_rect(BOUNDS, b).center()
}

#[test]
fn there_are_seven_steps_each_with_a_title_a_text_and_something_to_wait_for() {
    assert_eq!(Step::ALL.len(), STEP_COUNT);
    assert_eq!(STEP_COUNT, 7);
    let none = Progress::default();
    for step in Step::ALL {
        assert!(!step.title().is_empty());
        let lines = wrap(step.text(), WIDTH - 2.0 * PAD, TEXT_SIZE);
        assert!(
            (1..=TEXT_LINES).contains(&lines.len()),
            "{step:?} takes {} lines",
            lines.len()
        );
        let parts = step.parts(&none);
        assert!((1..=MAX_PARTS).contains(&parts.len()));
        assert!(parts.iter().all(|(what, done)| !what.is_empty() && !done));
        assert!(!step.done(&none));
    }
}

#[test]
fn each_step_is_done_by_its_own_part_of_the_progress() {
    let only = |f: fn(&mut Progress)| {
        let mut p = Progress::default();
        f(&mut p);
        p
    };
    assert!(Step::Instrument.done(&only(|p| p.instrument = true)));
    assert!(Step::Notes.done(&only(|p| p.notes = true)));
    assert!(Step::Loop.done(&only(|p| p.looping = true)));
    assert!(Step::Drums.done(&only(|p| p.drums = true)));
    assert!(Step::Save.done(&only(|p| p.saved = true)));
    assert!(Step::Export.done(&only(|p| p.exported = true)));
    // The mix needs both its parts.
    assert!(!Step::Mix.done(&only(|p| p.level = true)));
    assert!(!Step::Mix.done(&only(|p| p.effect = true)));
    assert!(Step::Mix.done(&only(|p| {
        p.level = true;
        p.effect = true;
    })));
    // And no step is done by another step's part.
    let p = only(|p| p.drums = true);
    for step in Step::ALL.into_iter().filter(|s| *s != Step::Drums) {
        assert!(!step.done(&p), "{step:?}");
    }
}

#[test]
fn doing_each_step_in_turn_moves_the_panel_on_to_the_closing_page() {
    let mut g = Guide::new();
    g.update(Progress::default());
    for n in 0..STEP_COUNT {
        assert_eq!(g.index(), n, "waiting at step {}", n + 1);
        assert_eq!(g.current(), Some(Step::ALL[n]));
        // Seen not done, then done: it moves on.
        g.update(done_through(n));
        assert_eq!(g.index(), n);
        g.update(done_through(n + 1));
        assert_eq!(g.index(), n + 1, "step {} done", n + 1);
    }
    assert!(g.finished());
    assert_eq!(g.current(), None);
    // The closing page stays.
    g.update(done_through(STEP_COUNT));
    assert!(g.finished());
}

#[test]
fn a_step_that_was_already_done_waits_for_next_instead_of_racing_past() {
    let mut g = Guide::new();
    // The song already has an instrument and notes before the panel looks.
    g.update(done_through(2));
    assert_eq!(g.index(), 0, "the first step was never seen undone");
    assert!(g.current_done());
    assert_eq!(g.label(Button::Skip), "Next");
    g.press(Button::Skip);
    assert_eq!(g.index(), 1);
    g.update(done_through(2));
    assert_eq!(g.index(), 1);
    g.press(Button::Skip);
    // The loop step is not done: it waits, then moves on when it is.
    g.update(done_through(2));
    assert_eq!(g.index(), 2);
    assert_eq!(g.label(Button::Skip), "Skip");
    g.update(done_through(3));
    assert_eq!(g.index(), 3);
}

#[test]
fn back_goes_to_the_step_before_and_does_not_move_on_by_itself() {
    let mut g = Guide::new();
    g.update(done_through(0));
    g.update(done_through(1));
    g.update(done_through(1));
    assert_eq!(g.index(), 1);
    g.press(Button::Back);
    assert_eq!(g.index(), 0);
    // The step before is done, yet it stays until Next.
    g.update(done_through(1));
    g.update(done_through(1));
    assert_eq!(g.index(), 0);
    // The first step has no step before it.
    assert!(!g.enabled(Button::Back));
    g.press(Button::Back);
    assert_eq!(g.index(), 0);
}

#[test]
fn skip_goes_past_a_step_that_is_not_done_and_the_last_skip_closes_the_tour_page() {
    let mut g = Guide::new();
    for n in 0..STEP_COUNT {
        assert!(g.enabled(Button::Skip));
        assert!(!g.current_done());
        g.press(Button::Skip);
        assert_eq!(g.index(), n + 1);
    }
    assert!(g.finished());
    assert!(
        !g.enabled(Button::Skip),
        "nothing to skip on the closing page"
    );
    g.press(Button::Skip);
    assert!(g.finished());
    g.press(Button::Back);
    assert_eq!(g.index(), STEP_COUNT - 1);
}

#[test]
fn the_panel_asks_to_close_and_moves_to_the_other_side() {
    let mut g = Guide::new();
    assert!(!g.on_left());
    let right = g.rect(BOUNDS);
    assert_eq!(
        g.handle(&down(centre(&g, Button::Move)), BOUNDS),
        GuideOutcome::Consumed
    );
    assert!(g.on_left());
    let left = g.rect(BOUNDS);
    assert!(left.x1 < right.x0, "{left:?} {right:?}");
    g.press(Button::Move);
    assert_eq!(g.rect(BOUNDS), right);
    assert_eq!(
        g.handle(&down(centre(&g, Button::Close)), BOUNDS),
        GuideOutcome::Close
    );
}

#[test]
fn only_the_pointer_inside_the_panel_is_taken_and_no_key_ever_is() {
    let mut g = Guide::new();
    let r = g.rect(BOUNDS);
    let inside = Point::new(r.x0 + 40.0, r.y0 + 60.0);
    let outside = Point::new(r.x0 - 40.0, r.y0 - 40.0);
    assert_eq!(g.handle(&down(inside), BOUNDS), GuideOutcome::Consumed);
    assert_eq!(g.handle(&down(outside), BOUNDS), GuideOutcome::Ignored);
    let wheel = |pos| UiEvent::Wheel {
        delta: Vec2::new(0.0, 1.0),
        pos,
        modifiers: Modifiers::NONE,
    };
    assert_eq!(g.handle(&wheel(inside), BOUNDS), GuideOutcome::Consumed);
    assert_eq!(g.handle(&wheel(outside), BOUNDS), GuideOutcome::Ignored);
    for key in [
        Key::Escape,
        Key::Enter,
        Key::Space,
        Key::Tab,
        Key::Char('a'),
    ] {
        let event = UiEvent::Key {
            key: key.clone(),
            modifiers: Modifiers::NONE,
            pressed: true,
        };
        assert_eq!(g.handle(&event, BOUNDS), GuideOutcome::Ignored, "{key:?}");
    }
    // A press on the panel that is not on a button changes nothing.
    assert_eq!(g.index(), 0);
}

#[test]
fn a_disabled_button_does_nothing() {
    let mut g = Guide::new();
    assert_eq!(
        g.handle(&down(centre(&g, Button::Back)), BOUNDS),
        GuideOutcome::Consumed
    );
    assert_eq!(g.index(), 0);
    let mut g = Guide::new();
    for _ in 0..STEP_COUNT {
        g.skip();
    }
    g.handle(&down(centre(&g, Button::Skip)), BOUNDS);
    assert!(g.finished());
}

#[test]
fn the_panel_and_its_buttons_stay_inside_the_window_even_a_small_one() {
    for bounds in [
        BOUNDS,
        Rect::new(0.0, 22.0, 640.0, 478.0),
        Rect::new(0.0, 22.0, 340.0, 300.0),
        Rect::new(240.0, 22.0, 900.0, 600.0),
    ] {
        for left in [false, true] {
            let mut g = Guide::new();
            if left {
                g.flip_side();
            }
            let r = g.rect(bounds);
            assert!(
                r.x0 >= bounds.x0 && r.x1 <= bounds.x1 && r.y0 >= bounds.y0 && r.y1 <= bounds.y1,
                "{r:?} in {bounds:?}"
            );
        }
    }
    // Every button sits inside the panel and no two overlap (at full size).
    let g = Guide::new();
    let r = g.rect(BOUNDS);
    let rects: Vec<Rect> = Button::ALL
        .iter()
        .map(|b| g.button_rect(BOUNDS, *b))
        .collect();
    for (i, a) in rects.iter().enumerate() {
        assert!(r.contains(a.center()) && a.x0 >= r.x0 && a.x1 <= r.x1 && a.y1 <= r.y1);
        for b in &rects[i + 1..] {
            assert!(a.intersect(*b).area() < 1.0, "{a:?} {b:?}");
        }
    }
}

#[test]
fn the_buttons_have_tooltips_and_the_rest_of_the_panel_has_none() {
    let mut g = Guide::new();
    for b in Button::ALL {
        let help = g.help_at(BOUNDS, centre(&g, b)).expect("a tooltip");
        assert!(help.text.len() < 80, "{}", help.text);
    }
    let r = g.rect(BOUNDS);
    assert!(
        g.help_at(BOUNDS, Point::new(r.x0 + 20.0, r.y0 + 50.0))
            .is_none()
    );
    let before = g.help_at(BOUNDS, centre(&g, Button::Skip)).unwrap().text;
    g.update(done_through(0));
    g.update(done_through(1));
    g.back();
    g.update(done_through(1));
    let after = g.help_at(BOUNDS, centre(&g, Button::Skip)).unwrap().text;
    assert_ne!(before, after, "Skip becomes Next on a step that is done");
}

#[test]
fn the_panel_says_which_step_it_is_on_and_what_it_waits_for() {
    let mut g = Guide::new();
    let lines = g.lines().join("\n");
    assert!(lines.contains("STEP 1 OF 7") && lines.contains("Add an instrument"));
    assert!(lines.contains("a track with an instrument"));
    g.press(Button::Skip);
    g.press(Button::Skip);
    g.press(Button::Skip);
    g.press(Button::Skip);
    assert!(g.lines().join("\n").contains("STEP 5 OF 7"));
    assert!(g.lines().join("\n").contains("an effect"));
    for _ in 0..3 {
        g.press(Button::Skip);
    }
    assert!(g.finished());
    let lines = g.lines().join("\n");
    assert!(lines.contains("That is a first song") && !lines.contains("STEP"));
}

#[test]
fn the_spots_name_a_control_for_every_step() {
    let spots: Vec<Spot> = Step::ALL.iter().map(|s| s.spot()).collect();
    assert_eq!(
        spots,
        [
            Spot::Menu("Create"),
            Spot::Song,
            Spot::Loop,
            Spot::Menu("Create"),
            Spot::Rack,
            Spot::Menu("File"),
            Spot::Menu("File"),
        ]
    );
}

fn check_golden(name: &str, actual: &str) {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(format!("{name}_1x.json"));
    if std::env::var_os("ATOMIK_UPDATE_GOLDEN").is_some_and(|v| v == "1") {
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| {
            panic!(
                "missing golden {}; run with ATOMIK_UPDATE_GOLDEN=1",
                path.display()
            )
        })
        .replace("\r\n", "\n");
    assert!(
        expected == actual,
        "{} differs (regenerate with ATOMIK_UPDATE_GOLDEN=1)",
        path.display()
    );
}

fn golden(name: &str, theme: &Theme) {
    let mut g = Guide::new();
    // On the mix step with its level moved and no effect yet, the pointer over Skip.
    for _ in 0..4 {
        g.skip();
    }
    g.update(Progress {
        level: true,
        ..Progress::default()
    });
    g.handle(
        &UiEvent::PointerMove {
            pos: centre(&g, Button::Skip),
            modifiers: Modifiers::NONE,
        },
        BOUNDS,
    );
    let mut list = DisplayList::new();
    g.paint(&mut list, theme, BOUNDS);
    check_golden(name, &list.to_json());
}

#[test]
fn golden_guide_panel_dark() {
    golden("guide_panel", &Theme::dark());
}

#[test]
fn golden_guide_panel_light() {
    golden("guide_panel_light", &Theme::light());
}

#[test]
fn the_closing_page_paints_without_parts() {
    let mut g = Guide::new();
    for _ in 0..STEP_COUNT {
        g.skip();
    }
    let mut list = DisplayList::new();
    g.paint(&mut list, &Theme::dark(), BOUNDS);
    let json = list.to_json();
    assert!(json.contains("That is a first song") && json.contains("Close"));
    assert!(!json.contains("an export finished"));
}
