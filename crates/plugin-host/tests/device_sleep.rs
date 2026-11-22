//! The sleep rule (one rule for both formats) and the bypass rule of the plugin device: a plugin
//! that has nothing to do is not called, and a skipped call gives exactly what the call would
//! have (silence), which the tests prove by rendering the same input through the device and
//! through the processor called in every block.
// Justification: copies of the same f32 samples are compared bit for bit.
#![allow(clippy::float_cmp)]

atomik_testkit::install_alloc_guard!();

mod support;

use atomik_core::{BATCH, NoteEvent, NoteKind};
use atomik_plugin_host::{
    EventList, PluginIo, PluginProcessor, PluginTransport, ProcessStatus, SleepRule, TailLength,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use support::*;

/// Blocks in one second at 48 kHz (the idle time of the rule).
const IDLE: usize = 750;

/// What a [`Ringer`] answers and reports.
#[derive(Clone, Copy)]
struct Behaviour {
    rule: SleepRule,
    tail: TailLength,
    /// Frames it is silent after its last non-silent input before it rings.
    delay: u32,
    /// Frames it keeps ringing after its last non-silent input.
    ring: u32,
    ringing: ProcessStatus,
    quiet: ProcessStatus,
}

/// A stereo effect that copies its input and adds 0.25 to it while it rings: for `ring` frames
/// after the last block with a non-silent input. Counts its calls.
struct Ringer {
    b: Behaviour,
    left: u32,
    calls: Arc<AtomicUsize>,
    request: Arc<AtomicBool>,
}

impl Ringer {
    fn new(b: Behaviour) -> (Self, Arc<AtomicUsize>, Arc<AtomicBool>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let request = Arc::new(AtomicBool::new(false));
        (
            Self {
                b,
                left: 0,
                calls: Arc::clone(&calls),
                request: Arc::clone(&request),
            },
            calls,
            request,
        )
    }
}

impl PluginProcessor for Ringer {
    fn start_processing(&mut self) -> bool {
        true
    }
    fn stop_processing(&mut self) {}
    fn reset(&mut self) {
        self.left = 0;
    }
    fn process(&mut self, io: &mut PluginIo<'_>) -> ProcessStatus {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if io
            .main_in
            .iter()
            .any(|b| b.iter().any(|s| s.to_bits() & 0x7FFF_FFFF != 0))
        {
            self.left = self.b.delay + self.b.ring;
        }
        let ringing = self.left > 0;
        let add = if ringing && self.left <= self.b.ring {
            0.25
        } else {
            0.0
        };
        for (ch, out) in io.main_out.iter_mut().enumerate() {
            for (o, x) in out.iter_mut().zip(&io.main_in[ch]) {
                *o = x + add;
            }
        }
        self.left = self.left.saturating_sub(BATCH as u32);
        if ringing {
            self.b.ringing
        } else {
            self.b.quiet
        }
    }
    fn sleep_rule(&self) -> SleepRule {
        self.b.rule
    }
    fn tail_samples(&self) -> TailLength {
        self.b.tail
    }
    fn wake_requested(&self) -> bool {
        self.request.swap(false, Ordering::Relaxed)
    }
}

fn zero_tail(tail: TailLength, ring: u32) -> Behaviour {
    Behaviour {
        rule: SleepRule::ZeroTail,
        tail,
        ring,
        delay: 0,
        ringing: ProcessStatus::Continue,
        quiet: ProcessStatus::Continue,
    }
}

fn status_rule(
    tail: TailLength,
    ring: u32,
    ringing: ProcessStatus,
    quiet: ProcessStatus,
) -> Behaviour {
    Behaviour {
        rule: SleepRule::PluginDecides,
        tail,
        ring,
        delay: 0,
        ringing,
        quiet,
    }
}

/// The input of block `b`: 0.5 in the first three blocks, silence after.
fn burst(b: usize) -> f32 {
    if b < 3 { 0.5 } else { 0.0 }
}

/// The left output of the processor called in every block over `blocks` blocks of `input`.
fn reference(b: Behaviour, blocks: usize, input: impl Fn(usize) -> f32) -> Vec<f32> {
    let (mut p, _, _) = Ringer::new(b);
    let mut events = EventList::with_capacity(8);
    let mut out = Vec::new();
    for n in 0..blocks {
        let x = [[input(n); BATCH]; 2];
        let mut y = [[0.0; BATCH]; 2];
        let mut io = PluginIo {
            frames: BATCH,
            main_in: &x,
            main_in_silent: false,
            sidechain: &[],
            sidechain_silent: true,
            main_out: &mut y,
            aux_out: &mut [],
            events_in: &[],
            events_out: &mut events,
            transport: PluginTransport::default(),
            dropped: 0,
        };
        p.process(&mut io);
        out.extend_from_slice(&y[0]);
    }
    out
}

/// Renders `blocks` blocks of `input` through a device over a [`Ringer`]; returns the left
/// output and the number of calls the plugin got.
fn render(b: Behaviour, blocks: usize, input: impl Fn(usize) -> f32) -> (Vec<f32>, usize) {
    let (ringer, calls, _) = Ringer::new(b);
    let (mut h, _r) = harness(rig("sl.ringer", &fx_layout(), Box::new(ringer)));
    let mut out = Vec::new();
    for n in 0..blocks {
        *h.input_mut("in_l") = [input(n); BATCH];
        *h.input_mut("in_r") = [input(n); BATCH];
        h.process();
        out.extend_from_slice(h.output("out_l"));
    }
    (out, calls.load(Ordering::Relaxed))
}

// ----- B: the sleep rule ------------------------------------------------------------------------

#[test]
fn an_effect_with_a_two_second_tail_is_called_through_its_tail_and_then_sleeps() {
    let b = zero_tail(TailLength::Finite(2 * 48_000), 2 * 48_000);
    let blocks = 3_000;
    let (out, calls) = render(b, blocks, burst);
    assert_eq!(
        out,
        reference(b, blocks, burst),
        "a skipped call is what the plugin would have made"
    );
    // Three blocks of signal, 1500 blocks of ring, then the idle second: then no more calls.
    assert_eq!(calls, 3 + 1_500 - 1 + IDLE);
    assert!(out[..1_400 * BATCH].iter().any(|s| *s != 0.0));
}

#[test]
fn an_effect_that_reports_no_tail_but_rings_for_a_second_is_not_cut() {
    let b = zero_tail(TailLength::Finite(0), 48_000);
    let blocks = 2_400;
    let (out, calls) = render(b, blocks, burst);
    assert_eq!(out, reference(b, blocks, burst), "the ring is not cut");
    let ringing = out.iter().filter(|s| **s != 0.0).count();
    assert!(
        ringing > 40_000,
        "the ring is heard in full ({ringing} frames)"
    );
    // Its own output keeps it called through the ring, and it sleeps one idle second after it.
    assert_eq!(calls, 3 + IDLE - 1 + IDLE);
}

#[test]
fn a_vst3_style_plugin_with_an_infinite_tail_is_never_skipped_so_a_late_echo_is_not_lost() {
    // Silent for two seconds after its input, then it sounds on its own: the declaration of an
    // infinite tail is the plugin saying it may. The idle second passes in the silence.
    let mut b = zero_tail(TailLength::Infinite, 48_000);
    b.delay = 2 * 48_000;
    let blocks = 4_000;
    let (out, calls) = render(b, blocks, burst);
    assert_eq!(out, reference(b, blocks, burst), "nothing is lost");
    assert_eq!(calls, blocks, "called in every block");
    let echo = out[3 * BATCH..].iter().filter(|s| **s != 0.0).count();
    assert_eq!(echo, 48_000, "the echo is heard in full");
    // Silent for the idle second before the echo: the output rule alone would have rested it.
    let quiet = out[(3 + IDLE) * BATCH..1_500 * BATCH]
        .iter()
        .all(|s| *s == 0.0);
    assert!(quiet);
}

#[test]
fn a_clap_plugin_that_answers_continue_if_not_quiet_rests_whatever_its_tail_extension_says() {
    let b = status_rule(
        TailLength::Infinite,
        48_000,
        ProcessStatus::Tail,
        ProcessStatus::ContinueIfNotQuiet,
    );
    let blocks = 2_400;
    let (out, calls) = render(b, blocks, burst);
    assert_eq!(out, reference(b, blocks, burst));
    assert_eq!(calls, 3 + IDLE - 1 + IDLE);
}

#[test]
fn a_plugin_that_never_goes_quiet_never_sleeps() {
    // A constant offset (or noise, or dither) on a silent input.
    let (probe, shared) = Probe::new();
    shared.set_rule(SleepRule::ZeroTail, TailLength::Finite(0));
    shared.set_out(ProbeOut::Constant(0.25));
    let (mut h, _r) = harness(rig("sl.offset", &fx_layout(), Box::new(probe)));
    for _ in 0..3 * IDLE {
        h.process();
    }
    assert_eq!(shared.calls(), 3 * IDLE);
}

#[test]
fn the_status_tail_keeps_a_plugin_called_until_it_stops_answering_it() {
    // Answering "tail" while quiet too: never sleeps, whatever the output.
    let b = status_rule(
        TailLength::Finite(0),
        0,
        ProcessStatus::Tail,
        ProcessStatus::Tail,
    );
    let (_, calls) = render(b, 3 * IDLE, burst);
    assert_eq!(calls, 3 * IDLE, "it is still answering tail");
    // Tail while it rings, "continue if not quiet" after: it sleeps once the output has been
    // silent for the idle time.
    let b = status_rule(
        TailLength::Finite(0),
        48_000,
        ProcessStatus::Tail,
        ProcessStatus::ContinueIfNotQuiet,
    );
    let blocks = 2_400;
    let (out, calls) = render(b, blocks, burst);
    assert_eq!(out, reference(b, blocks, burst));
    assert_eq!(calls, 3 + IDLE - 1 + IDLE);
}

#[test]
fn the_status_continue_if_not_quiet_lets_a_plugin_rest_when_the_input_is_quiet_and_honours_the_tail()
 {
    let quiet = ProcessStatus::ContinueIfNotQuiet;
    // With no tail: asleep after the idle second of silent output.
    let b = status_rule(TailLength::Finite(0), 0, quiet, quiet);
    let (out, calls) = render(b, 2 * IDLE, burst);
    assert_eq!(out, reference(b, 2 * IDLE, burst));
    assert_eq!(calls, 3 + IDLE);
    // With a two second tail: the input must have been silent for all of it.
    let b = status_rule(TailLength::Finite(2 * 48_000), 0, quiet, quiet);
    let (_, calls) = render(b, 3_000, burst);
    assert_eq!(calls, 3 + 1_500);
}

#[test]
fn the_status_continue_never_sleeps_and_the_status_sleep_rests_at_once() {
    let b = status_rule(
        TailLength::Finite(0),
        0,
        ProcessStatus::Continue,
        ProcessStatus::Continue,
    );
    let (_, calls) = render(b, 3 * IDLE, burst);
    assert_eq!(calls, 3 * IDLE);
    let b = status_rule(
        TailLength::Finite(0),
        48_000,
        ProcessStatus::Continue,
        ProcessStatus::Sleep,
    );
    let blocks = 2_400;
    let (out, calls) = render(b, blocks, burst);
    assert_eq!(out, reference(b, blocks, burst));
    // Three blocks of signal, the ring, and the first quiet block that answers sleep.
    assert_eq!(calls, 3 + IDLE - 1 + 1);
}

/// A harness over a sleeping [`Ringer`] (no tail, rings a second) and its call counter.
fn asleep() -> (
    atomik_testkit::harness::DeviceHarness,
    Arc<AtomicUsize>,
    Arc<AtomicBool>,
    Rest,
) {
    let b = zero_tail(TailLength::Finite(0), 48_000);
    let (ringer, calls, request) = Ringer::new(b);
    let (mut h, r) = harness(rig("sl.wake", &fx_layout(), Box::new(ringer)));
    *h.input_mut("in_l") = [0.5; BATCH];
    h.process();
    *h.input_mut("in_l") = [0.0; BATCH];
    for _ in 0..2 * IDLE + 10 {
        h.process();
    }
    let now = calls.load(Ordering::Relaxed);
    h.process();
    assert_eq!(calls.load(Ordering::Relaxed), now, "asleep");
    let mut m = [0.0f32; 4];
    h.device().meter_values(&mut m);
    assert_eq!(m[2], 1.0, "SLEEPING");
    (h, calls, request, r)
}

#[test]
fn a_sleeping_plugin_wakes_on_input_a_param_change_a_request_and_a_transport_change() {
    let (mut h, calls, request, _r) = asleep();
    let n = || calls.load(Ordering::Relaxed);
    let before = n();
    h.input_mut("in_r")[3] = 0.1;
    h.process();
    assert_eq!(n(), before + 1, "input");
    *h.input_mut("in_r") = [0.0; BATCH];
    // The plugin rings after that: wait for it to sleep again.
    for _ in 0..2 * IDLE + 10 {
        h.process();
    }
    let before = n();
    h.process();
    assert_eq!(n(), before, "asleep again");
    h.set_param("p2", 0.5);
    h.process();
    assert_eq!(n(), before + 1, "a param change");
    for _ in 0..IDLE + 10 {
        h.process();
    }
    let before = n();
    h.process();
    assert_eq!(n(), before, "asleep again");
    request.store(true, Ordering::Relaxed);
    h.process();
    assert_eq!(n(), before + 1, "a request of the plugin");
    for _ in 0..IDLE + 10 {
        h.process();
    }
    let before = n();
    h.process();
    assert_eq!(n(), before, "asleep again");
    let playing = h.time_mut().playing;
    h.time_mut().playing = !playing;
    h.process();
    assert_eq!(n(), before + 1, "a change of the play state");
}

#[test]
fn a_zero_tail_instrument_with_a_long_tail_sleeps_after_the_tail() {
    let (probe, shared) = Probe::new();
    shared.set_rule(SleepRule::ZeroTail, TailLength::Finite(2 * 48_000));
    shared.set_out(ProbeOut::Silent);
    let (mut h, _r) = harness(rig("sl.inst-tail", &synth_layout(), Box::new(probe)));
    let on = NoteEvent {
        offset: 0,
        kind: NoteKind::On {
            key: 60,
            velocity: 1.0,
        },
    };
    let off = NoteEvent {
        offset: 0,
        kind: NoteKind::Off { key: 60 },
    };
    h.push_note("notes_in", on);
    h.process();
    h.push_note("notes_in", off);
    h.process();
    let after_off = shared.calls();
    for _ in 0..1_499 {
        h.process();
    }
    assert_eq!(shared.calls(), after_off + 1_499, "called through the tail");
    for _ in 0..10 {
        h.process();
    }
    assert!(shared.calls() <= after_off + 1_500, "then asleep");
    assert_eq!(shared.log.lock().unwrap().stops, 1, "told to stop");
}
