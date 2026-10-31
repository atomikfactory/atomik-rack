//! The `fx-demo` example project and its bundle (bounce part).
//!
//! `examples/projects/fx-demo/` is a bundle folder: `fx-demo.atomik` and `Audio/` with four short
//! **procedurally generated** drum sounds ([`demo_samples`]; no third-party audio). An 8-bar
//! song in A minor (Am F C G twice) at 120 BPM uses all six effects the way a mix would:
//!
//! * **Drums:** a drum machine with the kick on its individual output, on its own console
//!   channel "Kick" (four on the floor); clap and hats go through the machine's own channel.
//!   The clap and hats send to the reverb.
//! * **Bass** (poly-synth saw, one voice): its channel's insert holds a **compressor** with
//!   **sidechain from the Kick channel** (the cables the sidechain helper makes: the Kick
//!   channel's `insert_send` into `sc_in_l`/`sc_in_r` and `sc_enable` on) and 3 ms of lookahead:
//!   classic ducking, with the plug-in delay compensated by PDC.
//! * **Pads** (poly-synth, slow attack): a **modulation** effect as a chorus on the insert, and a
//!   send to the reverb.
//! * **Lead** (poly-synth square, one voice): a **distortion** (soft clip, 4x oversampled) then an
//!   **EQ** (low cut, presence) in series on the insert, and a send to the reverb.
//! * **Reverb** (hall) on FX return 1.
//! * **Mastering** on the master insert (multiband, width, true-peak limiter).
//!
//! Regenerate the bundle and the project after an intentional change with:
//!
//! ```text
//! ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-render --test fx_demo regenerate
//! ```
//!
//! The generators use only IEEE `+ - * /` and comparisons (no libm calls), so they give the same
//! samples on every platform; the committed WAVs are checked against them bit for bit.

use atomik_audio_file::{AudioData, Bundle, content_hash};
use atomik_core::PPQ;
use atomik_model::{
    AssetMeta, Cable, Clip, ClipContent, ClipId, Command, ConsoleCommand, DeviceKey, Document,
    Note, RackDoc, SocketRef, SongCommand, StepGrid, StepResolution, empty_doc, load_project,
    pad_key, save_project, with_pad_files,
};
use atomik_render::bundle::{existing_bundle_root, missing_assets};
use atomik_render::default_registry;
use atomik_render::project::{ProjectOptions, render_doc_in};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";
const MAX_BUNDLE_BYTES: u64 = 1024 * 1024;
const BAR: i64 = 4 * PPQ;
const BARS: i64 = 8;
const STEP: i64 = PPQ / 4;
const KICK: u8 = 1;
const CLAP: u8 = 4;
const CLOSED: u8 = 7;
const OPEN: u8 = 11;

fn demo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/projects/fx-demo")
}

fn demo_file() -> PathBuf {
    demo_root().join("fx-demo.atomik")
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
                let hz = 45.0 + 120.0 * sweep;
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

/// The four demo sounds as `(file name, audio)`: kick, closed hat, open hat, clap.
pub fn demo_samples() -> Vec<(&'static str, AudioData)> {
    vec![
        ("kick.wav", kick(14_400)),
        ("hat_closed.wav", hat(2_880, 0.9985, 0x1234_5679)),
        ("hat_open.wav", hat(14_400, 0.9998, 0x0bad_cafe)),
        ("clap.wav", clap(7_200)),
    ]
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn sref(d: &DeviceKey, s: &str) -> SocketRef {
    SocketRef {
        device: d.clone(),
        socket: s.to_string(),
    }
}

/// The chord roots and tones per bar (Am F C G).
const CHORDS: [[u8; 3]; 4] = [[57, 60, 64], [53, 57, 60], [55, 60, 64], [55, 59, 62]];
/// The bass root per bar.
const ROOTS: [u8; 4] = [45, 41, 48, 43];
/// The lead per bar: (start in 1/8 notes, key, length in 1/8 notes).
const LEAD: [&[(i64, u8, i64)]; 4] = [
    &[(0, 76, 3), (4, 72, 2), (6, 74, 2)],
    &[(0, 77, 3), (4, 76, 2), (6, 72, 2)],
    &[(0, 79, 3), (4, 76, 2), (6, 74, 2)],
    &[(0, 74, 4), (4, 71, 4)],
];
/// The last bar of the second round of the lead.
const LEAD_END: &[(i64, u8, i64)] = &[(0, 79, 2), (2, 76, 2), (4, 74, 2), (6, 71, 2)];

struct Builder {
    doc: Document,
}

impl Builder {
    fn apply(&mut self, c: Command) {
        let registry = default_registry();
        self.doc
            .apply(c.clone(), &registry)
            .unwrap_or_else(|e| panic!("{c:?}: {e}"));
    }

    fn set(&mut self, k: &DeviceKey, p: &str, v: f32) {
        self.apply(Command::SetParam {
            key: k.clone(),
            param: p.into(),
            value: v,
            merge: None,
        });
    }

    fn add(&mut self, type_id: &str, k: &DeviceKey, name: &str) {
        self.apply(Command::AddDevice {
            index: usize::MAX,
            type_id: type_id.into(),
            key: Some(k.clone()),
        });
        self.apply(Command::Rename {
            key: k.clone(),
            name: name.into(),
        });
    }

    fn channel(&self, source: &str) -> DeviceKey {
        self.doc
            .doc()
            .console
            .channel_of(&key(source))
            .expect("channel")
            .channel
            .clone()
    }

    fn connect(&mut self, from: (&DeviceKey, &str), to: (&DeviceKey, &str)) {
        self.apply(Command::Connect(Cable {
            from: sref(from.0, from.1),
            to: sref(to.0, to.1),
        }));
    }

    /// Cables `host`'s insert (send ... return) through `chain` in series, both sides.
    fn insert_chain(&mut self, host: &DeviceKey, chain: &[&DeviceKey]) {
        for side in ["l", "r"] {
            let mut prev = (host, format!("insert_send_{side}"));
            for fx in chain {
                self.connect((prev.0, &prev.1), (fx, &format!("in_{side}")));
                prev = (fx, format!("out_{side}"));
            }
            self.connect((prev.0, &prev.1), (host, &format!("insert_return_{side}")));
        }
    }

    fn synth(&mut self, k: &str, name: &str, params: &[(&str, f32)]) {
        let dk = key(k);
        self.add("atomik.poly-synth", &dk, name);
        for (p, v) in params {
            self.set(&dk, p, *v);
        }
    }

    fn song_clip(&mut self, device: &str, start: i64, length: i64, notes: Vec<Note>) {
        self.apply(Command::Song(SongCommand::AddClip {
            device: key(device),
            lane: 0,
            clip: Clip {
                id: ClipId(0),
                start,
                length,
                offset: 0,
                loop_length: None,
                muted: false,
                scale: None,
                controllers: Vec::new(),
                grid: None,
                content: ClipContent::Notes(notes.into()),
            },
        }));
    }
}

fn note(tick: i64, length: i64, key: u8, velocity: f32) -> Note {
    Note::new(tick, length, key, velocity)
}

/// The project for sounds whose hashes and formats are `pool` (in [`demo_samples`] order).
pub fn demo_doc(pool: &[(String, AssetMeta)]) -> RackDoc {
    let registry = default_registry();
    let mut b = Builder {
        doc: Document::new(empty_doc()),
    };
    for (hash, meta) in pool {
        b.apply(Command::RegisterAsset {
            hash: hash.clone(),
            meta: meta.clone(),
        });
    }
    let _ = &registry;

    // Instruments (each gets its automatic console channel).
    b.add("atomik.drum-machine", &key("drums"), "Drums");
    b.set(&key("drums"), "master.level", 0.0);
    let mut zones = Vec::new();
    for (pad, index, level, decay) in [
        (KICK, 0usize, -1.0f32, 450.0f32),
        (CLOSED, 1, -12.0, 90.0),
        (OPEN, 2, -12.0, 500.0),
        (CLAP, 3, -7.0, 250.0),
    ] {
        let (hash, meta) = &pool[index];
        let file = (
            hash.clone(),
            demo_samples()[index].0.to_string(),
            meta.frames,
        );
        zones = with_pad_files(&zones, pad, &[file]);
        b.set(&key("drums"), &format!("pad{pad}.level"), level);
        b.set(&key("drums"), &format!("pad{pad}.decay"), decay);
    }
    b.set(&key("drums"), &format!("pad{CLOSED}.choke"), 1.0);
    b.set(&key("drums"), &format!("pad{OPEN}.choke"), 1.0);
    b.apply(Command::SetZones {
        device: key("drums"),
        zones,
        merge: None,
    });
    b.synth(
        "bass",
        "Bass",
        &[
            ("osc1.wave", 0.0),
            ("osc.mix", 0.0),
            ("voices", 1.0),
            ("filter.cutoff", 700.0),
            ("filter.resonance", 0.15),
            ("filter.env_amount", 0.2),
            ("env.amp.attack", 3.0),
            ("env.amp.decay", 200.0),
            ("env.amp.sustain", 0.85),
            ("env.amp.release", 60.0),
            ("master.level", -6.0),
        ],
    );
    b.synth(
        "pads",
        "Pads",
        &[
            ("osc1.wave", 0.0),
            ("osc2.wave", 2.0),
            ("osc.mix", 0.5),
            ("filter.cutoff", 1800.0),
            ("filter.env_amount", 0.0),
            ("env.amp.attack", 450.0),
            ("env.amp.decay", 600.0),
            ("env.amp.sustain", 0.8),
            ("env.amp.release", 700.0),
            ("master.level", -14.0),
        ],
    );
    b.synth(
        "lead",
        "Lead",
        &[
            ("osc1.wave", 1.0),
            ("osc.mix", 0.0),
            ("voices", 1.0),
            ("filter.cutoff", 3500.0),
            ("filter.env_amount", 0.2),
            ("env.amp.attack", 4.0),
            ("env.amp.decay", 250.0),
            ("env.amp.sustain", 0.6),
            ("env.amp.release", 160.0),
            ("master.level", -16.0),
        ],
    );
    // The kick's individual output goes to its own channel: the sidechain source.
    b.apply(Command::Console(ConsoleCommand::AddChannelFor {
        source: key("drums"),
        out_l: format!("pad{KICK}_out_l"),
        out_r: format!("pad{KICK}_out_r"),
        name: Some("Kick".into()),
    }));
    let kick_ch = b
        .doc
        .doc()
        .console
        .channels
        .last()
        .expect("the kick channel")
        .channel
        .clone();
    let (drums_ch, bass_ch, pads_ch, lead_ch) = (
        b.channel("drums"),
        b.channel("bass"),
        b.channel("pads"),
        b.channel("lead"),
    );
    for (ch, name, pan) in [
        (&drums_ch, "Drums", 0.0f32),
        (&bass_ch, "Bass", 0.0),
        (&pads_ch, "Pads", -0.2),
        (&lead_ch, "Lead", 0.25),
    ] {
        b.apply(Command::Rename {
            key: ch.clone(),
            name: name.into(),
        });
        b.set(ch, "pan", pan);
    }

    // Effects.
    for (type_id, k, name) in [
        ("atomik.compressor", "bass-comp", "Bass Comp"),
        ("atomik.modulation", "pad-chorus", "Pad Chorus"),
        ("atomik.distortion", "lead-dist", "Lead Drive"),
        ("atomik.eq", "lead-eq", "Lead EQ"),
        ("atomik.reverb", "hall", "Hall"),
        ("atomik.mastering", "master-fx", "Mastering"),
    ] {
        b.add(type_id, &key(k), name);
    }
    let (comp, chorus, dist, eq, hall, mastering) = (
        key("bass-comp"),
        key("pad-chorus"),
        key("lead-dist"),
        key("lead-eq"),
        key("hall"),
        key("master-fx"),
    );
    for (p, v) in [
        ("threshold", -34.0),
        ("ratio", 8.0),
        ("knee", 4.0),
        ("attack", 1.5),
        ("release", 140.0),
        ("lookahead", 3.0),
        ("makeup", 3.0),
    ] {
        b.set(&comp, p, v);
    }
    for (p, v) in [
        ("mode", 0.0),
        ("rate", 0.35),
        ("depth", 0.6),
        ("delay", 12.0),
        ("spread", 120.0),
        ("mix", 0.5),
    ] {
        b.set(&chorus, p, v);
    }
    for (p, v) in [
        ("curve", 0.0),
        ("drive", 14.0),
        ("tone", -0.2),
        ("output", -6.0),
        ("mix", 0.8),
        ("oversample", 2.0),
    ] {
        b.set(&dist, p, v);
    }
    for (p, v) in [
        ("band1.on", 1.0),
        ("band1.freq", 180.0),
        ("band4.gain", 3.0),
        ("band4.freq", 2800.0),
        ("band4.q", 1.2),
        ("band6.on", 1.0),
        ("band6.freq", 9000.0),
    ] {
        b.set(&eq, p, v);
    }
    for (p, v) in [
        ("algorithm", 1.0),
        ("predelay", 28.0),
        ("decay", 2.8),
        ("damping", 0.5),
        ("mix", 1.0),
        ("output", -3.0),
    ] {
        b.set(&hall, p, v);
    }
    for (p, v) in [("ceiling", -1.0), ("width", 1.1), ("output", 0.0)] {
        b.set(&mastering, p, v);
    }

    // Insert chains, the sidechain (what the helper's batch does), the return and the master.
    b.insert_chain(&bass_ch, &[&comp]);
    b.insert_chain(&pads_ch, &[&chorus]);
    b.insert_chain(&lead_ch, &[&dist, &eq]);
    for side in ["l", "r"] {
        b.connect(
            (&kick_ch, &format!("insert_send_{side}")),
            (&comp, &format!("sc_in_{side}")),
        );
    }
    b.set(&comp, "sc_enable", 1.0);
    b.apply(Command::Console(ConsoleCommand::SetReturn {
        slot: 0,
        effect: Some(hall.clone()),
    }));
    let master = b.doc.doc().console.master.clone().expect("master");
    b.insert_chain(&master, &[&mastering]);
    for (ch, level) in [(&drums_ch, -14.0f32), (&pads_ch, -7.0), (&lead_ch, -8.0)] {
        b.set(ch, "send1.on", 1.0);
        b.set(ch, "send1.level", level);
    }

    // The song.
    for d in ["drums", "bass", "pads", "lead"] {
        b.apply(Command::Song(SongCommand::AddTrack { device: key(d) }));
    }
    for bar in 0..BARS {
        let start = bar * BAR;
        let chord = (bar % 4) as usize;
        // Drums: a 16-step bar per clip.
        let mut notes = Vec::new();
        for step in 0..16i64 {
            let t = step * STEP;
            if step % 4 == 0 {
                notes.push(note(t, STEP, pad_key(KICK), 1.0));
            }
            if step == 4 || step == 12 {
                notes.push(note(t, STEP, pad_key(CLAP), 0.7));
            }
            if step == 14 && bar % 2 == 1 {
                notes.push(note(t, STEP, pad_key(OPEN), 0.8));
            } else if step % 2 == 0 {
                notes.push(note(
                    t,
                    STEP,
                    pad_key(CLOSED),
                    if step % 4 == 2 { 0.8 } else { 0.5 },
                ));
            }
        }
        notes.sort_by_key(|n| (n.tick, n.key));
        b.apply(Command::Song(SongCommand::AddClip {
            device: key("drums"),
            lane: 0,
            clip: Clip {
                id: ClipId(0),
                start,
                length: BAR,
                offset: 0,
                loop_length: None,
                muted: false,
                scale: None,
                controllers: Vec::new(),
                grid: Some(StepGrid {
                    resolution: StepResolution::Sixteenth,
                    steps: 16,
                    swing: 0,
                }),
                content: ClipContent::Notes(notes.into()),
            },
        }));
        // Bass: eighth notes on the root with octave jumps.
        let root = ROOTS[chord];
        let pattern = [0u8, 0, 12, 0, 0, 12, 0, 7];
        let notes = (0..8i64)
            .map(|i| {
                note(
                    i * PPQ / 2,
                    PPQ / 2 - PPQ / 8,
                    root + pattern[i as usize],
                    if i % 2 == 0 { 0.95 } else { 0.7 },
                )
            })
            .collect();
        b.song_clip("bass", start, BAR, notes);
        // Pads: the chord for the whole bar.
        let notes = CHORDS[chord]
            .iter()
            .map(|k| note(0, BAR - PPQ / 4, *k, 0.7))
            .collect();
        b.song_clip("pads", start, BAR, notes);
        // Lead: two rounds, the second ends differently.
        let phrase = if bar == BARS - 1 {
            LEAD_END
        } else {
            LEAD[chord]
        };
        let notes = phrase
            .iter()
            .map(|(s, k, l)| note(s * PPQ / 2, l * PPQ / 2 - PPQ / 8, *k, 0.85))
            .collect();
        b.song_clip("lead", start, BAR, notes);
    }
    b.apply(Command::Song(SongCommand::SetSongEnd(BARS * BAR)));
    b.apply(Command::Song(SongCommand::SetLoop {
        range: (0, BARS * BAR),
        enabled: false,
    }));
    b.doc.doc().clone()
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
    assert_eq!(a.len(), 4);
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
        "the project is the generator's output (see the module docs)"
    );
    let root = existing_bundle_root(&demo_file()).expect("its own folder is the bundle");
    assert_eq!(root, demo_root());
    let bundle = Bundle::open(&root).expect("bundle");
    assert!(missing_assets(&doc, Some(&bundle)).is_empty());
    assert_eq!(doc.assets.len(), 4);
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
    assert_eq!(names, ["Audio", "fx-demo.atomik"]);
}

#[test]
fn the_demo_uses_all_six_effects_where_a_mix_would_put_them() {
    let (_, doc) = registry_and_doc();
    let type_of = |k: &str| doc.device(&key(k)).unwrap().type_id.clone();
    let types: std::collections::BTreeSet<String> = doc
        .devices
        .iter()
        .map(|d| d.type_id.clone())
        .filter(|t| {
            [
                "atomik.reverb",
                "atomik.compressor",
                "atomik.eq",
                "atomik.distortion",
                "atomik.modulation",
                "atomik.mastering",
            ]
            .contains(&t.as_str())
        })
        .collect();
    assert_eq!(types.len(), 6, "{types:?}");
    assert_eq!(doc.console.returns[0], Some(key("hall")));
    assert_eq!(type_of("hall"), "atomik.reverb");
    let master = doc.console.master.clone().unwrap();
    let cabled = |from: &DeviceKey, fs: &str, to: &str, ts: &str| {
        doc.cables.iter().any(|c| {
            c.from.device == *from
                && c.from.socket == fs
                && c.to.device.as_str() == to
                && c.to.socket == ts
        })
    };
    assert!(cabled(&master, "insert_send_l", "master-fx", "in_l"));
    // The kick channel keys the bass compressor, which has the sidechain on and lookahead.
    let kick_ch = doc.console.channels.last().unwrap().channel.clone();
    assert!(cabled(&kick_ch, "insert_send_l", "bass-comp", "sc_in_l"));
    assert!(cabled(&kick_ch, "insert_send_r", "bass-comp", "sc_in_r"));
    let comp = doc.device(&key("bass-comp")).unwrap();
    assert_eq!(comp.params.get("sc_enable"), Some(&1.0));
    assert!(comp.params.get("lookahead").is_some_and(|v| *v > 0.0));
    assert!(cabled(
        &doc.console.channel_of(&key("lead")).unwrap().channel,
        "insert_send_l",
        "lead-dist",
        "in_l"
    ));
    assert!(cabled(&key("lead-dist"), "out_l", "lead-eq", "in_l"));
    assert_eq!(doc.song.tracks.len(), 4);
    assert_eq!(doc.song.end, BARS * BAR);
}

fn options(block: usize, workers: u16) -> ProjectOptions {
    ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    }
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

fn bits(r: &atomik_render::Rendered) -> Vec<u32> {
    r.channels.iter().flatten().map(|x| x.to_bits()).collect()
}

fn rms(channel: &[f32], from: usize, to: usize) -> f64 {
    let w = &channel[from..to.min(channel.len())];
    (w.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / w.len().max(1) as f64).sqrt()
}

#[test]
fn the_demo_sounds_stays_finite_and_the_limiter_holds_the_ceiling() {
    let (_, doc) = registry_and_doc();
    let all = bounce(&doc, 512, 0);
    assert!(all.channels.iter().flatten().all(|x| x.is_finite()));
    let peak = all
        .channels
        .iter()
        .flatten()
        .fold(0.0f32, |m, x| m.max(x.abs()));
    // -1 dBTP ceiling: the sample peak is at or below it (0.891).
    assert!(
        (0.1..=0.892).contains(&peak),
        "peak {peak}: loud enough and under the ceiling"
    );
    let seconds = all.channels[0].len() / 48_000;
    assert!(seconds >= 16, "{seconds} s");
    for second in 1..14 {
        assert!(
            rms(&all.channels[0], second * 48_000, (second + 1) * 48_000) > 0.01,
            "second {second} sounds"
        );
    }
}

/// The bounce with all six effects (inserts on channels, the
/// reverb on a return, mastering on the master, a sidechain and lookahead PDC) is bit-identical
/// across host blocks and worker counts.
#[test]
fn the_fx_bounce_is_bit_identical_across_host_blocks_and_worker_counts() {
    let (_, doc) = registry_and_doc();
    let reference = bits(&bounce(&doc, 512, 0));
    assert!(reference.len() > 1_000_000);
    for block in [64, 333, 512, 4096] {
        for workers in 0..=4 {
            assert!(
                bits(&bounce(&doc, block, workers)) == reference,
                "block {block}, {workers} workers"
            );
        }
    }
}

fn with_param(doc: &RackDoc, k: &DeviceKey, p: &str, v: f32) -> RackDoc {
    let mut out = doc.clone();
    out.devices
        .iter_mut()
        .find(|d| &d.key == k)
        .unwrap()
        .params
        .insert(p.into(), v);
    out
}

/// In the project: only the bass is heard (the other channels muted; the muted Kick
/// channel's insert send still keys the compressor). Its level in the 40 ms after each kick is
/// far below the same windows with the compressor out of the path (`mix` 0).
#[test]
fn the_kick_ducks_the_bass_in_the_demo() {
    let (_, doc) = registry_and_doc();
    let mut bass_only = doc.clone();
    for c in &doc.console.channels {
        let name = &doc.device(&c.channel).unwrap().name;
        if name != "Bass" {
            bass_only = with_param(&bass_only, &c.channel, "mute", 1.0);
        }
    }
    bass_only = with_param(&bass_only, &key("hall"), "mix", 0.0);
    let ducked = bounce(&bass_only, 512, 0);
    let plain = bounce(
        &with_param(&bass_only, &key("bass-comp"), "mix", 0.0),
        512,
        0,
    );
    let mut worst = 0.0f64;
    for beat in [9usize, 10, 11, 13, 14] {
        let at = beat * 24_000 + 480;
        let (d, p) = (
            rms(&ducked.channels[0], at, at + 1_920),
            rms(&plain.channels[0], at, at + 1_920),
        );
        assert!(p > 1e-3, "beat {beat}: the bass sounds ({p})");
        worst = worst.max(d / p);
    }
    eprintln!(
        "fx-demo: bass RMS in the 40 ms after a kick, ducked vs plain: worst {:.1} dB",
        20.0 * worst.log10()
    );
    assert!(worst < 0.6, "ducked to {worst:.3} of the plain bass");
}
