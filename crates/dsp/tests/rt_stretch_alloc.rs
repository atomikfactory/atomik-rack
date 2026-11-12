//! The real-time stretcher is built once (allocating) and then never allocates: `process`, a
//! locate, an explicit reset, a transposition change and a sample-rate change all run inside the
//! allocation guard.

atomik_testkit::install_alloc_guard!();

use atomik_dsp::stretch::{RT_MAX_SAMPLE_RATE, RtStretcher, pitch_ratio_for_cents};
use atomik_testkit::no_alloc::assert_no_alloc;

#[test]
fn process_and_reset_never_allocate() {
    let src: Vec<f32> = (0..200_000)
        .map(|i| ((i % 97) as f32 / 48.5 - 1.0) * 0.4)
        .collect();
    let map = [(0.0, 0.0), (48_000.0, 36_000.0), (200_000.0, 250_000.0)];
    let mut st = RtStretcher::new(RT_MAX_SAMPLE_RATE);
    let mut l = [0.0f32; 64];
    let mut r = [0.0f32; 64];
    assert_no_alloc(|| {
        for (k, from) in (0..2_000u64).map(|k| (k, k * 64)) {
            let ratio = if k < 1_000 {
                1.0
            } else {
                pitch_ratio_for_cents(-500)
            };
            let sr = if k < 1_500 { 48_000 } else { 96_000 };
            if k == 700 {
                st.reset(from);
            }
            // A locate halfway through.
            let at = if k >= 1_200 { from + 30_000 } else { from };
            st.process([&src, &src], &map, ratio, sr, at, [&mut l, &mut r]);
        }
    });
    assert!(l.iter().any(|v| *v != 0.0));
}
