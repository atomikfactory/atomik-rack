//! Audio slots of the clip launcher: a launched audio slot plays its regions from its start and
//! loops every cycle while the track's arrangement audio is silent, a stop gives the arrangement
//! back, and the render allocates nothing.
#![allow(clippy::needless_range_loop)] // Justification: sample-index loops read clearer than zipped iterators in these checks.
atomik_testkit::install_alloc_guard!();

mod common;

use atomik_engine::{
    DeviceInstanceId, HotEdit, LaunchAudioInput, LaunchCommand, LaunchGrid, LaunchSlotInput,
    LaunchTrackInput, TransportCommand,
};
use common::audio::*;

/// 3200 ticks are 5000 frames at 120 BPM and 48 kHz.
const SLOT_TICKS: i64 = 3_200;
const CYCLE: usize = 5_000;

fn audio_slot(frames: u64) -> LaunchSlotInput {
    LaunchSlotInput {
        length: SLOT_TICKS,
        notes: Vec::new(),
        controllers: Vec::new(),
        expression: Vec::new(),
        audio: Some(LaunchAudioInput {
            regions: vec![region(0, 0, frames)],
            envelopes: Vec::new(),
            assets: vec![asset(2, 20_000)],
        }),
    }
}

fn launch_track(slot: LaunchSlotInput) -> LaunchTrackInput {
    LaunchTrackInput {
        device: DeviceInstanceId(1),
        notes_socket: atomik_device_sdk::SocketId(0),
        slots: vec![Some(slot)],
    }
}

fn launch(frame: u64, command: LaunchCommand) -> atomik_engine::TimedEdit {
    edit(frame, HotEdit::Transport(TransportCommand::Launch(command)))
}

fn go(device: u32) -> LaunchCommand {
    LaunchCommand::Launch {
        device: DeviceInstanceId(device),
        slot: 0,
        grid: LaunchGrid::Ticks(1),
    }
}

fn run(total: usize, edits: Vec<atomik_engine::TimedEdit>, arrangement: bool) -> Vec<Vec<f32>> {
    // The arrangement of the track plays the asset from frame 1000 on, over the whole render.
    let regions = if arrangement {
        vec![region(0, 1_000, 20_000)]
    } else {
        Vec::new()
    };
    let mut b = Bounce::new(
        audio_input(vec![audio_track_of(1, regions)], vec![asset(2, 20_000)]),
        total,
    );
    b.launch = vec![launch_track(audio_slot(4_000))];
    b.edits = edits;
    b.run()
}

#[test]
fn a_launched_audio_slot_loops_and_the_arrangement_is_silent() {
    let out = run(12_000, vec![launch(0, go(1))], true);
    for n in 0..12_000usize {
        let local = n % CYCLE;
        for (c, ch) in out.iter().enumerate() {
            let want = if local < 4_000 { value(c, local) } else { 0.0 };
            assert_eq!(ch[n].to_bits(), want.to_bits(), "channel {c}, frame {n}");
        }
    }
}

#[test]
fn a_stop_gives_the_arrangement_back_at_its_boundary() {
    let stop = LaunchCommand::Stop {
        device: DeviceInstanceId(1),
        grid: LaunchGrid::Ticks(3_200),
    };
    // The stop is queued at frame 2000 and waits for tick 3200 (frame 5000).
    let out = run(12_000, vec![launch(0, go(1)), launch(2_000, stop)], true);
    for n in 0..5_000usize {
        let want = if n < 4_000 { value(0, n) } else { 0.0 };
        assert_eq!(out[0][n].to_bits(), want.to_bits(), "slot, frame {n}");
    }
    // The arrangement comes back with the next batch (64 frames after the boundary at most).
    assert!(out[0][5_000..5_056].iter().all(|s| s.to_bits() == 0));
    for n in 5_056..12_000usize {
        assert_eq!(
            out[0][n].to_bits(),
            value(0, 1_000 + n).to_bits(),
            "arrangement, frame {n}"
        );
    }
}

#[test]
fn a_track_with_no_arrangement_audio_plays_its_slot_and_falls_silent_on_stop() {
    let stop = LaunchCommand::Stop {
        device: DeviceInstanceId(1),
        grid: LaunchGrid::Ticks(3_200),
    };
    let out = run(9_000, vec![launch(0, go(1)), launch(2_000, stop)], false);
    for n in 0..5_000usize {
        let want = if n < 4_000 { value(1, n) } else { 0.0 };
        assert_eq!(out[1][n].to_bits(), want.to_bits(), "slot, frame {n}");
    }
    assert!(
        out[0][5_000..].iter().all(|s| s.to_bits() == 0),
        "silent after the stop"
    );
}
