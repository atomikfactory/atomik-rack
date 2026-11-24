use super::*;
use atomik_audio_file::AudioData;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

fn temp_bundle(name: &str) -> Bundle {
    let dir = std::env::temp_dir().join(format!(
        "atomik-stream-source-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    Bundle::create(&dir).unwrap()
}

fn signal(channels: usize, frames: usize) -> AudioData {
    let chans = (0..channels)
        .map(|c| {
            (0..frames)
                .map(|i| ((i * 7 + c * 1_000) % 9_973) as f32 / 4_000.0 - 1.0)
                .collect()
        })
        .collect();
    AudioData::new(48_000, chans).unwrap()
}

#[test]
fn the_threshold_is_64_mib_of_decoded_f32_and_the_comparison_is_strict() {
    assert_eq!(STREAM_THRESHOLD_BYTES, 64 * 1024 * 1024);
    assert_eq!(decoded_bytes(100, 2), 800);
    assert_eq!(decoded_bytes(u64::MAX, 2), u64::MAX, "saturates");
    // 64 MiB of stereo f32 is 8 388 608 frames.
    let frames = STREAM_THRESHOLD_BYTES / 8;
    assert!(
        !should_stream(frames, 2, STREAM_THRESHOLD_BYTES),
        "exactly at the limit stays in RAM"
    );
    assert!(should_stream(frames + 1, 2, STREAM_THRESHOLD_BYTES));
    // Mono takes half the room per frame.
    assert!(!should_stream(frames + 1, 1, STREAM_THRESHOLD_BYTES));
    assert!(should_stream(2 * frames + 1, 1, STREAM_THRESHOLD_BYTES));
    // The override makes anything stream.
    assert!(should_stream(1, 1, 0));
    assert!(!should_stream(0, 2, 0));
}

#[test]
fn the_bundle_source_returns_the_file_frames_bit_exactly() {
    let mut bundle = temp_bundle("exact");
    let data = signal(2, 10_000);
    let info = bundle.add_recording(&data, "t").unwrap();
    let asset = open_stream_asset(&bundle, &info.hash).unwrap();
    assert!(asset.is_stream());
    assert_eq!((asset.frames(), asset.channels()), (10_000, 2));
    let SongAsset::Stream { source, .. } = asset else {
        panic!("streamed");
    };
    for (start, want) in [
        (0u64, 300usize),
        (4_321, 512),
        (9_900, 300),
        (10_000, 16),
        (20_000, 8),
    ] {
        let mut out = vec![vec![9.0f32; want]; 2];
        let n = source.read(start, &mut out);
        let avail = 10_000u64.saturating_sub(start).min(want as u64) as usize;
        assert_eq!(n, avail, "start {start}");
        for (c, ch) in out.iter().enumerate() {
            let from = (start as usize).min(10_000);
            let expect = &data.channels[c][from..from + avail];
            let bits = |v: &[f32]| v.iter().map(|s| s.to_bits()).collect::<Vec<_>>();
            assert_eq!(
                bits(&ch[..avail]),
                bits(expect),
                "start {start} channel {c}"
            );
        }
    }
    let _ = std::fs::remove_dir_all(bundle.root());
}

#[test]
fn a_missing_file_is_an_error_not_a_panic() {
    let bundle = temp_bundle("missing");
    let hash = "0".repeat(64);
    assert!(open_stream_asset(&bundle, &hash).is_err());
    assert!(open_stream_asset(&bundle, "not-a-hash").is_err());
    let _ = std::fs::remove_dir_all(bundle.root());
}

/// A reader that sleeps per read and can start failing.
struct Slow {
    delay: Duration,
    fail_after: u64,
    reads: u64,
    reads_seen: std::sync::Arc<AtomicU64>,
}

impl FrameReader for Slow {
    fn read_frames(&mut self, start: u64, out: &mut [Vec<f32>]) -> Result<usize, String> {
        std::thread::sleep(self.delay);
        self.reads += 1;
        self.reads_seen.fetch_add(1, Ordering::Relaxed);
        if self.reads > self.fail_after {
            return Err("disk went away".into());
        }
        for ch in out.iter_mut() {
            for (i, s) in ch.iter_mut().enumerate() {
                *s = (start + i as u64) as f32;
            }
        }
        Ok(out[0].len())
    }
}

#[test]
fn a_failing_reader_plays_as_silence_and_is_counted() {
    let seen = std::sync::Arc::new(AtomicU64::new(0));
    let source = ReaderSource::new(Slow {
        delay: Duration::ZERO,
        fail_after: 1,
        reads: 0,
        reads_seen: seen,
    });
    let mut out = vec![vec![0.0f32; 4]; 1];
    assert_eq!(source.read(10, &mut out), 4);
    assert_eq!(out[0], vec![10.0, 11.0, 12.0, 13.0]);
    assert_eq!(source.errors(), 0);
    assert_eq!(
        source.read(10, &mut out),
        0,
        "an error reads as the end of the file"
    );
    assert_eq!(source.errors(), 1);
}

#[test]
fn a_slow_reader_is_only_ever_waited_for_on_the_calling_thread() {
    // The engine calls `read` from its streamer thread only; here the adapter is driven from two
    // threads: reads serialise on the mutex and none is lost.
    let seen = std::sync::Arc::new(AtomicU64::new(0));
    let source = std::sync::Arc::new(ReaderSource::new(Slow {
        delay: Duration::from_millis(5),
        fail_after: u64::MAX,
        reads: 0,
        reads_seen: std::sync::Arc::clone(&seen),
    }));
    let t0 = test_now();
    let handles: Vec<_> = (0..2)
        .map(|k| {
            let s = std::sync::Arc::clone(&source);
            std::thread::spawn(move || {
                let mut out = vec![vec![0.0f32; 2]; 1];
                for i in 0..4u64 {
                    assert_eq!(s.read(k * 100 + i, &mut out), 2);
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(seen.load(Ordering::Relaxed), 8);
    assert!(
        t0.elapsed() >= Duration::from_millis(35),
        "8 serialised reads of 5 ms"
    );
}

#[allow(clippy::disallowed_methods)] // Justification: test-only timing of blocking behaviour.
fn test_now() -> std::time::Instant {
    std::time::Instant::now()
}

// ------------------------------------------------------------------ the session budget

const MIB: u64 = 1 << 20;

fn item(key: &str, bytes: u64) -> Candidate<String> {
    Candidate {
        key: key.to_owned(),
        bytes,
        may_stream: true,
    }
}

fn keys(set: &BTreeSet<String>) -> Vec<&str> {
    set.iter().map(String::as_str).collect()
}

fn rule(budget: Option<u64>) -> StreamRule {
    StreamRule::new(STREAM_THRESHOLD_BYTES, budget, 0)
}

#[test]
fn under_the_budget_nothing_changes_and_no_budget_is_the_threshold_rule() {
    let items: Vec<_> = (0..10).map(|i| item(&format!("s{i}"), 50 * MIB)).collect();
    assert!(
        plan(&items, rule(Some(500 * MIB))).is_empty(),
        "exactly fits"
    );
    assert!(plan(&items, rule(None)).is_empty());
    // Today's rule: only what is above the threshold streams.
    let mixed = vec![
        item("a", 70 * MIB),
        item("b", 64 * MIB),
        item("c", 10 * MIB),
    ];
    let by_rule: BTreeSet<String> = mixed
        .iter()
        .filter(|i| i.bytes > STREAM_THRESHOLD_BYTES)
        .map(|i| i.key.clone())
        .collect();
    assert_eq!(plan(&mixed, rule(None)), by_rule);
    assert_eq!(keys(&by_rule), ["a"]);
    // A budget the whole list fits changes nothing either.
    assert_eq!(plan(&mixed, rule(Some(u64::MAX))), by_rule);
}

#[test]
fn forty_equal_stems_against_a_budget_for_ten_stream_thirty_in_key_order() {
    let items: Vec<_> = (0..40)
        .map(|i| item(&format!("s{i:02}"), 55 * MIB))
        .collect();
    let streams = plan(&items, rule(Some(10 * 55 * MIB)));
    assert_eq!(streams.len(), 30);
    // Equal sizes: the ties break by key, so the first thirty keys stream.
    assert!(streams.contains("s00") && streams.contains("s29"));
    assert!(!streams.contains("s30") && !streams.contains("s39"));
    // Any order of the list gives the same set.
    let mut reversed = items.clone();
    reversed.reverse();
    assert_eq!(plan(&reversed, rule(Some(10 * 55 * MIB))), streams);
}

#[test]
fn the_largest_stream_first_and_streaming_stops_when_the_rest_fits() {
    let items = vec![
        item("small", 6 * MIB),
        item("mid", 30 * MIB),
        item("big", 60 * MIB),
        item("huge", 64 * MIB),
    ];
    // 160 MiB held, 100 allowed: the biggest goes (96 left), nothing more.
    assert_eq!(keys(&plan(&items, rule(Some(100 * MIB)))), ["huge"]);
    // 70 allowed: huge, then big (36 left).
    assert_eq!(keys(&plan(&items, rule(Some(70 * MIB)))), ["big", "huge"]);
    // What the session holds already counts.
    let held = StreamRule::new(STREAM_THRESHOLD_BYTES, Some(100 * MIB), 20 * MIB);
    assert_eq!(keys(&plan(&items, held)), ["big", "huge"]);
}

#[test]
fn assets_at_or_below_the_floor_never_stream_whatever_the_budget() {
    let items = vec![
        item("a", STREAM_FLOOR_BYTES),
        item("b", STREAM_FLOOR_BYTES - 1),
        item("c", 1),
    ];
    assert!(plan(&items, rule(Some(0))).is_empty());
    let with_one_above = [items, vec![item("d", STREAM_FLOOR_BYTES + 1)]].concat();
    assert_eq!(keys(&plan(&with_one_above, rule(Some(0)))), ["d"]);
}

#[test]
fn assets_that_may_not_stream_stay_and_do_not_make_others_stream_beyond_need() {
    let mut zone = item("zone", 50 * MIB);
    zone.may_stream = false;
    let items = vec![zone.clone(), item("a", 40 * MIB), item("b", 30 * MIB)];
    // 120 held, 90 allowed: the zone asset is the largest but stays; "a" streams (80 left).
    let streams = plan(&items, rule(Some(90 * MIB)));
    assert_eq!(keys(&streams), ["a"]);
    // The zone asset alone is over the budget: everything that can move moves, it stays.
    let streams = plan(&items, rule(Some(10 * MIB)));
    assert_eq!(keys(&streams), ["a", "b"]);
    // Above the threshold the old rule still applies to it.
    let mut huge = item("huge", 70 * MIB);
    huge.may_stream = false;
    assert_eq!(keys(&plan(&[huge], rule(None))), ["huge"]);
}

#[test]
fn one_more_asset_streams_when_keeping_it_would_exceed_the_budget() {
    let mut r = StreamRule::new(STREAM_THRESHOLD_BYTES, Some(100 * MIB), 0);
    let frames = |mib: u64| mib * MIB / 4; // mono
    assert!(!r.admit(frames(60), 1, true));
    assert_eq!(r.resident, 60 * MIB);
    // 60 + 50 > 100: this one streams and is not counted.
    assert!(r.admit(frames(50), 1, true));
    assert_eq!(r.resident, 60 * MIB);
    // One that fits after all is kept.
    assert!(!r.admit(frames(40), 1, true));
    // Over the budget, but below the floor or exempt: stays.
    assert!(!r.admit(frames(4), 1, true));
    assert!(!r.admit(frames(30), 1, false));
    assert_eq!(r.resident, 134 * MIB);
    // Above the threshold: streams whatever the flag.
    assert!(r.admit(frames(70), 1, false));
    // Without a budget it is should_stream.
    let mut none = rule(None);
    for (f, ch) in [
        (1u64, 1u16),
        (10_000_000, 2),
        (8_388_608, 2),
        (8_388_609, 2),
    ] {
        assert_eq!(
            none.admit(f, ch, true),
            should_stream(f, ch, STREAM_THRESHOLD_BYTES)
        );
    }
}

#[test]
fn a_machine_with_plenty_of_memory_decides_as_the_threshold_rule_for_ordinary_projects() {
    let budget = crate::memory_limits::audio_budget_for(Some(16 << 30));
    let table = [
        (1_000u64, 1u16),
        (1_000_000, 2),
        (8_388_608, 2),
        (8_388_609, 2),
        (30_000_000, 1),
        (16_777_216, 1),
        (16_777_217, 1),
    ];
    let mut r = StreamRule::new(STREAM_THRESHOLD_BYTES, budget, 0);
    for (frames, channels) in table {
        assert_eq!(
            r.admit(frames, channels, true),
            should_stream(frames, channels, STREAM_THRESHOLD_BYTES),
            "{frames} x {channels}"
        );
    }
}
