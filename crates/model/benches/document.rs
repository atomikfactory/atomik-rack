//! Cost of the document operations the editor repeats: saving and loading a project, validating
//! it, expanding the rack, cloning the document, and one edit with its undo, on three generated
//! documents (`rack`: 100 devices and 300 cables; `song`: the 8-track stress song; `large`: a
//! song of 100 tracks, 500 clips and 2,000 notes). Run:
//! `cargo bench -p atomik-model --bench document`; a filter argument selects cases
//! (`-- large/save`). Each case prints criterion's report and a line `median .. p99 ..`;
//! `ATOMIK_BENCH_ONCE` runs each case once (for an instruction counter).
#![allow(
    clippy::disallowed_methods,
    reason = "wall-clock timing is the point of a benchmark"
)]

use atomik_device_sdk::Registry;
use atomik_model::stress::{
    STRESS_CABLES, STRESS_DEVICES, large_song_doc, stress_doc, stress_song_doc,
};
use atomik_model::{Command, Document, RackDoc, load_project, save_project};
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

/// The cases of one generated document.
fn cases_of(name: &str, doc: RackDoc, reg: &Rc<Registry>) -> Vec<Case> {
    let mut out: Vec<Case> = Vec::new();
    let text = save_project(&doc);
    let synth = doc
        .devices
        .iter()
        .find(|d| d.type_id == "atomik.poly-synth")
        .map(|d| d.key.clone());
    {
        let doc = doc.clone();
        out.push((
            format!("{name}/save"),
            Box::new(move || save_project(&doc).len()),
        ));
    }
    {
        let (text, reg) = (text.clone(), Rc::clone(reg));
        out.push((
            format!("{name}/load"),
            Box::new(move || {
                load_project(&text, &reg)
                    .expect("generated project loads")
                    .devices
                    .len()
            }),
        ));
    }
    {
        let doc = doc.clone();
        out.push((
            format!("{name}/validate"),
            Box::new(move || usize::from(doc.validate().is_ok())),
        ));
    }
    {
        let doc = doc.clone();
        out.push((
            format!("{name}/clone"),
            Box::new(move || doc.clone().devices.len()),
        ));
    }
    {
        let (doc, reg) = (doc.clone(), Rc::clone(reg));
        out.push((
            format!("{name}/expand"),
            Box::new(move || doc.expand(&reg).devices.len()),
        ));
    }
    if let Some(key) = synth {
        let (mut d, reg) = (Document::new(doc), Rc::clone(reg));
        let mut flip = 0.0f32;
        out.push((
            format!("{name}/edit+undo"),
            Box::new(move || {
                flip = 1.0 - flip;
                let cmd = Command::SetParam {
                    key: key.clone(),
                    param: "master.level".into(),
                    value: -6.0 - flip,
                    merge: None,
                };
                let ops = d.apply(cmd, &reg).expect("a valid edit");
                let undone = d.undo(&reg).map_or(0, |u| u.len());
                ops.len() + undone
            }),
        ));
    }
    out
}

fn main() {
    let reg = Rc::new(registry());
    let mut all = Vec::new();
    all.extend(cases_of(
        "rack",
        stress_doc(&reg, STRESS_DEVICES, STRESS_CABLES),
        &reg,
    ));
    all.extend(cases_of("song", stress_song_doc(&reg), &reg));
    all.extend(cases_of(
        "large",
        large_song_doc(&reg, 100, 500, 2000),
        &reg,
    ));
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
        let id = format!("document/{name}");
        let rec = Recorder::new();
        c.bench_function(&id, |b| b.iter_custom(rec.runner(&mut *f)));
        rec.report(&id);
    }
    c.final_summary();
}
