//! The mix check: the plan reads the channels, the returns, the master and the feed of every
//! non-linear insert; a rendered song gives the same numbers at every block size; and the findings
//! come out of measurements as the thresholds say (built from hand-made measurements, so every
//! kind is covered without a render).

use atomik_core::PPQ;
use atomik_dsp::masking::{BANDS, BandMap, SILENT_DB, band_of};
use atomik_dsp::mixcheck::{Lows, PEAK_WINDOW, SignalStats, key_hz};
use atomik_model::song::ClipId;
use atomik_model::{
    Clip, ClipContent, Command, ConsoleCommand, CreateOptions, CreateSource, DeviceKey, Document,
    InsertEffect, Note, RackDoc, SocketRef, SongCommand, StemKind, StemSource, WorkflowCommand,
    new_project_doc,
};
use atomik_render::default_registry;
use atomik_render::export::ExportSpan;
use atomik_render::mixcheck::{
    Finding, Fix, Kind, Measured, Probe, Rendered, Tool, findings, plan, render_check,
};
use atomik_render::plugin_pass::RenderControl;
use atomik_render::project::{PluginBounce, ProjectOptions};
use std::sync::atomic::AtomicBool;

const BAR: i64 = 4 * PPQ;
const SYNTH: &str = "atomik.poly-synth";
const RATE: u32 = 48_000;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn apply(d: &mut Document, c: Command) {
    let registry = default_registry();
    d.apply(c.clone(), &registry)
        .unwrap_or_else(|e| panic!("{c:?}: {e}"));
}

fn clip(notes: &[(i64, i64, u8)]) -> Clip {
    Clip {
        id: ClipId(0),
        start: 0,
        length: 4 * BAR,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        param_lanes: Vec::new(),
        midi: Default::default(),
        groove: None,
        controllers: Vec::new(),
        name: None,
        color: None,
        grid: None,
        content: ClipContent::Notes(
            notes
                .iter()
                .map(|&(t, l, k)| Note::new(t, l, k, 0.9))
                .collect(),
        ),
    }
}

/// A synth named `Bass` on D1 (36.7 Hz) with a compressor on its insert chain, and a synth named
/// `Pad` an octave and a half higher.
fn song() -> RackDoc {
    let mut d = Document::new(new_project_doc());
    for (k, name, note) in [("bass", "Bass", 26u8), ("pad", "Pad", 52u8)] {
        apply(
            &mut d,
            Command::Workflow(WorkflowCommand::CreateInstrument {
                index: usize::MAX,
                source: CreateSource::Type(SYNTH.into()),
                key: Some(key(k)),
                options: CreateOptions::default(),
            }),
        );
        apply(
            &mut d,
            Command::Rename {
                key: key(k),
                name: name.to_owned(),
            },
        );
        for (p, v) in [
            ("osc1.wave", 0.0f32),
            ("env.amp.attack", 5.0),
            ("env.amp.sustain", 1.0),
            ("env.amp.release", 100.0),
        ] {
            apply(
                &mut d,
                Command::SetParam {
                    key: key(k),
                    param: p.into(),
                    value: v,
                    merge: None,
                },
            );
        }
        apply(
            &mut d,
            Command::Song(SongCommand::AddClip {
                device: key(k),
                lane: 0,
                clip: clip(&[(0, 2 * BAR, note)]),
            }),
        );
    }
    let channel = d.doc().console.channels[0].channel.clone();
    apply(
        &mut d,
        Command::Console(ConsoleCommand::SetInsert {
            channel,
            slot: 0,
            effect: Some(InsertEffect::New {
                type_id: "atomik.compressor".into(),
            }),
        }),
    );
    apply(&mut d, Command::Song(SongCommand::SetSongEnd(2 * BAR)));
    d.doc().clone()
}

fn options(block: usize) -> ProjectOptions {
    ProjectOptions {
        host_block: std::num::NonZeroUsize::new(block).unwrap(),
        workers: 0,
        loop_count: None,
    }
}

fn check(doc: &RackDoc, block: usize) -> Rendered {
    let registry = default_registry();
    let plan = plan(doc, &registry).expect("a plan");
    render_check(
        doc,
        &registry,
        (&options(block), None),
        &mut PluginBounce::default(),
        (&plan, ExportSpan::Song),
        RenderControl::default(),
    )
    .expect("renders")
    .expect("not stopped")
}

#[test]
fn the_plan_reads_the_channels_the_master_and_the_feed_of_the_compressor() {
    let doc = song();
    let registry = default_registry();
    let plan = plan(&doc, &registry).expect("a plan");
    let kinds: Vec<StemKind> = plan.sources.iter().map(|s| s.kind).collect();
    assert_eq!(
        kinds,
        vec![StemKind::Channel, StemKind::Channel, StemKind::Master]
    );
    assert_eq!(plan.sources[0].name, "Bass");
    assert_eq!(plan.probes.len(), 1);
    assert_eq!(plan.probes[0].type_id, "atomik.compressor");
    assert_eq!(plan.probes[0].strip, plan.sources[0].key);
}

#[test]
fn a_song_without_channels_has_no_plan() {
    let doc = Document::new(new_project_doc()).doc().clone();
    let registry = default_registry();
    assert!(plan(&doc, &registry).is_err());
}

#[test]
fn the_measurements_are_the_same_at_every_block_size() {
    let doc = song();
    let a = check(&doc, 512);
    let b = check(&doc, 64);
    assert_eq!(a.signals.len(), 3);
    assert_eq!(a.feeds.len(), 1);
    for (x, y) in a.signals.iter().zip(&b.signals) {
        assert_eq!(x.stats, y.stats, "{}", x.source.name);
        assert_eq!(x.map, y.map, "{}", x.source.name);
    }
    assert_eq!(a.feeds[0].1, b.feeds[0].1);
    let master = a.signals.last().expect("master");
    assert!(master.stats.peak > 0.01 && master.stats.rms_db > -80.0);
    assert!(a.feeds[0].1.rms_db > -80.0, "the feed has signal");
}

#[test]
fn a_bass_with_its_root_under_40_hz_is_found_and_the_level_finding_follows_the_feed() {
    let doc = song();
    let rendered = check(&doc, 512);
    let found = findings(&doc, &rendered);
    let root = found
        .iter()
        .find(|f| f.kind == Kind::BassRoot)
        .expect("D1 is under the range");
    assert_eq!(root.track, "Bass");
    assert!((root.value - key_hz(26)).abs() < 0.1);
    assert!(root.measured.contains("D1"), "{}", root.measured);
    assert_eq!(root.tool, Some(Tool::KickBassCheck));
    assert!(root.fix.is_none());
    // The compressor is fed what the instrument plays: a finding exactly when that is far from
    // the reference, and then the fix trims it to the reference.
    let rms = rendered.feeds[0].1.rms_db;
    let level = found.iter().find(|f| f.kind == Kind::Level);
    assert_eq!(
        level.is_some(),
        (rms - atomik_dsp::mixcheck::REFERENCE_RMS_DB).abs()
            > atomik_dsp::mixcheck::OFF_REFERENCE_DB
            && atomik_dsp::mixcheck::trim_for(rms) != 0.0,
        "feed {rms}"
    );
    if let Some(Finding {
        fix: Some(Fix::Trim { db, device, .. }),
        ..
    }) = level
    {
        assert_eq!(*device, rendered.feeds[0].0.device);
        let after = rms + db;
        assert!((after + 18.0).abs() <= 0.26, "{rms} + {db}");
    }
}

#[test]
fn a_stopped_render_returns_nothing() {
    let doc = song();
    let registry = default_registry();
    let stop = AtomicBool::new(true);
    let plan = plan(&doc, &registry).expect("a plan");
    let result = render_check(
        &doc,
        &registry,
        (&options(512), None),
        &mut PluginBounce::default(),
        (&plan, ExportSpan::Song),
        RenderControl {
            cancel: Some(&stop),
            progress: None,
        },
    )
    .expect("no error");
    assert!(result.is_none());
}

// ---------------------------------------------------------------------------- hand-made

fn source(kind: StemKind, name: &str) -> StemSource {
    let tap = |s: &str| SocketRef {
        device: key(name),
        socket: s.to_owned(),
    };
    StemSource {
        kind,
        key: key(name),
        name: name.to_owned(),
        tap: [tap("out_l"), tap("out_r")],
        dry: None,
    }
}

/// A flat spectrum at `level` dB with `bands` raised to `peak` dB, 4 blocks long.
fn map(level: f32, bands: &[(usize, f32)]) -> BandMap {
    let mut row = [level; BANDS];
    for &(b, v) in bands {
        row[b] = v;
    }
    BandMap {
        rate: RATE,
        levels: vec![row; 4],
    }
}

fn stats(peak: f32, rms_db: f32, peaks: Vec<f32>, lows: Lows) -> SignalStats {
    SignalStats {
        frames: peaks.len() * PEAK_WINDOW,
        peak,
        rms_db,
        peaks,
        lows,
    }
}

fn quiet() -> SignalStats {
    stats(0.1, -30.0, vec![0.1; 100], Lows::default())
}

fn measured(kind: StemKind, name: &str, stats: SignalStats, map: BandMap) -> Measured {
    Measured {
        source: source(kind, name),
        stats,
        map,
    }
}

fn empty_doc() -> RackDoc {
    Document::new(new_project_doc()).doc().clone()
}

fn kinds(found: &[Finding]) -> Vec<Kind> {
    found.iter().map(|f| f.kind).collect()
}

#[test]
fn a_balanced_quiet_mix_has_no_findings() {
    let rendered = Rendered {
        rate: RATE,
        signals: vec![
            measured(
                StemKind::Channel,
                "Lead",
                quiet(),
                map(-50.0, &[(20, -30.0)]),
            ),
            measured(
                StemKind::Master,
                "Master",
                quiet(),
                map(-50.0, &[(20, -30.0)]),
            ),
        ],
        feeds: Vec::new(),
    };
    assert!(findings(&empty_doc(), &rendered).is_empty());
}

#[test]
fn a_low_mid_pile_up_names_the_tracks_with_their_shares_and_fixes_the_top_channel() {
    // Bands 6..=10 are 160 Hz to 400 Hz. The master has them 20 dB over the rest.
    let hump = |peak: f32| -> Vec<(usize, f32)> { (6..=10).map(|b| (b, peak)).collect() };
    let rendered = Rendered {
        rate: RATE,
        signals: vec![
            measured(StemKind::Channel, "Keys", quiet(), map(-60.0, &hump(-30.0))),
            measured(StemKind::Channel, "Pad", quiet(), map(-60.0, &hump(-36.0))),
            measured(StemKind::Return, "Hall", quiet(), map(-60.0, &hump(-42.0))),
            measured(
                StemKind::Master,
                "Master",
                quiet(),
                map(-60.0, &hump(-28.0)),
            ),
        ],
        feeds: Vec::new(),
    };
    let found = findings(&empty_doc(), &rendered);
    assert_eq!(kinds(&found), vec![Kind::LowMid]);
    let f = &found[0];
    assert!(f.value > 40.0, "{}", f.value);
    assert!(f.track.starts_with("Keys 7"), "{}", f.track);
    assert!(
        f.track.contains("Hall"),
        "reverb returns are named: {}",
        f.track
    );
    assert_eq!(f.tool, Some(Tool::MaskingMap));
    let Some(Fix::CutLowMids { channel, hz }) = &f.fix else {
        panic!("a cut: {:?}", f.fix);
    };
    assert_eq!(*channel, key("Keys"));
    assert!((150.0..=500.0).contains(hz), "{hz}");
}

#[test]
fn a_feed_far_from_the_reference_gets_a_trim_to_the_reference() {
    let probe = Probe {
        strip: key("Lead"),
        strip_name: "Lead".to_owned(),
        device: key("comp"),
        device_name: "Compressor".to_owned(),
        type_id: "atomik.compressor".to_owned(),
    };
    let feed = |rms: f32| stats(0.5, rms, vec![0.5; 4], Lows::default());
    let rendered = Rendered {
        rate: RATE,
        signals: Vec::new(),
        feeds: vec![
            (probe.clone(), feed(-34.0)),
            (probe.clone(), feed(-20.0)),
            (probe, feed(-6.0)),
        ],
    };
    let found = findings(&empty_doc(), &rendered);
    assert_eq!(found.len(), 2, "-20 dB is close enough");
    let Some(Fix::Trim { db, .. }) = &found[0].fix else {
        panic!("a trim");
    };
    assert_eq!(*db, 16.0);
    let Some(Fix::Trim { db, .. }) = &found[1].fix else {
        panic!("a trim");
    };
    assert_eq!(*db, -12.0);
    assert_eq!(found[0].track, "Lead / Compressor");
}

#[test]
fn the_peaks_of_the_master_name_the_tracks_that_are_loudest_there() {
    let mut master = vec![0.2f32; 200];
    master[20] = 0.99;
    let mut kick = vec![0.05f32; 200];
    kick[20] = 0.8;
    let mut vox = vec![0.05f32; 200];
    vox[20] = 0.6;
    let mut pad = vec![0.05f32; 200];
    pad[20] = 0.05;
    let rendered = Rendered {
        rate: RATE,
        signals: vec![
            measured(
                StemKind::Channel,
                "Kick",
                stats(0.8, -12.0, kick, Lows::default()),
                map(-60.0, &[]),
            ),
            measured(
                StemKind::Channel,
                "Vox",
                stats(0.6, -12.0, vox, Lows::default()),
                map(-60.0, &[]),
            ),
            measured(
                StemKind::Channel,
                "Pad",
                stats(0.05, -12.0, pad, Lows::default()),
                map(-60.0, &[]),
            ),
            measured(
                StemKind::Master,
                "Master",
                stats(0.99, -12.0, master, Lows::default()),
                map(-60.0, &[]),
            ),
        ],
        feeds: Vec::new(),
    };
    let found = findings(&empty_doc(), &rendered);
    let peaks: Vec<&Finding> = found.iter().filter(|f| f.kind == Kind::Peaks).collect();
    assert!(!peaks.is_empty());
    let top = peaks[0];
    assert!((top.value + 0.09).abs() < 0.05, "{}", top.value);
    assert_eq!(top.track, "Kick, Vox", "the quiet pad is not named");
    let at = top.at.expect("a time");
    assert!((at - 20.0 * PEAK_WINDOW as f64 / f64::from(RATE)).abs() < 1e-9);
    assert_eq!(top.tool, Some(Tool::Clipper));
    assert!(!top.tool.expect("tool").available());
    // A master with headroom has no peak finding.
    let calm = Rendered {
        rate: RATE,
        signals: vec![measured(
            StemKind::Master,
            "Master",
            stats(0.3, -20.0, vec![0.3; 10], Lows::default()),
            map(-60.0, &[]),
        )],
        feeds: Vec::new(),
    };
    assert!(findings(&empty_doc(), &calm).is_empty());
}

#[test]
fn a_low_end_that_cancels_in_mono_names_the_track_that_loses_it() {
    let lost = Lows {
        stereo: 0.01,
        mono: 0.001,
    };
    let fine = Lows {
        stereo: 0.01,
        mono: 0.0099,
    };
    let rendered = Rendered {
        rate: RATE,
        signals: vec![
            measured(
                StemKind::Channel,
                "Sub",
                stats(0.2, -20.0, vec![0.2; 4], lost),
                map(-60.0, &[]),
            ),
            measured(
                StemKind::Channel,
                "Kick",
                stats(0.2, -20.0, vec![0.2; 4], fine),
                map(-60.0, &[]),
            ),
            measured(
                StemKind::Master,
                "Master",
                stats(
                    0.2,
                    -20.0,
                    vec![0.2; 4],
                    Lows {
                        stereo: 0.02,
                        mono: 0.005,
                    },
                ),
                map(-60.0, &[]),
            ),
        ],
        feeds: Vec::new(),
    };
    let found = findings(&empty_doc(), &rendered);
    assert_eq!(kinds(&found), vec![Kind::Mono]);
    let f = &found[0];
    assert!(
        (f.value - 2.64).abs() < 0.1 || f.value >= 3.0,
        "{}",
        f.value
    );
}

#[test]
fn a_kick_whose_body_is_in_its_harmonics_has_a_weak_fundamental_and_a_good_kick_does_not() {
    let low = band_of(55.0);
    let harmonic = band_of(110.0);
    let weak = measured(
        StemKind::Channel,
        "Kick",
        quiet(),
        map(SILENT_DB, &[(low, -30.0), (harmonic, -12.0)]),
    );
    let good = measured(
        StemKind::Channel,
        "Kick 2",
        quiet(),
        map(SILENT_DB, &[(low, -10.0), (harmonic, -26.0)]),
    );
    let rendered = Rendered {
        rate: RATE,
        signals: vec![weak, good],
        feeds: Vec::new(),
    };
    let found = findings(&empty_doc(), &rendered);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].kind, Kind::WeakFundamental);
    assert_eq!(found[0].track, "Kick");
    assert!(found[0].value > 6.0);
    assert!(found[0].measured.contains("harmonic"));
}
