//! Cost of the rack and console commands on a song of growing size: `AddDevice`,
//! `AddAudioTrack`, `AddChannel`, `Connect` and `PasteDevices`, each applied and undone, on a song
//! of 10 tracks and 100 clips holding 10^3, 10^5 and 10^6 notes. These commands never touch a
//! note, so the cost should not grow with the song. Run:
//! `cargo bench -p atomik-model --bench commands`; a filter argument selects cases
//! (`-- addchannel/n1000000`). `ATOMIK_BENCH_ONCE` runs each case once (for an instruction counter).
#![allow(
    clippy::disallowed_methods,
    reason = "wall-clock timing is the point of a benchmark"
)]

use atomik_device_sdk::Registry;
use atomik_model::stress::large_song_doc;
use atomik_model::{
    Command, ConsoleCommand, ContainerCommand, DeviceKey, DeviceSnapshot, Document, SongCommand,
    WorkflowCommand, empty_doc,
};
use atomik_testkit::bench::{Recorder, SAMPLES, once_mode, time_once};
use criterion::Criterion;
use std::rc::Rc;
use std::time::Duration;

type Case = (String, Box<dyn FnMut() -> usize>);

fn registry() -> Registry {
    let mut r = Registry::new();
    atomik_devices::register_all(&mut r).expect("built-in devices register");
    r
}

/// Applies `cmd`, undoes it and returns the number of ops reported.
fn round_trip(d: &mut Document, reg: &Registry, cmd: Command) -> usize {
    let ops = d.apply(cmd, reg).expect("a valid command");
    ops.len() + d.undo(reg).map_or(0, |u| u.len())
}

fn cases_of(notes: usize, reg: &Rc<Registry>, out: &mut Vec<Case>) {
    let doc = large_song_doc(reg, 10, 100, notes);
    let source = DeviceKey("t001".into());
    let cable = doc.cables[0].clone();
    let snapshot = DeviceSnapshot::capture(&doc, std::slice::from_ref(&source), true)
        .expect("a track device can be copied");
    let tag = format!("n{notes}");
    let add = |name: &str, out: &mut Vec<Case>, mut f: Box<dyn FnMut(&mut Document) -> usize>| {
        let mut d = Document::new(doc.clone());
        out.push((format!("{name}/{tag}"), Box::new(move || f(&mut d))));
    };
    let r = Rc::clone(reg);
    add(
        "adddevice",
        out,
        Box::new(move |d| {
            let cmd = Command::AddDevice {
                index: usize::MAX,
                type_id: "atomik.poly-synth".into(),
                key: None,
            };
            round_trip(d, &r, cmd)
        }),
    );
    let r = Rc::clone(reg);
    add(
        "addaudiotrack",
        out,
        Box::new(move |d| {
            let cmd = Command::Song(SongCommand::AddAudioTrack { name: None });
            round_trip(d, &r, cmd)
        }),
    );
    let (r, src) = (Rc::clone(reg), source.clone());
    add(
        "addchannel",
        out,
        Box::new(move |d| {
            let cmd = Command::Console(ConsoleCommand::AddChannel {
                source: src.clone(),
            });
            // The source already has its channel: the command may be refused; it is the
            // planning that is measured.
            let ops = d.apply(cmd, &r).map_or(0, |o| o.len());
            ops + d.undo(&r).map_or(0, |u| u.len())
        }),
    );
    let r = Rc::clone(reg);
    add(
        "connect",
        out,
        Box::new(move |d| {
            let a = d
                .apply(
                    Command::Disconnect {
                        to: cable.to.clone(),
                    },
                    &r,
                )
                .expect("the cable exists")
                .len();
            let b = d
                .apply(Command::Connect(cable.clone()), &r)
                .expect("the cable fits again")
                .len();
            a + b + d.undo(&r).map_or(0, |u| u.len()) + d.undo(&r).map_or(0, |u| u.len())
        }),
    );
    let r = Rc::clone(reg);
    add(
        "paste",
        out,
        Box::new(move |d| {
            let cmd = Command::Workflow(WorkflowCommand::PasteDevices {
                route: false,
                snapshot: snapshot.clone(),
                container: None,
                at: usize::MAX,
            });
            round_trip(d, &r, cmd)
        }),
    );
}

/// A rack of `rack` devices and one full container (32 inner devices): adding an instrument
/// (which cables its channel) must not pay for the size of the graph at every cable.
fn container_case(rack: usize, reg: &Rc<Registry>, out: &mut Vec<Case>) {
    let mut d = Document::new(empty_doc());
    let mut apply = |c: Command| {
        d.apply(c, reg).expect("a valid setup command");
    };
    apply(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.container".into(),
        key: Some(DeviceKey("box".into())),
    });
    for i in 0..rack {
        apply(Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.stereo-delay".into(),
            key: Some(DeviceKey(format!("r{i:03}"))),
        });
    }
    for i in 0..32 {
        apply(Command::Container(ContainerCommand::AddInner {
            container: DeviceKey("box".into()),
            index: usize::MAX,
            type_id: "atomik.stereo-delay".into(),
            key: Some(DeviceKey(format!("x{i:03}"))),
            auto_split: false,
        }));
    }
    let reg = Rc::clone(reg);
    out.push((
        format!("adddevice/container-rack{rack}"),
        Box::new(move || {
            let cmd = Command::AddDevice {
                index: usize::MAX,
                type_id: "atomik.poly-synth".into(),
                key: None,
            };
            round_trip(&mut d, &reg, cmd)
        }),
    ));
}

fn main() {
    let reg = Rc::new(registry());
    let mut all = Vec::new();
    container_case(100, &reg, &mut all);
    for notes in [1_000, 100_000, 1_000_000] {
        cases_of(notes, &reg, &mut all);
    }
    if once_mode() {
        for (name, f) in &mut all {
            println!("{name}: once {:.6} s", time_once(f));
        }
        return;
    }
    let mut c = Criterion::default()
        .sample_size(SAMPLES)
        .warm_up_time(Duration::from_millis(100))
        .measurement_time(Duration::from_millis(500))
        .configure_from_args();
    for (name, f) in &mut all {
        let id = format!("commands/{name}");
        let rec = Recorder::new();
        c.bench_function(&id, |b| b.iter_custom(rec.runner(&mut *f)));
        rec.report(&id);
    }
    c.final_summary();
}
