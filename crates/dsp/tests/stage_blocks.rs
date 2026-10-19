//! Cross-cutting checks of the stage blocks: `process` never allocates, and a
//! fixed render of every block hashes to a pinned value, so a platform or compiler difference in
//! any per-sample path shows up as a failing pin on CI. The crossover is not pinned:
//! its coefficients come from the libm-based biquad design (as in every other filter).

atomik_testkit::install_alloc_guard!();

use atomik_dsp::{
    Bitcrush, Chorus, Comb, Crackle, Crossover, Echo, Leveller, Modal, Phaser, PinkNoise,
    PresenceGate, RingMod, Rng, Room, RoomRange, Rumble, Shaper, ShaperKind, Smoother2,
    SmootherClass, Spectral, SpectralAlgo, WhiteNoise, Wobble,
};
use atomik_testkit::no_alloc::assert_no_alloc;

const SR: f32 = 48_000.0;
const N: usize = 4_096;

fn input(seed: u64) -> Vec<f32> {
    let mut r = Rng::new(seed);
    (0..N)
        .map(|i| {
            let tone = ((i % 61) as f32 / 30.5 - 1.0) * 0.3; // a saw: no libm in the input
            tone + r.next_bipolar() * 0.2
        })
        .collect()
}

fn fnv(h: &mut u64, x: &[f32]) {
    for v in x {
        for b in v.to_bits().to_le_bytes() {
            *h ^= u64::from(b);
            *h = h.wrapping_mul(0x0000_0100_0000_01B3);
        }
    }
}

const FNV_START: u64 = 0xcbf2_9ce4_8422_2325;

/// Every block, configured, rendered over `input` into a hash; with `guard` the render runs
/// inside the allocation guard.
fn render_all(guard: bool) -> Vec<(&'static str, u64)> {
    let l0 = input(1);
    let r0 = input(2);
    let mut out = Vec::new();
    let mut run = |name: &'static str, f: &mut dyn FnMut(&mut Vec<f32>, &mut Vec<f32>)| {
        let (mut l, mut r) = (l0.clone(), r0.clone());
        if guard {
            assert_no_alloc(|| f(&mut l, &mut r));
        } else {
            f(&mut l, &mut r);
        }
        let mut h = FNV_START;
        fnv(&mut h, &l);
        fnv(&mut h, &r);
        out.push((name, h));
    };

    for (name, kind) in [
        ("shaper_soft", ShaperKind::SoftClip),
        ("shaper_asym", ShaperKind::Asymmetric),
        ("shaper_hard", ShaperKind::Hard),
        ("shaper_fold", ShaperKind::Fold),
        ("shaper_dynamic", ShaperKind::Dynamic),
    ] {
        let mut s = Shaper::new(SR);
        s.set_kind(kind);
        s.set_drive_db(18.0);
        run(name, &mut |l, r| {
            s.process_block(l);
            s.process_block(r);
        });
    }
    let mut lev = Leveller::new(SR);
    lev.set_ratio(0.7);
    run("leveller", &mut |l, r| lev.process_block(l, r));
    let mut xo = Crossover::new(SR);
    xo.set_freq(800.0);
    run("crossover", &mut |l, _| {
        for v in l.iter_mut() {
            let (a, b) = xo.process(*v);
            *v = a - b;
        }
    });
    let mut ph = Phaser::new(SR);
    ph.set_rate_hz(1.3);
    ph.set_feedback(0.6);
    ph.set_stereo(0.8);
    run("phaser", &mut |l, r| ph.process_block(l, r));
    let mut ch = Chorus::new(SR, 40.0);
    ch.set_feedback(0.4);
    run("chorus", &mut |l, r| ch.process_block(l, r));
    let mut ec = Echo::new(SR, 500.0);
    ec.set_time_ms(37.0);
    ec.set_modulation(2.0, 0.3);
    run("echo", &mut |l, r| ec.process_block(l, r));
    let mut cb = Comb::new(SR, 20.0);
    cb.set_delay_ms(3.3);
    cb.set_feedback(0.8);
    run("comb", &mut |l, r| {
        cb.process_block(l);
        cb.process_block(r);
    });
    for (name, range) in [
        ("room_small", RoomRange::Small),
        ("room_large", RoomRange::Large),
    ] {
        let mut rm = Room::new(SR, 5, range);
        rm.set_damping(0.4);
        run(name, &mut |l, r| rm.process_block(l, r));
    }
    let mut rg = RingMod::new(SR);
    rg.set_freq_hz(317.0);
    rg.set_amount(0.7);
    rg.set_stereo_phase(0.2);
    run("ringmod", &mut |l, r| rg.process_block(l, r));
    let mut bc = Bitcrush::new(SR);
    bc.set_bits(5.0);
    bc.set_rate_hz(7_300.0);
    bc.reset();
    run("bitcrush", &mut |l, r| {
        bc.process_block(l);
        bc.process_block(r);
    });
    let (mut w, mut p, mut ru, mut cr, mut gate) = (
        WhiteNoise::new(1),
        PinkNoise::new(2),
        Rumble::new(3, SR),
        Crackle::new(4, SR),
        PresenceGate::new(SR),
    );
    cr.set_rate_hz(300.0);
    run("noise", &mut |l, r| {
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            let g = gate.process(*a);
            *a = (w.next_sample() + p.next_sample() + cr.next_sample()) * g;
            *b = ru.next_sample();
        }
    });
    for (name, algo) in [
        ("spectral_identity", SpectralAlgo::Identity),
        (
            "spectral_gate",
            SpectralAlgo::Gate {
                threshold_db: -30.0,
                hold_ms: 40.0,
            },
        ),
        ("spectral_slew", SpectralAlgo::Slew { rise_db: 4.0 }),
        (
            "spectral_pass",
            SpectralAlgo::BandPass {
                lo_hz: 400.0,
                hi_hz: 6_000.0,
            },
        ),
        (
            "spectral_stop",
            SpectralAlgo::BandStop {
                lo_hz: 400.0,
                hi_hz: 6_000.0,
            },
        ),
        ("spectral_quant", SpectralAlgo::Quantise { step_db: 6.0 }),
        (
            "spectral_lossy",
            SpectralAlgo::Lossy {
                threshold_db: -30.0,
                hold_ms: 30.0,
                step_db: 4.0,
                dropout: 0.2,
            },
        ),
    ] {
        let mut sp = Spectral::new(SR, 256);
        sp.set_algorithm(algo);
        run(name, &mut |l, r| sp.process_block(l, r));
    }
    let mut md = Modal::new(SR);
    for i in 0..12 {
        md.set_mode(i, 180.0 * (1.0 + 0.37 * i as f32), 0.8, 0.6);
    }
    md.set_active(9);
    run("modal", &mut |l, r| {
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            let (x, y) = md.process(*a);
            *a = x;
            *b = y;
        }
    });
    let mut wb = Wobble::new(SR, 8, 2.0);
    wb.set_depth_ms(1.2);
    run("wobble", &mut |l, r| wb.process_block(l, r));
    let mut sm = Smoother2::with_class(0.0, SmootherClass::Medium, SR);
    run("smoother2", &mut |l, r| {
        for (i, (a, b)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
            if i % 700 == 0 {
                sm.set_target(if (i / 700) % 2 == 0 { 1.0 } else { -0.5 });
            }
            *a = sm.next_value();
            *b = *a * 2.0;
        }
    });
    out
}

#[test]
fn no_stage_block_allocates_while_processing() {
    // The blocks are built (allocating) inside `render_all`; only the processing is guarded.
    let _ = render_all(false);
    // A second pass with the guard: construction happens before `assert_no_alloc` per block,
    // the closure holds only `process` calls.
    let guarded = render_all(true);
    assert!(guarded.len() >= 25);
}

#[test]
fn renders_are_pinned_across_platforms() {
    let pinned = |v: Vec<(&'static str, u64)>| -> Vec<(&'static str, u64)> {
        v.into_iter().filter(|(n, _)| *n != "crossover").collect()
    };
    let got = pinned(render_all(false));
    let again = pinned(render_all(false));
    assert_eq!(got, again, "two renders in one process are identical");
    assert_eq!(got.as_slice(), PINS);
}

/// FNV-1a hashes of the left then right output of each block for `input(1)`, `input(2)`.
const PINS: &[(&str, u64)] = &[
    ("shaper_soft", 0xf76efcc77005b23d),
    ("shaper_asym", 0x1ec787f990bfef96),
    ("shaper_hard", 0x63d92ef923de9793),
    ("shaper_fold", 0xd7383d5cde7a69fc),
    ("shaper_dynamic", 0x4a1b641596116980),
    ("leveller", 0x1fa8262bdfeca469),
    ("phaser", 0x6e8e9ddf5851a6bc),
    ("chorus", 0x0a74998171eb99ff),
    ("echo", 0xb75ac87d8a3ea64f),
    ("comb", 0xa8418e704eb3bda8),
    ("room_small", 0xcc394d2875160e4b),
    ("room_large", 0x92da2996f26e42a9),
    ("ringmod", 0x808e277a0d4c7a94),
    ("bitcrush", 0x1f8ac832e179d278),
    ("noise", 0x9d816a0ec16499bf),
    ("spectral_identity", 0x3423ae3e77afbc33),
    ("spectral_gate", 0x681240388b306e47),
    ("spectral_slew", 0x202c60ff2e1683bb),
    ("spectral_pass", 0x1da3b60529fc7933),
    ("spectral_stop", 0xdea8eb6b775588cd),
    ("spectral_quant", 0xfb57fac711e662b2),
    ("spectral_lossy", 0x5672ec89910706c1),
    ("modal", 0xf5fb6dec967ea188),
    ("wobble", 0xf0452f8697a3dfaa),
    ("smoother2", 0x85d808cff66d9ccb),
];
