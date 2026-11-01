//! Opening a project with very many notes and automation points costs a handful of allocations
//! per item, not a heap path and a JSON value each.
#![allow(missing_docs)]

use atomik_device_sdk::Registry;
use atomik_model::stress::{lanes_song_doc, large_song_doc};
use atomik_model::{load_project, save_project};
use atomik_testkit::no_alloc::allocations_of;

atomik_testkit::install_alloc_guard!();

fn registry() -> Registry {
    let mut r = Registry::new();
    atomik_devices::register_all(&mut r).expect("built-in devices register");
    r
}

/// Loads `text` once for the figures and returns allocations per item.
fn per_item(text: &str, items: usize, what: &str, reg: &Registry) -> f64 {
    let (doc, allocs) = allocations_of(|| load_project(text, reg).expect("loads"));
    let per = allocs as f64 / items as f64;
    println!("{what}: {allocs} allocations for {items} items ({per:.2} each)");
    drop(doc);
    per
}

#[test]
fn a_project_of_200_000_notes_loads_with_at_most_two_allocations_per_note() {
    let reg = registry();
    let doc = large_song_doc(&reg, 100, 500, 200_000);
    let text = save_project(&doc);
    let per = per_item(&text, 200_000, "notes", &reg);
    assert!(per <= 2.0, "{per:.2} allocations per note");
}

#[test]
fn automation_points_load_with_at_most_two_allocations_each() {
    const POINTS: usize = 50_000;
    let reg = registry();
    let mut value: serde_json::Value =
        serde_json::from_str(&save_project(&lanes_song_doc(&reg, 1))).unwrap();
    let clip = value
        .pointer_mut("/song/tracks/0/lanes/1/clips/0")
        .expect("the automation clip");
    clip["loop_length"] = serde_json::Value::Null;
    clip["points"] = (0..POINTS)
        .map(|i| serde_json::json!([i * 10, (i % 7) as f64 / 7.0, "linear"]))
        .collect();
    let text = value.to_string();
    let per = per_item(&text, POINTS, "points", &reg);
    // A point is a JSON array and its curve name a string: two allocations in the parsed text,
    // and none of its own in the reader.
    assert!(per <= 2.1, "{per:.2} allocations per point");
}

/// Hand check: `cargo test -p atomik-model --release --test load_speed -- --ignored --nocapture`
/// prints the time to open the 200 000-note project (the same figure `perf/BASELINE.md` holds).
#[test]
#[ignore = "timing"]
fn time_to_open_200_000_notes() {
    #[allow(clippy::disallowed_methods)]
    let now = std::time::Instant::now;
    let reg = registry();
    let text = save_project(&large_song_doc(&reg, 100, 500, 200_000));
    let mut best = std::time::Duration::MAX;
    for _ in 0..5 {
        let t = now();
        let doc = load_project(&text, &reg).unwrap();
        best = best.min(t.elapsed());
        drop(doc);
    }
    println!(
        "load 200 000 notes: best of 5 {best:?} ({} MB of text)",
        text.len() >> 20
    );
}
