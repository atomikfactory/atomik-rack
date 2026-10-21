//! Keystroke-filter performance check (plan NFR-3: 50 000 rows in 8 ms, release build).
//!
//! Run with: `cargo test -p atomik-library --release -- --ignored --nocapture`

use atomik_library::{AudioFormat, FileKind, FileRow, Query};
use std::path::PathBuf;
use std::time::Instant;

const ROWS: usize = 50_000;
const BUDGET_MS: f64 = 8.0;

fn rows() -> Vec<FileRow> {
    const WORDS: [&str; 8] = [
        "Kick", "Snare", "HiHat", "Bass", "Loop", "Vocal", "Pad", "Perc",
    ];
    (0..ROWS)
        .map(|i| {
            let name = format!(
                "{} {} Sample_{:05} {}.wav",
                WORDS[i % 8],
                WORDS[(i / 8) % 8],
                i,
                if i % 7 == 0 { "Café" } else { "take" }
            );
            FileRow {
                path: PathBuf::from(format!("/samples/{name}")),
                name,
                kind: FileKind::Audio(AudioFormat::Wav),
                size: 1000,
                info: None,
                favorite: i % 11 == 0,
                tags: if i % 5 == 0 {
                    vec!["808".to_string(), "Drums".to_string()]
                } else {
                    Vec::new()
                },
            }
        })
        .collect()
}

#[test]
#[ignore = "timing test: run with --release -- --ignored"]
#[allow(clippy::disallowed_methods)] // timing in a perf test only
fn matching_50k_rows_with_a_two_word_query_fits_the_budget() {
    let rows = rows();
    let queries = [
        Query::parse("kick snare"),
        Query::parse("sample_4 #808"),
        Query::parse("café fav"),
    ];
    for query in &queries {
        // Warm up, then take the best of several runs to ignore scheduler noise.
        let mut best = f64::MAX;
        let mut hits = 0;
        for _ in 0..7 {
            let start = Instant::now();
            hits = rows.iter().filter(|r| query.matches(r)).count();
            best = best.min(start.elapsed().as_secs_f64() * 1000.0);
        }
        println!("{query:?}: {hits} hits, best {best:.3} ms");
        assert!(hits > 0);
        assert!(best <= BUDGET_MS, "{best:.3} ms > {BUDGET_MS} ms");
    }
}
