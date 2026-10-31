//! The shaped-text cache: cached glyphs equal the cold ones, hits cost far less, bounded.

use atomik_ui::{Align, TextEngine, TextRun};
use vello::kurbo::Point;
use vello::peniko::Color;

fn run(text: &str, x: f64, align: Align) -> TextRun {
    let mut r = TextRun::new(text, Point::new(x, 20.0), 12.0, Color::WHITE);
    r.align = align;
    r
}

#[test]
fn a_cached_shape_equals_a_cold_one() {
    let mut warm = TextEngine::new();
    for align in [Align::Start, Align::Center, Align::End] {
        let r = run("Filter Cutoff", 33.25, align);
        let cold = TextEngine::new().shape(&r);
        let first = warm.shape(&r);
        let again = warm.shape(&r);
        for got in [&first, &again] {
            assert_eq!(got.len(), cold.len());
            for (a, b) in got.iter().zip(&cold) {
                assert_eq!(a.glyphs, b.glyphs);
                assert_eq!(a.coords, b.coords);
                assert_eq!(a.font_size.to_bits(), b.font_size.to_bits());
            }
        }
    }
}

#[test]
fn the_cache_is_keyed_by_size_weight_and_spacing_not_position() {
    let mut e = TextEngine::new();
    let a = run("Attack", 0.0, Align::Start);
    let mut b = a.clone();
    b.pos = Point::new(90.0, 4.0);
    e.shape(&a);
    e.shape(&b);
    assert_eq!(
        e.cache_stats().0,
        1,
        "position and colour do not split entries"
    );
    let mut c = a.clone();
    c.size = 13.0;
    let mut d = a.clone();
    d.weight = 700;
    let mut f = a.clone();
    f.letter_spacing = 1.0;
    for r in [&c, &d, &f] {
        e.shape(r);
    }
    assert_eq!(e.cache_stats().0, 4);
    assert!(e.measure(&c) > e.measure(&a));
}

#[test]
fn the_cache_stays_bounded() {
    let mut e = TextEngine::new();
    for i in 0..6000 {
        e.shape(&run(&format!("label {i}"), 0.0, Align::Start));
    }
    let (entries, bytes, _, _) = e.cache_stats();
    assert!(entries <= 4096, "{entries}");
    assert!(bytes <= 8 << 20, "{bytes}");
    e.clear_cache();
    assert_eq!(e.cache_stats().0, 0);
}

#[test]
fn a_second_pass_over_the_same_labels_shapes_nothing() {
    let texts: Vec<String> = (0..300).map(|i| format!("Parameter {i} value")).collect();
    let mut e = TextEngine::new();
    for pass in 0..2 {
        for s in &texts {
            e.shape_shared(&run(s, 0.0, Align::Start));
        }
        let (entries, _, hits, misses) = e.cache_stats();
        assert_eq!((entries, hits, misses), (300, 300 * pass, 300));
    }
}

/// `cargo test -p atomik-ui --release --test text_cache -- --ignored --nocapture`.
#[test]
#[ignore = "benchmark"]
#[allow(
    clippy::disallowed_methods,
    reason = "wall-clock timing is the point of a benchmark"
)]
fn warm_shaping_is_much_cheaper_than_cold() {
    let texts: Vec<String> = (0..300).map(|i| format!("Parameter {i} value")).collect();
    let mut e = TextEngine::new();
    let t = std::time::Instant::now();
    for s in &texts {
        e.shape_shared(&run(s, 0.0, Align::Start));
    }
    let cold = t.elapsed();
    let t = std::time::Instant::now();
    for s in &texts {
        e.shape_shared(&run(s, 0.0, Align::Start));
    }
    let warm = t.elapsed();
    eprintln!("cold {cold:?} warm {warm:?}");
    assert!(warm * 10 < cold, "cold {cold:?} warm {warm:?}");
}
