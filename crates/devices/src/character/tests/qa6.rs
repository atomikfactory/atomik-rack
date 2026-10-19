//! Tests of the device shell: each one answers a mutant that survived the first mutation
//! pass or pins a design statement that had no test of
//! its own (the crossfade law, the resets, the meter order, the presence detector's channels).

use super::*;
use std::cell::RefCell;

// --- output safety ----------------------------------------------------------------------------

#[test]
fn the_safety_leaves_the_output_untouched_up_to_four_and_bounds_it_beyond() {
    for y in [0.0f32, 0.5, 1.0, 2.0, 3.5, 3.99, 4.0, -3.5, -4.0] {
        assert_eq!(safety(y).to_bits(), y.to_bits(), "{y} is untouched");
    }
    let mut prev = 4.0f32;
    for y in [4.01f32, 4.5, 5.0, 6.0, 10.0, 100.0, 1.0e9, f32::MAX] {
        let s = safety(y);
        assert!(s >= prev && s <= 8.0, "{y} -> {s}");
        assert!(
            y > 100.0 || s > prev,
            "strictly rising through the knee: {y}"
        );
        assert_eq!(safety(-y), -s, "odd");
        prev = s;
    }
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        assert_eq!(safety(bad), 0.0);
    }
}

// --- the preset crossfade law -----------------------------------------------------------------

fn smoothstep(w: f64) -> f64 {
    w * w * (3.0 - 2.0 * w)
}

#[test]
fn the_preset_crossfade_is_a_smoothstep_over_exactly_the_fade_length() {
    // Test chain 1 passes, chain 0 inverts: a constant input x crosses over as x (1 - 2 s(w)).
    let mut h = dev_with_test_chains();
    set_preset(&mut h, 1);
    let x = 0.25f32;
    let warm = vec![x; BATCH * 60];
    let _ = run(&mut h, &warm, None);
    set_preset(&mut h, 0);
    let fade = (0.030 * SR / SUB as f64).ceil() as usize * SUB; // whole 16-frame blocks
    assert_eq!(fade, 1440);
    let (l, _) = run(&mut h, &vec![x; BATCH * 40], None);
    // The incoming chain starts from reset, so its pad is silent for its own latency first.
    let skip = short_latency(SR as f32) as usize + 8;
    for k in (skip..fade).step_by(37) {
        let w = (k + 1) as f64 / fade as f64;
        let want = f64::from(x) * (1.0 - 2.0 * smoothstep(w));
        assert!(
            (f64::from(l[k]) - want).abs() < 2e-5,
            "k {k}: {} against {want}",
            l[k]
        );
    }
    for k in [fade, fade + 1, fade + 500, l.len() - 1] {
        assert!(
            (l[k] + x).abs() < 1e-6,
            "settled on the incoming chain at {k}"
        );
    }
}

#[test]
fn a_class_crossing_moves_the_dry_delay_with_the_same_smoothstep() {
    // Mix 0 plays the dry tap only; a ramp input shows the delay: out[n] = 1e-5 (n - delay).
    let mut h = dev_with_test_chains();
    h.set_param("mix", 0.0);
    set_preset(&mut h, 1);
    let ramp = |from: usize, n: usize| -> Vec<f32> {
        (from..from + n).map(|i| 1.0e-5 * i as f32).collect()
    };
    let n0 = BATCH * 40;
    let _ = run(&mut h, &ramp(0, n0), None);
    set_preset(&mut h, 8); // the long test chain: EXTRA frames above the short class
    let (l, _) = run(&mut h, &ramp(n0, BATCH * 30), None);
    let la = short_latency(SR as f32) as f64;
    let fade = 1440usize;
    for j in (0..fade).step_by(61) {
        let w = (j + 1) as f64 / fade as f64;
        let delay = la + f64::from(EXTRA) * smoothstep(w);
        let want = 1.0e-5 * ((n0 + j) as f64 - delay);
        assert!(
            (f64::from(l[j]) - want).abs() < 1.0e-6,
            "j {j}: {} against {want}",
            l[j]
        );
    }
    let last = l.len() - 1;
    let want = 1.0e-5 * ((n0 + last) as f64 - la - f64::from(EXTRA));
    assert!(
        (f64::from(l[last]) - want).abs() < 1.0e-6,
        "settled on the long delay"
    );
}

// --- resets at the two ends of a fade ---------------------------------------------------------

thread_local! {
    static RESETS: RefCell<[u32; 16]> = const { RefCell::new([0; 16]) };
}

/// A test chain that counts its resets per preset index (the device runs on the test thread).
struct Counting {
    idx: usize,
    inner: TestChain,
}

impl PresetChain for Counting {
    fn spec(&self) -> ChainSpec {
        self.inner.spec()
    }
    fn prepare(&mut self, sr: f32, seed: u64, v: &PresetVoicing) {
        self.inner.prepare(sr, seed, v);
    }
    fn reset(&mut self) {
        RESETS.with(|r| r.borrow_mut()[self.idx] += 1);
        self.inner.reset();
    }
    fn process(&mut self, l: &mut [f32; SUB], r: &mut [f32; SUB], m: &Morph, c: &Ctx) {
        self.inner.process(l, r, m, c);
    }
    fn latency(&self) -> u32 {
        self.inner.latency()
    }
    fn routing(&self) -> Routing {
        self.inner.routing()
    }
    fn activity(&self) -> f32 {
        self.inner.activity()
    }
    fn presence(&self) -> (f32, f32) {
        self.inner.presence()
    }
}

fn counting_chains(idx: usize) -> Box<dyn PresetChain> {
    Box::new(Counting {
        idx,
        inner: TestChain::default(),
    })
}

fn resets(i: usize) -> u32 {
    RESETS.with(|r| r.borrow()[i])
}

#[test]
fn a_preset_change_resets_the_incoming_chain_at_the_fade_start_and_the_outgoing_one_at_its_end() {
    let mut h = DeviceHarness::with_options(
        Box::new(Character::with_chains(counting_chains)),
        SR,
        120.0,
        11,
    );
    h.process();
    RESETS.with(|r| *r.borrow_mut() = [0; 16]);
    set_preset(&mut h, 3);
    h.process(); // the batch that starts the fade
    assert_eq!(resets(3), 1, "the incoming chain starts from reset");
    assert_eq!(resets(0), 0, "the outgoing chain plays on during the fade");
    for _ in 0..30 {
        h.process();
    }
    assert_eq!(
        resets(0),
        1,
        "the outgoing chain is reset when the fade ends"
    );
    assert_eq!(resets(3), 1, "and the incoming chain is not reset again");
    for _ in 0..10 {
        h.process();
    }
    let total: u32 = RESETS.with(|r| r.borrow().iter().sum());
    assert_eq!(total, 2, "nothing else is reset while the preset stands");
}

// --- the preset parameter ---------------------------------------------------------------------

#[test]
fn the_preset_parameter_rounds_to_the_nearest_choice() {
    for (v, long) in [(7.4f32, false), (7.6, true), (8.4, true), (8.6, false)] {
        // Straight into the device: the harness may round a discrete value itself.
        let mut c = Character::new();
        c.set_param(atomik_core::ParamId(P_PRESET), v);
        let mut h = DeviceHarness::with_options(Box::new(c), SR, 120.0, 11);
        h.process();
        let want = if long {
            long_latency(SR as f32)
        } else {
            short_latency(SR as f32)
        };
        assert_eq!(h.device().latency_frames(), want, "preset {v}");
    }
}

// --- meters -----------------------------------------------------------------------------------

#[test]
fn the_meters_are_in_order_and_the_input_peak_is_the_larger_channel() {
    let mut h = dev();
    set_preset(&mut h, 2);
    h.set_param("mix", 0.0); // the dry tap only: the output is the input times Volume
    h.set_param("gain", 1.0); // x4 on the wet path
    h.set_param("volume", 0.25); // x0.25
    h.set_param("transform", 0.8);
    let quiet_left = sine(BATCH * 40, 1000.0, 0.1, SR);
    let loud_right = sine(BATCH * 40, 1000.0, 0.2, SR);
    let _ = run(&mut h, &quiet_left, Some(&loud_right));
    let m = meters(&h);
    assert!(
        (m[0] - 0.8).abs() < 0.02,
        "in peak after the gain, larger channel: {}",
        m[0]
    );
    assert!(
        (m[1] - 0.05).abs() < 0.005,
        "out peak after the volume: {}",
        m[1]
    );
    assert!((m[2] - 0.8).abs() < 1e-5, "effective transform: {}", m[2]);
    assert!((0.0..=1.0).contains(&m[3]));
}

// --- the presence detector sees both channels, and holds ---------------------------------------

#[test]
fn a_programme_on_the_right_input_alone_opens_the_noise_gate() {
    let mut h = dev();
    set_preset(&mut h, 0); // reel: hiss in the wet path
    h.set_param("transform", 1.0);
    let silent = vec![0.0f32; BATCH * 750];
    let prog = noise(BATCH * 750, 77, 0.3);
    let mut left = Vec::new();
    for _ in 0..3 {
        left = run(&mut h, &silent, Some(&prog)).0;
    }
    // The left input is silent and the chain adds nothing to silence but the gated hiss.
    assert!(
        db(f64::from(peak(&left))) > -80.0,
        "the hiss is open: {} dBFS",
        db(f64::from(peak(&left)))
    );
}

#[test]
fn the_noise_stays_fully_open_for_the_whole_hold_time() {
    let mut h = dev();
    set_preset(&mut h, 0);
    h.set_param("transform", 1.0);
    let prog = noise((SR * 4.0) as usize / BATCH * BATCH, 78, 0.3);
    let _ = run(&mut h, &prog, None);
    let after = vec![0.0f32; (SR * 7.0) as usize / BATCH * BATCH];
    let (l, _) = run(&mut h, &after, None);
    let win = |from: f64, to: f64| &l[(from * SR) as usize..(to * SR) as usize];
    let early = db(rms(win(0.3, 0.8)));
    let late = db(rms(win(2.0, 2.5))); // the table holds 3 s after the detector lets go
    assert!(early > -90.0);
    assert!(
        (early - late).abs() < 0.5,
        "open: {early} dB then {late} dB"
    );
    let gone = db(f64::from(peak(win(6.2, 6.7))));
    assert!(gone < -80.0, "closed after the fade: {gone} dBFS");
}
