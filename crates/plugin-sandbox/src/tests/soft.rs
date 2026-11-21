//! Cosmetic requests: they return at once, the frame's allowance bounds what the main thread is
//! asked to wait for them whatever the plugins do, and what a plugin answers late is delivered in
//! a later frame. Nothing here asserts on how long a wait really took (a loaded machine wakes a
//! timed wait late): the waits are checked by what the link asked for (`SoftBudget::asked` and
//! `waits`), a late answer is a request the fake holds until the test releases it, and every
//! outcome is waited for on a condition with a generous bound.

use super::*;
use crate::fake::{Action, FakeOptions, start_with_budget, test_timeouts};
use atomik_plugin_host::{PluginInstance, SOFT_FRAME_BUDGET, SoftAnswer, SoftBudget};
use atomik_plugin_ipc::{FromChild, ToChild, WireErrorKind};
use std::time::Duration;

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

/// A fake that holds every text until the test releases it.
fn held_texts() -> FakeOptions {
    FakeOptions::default().script(|req, _| match req {
        ToChild::ParamText { id, plain, .. } => Some(Action::Blocked(vec![(
            FromChild::Text {
                id: *id,
                text: Some(format!("{plain:.2} units")),
            },
            None,
        )])),
        _ => None,
    })
}

fn requests_of(r: &Rig, f: fn(&ToChild) -> bool) -> usize {
    r.fake.ctl.requests().iter().filter(|q| f(q)).count()
}

/// Frames until `f` is true (the bound is generous: a slow runner needs seconds).
fn frames_until(r: &mut Rig, what: &str, mut f: impl FnMut(&mut Rig) -> bool) {
    let budget = r.instance.budget();
    let limit = crate::clock::now() + Duration::from_secs(60);
    while crate::clock::now() < limit {
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
    let mut r = rig_with(held_texts(), test_timeouts(), SoftBudget::default());
    let budget = r.instance.budget();
    budget.begin_frame();
    assert_eq!(r.instance.param_text(1, 0.25), None);
    assert_eq!(budget.waits(), 1, "it was waited for once");
    assert!(budget.asked() <= SOFT_FRAME_BUDGET, "{:?}", budget.asked());
    assert!(r.instance.soft_out(), "the request is still out");
    r.fake.ctl.release();
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
fn a_frame_that_asks_thirty_texts_is_asked_to_wait_no_more_than_the_allowance() {
    // The plugin holds every answer: the first request is waited for, the other 29 are not.
    let mut r = rig_with(held_texts(), test_timeouts(), SoftBudget::default());
    let budget = r.instance.budget();
    budget.begin_frame();
    for n in 0..30u32 {
        assert_eq!(r.instance.param_text(n, f64::from(n) / 100.0), None);
    }
    assert_eq!(budget.waits(), 1, "one wait for thirty requests");
    assert!(budget.asked() <= SOFT_FRAME_BUDGET, "{:?}", budget.asked());
    // Every request went out, and their answers are delivered, in order, in later frames.
    r.fake.ctl.release();
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
    let mut r = rig_with(held_texts(), test_timeouts(), SoftBudget::default());
    let budget = r.instance.budget();
    budget.begin_frame();
    assert_eq!(r.instance.param_text(1, 0.1), None);
    assert_eq!(budget.waits(), 1);
    let asked = budget.asked();
    assert_eq!(r.instance.param_text(2, 0.2), None);
    assert_eq!(r.instance.param_text(3, 0.3), None);
    assert_eq!(budget.waits(), 1, "no further wait in this frame");
    assert_eq!(budget.asked(), asked);
    budget.begin_frame();
    assert_eq!(r.instance.param_text(4, 0.4), None);
    assert_eq!(budget.waits(), 1, "the next frame waits again");
}

#[test]
fn every_instance_draws_on_the_same_allowance_in_a_frame() {
    let budget = SoftBudget::default();
    let mut a = rig_with(held_texts(), test_timeouts(), budget.clone());
    let mut b = rig_with(held_texts(), test_timeouts(), budget.clone());
    budget.begin_frame();
    assert_eq!(a.instance.param_text(1, 0.1), None);
    assert_eq!(budget.waits(), 1);
    // The first instance was waited for in vain, so the allowance is used up whatever the
    // clock did: the second is asked, and not waited for.
    assert_eq!(b.instance.param_text(1, 0.1), None);
    assert_eq!(budget.waits(), 1, "the allowance was shared");
    assert!(budget.asked() <= SOFT_FRAME_BUDGET, "{:?}", budget.asked());
    eventually("the second to be asked all the same", || {
        requests_of(&b, |q| matches!(q, ToChild::ParamText { .. })) == 1
    });
}

fn held_resize() -> FakeOptions {
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

fn resizes_sent(r: &Rig) -> Vec<(u32, u32)> {
    r.fake
        .ctl
        .requests()
        .iter()
        .filter_map(|q| match q {
            ToChild::ResizeGui { w, h, .. } => Some((*w, *h)),
            _ => None,
        })
        .collect()
}

#[test]
fn ten_sizes_asked_while_one_is_out_make_two_requests_the_first_and_the_last() {
    let mut r = rig_with(held_resize(), test_timeouts(), SoftBudget::default());
    let budget = r.instance.budget();
    budget.begin_frame();
    for n in 0..10 {
        assert_eq!(
            r.instance.request_resize_gui(300 + n, 200).ok(),
            Some(None),
            "asked, not answered"
        );
    }
    eventually("the first request to arrive", || {
        resizes_sent(&r) == vec![(300, 200)]
    });
    // The plugin is free again: the answer comes and the latest size goes out.
    r.fake.ctl.release();
    let mut got = Vec::new();
    frames_until(&mut r, "the answer", |r| {
        got.extend(r.instance.take_soft_answers());
        !got.is_empty()
    });
    assert_eq!(
        resizes_sent(&r),
        vec![(300, 200), (309, 200)],
        "first and last"
    );
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
            ToChild::ResizeGui { id, .. } => Some(Action::Blocked(vec![(
                FromChild::Err {
                    id: *id,
                    kind: WireErrorKind::Unsupported,
                    text: "fixed size".to_string(),
                },
                None,
            )])),
            _ => None,
        }),
        test_timeouts(),
        SoftBudget::default(),
    );
    r.instance.budget().begin_frame();
    assert_eq!(r.instance.request_resize_gui(400, 300).ok(), Some(None));
    r.fake.ctl.release();
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
fn a_resize_the_plugin_answers_reaches_the_caller_at_once_or_as_a_late_answer() {
    // The plugin answers at once; whether the wait of the frame caught it depends on the
    // machine, so the size must arrive one way or the other.
    let mut r = rig_with(
        FakeOptions::default(),
        test_timeouts(),
        SoftBudget::default(),
    );
    r.instance.budget().begin_frame();
    match r.instance.request_resize_gui(500, 400).ok() {
        Some(Some(size)) => assert_eq!(size, (500, 400)),
        Some(None) => {
            let mut got = Vec::new();
            frames_until(&mut r, "the late size", |r| {
                got.extend(r.instance.take_soft_answers());
                !got.is_empty()
            });
            assert_eq!(
                got,
                vec![SoftAnswer::GuiSized {
                    asked: (500, 400),
                    took: Some((500, 400))
                }]
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn default_pins_that_come_late_are_delivered() {
    let mut r = rig_with(
        FakeOptions::default().script(|req, _| match req {
            ToChild::DefaultPins { id } => Some(Action::Blocked(vec![(
                FromChild::Pins {
                    id: *id,
                    pins: vec![7, 8],
                },
                None,
            )])),
            _ => None,
        }),
        test_timeouts(),
        SoftBudget::default(),
    );
    r.instance.budget().begin_frame();
    assert!(r.instance.default_pins().is_empty(), "held by the plugin");
    r.fake.ctl.release();
    let mut got = Vec::new();
    frames_until(&mut r, "the pins", |r| {
        got.extend(r.instance.take_soft_answers());
        !got.is_empty()
    });
    assert_eq!(got, vec![SoftAnswer::DefaultPins(vec![7, 8])]);
}

#[test]
fn a_plugin_that_never_answers_is_never_asked_to_be_waited_for_more_than_the_allowance_in_a_frame()
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
    for frame in 0..40u32 {
        budget.begin_frame();
        for n in 0..20u32 {
            let _ = r.instance.param_text(n, f64::from(frame * 100 + n));
            let _ = r.instance.request_resize_gui(300 + n, 200);
        }
        assert!(budget.waits() <= 1, "frame {frame}: {}", budget.waits());
        assert!(budget.asked() <= SOFT_FRAME_BUDGET, "{:?}", budget.asked());
        r.instance.poll();
    }
    assert!(!r.instance.is_lost(), "asking stops nothing by itself");
}
