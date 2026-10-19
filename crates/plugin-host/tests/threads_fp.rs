//! (thread rules, FTZ/DAZ): the thread-check answers per thread through thread-local
//! guards, and every plugin `process` runs with flush-to-zero and denormals-are-zero set and
//! the flags restored afterwards.

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_plugin_host::{
    AudioThreadGuard, FtzGuard, denormals_flushed, is_audio_thread, is_main_thread,
    mark_main_thread,
};
use support::*;

#[test]
fn the_main_thread_is_the_one_that_marked_itself() {
    let marked = std::thread::spawn(|| {
        assert!(!is_main_thread());
        mark_main_thread();
        is_main_thread()
    })
    .join()
    .unwrap();
    assert!(marked);
    // Another thread is not the main thread just because one exists.
    let other = std::thread::spawn(is_main_thread).join().unwrap();
    assert!(!other);
    mark_main_thread();
    assert!(is_main_thread());
    let elsewhere = std::thread::spawn(is_main_thread).join().unwrap();
    assert!(!elsewhere, "marking is per thread");
}

#[test]
#[should_panic(expected = "main thread")]
fn a_main_thread_call_from_another_thread_trips_the_guard() {
    mark_main_thread();
    std::thread::spawn(|| atomik_plugin_host::assert_main_thread("test call"))
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e));
}

#[test]
fn the_audio_thread_guard_nests_and_is_per_thread() {
    assert!(!is_audio_thread());
    {
        let _a = AudioThreadGuard::enter();
        assert!(is_audio_thread());
        {
            let _b = AudioThreadGuard::enter();
            assert!(is_audio_thread());
        }
        assert!(is_audio_thread(), "still inside the outer guard");
        let elsewhere = std::thread::spawn(is_audio_thread).join().unwrap();
        assert!(!elsewhere);
    }
    assert!(!is_audio_thread());
}

#[test]
fn the_device_sets_the_audio_thread_guard_around_process_and_not_the_main_flag() {
    mark_main_thread();
    let (probe, shared) = Probe::new();
    let (mut h, _r) = harness(rig("th.guard", &fx_layout(), Box::new(probe)));
    h.process();
    h.process();
    assert!(!is_audio_thread(), "released after process");
    let log = shared.log.lock().unwrap();
    assert!(log.audio_thread_flags.iter().all(|f| *f));
    // The harness runs the device on the test thread, which marked itself main here: the two
    // checks are independent (a pinned plugin on the audio callback is "audio", not "main").
    assert!(log.main_thread_flags.iter().all(|f| *f));
}

#[test]
fn flush_to_zero_is_set_during_process_and_restored_afterwards() {
    let before = denormals_flushed();
    assert!(!before, "the test thread starts with denormals enabled");
    let (probe, shared) = Probe::new();
    let (mut h, _r) = harness(rig("th.ftz", &fx_layout(), Box::new(probe)));
    h.process();
    h.process();
    assert_eq!(denormals_flushed(), before, "restored");
    if atomik_plugin_host::FLUSH_SUPPORTED {
        let log = shared.log.lock().unwrap();
        assert!(log.flush_flags.iter().all(|f| *f), "set during process");
    }
}

#[test]
fn the_ftz_guard_flushes_denormal_results_and_restores_on_drop() {
    if !atomik_plugin_host::FLUSH_SUPPORTED {
        return;
    }
    let tiny = std::hint::black_box(f32::MIN_POSITIVE) * std::hint::black_box(0.5);
    assert!(tiny != 0.0, "a denormal is representable without the guard");
    {
        let _g = FtzGuard::enter();
        assert!(denormals_flushed());
        let flushed = std::hint::black_box(f32::MIN_POSITIVE) * std::hint::black_box(0.5);
        assert_eq!(flushed.to_bits(), 0, "flushed to zero");
        // Nested guards restore the outer state, not the default.
        {
            let _inner = FtzGuard::enter();
            assert!(denormals_flushed());
        }
        assert!(denormals_flushed());
    }
    assert!(!denormals_flushed());
    let again = std::hint::black_box(f32::MIN_POSITIVE) * std::hint::black_box(0.5);
    assert!(again != 0.0);
}
