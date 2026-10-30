use super::*;
use atomik_device_sdk::MidiEvent;
use std::sync::Mutex;

/// What a fake port was sent, with the order of arrival.
type Log = Arc<Mutex<Vec<(String, Vec<u8>)>>>;

struct Fake {
    name: String,
    log: Log,
    fail_next: Arc<Mutex<u32>>,
}

impl MidiOutPort for Fake {
    fn send(&mut self, bytes: &[u8]) -> Result<(), MidiError> {
        let mut fails = self.fail_next.lock().unwrap();
        if *fails > 0 {
            *fails -= 1;
            return Err(MidiError::Backend("unplugged".into()));
        }
        self.log
            .lock()
            .unwrap()
            .push((self.name.clone(), bytes.to_vec()));
        Ok(())
    }
}

struct Rig {
    log: Log,
    opens: Arc<Mutex<Vec<String>>>,
    missing: Arc<Mutex<Vec<String>>>,
    fail_next: Arc<Mutex<u32>>,
}

impl Rig {
    fn new() -> (Self, Opener) {
        let rig = Self {
            log: Arc::default(),
            opens: Arc::default(),
            missing: Arc::default(),
            fail_next: Arc::default(),
        };
        let (log, opens, missing, fail) = (
            Arc::clone(&rig.log),
            Arc::clone(&rig.opens),
            Arc::clone(&rig.missing),
            Arc::clone(&rig.fail_next),
        );
        let opener: Opener = Arc::new(move |name| {
            opens.lock().unwrap().push(name.to_owned());
            if missing.lock().unwrap().iter().any(|m| m == name) {
                return Err(MidiError::PortNotFound(name.to_owned()));
            }
            Ok(Box::new(Fake {
                name: name.to_owned(),
                log: Arc::clone(&log),
                fail_next: Arc::clone(&fail),
            }))
        });
        (rig, opener)
    }

    fn sent(&self) -> Vec<(String, Vec<u8>)> {
        std::mem::take(&mut *self.log.lock().unwrap())
    }
}

fn ms(base: Instant, n: u64) -> Instant {
    base + Duration::from_millis(n)
}

#[allow(clippy::disallowed_methods)] // Justification: a test anchor; nothing reads the wall clock.
fn base() -> Instant {
    Instant::now()
}

#[test]
fn a_frame_is_due_when_it_is_heard() {
    let t0 = base();
    let anchor = (1000, t0);
    // The anchor frame is heard one output latency after the callback started.
    let at = frame_due(anchor, 1000, 48_000.0, 480);
    assert_eq!(at - t0, Duration::from_millis(10));
    let at = frame_due(anchor, 1000 + 48_000, 48_000.0, 480);
    assert_eq!(at - t0, Duration::from_millis(1010));
    // A frame before the anchor is earlier, also before the anchor itself.
    let early = frame_due(anchor, 1000 - 960, 48_000.0, 0);
    assert_eq!(t0 - early, Duration::from_millis(20));
    let before = frame_due(anchor, 0, 48_000.0, 0);
    assert!(before < t0);
    // Garbage in does not panic.
    let _ = frame_due((u64::MAX, t0), 0, 0.0, u64::MAX);
}

#[test]
fn the_timeline_waits_for_the_clock_and_follows_the_latency() {
    let clock = Arc::new(FrameClock::new());
    let latency = Arc::new(AtomicU64::new(0));
    let timeline = Timeline::new(Arc::clone(&clock), 48_000.0, Arc::clone(&latency));
    assert_eq!(timeline.due(5), None, "nothing published yet");
    let t0 = base();
    clock.publish(0, t0);
    assert_eq!(timeline.due(480).unwrap() - t0, Duration::from_millis(10));
    latency.store(480, Ordering::Relaxed);
    assert_eq!(timeline.due(480).unwrap() - t0, Duration::from_millis(20));
    assert_eq!(timeline.sample_rate(), 48_000.0);
}

#[test]
fn messages_leave_in_time_order_and_only_when_due() {
    let (rig, opener) = Rig::new();
    let mut s = Scheduler::new(opener);
    let t0 = base();
    s.enqueue("A", 1, ms(t0, 20), &[0x80, 60, 0]);
    s.enqueue("A", 1, ms(t0, 10), &[0x90, 60, 100]);
    s.enqueue("A", 1, ms(t0, 10), &[0x90, 64, 100]);
    s.enqueue("B", 2, ms(t0, 15), &[0xB0, 7, 1]);
    assert_eq!(s.pump(t0), Some(ms(t0, 10)));
    assert!(rig.sent().is_empty());
    assert_eq!(s.pump(ms(t0, 12)), Some(ms(t0, 15)));
    assert_eq!(
        rig.sent(),
        vec![
            ("A".into(), vec![0x90, 60, 100]),
            ("A".into(), vec![0x90, 64, 100]),
        ],
        "equal times keep the order they were queued in"
    );
    assert_eq!(s.pump(ms(t0, 16)), Some(ms(t0, 20)));
    assert_eq!(s.pump(ms(t0, 30)), None);
    assert_eq!(
        rig.sent(),
        vec![
            ("B".into(), vec![0xB0, 7, 1]),
            ("A".into(), vec![0x80, 60, 0]),
        ]
    );
    assert_eq!(s.stats().sent, 4);
    assert_eq!(
        *rig.opens.lock().unwrap(),
        ["A", "B"],
        "each port opens once"
    );
}

#[test]
fn a_late_message_is_counted() {
    let (_rig, opener) = Rig::new();
    let mut s = Scheduler::new(opener);
    let t0 = base();
    s.enqueue("A", 1, t0, &[0xB0, 1, 1]);
    s.enqueue("A", 1, ms(t0, 4), &[0xB0, 1, 2]);
    s.pump(ms(t0, 5));
    let stats = s.stats();
    assert_eq!(
        (stats.sent, stats.late),
        (2, 1),
        "5 ms late, then 1 ms late"
    );
    assert_eq!(stats.max_late_us, 5_000);
}

#[test]
fn releasing_a_source_sends_its_note_offs_and_drops_its_queue() {
    let (rig, opener) = Rig::new();
    let mut s = Scheduler::new(opener);
    let t0 = base();
    s.enqueue("A", 1, t0, &[0x90, 60, 100]);
    s.enqueue("A", 1, t0, &[0x93, 72, 100]);
    s.enqueue("A", 2, t0, &[0x90, 40, 100]);
    s.enqueue("A", 1, ms(t0, 50), &[0x90, 61, 100]);
    s.enqueue("A", 1, ms(t0, 60), &[0x80, 61, 0]);
    s.pump(t0);
    rig.sent();
    s.release_source(1, ms(t0, 10));
    assert_eq!(
        rig.sent(),
        vec![
            ("A".into(), vec![0x80, 60, 0]),
            ("A".into(), vec![0xB0, 123, 0]),
            ("A".into(), vec![0x83, 72, 0]),
            ("A".into(), vec![0xB3, 123, 0]),
        ],
        "a note off per sounding note, then All Notes Off on its channels; source 2 is untouched"
    );
    assert_eq!(s.pump(ms(t0, 100)), None, "its queued messages are gone");
    assert!(rig.sent().is_empty());
    // Releasing again sends nothing, and source 2 still has its note.
    s.release_source(1, ms(t0, 110));
    assert!(rig.sent().is_empty());
    s.release_source(2, ms(t0, 120));
    assert_eq!(
        rig.sent(),
        vec![
            ("A".into(), vec![0x80, 40, 0]),
            ("A".into(), vec![0xB0, 123, 0]),
        ]
    );
}

#[test]
fn a_note_off_or_all_notes_off_from_the_source_clears_what_is_tracked() {
    let (rig, opener) = Rig::new();
    let mut s = Scheduler::new(opener);
    let t0 = base();
    s.enqueue("A", 1, t0, &[0x90, 60, 100]);
    s.enqueue("A", 1, t0, &[0x90, 62, 100]);
    s.enqueue("A", 1, t0, &[0x90, 64, 100]);
    s.enqueue("A", 1, t0, &[0x80, 60, 64]);
    s.enqueue("A", 1, t0, &[0x90, 62, 0]);
    s.pump(t0);
    s.release_source(1, t0);
    assert_eq!(
        rig.sent()[5..].to_vec(),
        vec![
            ("A".into(), vec![0x80, 64, 0]),
            ("A".into(), vec![0xB0, 123, 0]),
        ],
        "only key 64 was left"
    );
    s.enqueue("A", 1, t0, &[0x90, 60, 100]);
    s.enqueue("A", 1, t0, &[0xB0, 123, 0]);
    s.pump(t0);
    rig.sent();
    s.release_source(1, t0);
    assert!(rig.sent().is_empty(), "All Notes Off cleared the channel");
}

#[test]
fn a_port_that_will_not_open_drops_its_messages_and_is_tried_again_later() {
    let (rig, opener) = Rig::new();
    rig.missing.lock().unwrap().push("Gone".into());
    let mut s = Scheduler::new(opener);
    let t0 = base();
    s.enqueue("Gone", 1, t0, &[0x90, 60, 100]);
    s.enqueue("Gone", 1, ms(t0, 500), &[0x80, 60, 0]);
    s.pump(t0);
    s.pump(ms(t0, 500));
    assert_eq!(s.stats().dropped, 2);
    assert_eq!(
        rig.opens.lock().unwrap().len(),
        1,
        "no second attempt inside a second"
    );
    // Plugged in later.
    rig.missing.lock().unwrap().clear();
    s.enqueue("Gone", 1, ms(t0, 1500), &[0x90, 61, 100]);
    s.pump(ms(t0, 1500));
    assert_eq!(rig.sent(), vec![("Gone".into(), vec![0x90, 61, 100])]);
    assert_eq!(s.stats().port_errors, 1);
}

#[test]
fn a_refused_send_closes_the_port_and_it_opens_again() {
    let (rig, opener) = Rig::new();
    let mut s = Scheduler::new(opener);
    let t0 = base();
    s.enqueue("A", 1, t0, &[0xB0, 1, 1]);
    s.pump(t0);
    *rig.fail_next.lock().unwrap() = 1;
    s.enqueue("A", 1, t0, &[0xB0, 1, 2]);
    s.pump(t0);
    assert_eq!(s.stats().port_errors, 1);
    assert_eq!(s.stats().dropped, 1);
    // The back-off is measured from the wall clock; a pump far in the future retries.
    let later = t0 + Duration::from_secs(3600);
    s.enqueue("A", 1, later, &[0xB0, 1, 3]);
    s.pump(later);
    assert_eq!(rig.opens.lock().unwrap().len(), 2, "reopened");
    assert_eq!(rig.sent().last().unwrap().1, vec![0xB0, 1, 3]);
}

#[test]
fn messages_of_the_wrong_length_are_dropped() {
    let (_rig, opener) = Rig::new();
    let mut s = Scheduler::new(opener);
    s.enqueue("A", 1, base(), &[]);
    s.enqueue("A", 1, base(), &[1, 2, 3, 4]);
    assert_eq!(s.stats().dropped, 2);
}

fn core(rig_opener: Opener) -> (Core, Arc<FrameClock>, Arc<AtomicU64>) {
    let clock = Arc::new(FrameClock::new());
    let latency = Arc::new(AtomicU64::new(0));
    let timeline = Timeline::new(Arc::clone(&clock), 48_000.0, Arc::clone(&latency));
    (Core::new(rig_opener, timeline), clock, latency)
}

fn tap_with(events: &[(u64, &[u8])]) -> Arc<MidiTap> {
    let tap = Arc::new(MidiTap::new());
    for (frame, bytes) in events {
        assert!(tap.push(MidiEvent::new(*frame, bytes).unwrap()));
    }
    tap
}

fn ports(list: &[(u32, Option<&str>)]) -> BTreeMap<u32, Option<String>> {
    list.iter()
        .map(|(id, p)| (*id, p.map(str::to_owned)))
        .collect()
}

#[test]
fn the_core_sends_a_taps_messages_at_the_time_of_their_frames() {
    let (rig, opener) = Rig::new();
    let (mut core, clock, latency) = core(opener);
    let t0 = base();
    clock.publish(48_000, t0);
    latency.store(480, Ordering::Relaxed);
    let tap = tap_with(&[(48_000, &[0x90, 60, 100]), (48_000 + 4_800, &[0x80, 60, 0])]);
    core.set_taps(vec![(7, Arc::clone(&tap))], t0);
    core.set_ports(&ports(&[(7, Some("Synth"))]), t0);
    // The first note is heard 10 ms after the callback; the off 100 ms later.
    assert_eq!(core.step(t0), Some(ms(t0, 10)));
    assert!(rig.sent().is_empty());
    assert_eq!(core.step(ms(t0, 10)), Some(ms(t0, 110)));
    assert_eq!(rig.sent(), vec![("Synth".into(), vec![0x90, 60, 100])]);
    assert_eq!(core.step(ms(t0, 111)), None);
    assert_eq!(rig.sent(), vec![("Synth".into(), vec![0x80, 60, 0])]);
    assert!(!core.has_backlog());
}

#[test]
fn before_the_clock_publishes_messages_go_out_at_once() {
    let (rig, opener) = Rig::new();
    let (mut core, _clock, _) = core(opener);
    let t0 = base();
    core.set_taps(vec![(1, tap_with(&[(99_999, &[0xB0, 1, 1])]))], t0);
    core.set_ports(&ports(&[(1, Some("A"))]), t0);
    core.step(t0);
    assert_eq!(rig.sent().len(), 1);
}

#[test]
fn a_source_without_a_port_drops_its_messages() {
    let (rig, opener) = Rig::new();
    let (mut core, _clock, _) = core(opener);
    let t0 = base();
    core.set_taps(vec![(1, tap_with(&[(0, &[0xB0, 1, 1])]))], t0);
    core.step(t0);
    assert!(rig.sent().is_empty());
    assert_eq!(core.scheduler().stats().dropped, 1);
    assert!(rig.opens.lock().unwrap().is_empty());
}

#[test]
fn changing_a_sources_port_silences_it_on_the_old_one() {
    let (rig, opener) = Rig::new();
    let (mut core, _clock, _) = core(opener);
    let t0 = base();
    let tap = tap_with(&[(0, &[0x90, 60, 100])]);
    core.set_taps(vec![(1, Arc::clone(&tap))], t0);
    core.set_ports(&ports(&[(1, Some("A"))]), t0);
    core.step(t0);
    rig.sent();
    // The same tap, the same port: nothing happens.
    core.set_taps(vec![(1, Arc::clone(&tap))], t0);
    core.set_ports(&ports(&[(1, Some("A"))]), t0);
    assert!(rig.sent().is_empty());
    // Another port: the note on the first one is released there.
    core.set_ports(&ports(&[(1, Some("B"))]), t0);
    assert_eq!(
        rig.sent(),
        vec![
            ("A".into(), vec![0x80, 60, 0]),
            ("A".into(), vec![0xB0, 123, 0]),
        ]
    );
    tap.push(MidiEvent::new(0, &[0x90, 62, 100]).unwrap());
    core.step(t0);
    assert_eq!(rig.sent(), vec![("B".into(), vec![0x90, 62, 100])]);
}

#[test]
fn a_removed_tap_silences_its_notes_and_a_replaced_one_does_too() {
    let (rig, opener) = Rig::new();
    let (mut core, _clock, _) = core(opener);
    let t0 = base();
    let tap = tap_with(&[(0, &[0x90, 60, 100]), (0, &[0x91, 61, 100])]);
    core.set_taps(vec![(1, Arc::clone(&tap))], t0);
    core.set_ports(&ports(&[(1, Some("A"))]), t0);
    core.step(t0);
    rig.sent();
    core.set_taps(Vec::new(), ms(t0, 5));
    let released: Vec<_> = rig.sent().into_iter().map(|(_, b)| b).collect();
    assert_eq!(
        released,
        vec![
            vec![0x80, 60, 0],
            vec![0xB0, 123, 0],
            vec![0x81, 61, 0],
            vec![0xB1, 123, 0]
        ]
    );
    // The id comes back with another tap (a rebuilt engine): it starts clean.
    let other = tap_with(&[(0, &[0x90, 70, 100])]);
    core.set_taps(vec![(1, Arc::clone(&other))], ms(t0, 6));
    core.set_ports(&ports(&[(1, Some("A"))]), ms(t0, 6));
    core.step(ms(t0, 6));
    core.set_taps(vec![(1, tap_with(&[]))], ms(t0, 7));
    let released: Vec<_> = rig.sent().into_iter().map(|(_, b)| b).collect();
    assert_eq!(
        released,
        vec![vec![0x90, 70, 100], vec![0x80, 70, 0], vec![0xB0, 123, 0]],
        "a different tap under the same id is a different device"
    );
}

#[test]
fn a_lost_note_off_silences_the_source() {
    let (rig, opener) = Rig::new();
    let (mut core, _clock, _) = core(opener);
    let t0 = base();
    let tap = Arc::new(MidiTap::new());
    core.set_taps(vec![(1, Arc::clone(&tap))], t0);
    core.set_ports(&ports(&[(1, Some("A"))]), t0);
    tap.push(MidiEvent::new(0, &[0x90, 60, 100]).unwrap());
    core.step(t0);
    rig.sent();
    // Fill the tap with controllers and then lose a note off.
    let mut i = 0;
    while tap.push(MidiEvent::new(0, &[0xB0, 1, 1]).unwrap()) {
        i += 1;
    }
    assert!(i > 0);
    for _ in 0..atomik_device_sdk::MIDI_TAP_EVENTS {
        if !tap.push(MidiEvent::new(0, &[0x80, 61, 0]).unwrap()) {
            break;
        }
    }
    assert!(!tap.push(MidiEvent::new(0, &[0x80, 60, 0]).unwrap()));
    core.step(ms(t0, 1));
    let sent: Vec<_> = rig.sent().into_iter().map(|(_, b)| b).collect();
    assert!(
        sent.contains(&vec![0x80, 60, 0]) && sent.contains(&vec![0xB0, 123, 0]),
        "the sounding note was released"
    );
}

#[test]
fn shutdown_releases_everything_and_closes_the_ports() {
    let (rig, opener) = Rig::new();
    let (mut core, _clock, _) = core(opener);
    let t0 = base();
    core.set_taps(vec![(1, tap_with(&[(0, &[0x92, 60, 100])]))], t0);
    core.set_ports(&ports(&[(1, Some("A"))]), t0);
    core.step(t0);
    rig.sent();
    core.shutdown(t0);
    let released: Vec<_> = rig.sent().into_iter().map(|(_, b)| b).collect();
    assert_eq!(released, vec![vec![0x82, 60, 0], vec![0xB2, 123, 0]]);
}

/// Waits until `done` holds, up to `limit`; real threads on a loaded machine can be late, so the
/// tests below only bound the lateness generously. Exact timing is checked in the `Core::step`
/// tests with an injected clock.
fn wait_for(limit: Duration, mut done: impl FnMut() -> bool) -> bool {
    let end = Instant::now() + limit;
    while Instant::now() < end {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    done()
}

#[test]
fn the_thread_sends_in_order_and_releases_on_drop() {
    let (rig, opener) = Rig::new();
    let clock = Arc::new(FrameClock::new());
    let latency = Arc::new(AtomicU64::new(0));
    let timeline = Timeline::new(Arc::clone(&clock), 48_000.0, latency);
    let thread = OutputThread::spawn(opener, timeline).unwrap();
    let t0 = now();
    clock.publish(0, t0);
    // 50 ms and 60 ms from now.
    let tap = tap_with(&[(2_400, &[0x90, 60, 100]), (2_880, &[0x90, 62, 100])]);
    thread.set_routes(vec![(1, tap)], ports(&[(1, Some("A"))]));
    let mut sent = Vec::new();
    let arrived = wait_for(Duration::from_secs(5), || {
        sent.extend(rig.sent());
        sent.len() >= 2
    });
    let elapsed = t0.elapsed();
    assert!(arrived, "both notes arrive within seconds, got {sent:?}");
    // Never early: the first is due 50 ms after the anchor (a little slack for the clock read).
    assert!(elapsed >= Duration::from_millis(40), "sent early: {elapsed:?}");
    // Generously late at worst, even on a loaded runner.
    assert!(elapsed < Duration::from_secs(2), "sent far too late: {elapsed:?}");
    let order: Vec<_> = sent.iter().map(|(_, b)| b.clone()).collect();
    assert_eq!(order, vec![vec![0x90, 60, 100], vec![0x90, 62, 100]]);
    assert!(wait_for(Duration::from_secs(5), || thread.stats().sent >= 2));
    drop(thread);
    let after: Vec<_> = rig.sent().into_iter().map(|(_, b)| b).collect();
    assert_eq!(
        after,
        vec![vec![0x80, 60, 0], vec![0x80, 62, 0], vec![0xB0, 123, 0]],
        "dropping the thread releases the sounding notes"
    );
}
