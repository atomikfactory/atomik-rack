//! [HI-1/HI-2]: a song with notes, automation, a loop, a tempo change inside the loop,
//! the metronome, a locate and a snapshot swap while playing bounces bit-identically at host
//! blocks {1, 64, 333, 4096} and with {0, 3} workers, with no allocation on the audio thread
//! (`assert_no_alloc` around every render) or on any worker (`forbid_on_this_thread`). Own
//! binary: the worker violation counter is process-global.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_core::{PPQ, Ticks};
use atomik_devices::{line_mixer, poly_synth};
use atomik_engine::{
    DeviceInstanceId, Graph, HotEdit, OfflineJob, SongAutomation, SongCurve, SongInput, SongNote,
    SongPoint, SongTrack, TimedEdit, TransportCommand, compile_song, render_offline_song,
};
use atomik_testkit::no_alloc::{assert_no_alloc, forbid_on_this_thread, worker_violations};
use common::hot::{Rig, energy, param_id};
use common::song::{snote, tempo};
use common::*;

const FRAMES: usize = 72_000;

fn notes_socket() -> atomik_device_sdk::SocketId {
    poly_synth::factory()
        .descriptor
        .socket_by_key("notes_in")
        .unwrap()
        .id
}

fn song_input(s1: DeviceInstanceId, s2: DeviceInstanceId, variant: u8) -> SongInput {
    let cutoff = poly_synth::factory()
        .descriptor
        .param_by_key("filter.cutoff")
        .unwrap();
    let lin = SongCurve::Linear;
    let pt = |tick, value, curve| SongPoint {
        tick: Ticks(tick),
        value,
        curve,
    };
    let mut lead: Vec<SongNote> = [60, 64, 67]
        .into_iter()
        .map(|k| snote(0, 2 * PPQ, k))
        .collect();
    lead.push(snote(5 * PPQ / 2, PPQ, 72)); // crosses the loop end
    let bass: Vec<SongNote> = (0..8)
        .map(|k| snote(k * PPQ / 2, PPQ / 4, 36 + (k as u8 + variant) % 5))
        .collect();
    SongInput {
        tracks: vec![
            SongTrack {
                device: s1,
                notes_socket: notes_socket(),
                notes: lead,
                automation: vec![SongAutomation {
                    param: param_id(poly_synth::factory(), "filter.cutoff"),
                    points: vec![
                        pt(0, cutoff.min, lin),
                        pt(PPQ, cutoff.max, SongCurve::Step),
                        pt(2 * PPQ, (cutoff.min + cutoff.max) / 2.0, lin),
                        pt(3 * PPQ, cutoff.min, lin),
                    ],
                }],
            },
            SongTrack {
                device: s2,
                notes_socket: notes_socket(),
                notes: bass,
                automation: Vec::new(),
            },
        ],
        // Tempo change inside the loop [PPQ, 3 PPQ).
        tempo: tempo(&[(0, 240.0), (2 * PPQ, 180.0)]),
        beats_per_bar: 4,
        audio: Vec::new(),
        assets: Vec::new(),
    }
}

fn render(threads: u16, block: usize) -> Vec<Vec<f32>> {
    let c = cfg(0, 2);
    let mut rig = Rig::new(&c);
    let s1 = rig.insert(0, 1, poly_synth::factory(), &[("osc1.wave", 0.0)]);
    let s2 = rig.insert(0, 2, poly_synth::factory(), &[("osc1.wave", 1.0)]);
    let m = rig.insert(0, 3, line_mixer::factory(), &[]);
    rig.stereo(s1, "out_", m, "ch1_in_");
    rig.stereo(s2, "out_", m, "ch2_in_");
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
        edit: HotEdit::Song(compile_song(&song_input(s1, s2, 1)).unwrap()),
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
    let song = compile_song(&song_input(s1, s2, 0)).unwrap();
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
    assert_eq!(
        handle.dropped_events(),
        0,
        "every song note reached its synth"
    );
    drop(engine);
    out
}

#[test]
fn ac_s2_song_bounce_is_bit_identical_and_allocation_free() {
    let reference = render(0, 64);
    assert_eq!(reference.len(), 2);
    assert_eq!(reference[0].len(), FRAMES);
    assert!(
        energy(&reference[0], 0..FRAMES) > 1.0,
        "the song is audible"
    );
    assert!(
        energy(&reference[0], 60_000..FRAMES) > 0.1,
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
