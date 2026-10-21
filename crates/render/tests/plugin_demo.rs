//! The `plugin-demo` example project and plugins in the bounce: the in-repo CLAP test plugins bounced through the shared builder, bit-identical across
//! host blocks, worker counts and runs; the `--allow-plugins` gate; missing plugins.
//!
//! `examples/projects/plugin-demo/plugin-demo.atomik` (no audio files, so no bundle) holds, all
//! made from the test plugins of `crates/testplugins` and a built-in synth:
//!
//! * **Lead**, the CLAP test synth (`atomik.test.synth`), with a console channel, and **Echo**, the
//!   CLAP test gain effect (`atomik.test.gain`, 128 frames of latency, -3 dB) on that channel's
//!   insert. The built-in **Bass** synth feeds Echo's sidechain input (the effect ducks).
//! * **Arp**, the CLAP test note effect (`atomik.test.noteshift`, +7 semitones), cabled
//!   `Arp:notes_out -> Pad:notes_in`; **Pad** is a second CLAP test synth with its own channel.
//! * **Box**, a container holding a CLAP test synth (split to its keys), with a console channel.
//! * The song: a track on Lead, one on Arp, one on Box and one on Bass.
//!
//! The project never stores a plugin path. To open it in the app (or bounce it) build the test
//! plugins and let the scanner find them:
//!
//! ```text
//! cargo build -p atomik-test-clap
//! # put (or copy) target/debug/atomik_test_clap.dll (Windows), libatomik_test_clap.so (Linux)
//! # or libatomik_test_clap.dylib (macOS) in a folder named atomik_test_clap.clap, or simply
//! # rename the library to *.clap, then either set CLAP_PATH to that folder or add it in
//! # Settings > Plugins > Folders and press SCAN
//! ```
//!
//! Regenerate the project after an intentional change with:
//!
//! ```text
//! ATOMIK_UPDATE_GOLDEN=1 cargo test -p atomik-render --test plugin_demo regenerate
//! ```

use atomik_core::PPQ;
use atomik_device_sdk::{PluginLayout, PluginRef, Registry, RunMode};
use atomik_model::{
    Cable, Clip, ClipContent, ClipId, Command, ConsoleCommand, DeviceKey, Document, InsertEffect,
    Note, PluginCommand, RackDoc, SocketRef, SongCommand, empty_doc, load_project,
    register_stand_ins, save_project,
};
use atomik_plugin_host::PluginCache;
use atomik_plugin_host::testing::clap_test_plugin;
use atomik_render::cli::main_with;
use atomik_render::default_registry;
use atomik_render::plugins::{MissingPlugins, PluginHost, bounce_settings, scan_in_process};
use atomik_render::project::{PluginBounce, ProjectOptions, render_doc_with};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

const UPDATE_ENV: &str = "ATOMIK_UPDATE_GOLDEN";
const BAR: i64 = 4 * PPQ;
const BARS: i64 = 2;
const PKG: &str = "atomik-test-clap";

fn demo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/projects/plugin-demo")
}

fn demo_file() -> PathBuf {
    demo_root().join("plugin-demo.atomik")
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

/// The scan cache of the built test plugin library (in-process scan: trusted, in-repo file).
fn test_cache() -> PluginCache {
    let file = clap_test_plugin(PKG).expect("the test plugin builds");
    scan_in_process(&[file])
}

fn scanned(cache: &PluginCache, id: &str) -> (PluginRef, PluginLayout) {
    cache
        .files()
        .flat_map(|f| &f.plugins)
        .find(|p| p.plugin.id == id)
        .map(|p| (p.plugin.clone(), p.layout.clone()))
        .unwrap_or_else(|| panic!("{id} is in the scan"))
}

/// A registry with the built-ins and the scanned test plugins registered, and the host.
fn env(cache: &PluginCache) -> (PluginHost, Registry) {
    let mut registry = default_registry();
    let mut host = PluginHost::new();
    let report = host.set_catalog(cache, &bounce_settings(), &mut registry);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    (host, registry)
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

    fn add_plugin(&mut self, id: &str, k: &str, name: &str) {
        let (plugin, layout) = scanned(self.cache, id);
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
                grid: None,
                content: ClipContent::Notes(notes),
            },
        }));
    }
}

/// The demo project.
fn demo_doc(registry: &Registry, cache: &PluginCache) -> RackDoc {
    let mut b = Builder {
        doc: Document::new(empty_doc()),
        registry,
        cache,
    };
    // Lead and its effect.
    b.add_plugin("atomik.test.synth", "lead", "Lead");
    b.set("lead", "p2", 0.6);
    b.add_plugin("atomik.test.gain", "echo", "Echo");
    b.set("echo", "p1", -3.0);
    b.set("echo", "p3", 128.0);
    b.set("echo", "p4", 0.5);
    let channel = b.doc.doc().console.channels[0].channel.clone();
    b.apply(Command::Console(ConsoleCommand::SetInsert {
        channel,
        slot: 0,
        effect: Some(InsertEffect::Existing(key("echo"))),
    }));
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
    // Arp: a note effect into a second synth.
    b.add_plugin("atomik.test.noteshift", "arp", "Arp");
    b.set("arp", "p1", 7.0);
    b.add_plugin("atomik.test.synth", "pad", "Pad");
    b.set("pad", "p1", 1.0);
    b.set("pad", "p2", 0.5);
    b.connect("arp:notes_out", "pad:notes_in");
    // Box: a container holding a synth.
    b.apply(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.container".into(),
        key: Some(key("box")),
    });
    b.rename("box", "Box");
    let (plugin, layout) = scanned(cache, "atomik.test.synth");
    b.apply(Command::Plugin(PluginCommand::Add {
        index: usize::MAX,
        container: Some(key("box")),
        plugin,
        layout,
        path_hint: None,
        key: Some(key("syn")),
        auto_split: true,
    }));
    b.set("box/syn", "p1", 2.0);
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

/// `ATOMIK_PLUGIN_CACHE_OUT=<file>` writes the scan cache of the built test plugins there, so the
/// demo can be bounced with the real binary without opening the app first
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
    std::fs::write(demo_file(), save_project(&demo_doc(&registry, &cache))).expect("written");
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

/// Loads the example project with the test plugins installed and bounces it.
fn bounce(block: usize, workers: u16, mode: RunMode) -> atomik_render::Rendered {
    let cache = test_cache();
    let (mut host, mut registry) = env(&cache);
    let text = std::fs::read_to_string(demo_file()).expect("the example project");
    let doc = load_project(&text, &registry).expect("a valid project");
    register_stand_ins(&doc, &mut registry).unwrap();
    let mut plugins = PluginBounce {
        host: Some(&mut host),
        mode,
        missing: MissingPlugins::Error,
    };
    let r = render_doc_with(
        &doc,
        &registry,
        &options(block, workers),
        None,
        &mut plugins,
    )
    .expect("the bounce");
    assert_eq!(
        host.manager_ref().instance_count(),
        0,
        "every instance is destroyed after the bounce"
    );
    r
}

/// A bounce with `--loops`: the song's transport edits (release the loop, stop) are scheduled by
/// frame, which the plugin loop has to apply batch by batch like the plain one does.
#[test]
fn a_loop_count_bounce_of_a_plugin_project_runs_the_plugin_loop_and_is_stable() {
    let run = || {
        let cache = test_cache();
        let (mut host, mut registry) = env(&cache);
        let text = std::fs::read_to_string(demo_file()).expect("the example project");
        let doc = load_project(&text, &registry).expect("a valid project");
        register_stand_ins(&doc, &mut registry).unwrap();
        let mut plugins = PluginBounce {
            host: Some(&mut host),
            mode: RunMode::Offline,
            missing: MissingPlugins::Error,
        };
        let options = ProjectOptions {
            loop_count: Some(1),
            ..options(512, 2)
        };
        render_doc_with(&doc, &registry, &options, None, &mut plugins).expect("the bounce")
    };
    let (a, b) = (run(), run());
    assert!(a.channels[0].iter().any(|x| x.abs() > 1e-3), "it sounds");
    assert!(a.channels.iter().flatten().all(|x| x.is_finite()));
    assert_eq!(bits(&a), bits(&b), "two runs, the same bits");
    assert_eq!(a.plugins, 5);
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
    assert_eq!(
        doc.plugin_devices().len(),
        5,
        "lead, echo, arp, pad, box/syn"
    );
    assert!(
        !text.contains(".dll") && !text.contains(".so") && !text.contains(".dylib"),
        "the project stores no plugin path"
    );
    if std::env::var_os(UPDATE_ENV).is_none() {
        assert_eq!(
            save_project(&demo_doc(&registry, &cache)),
            text,
            "regenerate with {UPDATE_ENV}=1 after an intentional change"
        );
    }
}

#[test]
fn the_demo_bounce_is_audible_and_finite() {
    let r = bounce(512, 0, RunMode::Offline);
    assert_eq!(r.plugins, 5);
    assert_eq!(r.channels.len(), 2);
    assert!(r.channels.iter().flatten().all(|x| x.is_finite()));
    let sr = r.sample_rate as usize;
    // Bar 1 plays (lead, pad, box, bass), the tail after the song is quiet.
    assert!(rms(&r.channels[0], 0, sr) > 1e-3, "the rack sounds");
    assert!(rms(&r.channels[1], 0, sr) > 1e-3);
    let end = r.channels[0].len();
    assert!(rms(&r.channels[0], end - sr / 4, end) < rms(&r.channels[0], 0, sr));
}

#[test]
fn the_bounce_is_bit_identical_across_blocks_workers_and_runs() {
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

// ---- the command line -------------------------------------------------------------------------

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("atomik-plugin-demo-{tag}-{}", std::process::id()));
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
fn a_project_with_plugins_is_refused_without_allow_plugins() {
    let t = Tmp::new("gate");
    let out = t.0.join("o.wav");
    let (code, _, err) = cli(&[&demo_arg(), "-o", out.to_str().unwrap()]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("--allow-plugins"), "{err}");
    assert!(
        err.contains("Atomik Test Synth") || err.contains("Lead"),
        "{err}"
    );
    assert!(err.contains("clap.atomik.test.synth"), "{err}");
    assert!(!out.exists(), "nothing is written");
}

#[test]
fn allow_plugins_bounces_with_the_report_line_and_the_cache() {
    let t = Tmp::new("allow");
    let cache_path = t.0.join("plugins.json");
    test_cache().save(&cache_path).unwrap();
    let out = t.0.join("o.wav");
    let args = [
        demo_arg(),
        "-o".into(),
        out.to_string_lossy().into_owned(),
        "--allow-plugins".into(),
        "--plugin-cache".into(),
        cache_path.to_string_lossy().into_owned(),
        "--stats".into(),
    ];
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let (code, stdout, err) = cli(&args);
    assert_eq!(code, 0, "{err}");
    assert!(
        err.contains("plugins: 5 (determinism not guaranteed)"),
        "{err}"
    );
    assert!(
        stdout.contains("plugins: 5 (determinism not guaranteed)"),
        "{stdout}"
    );
    let wav = atomik_render::wav::read_wav_file(&out).unwrap();
    assert_eq!(wav.channels.len(), 2);
    // The command line equals the library bounce (the default mode is offline, block 512).
    let expected = bounce(512, 0, RunMode::Offline);
    let from_cli: Vec<u32> = wav.channels.iter().flatten().map(|x| x.to_bits()).collect();
    assert_eq!(from_cli, bits(&expected));
    // --plugin-mode realtime, and other blocks and workers, run too.
    let rt = t.0.join("rt.wav");
    let (code, _, err) = cli(&[
        &demo_arg(),
        "-o",
        rt.to_str().unwrap(),
        "--allow-plugins",
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
    let wav = atomik_render::wav::read_wav_file(&rt).unwrap();
    let from_cli: Vec<u32> = wav.channels.iter().flatten().map(|x| x.to_bits()).collect();
    assert_eq!(from_cli, bits(&bounce(512, 0, RunMode::Realtime)));
}

#[test]
fn a_named_cache_that_does_not_exist_is_reported_by_name() {
    let t = Tmp::new("nocache");
    let absent = t.0.join("nope.json");
    let out = t.0.join("o.wav");
    let (code, _, err) = cli(&[
        &demo_arg(),
        "-o",
        out.to_str().unwrap(),
        "--allow-plugins",
        "--plugin-cache",
        absent.to_str().unwrap(),
    ]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("plugin cache not found"), "{err}");
    assert!(err.contains("nope.json"), "{err}");
    assert!(err.contains("missing plugin"), "{err}");
}

#[test]
fn a_missing_plugin_is_an_error_naming_it_or_a_stand_in() {
    let t = Tmp::new("missing");
    let empty = t.0.join("plugins.json");
    PluginCache::default().save(&empty).unwrap();
    let out = t.0.join("o.wav");
    let base = [
        demo_arg(),
        "-o".into(),
        out.to_string_lossy().into_owned(),
        "--allow-plugins".into(),
        "--plugin-cache".into(),
        empty.to_string_lossy().into_owned(),
    ];
    let args: Vec<&str> = base.iter().map(String::as_str).collect();
    let (code, _, err) = cli(&args);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("missing plugin"), "{err}");
    assert!(err.contains("clap.atomik.test.synth"), "{err}");
    assert!(err.contains("clap.atomik.test.gain"), "{err}");
    assert!(!out.exists());
    // With stand-ins the project bounces: instruments are silent, so only Bass is heard, and the
    // effect passes its (silent) channel through.
    let mut with: Vec<&str> = args.clone();
    with.extend(["--missing-plugins", "stand-in"]);
    let (code, _, err) = cli(&with);
    assert_eq!(code, 0, "{err}");
    let wav = atomik_render::wav::read_wav_file(&out).unwrap();
    let real = bounce(512, 0, RunMode::Offline);
    let sr = wav.sample_rate as usize;
    assert!(
        rms(&wav.channels[0], 0, sr) > 1e-4,
        "the built-in Bass sounds"
    );
    assert!(
        rms(&wav.channels[0], 0, sr) < rms(&real.channels[0], 0, sr),
        "less than with the plugins"
    );
}

#[test]
fn without_a_plugin_host_a_plugin_project_needs_stand_ins() {
    let cache = test_cache();
    let (_host, mut registry) = env(&cache);
    let text = std::fs::read_to_string(demo_file()).unwrap();
    let doc = load_project(&text, &registry).unwrap();
    register_stand_ins(&doc, &mut registry).unwrap();
    let e = atomik_render::project::render_doc_in(&doc, &registry, &options(512, 0), None)
        .unwrap_err()
        .to_string();
    assert!(e.contains("missing plugin"), "{e}");
}
