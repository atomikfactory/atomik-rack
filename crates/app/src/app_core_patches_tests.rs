//! Device patches in the app core: a saved device patch applied to another device (one undo
//! step, the file name as the patch name), a mismatch changing nothing, settings copied and
//! pasted, reset, and the previous and next patch of a folder. Every test uses its own
//! temporary library folder.

use super::*;
use crate::sink::RecordingSink;
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_library::device_uri;
use atomik_model::{ZoneDoc, empty_doc, save_project};
use std::sync::atomic::{AtomicU64, Ordering};

const SYNTH: &str = "atomik.poly-synth";
const DELAY: &str = "atomik.stereo-delay";

struct Tmp(PathBuf);

impl Tmp {
    fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-app-patches-{tag}-{}-{n}",
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

fn core(lib: &Tmp) -> AppCore<RecordingSink> {
    let mut c = AppCore::with_registry(RecordingSink::new(), default_registry(), empty_doc(), None);
    c.view_mut().set_show_console_devices(false);
    c.set_library_root(Some(lib.path("Library")));
    c
}

fn add(c: &mut AppCore<RecordingSink>, type_id: &str, k: &str) {
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: type_id.to_string(),
        key: Some(key(k)),
    })
    .unwrap();
}

fn set(c: &mut AppCore<RecordingSink>, k: &str, param: &str, value: f32) {
    c.apply_command(Command::SetParam {
        key: key(k),
        param: param.to_string(),
        value,
        merge: None,
    })
    .unwrap();
}

fn select(c: &mut AppCore<RecordingSink>, k: &str) {
    c.after_view_change();
    c.view_mut().set_selection(&[key(k)], None);
}

fn param(c: &AppCore<RecordingSink>, k: &str, p: &str) -> Option<f32> {
    c.doc().device(&key(k)).unwrap().params.get(p).copied()
}

fn patch_path(lib: &Tmp, name: &str) -> PathBuf {
    lib.path(&format!("Library/Patches/{name}.atomikpatch"))
}

/// Two synths: `a` with the cutoff at `cutoff`, `b` untouched.
fn two_synths(lib: &Tmp, cutoff: f32) -> AppCore<RecordingSink> {
    let mut c = core(lib);
    add(&mut c, SYNTH, "a");
    add(&mut c, SYNTH, "b");
    set(&mut c, "a", "filter.cutoff", cutoff);
    c
}

#[test]
fn a_saved_patch_applies_to_another_device_as_one_undo_step() {
    let lib = Tmp::new("apply");
    let mut c = two_synths(&lib, 1234.0);
    c.save_device_patch(&key("a"), &patch_path(&lib, "Warm Pad"));
    // The device that was saved shows the name.
    assert_eq!(c.doc().device(&key("a")).unwrap().patch_name, "Warm Pad");
    let before = save_project(c.doc());
    c.load_library_item(&patch_path(&lib, "Warm Pad"), LoadTarget::Device(key("b")));
    assert_eq!(param(&c, "b", "filter.cutoff"), Some(1234.0));
    assert_eq!(c.doc().device(&key("b")).unwrap().patch_name, "Warm Pad");
    assert!(c.undo());
    assert_eq!(
        save_project(c.doc()),
        before,
        "one undo step restores the document"
    );
}

#[test]
fn a_patch_for_another_type_says_so_and_changes_nothing() {
    let lib = Tmp::new("mismatch");
    let mut c = two_synths(&lib, 1234.0);
    add(&mut c, DELAY, "fx");
    c.save_device_patch(&key("a"), &patch_path(&lib, "Lead"));
    let before = save_project(c.doc());
    c.load_library_item(&patch_path(&lib, "Lead"), LoadTarget::Device(key("fx")));
    let message = c.last_rejection().expect("a message").to_string();
    assert!(
        message.contains("is a patch for") && message.contains("Poly Synth"),
        "{message}"
    );
    assert_eq!(save_project(c.doc()), before);
}

#[test]
fn a_patch_of_several_devices_and_a_device_row_are_refused_on_a_device() {
    let lib = Tmp::new("several");
    let mut c = two_synths(&lib, 900.0);
    c.after_view_change();
    c.view_mut()
        .set_selection(&[key("a"), key("b")], Some(&key("a")));
    c.save_selection_to_library();
    let before = save_project(c.doc());
    c.load_library_item(
        &patch_path(&lib, "Poly Synth+1"),
        LoadTarget::Device(key("b")),
    );
    assert!(
        c.last_rejection().unwrap().contains("exactly one"),
        "{:?}",
        c.last_rejection()
    );
    c.load_library_item(&device_uri(SYNTH), LoadTarget::Device(key("b")));
    assert!(c.last_rejection().unwrap().contains("not a device patch"));
    assert_eq!(save_project(c.doc()), before);
}

#[test]
fn a_patch_without_a_name_takes_the_file_name() {
    let lib = Tmp::new("stem");
    let mut c = two_synths(&lib, 1500.0);
    c.save_device_patch(&key("a"), &patch_path(&lib, "Lead"));
    // The same text under another file name, with the name field removed.
    let text = std::fs::read_to_string(patch_path(&lib, "Lead")).unwrap();
    let nameless = text.replace(", \"patch\": \"Lead\"", "");
    assert_ne!(text, nameless);
    std::fs::write(patch_path(&lib, "Bright Lead"), nameless).unwrap();
    c.load_library_item(
        &patch_path(&lib, "Bright Lead"),
        LoadTarget::Device(key("b")),
    );
    assert_eq!(c.doc().device(&key("b")).unwrap().patch_name, "Bright Lead");
}

#[test]
fn saving_over_an_existing_patch_replaces_the_file() {
    let lib = Tmp::new("overwrite");
    let mut c = two_synths(&lib, 1000.0);
    c.save_device_patch(&key("a"), &patch_path(&lib, "Pad"));
    set(&mut c, "a", "filter.cutoff", 3000.0);
    c.save_device_patch(&key("a"), &patch_path(&lib, "Pad"));
    c.load_library_item(&patch_path(&lib, "Pad"), LoadTarget::Device(key("b")));
    assert_eq!(param(&c, "b", "filter.cutoff"), Some(3000.0));
}

#[test]
fn settings_copy_to_the_clipboard_and_paste_onto_a_device_of_the_same_type() {
    let lib = Tmp::new("settings");
    let mut c = two_synths(&lib, 777.0);
    add(&mut c, DELAY, "fx");
    select(&mut c, "a");
    c.copy_settings();
    assert_eq!(
        c.clipboard().kind(),
        Some(crate::clipboard::ClipboardKind::Settings)
    );
    // A device of another type cannot take them.
    select(&mut c, "fx");
    assert!(!c.can_paste_settings());
    let before = save_project(c.doc());
    c.paste_settings();
    assert!(
        c.last_rejection().unwrap().contains("Poly Synth"),
        "{:?}",
        c.last_rejection()
    );
    assert_eq!(save_project(c.doc()), before);
    // A synth can.
    select(&mut c, "b");
    assert!(c.can_paste_settings());
    c.paste_settings();
    assert_eq!(param(&c, "b", "filter.cutoff"), Some(777.0));
    assert!(c.undo());
    assert_eq!(param(&c, "b", "filter.cutoff"), None);
    // The plain Paste does not take settings.
    assert!(!c.can_paste(Pane::Rack));
}

#[test]
fn reset_puts_the_selected_device_back_to_a_new_one() {
    let lib = Tmp::new("reset");
    let mut c = two_synths(&lib, 777.0);
    select(&mut c, "a");
    c.reset_selected_device();
    assert_eq!(param(&c, "a", "filter.cutoff"), None);
    assert!(c.undo());
    assert_eq!(param(&c, "a", "filter.cutoff"), Some(777.0));
}

#[test]
fn previous_and_next_walk_the_patches_of_the_type_in_natural_order_and_wrap() {
    let lib = Tmp::new("step");
    let mut c = core(&lib);
    add(&mut c, SYNTH, "src");
    add(&mut c, DELAY, "fx");
    add(&mut c, SYNTH, "d");
    for (name, cutoff) in [("Pad 2", 200.0), ("Pad 10", 1000.0), ("Pad 1", 100.0)] {
        set(&mut c, "src", "filter.cutoff", cutoff);
        c.save_device_patch(&key("src"), &patch_path(&lib, name));
    }
    // A patch of another type in the same folder is skipped.
    c.save_device_patch(&key("fx"), &patch_path(&lib, "Echo"));
    std::fs::write(lib.path("Library/Patches/notes.txt"), "x").unwrap();
    let cutoff = |c: &AppCore<RecordingSink>| param(c, "d", "filter.cutoff");

    // Without a patch behind it, next is the first.
    c.step_patch(&key("d"), 1);
    assert_eq!(cutoff(&c), Some(100.0));
    c.step_patch(&key("d"), 1);
    assert_eq!(cutoff(&c), Some(200.0));
    c.step_patch(&key("d"), 1);
    assert_eq!(cutoff(&c), Some(1000.0));
    c.step_patch(&key("d"), 1);
    assert_eq!(cutoff(&c), Some(100.0), "wraps around");
    c.step_patch(&key("d"), -1);
    assert_eq!(cutoff(&c), Some(1000.0), "wraps back");
    assert_eq!(c.doc().device(&key("d")).unwrap().patch_name, "Pad 10");
    // Every step is its own undo step.
    assert!(c.undo());
    assert_eq!(cutoff(&c), Some(100.0));
}

#[test]
fn stepping_with_no_patches_for_the_type_says_so() {
    let lib = Tmp::new("none");
    let mut c = two_synths(&lib, 500.0);
    std::fs::create_dir_all(lib.path("Library/Patches")).unwrap();
    let before = save_project(c.doc());
    c.step_patch(&key("b"), 1);
    assert!(
        c.last_rejection().unwrap().contains("no patches"),
        "{:?}",
        c.last_rejection()
    );
    assert_eq!(save_project(c.doc()), before);
}

#[test]
fn the_save_dialog_starts_in_the_patches_folder_with_the_patch_name() {
    let lib = Tmp::new("suggest");
    let mut c = two_synths(&lib, 500.0);
    let (dir, name) = c.patch_save_suggestion(&key("a")).unwrap();
    assert_eq!(dir, lib.path("Library/Patches"));
    assert_eq!(name, "Poly Synth.atomikpatch");
    c.save_device_patch(&key("a"), &patch_path(&lib, "Warm Pad"));
    assert_eq!(
        c.patch_save_suggestion(&key("a")).unwrap().1,
        "Warm Pad.atomikpatch"
    );
}

#[test]
fn a_sampler_patch_registers_its_audio_in_another_project() {
    let lib = Tmp::new("audio");
    let wav = lib.path("kick.wav");
    let data: Vec<f32> = (0..4_800).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
    write_wav_f32(&wav, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    let mut a = core(&lib);
    a.import_audio(vec![wav], atomik_ui::ImportTarget::NewTracks, 0);
    a.wait_background();
    let hash = a.doc().assets.keys().next().expect("an asset").clone();
    add(&mut a, "atomik.sampler", "s");
    a.apply_command(Command::SetZones {
        device: key("s"),
        zones: vec![ZoneDoc::new(hash.clone(), 4_800)],
        merge: None,
    })
    .unwrap();
    a.save_device_patch(&key("s"), &patch_path(&lib, "Kit"));
    for _ in 0..3_000 {
        a.frame();
        if !a.browser_busy() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    // Another project with a sampler of its own takes the patch: the sample enters its pool
    // and the audio arrives through the adopt worker.
    let mut b = core(&lib);
    add(&mut b, "atomik.sampler", "t");
    b.load_library_item(&patch_path(&lib, "Kit"), LoadTarget::Device(key("t")));
    b.wait_background();
    assert!(b.doc().assets.contains_key(&hash));
    let zones = &b.doc().device(&key("t")).unwrap().zones;
    assert_eq!(zones.len(), 1);
    assert_eq!(zones[0].asset, hash);
    assert!(b.undo());
    assert!(b.doc().device(&key("t")).unwrap().zones.is_empty());
}
