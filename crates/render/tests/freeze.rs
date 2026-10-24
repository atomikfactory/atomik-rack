//! Freeze and bounce in place through the offline render: the render of a track's chain is a
//! slice of the project bounce, the project bounced with the track frozen equals the unfrozen
//! bounce (bit for bit for a chain without tail and latency, within a tolerance with one),
//! frozen devices are not built, and a long render can be watched and stopped.

use atomik_audio_file::Bundle;
use atomik_core::PPQ;
use atomik_model::song::ClipId;
use atomik_model::{
    AssetMeta, AudioClipContent, Clip, ClipContent, Command, ConsoleCommand, CreateOptions,
    CreateSource, DeviceKey, Document, Frozen, InsertEffect, Note, RackDoc, SongCommand,
    WorkflowCommand, clips_span, insert_chain, new_project_doc, track_chain,
};
use atomik_render::freeze::{FreezeRender, FreezeRequest, render_freeze};
use atomik_render::plugin_pass::RenderControl;
use atomik_render::project::{PluginBounce, ProjectOptions, render_doc_in};
use atomik_render::{Rendered, default_registry};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

const BAR: i64 = 4 * PPQ;
const SYNTH: &str = "atomik.poly-synth";

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-render-freeze-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn options(block: usize, workers: u16) -> ProjectOptions {
    ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    }
}

fn apply(d: &mut Document, c: Command) {
    let registry = default_registry();
    d.apply(c.clone(), &registry)
        .unwrap_or_else(|e| panic!("{c:?}: {e}"));
}

fn notes_clip(start: i64, length: i64, notes: &[(i64, i64, u8)]) -> Clip {
    Clip {
        id: ClipId(0),
        start,
        length,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        controllers: Vec::new(),
        grid: None,
        content: ClipContent::Notes(
            notes
                .iter()
                .map(|&(t, l, k)| Note::new(t, l, k, 0.8))
                .collect(),
        ),
    }
}

/// Two synths with channels on the starter master. `lead` plays a short phrase in bar 1 and 2 (its
/// clip is four bars long, so there is a silent end), `bass` plays under it. `fx` insert effects
/// (a reverb, then a delay) sit on lead's channel.
fn project(fx: usize) -> Document {
    let mut d = Document::new(new_project_doc());
    for (k, wave, cutoff) in [("lead", 1.0, 3000.0), ("bass", 0.0, 700.0)] {
        apply(
            &mut d,
            Command::Workflow(WorkflowCommand::CreateInstrument {
                index: usize::MAX,
                source: CreateSource::Type(SYNTH.into()),
                key: Some(key(k)),
                options: CreateOptions::default(),
            }),
        );
        for (p, v) in [
            ("osc1.wave", wave),
            ("filter.cutoff", cutoff),
            ("env.amp.release", 150.0),
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
            Command::Song(SongCommand::SetTrackFlags {
                device: key(k),
                muted: false,
                solo: false,
                armed: false,
            }),
        );
    }
    apply(
        &mut d,
        Command::Song(SongCommand::AddClip {
            device: key("lead"),
            lane: 0,
            clip: notes_clip(
                0,
                4 * BAR,
                &[(0, PPQ, 64), (PPQ, PPQ, 67), (BAR, 2 * PPQ, 71)],
            ),
        }),
    );
    apply(
        &mut d,
        Command::Song(SongCommand::AddClip {
            device: key("bass"),
            lane: 0,
            clip: notes_clip(0, 4 * BAR, &[(0, 2 * PPQ, 40), (2 * PPQ, 2 * PPQ, 43)]),
        }),
    );
    apply(&mut d, Command::Song(SongCommand::SetSongEnd(5 * BAR)));
    let channel = d
        .doc()
        .console
        .channels
        .iter()
        .find(|c| c.source.as_ref() == Some(&key("lead")))
        .unwrap()
        .channel
        .clone();
    for (slot, t) in ["atomik.reverb", "atomik.stereo-delay"]
        .into_iter()
        .take(fx)
        .enumerate()
    {
        apply(
            &mut d,
            Command::Console(ConsoleCommand::SetInsert {
                channel: channel.clone(),
                slot,
                effect: Some(InsertEffect::New { type_id: t.into() }),
            }),
        );
    }
    d
}

fn request(doc: &RackDoc, effects: usize, tail_seconds: f64) -> FreezeRequest {
    FreezeRequest {
        device: key("lead"),
        effects,
        clips: None,
        span: clips_span(doc.song.track(&key("lead")).unwrap()).unwrap(),
        tail_seconds,
    }
}

fn freeze_render(
    doc: &RackDoc,
    request: &FreezeRequest,
    (block, workers): (usize, u16),
    control: RenderControl<'_>,
) -> Option<FreezeRender> {
    render_freeze(
        doc,
        &default_registry(),
        (&options(block, workers), None),
        &mut PluginBounce::default(),
        request,
        control,
    )
    .unwrap()
}

fn render_now(doc: &RackDoc, request: &FreezeRequest) -> FreezeRender {
    freeze_render(doc, request, (512, 0), RenderControl::default()).unwrap()
}

/// Stores `render` in a bundle under a temp dir and returns the document with it frozen.
fn frozen_doc(
    doc: &RackDoc,
    render: &FreezeRender,
    effects: &[DeviceKey],
    tag: &str,
) -> (RackDoc, Bundle, TempDir) {
    let dir = TempDir::new(tag);
    let mut bundle = Bundle::create(&dir.0.join("song")).unwrap();
    let info = bundle.add_recording(&render.audio, "freeze.wav").unwrap();
    let registry = default_registry();
    let mut d = Document::new(doc.clone());
    d.apply(
        Command::RegisterAsset {
            hash: info.hash.clone(),
            meta: AssetMeta {
                channels: info.channels,
                sample_rate: info.sample_rate,
                frames: info.frames,
                original_name: info.original_name.clone(),
            },
        },
        &registry,
    )
    .unwrap();
    d.apply(
        Command::Song(SongCommand::FreezeTrack {
            device: key("lead"),
            frozen: Frozen {
                start: render.start,
                length: render.length,
                audio: AudioClipContent::new(info.hash, 0),
                effects: effects.to_vec(),
            },
        }),
        &registry,
    )
    .unwrap();
    (d.doc().clone(), bundle, dir)
}

fn bounce(doc: &RackDoc, bundle: Option<&Bundle>) -> Rendered {
    render_doc_in(
        doc,
        &default_registry(),
        &options(512, 0),
        bundle.map(Bundle::root),
    )
    .unwrap()
}

fn chain(doc: &RackDoc) -> Vec<DeviceKey> {
    track_chain(doc, &default_registry(), &key("lead"))
        .unwrap()
        .effects
}

fn peak(r: &Rendered) -> f32 {
    r.channels
        .iter()
        .flatten()
        .fold(0.0f32, |m, x| m.max(x.abs()))
}

fn max_diff(a: &Rendered, b: &Rendered) -> f32 {
    assert_eq!(a.channels.len(), b.channels.len());
    a.channels
        .iter()
        .zip(&b.channels)
        .map(|(x, y)| {
            assert_eq!(x.len(), y.len(), "same length");
            x.iter()
                .zip(y)
                .fold(0.0f32, |m, (p, q)| m.max((p - q).abs()))
        })
        .fold(0.0f32, f32::max)
}

#[test]
fn a_frozen_instrument_bounces_bit_for_bit_like_the_unfrozen_project() {
    let doc = project(0).doc().clone();
    let render = render_now(&doc, &request(&doc, 0, 0.0));
    assert_eq!(render.audio.channels.len(), 2);
    assert_eq!(render.start, 0);
    assert_eq!(render.tail_frames, 0);
    // The render is the span: four bars at 120 BPM and 48 kHz.
    assert_eq!(render.audio.frames(), 4 * 2 * 48_000);
    assert!(render.audio.channels[0].iter().any(|x| x.abs() > 0.01));
    assert!(render.length >= 4 * BAR);

    let (frozen, bundle, _dir) = frozen_doc(&doc, &render, &[], "bit");
    let plain = bounce(&doc, None);
    let with_freeze = bounce(&frozen, Some(&bundle));
    assert!(peak(&plain) > 0.01, "the project makes sound");
    assert_eq!(
        plain.channels, with_freeze.channels,
        "frozen == unfrozen, sample for sample"
    );
}

#[test]
fn the_render_does_not_depend_on_the_block_size_or_the_workers() {
    let doc = project(1).doc().clone();
    let req = request(&doc, 1, 2.0);
    let a = freeze_render(&doc, &req, (64, 0), RenderControl::default()).unwrap();
    let b = freeze_render(&doc, &req, (1000, 2), RenderControl::default()).unwrap();
    let c = freeze_render(&doc, &req, (512, 0), RenderControl::default()).unwrap();
    assert_eq!(a.audio, b.audio);
    assert_eq!(a.audio, c.audio);
    assert_eq!(
        (a.start, a.length, a.tail_frames),
        (c.start, c.length, c.tail_frames)
    );
}

#[test]
fn a_tail_is_kept_while_it_is_audible_and_a_dry_track_has_none() {
    let doc = project(1).doc().clone();
    // A span that ends while the notes still sound, so what follows is a tail.
    let short = |effects, tail_seconds| FreezeRequest {
        span: (0, 2 * PPQ),
        ..request(&doc, effects, tail_seconds)
    };
    let none = render_now(&doc, &short(1, 0.0));
    let tail = render_now(&doc, &short(1, 6.0));
    assert_eq!(none.tail_frames, 0);
    assert!(
        tail.tail_frames > 24_000,
        "a reverb rings on: {}",
        tail.tail_frames
    );
    assert!(
        tail.audio.frames() < none.audio.frames() + 6 * 48_000,
        "and ends before the allowed tail"
    );
    let end = tail.audio.channels[0].len();
    // The cut tail is quiet at its end, and the audio before the span is the same.
    assert!(tail.audio.channels[0][end - 1].abs() < 1e-3);
    let span = none.audio.frames();
    assert_eq!(
        tail.audio.channels[0][..span],
        none.audio.channels[0][..span]
    );
    assert!(tail.length >= none.length);
    // A span that ends in silence adds nothing: the whole of the track's own span.
    let quiet = render_now(&doc, &request(&doc, 0, 6.0));
    assert!(
        quiet.tail_frames <= 1,
        "silence after the notes: {}",
        quiet.tail_frames
    );
}

#[test]
fn a_frozen_chain_with_a_tail_bounces_like_the_unfrozen_one() {
    let doc = project(2).doc().clone();
    let fx = chain(&doc);
    assert_eq!(fx.len(), 2);
    let plain = bounce(&doc, None);
    for (through, tail) in [(1usize, 8.0), (2, 8.0)] {
        let render = render_now(&doc, &request(&doc, through, tail));
        let (frozen, bundle, _dir) = frozen_doc(&doc, &render, &fx[..through], "tail");
        let with_freeze = bounce(&frozen, Some(&bundle));
        let diff = max_diff(&plain, &with_freeze);
        assert!(
            diff < 2e-4,
            "frozen through {through} effect(s): the bounces differ by {diff}"
        );
    }
}

#[test]
fn the_devices_of_a_frozen_track_are_not_in_the_engine_graph() {
    let doc = project(2).doc().clone();
    let fx = chain(&doc);
    let render = render_now(&doc, &request(&doc, 1, 0.0));
    let (frozen, _bundle, _dir) = frozen_doc(&doc, &render, &fx[..1], "graph");
    let registry = default_registry();
    let x = frozen.expand(&registry);
    let (patch, ids) = atomik_render::flat::expanded_patch(&frozen, &x);
    let mut options = atomik_render::project::console_options(&frozen, &registry);
    options.omit = x
        .devices
        .iter()
        .filter(|d| d.frozen)
        .map(|d| ids[&d.key].clone())
        .collect();
    assert_eq!(options.omit.len(), 2, "the instrument and the first effect");
    let built = atomik_render::build::build_with(&patch, &registry, &options).unwrap();
    let order: Vec<u32> = built.plan.order().iter().map(|id| id.0).collect();
    let id_of = |k: &DeviceKey| 1 + x.devices.iter().position(|d| &d.key == k).unwrap() as u32;
    for gone in [key("lead"), fx[0].clone()] {
        assert!(!order.contains(&id_of(&gone)), "{gone} is processed");
    }
    // Everything else is, under the id it has without a freeze: positions never shift.
    let unfrozen = doc.expand(&registry);
    for (i, d) in unfrozen.devices.iter().enumerate() {
        assert_eq!(id_of(&d.key), i as u32 + 1, "{}", d.key);
        if d.key != key("lead") && d.key != fx[0] {
            assert!(order.contains(&id_of(&d.key)), "{} is not processed", d.key);
        }
    }
    let player = x.devices.last().unwrap();
    assert!(player.key.0.ends_with("~freeze") && order.contains(&id_of(&player.key)));
}

#[test]
fn bouncing_chosen_clips_renders_only_those_through_the_chain() {
    let mut d = project(1);
    apply(
        &mut d,
        Command::Song(SongCommand::AddClip {
            device: key("lead"),
            lane: 0,
            clip: notes_clip(6 * BAR, BAR, &[(0, PPQ, 60)]),
        }),
    );
    let doc = d.doc().clone();
    let ids: Vec<ClipId> = doc.song.track(&key("lead")).unwrap().lanes[0]
        .clips
        .iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(ids.len(), 2);
    let second = &doc.song.track(&key("lead")).unwrap().lanes[0].clips[1];
    let req = FreezeRequest {
        device: key("lead"),
        effects: 1,
        clips: Some(vec![ids[1]]),
        span: (second.start, second.end()),
        tail_seconds: 3.0,
    };
    let render = render_now(&doc, &req);
    assert_eq!(render.start, 6 * BAR);
    assert!(render.audio.channels[0].iter().any(|x| x.abs() > 0.01));
    // The same as rendering a document that has only that clip.
    let mut only = doc.clone();
    only.song.tracks[0]
        .lanes
        .iter_mut()
        .for_each(|l| l.clips.retain(|c| c.id == ids[1]));
    let lead = only
        .song
        .tracks
        .iter()
        .position(|t| t.device == key("lead"))
        .unwrap();
    assert_eq!(only.song.tracks[lead].lanes[0].clips.len(), 1);
    let alone = FreezeRequest {
        clips: None,
        ..req.clone()
    };
    let lone = render_now(&only, &alone);
    assert_eq!(render.audio, lone.audio);
    // The other clip did not leak in: the first clip's span of a whole-track render is louder.
    let whole = render_now(&doc, &request(&doc, 1, 0.0));
    assert!(whole.audio.frames() > 0 && render.start > whole.start);
}

#[test]
fn a_muted_track_is_rendered_as_if_it_played() {
    let mut d = project(0);
    apply(
        &mut d,
        Command::Song(SongCommand::SetTrackFlags {
            device: key("lead"),
            muted: true,
            solo: false,
            armed: false,
        }),
    );
    let doc = d.doc().clone();
    let render = render_now(&doc, &request(&doc, 0, 0.0));
    assert!(render.audio.channels[0].iter().any(|x| x.abs() > 0.01));
}

#[test]
fn a_render_can_be_stopped_and_reports_its_progress() {
    let doc = project(1).doc().clone();
    let req = request(&doc, 1, 0.0);
    let stop = AtomicBool::new(true);
    let stopped = freeze_render(
        &doc,
        &req,
        (512, 0),
        RenderControl {
            cancel: Some(&stop),
            progress: None,
        },
    );
    assert!(stopped.is_none(), "a cancelled render has no result");
    let seen = std::sync::Mutex::new(Vec::<f32>::new());
    let progress = |p: f32| seen.lock().unwrap().push(p);
    let done = freeze_render(
        &doc,
        &req,
        (512, 0),
        RenderControl {
            cancel: None,
            progress: Some(&progress),
        },
    );
    assert!(done.is_some());
    let seen = seen.into_inner().unwrap();
    assert!(seen.len() > 3 && seen.first() == Some(&0.0) && seen.last() == Some(&1.0));
    assert!(seen.windows(2).all(|w| w[0] <= w[1]), "never goes back");
}

#[test]
fn a_freeze_that_cannot_render_says_why() {
    let doc = project(1).doc().clone();
    let registry = default_registry();
    let try_render = |doc: &RackDoc, req: &FreezeRequest| {
        render_freeze(
            doc,
            &registry,
            (&options(512, 0), None),
            &mut PluginBounce::default(),
            req,
            RenderControl::default(),
        )
        .expect_err("must fail")
        .to_string()
    };
    let ok = request(&doc, 1, 0.0);
    assert!(
        try_render(
            &doc,
            &FreezeRequest {
                effects: 2,
                ..ok.clone()
            }
        )
        .contains("effect")
    );
    assert!(
        try_render(
            &doc,
            &FreezeRequest {
                span: (BAR, BAR),
                ..ok.clone()
            }
        )
        .contains("empty")
    );
    assert!(
        try_render(
            &doc,
            &FreezeRequest {
                tail_seconds: 100.0,
                ..ok.clone()
            }
        )
        .contains("tail")
    );
    assert!(
        try_render(
            &doc,
            &FreezeRequest {
                device: key("nope"),
                ..ok.clone()
            }
        )
        .contains("track")
    );
    let render = render_now(&doc, &ok);
    let (frozen, _bundle, _dir) = frozen_doc(&doc, &render, &chain(&doc)[..1], "again");
    assert!(try_render(&frozen, &ok).contains("frozen already"));
}

#[test]
fn the_insert_chain_the_render_follows_is_the_channels() {
    let doc = project(2).doc().clone();
    let channel = track_chain(&doc, &default_registry(), &key("lead"))
        .unwrap()
        .channel;
    assert_eq!(
        insert_chain(&doc, &default_registry(), &channel),
        chain(&doc)
    );
}

// ---- plugins -----------------------------------------------------------------------------

use atomik_device_sdk::RunMode;
use atomik_model::{load_project, register_stand_ins};
use atomik_plugin_host::testing::clap_test_plugin;
use atomik_render::plugins::{MissingPlugins, PluginHost, bounce_settings, scan_in_process};

/// The plugin demo project (CLAP test plugins: the Lead synth, and Echo, a gain effect with 128
/// frames of latency, on Lead's channel insert), its registry and an in-process host.
fn plugin_demo() -> (RackDoc, atomik_device_sdk::Registry, PluginHost) {
    let cache =
        scan_in_process(&[clap_test_plugin("atomik-test-clap").expect("the plugin builds")]);
    let mut registry = default_registry();
    let mut host = PluginHost::new();
    let report = host.set_catalog(&cache, &bounce_settings(), &mut registry);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/projects/plugin-demo/plugin-demo.atomik");
    let text = std::fs::read_to_string(path).expect("the example project");
    let doc = load_project(&text, &registry).expect("a valid project");
    register_stand_ins(&doc, &mut registry).unwrap();
    (doc, registry, host)
}

#[test]
fn plugins_render_in_a_freeze_and_a_frozen_plugin_is_not_built() {
    let (doc, registry, mut host) = plugin_demo();
    let chain = track_chain(&doc, &registry, &key("lead")).unwrap();
    assert_eq!(chain.effects, [key("echo")]);
    let req = FreezeRequest {
        device: key("lead"),
        effects: 1,
        clips: None,
        span: clips_span(doc.song.track(&key("lead")).unwrap()).unwrap(),
        tail_seconds: 2.0,
    };
    let run = |host: &mut PluginHost| {
        render_freeze(
            &doc,
            &registry,
            (&options(512, 0), None),
            &mut PluginBounce {
                host: Some(host),
                mode: RunMode::Offline,
                missing: MissingPlugins::Error,
            },
            &req,
            RenderControl::default(),
        )
        .expect("the freeze renders")
        .expect("and finishes")
    };
    let a = run(&mut host);
    let b = run(&mut host);
    assert!(
        a.audio.channels[0].iter().any(|x| x.abs() > 1e-3),
        "it sounds"
    );
    assert!(a.audio.channels.iter().flatten().all(|x| x.is_finite()));
    assert_eq!(a.audio, b.audio, "the test plugins are deterministic");

    // Bounced with Lead and Echo frozen, the project builds two plugins fewer.
    let dir = TempDir::new("plugins");
    let mut bundle = Bundle::create(&dir.0.join("song")).unwrap();
    let info = bundle.add_recording(&a.audio, "freeze.wav").unwrap();
    let mut d = Document::new(doc.clone());
    d.apply(
        Command::RegisterAsset {
            hash: info.hash.clone(),
            meta: AssetMeta {
                channels: info.channels,
                sample_rate: info.sample_rate,
                frames: info.frames,
                original_name: info.original_name.clone(),
            },
        },
        &registry,
    )
    .unwrap();
    d.apply(
        Command::Song(SongCommand::FreezeTrack {
            device: key("lead"),
            frozen: Frozen {
                start: a.start,
                length: a.length,
                audio: AudioClipContent::new(info.hash, 0),
                effects: chain.effects.clone(),
            },
        }),
        &registry,
    )
    .unwrap();
    let bounce_with = |doc: &RackDoc, root: Option<&std::path::Path>, host: &mut PluginHost| {
        render_doc_with_host(doc, &registry, root, host)
    };
    let plain = bounce_with(&doc, None, &mut host);
    let frozen = bounce_with(d.doc(), Some(bundle.root()), &mut host);
    assert_eq!(plain.plugins, 5);
    assert_eq!(frozen.plugins, 3, "Lead and Echo are not built");
    assert!(frozen.channels[0].iter().any(|x| x.abs() > 1e-3));
    // Echo has 128 frames of latency. The unfrozen mix is delayed by it (compensation aligns
    // every track to the slowest path); the frozen track has none, so the frozen mix is the
    // unfrozen one that much earlier: the render was cut by the latency, tracks stay aligned.
    let latency = 128;
    let mut worst = 0.0f32;
    for (p, f) in plain.channels.iter().zip(&frozen.channels) {
        for i in 0..f.len() - latency {
            worst = worst.max((f[i] - p[i + latency]).abs());
        }
    }
    assert!(
        worst < 1e-4,
        "frozen[n] == unfrozen[n + 128], worst {worst}"
    );
}

fn render_doc_with_host(
    doc: &RackDoc,
    registry: &atomik_device_sdk::Registry,
    root: Option<&std::path::Path>,
    host: &mut PluginHost,
) -> Rendered {
    atomik_render::project::render_doc_with(
        doc,
        registry,
        &options(512, 0),
        root,
        &mut PluginBounce {
            host: Some(host),
            mode: RunMode::Offline,
            missing: MissingPlugins::Error,
        },
    )
    .expect("the bounce")
}

// ---- audio tracks ------------------------------------------------------------------------

#[test]
fn an_audio_track_freezes_through_its_insert_effects_too() {
    let registry = default_registry();
    let dir = TempDir::new("audio");
    let mut bundle = Bundle::create(&dir.0.join("song")).unwrap();
    let tone: Vec<f32> = (0..96_000)
        .map(|i| (0.3 * (std::f64::consts::TAU * 330.0 * f64::from(i) / 48_000.0).sin()) as f32)
        .collect();
    let info = bundle
        .add_recording(
            &atomik_audio_file::AudioData::new(48_000, vec![tone.clone(), tone]).unwrap(),
            "tone.wav",
        )
        .unwrap();
    let mut d = Document::new(new_project_doc());
    d.apply(
        Command::RegisterAsset {
            hash: info.hash.clone(),
            meta: AssetMeta {
                channels: info.channels,
                sample_rate: info.sample_rate,
                frames: info.frames,
                original_name: info.original_name.clone(),
            },
        },
        &registry,
    )
    .unwrap();
    apply(
        &mut d,
        Command::Song(SongCommand::AddAudioTrack { name: None }),
    );
    apply(
        &mut d,
        Command::Song(SongCommand::AddAudioClip {
            device: key("audio-track"),
            start: 0,
            length: 2 * BAR,
            asset: info.hash.clone(),
            source_offset: 0,
            gain_db: -3.0,
            fade_in: 0,
            fade_out: 2_000,
            fade_curve: atomik_model::FadeCurve::Linear,
        }),
    );
    apply(&mut d, Command::Song(SongCommand::SetSongEnd(4 * BAR)));
    let channel = d.doc().console.channels[0].channel.clone();
    apply(
        &mut d,
        Command::Console(ConsoleCommand::SetInsert {
            channel,
            slot: 0,
            effect: Some(InsertEffect::New {
                type_id: "atomik.reverb".into(),
            }),
        }),
    );
    let doc = d.doc().clone();
    let req = FreezeRequest {
        device: key("audio-track"),
        effects: 1,
        clips: None,
        span: clips_span(doc.song.track(&key("audio-track")).unwrap()).unwrap(),
        tail_seconds: 8.0,
    };
    let render = render_freeze(
        &doc,
        &registry,
        (&options(512, 0), Some(bundle.root())),
        &mut PluginBounce::default(),
        &req,
        RenderControl::default(),
    )
    .unwrap()
    .unwrap();
    assert!(render.tail_frames > 0 && render.audio.channels[0].iter().any(|x| x.abs() > 0.01));
    let frozen_info = bundle.add_recording(&render.audio, "freeze.wav").unwrap();
    let mut frozen = Document::new(doc.clone());
    frozen
        .apply(
            Command::RegisterAsset {
                hash: frozen_info.hash.clone(),
                meta: AssetMeta {
                    channels: frozen_info.channels,
                    sample_rate: frozen_info.sample_rate,
                    frames: frozen_info.frames,
                    original_name: frozen_info.original_name.clone(),
                },
            },
            &registry,
        )
        .unwrap();
    let fx = track_chain(&doc, &registry, &key("audio-track"))
        .unwrap()
        .effects;
    frozen
        .apply(
            Command::Song(SongCommand::FreezeTrack {
                device: key("audio-track"),
                frozen: Frozen {
                    start: render.start,
                    length: render.length,
                    audio: AudioClipContent::new(frozen_info.hash, 0),
                    effects: fx,
                },
            }),
            &registry,
        )
        .unwrap();
    let plain = bounce(&doc, Some(&bundle));
    let with_freeze = bounce(frozen.doc(), Some(&bundle));
    let diff = max_diff(&plain, &with_freeze);
    assert!(peak(&plain) > 0.05 && diff < 2e-4, "differ by {diff}");
}
