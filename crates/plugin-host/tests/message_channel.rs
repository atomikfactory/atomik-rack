//! What a plugin tells the app is never lost to a full or busy ring: the flags, the queue of the
//! threads outside `process`, the reserve of the ring and the one order of all of them. Every
//! test here stalls the consumer (nothing is received until the end) or races it with joins and
//! counts, never with sleeps.

// Justification: the waits are bounded against the wall clock; only a failure reaches the bound.
#![allow(clippy::disallowed_methods)]

use atomik_plugin_host::{
    AudioThreadGuard, CHANGE_RESERVE, HostMessenger, InstanceId, LogLevel, MESSAGE_CAPACITY,
    ManagerEnd, ParamPhase, PluginMessage, QUEUE_HARD_CAP, QUEUE_SOFT_CAP, plugin_channel,
};
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

/// Bounds a wait that ends on its condition: only a failure waits this long.
const PATIENCE: Duration = Duration::from_secs(60);

fn channel() -> (ManagerEnd, HostMessenger) {
    let (_device, manager) = plugin_channel(InstanceId(1));
    let messenger = manager.messenger();
    (manager, messenger)
}

fn drain(manager: &ManagerEnd) -> Vec<PluginMessage> {
    std::iter::from_fn(|| manager.try_recv()).collect()
}

fn edit(id: u32, value: f64, phase: ParamPhase) -> PluginMessage {
    PluginMessage::ParamEdit { id, value, phase }
}

fn begin(id: u32) -> PluginMessage {
    edit(id, 0.0, ParamPhase::Begin)
}

fn end(id: u32) -> PluginMessage {
    edit(id, 0.0, ParamPhase::End)
}

fn change(id: u32, value: f64) -> PluginMessage {
    edit(id, value, ParamPhase::Change)
}

fn log(text: &str) -> PluginMessage {
    PluginMessage::Log {
        level: LogLevel::Info,
        text: text.to_string(),
    }
}

/// Posts `messages` from a thread inside a plugin's `process`, and waits for it.
fn post_from_audio_thread(messenger: &HostMessenger, messages: Vec<PluginMessage>) -> Vec<bool> {
    let messenger = messenger.clone();
    std::thread::spawn(move || {
        let _audio = AudioThreadGuard::enter();
        messages.into_iter().map(|m| messenger.post(m)).collect()
    })
    .join()
    .unwrap()
}

// ----- the flags ---------------------------------------------------------------------------------

#[test]
fn a_flag_posted_five_times_arrives_once() {
    let (manager, messenger) = channel();
    for _ in 0..5 {
        assert!(messenger.post(PluginMessage::StateDirty));
    }
    assert_eq!(drain(&manager), vec![PluginMessage::StateDirty]);
    assert_eq!(messenger.dropped(), 0);
    // Once taken, it can be said again.
    assert!(messenger.post(PluginMessage::StateDirty));
    assert_eq!(drain(&manager), vec![PluginMessage::StateDirty]);
}

#[test]
fn all_five_flags_arrive_in_a_fixed_order_before_the_other_messages() {
    let (manager, messenger) = channel();
    assert!(messenger.post(PluginMessage::GuiClosed));
    for m in [
        PluginMessage::RequestCallback,
        PluginMessage::RequestRestart,
        PluginMessage::RescanParams,
        PluginMessage::LatencyChanged,
        PluginMessage::StateDirty,
    ] {
        assert!(messenger.post(m));
    }
    assert_eq!(
        drain(&manager),
        vec![
            PluginMessage::StateDirty,
            PluginMessage::LatencyChanged,
            PluginMessage::RescanParams,
            PluginMessage::RequestRestart,
            PluginMessage::RequestCallback,
            PluginMessage::GuiClosed,
        ]
    );
}

#[test]
fn a_flag_is_accepted_from_the_audio_thread_even_when_the_ring_is_full() {
    let (manager, messenger) = channel();
    let fill = vec![PluginMessage::GuiClosed; MESSAGE_CAPACITY + 10];
    let took = post_from_audio_thread(&messenger, fill);
    assert_eq!(took.iter().filter(|t| **t).count(), MESSAGE_CAPACITY);
    let took = post_from_audio_thread(&messenger, vec![PluginMessage::LatencyChanged]);
    assert_eq!(took, vec![true]);
    assert_eq!(manager.try_recv(), Some(PluginMessage::LatencyChanged));
}

// ----- the queue of the threads outside `process` --------------------------------------------------

#[test]
fn two_threads_posting_at_once_lose_nothing() {
    for _ in 0..20 {
        let (manager, messenger) = channel();
        let barrier = Arc::new(Barrier::new(2));
        let threads: Vec<_> = (0..2u32)
            .map(|t| {
                let messenger = messenger.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    for n in 0..10_000u32 {
                        let id = t * 100_000 + n;
                        assert!(messenger.post(begin(id)));
                        assert!(messenger.post(end(id)));
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(messenger.dropped(), 0);
        let got = drain(&manager);
        assert_eq!(got.len(), 40_000);
        // Each thread's messages are all there, in its own order.
        for t in 0..2u32 {
            let mine: Vec<_> = got
                .iter()
                .filter_map(|m| match m {
                    PluginMessage::ParamEdit { id, phase, .. } if *id / 100_000 == t => {
                        Some((*id, *phase))
                    }
                    _ => None,
                })
                .collect();
            let want: Vec<_> = (0..10_000u32)
                .flat_map(|n| {
                    let id = t * 100_000 + n;
                    [(id, ParamPhase::Begin), (id, ParamPhase::End)]
                })
                .collect();
            assert_eq!(mine, want, "thread {t}");
        }
    }
}

#[test]
fn a_flood_of_changes_in_process_takes_nothing_from_the_other_threads() {
    for _ in 0..20 {
        let (manager, messenger) = channel();
        let barrier = Arc::new(Barrier::new(2));
        let flood = {
            let messenger = messenger.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let _audio = AudioThreadGuard::enter();
                barrier.wait();
                for n in 0..100_000u32 {
                    messenger.post(change(n % 64, f64::from(n)));
                }
            })
        };
        // What the other threads of the plugin and of the host say meanwhile.
        let mut sent = Vec::new();
        let talker = {
            let messenger = messenger.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                for m in [
                    PluginMessage::StateDirty,
                    PluginMessage::LatencyChanged,
                    PluginMessage::RescanParams,
                    PluginMessage::RequestRestart,
                    PluginMessage::RequestCallback,
                ] {
                    assert!(messenger.post(m));
                }
                let mut sent = Vec::new();
                for n in 0..40u32 {
                    let id = 1_000 + n;
                    for m in [begin(id), change(id, 0.5), end(id)] {
                        assert!(messenger.post(m.clone()));
                        sent.push(m);
                    }
                }
                for m in [
                    PluginMessage::GuiResize { w: 3, h: 4 },
                    PluginMessage::GuiFrame {
                        x: 1,
                        y: 2,
                        w: 3,
                        h: 4,
                    },
                    PluginMessage::GuiRequestShow,
                    PluginMessage::GuiRequestHide,
                    PluginMessage::GuiClosed,
                    PluginMessage::Failed { reason: "gone" },
                    PluginMessage::ProcessExited {
                        reason: "exit code 3".to_string(),
                    },
                ] {
                    assert!(messenger.post(m.clone()));
                    sent.push(m);
                }
                sent
            })
        };
        flood.join().unwrap();
        sent.extend(talker.join().unwrap());

        let got = drain(&manager);
        // The flags first, each once.
        assert_eq!(
            got[..5],
            [
                PluginMessage::StateDirty,
                PluginMessage::LatencyChanged,
                PluginMessage::RescanParams,
                PluginMessage::RequestRestart,
                PluginMessage::RequestCallback,
            ]
        );
        // Every other message of the second thread arrived, in order.
        let theirs: Vec<_> = got[5..]
            .iter()
            .filter(|m| !matches!(m, PluginMessage::ParamEdit { id, .. } if *id < 64))
            .cloned()
            .collect();
        assert_eq!(theirs, sent);
        // The changes that arrived are some of those posted, and the rest were counted.
        let changes = got
            .iter()
            .filter(|m| matches!(m, PluginMessage::ParamEdit { id, phase: ParamPhase::Change, .. } if *id < 64))
            .count();
        assert!(changes > 0);
        assert!(changes <= MESSAGE_CAPACITY - CHANGE_RESERVE, "{changes}");
        assert_eq!(messenger.dropped(), (100_000 - changes) as u64);
    }
}

// ----- tail replacement --------------------------------------------------------------------------------

#[test]
fn a_drag_holds_one_message() {
    let (manager, messenger) = channel();
    assert!(messenger.post(begin(7)));
    for n in 0..1_000 {
        assert!(messenger.post(change(7, f64::from(n))));
    }
    assert!(messenger.post(end(7)));
    assert_eq!(
        drain(&manager),
        vec![begin(7), change(7, 999.0), end(7)],
        "the end of the drag still has the last value set"
    );
    assert_eq!(messenger.dropped(), 0);
}

#[test]
fn a_change_is_never_merged_across_a_begin_an_end_or_another_parameter() {
    let (manager, messenger) = channel();
    let posts = [
        change(7, 1.0),
        begin(7),
        change(7, 2.0),
        change(7, 3.0),
        end(7),
        change(7, 4.0),
        change(8, 5.0),
        change(7, 6.0),
        log("a line"),
        change(7, 7.0),
        change(7, 8.0),
    ];
    for m in posts {
        assert!(messenger.post(m));
    }
    assert_eq!(
        drain(&manager),
        vec![
            change(7, 1.0),
            begin(7),
            change(7, 3.0),
            end(7),
            change(7, 4.0),
            change(8, 5.0),
            change(7, 6.0),
            log("a line"),
            change(7, 8.0),
        ]
    );
}

#[test]
fn a_change_is_not_merged_across_a_message_the_ring_took_in_between() {
    let (manager, messenger) = channel();
    assert!(messenger.post(change(7, 1.0)));
    // The plugin's audio thread says the gesture begins, between the two changes.
    assert_eq!(post_from_audio_thread(&messenger, vec![begin(7)]), [true]);
    assert!(messenger.post(change(7, 2.0)));
    assert_eq!(
        drain(&manager),
        vec![change(7, 1.0), begin(7), change(7, 2.0)]
    );
}

// ----- the caps --------------------------------------------------------------------------------------------

#[test]
fn past_the_soft_cap_a_change_and_a_log_are_dropped_and_the_rest_is_queued() {
    let (manager, messenger) = channel();
    // Changes of two parameters in turn cannot replace each other.
    for n in 0..QUEUE_SOFT_CAP as u32 {
        assert!(messenger.post(change(n % 2, f64::from(n))));
    }
    assert!(!messenger.post(change(0, -1.0)));
    // The same parameter as the last message: it replaces it, nothing grows.
    assert!(messenger.post(change(1, -1.0)));
    assert!(!messenger.post(log("too many")));
    assert_eq!(messenger.dropped(), 2);
    for m in [
        begin(5),
        end(5),
        PluginMessage::GuiClosed,
        PluginMessage::Failed { reason: "late" },
        PluginMessage::ProcessExited {
            reason: "gone".to_string(),
        },
        PluginMessage::StateDirty,
    ] {
        assert!(messenger.post(m), "still queued");
    }
    assert_eq!(messenger.dropped(), 2);
    let got = drain(&manager);
    assert_eq!(got.len(), QUEUE_SOFT_CAP + 6);
    assert_eq!(got[0], PluginMessage::StateDirty);
    assert_eq!(
        got[got.len() - 1],
        PluginMessage::ProcessExited {
            reason: "gone".to_string(),
        }
    );
}

#[test]
fn past_the_hard_cap_everything_is_dropped_and_counted() {
    let (manager, messenger) = channel();
    for _ in 0..QUEUE_HARD_CAP {
        assert!(messenger.post(PluginMessage::GuiClosed));
    }
    assert!(!messenger.post(PluginMessage::GuiClosed));
    assert!(!messenger.post(PluginMessage::Failed { reason: "no room" }));
    assert_eq!(messenger.dropped(), 2);
    // A flag has no room to run out of.
    assert!(messenger.post(PluginMessage::StateDirty));
    assert_eq!(manager.try_recv(), Some(PluginMessage::StateDirty));
    assert_eq!(drain(&manager).len(), QUEUE_HARD_CAP);
    // Draining made room.
    assert!(messenger.post(PluginMessage::GuiClosed));
}

// ----- the ring, inside process ---------------------------------------------------------------------------

#[test]
fn a_flood_of_changes_leaves_the_reserve_to_the_other_messages() {
    let (manager, messenger) = channel();
    let mut posts: Vec<PluginMessage> = (0..MESSAGE_CAPACITY as u32 * 2)
        .map(|n| change(n % 64, f64::from(n)))
        .collect();
    // Then the messages the audio path must be able to say.
    posts.push(begin(9));
    posts.push(end(9));
    posts.push(log("from process"));
    posts.push(PluginMessage::Failed { reason: "broken" });
    let took = post_from_audio_thread(&messenger, posts);
    let changes = MESSAGE_CAPACITY * 2;
    assert_eq!(
        took[..changes].iter().filter(|t| **t).count(),
        MESSAGE_CAPACITY - CHANGE_RESERVE
    );
    assert_eq!(took[changes..], [true; 4], "a begin still fits");
    let got = drain(&manager);
    assert_eq!(got.len(), MESSAGE_CAPACITY - CHANGE_RESERVE + 4);
    assert_eq!(got[got.len() - 4], begin(9));
    assert_eq!(
        got[got.len() - 1],
        PluginMessage::Failed { reason: "broken" }
    );
    assert_eq!(
        messenger.dropped(),
        (changes - (MESSAGE_CAPACITY - CHANGE_RESERVE)) as u64
    );
}

#[test]
fn the_reserve_is_finite_and_a_full_ring_refuses_and_counts() {
    let (manager, messenger) = channel();
    let took = post_from_audio_thread(
        &messenger,
        vec![PluginMessage::GuiClosed; MESSAGE_CAPACITY + 5],
    );
    assert_eq!(took.iter().filter(|t| **t).count(), MESSAGE_CAPACITY);
    assert_eq!(messenger.dropped(), 5);
    assert_eq!(drain(&manager).len(), MESSAGE_CAPACITY);
    // Draining makes room again.
    assert_eq!(post_from_audio_thread(&messenger, vec![begin(1)]), [true]);
}

// ----- one order --------------------------------------------------------------------------------------------

#[test]
fn the_ring_and_the_queue_come_out_in_the_order_they_were_posted() {
    let (manager, messenger) = channel();
    // The queue, the ring, the queue, the ring, one after the other.
    assert!(messenger.post(begin(1)));
    assert_eq!(post_from_audio_thread(&messenger, vec![begin(2)]), [true]);
    assert!(messenger.post(end(1)));
    assert_eq!(
        post_from_audio_thread(&messenger, vec![end(2), log("x")]),
        [true, true]
    );
    assert!(messenger.post(PluginMessage::GuiClosed));
    assert_eq!(
        drain(&manager),
        vec![
            begin(1),
            begin(2),
            end(1),
            end(2),
            log("x"),
            PluginMessage::GuiClosed
        ]
    );
}

#[test]
fn a_gesture_that_begins_in_process_and_ends_outside_it_keeps_its_order() {
    // And the reverse: the order is the posting order whichever road each message took.
    for reverse in [false, true] {
        let (manager, messenger) = channel();
        let (first, second) = if reverse { (0, 1) } else { (1, 0) };
        for step in [first, second] {
            let message = if step == 1 { begin(3) } else { end(3) };
            if reverse == (step == 1) {
                assert_eq!(post_from_audio_thread(&messenger, vec![message]), [true]);
            } else {
                assert!(messenger.post(message));
            }
        }
        let got = drain(&manager);
        let want = if reverse {
            vec![end(3), begin(3)]
        } else {
            vec![begin(3), end(3)]
        };
        assert_eq!(got, want);
    }
}

#[test]
fn a_consumer_that_drains_while_two_roads_post_sees_each_roads_order() {
    for _ in 0..20 {
        let (manager, messenger) = channel();
        let barrier = Arc::new(Barrier::new(3));
        let audio = {
            let messenger = messenger.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let _audio = AudioThreadGuard::enter();
                barrier.wait();
                // The ring is small and the consumer is running: retry what it refused.
                let limit = Instant::now() + PATIENCE;
                for n in 0..2_000u32 {
                    while !messenger.post(begin(n)) {
                        assert!(Instant::now() < limit, "the ring stayed full");
                        std::thread::yield_now();
                    }
                }
            })
        };
        let other = {
            let messenger = messenger.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                for n in 0..2_000u32 {
                    assert!(messenger.post(end(100_000 + n)));
                }
            })
        };
        barrier.wait();
        let (mut a, mut b) = (0u32, 0u32);
        let limit = Instant::now() + PATIENCE;
        while a < 2_000 || b < 2_000 {
            assert!(Instant::now() < limit, "lost messages: {a} and {b} arrived");
            match manager.try_recv() {
                Some(PluginMessage::ParamEdit {
                    id,
                    phase: ParamPhase::Begin,
                    ..
                }) => {
                    assert_eq!(id, a, "the ring's messages in order");
                    a += 1;
                }
                Some(PluginMessage::ParamEdit {
                    id,
                    phase: ParamPhase::End,
                    ..
                }) => {
                    assert_eq!(id, 100_000 + b, "the queue's messages in order");
                    b += 1;
                }
                Some(other) => panic!("{other:?}"),
                None => std::thread::yield_now(),
            }
        }
        audio.join().unwrap();
        other.join().unwrap();
        assert!(manager.try_recv().is_none());
    }
}
