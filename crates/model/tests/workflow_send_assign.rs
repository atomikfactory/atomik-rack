//! Send/return auto-assign (model half): an effect created with
//! the master selected goes to the lowest free send/return slot, `AssignSend` ("Use as Send
//! Effect") does the same for an existing loose effect. Both are one undo step, byte-identical
//! after undo and redo, and never fail the creation: all eight used leaves the effect unrouted.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::{
    Command, ConsoleCommand, CreateOptions, CreateSource, CreateTarget, DeviceKey, Document,
    ModelError, RouteNote, WorkflowCommand, effect_route_note, first_free_send_slot, insert_chain,
    new_project_doc, save_project,
};
use common::workflow::*;
use common::*;

const REVERB: &str = "atomik.reverb";

fn reg() -> Registry {
    console_registry()
}

fn project() -> Document {
    Document::new(new_project_doc())
}

fn typ(t: &str) -> CreateSource {
    CreateSource::Type(t.into())
}

fn create(t: &str, target: CreateTarget, options: CreateOptions) -> Command {
    wf(WorkflowCommand::CreateEffect {
        source: typ(t),
        target,
        key: None,
        options,
    })
}

fn on_master(t: &str) -> Command {
    create(
        t,
        CreateTarget::Selected(key("master")),
        CreateOptions::default(),
    )
}

fn no_route() -> CreateOptions {
    CreateOptions {
        route: false,
        ..CreateOptions::default()
    }
}

fn assign(k: &str) -> Command {
    wf(WorkflowCommand::AssignSend { effect: key(k) })
}

fn returns(d: &Document) -> Vec<Option<String>> {
    d.doc()
        .console
        .returns
        .iter()
        .map(|r| r.as_ref().map(|k| k.0.clone()))
        .collect()
}

fn slot(d: &Document, k: &str) -> Option<usize> {
    d.doc().console.return_slot(&key(k))
}

/// The key of the device created last (the one `create` generated).
fn newest(d: &Document, before: &[String]) -> String {
    let now = keys(d);
    let mut new: Vec<String> = now.into_iter().filter(|k| !before.contains(k)).collect();
    assert_eq!(new.len(), 1, "exactly one new device: {new:?}");
    new.pop().unwrap()
}

fn cables_of(d: &Document, k: &str) -> usize {
    d.doc()
        .cables
        .iter()
        .filter(|c| c.from.device.0 == k || c.to.device.0 == k)
        .count()
}

fn has(d: &Document, from: &str, to: &str) -> bool {
    d.doc().cables.contains(&cable(from, to))
}

fn expect_bad(e: ModelError, text: &str) {
    assert!(e.to_string().contains(text), "{e} should mention {text:?}");
}

/// A project that has the starter master, a synth (with channel) and `n` effects on returns.
fn with_sends(r: &Registry, n: usize) -> Document {
    let mut d = project();
    one_step(
        &mut d,
        r,
        wf(WorkflowCommand::CreateInstrument {
            index: usize::MAX,
            source: typ(SYNTH),
            key: Some(key("synth")),
            options: CreateOptions::default(),
        }),
    );
    for _ in 0..n {
        d.apply(on_master(DELAY), r).unwrap();
    }
    d
}

// --------------------------------------------------------------------- master selected

#[test]
fn master_selected_takes_the_first_free_slot_in_one_undo_step() {
    let r = reg();
    let mut d = project();
    let start = text(&d);
    let before = keys(&d);
    one_step(&mut d, &r, on_master(REVERB));
    let fx = newest(&d, &before);
    assert_eq!(returns(&d)[0].as_deref(), Some(fx.as_str()));
    assert_eq!(slot(&d, &fx), Some(0));
    for side in ["l", "r"] {
        assert!(
            has(
                &d,
                &format!("master:fx_send1_{side}"),
                &format!("{fx}:in_{side}")
            ),
            "send 1 feeds the effect ({side})"
        );
        assert!(
            has(
                &d,
                &format!("{fx}:out_{side}"),
                &format!("master:return1_{side}")
            ),
            "the effect feeds return 1 ({side})"
        );
    }
    assert_eq!(cables_of(&d, &fx), 4);
    // Directly under the master (the starter holds only the master).
    assert_eq!(keys(&d), vec!["master".to_string(), fx]);
    d.undo(&r).unwrap();
    assert_eq!(text(&d), start, "undo of the one step restores the starter");
    assert!(!d.can_undo());
}

#[test]
fn the_effect_never_follows_main_the_seam_regression() {
    // The master has audio outputs, but the generic "after the selected device" rule must not
    // apply to it: nothing is cabled after `main`, and the master's own cables stay.
    let r = reg();
    let mut d = project();
    let main_cables: Vec<_> = d.doc().cables.clone();
    one_step(&mut d, &r, on_master(DELAY));
    for c in &d.doc().cables {
        assert!(
            !(c.from.device.0 == "master" && c.from.socket.starts_with("main_"))
                || main_cables.contains(c),
            "a new cable leaves main: {c:?}"
        );
    }
    assert!(
        d.doc()
            .cables
            .iter()
            .all(|c| c.to.socket != "in_l" || c.from.socket.starts_with("fx_send"))
    );
}

#[test]
fn successive_effects_take_slots_one_two_three_after_the_last_return_effect() {
    let r = reg();
    let mut d = project();
    let mut made = Vec::new();
    for want in 0..3 {
        let before = keys(&d);
        one_step(&mut d, &r, on_master(DELAY));
        let fx = newest(&d, &before);
        assert_eq!(slot(&d, &fx), Some(want));
        made.push(fx);
    }
    let mut expect = vec!["master".to_string()];
    expect.extend(made);
    assert_eq!(keys(&d), expect, "each effect lands after the previous one");
}

#[test]
fn the_lowest_free_slot_is_chosen_when_the_used_ones_are_not_contiguous() {
    let r = reg();
    let mut d = project();
    // Slots 1 and 3 used (indices 0 and 2) through the console command, as the console view does.
    run(
        &mut d,
        &r,
        [
            add(usize::MAX, DELAY, Some("a")),
            add(usize::MAX, DELAY, Some("b")),
        ],
    );
    run(
        &mut d,
        &r,
        [
            Command::Console(ConsoleCommand::SetReturn {
                slot: 0,
                effect: Some(key("a")),
            }),
            Command::Console(ConsoleCommand::SetReturn {
                slot: 2,
                effect: Some(key("b")),
            }),
        ],
    );
    assert_eq!(first_free_send_slot(d.doc()), Some(1));
    let before = keys(&d);
    one_step(&mut d, &r, on_master(REVERB));
    let fx = newest(&d, &before);
    assert_eq!(slot(&d, &fx), Some(1), "slot 2 is the lowest free one");
    // Placed right after the last effect on a return (`b`, the last device of the rack).
    assert_eq!(keys(&d).last().unwrap(), &fx);
    assert_eq!(
        returns(&d)[..3].to_vec(),
        vec![Some("a".into()), Some(fx), Some("b".into())]
    );
}

#[test]
fn a_gap_in_the_middle_places_the_effect_after_the_last_return_effect_not_at_the_end() {
    let r = reg();
    let mut d = project();
    run(
        &mut d,
        &r,
        [
            add(usize::MAX, DELAY, Some("a")),
            add(usize::MAX, DELAY, Some("loose")),
            Command::Console(ConsoleCommand::SetReturn {
                slot: 0,
                effect: Some(key("a")),
            }),
        ],
    );
    let before = keys(&d);
    one_step(&mut d, &r, on_master(REVERB));
    let fx = newest(&d, &before);
    assert_eq!(keys(&d), vec!["master", "a", fx.as_str(), "loose"]);
}

#[test]
fn all_eight_used_adds_the_effect_unrouted_and_reports_it() {
    let r = reg();
    let mut d = with_sends(&r, 8);
    assert!(returns(&d).iter().all(Option::is_some));
    assert_eq!(first_free_send_slot(d.doc()), None);
    let command = on_master(REVERB);
    let (source, target) = (typ(REVERB), CreateTarget::Selected(key("master")));
    assert_eq!(
        effect_route_note(d.doc(), &r, &source, &target, &CreateOptions::default()),
        Some(RouteNote::SendSlotsFull)
    );
    let before = keys(&d);
    let returns_before = returns(&d);
    one_step(&mut d, &r, command);
    let fx = newest(&d, &before);
    assert_eq!(cables_of(&d, &fx), 0, "unrouted");
    assert_eq!(returns(&d), returns_before, "no slot changes");
    assert_eq!(
        RouteNote::SendSlotsFull.text(),
        "All 8 send and return slots are in use"
    );
}

#[test]
fn an_unrouted_effect_for_the_master_goes_right_after_it_for_both_target_forms() {
    // All eight used, or `route` off: the effect is only added, directly below the master, and
    // `CreateTarget::Master` places it exactly like selecting the master's key.
    let r = reg();
    for (n, options) in [(8, CreateOptions::default()), (2, no_route())] {
        let mut by_key = with_sends(&r, n);
        let mut by_target = with_sends(&r, n);
        let before = keys(&by_key);
        assert!(before.len() > 3, "devices after the master: {before:?}");
        one_step(
            &mut by_key,
            &r,
            create(
                REVERB,
                CreateTarget::Selected(key("master")),
                options.clone(),
            ),
        );
        one_step(
            &mut by_target,
            &r,
            create(REVERB, CreateTarget::Master, options),
        );
        let fx = newest(&by_key, &before);
        let order = keys(&by_key);
        assert_eq!(order[0], "master");
        assert_eq!(order[1], fx, "right after the master");
        assert_eq!(keys(&by_target), order);
        assert_eq!(save_project(by_target.doc()), save_project(by_key.doc()));
    }
}

#[test]
fn the_note_is_only_given_when_the_send_rule_ran_out_of_slots() {
    let r = reg();
    let full = with_sends(&r, 8);
    let free = with_sends(&r, 7);
    let (s, m) = (typ(REVERB), CreateTarget::Selected(key("master")));
    let note = |d: &Document, t: &CreateTarget, o: &CreateOptions| {
        effect_route_note(d.doc(), &r, &s, t, o)
    };
    assert_eq!(note(&free, &m, &CreateOptions::default()), None);
    assert_eq!(note(&full, &m, &no_route()), None, "route false adds only");
    assert_eq!(
        note(&full, &CreateTarget::Nothing, &CreateOptions::default()),
        None
    );
    let ch = full.doc().console.channels[0].channel.clone();
    assert_eq!(
        note(
            &full,
            &CreateTarget::Selected(ch),
            &CreateOptions::default()
        ),
        None,
        "a channel selection uses the insert rule"
    );
    // A device that is not a stereo effect is never routed, so there is nothing to report.
    let mixer = typ(MIXER);
    assert_eq!(
        effect_route_note(full.doc(), &r, &mixer, &m, &CreateOptions::default()),
        None
    );
}

#[test]
fn route_false_and_non_stereo_effects_are_only_added() {
    let r = reg();
    let mut d = project();
    let before = keys(&d);
    one_step(
        &mut d,
        &r,
        create(DELAY, CreateTarget::Selected(key("master")), no_route()),
    );
    let fx = newest(&d, &before);
    assert_eq!(cables_of(&d, &fx), 0);
    assert!(returns(&d).iter().all(Option::is_none));

    let before = keys(&d);
    one_step(&mut d, &r, on_master(MIXER));
    let mixer = newest(&d, &before);
    assert_eq!(cables_of(&d, &mixer), 0);
    assert!(
        returns(&d).iter().all(Option::is_none),
        "a mixer is no send effect"
    );
}

#[test]
fn the_master_target_variant_is_the_same_as_selecting_the_master() {
    let r = reg();
    let mut a = project();
    let mut b = project();
    a.apply(on_master(DELAY), &r).unwrap();
    b.apply(
        create(DELAY, CreateTarget::Master, CreateOptions::default()),
        &r,
    )
    .unwrap();
    assert_eq!(text(&a), text(&b));
    // Without a master the variant is "nothing selected".
    let mut e = Document::new(atomik_model::empty_doc());
    one_step(
        &mut e,
        &r,
        create(DELAY, CreateTarget::Master, CreateOptions::default()),
    );
    assert_eq!(e.doc().cables.len(), 0);
}

#[test]
fn a_channel_selection_still_gets_an_insert_and_the_master_never_does() {
    let r = reg();
    let mut d = with_sends(&r, 0);
    let ch = d.doc().console.channels[0].channel.clone();
    let before = keys(&d);
    one_step(
        &mut d,
        &r,
        create(
            DELAY,
            CreateTarget::Selected(ch.clone()),
            CreateOptions::default(),
        ),
    );
    let fx = newest(&d, &before);
    assert_eq!(insert_chain(d.doc(), &r, &ch), vec![key(&fx)]);
    assert!(returns(&d).iter().all(Option::is_none));
    one_step(&mut d, &r, on_master(REVERB));
    assert!(
        insert_chain(d.doc(), &r, &key("master")).is_empty(),
        "master inserts are not an auto-route target"
    );
    assert_eq!(returns(&d).iter().flatten().count(), 1);
}

#[test]
fn channel_send_switches_stay_off_by_default() {
    let r = reg();
    let d = with_sends(&r, 3);
    let ch = &d.doc().console.channels[0].channel;
    for k in 1..=8 {
        let v = d
            .doc()
            .param_value(ch, &format!("send{k}.on"), &r)
            .expect("a channel send switch");
        assert_eq!(v, 0.0, "send{k}.on");
    }
}

#[test]
fn identical_inputs_give_identical_saved_text() {
    let r = reg();
    let run_it = || {
        let mut d = with_sends(&r, 0);
        for t in [DELAY, REVERB, DELAY] {
            d.apply(on_master(t), &r).unwrap();
        }
        text(&d)
    };
    assert_eq!(run_it(), run_it());
}

#[test]
fn several_creates_undo_back_to_the_starting_document() {
    let r = reg();
    let mut d = project();
    let start = text(&d);
    let mut states = vec![start.clone()];
    for t in [DELAY, REVERB, DELAY, REVERB] {
        d.apply(on_master(t), &r).unwrap();
        states.push(text(&d));
    }
    for want in states.iter().rev().skip(1) {
        d.undo(&r).unwrap();
        assert_eq!(&text(&d), want);
    }
    for want in states.iter().skip(1) {
        d.redo(&r).unwrap();
        assert_eq!(&text(&d), want, "redo is byte-identical");
    }
}

// ------------------------------------------------------------------------- AssignSend

#[test]
fn use_as_send_effect_takes_the_first_free_slot_in_one_step() {
    let r = reg();
    let mut d = project();
    run(&mut d, &r, [add(usize::MAX, REVERB, Some("rev"))]);
    one_step(&mut d, &r, assign("rev"));
    assert_eq!(slot(&d, "rev"), Some(0));
    assert!(has(&d, "master:fx_send1_l", "rev:in_l"));
    assert!(has(&d, "rev:out_r", "master:return1_r"));
    assert_eq!(cables_of(&d, "rev"), 4);
    // A second one takes slot 2, skipping a slot used by another path.
    run(&mut d, &r, [add(usize::MAX, DELAY, Some("dly"))]);
    one_step(&mut d, &r, assign("dly"));
    assert_eq!(slot(&d, "dly"), Some(1));
}

#[test]
fn use_as_send_effect_skips_used_slots() {
    let r = reg();
    let mut d = with_sends(&r, 2);
    run(&mut d, &r, [add(usize::MAX, REVERB, Some("rev"))]);
    one_step(&mut d, &r, assign("rev"));
    assert_eq!(slot(&d, "rev"), Some(2));
}

#[test]
fn use_as_send_effect_rejections_say_why_and_change_nothing() {
    let r = reg();
    let mut d = with_sends(&r, 1);
    let on_return = returns(&d)[0].clone().unwrap();
    let ch = d.doc().console.channels[0].channel.clone();
    // Already on a return.
    expect_bad(
        rejected(&mut d, &r, assign(&on_return)),
        "is already the effect of send/return 1",
    );
    // In an insert chain.
    run(
        &mut d,
        &r,
        [
            add(usize::MAX, DELAY, Some("ins")),
            Command::Console(ConsoleCommand::SetInsert {
                channel: ch.clone(),
                slot: 0,
                effect: Some(atomik_model::InsertEffect::Existing(key("ins"))),
            }),
        ],
    );
    expect_bad(
        rejected(&mut d, &r, assign("ins")),
        "is already an insert effect",
    );
    // Console devices, an instrument, a device that is no stereo effect.
    for k in [ch.0.as_str(), "master", "synth"] {
        rejected(&mut d, &r, assign(k));
    }
    run(&mut d, &r, [add(usize::MAX, MIXER, Some("mix"))]);
    expect_bad(rejected(&mut d, &r, assign("mix")), "stereo effect");
    // An effect that has cables.
    run(
        &mut d,
        &r,
        [
            add(usize::MAX, DELAY, Some("wired")),
            connect("synth:out_l", "wired:in_l"),
        ],
    );
    expect_bad(rejected(&mut d, &r, assign("wired")), "connected");
    // Unknown device, inside a container.
    rejected(&mut d, &r, assign("ghost"));
    rejected(&mut d, &r, assign("a/b"));
}

#[test]
fn use_as_send_effect_with_all_eight_used_is_rejected_with_the_status_text() {
    let r = reg();
    let mut d = with_sends(&r, 8);
    run(&mut d, &r, [add(usize::MAX, REVERB, Some("rev"))]);
    expect_bad(
        rejected(&mut d, &r, assign("rev")),
        "All 8 send and return slots are in use",
    );
}

#[test]
fn use_as_send_effect_without_a_master_is_rejected() {
    let r = reg();
    let mut d = Document::new(atomik_model::empty_doc());
    run(&mut d, &r, [add(usize::MAX, REVERB, Some("rev"))]);
    rejected(&mut d, &r, assign("rev"));
}

// ------------------------------------------------------------------------- proptest

use proptest::prelude::*;

#[derive(Debug, Clone)]
enum Op {
    Create(usize, Option<usize>, bool),
    Assign(usize),
    AddLoose(usize),
    Remove(usize),
    Undo,
    Redo,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => (0usize..2, proptest::option::of(0usize..14), any::<bool>())
            .prop_map(|(t, s, route)| Op::Create(t, s, route)),
        3 => (0usize..14).prop_map(Op::Assign),
        2 => (0usize..2).prop_map(Op::AddLoose),
        2 => (0usize..14).prop_map(Op::Remove),
        1 => Just(Op::Undo),
        1 => Just(Op::Redo),
    ]
}

fn top(d: &Document) -> Vec<DeviceKey> {
    d.doc().devices.iter().map(|x| x.key.clone()).collect()
}

fn pick(list: &[DeviceKey], i: usize) -> Option<DeviceKey> {
    (!list.is_empty()).then(|| list[i % list.len()].clone())
}

fn resolve(o: &Op, d: &Document) -> Option<Command> {
    let keys = top(d);
    Some(match o {
        Op::Create(t, s, route) => create(
            [DELAY, REVERB][*t],
            if *t == 1 && s.is_none() {
                CreateTarget::Master
            } else {
                s.and_then(|i| pick(&keys, i))
                    .map_or(CreateTarget::Nothing, CreateTarget::Selected)
            },
            CreateOptions {
                route: *route,
                ..CreateOptions::default()
            },
        ),
        Op::Assign(i) => wf(WorkflowCommand::AssignSend {
            effect: pick(&keys, *i)?,
        }),
        Op::AddLoose(t) => add(usize::MAX, [DELAY, REVERB][*t], None),
        Op::Remove(i) => Command::RemoveDevice {
            key: pick(&keys, *i)?,
        },
        Op::Undo | Op::Redo => return None,
    })
}

/// Every slot holds an existing device with the four cables of a send loop.
fn consistent(d: &Document) {
    let doc = d.doc();
    let mut seen = Vec::new();
    for (i, r) in doc.console.returns.iter().enumerate() {
        let Some(fx) = r else { continue };
        assert!(doc.device(fx).is_some(), "slot {} holds a device", i + 1);
        assert!(!seen.contains(fx), "{fx} is on one slot only");
        seen.push(fx.clone());
        let k = i + 1;
        for side in ["l", "r"] {
            assert!(
                doc.cables.contains(&cable(
                    &format!("master:fx_send{k}_{side}"),
                    &format!("{}:in_{side}", fx.0)
                )),
                "send {k} feeds {fx}"
            );
            // The return is fed by the last effect of the loop (a chain of effects may follow
            // the one on the slot); a plain removal of a chain member may leave it unfed.
            let feeders = doc
                .cables
                .iter()
                .filter(|c| c.to == sock(&format!("master:return{k}_{side}")))
                .count();
            assert!(feeders <= 1, "return {k} {side} has at most one feeder");
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    #[test]
    fn create_assign_delete_undo_redo_sequences_stay_consistent(
        ops in proptest::collection::vec(op(), 1..40),
    ) {
        let r = reg();
        let mut d = project();
        let start = text(&d);
        run(&mut d, &r, [wf(WorkflowCommand::CreateInstrument {
            index: usize::MAX,
            source: typ(SYNTH),
            key: Some(key("lead")),
            options: CreateOptions::default(),
        })]);
        for o in &ops {
            match o {
                Op::Undo => { d.undo(&r); }
                Op::Redo => { d.redo(&r); }
                _ => {
                    let Some(cmd) = resolve(o, &d) else { continue };
                    let before = text(&d);
                    match d.apply(cmd, &r) {
                        Ok(_) => {
                            let after = text(&d);
                            d.undo(&r).expect("one step to undo");
                            prop_assert_eq!(text(&d), before);
                            d.redo(&r).expect("one step to redo");
                            prop_assert_eq!(text(&d), after);
                        }
                        Err(_) => prop_assert_eq!(text(&d), before, "a rejection changes nothing"),
                    }
                }
            }
            consistent(&d);
            prop_assert_eq!(d.doc().devices[0].key.clone(), key("master"));
        }
        while d.undo(&r).is_some() {}
        prop_assert_eq!(text(&d), start);
    }
}
