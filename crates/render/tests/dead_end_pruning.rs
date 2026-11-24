//! Dead-end pruning at the render level: a project renders the same bytes with the pruning on
//! and off (every example project, and a synthetic patch full of dead ends and observers), the
//! set of devices left out is the same with any worker count, and a real echo that was unplugged
//! while it rang plays no stale echo when it is plugged in again. The counts of dead ends in the
//! example projects are printed (`--nocapture`).
atomik_testkit::install_alloc_guard!();

use atomik_core::{BATCH, NoteKind};
use atomik_device_sdk::SocketId;
use atomik_engine::{
    DeviceInstanceId, Endpoint, EngineOptions, Graph, OfflineJob, ScheduledNote, Topology,
    WorkerConfig, new_engine, new_engine_with_options, render_offline_scripted, without_pruning,
};
use atomik_render::build::{BuildOptions, build_with};
use atomik_render::patch::Patch;
use atomik_render::project::{ProjectOptions, console_options, render_project_file};
use atomik_render::{default_registry, flat};
use atomik_testkit::no_alloc::assert_no_alloc;
use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

fn examples() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "atomik")
                && !path.to_string_lossy().contains("plugin-demo")
            {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/projects"),
        &mut files,
    );
    files.sort();
    files
}

fn bits(path: &Path, workers: u16) -> Vec<u32> {
    let options = ProjectOptions {
        host_block: NonZeroUsize::new(512).unwrap(),
        workers,
        loop_count: None,
    };
    render_project_file(path, &default_registry(), &options)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .channels
        .iter()
        .flatten()
        .map(|x| x.to_bits())
        .collect()
}

#[test]
fn every_example_project_renders_the_same_bytes_with_the_pruning_on_and_off() {
    let files = examples();
    assert!(files.len() >= 10, "the examples are there: {files:?}");
    for path in files {
        let on = bits(&path, 0);
        assert!(!on.is_empty(), "{}: renders something", path.display());
        let off = without_pruning(|| bits(&path, 0));
        assert!(
            on == off,
            "{}: the pruning changed a sample",
            path.display()
        );
        let on_workers = bits(&path, 2);
        assert!(
            on == on_workers,
            "{}: two workers changed a sample",
            path.display()
        );
    }
}

/// The dead ends of one document's project render, by device type, added to `tally`
/// (devices left out, devices, projects, by type). `None` when the document cannot be built here
/// (audio assets the corpus does not carry, plugins).
fn dead_ends(
    label: &str,
    doc: &atomik_model::RackDoc,
    by_type: &mut BTreeMap<String, usize>,
    tally: &mut (usize, usize, usize),
) {
    let registry = default_registry();
    let expanded = doc.expand(&registry);
    let (patch, _) = flat::expanded_patch(doc, &expanded);
    let mut options: BuildOptions = console_options(doc, &registry);
    options.prune_dead_ends = true;
    let Ok(built) = build_with(&patch, &registry, &options) else {
        println!("{label}: not built here");
        return;
    };
    let pruned = built.plan.pruned_devices();
    let again = build_with(&patch, &registry, &options)
        .unwrap()
        .plan
        .pruned_devices();
    assert_eq!(pruned, again, "two compiles leave out the same devices");
    // Instance ids follow the patch's device order, from 1.
    let kinds: Vec<String> = pruned
        .iter()
        .map(|id| patch.devices[id.0 as usize - 1].device_type.clone())
        .collect();
    for kind in &kinds {
        *by_type.entry(kind.clone()).or_default() += 1;
    }
    println!(
        "{label}: {} of {} devices are dead ends {kinds:?}",
        pruned.len(),
        patch.devices.len(),
    );
    tally.0 += pruned.len();
    tally.1 += patch.devices.len();
    tally.2 += 1;
}

/// The dead ends of the example projects, the stress projects the benches use and the format
/// corpus (printed with `--nocapture`; asserted only to be computed the same way twice).
#[test]
fn the_dead_ends_of_the_example_stress_and_corpus_projects_are_counted() {
    use atomik_model::stress::{
        STRESS_CABLES, STRESS_DEVICES, console_doc, lanes_song_doc, large_song_doc, stress_doc,
        stress_song_doc,
    };
    let registry = default_registry();
    let mut by_type: BTreeMap<String, usize> = BTreeMap::new();
    let mut tally = (0usize, 0usize, 0usize);
    for path in examples() {
        let file = atomik_render::bundle::resolve_project_path(&path);
        let text = atomik_model::read_document_text(&file).unwrap();
        let doc = atomik_model::load_project(&text, &registry).unwrap();
        let label = format!("example {}", path.file_name().unwrap().to_string_lossy());
        dead_ends(&label, &doc, &mut by_type, &mut tally);
    }
    let examples = tally;
    let stress = [
        (
            "stress rack",
            stress_doc(&registry, STRESS_DEVICES, STRESS_CABLES),
        ),
        ("stress song", stress_song_doc(&registry)),
        ("stress console-64", console_doc(&registry, 64)),
        ("stress lanes-200", lanes_song_doc(&registry, 200)),
        (
            "stress large-song",
            large_song_doc(&registry, 100, 500, 2000),
        ),
    ];
    for (label, doc) in &stress {
        dead_ends(label, doc, &mut by_type, &mut tally);
    }
    let stress_tally = (
        tally.0 - examples.0,
        tally.1 - examples.1,
        tally.2 - examples.2,
    );
    let corpus_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../model/tests/fixtures/corpus");
    let mut corpus: Vec<PathBuf> = std::fs::read_dir(corpus_dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "atomik"))
        .collect();
    corpus.sort();
    let before = tally;
    for path in &corpus {
        let text = std::fs::read_to_string(path).unwrap();
        let Ok(doc) = atomik_model::load_project(&text, &registry) else {
            continue;
        };
        let label = format!("corpus {}", path.file_name().unwrap().to_string_lossy());
        dead_ends(&label, &doc, &mut by_type, &mut tally);
    }
    println!(
        "dead ends: examples {examples:?}, stress {stress_tally:?}, corpus {:?} \
         (devices left out, devices, projects); by type: {by_type:?}",
        (tally.0 - before.0, tally.1 - before.1, tally.2 - before.2)
    );
}

// ---------------------------------------------------------------------------------------------
// A synthetic patch with dead-end chains and an observer that must stay live
// ---------------------------------------------------------------------------------------------

const SYNTH: &str = r#""type": "atomik.poly-synth",
    "params": {"osc1.wave": 1, "osc.mix": 0.0, "voices": 2, "filter.cutoff": 6000,
               "env.amp.attack": 2, "env.amp.decay": 200, "env.amp.sustain": 0.8,
               "env.amp.release": 80}"#;

/// A live synth into a gain tool into the interface; three more synths playing into nothing; an
/// echo fed by a fourth with its outputs unplugged (every built-in effect publishes its match
/// gain, so it is kept, and so is the synth that feeds it); and an analyser at the end of a chain
/// fed by a fifth synth: that chain has no audio consumer and must stay live.
fn synthetic() -> String {
    let synth = |id: &str| format!(r#"{{"id": "{id}", {SYNTH}}}"#);
    let mut devices = vec![
        synth("live"),
        r#"{"id": "gain", "type": "atomik.gain-tool", "params": {"gain": -6}}"#.to_owned(),
    ];
    for id in ["d1", "d2", "d3", "d4", "seen"] {
        devices.push(synth(id));
    }
    devices.push(
        r#"{"id": "echo", "type": "atomik.echo", "params": {"feedback": 0.7, "mix": 1.0}}"#
            .to_owned(),
    );
    devices.push(r#"{"id": "analyser", "type": "atomik.spectrum"}"#.to_owned());
    let mut cables = Vec::new();
    let mut link = |from: &str, to: &str| {
        for side in ["l", "r"] {
            cables.push(format!(
                r#"{{"from": "{from}:out_{side}", "to": "{to}:in_{side}"}}"#
            ));
        }
    };
    link("live", "gain");
    link("d4", "echo");
    link("seen", "analyser");
    cables.push(r#"{"from": "gain:out_l", "to": "interface:out_1"}"#.to_owned());
    cables.push(r#"{"from": "gain:out_r", "to": "interface:out_2"}"#.to_owned());
    let mut notes = Vec::new();
    for id in ["live", "d1", "d2", "d3", "d4", "seen"] {
        for (i, key) in [45, 48, 52, 57].iter().enumerate() {
            notes.push(format!(
                r#"{{"device": "{id}", "socket": "notes_in", "at_beats": {i}.0, "length_beats": 0.75, "key": {key}, "velocity": 0.9}}"#
            ));
        }
    }
    format!(
        r#"{{"format": "atomik-patch", "version": 1, "sample_rate": 48000, "tempo_bpm": 120,
            "seed": 3, "length_beats": 4, "output_channels": 2,
            "devices": [{}], "cables": [{}], "notes": [{}]}}"#,
        devices.join(","),
        cables.join(","),
        notes.join(",")
    )
}

/// Renders the synthetic patch and returns the bits and the devices that were left out.
fn render_synthetic(prune: bool, workers: u16, block: usize) -> (Vec<u32>, Vec<DeviceInstanceId>) {
    let json = synthetic();
    let patch = Patch::from_json(&json).unwrap_or_else(|e| panic!("{e}\n{json}"));
    let options = BuildOptions {
        prune_dead_ends: prune,
        ..BuildOptions::default()
    };
    let built = build_with(&patch, &default_registry(), &options).unwrap();
    let pruned = built.plan.pruned_devices();
    let job = OfflineJob {
        frames: built.frames,
        host_block: block,
        notes: &built.notes,
        params: &[],
        inputs: &[],
    };
    let (mut engine, mut handle) = new_engine_with_options(
        built.config,
        EngineOptions {
            workers: WorkerConfig {
                threads: workers,
                on_thread_start: None,
            },
            worker_linger_spins: 0,
            realtime: false,
        },
    );
    handle.submit_plan(built.plan).unwrap();
    let rendered = render_offline_scripted(&mut engine, &mut handle, &job, Vec::new(), |r| {
        assert_no_alloc(r)
    });
    let bits = rendered.iter().flatten().map(|x| x.to_bits()).collect();
    (bits, pruned)
}

#[test]
fn a_patch_full_of_dead_ends_renders_the_same_bytes_and_keeps_its_observer() {
    let (on, pruned) = render_synthetic(true, 0, 512);
    let (off, none) = render_synthetic(false, 0, 512);
    assert!(none.is_empty());
    assert!(on == off, "the pruning changed a sample");
    assert!(
        on.iter().any(|b| f32::from_bits(*b).abs() > 0.01),
        "audible"
    );
    // Devices are numbered in the patch's order: live 1, gain 2, d1 3, d2 4, d3 5, d4 6, seen 7,
    // echo 8, analyser 9. The chains into the echo and the analyser are live: both publish.
    let ids: Vec<u32> = pruned.iter().map(|d| d.0).collect();
    assert_eq!(ids, vec![3, 4, 5], "d1 d2 d3");
    // Another host block, and workers: the same set and the same bytes.
    let (block, again) = render_synthetic(true, 0, 333);
    assert_eq!(again, pruned);
    assert!(block == on, "the host block changed a sample");
    let (workers, with_workers) = render_synthetic(true, 4, 512);
    assert_eq!(with_workers, pruned);
    assert!(workers == on, "four workers changed a sample");
}

// ---------------------------------------------------------------------------------------------
// A real synth that holds a note while it is unplugged
// ---------------------------------------------------------------------------------------------

/// A synth holds a note (no note off ever comes) and feeds the interface. Its output is
/// unplugged 0.3 s in, for 0.2 s, then plugged in again. Returns the sound heard in the second
/// that follows. (A built-in effect publishes its match gain, so the pruning keeps it: the
/// instruments are what a dead end can be.)
fn heard_after_replug(prune: bool) -> (Vec<f32>, Vec<f32>) {
    let registry = default_registry();
    let config = atomik_engine::EngineConfig {
        sample_rate: atomik_core::SampleRate::new(48_000.0).unwrap(),
        tempo: atomik_core::Tempo::new(120.0).unwrap(),
        input_channels: 0,
        output_channels: 2,
        seed: 1,
    };
    let (mut engine, mut handle) = new_engine(config);
    let mut topology = Topology::new(&config);
    topology.set_prune_dead_ends(prune);
    let synth = DeviceInstanceId(1);
    let device = registry.create("atomik.poly-synth").unwrap();
    let sd = device.descriptor();
    topology.add_node(synth, sd).unwrap();
    handle.insert_device(synth, device, &[]).unwrap();
    let key =
        |d: &'static atomik_device_sdk::DeviceDescriptor, k: &str| d.socket_by_key(k).unwrap().id;
    let end = |device, socket: SocketId| Endpoint { device, socket };
    let iface = topology.descriptor(Graph::INTERFACE).unwrap();
    let out_l = end(Graph::INTERFACE, key(iface, "out_1"));
    let out_r = end(Graph::INTERFACE, key(iface, "out_2"));
    let plug = |topology: &mut Topology, on: bool| {
        if on {
            topology
                .connect(end(synth, key(sd, "out_l")), out_l)
                .unwrap();
            topology
                .connect(end(synth, key(sd, "out_r")), out_r)
                .unwrap();
        } else {
            topology.disconnect(out_l).unwrap();
            topology.disconnect(out_r).unwrap();
        }
    };
    let submit = |handle: &mut atomik_engine::EngineHandle, topology: &mut Topology| {
        handle.fill_latencies(topology);
        handle.submit_routing(topology.compile().unwrap()).unwrap();
    };
    plug(&mut topology, true);
    submit(&mut handle, &mut topology);
    handle
        .send_note(ScheduledNote {
            frame: 0,
            device: synth,
            socket: key(sd, "notes_in"),
            kind: NoteKind::On {
                key: 57,
                velocity: 0.9,
            },
        })
        .unwrap();
    let render = |engine: &mut atomik_engine::Engine, frames: usize| -> Vec<f32> {
        let mut left = vec![0.0f32; frames];
        let mut right = vec![0.0f32; frames];
        for (l, r) in left.chunks_mut(BATCH).zip(right.chunks_mut(BATCH)) {
            engine.render(&[], &mut [l, r]);
        }
        left
    };
    let before = render(&mut engine, 14_400);
    plug(&mut topology, false);
    submit(&mut handle, &mut topology);
    render(&mut engine, 9_600);
    plug(&mut topology, true);
    submit(&mut handle, &mut topology);
    // The first batches carry the engine's output latency.
    render(&mut engine, BATCH * 2);
    (before, render(&mut engine, 48_000))
}

#[test]
fn a_synth_that_was_unplugged_while_it_held_a_note_plays_no_old_note_when_it_returns() {
    let (before, pruned) = heard_after_replug(true);
    assert!(before.iter().any(|x| x.abs() > 0.01), "the note sounds");
    assert!(
        pruned.iter().all(|x| x.to_bits() == 0),
        "no old note after the replug"
    );
    // The control: without the pruning the synth kept running, the note is still held.
    let (_, kept) = heard_after_replug(false);
    assert!(
        kept.iter().any(|x| x.abs() > 0.01),
        "the same edit without the pruning still plays the note"
    );
}

/// Which device types can be left out at all (printed): the ones that publish nothing themselves.
#[test]
fn the_device_types_that_can_be_left_out_are_listed() {
    let registry = default_registry();
    let (mut free, mut pinned) = (Vec::new(), Vec::new());
    let mut ids: Vec<&str> = registry.type_ids().collect();
    ids.sort_unstable();
    for ty in ids {
        let device = registry.create(ty).unwrap();
        let observed = device.has_meter_values()
            || device.scope_tap().is_some()
            || device.midi_tap().is_some()
            || device.variable_latency()
            || device.pinned_to_audio_thread();
        if observed {
            pinned.push(ty.to_owned());
        } else {
            free.push(format!("{ty} ({:?})", device.descriptor().kind));
        }
    }
    println!(
        "can be left out ({}): {free:?}\nalways kept ({}): {pinned:?}",
        free.len(),
        pinned.len()
    );
    assert!(!free.is_empty());
}
