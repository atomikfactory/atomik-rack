//! The `plugin-demo-vst3` example project, CLAP and VST3 test plugins in one bounce: the project-file and `atomik-render` half of
//! the cross-format criterion.
//!
//! `examples/projects/plugin-demo-vst3/plugin-demo-vst3.atomik` (no audio files, so no bundle)
//! mixes the two formats, all made from the test plugins of `crates/testplugins` and a built-in
//! synth:
//!
//! * **Lead**, the CLAP test synth, with a console channel, and **Echo**, the **VST3** test gain
//!   effect (128 frames of latency, -3 dB) on that channel's insert. The built-in **Bass** synth
//!   feeds Echo's sidechain input (the effect ducks).
//! * **Arp**, the **VST3** test note effect (+7 semitones), cabled `Arp:notes_out -> Pad:notes_in`;
//!   **Pad** is a CLAP test synth with its own channel (notes cross the format border).
//! * **Vox**, the **VST3** test synth with its own channel, and **Wash**, the CLAP test gain
//!   effect (64 frames of latency, -6 dB) on that channel's insert.
//! * **Box**, a container holding a **VST3** test synth (split to its keys), with a console channel.
//! * The song: a track on Lead, Arp, Vox, Box and Bass.
//!
//! The twin of the project, the same rack with every plugin replaced by its CLAP counterpart
//! (the same DSP, other value mapping), bounces to the same bits: the two formats are the same
//! arithmetic.
//!
//! The project never stores a plugin path. To open it in the app (or bounce it) build the test
//! plugins and let the scanner find them (see the README of the example).
//!
//! Regenerate the project after an intentional change with:
//!
//! ```text
//! ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-render --test plugin_demo_vst3 regenerate
//! ```

use atomik_core::PPQ;
use atomik_device_sdk::{PluginFormat, PluginLayout, PluginRef, Registry, RunMode};
use atomik_model::{
    Cable, Clip, ClipContent, ClipId, Command, ConsoleCommand, DeviceKey, Document, InsertEffect,
    Note, PluginCommand, RackDoc, SocketRef, SongCommand, empty_doc, load_project,
    register_stand_ins, save_project,
};
use atomik_plugin_host::PluginCache;
use atomik_plugin_host::testing::{clap_test_plugin, vst3_test_plugin};
use atomik_render::cli::main_with;
use atomik_render::default_registry;
use atomik_render::plugins::{MissingPlugins, PluginHost, bounce_settings, scan_in_process};
use atomik_render::project::{PluginBounce, ProjectOptions, render_doc_with};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";
const BAR: i64 = 4 * PPQ;
const BARS: i64 = 2;
const PKG_CLAP: &str = "atomik-test-clap";
const PKG_VST3: &str = "atomik-test-vst3";

fn demo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/projects/plugin-demo-vst3")
}

fn demo_file() -> PathBuf {
    demo_root().join("plugin-demo-vst3.atomik")
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn sref(spec: &str) -> SocketRef {
    let (device, socket) = spec.split_once(':').expect("device:socket");
    SocketRef {
        device: key(device),
        socket: socket.to_string(),
    }
}

/// The scan cache of both built test libraries (in-process scan: trusted, in-repo files).
fn test_cache() -> PluginCache {
    let clap = clap_test_plugin(PKG_CLAP).expect("the CLAP test plugin builds");
    let vst3 = vst3_test_plugin(PKG_VST3).expect("the VST3 test plugin builds");
    scan_in_process(&[clap, vst3])
}

/// The four kinds of test plugin; the name is the same in both formats.
#[derive(Clone, Copy)]
enum Role {
    Synth,
    Gain,
    Shift,
}

impl Role {
    fn name(self) -> &'static str {
        match self {
            Role::Synth => "Atomik Test Synth",
            Role::Gain => "Atomik Test Gain",
            Role::Shift => "Atomik Test Note Shift",
        }
    }
}

fn scanned(cache: &PluginCache, format: PluginFormat, role: Role) -> (PluginRef, PluginLayout) {
    cache
        .files()
        .flat_map(|f| &f.plugins)
        .find(|p| p.plugin.format == format && p.layout.name == role.name())
        .map(|p| (p.plugin.clone(), p.layout.clone()))
        .unwrap_or_else(|| panic!("{format:?} {} is in the scan", role.name()))
}

/// A registry with the built-ins and the scanned test plugins registered, and the host.
fn env(cache: &PluginCache) -> (PluginHost, Registry) {
    let mut registry = default_registry();
    let mut host = PluginHost::new();
    let report = host.set_catalog(cache, &bounce_settings(), &mut registry);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    (host, registry)
}

/// The value of a plugin param in the rack's units for `format`, from the plugin's own plain
/// value: CLAP takes the plain value, VST3 a normalised value (or a step index from the minimum
/// for a stepped param). The test plugins' VST3 gain spans -64 to 64 dB.
struct Val;

impl Val {
    fn gain_db(format: PluginFormat, db: f32) -> f32 {
        match format {
            PluginFormat::Clap => db,
            PluginFormat::Vst3 => (db + 64.0) / 128.0,
        }
    }

    fn semitones(format: PluginFormat, semis: f32) -> f32 {
        match format {
            PluginFormat::Clap => semis,
            PluginFormat::Vst3 => semis + 24.0,
        }
    }
}

/// The format of each plugin of the demo.
#[derive(Clone, Copy)]
struct Formats {
    lead: PluginFormat,
    echo: PluginFormat,
    arp: PluginFormat,
    pad: PluginFormat,
    vox: PluginFormat,
    wash: PluginFormat,
    syn: PluginFormat,
}

impl Formats {
    /// The demo: the two formats mixed.
    fn mixed() -> Self {
        use PluginFormat::{Clap, Vst3};
        Self {
            lead: Clap,
            echo: Vst3,
            arp: Vst3,
            pad: Clap,
            vox: Vst3,
            wash: Clap,
            syn: Vst3,
        }
    }

    /// The twin: every plugin in CLAP.
    fn all_clap() -> Self {
        use PluginFormat::Clap;
        Self {
            lead: Clap,
            echo: Clap,
            arp: Clap,
            pad: Clap,
            vox: Clap,
            wash: Clap,
            syn: Clap,
        }
    }
}

struct Builder<'r> {
    doc: Document,
    registry: &'r Registry,
    cache: &'r PluginCache,
}

impl Builder<'_> {
    fn apply(&mut self, c: Command) {
        self.doc
            .apply(c.clone(), self.registry)
            .unwrap_or_else(|e| panic!("{c:?}: {e}"));
    }

    fn add_plugin(&mut self, format: PluginFormat, role: Role, k: &str, name: &str) {
        let (plugin, layout) = scanned(self.cache, format, role);
        self.apply(Command::Plugin(PluginCommand::Add {
            index: usize::MAX,
            container: None,
            plugin,
            layout,
            path_hint: None,
            key: Some(key(k)),
            auto_split: false,
        }));
        self.rename(k, name);
    }

    fn rename(&mut self, k: &str, name: &str) {
        self.apply(Command::Rename {
            key: key(k),
            name: name.into(),
        });
    }

    fn set(&mut self, path: &str, p: &str, v: f32) {
        self.apply(Command::SetParam {
            key: key(path),
            param: p.into(),
            value: v,
            merge: None,
        });
    }

    fn connect(&mut self, from: &str, to: &str) {
        self.apply(Command::Connect(Cable {
            from: sref(from),
            to: sref(to),
        }));
    }

    fn channel_of(&self, source: &str) -> DeviceKey {
        self.doc
            .doc()
            .console
            .channels
            .iter()
            .find(|c| c.source.as_ref() == Some(&key(source)))
            .map(|c| c.channel.clone())
            .unwrap_or_else(|| panic!("{source} has a console channel"))
    }

    fn insert(&mut self, source: &str, effect: &str) {
        let channel = self.channel_of(source);
        self.apply(Command::Console(ConsoleCommand::SetInsert {
            channel,
            slot: 0,
            effect: Some(InsertEffect::Existing(key(effect))),
        }));
    }

    fn track(&mut self, device: &str, notes: &[(i64, i64, u8)]) {
        self.apply(Command::Song(SongCommand::AddTrack {
            device: key(device),
        }));
        let mut notes: Vec<Note> = notes
            .iter()
            .map(|&(beat, beats, k)| Note::new(beat * PPQ, beats * PPQ, k, 0.8))
            .collect();
        notes.sort_by_key(|n| (n.tick, n.key));
        self.apply(Command::Song(SongCommand::AddClip {
            device: key(device),
            lane: 0,
            clip: Clip {
                id: ClipId(0),
                start: 0,
                length: BARS * BAR,
                offset: 0,
                loop_length: None,
                muted: false,
                scale: None,
                controllers: Vec::new(),
                grid: None,
                content: ClipContent::Notes(notes),
            },
        }));
    }
}

/// The demo project with the given formats.
fn demo_doc(registry: &Registry, cache: &PluginCache, f: Formats) -> RackDoc {
    let mut b = Builder {
        doc: Document::new(empty_doc()),
        registry,
        cache,
    };
    // Lead (CLAP) and its VST3 effect.
    b.add_plugin(f.lead, Role::Synth, "lead", "Lead");
    b.set("lead", "p2", 0.6);
    b.add_plugin(f.echo, Role::Gain, "echo", "Echo");
    b.set("echo", "p1", Val::gain_db(f.echo, -3.0));
    b.set("echo", "p3", 128.0);
    b.set("echo", "p4", 0.5);
    b.insert("lead", "echo");
    // Bass: a built-in synth that ducks Echo through its sidechain.
    b.apply(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.poly-synth".into(),
        key: Some(key("bass")),
    });
    b.rename("bass", "Bass");
    b.set("bass", "voices", 2.0);
    b.set("bass", "master.level", -10.0);
    b.connect("bass:out_l", "echo:sc_in_l");
    b.connect("bass:out_r", "echo:sc_in_r");
    // Arp (VST3 note effect) into Pad (CLAP): notes cross the format border.
    b.add_plugin(f.arp, Role::Shift, "arp", "Arp");
    b.set("arp", "p1", Val::semitones(f.arp, 7.0));
    b.add_plugin(f.pad, Role::Synth, "pad", "Pad");
    b.set("pad", "p1", 1.0);
    b.set("pad", "p2", 0.5);
    b.connect("arp:notes_out", "pad:notes_in");
    // Vox (VST3 synth) and Wash (CLAP effect) on its insert.
    b.add_plugin(f.vox, Role::Synth, "vox", "Vox");
    b.set("vox", "p2", 0.4);
    b.add_plugin(f.wash, Role::Gain, "wash", "Wash");
    b.set("wash", "p1", Val::gain_db(f.wash, -6.0));
    b.set("wash", "p3", 64.0);
    b.insert("vox", "wash");
    // Box: a container holding a VST3 synth.
    b.apply(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.container".into(),
        key: Some(key("box")),
    });
    b.rename("box", "Box");
    let (plugin, layout) = scanned(cache, f.syn, Role::Synth);
    b.apply(Command::Plugin(PluginCommand::Add {
        index: usize::MAX,
        container: Some(key("box")),
        plugin,
        layout,
        path_hint: None,
        key: Some(key("syn")),
        auto_split: true,
    }));
    b.set("box/syn", "p1", 1.0);
    b.set("box/syn", "p2", 0.5);
    for side in ["l", "r"] {
        b.connect(
            &format!("box/syn:out_{side}"),
            &format!("box/boundary:out_{side}"),
        );
    }
    b.apply(Command::Console(ConsoleCommand::AddChannel {
        source: key("box"),
    }));
    // The song.
    b.track(
        "lead",
        &[(0, 2, 60), (2, 2, 64), (4, 2, 67), (6, 2, 64), (0, 8, 72)],
    );
    b.track("arp", &[(0, 1, 48), (1, 1, 52), (4, 1, 55), (5, 1, 52)]);
    b.track("vox", &[(0, 3, 55), (4, 3, 59)]);
    b.track("box", &[(0, 4, 48), (4, 4, 53)]);
    b.track("bass", &[(0, 2, 36), (2, 2, 36), (4, 2, 41), (6, 2, 41)]);
    let length = BARS * BAR;
    b.apply(Command::Song(SongCommand::SetSongEnd(length)));
    b.apply(Command::Song(SongCommand::SetLoop {
        range: (0, length),
        enabled: false,
    }));
    b.doc.doc().clone()
}

/// `ATOMIK_PLUGIN_CACHE_OUT=<file>` writes the scan cache of both built test libraries there, so
/// the demo can be bounced with the real binary without opening the app first
/// (`atomik-render ... --allow-plugins --plugin-cache <file>`).
#[test]
fn regenerate() {
    if let Some(out) = std::env::var_os("ATOMIK_PLUGIN_CACHE_OUT") {
        test_cache().save(Path::new(&out)).expect("cache written");
    }
    if std::env::var_os(UPDATE_ENV).is_none() {
        return;
    }
    let cache = test_cache();
    let (_host, registry) = env(&cache);
    std::fs::create_dir_all(demo_root()).expect("project folder");
    std::fs::write(
        demo_file(),
        save_project(&demo_doc(&registry, &cache, Formats::mixed())),
    )
    .expect("written");
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

/// Bounces `doc` (already built against `registry`) with the plugins of `host`.
fn bounce_doc(
    doc: &RackDoc,
    host: &mut PluginHost,
    registry: &Registry,
    block: usize,
    workers: u16,
    mode: RunMode,
) -> atomik_render::Rendered {
    let mut plugins = PluginBounce {
        host: Some(host),
        mode,
        missing: MissingPlugins::Error,
    };
    let r = render_doc_with(doc, registry, &options(block, workers), None, &mut plugins)
        .expect("the bounce");
    assert_eq!(
        host.manager_ref().instance_count(),
        0,
        "every instance is destroyed after the bounce"
    );
    r
}

/// Loads the example project with the test plugins installed and bounces it.
fn bounce(block: usize, workers: u16, mode: RunMode) -> atomik_render::Rendered {
    let cache = test_cache();
    let (mut host, mut registry) = env(&cache);
    let text = std::fs::read_to_string(demo_file()).expect("the example project");
    let doc = load_project(&text, &registry).expect("a valid project");
    register_stand_ins(&doc, &mut registry).unwrap();
    bounce_doc(&doc, &mut host, &registry, block, workers, mode)
}

/// The twin (every plugin CLAP) bounced.
fn bounce_twin(block: usize, workers: u16, mode: RunMode) -> atomik_render::Rendered {
    let cache = test_cache();
    let (mut host, registry) = env(&cache);
    let doc = demo_doc(&registry, &cache, Formats::all_clap());
    bounce_doc(&doc, &mut host, &registry, block, workers, mode)
}

fn rms(channel: &[f32], from: usize, to: usize) -> f64 {
    let w = &channel[from..to.min(channel.len())];
    (w.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / w.len().max(1) as f64).sqrt()
}

#[test]
fn the_example_project_is_what_the_builder_makes() {
    let cache = test_cache();
    let (_host, registry) = env(&cache);
    let text = std::fs::read_to_string(demo_file()).expect("the example project");
    let doc = load_project(&text, &registry).expect("a valid project");
    assert_eq!(
        save_project(&doc),
        text,
        "the committed file is canonical (save, load, save)"
    );
    let devices = doc.plugin_devices();
    assert_eq!(devices.len(), 7, "lead, echo, arp, pad, vox, wash, box/syn");
    let vst3 = devices
        .iter()
        .filter(|(_, d)| d.plugin.as_ref().unwrap().format == PluginFormat::Vst3)
        .count();
    assert_eq!(
        vst3, 4,
        "echo, arp, vox and box/syn are VST3, the rest CLAP"
    );
    assert!(
        !text.contains(".dll")
            && !text.contains(".so")
            && !text.contains(".dylib")
            && !text.contains(".vst3")
            && !text.contains(".clap"),
        "the project stores no plugin path"
    );
    if std::env::var_os(UPDATE_ENV).is_none() {
        assert_eq!(
            save_project(&demo_doc(&registry, &cache, Formats::mixed())),
            text,
            "regenerate with {UPDATE_ENV}=1 after an intentional change"
        );
    }
}

#[test]
fn the_demo_bounce_is_audible_and_finite() {
    let r = bounce(512, 0, RunMode::Offline);
    assert_eq!(r.plugins, 7);
    assert_eq!(r.channels.len(), 2);
    assert!(r.channels.iter().flatten().all(|x| x.is_finite()));
    let sr = r.sample_rate as usize;
    assert!(rms(&r.channels[0], 0, sr) > 1e-3, "the rack sounds");
    assert!(rms(&r.channels[1], 0, sr) > 1e-3);
    let end = r.channels[0].len();
    assert!(rms(&r.channels[0], end - sr / 4, end) < rms(&r.channels[0], 0, sr));
}

#[test]
fn the_mixed_project_bounces_bit_identically_across_blocks_workers_and_runs() {
    for mode in [RunMode::Offline, RunMode::Realtime] {
        let reference = bits(&bounce(512, 0, mode));
        assert_eq!(bits(&bounce(512, 0, mode)), reference, "two runs, {mode:?}");
        for (block, workers) in [(64, 0), (480, 0), (512, 1), (512, 2), (64, 4), (333, 4)] {
            assert_eq!(
                bits(&bounce(block, workers, mode)),
                reference,
                "block {block}, workers {workers}, {mode:?}"
            );
        }
    }
}

/// At project level: the same rack with the VST3 test plugins swapped for the CLAP ones
/// (the same DSP core) gives the same bits, so the library-level reference of the twin is the
/// reference of the mixed project.
#[test]
fn the_mixed_project_equals_its_all_clap_twin() {
    for mode in [RunMode::Offline, RunMode::Realtime] {
        let twin = bounce_twin(512, 0, mode);
        assert_eq!(twin.plugins, 7);
        for (block, workers) in [(512, 0), (64, 0), (333, 4)] {
            assert_eq!(
                bits(&bounce(block, workers, mode)),
                bits(&bounce_twin(block, workers, mode)),
                "block {block}, workers {workers}, {mode:?}"
            );
        }
    }
}

// ---- the command line -------------------------------------------------------------------------

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "atomik-plugin-demo-vst3-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn cli(args: &[&str]) -> (i32, String, String) {
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = main_with(
        std::iter::once("atomik-render").chain(args.iter().copied()),
        &mut out,
        &mut err,
    );
    (
        code,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}

fn demo_arg() -> String {
    demo_file().to_string_lossy().into_owned()
}

#[test]
fn a_project_with_vst3_plugins_is_refused_without_allow_plugins_and_names_them() {
    let t = Tmp::new("gate");
    let out = t.0.join("o.wav");
    let (code, _, err) = cli(&[&demo_arg(), "-o", out.to_str().unwrap()]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("--allow-plugins"), "{err}");
    assert!(err.contains("vst3."), "the VST3 plugins are named: {err}");
    assert!(err.contains("clap.atomik.test.synth"), "{err}");
    assert!(!out.exists(), "nothing is written");
}

fn read_bits(path: &Path) -> Vec<u32> {
    let wav = atomik_render::wav::read_wav_file(path).unwrap();
    wav.channels.iter().flatten().map(|x| x.to_bits()).collect()
}

#[test]
fn allow_plugins_bounces_both_formats_and_equals_the_library_bounce() {
    let t = Tmp::new("allow");
    let cache_path = t.0.join("plugins.json");
    test_cache().save(&cache_path).unwrap();
    let out = t.0.join("o.wav");
    let args = [
        demo_arg(),
        "-o".into(),
        out.to_string_lossy().into_owned(),
        "--allow-plugins".into(),
        "--plugin-sandbox".into(),
        "off".into(),
        "--plugin-cache".into(),
        cache_path.to_string_lossy().into_owned(),
        "--stats".into(),
    ];
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let (code, stdout, err) = cli(&args);
    assert_eq!(code, 0, "{err}");
    assert!(
        err.contains("plugins: 7 (determinism not guaranteed)"),
        "{err}"
    );
    assert!(
        stdout.contains("plugins: 7 (determinism not guaranteed)"),
        "{stdout}"
    );
    assert_eq!(read_bits(&out), bits(&bounce(512, 0, RunMode::Offline)));
    // --plugin-mode realtime with other blocks and workers equals the library bounce.
    let rt = t.0.join("rt.wav");
    let (code, _, err) = cli(&[
        &demo_arg(),
        "-o",
        rt.to_str().unwrap(),
        "--allow-plugins",
        "--plugin-sandbox",
        "off",
        "--plugin-cache",
        cache_path.to_str().unwrap(),
        "--plugin-mode",
        "realtime",
        "--block",
        "64",
        "--workers",
        "2",
    ]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(read_bits(&rt), bits(&bounce(512, 0, RunMode::Realtime)));
}

#[test]
fn a_missing_vst3_plugin_is_an_error_naming_it_or_a_stand_in() {
    let t = Tmp::new("missing");
    // A cache that has the CLAP plugins only: the VST3 ones are missing.
    let clap_only = {
        let file = clap_test_plugin(PKG_CLAP).unwrap();
        scan_in_process(&[file])
    };
    let cache_path = t.0.join("plugins.json");
    clap_only.save(&cache_path).unwrap();
    let out = t.0.join("o.wav");
    let base = [
        demo_arg(),
        "-o".into(),
        out.to_string_lossy().into_owned(),
        "--allow-plugins".into(),
        "--plugin-sandbox".into(),
        "off".into(),
        "--plugin-cache".into(),
        cache_path.to_string_lossy().into_owned(),
    ];
    let args: Vec<&str> = base.iter().map(String::as_str).collect();
    let (code, _, err) = cli(&args);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("missing plugin"), "{err}");
    assert!(err.contains("vst3."), "{err}");
    assert!(!out.exists());
    let mut with: Vec<&str> = args.clone();
    with.extend(["--missing-plugins", "stand-in"]);
    let (code, _, err) = cli(&with);
    assert_eq!(code, 0, "{err}");
    let wav = atomik_render::wav::read_wav_file(&out).unwrap();
    let real = bounce(512, 0, RunMode::Offline);
    let sr = wav.sample_rate as usize;
    assert!(rms(&wav.channels[0], 0, sr) > 1e-4, "Lead and Bass sound");
    // The VST3 synths are silent and the VST3 effects pass their input, so it is another sound
    // than the real project's (it is not "quieter": the real Lead is ducked by the VST3 gain's
    // sidechain, which the stand-in does not do).
    assert!(
        wav.channels[0]
            .iter()
            .zip(&real.channels[0])
            .any(|(a, b)| a.to_bits() != b.to_bits()),
        "not the sound of the project with the VST3 plugins"
    );
}
