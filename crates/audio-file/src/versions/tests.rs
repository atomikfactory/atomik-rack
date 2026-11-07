use super::*;
use std::sync::atomic::{AtomicU32, Ordering};

/// A scratch folder that is removed when the test ends.
struct Dir(PathBuf);

impl std::ops::Deref for Dir {
    type Target = PathBuf;
    fn deref(&self) -> &PathBuf {
        &self.0
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn dir(name: &str) -> Dir {
    static N: AtomicU32 = AtomicU32::new(0);
    let d = std::env::temp_dir()
        .join("atomik-audio-file-versions-unit")
        .join(format!(
            "{}-{}-{name}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    Dir(d)
}

fn hash(i: usize) -> String {
    format!("{i:064x}")
}

fn no_assets() -> BTreeSet<String> {
    BTreeSet::new()
}

fn auto(store: &mut VersionStore, text: &str, time: u64) -> u32 {
    store
        .add(VersionKind::Automatic, "", text, &no_assets(), time)
        .unwrap()
}

#[test]
fn the_folder_is_inside_the_bundle_or_beside_a_flat_project() {
    let root = dir("place");
    // A project in a bundle folder named like it.
    let own = root.join("song").join("song.atomik");
    assert_eq!(versions_dir(&own), root.join("song").join("Versions"));
    // A flat project with a sibling bundle folder.
    fs::create_dir_all(root.join("beat")).unwrap();
    assert_eq!(
        versions_dir(&root.join("beat.atomik")),
        root.join("beat").join("Versions")
    );
    // A flat project with no bundle: beside it, named after it.
    assert_eq!(
        versions_dir(&root.join("loop.atomik")),
        root.join("loop Versions")
    );
}

#[test]
fn an_existing_index_wins_over_a_bundle_folder_made_later() {
    let root = dir("late-bundle");
    let file = root.join("late.atomik");
    let mut store = VersionStore::for_project(&file).unwrap();
    auto(&mut store, "one", 1);
    assert_eq!(store.dir(), root.join("late Versions"));
    // The project gains a bundle folder: the history stays where it is.
    fs::create_dir_all(root.join("late")).unwrap();
    let again = VersionStore::for_project(&file).unwrap();
    assert_eq!(again.dir(), root.join("late Versions"));
    assert_eq!(again.entries().len(), 1);
}

#[test]
fn versions_are_kept_as_plain_files_and_an_index_that_reads_back() {
    let root = dir("round");
    let mut store = VersionStore::open(&root.join("Versions"), "song").unwrap();
    assert!(store.entries().is_empty());
    let mut assets = BTreeSet::new();
    assets.insert(hash(1));
    let a = store
        .add(VersionKind::Automatic, "", "first text", &assets, 100)
        .unwrap();
    let b = store
        .add(
            VersionKind::Named,
            "Before the mix",
            "second text",
            &no_assets(),
            200,
        )
        .unwrap();
    assert_eq!((a, b), (1, 2));
    assert!(store.set_protected(a, true).unwrap());
    assert!(!store.set_protected(99, true).unwrap());

    let back = VersionStore::open(&root.join("Versions"), "song").unwrap();
    assert_eq!(back.entries(), store.entries());
    assert_eq!(back.read(a).unwrap(), "first text");
    assert_eq!(back.read(b).unwrap(), "second text");
    let named = back.get(b).unwrap();
    assert_eq!(named.kind, VersionKind::Named);
    assert_eq!(named.name, "Before the mix");
    assert_eq!(named.time, 200);
    assert!(back.get(a).unwrap().protected);
    assert_eq!(back.assets(), assets);
    // The file is the project text, byte for byte, under a name that is not `.atomik`.
    let file = back.path_of(a).unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), "first text");
    assert_eq!(file.extension().unwrap(), VERSION_EXTENSION);
    // Numbers keep growing after a reopen.
    let mut back = back;
    assert_eq!(auto(&mut back, "third", 300), 3);
}

#[test]
fn names_are_cleaned_and_limited() {
    let root = dir("names");
    let mut store = VersionStore::open(&root, "song").unwrap();
    let long = "x".repeat(300);
    let id = store
        .add(
            VersionKind::Named,
            &format!("  a\tb\nc {long}"),
            "t",
            &no_assets(),
            1,
        )
        .unwrap();
    let name = &store.get(id).unwrap().name;
    assert!(name.starts_with("a b c x"), "{name:?}");
    assert_eq!(name.chars().count(), MAX_VERSION_NAME_CHARS);
    let back = VersionStore::open(&root, "song").unwrap();
    assert_eq!(&back.get(id).unwrap().name, name);
}

#[test]
fn prune_removes_only_the_oldest_unprotected_automatic_versions() {
    let root = dir("prune");
    let mut store = VersionStore::open(&root, "song").unwrap();
    let ids: Vec<u32> = (0..6)
        .map(|i| auto(&mut store, &format!("auto {i}"), i))
        .collect();
    let named = store
        .add(VersionKind::Named, "Keep me", "named", &no_assets(), 10)
        .unwrap();
    store.set_protected(ids[0], true).unwrap();

    let removed = store.prune(2).unwrap();
    // Six automatic, one protected: five count, so the three oldest of those go.
    assert_eq!(removed, vec![ids[1], ids[2], ids[3]]);
    let left: Vec<u32> = store.entries().iter().map(|v| v.id).collect();
    assert_eq!(left, vec![ids[0], ids[4], ids[5], named]);
    for gone in &removed {
        let name = format!("song v{gone:04}.{VERSION_EXTENSION}");
        assert!(!root.join(name).exists());
    }
    // The protected and the named files are still there, with their text.
    assert_eq!(store.read(ids[0]).unwrap(), "auto 0");
    assert_eq!(store.read(named).unwrap(), "named");
    // The index on disk agrees.
    let back = VersionStore::open(&root, "song").unwrap();
    assert_eq!(back.entries(), store.entries());
    // Pruning again changes nothing; keeping none leaves only what is kept.
    assert!(store.prune(2).unwrap().is_empty());
    store.prune(0).unwrap();
    let left: Vec<u32> = store.entries().iter().map(|v| v.id).collect();
    assert_eq!(left, vec![ids[0], named]);
}

#[test]
fn prune_never_touches_a_file_the_index_does_not_list() {
    let root = dir("unrelated");
    let mut store = VersionStore::open(&root, "song").unwrap();
    for i in 0..5 {
        auto(&mut store, &format!("auto {i}"), i);
    }
    // Strangers in the folder: other files, a version file nobody listed, a sub-folder.
    fs::write(root.join("notes.txt"), "mine").unwrap();
    fs::write(
        root.join(format!("song v0099.{VERSION_EXTENSION}")),
        "stray",
    )
    .unwrap();
    fs::write(root.join("song.atomik"), "a project").unwrap();
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::write(root.join("sub").join("deep.txt"), "deep").unwrap();
    // A file outside the folder with a name an index might try to reach.
    let outside = root.parent().unwrap().join("outside.txt");
    fs::write(&outside, "outside").unwrap();

    store.prune(1).unwrap();
    assert_eq!(store.entries().len(), 1);
    assert_eq!(fs::read_to_string(root.join("notes.txt")).unwrap(), "mine");
    assert_eq!(
        fs::read_to_string(root.join(format!("song v0099.{VERSION_EXTENSION}"))).unwrap(),
        "stray"
    );
    assert_eq!(
        fs::read_to_string(root.join("song.atomik")).unwrap(),
        "a project"
    );
    assert_eq!(
        fs::read_to_string(root.join("sub").join("deep.txt")).unwrap(),
        "deep"
    );
    assert_eq!(fs::read_to_string(&outside).unwrap(), "outside");
}

#[test]
fn an_index_that_names_a_path_or_a_foreign_file_cannot_reach_a_delete() {
    let root = dir("hostile");
    let victim = root.join("victim.txt");
    fs::write(&victim, "precious").unwrap();
    let inner = root.join("Versions");
    fs::create_dir_all(&inner).unwrap();
    fs::write(inner.join("plain.txt"), "plain").unwrap();
    let h = hash(7);
    let index = format!(
        "{HEADER}\nnext\t10\n\
         v\t1\ta\t0\t1\t..\\victim.txt\t\tx\n\
         v\t2\ta\t0\t2\t../victim.txt.atomikver\t\tx\n\
         v\t3\ta\t0\t3\tplain.txt\t\tx\n\
         v\t4\ta\t0\t4\t{INDEX_FILE}\t\tx\n\
         v\t5\ta\t0\t5\tok v0005.{VERSION_EXTENSION}\t{h},not-a-hash\tgood\n\
         v\tnine\ta\t0\t6\tbad.{VERSION_EXTENSION}\t\tx\n"
    );
    fs::write(inner.join(INDEX_FILE), index).unwrap();
    fs::write(inner.join(format!("ok v0005.{VERSION_EXTENSION}")), "ok").unwrap();

    let mut store = VersionStore::open(&inner, "song").unwrap();
    let ids: Vec<u32> = store.entries().iter().map(|v| v.id).collect();
    assert_eq!(ids, vec![5], "only the plain version file is listed");
    assert_eq!(store.get(5).unwrap().assets, vec![h]);
    store.prune(0).unwrap();
    assert!(store.entries().is_empty());
    assert_eq!(fs::read_to_string(&victim).unwrap(), "precious");
    assert_eq!(
        fs::read_to_string(inner.join("plain.txt")).unwrap(),
        "plain"
    );
    assert!(inner.join(INDEX_FILE).is_file());
}

#[test]
fn a_damaged_index_is_moved_aside_and_no_version_file_is_removed() {
    let root = dir("damaged");
    let mut store = VersionStore::open(&root, "song").unwrap();
    let id = auto(&mut store, "keep", 1);
    let file = store.path_of(id).unwrap();
    fs::write(root.join(INDEX_FILE), "this is not an index").unwrap();
    let mut again = VersionStore::open(&root, "song").unwrap();
    assert!(again.entries().is_empty());
    assert!(root.join(format!("{INDEX_FILE}.bad")).is_file());
    assert!(again.prune(0).unwrap().is_empty());
    assert_eq!(fs::read_to_string(file).unwrap(), "keep");
}

#[test]
fn the_newest_version_can_be_compared_with_a_text() {
    let root = dir("latest");
    let mut store = VersionStore::open(&root, "song").unwrap();
    assert!(!store.latest_is("a"));
    auto(&mut store, "a", 1);
    assert!(store.latest_is("a"));
    assert!(!store.latest_is("b"));
}

#[test]
fn scanning_a_project_text_finds_its_audio_files() {
    let text = format!(
        "{{\"assets\":{{\"{}\":{{\"name\":\"kick\"}},\"{}\":{{}}}},\"clip\":\"{}\",\"x\":\"abc\"}}",
        hash(1),
        hash(2),
        hash(1)
    );
    let found = scan_assets(&text);
    assert_eq!(found.len(), 2);
    assert!(found.contains(&hash(1)) && found.contains(&hash(2)));
}

#[test]
fn moving_carries_the_versions_and_removes_the_old_ones() {
    let root = dir("move");
    let old = root.join("old").join("song.atomik");
    let new = root.join("new").join("song").join("song.atomik");
    fs::create_dir_all(old.parent().unwrap()).unwrap();
    fs::create_dir_all(new.parent().unwrap()).unwrap();
    let mut store = VersionStore::for_project(&old).unwrap();
    let a = auto(&mut store, "auto text", 5);
    let b = store
        .add(
            VersionKind::Named,
            "Named one",
            "named text",
            &no_assets(),
            6,
        )
        .unwrap();
    store.set_protected(a, true).unwrap();
    // Something else in the old versions folder stays, and so does the folder.
    fs::write(store.dir().join("notes.txt"), "mine").unwrap();

    assert_eq!(carry_versions(&old, &new).unwrap(), 2);
    let moved = VersionStore::for_project(&new).unwrap();
    assert_eq!(moved.dir(), new.parent().unwrap().join("Versions"));
    let names: Vec<(&str, bool, VersionKind)> = moved
        .entries()
        .iter()
        .map(|v| (v.name.as_str(), v.protected, v.kind))
        .collect();
    assert_eq!(
        names,
        vec![
            ("", true, VersionKind::Automatic),
            ("Named one", false, VersionKind::Named)
        ]
    );
    assert_eq!(moved.read(moved.entries()[0].id).unwrap(), "auto text");
    assert_eq!(moved.read(moved.entries()[1].id).unwrap(), "named text");
    assert!(
        store.read(a).is_ok() && store.read(b).is_ok(),
        "carrying removes nothing"
    );

    assert_eq!(retire_versions(&old, &new).unwrap(), 2);
    let old_dir = store.dir().to_path_buf();
    assert!(!old_dir.join(INDEX_FILE).exists());
    assert!(
        old_dir.join("notes.txt").is_file(),
        "an unrelated file stays"
    );
    assert!(
        VersionStore::for_project(&old)
            .unwrap()
            .entries()
            .is_empty()
    );
}

#[test]
fn retiring_the_versions_removes_an_empty_folder_and_nothing_when_the_place_is_the_same() {
    let root = dir("retire");
    let file = root.join("a").join("song.atomik");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    let mut store = VersionStore::for_project(&file).unwrap();
    auto(&mut store, "x", 1);
    // The same place: nothing happens.
    assert_eq!(carry_versions(&file, &file).unwrap(), 0);
    assert_eq!(retire_versions(&file, &file).unwrap(), 0);
    assert_eq!(VersionStore::for_project(&file).unwrap().entries().len(), 1);
    // A different place: the folder goes with the last version.
    let other = root.join("b").join("song.atomik");
    fs::create_dir_all(other.parent().unwrap()).unwrap();
    assert_eq!(carry_versions(&file, &other).unwrap(), 1);
    assert_eq!(retire_versions(&file, &other).unwrap(), 1);
    assert!(!store.dir().exists());
}

#[test]
fn a_version_that_cannot_be_read_stops_the_carry_and_nothing_is_removed() {
    let root = dir("unreadable");
    let old_file = root.join("old").join("song.atomik");
    let new_file = root.join("new").join("song.atomik");
    fs::create_dir_all(old_file.parent().unwrap()).unwrap();
    let mut old = VersionStore::for_project(&old_file).unwrap();
    let good = auto(&mut old, "first", 1);
    let bad = auto(&mut old, "second", 2);
    // Not text: the version cannot be read back as a project.
    fs::write(old.path_of(bad).unwrap(), [0xff, 0xfe, 0x00, 0xc3]).unwrap();
    assert!(matches!(
        carry_versions(&old_file, &new_file),
        Err(AudioFileError::Unverified(_))
    ));
    // Every old version is still there.
    assert!(old.path_of(good).unwrap().is_file());
    assert!(old.path_of(bad).unwrap().is_file());
}
