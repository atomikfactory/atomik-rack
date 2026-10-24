//! Audio editing in the engine: reverse, gain envelopes, the new fade shapes and equal-power
//! crossfades play the exact samples the model's reference says, and a streamed asset plays them
//! like the same asset in memory, bit for bit.
#![allow(clippy::needless_range_loop)] // Justification: sample-index loops read clearer than zipped iterators in these checks.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_devices::audio_track;
use atomik_engine::{
    AssetId, SongAsset, SongAudioRegion, SongAudioTrack, SongFadeCurve, StreamSource,
    TransportCommand, compile_song,
};
use common::audio::*;
use common::hot::Rig;
use common::*;
use std::sync::Arc;

fn bits(x: f32) -> u32 {
    x.to_bits()
}

fn track(regions: Vec<SongAudioRegion>, envelopes: Vec<Vec<(u64, f32)>>) -> SongAudioTrack {
    SongAudioTrack {
        device: atomik_engine::DeviceInstanceId(1),
        regions,
        envelopes,
    }
}

fn bounce(
    regions: Vec<SongAudioRegion>,
    envelopes: Vec<Vec<(u64, f32)>>,
    assets: Vec<SongAsset>,
    frames: usize,
) -> Vec<Vec<f32>> {
    Bounce::new(audio_input(vec![track(regions, envelopes)], assets), frames).run()
}

/// An asset of `frames` frames, every sample `v` on both channels.
fn constant(v: f32, frames: usize) -> SongAsset {
    SongAsset::memory(vec![vec![v; frames]; 2].into())
}

#[test]
fn a_reversed_region_plays_the_span_backwards() {
    let mut r = region(1_000, 4_999, 2_000);
    r.reverse = true;
    let out = bounce(vec![r], vec![], vec![asset(2, 20_000)], 4_000);
    for n in 0..4_000usize {
        for (c, ch) in out.iter().enumerate() {
            let want = if (1_000..3_000).contains(&n) {
                value(c, 4_999 - (n - 1_000))
            } else {
                0.0
            };
            assert_eq!(bits(ch[n]), bits(want), "channel {c}, frame {n}");
        }
    }
}

#[test]
fn a_reversed_region_is_silent_until_it_reaches_the_asset() {
    // The asset has 20 000 frames; the region starts reading at frame 20 499.
    let mut r = region(100, 20_499, 1_000);
    r.reverse = true;
    let out = bounce(vec![r], vec![], vec![asset(2, 20_000)], 1_500);
    for n in 0..1_500usize {
        let want = if (600..1_100).contains(&n) {
            value(0, 20_499 - (n - 100))
        } else {
            0.0
        };
        assert_eq!(bits(out[0][n]), bits(want), "frame {n}");
    }
}

#[test]
fn reverse_works_with_fades_that_stay_in_clip_time() {
    let mut r = region(500, 2_999, 3_000);
    r.reverse = true;
    r.fade_in = 400;
    r.fade_out = 600;
    r.fade_curve = SongFadeCurve::SCurve;
    r.gain = 0.5;
    let out = bounce(vec![r], vec![], vec![asset(2, 20_000)], 4_000);
    for i in 0..3_000u64 {
        let n = 500 + i as usize;
        let want = value(0, 2_999 - i as usize) * (0.5f32 * r.fade_gain(i));
        assert_eq!(bits(out[0][n]), bits(want), "frame {i}");
    }
    // The fade-in is at the start of the *playback*, i.e. on the highest source samples.
    assert_eq!(out[0][500], 0.0);
    assert!(out[0][2_000].abs() > 1e-3);
}

#[test]
fn an_envelope_ramps_in_amplitude_and_holds_outside_its_points() {
    let mut r = region(200, 100, 3_000);
    r.env = Some(0);
    let env = vec![(500u64, 0.25f32), (1_500, 1.0), (2_000, 0.5)];
    let out = bounce(vec![r], vec![env.clone()], vec![asset(2, 20_000)], 4_000);
    for i in 0..3_000usize {
        let n = i as u64;
        let e = if n < 500 {
            0.25
        } else if n >= 2_000 {
            0.5
        } else if n < 1_500 {
            let t = (n - 500) as f64 / 1_000.0;
            (0.25f64 + (1.0f64 - 0.25) * t) as f32
        } else {
            let t = (n - 1_500) as f64 / 500.0;
            (1.0f64 + (0.5f64 - 1.0) * t) as f32
        };
        let want = value(0, 100 + i) * (1.0f32 * 1.0 * e);
        assert_eq!(bits(out[0][200 + i]), bits(want), "frame {i}");
    }
    // No steps: neighbouring gains differ by far less than the whole range.
    for i in 1..3_000usize {
        let (a, b) = (
            out[0][200 + i - 1] / value(0, 99 + i),
            out[0][200 + i] / value(0, 100 + i),
        );
        assert!((a - b).abs() < 0.01, "frame {i}");
    }
}

#[test]
fn the_new_fade_shapes_follow_their_polynomials() {
    for curve in [SongFadeCurve::Exponential, SongFadeCurve::SCurve] {
        let mut r = region(0, 0, 2_000);
        r.fade_in = 800;
        r.fade_out = 500;
        r.fade_curve = curve;
        let out = bounce(vec![r], vec![], vec![asset(2, 20_000)], 2_500);
        for n in 0..2_000usize {
            let mut g = 1.0f64;
            if n < 800 {
                g *= curve.eval(n as f64 / 800.0);
            }
            if n + 500 >= 2_000 {
                g *= curve.eval((2_000 - n) as f64 / 500.0);
            }
            let want = value(0, n) * (1.0f32 * g as f32);
            assert_eq!(bits(out[0][n]), bits(want), "{curve:?} frame {n}");
        }
    }
}

/// An earlier clip over `[1000, 3000)` of asset 0 and a later one over `[2000, 4000)` of asset
/// 1 crossfading over `[2000, 3000)` (the tail region carries the earlier clip's overlap).
fn crossfade_regions(curve: SongFadeCurve) -> Vec<SongAudioRegion> {
    let head = SongAudioRegion {
        clip_frames: 2_000,
        ..region(1_000, 0, 1_000)
    };
    let tail = SongAudioRegion {
        clip_pos: 1_000,
        clip_frames: 2_000,
        x_out: 1_000,
        x_curve: curve,
        ..region(2_000, 1_000, 1_000)
    };
    let later = SongAudioRegion {
        asset: AssetId(1),
        x_in: 1_000,
        x_curve: curve,
        ..region(2_000, 0, 2_000)
    };
    vec![head, tail, later]
}

#[test]
fn an_equal_power_crossfade_keeps_constant_power() {
    // Two unit signals: with equal power the gains square-sum to 1 at every frame.
    let out = bounce(
        crossfade_regions(SongFadeCurve::EqualPower),
        vec![],
        vec![constant(1.0, 10_000), constant(1.0, 10_000)],
        5_000,
    );
    for n in 1_000..2_000 {
        assert_eq!(out[0][n], 1.0, "only the earlier clip before the overlap");
    }
    for n in 2_000..3_000usize {
        let t = (n - 2_000) as f64 / 1_000.0;
        let (g_out, g_in) = (
            (((1.0 - t) * std::f64::consts::FRAC_PI_2).sin()) as f32,
            ((t * std::f64::consts::FRAC_PI_2).sin()) as f32,
        );
        assert!((out[0][n] - (g_out + g_in)).abs() < 1e-6, "frame {n}");
        assert!((g_out * g_out + g_in * g_in - 1.0).abs() < 1e-5);
    }
    for n in 3_000..4_000 {
        assert_eq!(out[0][n], 1.0, "only the later clip after the overlap");
    }
    assert_eq!(out[0][4_000], 0.0);
    // Two uncorrelated signals keep their power: asset 0 is the left unit, asset 1 is silent
    // on the earlier clip's side, so the sum is a clean fade in/out pair.
    let out = bounce(
        crossfade_regions(SongFadeCurve::EqualPower),
        vec![],
        vec![constant(1.0, 10_000), constant(0.0, 10_000)],
        5_000,
    );
    assert!(
        out[0][2_000] > 0.999 && out[0][2_999] < 0.01,
        "the earlier clip fades out"
    );
}

#[test]
fn a_linear_crossfade_of_equal_signals_is_flat() {
    let out = bounce(
        crossfade_regions(SongFadeCurve::Linear),
        vec![],
        vec![constant(0.8, 10_000), constant(0.8, 10_000)],
        5_000,
    );
    for n in 1_000..4_000 {
        assert!((out[0][n] - 0.8).abs() < 1e-6, "frame {n}: {}", out[0][n]);
    }
}

#[test]
fn a_crossfade_with_sample_data_adds_the_two_scaled_signals() {
    let out = bounce(
        crossfade_regions(SongFadeCurve::SCurve),
        vec![],
        vec![asset(2, 20_000), asset(2, 30_000)],
        5_000,
    );
    for n in 2_000..3_000usize {
        let i = (n - 2_000) as u64;
        let t = i as f64 / 1_000.0;
        let g_in = SongFadeCurve::SCurve.eval(t) as f32;
        let g_out = SongFadeCurve::SCurve.eval((1_000 - i) as f64 / 1_000.0) as f32;
        let want = value(0, 1_000 + (n - 2_000)) * (1.0f32 * g_out)
            + value(0, n - 2_000) * (1.0f32 * g_in);
        assert_eq!(bits(out[0][n]), bits(want), "frame {n}");
    }
}

// ---- streaming ----------------------------------------------------------------------------

struct Fake {
    data: Arc<[Vec<f32>]>,
}

impl StreamSource for Fake {
    fn read(&self, start_frame: u64, out: &mut [Vec<f32>]) -> usize {
        let frames = self.data[0].len() as u64;
        let n = (out[0].len() as u64).min(frames.saturating_sub(start_frame)) as usize;
        for (o, d) in out.iter_mut().zip(self.data.iter()) {
            o[..n].copy_from_slice(&d[start_frame as usize..start_frame as usize + n]);
        }
        n
    }
}

fn streamed(data: &Arc<[Vec<f32>]>) -> SongAsset {
    SongAsset::stream(
        data[0].len() as u64,
        data.len() as u16,
        Arc::new(Fake {
            data: Arc::clone(data),
        }),
    )
}

/// Plays `regions` from the start in blocks of `block` frames and returns the output.
fn play(
    assets: Vec<SongAsset>,
    regions: Vec<SongAudioRegion>,
    envelopes: Vec<Vec<(u64, f32)>>,
    frames: usize,
    block: usize,
) -> Vec<Vec<f32>> {
    let c = cfg(0, 2);
    let mut rig = Rig::new(&c);
    let id = rig.insert(0, 1, audio_track::factory(), &[]);
    rig.wire_outputs(id, "out_1", "out_2");
    rig.route(0);
    let (mut engine, mut handle) = engine_with(c, 0, None);
    for e in rig.edits {
        handle.schedule(0, e.edit).unwrap();
    }
    let input = audio_input(vec![track(regions, envelopes)], assets);
    handle.submit_song(compile_song(&input).unwrap()).unwrap();
    for _ in 0..5_000 {
        render_block(&mut engine, &[], 2, 64);
        if handle.stream_primed() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    handle.transport(TransportCommand::Play).unwrap();
    let out = render_blocks(&mut engine, &[], 2, frames, block);
    handle.collect_garbage();
    assert_eq!(handle.stream_underruns(), 0, "the streamer kept up");
    out
}

fn mixed(reverse_first: bool) -> (Vec<SongAudioRegion>, Vec<Vec<(u64, f32)>>) {
    let mut regions = crossfade_regions(SongFadeCurve::EqualPower);
    for r in &mut regions[..2] {
        r.reverse = reverse_first;
        if reverse_first {
            // The earlier clip (3000 frames of asset 0 from frame 0) plays its span backwards:
            // clip frame n reads source 2999 - n.
            r.source_offset = 2_999 - r.clip_pos;
        }
        r.fade_in = 300;
        r.gain = 0.7;
        r.fade_curve = SongFadeCurve::EqualPower;
    }
    regions[2].env = Some(0);
    regions[2].reverse = true;
    regions[2].source_offset = 5_999;
    regions[2].fade_out = 500;
    (regions, vec![vec![(0, 0.2), (1_200, 1.0), (1_900, 0.6)]])
}

#[test]
fn streamed_assets_play_editing_bit_for_bit_like_memory_assets() {
    for reverse_first in [false, true] {
        for block in [64usize, 333] {
            let (regions, envelopes) = mixed(reverse_first);
            let data0 = planes(2, 20_000);
            let data1 = planes(2, 30_000);
            let memory = play(
                vec![
                    SongAsset::memory(Arc::clone(&data0)),
                    SongAsset::memory(Arc::clone(&data1)),
                ],
                regions.clone(),
                envelopes.clone(),
                6_000,
                block,
            );
            assert!(memory[0].iter().any(|s| s.abs() > 1e-3));
            for variant in 0..3 {
                let assets = match variant {
                    0 => vec![streamed(&data0), streamed(&data1)],
                    1 => vec![streamed(&data0), SongAsset::memory(Arc::clone(&data1))],
                    _ => vec![SongAsset::memory(Arc::clone(&data0)), streamed(&data1)],
                };
                let got = play(assets, regions.clone(), envelopes.clone(), 6_000, block);
                for c in 0..2 {
                    for n in 0..6_000 {
                        assert_eq!(
                            bits(got[c][n]),
                            bits(memory[c][n]),
                            "reverse {reverse_first}, block {block}, variant {variant}, channel {c}, frame {n}"
                        );
                    }
                }
            }
        }
    }
}
