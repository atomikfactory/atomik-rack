//! Bounces of audio editing: fades of every shape, equal-power crossfades, reverse, clip gain
//! and gain envelopes, strip silence, and the old projects that use none of it.
//!
//! The assets are procedural. The console between the track and the output is not the subject,
//! so the expectations are relative to a **calibration bounce** (the console's gain `c` at the
//! output) and to the model's `flatten_audio` regions, which are the reference definition of
//! what plays.
#![allow(clippy::needless_range_loop)] // Justification: sample-index loops read clearer than zipped iterators in these checks.

use atomik_audio_file::{AudioData, Bundle};
use atomik_core::PPQ;
use atomik_model::song::flatten_audio;
use atomik_model::{
    AssetMeta, AudioClipContent, ClipContent, Command, DeviceKey, Document, FadeCurve, GainPoint,
    RackDoc, SongCommand, StripParams, load_project, save_project, strip_silence,
};
use atomik_render::project::{ProjectOptions, render_doc_in, render_project_json_in};
use atomik_render::{Rendered, default_registry};
use std::num::NonZeroUsize;
use std::path::PathBuf;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "atomik-render-edit-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn options(block: usize, workers: u16) -> ProjectOptions {
    ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    }
}

fn bits(r: &Rendered) -> Vec<u32> {
    r.channels.iter().flatten().map(|x| x.to_bits()).collect()
}

fn constant(value: f32, frames: usize) -> AudioData {
    AudioData::new(48_000, vec![vec![value; frames], vec![value; frames]]).unwrap()
}

/// A deterministic stereo noise asset (integer xorshift, no libm).
fn noise(frames: usize) -> AudioData {
    let mut x: u32 = 0x1234_5678;
    let mut channel = |n: usize| -> Vec<f32> {
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                ((x >> 8) as f32 / 8_388_608.0 - 1.0) * 0.4
            })
            .collect()
    };
    let l = channel(frames);
    let r = channel(frames);
    AudioData::new(48_000, vec![l, r]).unwrap()
}

fn ramp(frames: usize) -> AudioData {
    let r: Vec<f32> = (0..frames).map(|i| i as f32 / frames as f32).collect();
    AudioData::new(48_000, vec![r.clone(), r]).unwrap()
}

struct Song {
    doc: RackDoc,
    bundle: Bundle,
    asset: String,
    _dir: TempDir,
}

/// One audio track over `asset`; `build` adds clips (it gets the document and the asset id).
fn song(tag: &str, asset: &AudioData, build: impl FnOnce(&mut Document, &str)) -> Song {
    let dir = TempDir::new(tag);
    let mut bundle = Bundle::create(&dir.0.join("song")).unwrap();
    let info = bundle.add_recording(asset, "fixture.wav").unwrap();
    let registry = default_registry();
    let mut d = Document::new(atomik_model::empty_doc());
    d.apply(
        Command::RegisterAsset {
            hash: info.hash.clone(),
            meta: AssetMeta {
                channels: info.channels,
                sample_rate: info.sample_rate,
                frames: info.frames,
                original_name: info.original_name.clone(),
            },
        },
        &registry,
    )
    .unwrap();
    d.apply(
        Command::Song(SongCommand::AddAudioTrack { name: None }),
        &registry,
    )
    .unwrap();
    d.apply(Command::Song(SongCommand::SetSongEnd(8 * PPQ)), &registry)
        .unwrap();
    build(&mut d, &info.hash);
    Song {
        doc: d.doc().clone(),
        bundle,
        asset: info.hash,
        _dir: dir,
    }
}

/// Adds a clip with the full content (`AddAudioClip`, then `SetAudioClip` for the new fields).
fn put(
    d: &mut Document,
    asset: &str,
    start: i64,
    length: i64,
    f: impl FnOnce(&mut AudioClipContent),
) {
    let registry = default_registry();
    let mut a = AudioClipContent::new(asset, 0);
    f(&mut a);
    d.apply(
        Command::Song(SongCommand::AddAudioClip {
            device: DeviceKey("audio-track".into()),
            start,
            length,
            asset: asset.to_owned(),
            source_offset: a.source_offset,
            gain_db: a.gain_db,
            fade_in: a.fade_in,
            fade_out: a.fade_out,
            fade_curve: a.fade_curve,
        }),
        &registry,
    )
    .unwrap();
    let id = d.last_clip_id().unwrap();
    d.apply(
        Command::Song(SongCommand::SetAudioClip {
            id,
            start,
            length,
            muted: false,
            content: a,
            merge: None,
        }),
        &registry,
    )
    .unwrap();
}

fn render(s: &Song, block: usize, workers: u16) -> Rendered {
    render_doc_in(
        &s.doc,
        &default_registry(),
        &options(block, workers),
        Some(s.bundle.root()),
    )
    .unwrap()
}

/// The console's gain at the output (pan centre, fader at unity).
fn console_gain() -> f32 {
    let s = song("calibration", &constant(0.25, 48_000), |d, a| {
        put(d, a, 0, 2 * PPQ, |_| {});
    });
    render(&s, 512, 0).channels[0][20_000] / 0.25
}

/// What the model says plays, summed over the regions, times the console gain.
fn expected(s: &Song, planes: &AudioData, frames: usize, c: f32) -> Vec<f32> {
    let song = &s.doc.song;
    let regions = flatten_audio(&song.tracks[0], song, s.doc.sample_rate);
    let mut out = vec![0.0f32; frames];
    for r in &regions {
        let asset_frames = planes.frames() as u64;
        for i in 0..r.frames {
            let src = if r.reverse {
                r.source_offset.checked_sub(i)
            } else {
                Some(r.source_offset + i)
            };
            let Some(src) = src.filter(|s| *s < asset_frames) else {
                continue;
            };
            let at = (r.song_start_frame + i) as usize;
            if at < frames {
                out[at] += planes.channels[0][src as usize] * r.total_gain(i);
            }
        }
    }
    out.iter().map(|v| v * c).collect()
}

fn assert_close(got: &[f32], want: &[f32], tol: f32, what: &str) {
    for n in 0..want.len() {
        assert!(
            (got[n] - want[n]).abs() <= tol,
            "{what}: frame {n}: {} vs {}",
            got[n],
            want[n]
        );
    }
}

#[test]
fn every_fade_shape_follows_the_model_reference() {
    let c = console_gain();
    for curve in FadeCurve::ALL {
        let asset = noise(60_000);
        let s = song("fades", &asset, |d, a| {
            put(d, a, PPQ, 2 * PPQ, |x| {
                x.fade_in = 9_000;
                x.fade_out = 7_000;
                x.fade_curve = curve;
                x.gain_db = -2.0;
            });
        });
        let r = render(&s, 512, 0);
        let want = expected(&s, &asset, r.channels[0].len(), c);
        assert!(want.iter().any(|v| v.abs() > 0.01));
        assert_close(&r.channels[0], &want, 3e-6, curve.id());
    }
}

#[test]
fn an_equal_power_crossfade_plays_both_clips_with_constant_power() {
    let c = console_gain();
    // Two unit-level clips of the same constant asset overlap by one beat.
    let asset = constant(0.25, 120_000);
    let s = song("xfade", &asset, |d, a| {
        put(d, a, 0, 2 * PPQ, |_| {});
        put(d, a, PPQ, 2 * PPQ, |_| {});
    });
    let r = render(&s, 512, 0);
    let ch = &r.channels[0];
    // The overlap is frames 24 000..48 000; there both clips play with sin/cos gains.
    for n in (24_000..48_000).step_by(401) {
        let t = (n - 24_000) as f64 / 24_000.0;
        let (g_out, g_in) = (
            ((1.0 - t) * std::f64::consts::FRAC_PI_2).sin(),
            (t * std::f64::consts::FRAC_PI_2).sin(),
        );
        let want = 0.25 * f64::from(c) * (g_out + g_in);
        assert!((f64::from(ch[n]) - want).abs() < 3e-6, "frame {n}");
        assert!((g_out * g_out + g_in * g_in - 1.0).abs() < 1e-12);
    }
    // Outside the overlap each clip plays alone at unity.
    for n in (0..24_000)
        .step_by(997)
        .chain((48_000..72_000).step_by(997))
    {
        assert!((ch[n] - 0.25 * c).abs() < 1e-6, "frame {n}");
    }
    assert_close(ch, &expected(&s, &asset, ch.len(), c), 3e-6, "reference");
    // Turned off, the later clip simply takes over at the overlap start.
    let hard = song("hard", &asset, |d, a| {
        put(d, a, 0, 2 * PPQ, |x| x.xfade = None);
        put(d, a, PPQ, 2 * PPQ, |x| x.xfade = None);
    });
    let h = render(&hard, 512, 0);
    for n in (24_000..72_000).step_by(997) {
        assert!((h.channels[0][n] - 0.25 * c).abs() < 1e-6, "frame {n}");
    }
}

#[test]
fn a_crossfade_is_click_free_where_a_hard_cut_is_not() {
    // Two stretches of a ramp that do not meet: the hard cut jumps, the crossfade does not.
    let a = ramp(120_000);
    let faded = song("clickfree", &a, |d, x| {
        put(d, x, 0, 2 * PPQ, |_| {});
        put(d, x, PPQ, 2 * PPQ, |y| y.source_offset = 60_000);
    });
    let cut = song("clickcut", &a, |d, x| {
        put(d, x, 0, 2 * PPQ, |y| y.xfade = None);
        put(d, x, PPQ, 2 * PPQ, |y| {
            y.source_offset = 60_000;
            y.xfade = None;
        });
    });
    let worst = |s: &Song| {
        let r = render(s, 512, 0);
        // Up to the end of the second clip (frame 72 000), which stops short of its ramp.
        r.channels[0][..70_000]
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max)
    };
    assert!(worst(&cut) > 0.1, "the hard cut jumps");
    assert!(worst(&faded) < 0.02, "the crossfade does not");
}

#[test]
fn reverse_plays_backwards_through_fades_and_the_console() {
    let c = console_gain();
    let asset = ramp(48_000);
    let s = song("reverse", &asset, |d, a| {
        put(d, a, 0, PPQ, |x| {
            x.source_offset = 6_000;
            x.reverse = true;
            x.fade_in = 2_000;
        });
    });
    let r = render(&s, 512, 0);
    let ch = &r.channels[0];
    // Clip frame n plays asset frame 6000 + 24000 - 1 - n.
    for n in [2_000usize, 5_000, 12_345, 23_999] {
        let want = c * asset.channels[0][6_000 + 24_000 - 1 - n];
        assert!((ch[n] - want).abs() < 3e-6, "frame {n}");
    }
    assert!(ch[100] < ch[2_500], "faded in at the start of the playback");
    assert!(ch[3_000] > ch[20_000], "and the ramp runs downwards");
    assert_close(ch, &expected(&s, &asset, ch.len(), c), 3e-6, "reference");
}

#[test]
fn clip_gain_and_gain_envelopes_scale_the_clip() {
    let c = console_gain();
    let asset = constant(0.25, 96_000);
    let s = song("envelope", &asset, |d, a| {
        put(d, a, 0, 3 * PPQ, |x| {
            x.gain_db = -6.0;
            x.envelope = vec![
                GainPoint {
                    frame: 6_000,
                    gain_db: -12.0,
                },
                GainPoint {
                    frame: 30_000,
                    gain_db: 0.0,
                },
                GainPoint {
                    frame: 54_000,
                    gain_db: 6.0,
                },
            ];
        });
    });
    let r = render(&s, 512, 0);
    let ch = &r.channels[0];
    let g = |db: f32| atomik_core::db_to_gain(db);
    let level = |n: usize| ch[n] / (0.25 * c);
    assert!((level(0) - g(-6.0) * g(-12.0)).abs() < 1e-5, "held before");
    assert!((level(30_000) - g(-6.0)).abs() < 1e-5, "on a point");
    assert!(
        (level(70_000) - g(-6.0) * g(6.0)).abs() < 1e-5,
        "held after"
    );
    // Between points the gain ramps in amplitude, with no steps.
    let mid = level(18_000);
    assert!(
        (mid - g(-6.0) * (g(-12.0) + 1.0) / 2.0).abs() < 1e-5,
        "{mid}"
    );
    for n in 1..72_000 {
        assert!((ch[n] - ch[n - 1]).abs() < 1e-4, "step at {n}");
    }
    assert_close(ch, &expected(&s, &asset, ch.len(), c), 3e-6, "reference");
}

#[test]
fn strip_silence_removes_the_quiet_parts_and_keeps_the_sound_in_place() {
    // Three bursts of noise separated by silence.
    let mut l = vec![0.0f32; 144_000];
    let n = noise(144_000);
    for (lo, hi) in [(4_000, 20_000), (50_000, 70_000), (110_000, 140_000)] {
        l[lo..hi].copy_from_slice(&n.channels[0][lo..hi]);
    }
    let asset = AudioData::new(48_000, vec![l.clone(), l]).unwrap();
    let mut s = song("strip", &asset, |d, a| {
        put(d, a, 0, 6 * PPQ, |_| {});
    });
    let before = render(&s, 512, 0);
    // Detect, then apply as the app does.
    let params = StripParams {
        threshold_db: -40.0,
        min_silence: 2_000,
        pad_before: 100,
        pad_after: 100,
    };
    let pieces = strip_silence(&asset.channels, 0, 144_000, &params);
    assert_eq!(pieces.len(), 3);
    let mut d = Document::new(s.doc.clone());
    let id = s.doc.song.tracks[0].lanes[0].clips[0].id;
    d.apply(
        Command::Song(SongCommand::StripSilence {
            id,
            pieces,
            fade: 48,
        }),
        &default_registry(),
    )
    .unwrap();
    s.doc = d.doc().clone();
    assert_eq!(s.doc.song.tracks[0].lanes[0].clips.len(), 3);
    let after = render(&s, 512, 0);
    let (b, a) = (&before.channels[0], &after.channels[0]);
    // The silent stretches are silent either way; inside the kept pieces (clear of the fades
    // and of tick rounding) the audio is the same samples at the same place.
    for (lo, hi) in [(4_200, 19_800), (50_200, 69_800), (110_200, 139_800)] {
        for n in lo..hi {
            assert!((a[n] - b[n]).abs() < 1e-6, "frame {n}");
        }
    }
    assert!(a[30_000..45_000].iter().all(|x| x.abs() < 1e-9));
}

#[test]
fn bounces_with_every_edit_are_bit_identical_across_blocks_and_workers() {
    let asset = noise(96_000);
    let s = song("identical", &asset, |d, a| {
        put(d, a, 0, 3 * PPQ, |x| {
            x.fade_in = 2_000;
            x.fade_curve = FadeCurve::SCurve;
            x.envelope = vec![
                GainPoint {
                    frame: 0,
                    gain_db: -9.0,
                },
                GainPoint {
                    frame: 50_000,
                    gain_db: 0.0,
                },
            ];
        });
        put(d, a, 2 * PPQ, 3 * PPQ, |x| {
            x.reverse = true;
            x.source_offset = 500;
            x.xfade = Some(FadeCurve::EqualPower);
            x.fade_out = 9_000;
            x.fade_curve = FadeCurve::Exponential;
        });
        put(d, a, 4 * PPQ, PPQ, |x| x.gain_db = 3.0);
    });
    let reference = bits(&render(&s, 64, 0));
    assert!(reference.iter().any(|b| f32::from_bits(*b) != 0.0));
    for block in [1, 64, 4096] {
        for workers in [0, 4] {
            assert!(
                bits(&render(&s, block, workers)) == reference,
                "block {block}, workers {workers}"
            );
        }
    }
    // Saved and loaded, it bounces the same bytes.
    let text = save_project(&s.doc);
    let loaded = load_project(&text, &default_registry()).unwrap();
    let again = Song {
        doc: loaded,
        bundle: Bundle::open(s.bundle.root()).unwrap(),
        asset: s.asset.clone(),
        _dir: TempDir::new("unused"),
    };
    assert!(bits(&render(&again, 512, 0)) == reference);
}

// ---- projects from before audio editing -------------------------------------------------

/// A version 22 project (clips with gain, fades and overlaps, no editing features) bounced by
/// the code before audio editing: the bytes must not change.
const V22_PROJECT: &str = include_str!("fixtures/audio_v22.atomik");
/// SHA-256 of the 240 000-frame stereo bounce as little-endian `f32`, taken before audio editing.
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
const V22_SHA256: &str = "737d0a3b0e05f98c834945eebdaed5daa3366e8aeb22a027efbe3427a6e14258";

fn v22_bundle(tag: &str) -> (Bundle, TempDir) {
    let dir = TempDir::new(tag);
    let mut bundle = Bundle::create(&dir.0.join("song")).unwrap();
    // The fixture's asset: the same xorshift noise as the generator that wrote the project.
    let info = bundle.add_recording(&noise(96_000), "noise.wav").unwrap();
    assert_eq!(
        info.hash, "d5745abdec5bd27f862a47bc2aac55040a6e5a3af512f8684d8e4fb9cd6bb9de",
        "the fixture asset changed"
    );
    (bundle, dir)
}

fn bounce_text(text: &str, bundle: &Bundle, block: usize) -> Rendered {
    render_project_json_in(
        text,
        &default_registry(),
        &options(block, 0),
        Some(bundle.root()),
    )
    .unwrap()
}

#[test]
fn a_version_22_project_loads_with_hard_cuts_and_bounces_the_same_as_before() {
    let (bundle, _dir) = v22_bundle("v22");
    // Loading marks every clip as a hard cut, so the overlaps are the old later-wins cuts.
    let doc = load_project(V22_PROJECT, &default_registry()).unwrap();
    let clips = &doc.song.tracks[0].lanes[0].clips;
    assert_eq!(clips.len(), 4);
    for c in clips {
        let ClipContent::Audio(a) = &c.content else {
            panic!()
        };
        assert_eq!(a.xfade, None);
        assert!(!a.reverse && a.envelope.is_empty());
    }
    let r = bounce_text(V22_PROJECT, &bundle, 512);
    assert_eq!(r.channels[0].len(), 240_000);
    // Saved by the new version it says so, loads again, and bounces the same bytes.
    let resaved = save_project(&doc);
    assert!(resaved.contains("\"version\": 28") && resaved.contains("\"xfade\": \"off\""));
    assert!(bits(&bounce_text(&resaved, &bundle, 64)) == bits(&r));
    // The same file with the version bumped and no "xfade" keys crossfades instead: only the
    // overlap (beats 2..3) differs.
    let bumped = V22_PROJECT.replace("\"version\": 22", "\"version\": 28");
    let x = bounce_text(&bumped, &bundle, 512);
    let beat = 24_000;
    for ch in 0..2 {
        assert_eq!(x.channels[ch][..2 * beat], r.channels[ch][..2 * beat]);
        assert_eq!(x.channels[ch][3 * beat..], r.channels[ch][3 * beat..]);
    }
    assert!(
        x.channels[0][2 * beat..3 * beat] != r.channels[0][2 * beat..3 * beat],
        "the overlap crossfades"
    );
    exact_hash(&r);
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn exact_hash(r: &Rendered) {
    use sha2::{Digest, Sha256};
    let mut bytes = Vec::new();
    for i in 0..r.channels[0].len() {
        for c in &r.channels {
            bytes.extend_from_slice(&c[i].to_le_bytes());
        }
    }
    let hash: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(hash, V22_SHA256, "the pre-editing bounce changed");
}

/// Exact bytes are checked on Windows x86-64 only (see `project.rs`); elsewhere the checks
/// above (load as hard cuts, same bytes as the same song with explicit cuts) stand.
#[cfg(not(all(target_os = "windows", target_arch = "x86_64")))]
fn exact_hash(_: &Rendered) {}
