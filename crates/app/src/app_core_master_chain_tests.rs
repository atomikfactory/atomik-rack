//! The master chain in the app core: saving it to the library, loading it back onto the master
//! (from the menu command, a double click or a drop), the category the browser lists it under,
//! and the one undo step a load is.

use super::master_chain::MASTER_CHAINS_CATEGORY;
use super::*;
use crate::plugin_test_support::{GAIN, PKG, add_command, config};
use crate::sink::RecordingSink;
use atomik_model::{
    ConsoleCommand, CreateOptions, CreateSource, CreateTarget, InsertEffect, PluginCommand,
    WorkflowCommand, capture_master_chain, empty_doc, insert_chain, master_stack, send_chain,
};
use std::sync::atomic::{AtomicU64, Ordering};

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-app-master-chain-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self, rel: &str) -> PathBuf {
        self.0.join(rel)
    }
}

impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

/// A core with a master (made by its first instrument) and `<tmp>/Library` as the library.
fn core(lib: &Tmp) -> AppCore<RecordingSink> {
    let mut c = AppCore::with_registry(RecordingSink::new(), default_registry(), empty_doc(), None);
    c.set_library_root(Some(lib.path("Library")));
    c.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
        index: usize::MAX,
        source: CreateSource::Type("atomik.poly-synth".into()),
        key: Some(key("synth")),
        options: CreateOptions::default(),
    }))
    .unwrap();
    c
}

fn master(c: &AppCore<RecordingSink>) -> DeviceKey {
    c.doc().console.master.clone().expect("a master")
}

fn insert(c: &mut AppCore<RecordingSink>, slot: usize, type_id: &str) {
    let channel = master(c);
    c.apply_command(Command::Console(ConsoleCommand::AddInsert {
        channel,
        slot,
        effect: InsertEffect::New {
            type_id: type_id.into(),
        },
    }))
    .unwrap();
}

fn send(c: &mut AppCore<RecordingSink>, type_id: &str) {
    c.apply_command(Command::Workflow(WorkflowCommand::CreateEffect {
        source: CreateSource::Type(type_id.into()),
        target: CreateTarget::Master,
        key: None,
        options: CreateOptions::default(),
    }))
    .unwrap();
}

/// The master section as a list: type and name of each effect, then the send slots in use.
fn shape(c: &AppCore<RecordingSink>) -> Vec<String> {
    let doc = c.doc();
    let chain = capture_master_chain(doc, c.registry()).unwrap();
    let mut out: Vec<String> = chain
        .snapshot
        .devices
        .iter()
        .map(|d| format!("{} {} {:?}", d.type_id, d.name, d.params))
        .collect();
    out.push(format!("inserts {}", chain.inserts));
    out.push(format!("sends {:?}", chain.return_params));
    out.push(format!(
        "slots {:?}",
        (0..8)
            .map(|s| send_chain(doc, c.registry(), s).len())
            .collect::<Vec<_>>()
    ));
    out
}

fn text(c: &AppCore<RecordingSink>) -> String {
    atomik_model::save_project(c.doc())
}

/// A core whose master has two inserts and two sends (the second with two effects).
fn chained(lib: &Tmp) -> AppCore<RecordingSink> {
    let mut c = core(lib);
    insert(&mut c, 0, "atomik.eq");
    insert(&mut c, 1, "atomik.distortion");
    send(&mut c, "atomik.reverb");
    send(&mut c, "atomik.stereo-delay");
    let second = send_chain(c.doc(), c.registry(), 1)[0].clone();
    c.apply_command(Command::Workflow(WorkflowCommand::CreateEffect {
        source: CreateSource::Type("atomik.eq".into()),
        target: CreateTarget::Selected(second),
        key: None,
        options: CreateOptions::default(),
    }))
    .unwrap();
    c
}

#[test]
fn saving_writes_a_patch_in_the_master_chains_category_and_changes_nothing_else() {
    let lib = Tmp::new("save");
    let mut c = chained(&lib);
    let before = text(&c);
    let path = lib.path("Library/Patches/Warm Bus.atomikpatch");
    c.save_master_chain(&path);
    assert!(path.is_file(), "{:?}", c.last_rejection());
    assert_eq!(text(&c), before, "saving is no edit");
    let mark = c.library_mark(&path);
    assert_eq!(
        mark.categories
            .iter()
            .map(|t| t.as_str())
            .collect::<Vec<_>>(),
        [MASTER_CHAINS_CATEGORY]
    );
    let item = std::fs::read_to_string(&path).unwrap();
    assert!(atomik_model::is_master_chain_item(&item));
    let s = atomik_model::summarize_item(atomik_model::ItemKind::Patch, &item).unwrap();
    assert_eq!(s.detail, "Master chain: 5 effects, 2 sends");
}

#[test]
fn a_file_outside_the_library_gets_no_category() {
    let lib = Tmp::new("outside");
    let mut c = chained(&lib);
    let path = lib.path("elsewhere/chain.atomikpatch");
    c.save_master_chain(&path);
    assert!(path.is_file());
    assert!(c.library_mark(&path).categories.is_empty());
}

#[test]
fn a_name_without_the_extension_gets_it() {
    let lib = Tmp::new("ext");
    let mut c = chained(&lib);
    c.save_master_chain(&lib.path("Library/Patches/Plain"));
    assert!(lib.path("Library/Patches/Plain.atomikpatch").is_file());
}

#[test]
fn loading_replaces_the_masters_effects_in_one_undo_step() {
    let lib = Tmp::new("load");
    let source = chained(&lib);
    let path = lib.path("Library/Patches/Chain.atomikpatch");
    let mut source = source;
    source.save_master_chain(&path);
    let want = shape(&source);
    // Another project with other effects on its master.
    let lib2 = Tmp::new("load2");
    let mut c = core(&lib2);
    insert(&mut c, 0, "atomik.reverb");
    send(&mut c, "atomik.eq");
    assert_ne!(shape(&c), want);
    let before = text(&c);
    c.load_master_chain(&path);
    assert_eq!(shape(&c), want, "{:?}", c.last_rejection());
    assert!(
        c.last_rejection()
            .unwrap()
            .contains("Loaded master chain 'Chain': 5 effects, 2 sends"),
        "{:?}",
        c.last_rejection()
    );
    let m = master(&c);
    let all: Vec<DeviceKey> = c.doc().devices.iter().map(|d| d.key.clone()).collect();
    let at = all.iter().position(|k| *k == m).unwrap();
    let stack = master_stack(c.doc(), c.registry());
    assert_eq!(stack.len(), 5);
    assert_eq!(&all[at + 1..at + 6], &stack[..]);
    assert!(c.undo(), "one step to undo");
    assert_eq!(text(&c), before, "Undo puts the old chain back");
}

#[test]
fn a_double_click_or_a_drop_on_the_master_loads_the_chain_too() {
    let lib = Tmp::new("targets");
    let mut source = chained(&lib);
    let path = lib.path("Library/Patches/Chain.atomikpatch");
    source.save_master_chain(&path);
    let want = shape(&source);
    for (tag, target) in [
        ("default", LoadTarget::Default),
        ("replace", LoadTarget::Replace(key("master"))),
        ("device", LoadTarget::Device(key("master"))),
        (
            "rack",
            LoadTarget::Rack {
                container: None,
                slot: atomik_model::RackSlot {
                    column: 0,
                    index: 0,
                },
            },
        ),
    ] {
        let lib2 = Tmp::new(tag);
        let mut c = core(&lib2);
        assert_eq!(master(&c), key("master"));
        c.load_library_item(&path, target);
        assert_eq!(shape(&c), want, "{tag}: {:?}", c.last_rejection());
    }
}

#[test]
fn a_drop_on_another_device_is_refused_and_changes_nothing() {
    let lib = Tmp::new("refuse");
    let mut source = chained(&lib);
    let path = lib.path("Library/Patches/Chain.atomikpatch");
    source.save_master_chain(&path);
    let mut c = core(&Tmp::new("refuse2"));
    let before = text(&c);
    c.load_library_item(&path, LoadTarget::Replace(key("synth")));
    assert_eq!(text(&c), before);
    assert!(
        c.last_rejection().unwrap().contains("master section"),
        "{:?}",
        c.last_rejection()
    );
}

#[test]
fn loading_a_patch_that_is_no_master_chain_says_so() {
    let lib = Tmp::new("notchain");
    let mut c = chained(&lib);
    c.after_view_change();
    c.view_mut().set_selection(&[key("synth")], None);
    c.save_selection_to_library();
    let patch = lib.path("Library/Patches/Poly Synth.atomikpatch");
    assert!(patch.is_file(), "{:?}", c.last_rejection());
    let before = text(&c);
    c.load_master_chain(&patch);
    assert_eq!(text(&c), before);
    assert!(
        c.last_rejection()
            .unwrap()
            .contains("is not a master chain patch"),
        "{:?}",
        c.last_rejection()
    );
}

#[test]
fn saving_without_a_master_says_so() {
    let lib = Tmp::new("nomaster");
    let mut c = AppCore::with_registry(RecordingSink::new(), default_registry(), empty_doc(), None);
    c.set_library_root(Some(lib.path("Library")));
    let path = lib.path("Library/Patches/None.atomikpatch");
    c.save_master_chain(&path);
    assert!(!path.exists());
    assert!(
        c.last_rejection().unwrap().contains("no master"),
        "{:?}",
        c.last_rejection()
    );
}

#[test]
fn the_save_dialog_starts_in_the_patches_folder() {
    let lib = Tmp::new("suggest");
    let c = core(&lib);
    let (dir, name) = c.master_chain_save_suggestion().unwrap();
    assert_eq!(dir, lib.path("Library/Patches"));
    assert_eq!(name, "Master Chain.atomikpatch");
    assert_eq!(c.master_chain_folder(), Some(dir));
    let bare = AppCore::with_registry(RecordingSink::new(), default_registry(), empty_doc(), None);
    assert!(bare.master_chain_save_suggestion().is_none());
}

#[test]
fn a_plugin_effect_keeps_its_state_through_the_patch() {
    let lib = Tmp::new("plugin");
    let mut a = AppCore::with_doc(RecordingSink::new(), atomik_model::new_project_doc(), None);
    a.configure_plugins(config(true, &[PKG]));
    a.set_library_root(Some(lib.path("Library")));
    let add = add_command(&a, GAIN, "gain");
    a.apply_command(add).unwrap();
    let m = master(&a);
    a.apply_command(Command::Console(ConsoleCommand::AddInsert {
        channel: m,
        slot: 0,
        effect: InsertEffect::Existing(key("gain")),
    }))
    .unwrap();
    a.apply_command(Command::Plugin(PluginCommand::SyncState {
        device: key("gain"),
        state: atomik_device_sdk::PluginStateBlob(b"chain state".to_vec()),
    }))
    .unwrap();
    let path = lib.path("Library/Patches/Plugin Chain.atomikpatch");
    a.save_master_chain(&path);
    assert!(path.is_file(), "{:?}", a.last_rejection());
    let mut b = AppCore::with_doc(RecordingSink::new(), atomik_model::new_project_doc(), None);
    b.configure_plugins(config(true, &[PKG]));
    b.load_master_chain(&path);
    let m = master(&b);
    let chain = insert_chain(b.doc(), b.registry(), &m);
    assert_eq!(chain.len(), 1, "{:?}", b.last_rejection());
    let state = b
        .doc()
        .device(&chain[0])
        .and_then(|d| d.plugin.as_ref())
        .and_then(|p| p.state.clone())
        .map(|s| s.0.clone());
    assert_eq!(state, Some(b"chain state".to_vec()));
}
