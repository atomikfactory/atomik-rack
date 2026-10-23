//! HI-1: the processor's audio methods allocate nothing (the test binary installs the
//! allocation guard in `lib.rs`), on the answered path, the late path and the lost path.

use super::processor::{Bufs, active};
use super::*;
use crate::fake::AudioMode;
use atomik_plugin_host::{PluginInstance, ProcessStatus, SleepRule, TailLength};
use atomik_testkit::no_alloc::assert_no_alloc;

#[test]
fn process_start_stop_and_reset_never_allocate_with_a_child_answering() {
    let mut r = rig(FakeOptions::default());
    let mut p = active(&mut r, 100.0, RunMode::Realtime);
    let mut b = Bufs::new();
    // Warm up: first use of the block, the doorbell and the thread-locals.
    assert!(p.start_processing());
    assert_eq!(p.process(&mut b.io()), ProcessStatus::Continue);
    assert_no_alloc(|| {
        assert!(p.start_processing());
        for _ in 0..100 {
            assert_eq!(p.process(&mut b.io()), ProcessStatus::Continue);
        }
        p.reset();
        p.stop_processing();
        assert_eq!(p.sleep_rule(), SleepRule::PluginDecides);
        assert_eq!(p.tail_samples(), TailLength::Finite(0));
        let _ = p.wake_requested();
    });
}

#[test]
fn the_late_path_never_allocates() {
    let mut r = rig(FakeOptions::default());
    let mut p = active(&mut r, 500.0, RunMode::Realtime);
    let mut b = Bufs::new();
    assert_eq!(p.process(&mut b.io()), ProcessStatus::Continue);
    r.fake.ctl.set_audio(AudioMode::Stall);
    assert_no_alloc(|| {
        for _ in 0..5 {
            assert_eq!(p.process(&mut b.io()), ProcessStatus::Late);
        }
        assert!(!p.start_processing());
    });
}

#[test]
fn the_lost_path_never_allocates() {
    let mut r = rig(FakeOptions::default());
    let mut p = active(&mut r, 100.0, RunMode::Realtime);
    let mut b = Bufs::new();
    assert_eq!(p.process(&mut b.io()), ProcessStatus::Continue);
    r.fake
        .ctl
        .exit("the plugin process exited with code 9 (0x9)");
    eventually("the instance to be lost", || r.instance.is_lost());
    assert_no_alloc(|| {
        assert_eq!(p.process(&mut b.io()), ProcessStatus::Lost);
        assert!(!p.start_processing());
        p.stop_processing();
        p.reset();
    });
}

#[test]
fn a_late_batch_followed_by_a_submit_with_carried_events_never_allocates() {
    use atomik_plugin_host::{PluginEvent, PluginEventKind};
    let mut r = rig(FakeOptions::default());
    let mut p = active(&mut r, 500.0, RunMode::Realtime);
    let mut b = Bufs::new();
    let note = |offset| PluginEvent {
        offset,
        kind: PluginEventKind::NoteOff {
            key: 60,
            note_id: 1,
        },
    };
    b.events_in = vec![note(3), note(9)];
    // Warm up one carried exchange outside the guard.
    r.fake.ctl.set_audio(AudioMode::Stall);
    assert_eq!(p.process(&mut b.io()), ProcessStatus::Late);
    assert_eq!(p.process(&mut b.io()), ProcessStatus::Late);
    r.fake.ctl.set_audio(AudioMode::Echo);
    eventually("the old request", || r.fake.ctl.audio_ops().len() == 1);
    assert_eq!(p.process(&mut b.io()), ProcessStatus::Continue);
    r.fake.ctl.set_audio(AudioMode::Stall);
    assert_no_alloc(|| {
        assert_eq!(p.process(&mut b.io()), ProcessStatus::Late);
        assert_eq!(p.process(&mut b.io()), ProcessStatus::Late);
    });
    r.fake.ctl.set_audio(AudioMode::Echo);
    eventually("the old request", || r.fake.ctl.audio_ops().len() == 3);
    assert_no_alloc(|| {
        assert_eq!(p.process(&mut b.io()), ProcessStatus::Continue);
    });
    assert_eq!(r.fake.ctl.audio_events().last().map(Vec::len), Some(4));
}
