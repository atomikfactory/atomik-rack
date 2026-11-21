//! Cosmetic requests: they return at once, the frame's allowance bounds what the main thread
//! waits for them whatever the plugins do, and what a plugin answers late is delivered in a
//! later frame. The child is a fake whose answers are delayed; the waits asserted on are the
//! link's own accounting (`SoftBudget::spent`), never the wall clock of the test.

use super::*;
use crate::clock::now;
use crate::fake::{Action, FakeOptions, start_with_budget, test_timeouts};
use atomik_plugin_host::{PluginInstance, SOFT_FRAME_BUDGET, SoftAnswer, SoftBudget};
use atomik_plugin_ipc::{FromChild, ToChild, WireErrorKind};
use std::time::Duration;

/// How far past the allowance the accounting may run (a loaded machine wakes a wait late).
const MARGIN: Duration = Duration::from_millis(150);

fn rig_with(options: FakeOptions, timeouts: crate::link::Timeouts, budget: SoftBudget) -> Rig {
    let s = start_with_budget(options, timeouts, budget);
    Rig {
        instance: s
            .instance
            .map_err(|e| e.to_string())
            .expect("the fake greets"),
        fake: s.fake,
        manager: s.manager,
        live: s.live,
    }
}

fn text_after(delay: Duration) -> FakeOptions {
    FakeOptions::default().script(move |req, _| match req {
        ToChild::ParamText { id, plain, .. } => Some(Action::Delayed(
            delay,
            vec![(
                FromChild::Text {
                    id: *id,
                    text: Some(format!("{plain:.2} units")),
                },
                None,
            )],
        )),
        _ => None,
    })
}

fn requests_of(r: &Rig, f: fn(&ToChild) -> bool) -> usize {
    r.fake.ctl.requests().iter().filter(|q| f(q)).count()
}

/// Frames until `f` is true (a generous bound: a slow runner needs seconds, not milliseconds).
fn frames_until(r: &mut Rig, what: &str, mut f: impl FnMut(&mut Rig) -> bool) {
    let budget = r.instance.budget();
    let limit = now() + Duration::from_secs(60);
    while now() < limit {
        budget.begin_frame();
        r.instance.poll();
        if f(r) {
            return;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    panic!("timed out waiting for {what}");
}

#[test]
fn a_cosmetic_request_returns_without_its_answer_which_is_collected_in_a_later_frame() {
    let mut r = rig_with(
        text_after(Duration::from_millis(150)),
        test_timeouts(),
        SoftBudget::default(),
    );
    let budget = r.instance.budget();
    budget.begin_frame();
    assert_eq!(r.instance.param_text(1, 0.25), None);
    assert!(
        budget.spent() <= SOFT_FRAME_BUDGET + MARGIN,
        "{:?}",
        budget.spent()
    );
    assert!(r.instance.soft_out(), "the request is still out");
    let mut got = Vec::new();
    frames_until(&mut r, "the answer", |r| {
        got.extend(r.instance.take_soft_answers());
        !got.is_empty()
    });
    assert_eq!(
        got,
        vec![SoftAnswer::ParamText {
            param: 1,
            plain: 0.25,
            text: "0.25 units".to_string()
        }]
    );
    assert!(!r.instance.soft_out());
    assert!(!r.instance.is_lost() && !r.fake.ctl.was_killed());
}

#[test]
fn a_frame_that_asks_thirty_texts_waits_no_more_than_the_allowance() {
    // The plugin answers each request after 100 ms, one after the other.
    let mut r = rig_with(
        text_after(Duration::from_millis(100)),
        test_timeouts(),
        SoftBudget::default(),
    );
    let budget = r.instance.budget();
    budget.begin_frame();
    for n in 0..30u32 {
        assert_eq!(r.instance.param_text(n, f64::from(n) / 100.0), None);
    }
    assert!(
        budget.spent() <= SOFT_FRAME_BUDGET + MARGIN,
        "thirty requests waited {:?}",
        budget.spent()
    );
    // Every request went out, and their answers are delivered, in order, in later frames.
    let mut got: Vec<SoftAnswer> = Vec::new();
    frames_until(&mut r, "thirty answers", |r| {
        got.extend(r.instance.take_soft_answers());
        got.len() >= 30
    });
    let params: Vec<u32> = got
        .iter()
        .filter_map(|a| match a {
            SoftAnswer::ParamText { param, .. } => Some(*param),
            _ => None,
        })
        .collect();
    assert_eq!(params, (0..30).collect::<Vec<u32>>());
    assert!(!r.instance.is_lost() && !r.fake.ctl.was_killed());
}

#[test]
fn an_instance_waited_for_in_vain_is_not_waited_for_again_in_the_frame() {
    let mut r = rig_with(
        text_after(Duration::from_millis(200)),
        test_timeouts(),
        SoftBudget::default(),
    );
    let budget = r.instance.budget();
    budget.begin_frame();
    assert_eq!(r.instance.param_text(1, 0.1), None);
    let after_first = budget.spent();
    assert_eq!(r.instance.param_text(2, 0.2), None);
    assert_eq!(r.instance.param_text(3, 0.3), None);
    assert_eq!(budget.spent(), after_first, "no further wait in this frame");
    budget.begin_frame();
    assert_eq!(r.instance.param_text(4, 0.4), None);
    assert!(
        budget.spent() > Duration::ZERO,
        "the next frame waits again"
    );
}

#[test]
fn every_instance_draws_on_the_same_allowance_in_a_frame() {
    let budget = SoftBudget::default();
    let mut a = rig_with(
        text_after(Duration::from_millis(200)),
        test_timeouts(),
        budget.clone(),
    );
    let mut b = rig_with(
        text_after(Duration::from_millis(200)),
        test_timeouts(),
        budget.clone(),
    );
    budget.begin_frame();
    assert_eq!(a.instance.param_text(1, 0.1), None);
    let after_a = budget.spent();
    assert!(after_a >= Duration::from_millis(10), "{after_a:?}");
    assert_eq!(b.instance.param_text(1, 0.1), None);
    assert!(
        budget.spent() <= SOFT_FRAME_BUDGET + MARGIN,
        "two instances waited {:?} together",
        budget.spent()
    );
    eventually("the second to be asked all the same", || {
        requests_of(&b, |q| matches!(q, ToChild::ParamText { .. })) == 1
    });
}

fn blocked_resize() -> FakeOptions {
    FakeOptions::default().script(|req, _| match req {
        ToChild::ResizeGui { id, w, h } => Some(Action::Blocked(vec![(
            FromChild::GuiSized {
                id: *id,
                w: *w,
                h: *h,
            },
            None,
        )])),
        _ => None,
    })
}

#[test]
fn ten_sizes_asked_while_one_is_out_make_two_requests_the_first_and_the_last() {
    let mut r = rig_with(blocked_resize(), test_timeouts(), SoftBudget::default());
    let budget = r.instance.budget();
    budget.begin_frame();
    for n in 0..10 {
        assert_eq!(
            r.instance.request_resize_gui(300 + n, 200).ok(),
            Some(None),
            "asked, not answered"
        );
    }
    let sent: Vec<(u32, u32)> = r
        .fake
        .ctl
        .requests()
        .iter()
        .filter_map(|q| match q {
            ToChild::ResizeGui { w, h, .. } => Some((*w, *h)),
            _ => None,
        })
        .collect();
    assert_eq!(sent, vec![(300, 200)], "one request is out, no backlog");
    // The plugin is free again: the answer comes and the latest size goes out.
    r.fake.ctl.release();
    let mut got = Vec::new();
    frames_until(&mut r, "the answer", |r| {
        got.extend(r.instance.take_soft_answers());
        !got.is_empty()
    });
    let sent: Vec<(u32, u32)> = r
        .fake
        .ctl
        .requests()
        .iter()
        .filter_map(|q| match q {
            ToChild::ResizeGui { w, h, .. } => Some((*w, *h)),
            _ => None,
        })
        .collect();
    assert_eq!(sent, vec![(300, 200), (309, 200)], "the first and the last");
    assert_eq!(
        got,
        vec![SoftAnswer::GuiSized {
            asked: (309, 200),
            took: Some((309, 200))
        }],
        "only the last is answered"
    );
}

#[test]
fn an_editor_that_refuses_the_size_says_so_when_it_answers_late() {
    let mut r = rig_with(
        FakeOptions::default().script(|req, _| match req {
            ToChild::ResizeGui { id, .. } => Some(Action::Delayed(
                Duration::from_millis(100),
                vec![(
                    FromChild::Err {
                        id: *id,
                        kind: WireErrorKind::Unsupported,
                        text: "fixed size".to_string(),
                    },
                    None,
                )],
            )),
            _ => None,
        }),
        test_timeouts(),
        SoftBudget::default(),
    );
    r.instance.budget().begin_frame();
    assert_eq!(r.instance.request_resize_gui(400, 300).ok(), Some(None));
    let mut got = Vec::new();
    frames_until(&mut r, "the answer", |r| {
        got.extend(r.instance.take_soft_answers());
        !got.is_empty()
    });
    assert_eq!(
        got,
        vec![SoftAnswer::GuiSized {
            asked: (400, 300),
            took: None
        }]
    );
}

#[test]
fn a_resize_that_the_plugin_answers_in_time_is_returned_at_once() {
    let mut r = rig_with(
        FakeOptions::default(),
        test_timeouts(),
        SoftBudget::default(),
    );
    r.instance.budget().begin_frame();
    assert_eq!(
        r.instance.request_resize_gui(500, 400).ok(),
        Some(Some((500, 400)))
    );
    assert!(r.instance.take_soft_answers().is_empty(), "nothing late");
}

#[test]
fn default_pins_that_come_late_are_delivered() {
    let mut r = rig_with(
        FakeOptions::default().script(|req, _| match req {
            ToChild::DefaultPins { id } => Some(Action::Delayed(
                Duration::from_millis(100),
                vec![(
                    FromChild::Pins {
                        id: *id,
                        pins: vec![7, 8],
                    },
                    None,
                )],
            )),
            _ => None,
        }),
        test_timeouts(),
        SoftBudget::default(),
    );
    r.instance.budget().begin_frame();
    assert!(r.instance.default_pins().is_empty(), "not in time");
    let mut got = Vec::new();
    frames_until(&mut r, "the pins", |r| {
        got.extend(r.instance.take_soft_answers());
        !got.is_empty()
    });
    assert_eq!(got, vec![SoftAnswer::DefaultPins(vec![7, 8])]);
}

#[test]
fn a_plugin_that_never_answers_costs_a_frame_no_more_than_the_allowance_however_often_it_is_asked()
{
    let mut r = rig_with(
        FakeOptions::default().script(|req, _| {
            matches!(req, ToChild::ParamText { .. } | ToChild::ResizeGui { .. })
                .then_some(Action::Silent)
        }),
        test_timeouts(),
        SoftBudget::default(),
    );
    let budget = r.instance.budget();
    let mut worst = Duration::ZERO;
    for frame in 0..40u32 {
        budget.begin_frame();
        for n in 0..20u32 {
            let _ = r.instance.param_text(n, f64::from(frame * 100 + n));
            let _ = r.instance.request_resize_gui(300 + n, 200);
        }
        worst = worst.max(budget.spent());
        r.instance.poll();
    }
    assert!(worst <= SOFT_FRAME_BUDGET + MARGIN, "{worst:?}");
    assert!(!r.instance.is_lost(), "asking stops nothing by itself");
}
