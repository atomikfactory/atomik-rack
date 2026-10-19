//! The starter document (Audio I/O, the locked master with `main` cabled to
//! outputs 1 and 2) with instruments and effects added through the real model commands, so the
//! documents carry the channels, console links and folded strips a user's project has.
#![allow(dead_code, reason = "each test binary uses a subset")]

use super::*;
use atomik_model::{Command, Document, RackDoc, new_project_doc};

/// The starter project: the master first, nothing else.
pub fn starter() -> Document {
    Document::new(new_project_doc())
}

/// Applies `commands`, panicking on a rejection.
pub fn run(d: &mut Document, reg: &Registry, commands: impl IntoIterator<Item = Command>) {
    for c in commands {
        d.apply(c, reg).expect("command applies");
    }
}

/// `AddDevice` at the end of the rack with a fixed key.
pub fn add_end(type_id: &str, k: &str) -> Command {
    Command::AddDevice {
        index: usize::MAX,
        type_id: type_id.to_string(),
        key: Some(key(k)),
    }
}

/// An instrument made the way the browser makes it (`CreateInstrument`): its channel is folded.
pub fn create_end(type_id: &str, k: &str) -> Command {
    Command::Workflow(atomik_model::WorkflowCommand::CreateInstrument {
        index: usize::MAX,
        source: atomik_model::CreateSource::Type(type_id.to_string()),
        key: Some(key(k)),
        options: atomik_model::CreateOptions::default(),
    })
}

/// The starter with two synths (`synth`, `bass`; channels `channel`, `channel-2`, folded).
pub fn two_instruments(reg: &Registry) -> Document {
    let mut d = starter();
    run(
        &mut d,
        reg,
        [
            create_end("atomik.poly-synth", "synth"),
            create_end("atomik.poly-synth", "bass"),
        ],
    );
    d
}

/// The starter with two synths, a reverb (`verb`, loose) and a delay on return 1 (`echo`).
pub fn with_effects(reg: &Registry) -> Document {
    let mut d = two_instruments(reg);
    run(
        &mut d,
        reg,
        [
            add_end("atomik.reverb", "verb"),
            add_end("atomik.stereo-delay", "echo"),
            Command::Console(atomik_model::ConsoleCommand::SetReturn {
                slot: 0,
                effect: Some(key("echo")),
            }),
        ],
    );
    d
}

pub fn doc_of(d: &Document) -> RackDoc {
    d.doc().clone()
}

/// Compares `actual` with the golden `tests/golden/<name>`; `ATOMIK_UPDATE_GOLDEN=1` rewrites it.
pub fn check_golden(name: &str, actual: &str) {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    if std::env::var_os("ATOMIK_UPDATE_GOLDEN").is_some_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| {
            panic!(
                "missing golden {}; run with ATOMIK_UPDATE_GOLDEN=1",
                path.display()
            )
        })
        .replace("\r\n", "\n");
    if expected != actual {
        let line = expected
            .lines()
            .zip(actual.lines())
            .position(|(a, b)| a != b)
            .unwrap_or(expected.lines().count().min(actual.lines().count()));
        panic!(
            "{name} differs from golden at line {}:\n  golden: {:?}\n  actual: {:?}\n(regenerate with ATOMIK_UPDATE_GOLDEN=1)",
            line + 1,
            expected.lines().nth(line),
            actual.lines().nth(line)
        );
    }
}

/// The display list of a full paint of `v`.
pub fn paint_json(v: &mut atomik_ui::RackView, doc: &RackDoc, reg: &Registry) -> String {
    let mut dl = atomik_ui::DisplayList::new();
    v.paint(&mut dl, doc, reg);
    dl.to_json()
}
