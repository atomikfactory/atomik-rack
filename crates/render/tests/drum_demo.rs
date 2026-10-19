//! The `drum-demo` example project and its bundle, and the bounce of drum
//! projects with individual outputs through console channels.
//!
//! `examples/projects/drum-demo/` is a bundle folder: `drum-demo.atomik` and `Audio/` with
//! five short **procedurally generated** mono drum sounds ([`demo_samples`]: kick, snare, closed
//! and open hat, clap; no third-party audio). One drum machine plays a two-bar pattern (a clip
//! with a 32 x 1/16 step grid and 30 % swing):
//!
//! * pad 1 kick, pad 3 snare, pad 4 clap, pad 7 closed hat, pad 11 open hat (GM keys 36, 38,
//!   39, 42, 46);
//! * the closed and the open hat share choke group 1, so a closed hat cuts the open one;
//! * the **snare's individual output is cabled to its own console channel** ("Snare", panned a
//!   little to the right), which feeds the master bus like every other channel; the other pads
//!   go through the drum machine's own channel.
//!
//! Regenerate the bundle and the project after an intentional change with:
//!
//! ```text
//! ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-render --test drum_demo regenerate
//! ```
//!
//! The generator uses only IEEE `+ - * /` and comparisons (no libm calls), so it gives the same
//! samples on every platform; the committed WAVs are checked against it bit for bit.

use atomik_audio_file::{AudioData, Bundle, content_hash};
use atomik_core::PPQ;
use atomik_model::{
    AssetMeta, Cable, Clip, ClipContent, ClipId, Command, ConsoleCommand, DeviceKey, Document,
    Note, RackDoc, SongCommand, StepGrid, StepResolution, empty_doc, load_project, pad_key,
    save_project, with_pad_files,
};
use atomik_render::bundle::{existing_bundle_root, missing_assets};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc_in, render_project_file};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";
const MAX_BUNDLE_BYTES: u64 = 1024 * 1024;
/// 1/16 note.
const STEP: i64 = PPQ / 4;
const STEPS: u16 = 32;
const SWING: u8 = 30;
const KICK: u8 = 1;
const SNARE: u8 = 3;
const CLAP: u8 = 4;
const CLOSED: u8 = 7;
const OPEN: u8 = 11;

fn demo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/projects/drum-demo")
}

fn demo_file() -> PathBuf {
    demo_root().join("drum-demo.atomik")
}

/// A sine-shaped wave of phase `x` in `[0, 1)` from two parabolas (no libm call).
fn wave(x: f32) -> f32 {
    if x < 0.5 {
        16.0 * x * (0.5 - x)
    } else {
        -16.0 * (x - 0.5) * (1.0 - x)
    }
}

/// A xorshift noise source in `[-1, 1)`.
struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / 8_388_608.0 - 1.0
    }
}

/// The last 200 frames fade to exactly zero.
fn tail(i: usize, frames: usize) -> f32 {
    let left = (frames - i) as f32;
    if left < 200.0 {
        (left - 1.0) / 200.0
    } else {
        1.0
    }
}

fn mono(out: Vec<f32>) -> AudioData {
    AudioData::new(48_000, vec![out]).expect("valid mono data")
}

fn kick(frames: usize) -> AudioData {
    let (mut phase, mut sweep, mut amp) = (0.0f32, 1.0f32, 0.95f32);
    mono(
        (0..frames)
            .map(|i| {
                let hz = 48.0 + 120.0 * sweep;
                let s = wave(phase) * amp * tail(i, frames);
                phase += hz / 48_000.0;
                phase -= if phase >= 1.0 { 1.0 } else { 0.0 };
                sweep *= 0.9992;
                amp *= 0.99988;
                s
            })
            .collect(),
    )
}

fn snare(frames: usize) -> AudioData {
    let (mut noise, mut phase) = (Noise(0x2545_f491), 0.0f32);
    let (mut amp_n, mut amp_t) = (0.55f32, 0.5f32);
    mono(
        (0..frames)
            .map(|i| {
                let s = (noise.next() * amp_n + wave(phase) * amp_t) * tail(i, frames);
                phase += 190.0 / 48_000.0;
                phase -= if phase >= 1.0 { 1.0 } else { 0.0 };
                amp_n *= 0.99935;
                amp_t *= 0.9985;
                s
            })
            .collect(),
    )
}

/// Differentiated noise: a hat. `decay` is the per-frame amplitude factor.
fn hat(frames: usize, decay: f32, seed: u32) -> AudioData {
    let (mut noise, mut last, mut amp) = (Noise(seed), 0.0f32, 0.6f32);
    mono(
        (0..frames)
            .map(|i| {
                let n = noise.next();
                let s = (n - last) * 0.5 * amp * tail(i, frames);
                last = n;
                amp *= decay;
                s
            })
            .collect(),
    )
}

/// Three quick noise bursts and a tail.
fn clap(frames: usize) -> AudioData {
    let (mut noise, mut last, mut amp) = (Noise(0x9e37_79b9), 0.0f32, 0.0f32);
    mono(
        (0..frames)
            .map(|i| {
                if i == 0 || i == 700 || i == 1_400 {
                    amp = 0.7;
                }
                let n = noise.next();
                let s = (n - last) * 0.5 * amp * tail(i, frames);
                last = n;
                amp *= if i < 2_100 { 0.995 } else { 0.9993 };
                s
            })
            .collect(),
    )
}

/// The five demo sounds as `(file name, audio)`.
pub fn demo_samples() -> Vec<(&'static str, AudioData)> {
    vec![
        ("kick.wav", kick(14_400)),
        ("snare.wav", snare(9_600)),
        ("hat_closed.wav", hat(2_880, 0.9985, 0x1234_5679)),
        ("hat_open.wav", hat(14_400, 0.9998, 0x0bad_cafe)),
        ("clap.wav", clap(7_200)),
    ]
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

/// Gives pad `pad` of `drums` its own console channel through its individual output
/// (`ConsoleCommand::AddChannelFor`, the command of the drum panel's TO CH button), named
/// `name`. The channel feeds the master bus like every channel. Returns the channel's key.
pub fn add_pad_channel(d: &mut Document, drums: &str, pad: u8, name: &str) -> DeviceKey {
    let registry = default_registry();
    d.apply(
        Command::Console(ConsoleCommand::AddChannelFor {
            source: key(drums),
            out_l: format!("pad{pad}_out_l"),
            out_r: format!("pad{pad}_out_r"),
            name: Some(name.into()),
        }),
        &registry,
    )
    .unwrap_or_else(|e| panic!("AddChannelFor pad {pad}: {e}"));
    d.doc()
        .console
        .channels
        .last()
        .expect("the new channel is the last one")
        .channel
        .clone()
}

/// `(beat step, velocity)` lists of the pattern, steps 0..32.
fn pattern() -> Vec<(u8, u16, f32)> {
    let mut out = Vec::new();
    for step in [0u16, 6, 10, 16, 22, 26, 30] {
        out.push((KICK, step, if step % 16 == 0 { 1.0 } else { 0.75 }));
    }
    for step in [4u16, 12, 20, 28] {
        out.push((SNARE, step, if step == 28 { 0.8 } else { 1.0 }));
    }
    for step in [12u16, 28] {
        out.push((CLAP, step, 0.45));
    }
    for step in 0..STEPS {
        if step == 14 || step == 30 {
            out.push((OPEN, step, 0.85));
        } else {
            out.push((CLOSED, step, if step % 2 == 0 { 0.8 } else { 0.45 }));
        }
    }
    out
}

/// The project for sounds whose hashes and formats are `pool` (in [`demo_samples`] order).
pub fn demo_doc(pool: &[(String, AssetMeta)]) -> RackDoc {
    let registry = default_registry();
    let mut d = Document::new(empty_doc());
    for (hash, meta) in pool {
        d.apply(
            Command::RegisterAsset {
                hash: hash.clone(),
                meta: meta.clone(),
            },
            &registry,
        )
        .expect("asset registers");
    }
    let mut apply = |c: Command| {
        d.apply(c.clone(), &registry)
            .unwrap_or_else(|e| panic!("{c:?}: {e}"));
    };
    apply(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.drum-machine".into(),
        key: Some(key("drums")),
    });
    apply(Command::Rename {
        key: key("drums"),
        name: "Drums".into(),
    });
    apply(Command::SetColor {
        key: key("drums"),
        color: Some([0xd9, 0x4f, 0x4f]),
    });
    let param = |p: &str, value: f32| Command::SetParam {
        key: key("drums"),
        param: p.into(),
        value,
        merge: None,
    };
    // Individual outputs are not scaled by the master level, so 0 dB keeps every pad at the
    // level it has through its own channel as well.
    apply(param("master.level", 0.0));
    let mut zones = Vec::new();
    for (pad, index, level, decay) in [
        (KICK, 0usize, -2.0f32, 450.0f32),
        (SNARE, 1, -4.0, 300.0),
        (CLOSED, 2, -10.0, 90.0),
        (OPEN, 3, -10.0, 600.0),
        (CLAP, 4, -8.0, 250.0),
    ] {
        let (hash, meta) = &pool[index];
        let file = (
            hash.clone(),
            demo_samples()[index].0.to_string(),
            meta.frames,
        );
        zones = with_pad_files(&zones, pad, &[file]);
        apply(param(&format!("pad{pad}.level"), level));
        apply(param(&format!("pad{pad}.decay"), decay));
    }
    // The hats choke each other.
    apply(param(&format!("pad{CLOSED}.choke"), 1.0));
    apply(param(&format!("pad{OPEN}.choke"), 1.0));
    apply(Command::SetZones {
        device: key("drums"),
        zones,
        merge: None,
    });
    let snare_channel = add_pad_channel(&mut d, "drums", SNARE, "Snare");
    let mut apply = |c: Command| {
        d.apply(c.clone(), &registry)
            .unwrap_or_else(|e| panic!("{c:?}: {e}"));
    };
    apply(Command::SetParam {
        key: snare_channel,
        param: "pan".into(),
        value: 0.15,
        merge: None,
    });
    let length = i64::from(STEPS) * STEP;
    let notes = pattern()
        .into_iter()
        .map(|(pad, step, velocity)| Note {
            tick: i64::from(step) * STEP,
            length: STEP,
            key: pad_key(pad),
            velocity,
        })
        .collect();
    let mut notes: Vec<Note> = notes;
    notes.sort_by_key(|n| (n.tick, n.key));
    apply(Command::Song(SongCommand::AddTrack {
        device: key("drums"),
    }));
    apply(Command::Song(SongCommand::AddClip {
        device: key("drums"),
        lane: 0,
        clip: Clip {
            id: ClipId(0),
            start: 0,
            length,
            offset: 0,
            loop_length: None,
            muted: false,
            grid: Some(StepGrid {
                resolution: StepResolution::Sixteenth,
                steps: STEPS,
                swing: SWING,
            }),
            content: ClipContent::Notes(notes),
        },
    }));
    apply(Command::Song(SongCommand::SetSongEnd(length)));
    apply(Command::Song(SongCommand::SetLoop {
        range: (0, length),
        enabled: false,
    }));
    d.doc().clone()
}

/// `(hash, meta)` of every demo sound, in [`demo_samples`] order.
fn demo_pool() -> Vec<(String, AssetMeta)> {
    demo_samples()
        .into_iter()
        .map(|(name, data)| {
            (
                content_hash(&data),
                AssetMeta {
                    channels: 1,
                    sample_rate: 48_000,
                    frames: data.frames() as u64,
                    original_name: name.to_string(),
                },
            )
        })
        .collect()
}

#[test]
fn regenerate() {
    if std::env::var_os(UPDATE_ENV).is_none() {
        return;
    }
    let root = demo_root();
    let mut bundle = Bundle::create(&root).expect("bundle folder");
    for (name, data) in demo_samples() {
        bundle.add_recording(&data, name).expect("asset written");
    }
    // Only the assets belong in the repository; the peak cache is rebuilt on demand.
    let _ = std::fs::remove_dir_all(bundle.peaks_dir());
    let doc = demo_doc(&demo_pool());
    bundle
        .remove_orphans(&doc.referenced_assets())
        .expect("orphans removed");
    std::fs::write(demo_file(), save_project(&doc)).expect("project written");
}

fn registry_and_doc() -> (atomik_device_sdk::Registry, RackDoc) {
    let registry = default_registry();
    let text = std::fs::read_to_string(demo_file()).expect("the example project");
    let doc = load_project(&text, &registry).expect("a valid project");
    (registry, doc)
}

fn dir_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|d| {
            d.flatten()
                .map(|e| {
                    let p = e.path();
                    if p.is_dir() {
                        dir_bytes(&p)
                    } else {
                        e.metadata().map_or(0, |m| m.len())
                    }
                })
                .sum()
        })
        .unwrap_or(0)
}

#[test]
fn the_generators_are_deterministic_short_and_finite() {
    let a = demo_samples();
    let b = demo_samples();
    assert_eq!(a.len(), 5);
    for ((n1, d1), (n2, d2)) in a.iter().zip(&b) {
        assert_eq!(n1, n2);
        assert!(d1 == d2, "{n1} is deterministic");
        assert!(d1.channels[0].iter().all(|x| x.is_finite()), "{n1}");
        let peak = d1.channels[0].iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!((0.05..0.99).contains(&peak), "{n1} peak {peak}");
        assert_eq!(*d1.channels[0].last().unwrap(), 0.0, "{n1} ends silent");
        assert!(d1.frames() <= 48_000);
    }
}

#[test]
fn the_committed_bundle_is_intact_the_project_canonical_and_the_whole_small() {
    let (_, doc) = registry_and_doc();
    let text = std::fs::read_to_string(demo_file()).unwrap();
    assert_eq!(save_project(&doc), text.replace("\r\n", "\n"), "canonical");
    assert_eq!(
        save_project(&demo_doc(&demo_pool())),
        text.replace("\r\n", "\n"),
        "the project is the generator's output"
    );
    let root = existing_bundle_root(&demo_file()).expect("its own folder is the bundle");
    assert_eq!(root, demo_root());
    let bundle = Bundle::open(&root).expect("bundle");
    assert!(missing_assets(&doc, Some(&bundle)).is_empty());
    assert_eq!(doc.assets.len(), 5);
    for (name, data) in demo_samples() {
        let hash = content_hash(&data);
        assert!(doc.assets.contains_key(&hash), "{name} is in the pool");
        let stored = bundle.load(&hash).expect("the asset decodes");
        assert!(stored == data, "{name} is the generator's output");
    }
    assert!(
        dir_bytes(&root) <= MAX_BUNDLE_BYTES,
        "{} bytes",
        dir_bytes(&root)
    );
    let mut names: Vec<String> = std::fs::read_dir(&root)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["Audio", "drum-demo.atomik"]);
}

#[test]
fn the_demo_has_swung_steps_a_hat_choke_group_and_the_snare_on_its_own_channel() {
    let (_, doc) = registry_and_doc();
    let drums = doc.device(&key("drums")).unwrap();
    assert_eq!(drums.zones.len(), 5);
    let pads: Vec<u8> = [KICK, SNARE, CLAP, CLOSED, OPEN].to_vec();
    for pad in pads {
        assert!(
            drums
                .zones
                .iter()
                .any(|z| z.key_lo == pad_key(pad) && z.key_hi == pad_key(pad)),
            "pad {pad} has a zone"
        );
    }
    let choke = |pad: u8| drums.params.get(&format!("pad{pad}.choke")).copied();
    assert_eq!(choke(CLOSED), Some(1.0));
    assert_eq!(choke(OPEN), Some(1.0));
    let track = doc.song.track(&key("drums")).unwrap();
    let clip = &track.lanes[0].clips[0];
    assert_eq!(
        clip.grid,
        Some(StepGrid {
            resolution: StepResolution::Sixteenth,
            steps: STEPS,
            swing: SWING
        })
    );
    assert_eq!(clip.length, 2 * 4 * PPQ, "two bars");
    // The snare's individual output feeds a console channel of its own (source: the drum
    // machine); the snare's main-mix sockets are not involved.
    let snare_cables: Vec<&Cable> = doc
        .cables
        .iter()
        .filter(|c| c.from.device == key("drums") && c.from.socket.starts_with("pad3_out_"))
        .collect();
    assert_eq!(snare_cables.len(), 2);
    let channel = &snare_cables[0].to.device;
    assert!(doc.console.channels.iter().any(|c| &c.channel == channel));
    assert_eq!(doc.console.channels.len(), 2, "the drums' and the snare's");
}

fn options(block: usize, workers: u16) -> ProjectOptions {
    ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    }
}

fn bits(r: &atomik_render::Rendered) -> Vec<u32> {
    r.channels.iter().flatten().map(|x| x.to_bits()).collect()
}

fn bounce(doc: &RackDoc, block: usize, workers: u16) -> atomik_render::Rendered {
    render_doc_in(
        doc,
        &default_registry(),
        &options(block, workers),
        Some(&demo_root()),
    )
    .unwrap()
}

fn rms(channel: &[f32], from: usize, to: usize) -> f64 {
    let w = &channel[from..to.min(channel.len())];
    (w.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / w.len().max(1) as f64).sqrt()
}

/// The demo's snare channel (named "Snare").
fn snare_channel(doc: &RackDoc) -> DeviceKey {
    doc.console
        .channels
        .iter()
        .find(|c| doc.device(&c.channel).is_some_and(|d| d.name == "Snare"))
        .expect("the snare channel")
        .channel
        .clone()
}

/// The demo with the kick on its own console channel too (kick and snare).
fn kick_and_snare_doc() -> (RackDoc, DeviceKey, DeviceKey) {
    let (_, doc) = registry_and_doc();
    let snare = snare_channel(&doc);
    let mut d = Document::new(doc);
    let kick = add_pad_channel(&mut d, "drums", KICK, "Kick");
    (d.doc().clone(), kick, snare)
}

/// The same project with every individual cable removed: all pads in the main mix.
fn without_individual_cables(doc: &RackDoc) -> RackDoc {
    let mut out = doc.clone();
    out.cables
        .retain(|c| !(c.from.device == key("drums") && c.from.socket.starts_with("pad")));
    out
}

/// The console channel fed by a source.
fn channel_of(doc: &RackDoc, source: &str) -> DeviceKey {
    doc.console
        .channel_of(&key(source))
        .expect("channel")
        .channel
        .clone()
}

fn set(doc: &mut RackDoc, device: &DeviceKey, param: &str, value: f32) {
    doc.devices
        .iter_mut()
        .find(|d| &d.key == device)
        .unwrap()
        .params
        .insert(param.into(), value);
}

/// The bounce does not depend on the host block size or the worker count,
/// with the snare (and the kick) on their own channels.
#[test]
fn the_bounce_is_bit_identical_across_host_blocks_and_worker_counts() {
    let (_, demo) = registry_and_doc();
    let (both, _, _) = kick_and_snare_doc();
    for (name, doc) in [("demo", demo), ("kick and snare", both)] {
        let reference = bits(&bounce(&doc, 512, 0));
        assert!(reference.len() > 300_000, "{name}");
        for block in [64, 333, 512, 4096] {
            for workers in 0..=4 {
                assert!(
                    bits(&bounce(&doc, block, workers)) == reference,
                    "{name}: block {block}, {workers} workers"
                );
            }
        }
    }
}

/// Pads move, nothing is lost: with the kick and snare on their channels the master gets the
/// same signal as with every pad in the main mix (the only difference is the order of the
/// additions), and each channel carries exactly its pad.
#[test]
fn individual_outputs_move_pads_to_their_channels_and_lose_nothing() {
    let (doc, kick_channel, snare_channel) = kick_and_snare_doc();
    let plain = without_individual_cables(&doc);
    let with = bounce(&doc, 512, 0);
    let reference = bounce(&plain, 512, 0);
    let peak = reference
        .channels
        .iter()
        .flatten()
        .fold(0.0f32, |m, x| m.max(x.abs()));
    assert!((0.05..1.0).contains(&peak), "reference peak {peak}");
    // The strips are alike (default fader, pan 0) except the snare's pan: reset it, so the
    // two renders are the same signal.
    let mut doc = doc;
    set(&mut doc, &snare_channel, "pan", 0.0);
    let with_flat = bounce(&doc, 512, 0);
    for (c, (a, b)) in with_flat
        .channels
        .iter()
        .zip(&reference.channels)
        .enumerate()
    {
        assert_eq!(a.len(), b.len());
        let worst = a
            .iter()
            .zip(b)
            .fold(0.0f32, |m, (x, y)| m.max((x - y).abs()));
        assert!(worst < 2e-5, "channel {c}: master sums differ by {worst}");
    }
    assert!(bits(&with) != bits(&with_flat), "the snare pan is audible");

    // Where the pads went: mute the drums' own channel and only kick and snare remain.
    let main = channel_of(&doc, "drums");
    let mut only_individual = doc.clone();
    set(&mut only_individual, &main, "mute", 1.0);
    let heard = bounce(&only_individual, 512, 0);
    assert!(
        rms(&heard.channels[0], 0, 24_000) > 1e-3,
        "kick and snare sound"
    );
    // Without cables the same mute silences everything: nothing was left in the main mix.
    let mut all_main = plain.clone();
    set(&mut all_main, &main, "mute", 1.0);
    let silent = bounce(&all_main, 512, 0);
    assert!(rms(&silent.channels[0], 0, 96_000) < 1e-6);
    // The other way: mute both pad channels and the hats, clap remain, no kick or snare.
    let mut rest = doc.clone();
    set(&mut rest, &kick_channel, "mute", 1.0);
    set(&mut rest, &snare_channel, "mute", 1.0);
    let rest = bounce(&rest, 512, 0);
    let mut no_kick_snare = plain;
    {
        let track = no_kick_snare
            .song
            .tracks
            .iter_mut()
            .find(|t| t.device == key("drums"))
            .unwrap();
        for clip in &mut track.lanes[0].clips {
            if let ClipContent::Notes(n) = &mut clip.content {
                n.retain(|n| n.key != pad_key(KICK) && n.key != pad_key(SNARE));
            }
        }
    }
    let expected = bounce(&no_kick_snare, 512, 0);
    for (a, b) in rest.channels.iter().zip(&expected.channels) {
        let worst = a
            .iter()
            .zip(b)
            .fold(0.0f32, |m, (x, y)| m.max((x - y).abs()));
        assert!(
            worst < 2e-5,
            "muting the pad channels removes exactly those pads: {worst}"
        );
    }
}

#[test]
fn swing_and_choke_change_the_sound() {
    let (_, doc) = registry_and_doc();
    let base = bounce(&doc, 512, 0);
    // Without swing the odd steps move back.
    let mut straight = doc.clone();
    for t in &mut straight.song.tracks {
        for clip in &mut t.lanes[0].clips {
            clip.grid.as_mut().unwrap().swing = 0;
        }
    }
    assert!(
        bits(&bounce(&straight, 512, 0)) != bits(&base),
        "swing is audible"
    );
    // Without the choke group the open hat rings through the closed hat.
    let mut open = doc.clone();
    set(&mut open, &key("drums"), &format!("pad{CLOSED}.choke"), 0.0);
    assert!(
        bits(&bounce(&open, 512, 0)) != bits(&base),
        "the choke is audible"
    );
}

#[test]
fn the_demo_sounds_and_is_not_clipping() {
    let (_, doc) = registry_and_doc();
    let all = bounce(&doc, 512, 0);
    let peak = all
        .channels
        .iter()
        .flatten()
        .fold(0.0f32, |m, x| m.max(x.abs()));
    assert!((0.05..1.0).contains(&peak), "peak {peak}");
    assert!(rms(&all.channels[0], 0, 48_000) > 1e-2);
}

#[test]
fn a_drum_project_without_its_bundle_fails_naming_the_missing_audio() {
    let (registry, doc) = registry_and_doc();
    let e = render_doc_in(&doc, &registry, &ProjectOptions::default(), None).unwrap_err();
    assert!(e.to_string().contains("kick.wav"), "{e}");
}

#[test]
fn the_example_bounces_from_its_file_and_matches_the_document_bounce() {
    let (registry, doc) = registry_and_doc();
    let from_file = render_project_file(&demo_file(), &registry, &options(512, 0)).unwrap();
    assert!(bits(&from_file) == bits(&bounce(&doc, 512, 0)));
    assert!(from_file.warnings.is_empty(), "{:?}", from_file.warnings);
}
