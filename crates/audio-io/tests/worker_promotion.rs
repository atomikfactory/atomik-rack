//! Worker-thread real-time promotion hook (for `WorkerConfig::on_thread_start`).

#[test]
fn promote_worker_thread_is_a_plain_fn_usable_as_on_thread_start() {
    let hook: fn() = atomik_audio_io::promote_worker_thread;
    let before = atomik_audio_io::worker_promotion_failures();
    std::thread::spawn(hook)
        .join()
        .expect("promotion must not panic");
    let after = atomik_audio_io::worker_promotion_failures();
    assert!(
        after == before || after == before + 1,
        "counts at most one failure per call"
    );
}
