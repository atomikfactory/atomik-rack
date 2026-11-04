//! Pure maths behind the analyser panels: the log frequency axis, the level axis, the
//! goniometer and Lissajous mapping of a sample pair, and the readout texts.

use atomik_ui::analyser::{
    LOUDNESS_WINDOW_S, PLOT_HI_HZ, PLOT_LO_HZ, SPECTROGRAM_STEPS, ScopeMode, balance_text,
    band_centre_hz, cell_intensity, correlation_text, freq_pos, freq_x, grid_step_db, hz_text,
    level_text, level_y, loudness_runs, loudness_y, lra_text, peak_text, scope_point,
    spectrogram_step, target_text, width_text,
};
use vello::kurbo::Rect;

fn near(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

#[test]
fn the_frequency_axis_is_logarithmic_from_20_hz_to_20_khz() {
    assert!(near(freq_pos(PLOT_LO_HZ), 0.0, 1e-12));
    assert!(near(freq_pos(PLOT_HI_HZ), 1.0, 1e-12));
    // Three decades: 200 Hz and 2 kHz sit a third and two thirds of the way.
    assert!(near(freq_pos(200.0), 1.0 / 3.0, 1e-9));
    assert!(near(freq_pos(2000.0), 2.0 / 3.0, 1e-9));
    // Outside the axis it clamps.
    assert!(near(freq_pos(5.0), 0.0, 0.0) && near(freq_pos(40_000.0), 1.0, 0.0));
    let plot = Rect::new(10.0, 0.0, 510.0, 100.0);
    assert!(near(freq_x(plot, 20.0), 10.0, 1e-9));
    assert!(near(freq_x(plot, 20_000.0), 510.0, 1e-9));
    assert!(
        near(freq_x(plot, 632.455_532), 260.0, 1e-3),
        "the geometric centre"
    );
}

#[test]
fn the_level_axis_puts_zero_db_on_top_and_the_range_at_the_bottom() {
    let plot = Rect::new(0.0, 20.0, 100.0, 220.0);
    assert!(near(level_y(plot, 0.0, 90.0), 20.0, 1e-9));
    assert!(near(level_y(plot, -90.0, 90.0), 220.0, 1e-9));
    assert!(near(level_y(plot, -45.0, 90.0), 120.0, 1e-9));
    // Louder than 0 dB and quieter than the range clamp to the plot.
    assert!(near(level_y(plot, 12.0, 90.0), 20.0, 0.0));
    assert!(near(level_y(plot, -200.0, 90.0), 220.0, 0.0));
    // A degenerate range does not divide by zero.
    assert!(level_y(plot, -3.0, 0.0).is_finite());
    assert!(near(f64::from(grid_step_db(30.0)), 5.0, 0.0));
    assert!(near(f64::from(grid_step_db(60.0)), 10.0, 0.0));
    assert!(near(f64::from(grid_step_db(120.0)), 20.0, 0.0));
}

#[test]
fn the_goniometer_shows_mono_as_a_vertical_line_and_one_channel_at_45_degrees() {
    let g = |l, r| scope_point(l, r, ScopeMode::Goniometer, 0.0);
    // Mono: no side, so x is 0 and y grows with the level.
    let (x, y) = g(0.5, 0.5);
    assert!(
        near(x, 0.0, 1e-9) && near(y, 0.5 * 2f64.sqrt(), 1e-6),
        "{x} {y}"
    );
    // Opposite polarity: a horizontal line; a positive left sample pulls it to the left.
    let (x, y) = g(0.5, -0.5);
    assert!(near(y, 0.0, 1e-9) && near(x, -0.5 * 2f64.sqrt(), 1e-6));
    let (x, y) = g(-0.5, 0.5);
    assert!(near(y, 0.0, 1e-9) && near(x, 0.5 * 2f64.sqrt(), 1e-6));
    // Left only: up and to the left, at 45 degrees, and the length is kept.
    let (x, y) = g(0.5, 0.0);
    assert!(
        near(x, -y, 1e-9) && y > 0.0 && near(x.hypot(y), 0.5, 1e-6),
        "{x} {y}"
    );
    // Right only is the mirror image.
    let (x2, y2) = g(0.0, 0.5);
    assert!(near(x2, -x, 1e-9) && near(y2, y, 1e-9));
    // Silence is the centre.
    assert_eq!(g(0.0, 0.0), (0.0, 0.0));
}

#[test]
fn the_lissajous_figure_is_left_across_and_right_up_with_a_gain_and_a_frame() {
    let l = |l, r, gain| scope_point(l, r, ScopeMode::Lissajous, gain);
    assert_eq!(l(0.25, -0.5, 0.0), (0.25, -0.5));
    let (x, y) = l(0.25, 0.1, 6.0206);
    assert!(near(x, 0.5, 1e-3) && near(y, 0.2, 1e-3), "{x} {y}");
    // Past the frame it clamps, and garbage maps to the centre.
    assert_eq!(l(0.9, -0.9, 12.0), (1.0, -1.0));
    assert_eq!(l(f32::NAN, 0.5, 0.0), (0.0, 0.0));
    assert_eq!(l(f32::INFINITY, 0.5, 0.0), (0.0, 0.0));
    assert_eq!(ScopeMode::of_choice(0.0), ScopeMode::Goniometer);
    assert_eq!(ScopeMode::of_choice(1.0), ScopeMode::Lissajous);
}

#[test]
fn the_readout_texts_are_short_and_stable() {
    assert_eq!(hz_text(62.5), "62.5 Hz");
    assert_eq!(hz_text(440.0), "440 Hz");
    assert_eq!(hz_text(1000.0), "1.00 kHz");
    assert_eq!(hz_text(12_345.0), "12.3 kHz");
    assert_eq!(peak_text(1000.0, -20.06), "PEAK 1.00 kHz  -20.1 dB");
    assert_eq!(peak_text(0.0, -200.0), "PEAK --");
    assert_eq!(level_text(-23.04), "-23.0");
    assert_eq!(level_text(-120.0), "--");
    assert_eq!(correlation_text(1.0), "+1.00");
    assert_eq!(correlation_text(-0.373), "-0.37");
    assert_eq!(balance_text(0.0), "C");
    assert_eq!(balance_text(-0.4), "L 40%");
    assert_eq!(balance_text(0.123), "R 12%");
    assert_eq!(width_text(0.5), "50%");
    assert_eq!(width_text(0.0), "0%");
}

#[test]
fn the_band_centres_are_the_dsp_centres() {
    for (i, centre) in atomik_dsp::spectrum::band_centres().into_iter().enumerate() {
        assert!(
            near(band_centre_hz(i) / f64::from(centre), 1.0, 1e-4),
            "{i}"
        );
    }
}

#[test]
fn a_spectrogram_cell_reads_louder_the_closer_it_is_to_zero_db() {
    use atomik_dsp::spectrogram::quantise;
    assert!(near(f64::from(cell_intensity(0, 80.0)), 0.0, 0.0));
    assert!(near(
        f64::from(cell_intensity(quantise(0.0), 80.0)),
        1.0,
        1e-6
    ));
    assert!(near(
        f64::from(cell_intensity(quantise(-40.0), 80.0)),
        0.5,
        1e-6
    ));
    // Quieter than the range: nothing.
    assert!(near(
        f64::from(cell_intensity(quantise(-90.0), 80.0)),
        0.0,
        0.0
    ));
    // The palette: nothing for the floor, the first step just above it, the last at the top.
    assert_eq!(spectrogram_step(0.0), 0);
    assert_eq!(spectrogram_step(0.03), 1);
    assert_eq!(spectrogram_step(1.0), SPECTROGRAM_STEPS);
    let mut last = 0;
    for i in 0..=100 {
        let step = spectrogram_step(i as f32 / 100.0);
        assert!(step >= last && step <= SPECTROGRAM_STEPS);
        last = step;
    }
}

#[test]
fn the_loudness_graph_puts_the_target_a_little_below_the_top() {
    let plot = Rect::new(0.0, 10.0, 300.0, 52.0);
    // 12 LU above the target at the top, 30 below at the bottom: 42 LU over 42 px.
    assert!(near(loudness_y(plot, -2.0, -14.0), 10.0, 1e-4));
    assert!(near(loudness_y(plot, -14.0, -14.0), 22.0, 1e-4));
    assert!(near(loudness_y(plot, -44.0, -14.0), 52.0, 1e-4));
    assert!(near(loudness_y(plot, 0.0, -14.0), 10.0, 0.0));
    assert!(near(loudness_y(plot, -90.0, -14.0), 52.0, 0.0));
    // The target moves the whole graph.
    assert!(near(loudness_y(plot, -23.0, -23.0), 22.0, 1e-4));
}

#[test]
fn the_loudness_runs_end_at_the_right_edge_and_break_at_silence() {
    let plot = Rect::new(0.0, 0.0, 299.0, 42.0);
    let runs = loudness_runs(plot, &[-20.0, -20.0, -120.0, -18.0, -18.0, -18.0], -14.0);
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].len(), 2);
    assert_eq!(runs[1].len(), 3);
    // One second is one pixel here (299 px for 300 s); the newest is at the right edge.
    assert!(near(runs[1][2].0, 299.0, 1e-9));
    assert!(near(runs[1][1].0, 298.0, 1e-9));
    assert!(near(runs[0][1].0, 295.0, 1e-9));
    assert!(near(runs[0][0].0, 294.0, 1e-9));
    // Only the last five minutes are drawn.
    let long = vec![-20.0f32; LOUDNESS_WINDOW_S + 50];
    let runs = loudness_runs(plot, &long, -14.0);
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].len(), LOUDNESS_WINDOW_S);
    assert!(near(runs[0][0].0, 0.0, 1e-9));
    assert!(loudness_runs(plot, &[], -14.0).is_empty());
}

#[test]
fn the_loudness_texts_are_short_and_stable() {
    assert_eq!(lra_text(Some(6.24)), "LRA 6.2 LU");
    assert_eq!(lra_text(Some(0.0)), "LRA 0.0 LU");
    assert_eq!(lra_text(None), "LRA --");
    assert_eq!(target_text(-14.0), "TARGET -14");
    assert_eq!(target_text(-23.4), "TARGET -23");
}
