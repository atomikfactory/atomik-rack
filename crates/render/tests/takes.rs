//! Take groups in a bounce: the comp plays the segments it names (audio and notes), the splice
//! between two takes of an audio comp does not click where the comp fades (and a plain splice
//! does), a comp bounces exactly like the plain clips it compiles to and like the flattened
//! group, and the bounce does not depend on the host block or the workers.

use atomik_audio_file::{AudioData, Bundle};
use atomik_core::PPQ;
use atomik_model::{
    AssetMeta, Clip, ClipContent, ClipId, Command, DeviceKey, Document, FadeCurve, Note, RackDoc,
    SongCommand, empty_doc, load_project, save_project,
};
use atomik_render::project::{ProjectOptions, render_doc, render_doc_in};
use atomik_render::{Rendered, default_registry};
use atomik_testkit::signal::dominant_frequency;
use std::num::NonZeroUsize;
use std::path::PathBuf;

const BAR: i64 = 4 * PPQ;
const SR: f64 = 48_000.0;
/// One bar at 120 BPM and 48 kHz.
const BAR_FRAMES: usize = 96_000;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("atomik-render-takes-{tag}-{}", std::process::id()));
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

fn apply(d: &mut Document, c: Command) {
    d.apply(c.clone(), &default_registry())
        .unwrap_or_else(|e| panic!("{c:?}: {e}"));
}

fn song(c: SongCommand) -> Command {
    Command::Song(c)
}

// ---- audio ------------------------------------------------------------------------------------

/// A mono-in-stereo sine of `hz` and amplitude 0.5, `frames` long. The frequencies of the tests
/// are not whole cycles per bar, so two takes never meet at the same phase.
fn tone(hz: f64, frames: usize) -> AudioData {
    let ch: Vec<f32> = (0..frames)
        .map(|n| (0.5 * (2.0 * std::f64::consts::PI * hz * n as f64 / SR).sin()) as f32)
        .collect();
    AudioData::new(48_000, vec![ch.clone(), ch]).unwrap()
}

const HZ_A: f64 = 441.3;
const HZ_B: f64 = 662.9;

/// An audio track with two long tones, take A (asset 0) and take B, each four bars long, as a
/// take group in recording order [A, B], or (`grouped` false) nothing on the lane yet.
fn audio_project(dir: &std::path::Path) -> (Document, Bundle, [String; 2]) {
    let mut bundle = Bundle::create(dir).unwrap();
    let mut d = Document::new(empty_doc());
    let mut hashes = Vec::new();
    for hz in [HZ_A, HZ_B] {
        let info = bundle
            .add_recording(&tone(hz, 4 * BAR_FRAMES + 10_000), "tone.wav")
            .unwrap();
        apply(
            &mut d,
            Command::RegisterAsset {
                hash: info.hash.clone(),
                meta: AssetMeta {
                    channels: info.channels,
                    sample_rate: info.sample_rate,
                    frames: info.frames,
                    original_name: info.original_name.clone(),
                },
            },
        );
        hashes.push(info.hash);
    }
    apply(&mut d, song(SongCommand::AddAudioTrack { name: None }));
    apply(&mut d, song(SongCommand::SetSongEnd(4 * BAR)));
    let hashes: [String; 2] = hashes.try_into().unwrap();
    (d, bundle, hashes)
}

fn audio_take(asset: &str) -> Clip {
    Clip {
        id: ClipId(0),
        start: 0,
        length: 4 * BAR,
        offset: 0,
        loop_length: None,
        muted: false,
        grid: None,
        scale: None,
        controllers: Vec::new(),
        content: ClipContent::Audio(atomik_model::AudioClipContent {
            asset: asset.to_owned(),
            source_offset: 0,
            gain_db: 0.0,
            fade_in: 0,
            fade_out: 0,
            fade_curve: FadeCurve::Linear,
            reverse: false,
            envelope: Vec::new(),
            xfade: None,
            warp: Default::default(),
        }),
    }
}

/// The comp of the tests: take A for bar 2, take B for the rest.
fn comped_audio(dir: &std::path::Path) -> (Document, Bundle, [String; 2]) {
    let (mut d, bundle, hashes) = audio_project(dir);
    apply(
        &mut d,
        song(SongCommand::NewTakeGroup {
            device: DeviceKey("audio-track".into()),
            lane: 0,
            takes: vec![audio_take(&hashes[0]), audio_take(&hashes[1])],
        }),
    );
    apply(
        &mut d,
        song(SongCommand::SetCompSegment {
            group: 1,
            take: 0,
            start: BAR,
            end: 2 * BAR,
        }),
    );
    (d, bundle, hashes)
}

fn render(doc: &RackDoc, bundle: &Bundle, block: usize, workers: u16) -> Rendered {
    let options = ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    };
    render_doc_in(doc, &default_registry(), &options, Some(bundle.root())).unwrap()
}

/// The largest sample-to-sample step in `x[a..b]`.
fn max_step(x: &[f32], a: usize, b: usize) -> f32 {
    x[a..b]
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max)
}

fn bits(r: &Rendered) -> Vec<u32> {
    r.channels.iter().flatten().map(|x| x.to_bits()).collect()
}

#[test]
fn an_audio_comp_plays_the_tone_of_the_take_each_segment_names() {
    let t = TempDir::new("tones");
    let (d, bundle, _) = comped_audio(&t.0.join("song"));
    let out = render(d.doc(), &bundle, 512, 0).channels.remove(0);
    let at = |bar: usize| {
        // The middle of the bar, clear of the splices.
        dominant_frequency(
            &out[bar * BAR_FRAMES + 20_000..(bar + 1) * BAR_FRAMES - 20_000],
            SR,
        )
    };
    assert!((at(0) - HZ_B).abs() < 3.0, "bar 1 is take B: {}", at(0));
    assert!((at(1) - HZ_A).abs() < 3.0, "bar 2 is take A: {}", at(1));
    assert!((at(2) - HZ_B).abs() < 3.0, "bar 3 is take B: {}", at(2));
    // Sample exact inside a segment: bar 2 reads take A from the asset position of bar 2 (the
    // audio track's centre pan law is -3 dB).
    let want = (std::f64::consts::FRAC_1_SQRT_2
        * 0.5
        * (2.0 * std::f64::consts::PI * HZ_A * (BAR_FRAMES + 50_000) as f64 / SR).sin())
        as f32;
    assert!(
        (out[BAR_FRAMES + 50_000] - want).abs() < 1e-5,
        "{} vs {want}",
        out[BAR_FRAMES + 50_000]
    );
}

#[test]
fn the_comp_splices_do_not_click_and_a_plain_splice_does() {
    let t = TempDir::new("clicks");
    let (d, bundle, hashes) = comped_audio(&t.0.join("song"));
    let out = render(d.doc(), &bundle, 512, 0).channels.remove(0);
    // A tone of this frequency and level steps at most 2 pi f / sr * 0.5 per sample.
    let natural = (2.0 * std::f64::consts::PI * HZ_B / SR * 0.5) as f32 * 1.05;
    for boundary in [BAR_FRAMES, 2 * BAR_FRAMES] {
        let step = max_step(&out, boundary - 400, boundary + 400);
        assert!(
            step <= natural,
            "splice at {boundary}: step {step} above {natural}"
        );
        // The fades dip to silence at the splice itself.
        assert!(out[boundary - 1].abs() < 0.01 || out[boundary].abs() < 0.01);
    }
    // The same cuts without fades, as plain clips: the splice jumps.
    let (mut p, pbundle, _) = audio_project(&t.0.join("plain"));
    let plain = |p: &mut Document, asset: &str, start: i64, length: i64, offset: u64| {
        apply(
            p,
            song(SongCommand::AddAudioClip {
                device: DeviceKey("audio-track".into()),
                start,
                length,
                asset: asset.to_owned(),
                source_offset: offset,
                gain_db: 0.0,
                fade_in: 0,
                fade_out: 0,
                fade_curve: FadeCurve::Linear,
            }),
        );
    };
    plain(&mut p, &hashes[1], 0, BAR, 0);
    plain(&mut p, &hashes[0], BAR, BAR, BAR_FRAMES as u64);
    plain(&mut p, &hashes[1], 2 * BAR, 2 * BAR, 2 * BAR_FRAMES as u64);
    let hard = render(p.doc(), &pbundle, 512, 0).channels.remove(0);
    let worst = max_step(&hard, BAR_FRAMES - 400, 2 * BAR_FRAMES + 400);
    assert!(worst > 0.15, "the unfaded splice must click, step {worst}");
    // Away from the splices the two bounces are the same samples.
    assert_eq!(
        out[BAR_FRAMES + 5_000].to_bits(),
        hard[BAR_FRAMES + 5_000].to_bits()
    );
    assert_eq!(
        out[3 * BAR_FRAMES].to_bits(),
        hard[3 * BAR_FRAMES].to_bits()
    );
}

#[test]
fn picking_a_take_bounces_that_take_alone_and_undo_brings_the_comp_back() {
    let t = TempDir::new("pick");
    let (mut d, bundle, _) = comped_audio(&t.0.join("song"));
    let comp = bits(&render(d.doc(), &bundle, 512, 0));
    apply(&mut d, song(SongCommand::PickTake { group: 1, take: 0 }));
    let out = render(d.doc(), &bundle, 512, 0).channels.remove(0);
    for bar in 0..4 {
        let f = dominant_frequency(
            &out[bar * BAR_FRAMES + 20_000..(bar + 1) * BAR_FRAMES - 20_000],
            SR,
        );
        assert!((f - HZ_A).abs() < 3.0, "bar {}: {f}", bar + 1);
    }
    d.undo(&default_registry()).unwrap();
    assert!(bits(&render(d.doc(), &bundle, 512, 0)) == comp);
}

#[test]
fn a_comp_bounces_like_the_flattened_group_and_at_any_block_and_worker_count() {
    let t = TempDir::new("same");
    let (mut d, bundle, _) = comped_audio(&t.0.join("song"));
    let reference = bits(&render(d.doc(), &bundle, 512, 0));
    assert!(reference.iter().any(|b| f32::from_bits(*b) != 0.0));
    for (block, workers) in [(64, 0), (333, 0), (4096, 0), (512, 2), (64, 3)] {
        assert!(
            bits(&render(d.doc(), &bundle, block, workers)) == reference,
            "block {block}, workers {workers}"
        );
    }
    // Flatten: the takes go, the plain clips stay, the sound is the same.
    apply(&mut d, song(SongCommand::FlattenTakes { group: 1 }));
    assert_eq!(d.doc().song.take_groups().count(), 0);
    assert!(bits(&render(d.doc(), &bundle, 512, 0)) == reference);
    // And a saved and loaded project bounces the same.
    let loaded = load_project(&save_project(d.doc()), &default_registry()).unwrap();
    assert!(bits(&render(&loaded, &bundle, 512, 0)) == reference);
}

// ---- notes ------------------------------------------------------------------------------------

/// A sine synth track with three takes of three bars: take `t` holds key `60 + 4 t` for every bar.
fn notes_project() -> (Document, [u8; 3]) {
    let keys = [60, 64, 67];
    let synth = DeviceKey("synth".into());
    let mut d = Document::new(empty_doc());
    apply(
        &mut d,
        Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".into(),
            key: Some(synth.clone()),
        },
    );
    apply(
        &mut d,
        song(SongCommand::AddTrack {
            device: synth.clone(),
        }),
    );
    apply(&mut d, song(SongCommand::SetSongEnd(3 * BAR)));
    let take = |key: u8| Clip {
        id: ClipId(0),
        start: 0,
        length: 3 * BAR,
        offset: 0,
        loop_length: None,
        muted: false,
        grid: None,
        scale: None,
        controllers: Vec::new(),
        content: ClipContent::Notes(
            (0..3)
                .map(|b| Note::new(b * BAR, BAR - PPQ, key, 1.0))
                .collect(),
        ),
    };
    apply(
        &mut d,
        song(SongCommand::NewTakeGroup {
            device: synth.clone(),
            lane: 0,
            takes: keys.iter().map(|k| take(*k)).collect(),
        }),
    );
    // A pure sine so the pitch is easy to read.
    let mut doc = d.doc().clone();
    let dev = doc.devices.iter_mut().find(|x| x.key == synth).unwrap();
    for (k, v) in [
        ("osc1.wave", 3.0),
        ("osc.mix", 0.0),
        ("filter.cutoff", 20_000.0),
        ("filter.env_amount", 0.0),
        ("filter.key_track", 0.0),
        ("filter.resonance", 0.0),
    ] {
        dev.params.insert(k.to_owned(), v);
    }
    (Document::new(doc), keys)
}

fn hz(key: u8) -> f64 {
    440.0 * 2f64.powf((f64::from(key) - 69.0) / 12.0)
}

fn bounce(doc: &RackDoc, block: usize, workers: u16) -> Vec<f32> {
    let options = ProjectOptions {
        host_block: NonZeroUsize::new(block).unwrap(),
        workers,
        loop_count: None,
    };
    render_doc(doc, &default_registry(), &options)
        .unwrap()
        .channels
        .remove(0)
}

#[test]
fn a_note_comp_plays_each_bar_from_the_take_it_names() {
    let (mut d, keys) = notes_project();
    // A fresh group plays the last take; comp bar 1 from take 0 and bar 2 from take 1.
    for (take, bar) in [(0usize, 0i64), (1, 1)] {
        apply(
            &mut d,
            song(SongCommand::SetCompSegment {
                group: 1,
                take,
                start: bar * BAR,
                end: (bar + 1) * BAR,
            }),
        );
    }
    let out = bounce(d.doc(), 512, 0);
    for (bar, key) in [(0usize, keys[0]), (1, keys[1]), (2, keys[2])] {
        // The note holds three beats of the bar: read the middle of it.
        let a = bar * BAR_FRAMES + 20_000;
        let f = dominant_frequency(&out[a..a + 40_000], SR);
        assert!(
            (f - hz(key)).abs() < 2.0,
            "bar {}: {f} vs {}",
            bar + 1,
            hz(key)
        );
    }
}

#[test]
fn a_note_comp_bounces_exactly_like_the_plain_clips_it_compiles_to() {
    let (mut d, _) = notes_project();
    apply(
        &mut d,
        song(SongCommand::SetCompSegment {
            group: 1,
            take: 0,
            start: BAR,
            end: 2 * BAR,
        }),
    );
    let comp = bounce(d.doc(), 512, 0);
    assert!(comp.iter().any(|s| *s != 0.0));
    // The same song as plain clips: flatten, and (separately) rebuild the three clips by hand.
    let mut flat = d.clone();
    apply(&mut flat, song(SongCommand::FlattenTakes { group: 1 }));
    assert!(bounce(flat.doc(), 512, 0) == comp);
    // The determinism contract: any block and any worker count.
    for (block, workers) in [(1, 0), (64, 0), (333, 2), (4096, 3)] {
        assert!(
            bounce(d.doc(), block, workers) == comp,
            "block {block}, workers {workers}"
        );
    }
    // A comp that picks one whole take sounds like a plain clip of that take.
    let mut pick = d.clone();
    apply(&mut pick, song(SongCommand::PickTake { group: 1, take: 2 }));
    let mut plain = Document::new(empty_doc());
    for c in [
        Command::AddDevice {
            index: usize::MAX,
            type_id: "atomik.poly-synth".into(),
            key: Some(DeviceKey("synth".into())),
        },
        song(SongCommand::AddTrack {
            device: DeviceKey("synth".into()),
        }),
        song(SongCommand::SetSongEnd(3 * BAR)),
    ] {
        apply(&mut plain, c);
    }
    let picked_take = d.doc().song.take_groups().next().unwrap().2.takes[2].clone();
    apply(
        &mut plain,
        song(SongCommand::AddClip {
            device: DeviceKey("synth".into()),
            lane: 0,
            clip: picked_take,
        }),
    );
    let mut plain_doc = plain.doc().clone();
    plain_doc.devices[0].params = pick.doc().devices[0].params.clone();
    assert!(bounce(&plain_doc, 512, 0) == bounce(pick.doc(), 512, 0));
}
