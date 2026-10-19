//! [HI-1/HI-2]: a bounce with audio regions (Memory assets) -- two audio
//! tracks (one mono, equal-power and linear fades, a cut clip), a loop, a locate, a snapshot
//! swap, a tempo change and the metronome -- is bit-identical at host blocks {1, 64, 333, 4096}
//! and with {0, 3} workers, with no allocation on the audio thread (`assert_no_alloc` around
//! every render) or on any worker (`forbid_on_this_thread`). Own binary: the worker violation
//! counter is process-global.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::{PPQ, Ticks};
use atomik_devices::{audio_track, line_mixer};
use atomik_engine::{
    AssetId, Graph, HotEdit, OfflineJob, SongAudioRegion, SongFadeCurve, SongInput, TimedEdit,
    TransportCommand, compile_song, render_offline_song,
};
use atomik_testkit::no_alloc::{assert_no_alloc, forbid_on_this_thread, worker_violations};
use common::audio::{asset, audio_track_of, region};
use common::hot::{Rig, energy};
use common::song::tempo;
use common::*;

const FRAMES: usize = 90_000;

fn regions(variant: u8) -> (Vec<SongAudioRegion>, Vec<SongAudioRegion>) {
    let mut fade = region(3_000, 700, 20_000);
    fade.fade_in = 2_000;
    fade.fade_out = 3_000;
    fade.fade_curve = SongFadeCurve::EqualPower;
    fade.gain = 0.7;
    // A clip cut in two where a later clip covers it.
    let cut_a = SongAudioRegion {
        song_start_frame: 30_000,
        frames: 4_000,
        clip_frames: 9_000,
        fade_in: 500,
        fade_out: 1_500,
        fade_curve: SongFadeCurve::Linear,
        ..region(30_000, 100, 4_000)
    };
    let cut_b = SongAudioRegion {
        song_start_frame: 36_000,
        source_offset: 6_100,
        frames: 3_000,
        clip_pos: 6_000,
        ..cut_a
    };
    let mut a = vec![fade, cut_a, cut_b];
    let mut mono = region(1_000, 0, 60_000);
    mono.asset = AssetId(1);
    mono.gain = 0.4;
    let mut b = vec![mono];
    if variant == 1 {
        a.truncate(2);
        b[0].gain = 0.9;
        b[0].song_start_frame = 2_000;
    }
    (a, b)
}

fn input(variant: u8) -> SongInput {
    let (a, b) = regions(variant);
    SongInput {
        tracks: Vec::new(),
        // Tempo change inside the loop [PPQ, 3 PPQ).
        tempo: tempo(&[(0, 240.0), (2 * PPQ, 180.0)]),
        beats_per_bar: 4,
        audio: vec![audio_track_of(1, a), audio_track_of(2, b)],
        assets: vec![asset(2, 50_000), asset(1, 80_000)],
    }
}

fn render(threads: u16, block: usize) -> Vec<Vec<f32>> {
    let c = cfg(0, 2);
    let mut rig = Rig::new(&c);
    let t1 = rig.insert(0, 1, audio_track::factory(), &[]);
    let t2 = rig.insert(0, 2, audio_track::factory(), &[]);
    let m = rig.insert(0, 3, line_mixer::factory(), &[]);
    rig.stereo(t1, "out_", m, "ch1_in_");
    rig.stereo(t2, "out_", m, "ch2_in_");
    rig.connect((m, "master_l"), (Graph::INTERFACE, "out_1"));
    rig.connect((m, "master_r"), (Graph::INTERFACE, "out_2"));
    rig.route(0);
    let mut edits = rig.edits;
    edits.push(TimedEdit {
        frame: 20_000,
        edit: HotEdit::Transport(TransportCommand::Locate(Ticks(PPQ / 2))),
    });
    edits.push(TimedEdit {
        frame: 40_000,
        edit: HotEdit::Song(compile_song(&input(1)).unwrap()),
    });
    edits.push(TimedEdit {
        frame: 50_000,
        edit: HotEdit::Tempo(atomik_core::Tempo::new(100.0).unwrap()),
    });
    let job = OfflineJob {
        frames: FRAMES,
        host_block: block,
        notes: &[],
        params: &[],
        inputs: &[],
    };
    let baseline = worker_violations();
    let (mut engine, mut handle) = engine_with(c, threads, Some(forbid_on_this_thread));
    handle
        .transport(TransportCommand::SetLoop {
            start: Ticks(PPQ),
            end: Ticks(3 * PPQ),
            enabled: true,
        })
        .unwrap();
    handle
        .transport(TransportCommand::Metronome {
            enabled: true,
            gain: 0.3,
        })
        .unwrap();
    let song = compile_song(&input(0)).unwrap();
    let out = render_offline_song(&mut engine, &mut handle, &job, song, edits, |f| {
        assert_no_alloc(f)
    });
    assert_eq!(
        worker_violations() - baseline,
        0,
        "threads {threads}, block {block}"
    );
    let state = handle.transport_state();
    assert!(state.playing);
    assert!(state.loop_count >= 1, "the loop wrapped");
    drop(engine);
    out
}

#[test]
fn ac_a3_audio_bounce_is_bit_identical_and_allocation_free() {
    let reference = render(0, 64);
    assert_eq!(reference.len(), 2);
    assert_eq!(reference[0].len(), FRAMES);
    assert!(
        energy(&reference[0], 0..FRAMES) > 10.0,
        "the song is audible"
    );
    assert!(
        energy(&reference[1], 60_000..FRAMES) > 1.0,
        "still playing after the swap"
    );
    for threads in [0u16, 3] {
        for block in [1usize, 64, 333, 4096] {
            let out = render(threads, block);
            assert_eq!(
                bits(&out),
                bits(&reference),
                "threads {threads}, block {block}"
            );
        }
    }
}
