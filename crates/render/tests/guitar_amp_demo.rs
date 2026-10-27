//! The guitar amp bounced offline: a synthesised guitar DI (Karplus-Strong plucks: palm-muted
//! chugs on a low D, a lead line, a power chord, and a faint pickup hum under all of it) goes into
//! `interface:in_1`, through `atomik.guitar-amp` loaded with each factory setup, to the outputs.
//! No third-party audio: the DI is generated here with IEEE `+ - * /` only, so it is the same
//! samples on every platform.
//!
//! * every setup sounds, stays finite and bounded, at a comparable level;
//! * a bounce is bit-identical at every host block size, under the allocation guard (HI-1, HI-2);
//! * the setups differ, and do what their names say: the industrial rhythm's gate keeps the hum out
//!   of the gaps, the lead's delay and reverb ring on after the last note, the synthwave sound is
//!   wide and the dry amp sounds are not;
//! * a fingerprint (RMS per half second, per setup) is checked within 0.5 dB on every platform.
//!   Regenerate it after an intentional change to the sound, and listen first:
//!
//! ```text
//! ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-render --test guitar_amp_demo fingerprint
//! ```
atomik_testkit::install_alloc_guard!();

use atomik_devices::guitar_amp::setups::{SETUP_NAMES, SETUPS};
use atomik_engine::{OfflineJob, render_offline_with};
use atomik_render::build::{BuildOptions, build_with};
use atomik_render::patch::Patch;
use atomik_testkit::no_alloc::assert_no_alloc;
use std::path::PathBuf;

const RATE: usize = 48_000;
/// The DI lasts 6 s; the render adds the patch's 2-beat tail (1 s at 120 BPM) and a margin.
const DI_FRAMES: usize = 6 * RATE;
const LENGTH_BEATS: u32 = 14;
const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";
const WINDOW: usize = RATE / 2;
const FLOOR_DB: f64 = -100.0;

/// xorshift32 noise in `[-1, 1)`.
struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / 8_388_608.0 - 1.0
    }
}

/// Adds a Karplus-Strong pluck of `hz` starting at `start` seconds, `len` seconds long, into
/// `out`: a burst of low-passed noise circulating in a delay line with a two-point average (the
/// string loses its top as it rings), `decay` per trip.
fn pluck(out: &mut [f32], start: f32, len: f32, hz: f32, decay: f32, amp: f32, seed: u32) {
    let n = ((RATE as f32 / hz).round() as usize).max(2);
    let mut noise = Noise(seed);
    let mut line = vec![0.0f32; n];
    let mut last = 0.0f32;
    for v in &mut line {
        // A soft pick: the burst is low-passed once.
        last += 0.55 * (noise.next() - last);
        *v = last * 1.7;
    }
    let from = (start * RATE as f32) as usize;
    let frames = (len * RATE as f32) as usize;
    let mut i = 0;
    for k in 0..frames {
        let next = line[(i + 1) % n];
        let y = line[i];
        line[i] = decay * 0.5 * (y + next);
        let idx = from + k;
        if idx < out.len() {
            // A short fade-out so a note ends without a click.
            let left = (frames - k) as f32;
            let fade = if left < 480.0 { left / 480.0 } else { 1.0 };
            out[idx] += y * amp * fade;
        }
        i = (i + 1) % n;
    }
}

/// The guitar DI: 0 to 2 s palm-muted chugs on D2 (eighths at 120 BPM), 2 to 4 s a lead line
/// (D4 F4 A4 D5, half a second each), 4 to 5 s a D power chord, then 1 s of nothing but the
/// pickup hum (white noise at about -66 dBFS, under everything).
fn guitar_di() -> Vec<f32> {
    let mut out = vec![0.0f32; DI_FRAMES];
    for k in 0..8 {
        pluck(&mut out, 0.25 * k as f32, 0.2, 73.42, 0.985, 0.35, 11 + k);
    }
    for (k, hz) in [293.66f32, 349.23, 440.0, 587.33].into_iter().enumerate() {
        pluck(
            &mut out,
            2.0 + 0.5 * k as f32,
            0.5,
            hz,
            0.9985,
            0.3,
            40 + k as u32,
        );
    }
    for (k, hz) in [73.42f32, 110.0, 146.83].into_iter().enumerate() {
        pluck(&mut out, 4.0, 1.0, hz, 0.9992, 0.22, 90 + k as u32);
    }
    let mut hum = Noise(0x9e37_79b9);
    for s in &mut out {
        *s += hum.next() * 0.0009;
    }
    out
}

/// The patch: the amp between the first host input and the two outputs, `params` set on it.
fn patch_text(params: &[(&str, f32)]) -> String {
    let params: serde_json::Map<String, serde_json::Value> = params
        .iter()
        .map(|(k, v)| ((*k).to_string(), serde_json::json!(*v)))
        .collect();
    serde_json::json!({
        "format": "atomik-patch", "version": 1,
        "sample_rate": RATE, "tempo_bpm": 120, "seed": 5,
        "length_beats": LENGTH_BEATS, "output_channels": 2,
        "devices": [{ "id": "amp", "type": "atomik.guitar-amp", "params": params }],
        "cables": [
            { "from": "interface:in_1", "to": "amp:in_l" },
            { "from": "amp:out_l", "to": "interface:out_1" },
            { "from": "amp:out_r", "to": "interface:out_2" }
        ]
    })
    .to_string()
}

/// The values of setup `i` as params, with the setup's own index.
fn setup_params(i: usize) -> Vec<(&'static str, f32)> {
    let mut p = vec![("setup", i as f32)];
    p.extend(SETUPS[i].values);
    p
}

/// Bounces the DI through the amp with `params`, in host blocks of `host_block`, under the guard.
fn bounce(params: &[(&str, f32)], host_block: usize) -> Vec<Vec<f32>> {
    let patch = Patch::from_json(&patch_text(params)).unwrap();
    let built = build_with(
        &patch,
        &atomik_render::default_registry(),
        &BuildOptions {
            input_channels: 1,
            ..BuildOptions::default()
        },
    )
    .unwrap();
    assert!(built.warnings.is_empty(), "{:?}", built.warnings);
    let job = OfflineJob {
        frames: built.frames,
        host_block,
        notes: &built.notes,
        params: &[],
        inputs: &[guitar_di()],
    };
    render_offline_with(built.plan, built.config, &job, |f| assert_no_alloc(f))
}

fn rms_db(x: &[f32]) -> f64 {
    let ms = x.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / x.len().max(1) as f64;
    (10.0 * ms.log10()).max(FLOOR_DB)
}

/// The seconds `from..to` of a channel.
fn seconds(x: &[f32], from: f32, to: f32) -> &[f32] {
    &x[(from * RATE as f32) as usize..(to * RATE as f32) as usize]
}

fn bits(x: &[Vec<f32>]) -> Vec<u32> {
    x.iter().flatten().map(|s| s.to_bits()).collect()
}

#[test]
fn every_setup_sounds_and_they_are_about_equally_loud() {
    let mut levels = Vec::new();
    for (i, name) in SETUP_NAMES.iter().enumerate() {
        let out = bounce(&setup_params(i), 512);
        assert_eq!(out.len(), 2);
        assert!(
            out.iter()
                .flatten()
                .all(|s| s.is_finite() && s.abs() < 16.0),
            "{name}"
        );
        let played = rms_db(seconds(&out[0], 0.0, 5.0));
        println!("{name}: {played:.1} dBFS over the playing");
        assert!(played > -30.0 && played < -6.0, "{name}: {played}");
        levels.push(played);
    }
    let (lo, hi) = levels
        .iter()
        .fold((f64::MAX, f64::MIN), |(a, b), l| (a.min(*l), b.max(*l)));
    assert!(hi - lo < 4.0, "comparable loudness: {levels:?}");
}

#[test]
fn a_bounce_is_bit_identical_at_every_host_block_and_when_repeated() {
    // The two with the most going on: the lead (delay, reverb) and the synthwave sound (chorus,
    // synced delay, reverb).
    for i in [3, 4] {
        let reference = bounce(&setup_params(i), 1);
        assert!(
            bits(&bounce(&setup_params(i), 1)) == bits(&reference),
            "repeat"
        );
        for block in [7, 63, 64, 480, 4096] {
            assert!(
                bits(&bounce(&setup_params(i), block)) == bits(&reference),
                "{}: host block {block} differs from host block 1",
                SETUP_NAMES[i]
            );
        }
    }
}

#[test]
fn the_setups_differ_from_each_other() {
    let outs: Vec<Vec<f32>> = (0..SETUPS.len())
        .map(|i| bounce(&setup_params(i), 512).swap_remove(0))
        .collect();
    for a in 0..outs.len() {
        for b in a + 1..outs.len() {
            // Level-matched, so a difference is a different sound, not only a different volume.
            let diff: f64 = outs[a]
                .iter()
                .zip(&outs[b])
                .map(|(x, y)| f64::from(x - y).powi(2))
                .sum::<f64>()
                / outs[a].len() as f64;
            let reference: f64 =
                outs[a].iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / outs[a].len() as f64;
            assert!(
                diff > 0.2 * reference,
                "{} and {} are too alike ({diff} against {reference})",
                SETUP_NAMES[a],
                SETUP_NAMES[b]
            );
        }
    }
}

#[test]
fn the_industrial_rhythm_gate_keeps_the_hum_out_of_the_gap() {
    let gated = setup_params(2);
    let mut open: Vec<(&str, f32)> = gated.clone();
    open.retain(|(k, _)| *k != "gate");
    open.push(("gate", -80.0));
    // The last second of the DI is the hum alone (the tail of the chord is gone by 5.4 s).
    let quiet = |p: &[(&str, f32)]| rms_db(seconds(&bounce(p, 512)[0], 5.6, 6.0));
    let (with, without) = (quiet(&gated), quiet(&open));
    println!("hum through the gate {with:.1} dB, without {without:.1} dB");
    assert!(without - with > 20.0, "{with} {without}");
    // And the chugs still come through: the gate does not eat the playing.
    let played = |p: &[(&str, f32)]| rms_db(seconds(&bounce(p, 512)[0], 0.0, 2.0));
    assert!((played(&gated) - played(&open)).abs() < 1.5);
}

#[test]
fn the_lead_rings_on_after_the_last_note_through_its_delay_and_reverb() {
    let lead = setup_params(3);
    let mut dry = lead.clone();
    dry.retain(|(k, _)| *k != "delay.on" && *k != "reverb.on");
    dry.push(("delay.on", 0.0));
    dry.push(("reverb.on", 0.0));
    // The chord is over by 5.2 s; look at 5.6 to 6.6 s.
    let tail = |p: &[(&str, f32)]| rms_db(seconds(&bounce(p, 512)[0], 5.6, 6.6));
    let (wet, dried) = (tail(&lead), tail(&dry));
    println!("tail with the delay and reverb {wet:.1} dB, without {dried:.1} dB");
    assert!(wet - dried > 12.0, "{wet} {dried}");
}

#[test]
fn the_synthwave_sound_is_wide_and_the_industrial_one_is_not() {
    let wide = bounce(&setup_params(4), 512);
    let narrow = bounce(&setup_params(2), 512);
    assert_eq!(
        bits(&narrow[..1]),
        bits(&narrow[1..]),
        "no effects: both outputs alike"
    );
    let side: f64 = wide[0]
        .iter()
        .zip(&wide[1])
        .map(|(l, r)| f64::from(l - r).powi(2))
        .sum::<f64>();
    let mid: f64 = wide[0]
        .iter()
        .zip(&wide[1])
        .map(|(l, r)| f64::from(l + r).powi(2))
        .sum::<f64>();
    println!("synthwave side/mid {:.1} dB", 10.0 * (side / mid).log10());
    assert!(side > 0.02 * mid, "{side} {mid}");
}

fn golden_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/golden/guitar-amp.fingerprint.json")
}

fn fingerprint() -> serde_json::Value {
    let mut setups = serde_json::Map::new();
    for (i, name) in SETUP_NAMES.iter().enumerate() {
        let out = bounce(&setup_params(i), 512);
        let rms: Vec<f64> = out[0]
            .chunks(WINDOW)
            .map(|w| (rms_db(w) * 100.0).round() / 100.0)
            .collect();
        setups.insert((*name).to_string(), serde_json::json!(rms));
    }
    serde_json::json!({
        "what": "atomik.guitar-amp through each setup, the synthesised guitar DI of guitar_amp_demo.rs",
        "window_frames": WINDOW,
        "floor_db": FLOOR_DB,
        "rms_db_left": setups,
    })
}

#[test]
fn fingerprint_matches_the_golden_within_half_a_decibel() {
    let got = fingerprint();
    if std::env::var_os(UPDATE_ENV).is_some_and(|v| v == "1") {
        std::fs::write(
            golden_path(),
            serde_json::to_string_pretty(&got).unwrap() + "\n",
        )
        .unwrap();
        return;
    }
    let text = std::fs::read_to_string(golden_path())
        .unwrap_or_else(|_| panic!("missing golden; run with {UPDATE_ENV}=1"));
    let want: serde_json::Value = serde_json::from_str(&text).unwrap();
    let (got, want) = (&got["rms_db_left"], &want["rms_db_left"]);
    for name in SETUP_NAMES {
        let (g, w) = (
            got[name].as_array().unwrap(),
            want[name].as_array().unwrap(),
        );
        assert_eq!(g.len(), w.len(), "{name}: windows");
        for (k, (a, b)) in g.iter().zip(w).enumerate() {
            let (a, b) = (a.as_f64().unwrap(), b.as_f64().unwrap());
            assert!(
                (a - b).abs() <= 0.5,
                "{name}, window {k}: {a} dB against the golden {b} dB"
            );
        }
    }
}
