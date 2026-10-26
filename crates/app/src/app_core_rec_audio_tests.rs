//! Audio recording, monitoring and streaming through the core (part 2). A real
//! offline engine ([`OfflineSink`]) is rendered by hand with scripted input, so the tap, the
//! transport states, `position_at` and the writer thread are all the real ones.

use super::*;
use crate::offline_sink::OfflineSink;
use crate::sink::{RecordingSink, SinkCall};
use atomik_audio_file::{AudioData, Bundle, write_wav_f32};
use atomik_core::PPQ;
use atomik_engine::SongAsset;
use atomik_model::song::AUDIO_TRACK_TYPE;
use atomik_model::{ClipContent, empty_doc};
use atomik_ui::TransportAction;

/// Input channel `c` at host frame `f`: unique per frame and exactly representable.
fn sample(c: usize, f: u64) -> f32 {
    (f as f32) * 2.0 + c as f32
}

fn core_with(inputs: u16) -> AppCore<OfflineSink> {
    let mut doc = empty_doc();
    doc.input_channels = inputs;
    let mut sink = OfflineSink::new();
    sink.input = Box::new(sample);
    AppCore::with_doc(sink, doc, None)
}

fn add_audio_track(core: &mut AppCore<impl EngineSink>, name: &str) -> DeviceKey {
    core.apply_command(Command::Song(SongCommand::AddAudioTrack {
        name: Some(name.to_owned()),
    }))
    .unwrap();
    core.doc().song.tracks.last().unwrap().device.clone()
}

fn arm(core: &mut AppCore<impl EngineSink>, key: &DeviceKey, armed: bool) {
    core.apply_command(Command::Song(SongCommand::SetTrackFlags {
        device: key.clone(),
        muted: false,
        solo: false,
        armed,
    }))
    .unwrap();
}

fn pump(core: &mut AppCore<OfflineSink>, frames: usize, block: usize) {
    let mut done = 0;
    while done < frames {
        let n = block.min(frames - done);
        core.sink_mut().render(n);
        core.frame();
        done += n;
    }
}

/// Engine frame at which the transport started from song frame 0 (no loop yet).
fn start_frame(core: &AppCore<OfflineSink>) -> u64 {
    let st = core.sink().transport_state();
    assert!(st.playing);
    st.engine_frame - st.song_frame
}

fn clips(core: &AppCore<impl EngineSink>, key: &DeviceKey) -> Vec<atomik_model::Clip> {
    core.doc().song.track(key).unwrap().lanes[0].clips.clone()
}

/// The takes of the one take group on the track's lane.
fn takes(core: &AppCore<impl EngineSink>, key: &DeviceKey) -> Vec<atomik_model::Clip> {
    let groups = &core.doc().song.track(key).unwrap().lanes[0].take_groups;
    assert_eq!(groups.len(), 1, "one group per recording pass");
    groups[0].takes.clone()
}

fn asset_of(c: &atomik_model::Clip) -> (&str, u64) {
    match &c.content {
        ClipContent::Audio(a) => (a.asset.as_str(), a.source_offset),
        other => panic!("not an audio clip: {other:?}"),
    }
}

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|s| s.to_bits()).collect()
}

fn bundle_of(core: &AppCore<impl EngineSink>) -> Bundle {
    Bundle::open(core.bundle_root().expect("a bundle")).unwrap()
}

#[test]
fn recording_places_the_take_on_the_timeline_with_the_latency_compensated_exactly() {
    let mut core = core_with(2);
    core.sink_mut().compensation = 300;
    let key = add_audio_track(&mut core, "Guitar");
    arm(&mut core, &key, true);
    core.transport_action(TransportAction::ToggleRecord);
    assert!(core.is_recording());
    pump(&mut core, 4_800, 480);
    let f_start = start_frame(&core);
    pump(&mut core, 48_000, 480);
    core.transport_action(TransportAction::Stop);
    assert!(!core.is_recording());
    pump(&mut core, 4_800, 480);
    core.wait_background();

    let list = clips(&core, &key);
    assert_eq!(list.len(), 1, "one take, one clip");
    let (hash, offset) = asset_of(&list[0]);
    let rate = core.doc().sample_rate;
    // The first kept frame is song frame 0 (everything before the transport started, plus the
    // compensation, is dropped): tick 0, no offset.
    assert_eq!(list[0].start, 0);
    assert_eq!(offset, 0);
    let data = bundle_of(&core).load(hash).unwrap();
    assert_eq!(
        data.channels.len(),
        2,
        "the default track records a stereo pair"
    );
    assert!(data.frames() > 48_000);
    // Every sample is where it was played: input frame E lies at song frame E - f_start - 300.
    for (c, ch) in data.channels.iter().enumerate() {
        let expect: Vec<u32> = (0..ch.len() as u64)
            .map(|j| sample(c, f_start + 300 + j).to_bits())
            .collect();
        assert_eq!(bits(ch), expect, "channel {c} is bit-exact and aligned");
    }
    // The clip covers the audio, in whole ticks.
    let frames = data.frames() as u64;
    let end_frame = core
        .doc()
        .song
        .frame_at(list[0].start + list[0].length, rate);
    assert!(
        end_frame >= frames && end_frame - frames < 4,
        "{end_frame} vs {frames}"
    );
    // Undo removes the clip; the asset stays registered (a save prunes it).
    assert!(core.undo());
    assert!(clips(&core, &key).is_empty());
    assert!(core.doc().assets.contains_key(hash));
    assert!(core.redo());
    assert_eq!(clips(&core, &key).len(), 1);
}

#[test]
fn the_compensation_follows_the_sign_convention_a_positive_value_moves_the_audio_earlier() {
    // With 0 compensation the audio of engine frame E is at song frame E - f_start, with 480
    // frames it is 480 earlier: input frame (f_start + 480) is song frame 0.
    for comp in [0i64, 480] {
        let mut core = core_with(1);
        core.sink_mut().compensation = comp;
        let key = add_audio_track(&mut core, "Line");
        arm(&mut core, &key, true);
        core.transport_action(TransportAction::ToggleRecord);
        pump(&mut core, 4_800, 480);
        let f_start = start_frame(&core);
        pump(&mut core, 9_600, 480);
        core.transport_action(TransportAction::Stop);
        pump(&mut core, 2_400, 480);
        core.wait_background();
        let list = clips(&core, &key);
        let (hash, _) = asset_of(&list[0]);
        let data = bundle_of(&core).load(hash).unwrap();
        assert_eq!(
            data.channels.len(),
            1,
            "one host input: both sockets from in_1, mono"
        );
        assert_eq!(
            data.channels[0][0].to_bits(),
            sample(0, f_start + comp as u64).to_bits(),
            "comp {comp}: song frame 0 holds engine frame f_start + comp"
        );
    }
}

#[test]
fn each_loop_pass_becomes_one_take_of_one_group_and_the_group_is_one_undo_step() {
    let mut core = core_with(2);
    core.sink_mut().compensation = 200;
    let key = add_audio_track(&mut core, "Voice");
    // A half-beat loop: 24 000 frames at 120 BPM and 48 kHz.
    core.apply_command(Command::Song(SongCommand::SetLoop {
        range: (0, PPQ),
        enabled: true,
    }))
    .unwrap();
    arm(&mut core, &key, true);
    core.transport_action(TransportAction::ToggleRecord);
    pump(&mut core, 2_400, 480);
    let f_start = start_frame(&core);
    // 2.6 passes.
    pump(&mut core, 62_400 - 2_400, 480);
    core.transport_action(TransportAction::Stop);
    pump(&mut core, 4_800, 480);
    core.wait_background();

    let list = takes(&core, &key);
    assert_eq!(list.len(), 3, "passes 0, 1 and 2");
    let loop_frames = 24_000u64;
    for (lap, clip) in list.iter().enumerate() {
        let (hash, offset) = asset_of(clip);
        let data = bundle_of(&core).load(hash).unwrap();
        // Lap n holds song frames 0.. of pass n: engine frame f_start + n * loop + 200 + j.
        let n = data.frames() as u64;
        if lap < 2 {
            assert_eq!(n, loop_frames, "lap {lap} is one whole pass");
        } else {
            assert!(n < loop_frames, "the last pass is partial: {n}");
        }
        assert_eq!(clip.start, 0, "every pass starts at the loop start");
        assert_eq!(offset, 0);
        let expect: Vec<u32> = (0..n)
            .map(|j| sample(0, f_start + lap as u64 * loop_frames + 200 + j).to_bits())
            .collect();
        assert_eq!(bits(&data.channels[0]), expect, "lap {lap}");
        if lap < 2 {
            assert_eq!(clip.length, PPQ, "a whole pass is the loop length");
        }
    }
    // The last pass plays where it has audio and the pass before it for the rest of the loop:
    // two compiled clips on the lane, the takes themselves are not there.
    let group = &core.doc().song.track(&key).unwrap().lanes[0].take_groups[0];
    assert_eq!(group.comp.len(), 2);
    assert_eq!((group.comp[0].take, group.comp[1].take), (2, 1));
    let lane = clips(&core, &key);
    assert_eq!(lane.len(), 2);
    assert_eq!((lane[0].start, lane[0].length), (0, group.comp[0].end));
    assert_eq!((lane[1].start, lane[1].end()), (group.comp[1].start, PPQ));
    // One undo step for the whole pass: the group and its clips go, the arm is a step of its own.
    assert!(core.undo());
    assert!(clips(&core, &key).is_empty());
    assert!(
        core.doc().song.track(&key).unwrap().lanes[0]
            .take_groups
            .is_empty()
    );
    assert!(
        core.doc().song.track(&key).unwrap().armed,
        "the arm is a step of its own"
    );
    assert!(core.undo());
    assert!(!core.doc().song.track(&key).unwrap().armed);
}

#[test]
fn several_armed_tracks_record_their_own_inputs_in_one_take() {
    let mut core = core_with(4);
    let a = add_audio_track(&mut core, "A");
    let b = add_audio_track(&mut core, "B");
    // B takes only input 4 (mono): in_l from in_4, nothing on in_r.
    for (sock, from) in [("in_l", Some("in_4")), ("in_r", None)] {
        core.apply_command(Command::Disconnect {
            to: atomik_model::SocketRef {
                device: b.clone(),
                socket: sock.into(),
            },
        })
        .unwrap();
        if let Some(from) = from {
            core.apply_command(Command::Connect(atomik_model::Cable {
                from: atomik_model::SocketRef {
                    device: DeviceKey("interface".into()),
                    socket: from.into(),
                },
                to: atomik_model::SocketRef {
                    device: b.clone(),
                    socket: sock.into(),
                },
            }))
            .unwrap();
        }
    }
    arm(&mut core, &a, true);
    arm(&mut core, &b, true);
    core.transport_action(TransportAction::ToggleRecord);
    pump(&mut core, 4_800, 480);
    let f_start = start_frame(&core);
    pump(&mut core, 14_400, 480);
    core.transport_action(TransportAction::Stop);
    pump(&mut core, 2_400, 480);
    core.wait_background();
    let (ca, cb) = (clips(&core, &a), clips(&core, &b));
    assert_eq!((ca.len(), cb.len()), (1, 1));
    let comp = 64u64;
    let da = bundle_of(&core).load(asset_of(&ca[0]).0).unwrap();
    let db = bundle_of(&core).load(asset_of(&cb[0]).0).unwrap();
    assert_eq!((da.channels.len(), db.channels.len()), (2, 1));
    assert_eq!(
        da.channels[0][100].to_bits(),
        sample(0, f_start + comp + 100).to_bits()
    );
    assert_eq!(
        da.channels[1][100].to_bits(),
        sample(1, f_start + comp + 100).to_bits()
    );
    assert_eq!(
        db.channels[0][100].to_bits(),
        sample(3, f_start + comp + 100).to_bits()
    );
}

#[test]
fn an_armed_track_without_input_cables_records_nothing_and_says_so() {
    let mut core = core_with(2);
    let key = add_audio_track(&mut core, "Silent");
    for sock in ["in_l", "in_r"] {
        core.apply_command(Command::Disconnect {
            to: atomik_model::SocketRef {
                device: key.clone(),
                socket: sock.into(),
            },
        })
        .unwrap();
    }
    arm(&mut core, &key, true);
    core.transport_action(TransportAction::ToggleRecord);
    assert!(!core.is_recording(), "nothing to record");
    assert!(!core.is_playing(), "and the transport does not start");
    let msg = core.last_rejection().expect("a message").to_owned();
    assert!(msg.contains("Silent") || msg.contains("no input"), "{msg}");
    assert!(msg.contains("input"), "{msg}");

    // With a second, cabled track the take runs and the message still names the first.
    let other = add_audio_track(&mut core, "Cabled");
    arm(&mut core, &other, true);
    core.transport_action(TransportAction::ToggleRecord);
    assert!(core.is_recording());
    let msg = core.last_rejection().unwrap().to_owned();
    assert!(msg.contains("no input"), "{msg}");
    // one name for the device in every text a user can read.
    assert!(
        msg.contains("Audio I/O inputs") && !msg.contains("interface"),
        "{msg}"
    );
    pump(&mut core, 4_800, 480);
    core.transport_action(TransportAction::Stop);
    pump(&mut core, 2_400, 480);
    core.wait_background();
    assert!(clips(&core, &key).is_empty());
    assert_eq!(clips(&core, &other).len(), 1);
}

#[test]
fn recording_without_a_running_engine_is_refused_with_a_message() {
    let mut doc = empty_doc();
    doc.input_channels = 2;
    let mut core = AppCore::with_doc(RecordingSink::new(), doc, None);
    let key = add_audio_track(&mut core, "A");
    arm(&mut core, &key, true);
    core.transport_action(TransportAction::ToggleRecord);
    assert!(!core.is_recording());
    assert!(core.last_rejection().unwrap().contains("audio"));
}

#[test]
fn an_overflow_is_reported_and_the_take_stays_aligned_with_silent_gaps() {
    let mut core = core_with(1);
    core.sink_mut().compensation = 64;
    core.sink_mut().tap_capacity = Some(256); // four batches: a burst overflows
    let key = add_audio_track(&mut core, "A");
    arm(&mut core, &key, true);
    core.transport_action(TransportAction::ToggleRecord);
    pump(&mut core, 4_800, 480);
    let f_start = start_frame(&core);
    // A long render with nobody pumping the UI: the writer cannot keep up with a 4-batch ring.
    core.sink_mut().render(64_000);
    core.frame();
    core.transport_action(TransportAction::Stop);
    pump(&mut core, 2_400, 480);
    core.wait_background();
    let msg = core
        .last_rejection()
        .expect("the overflow is reported")
        .to_owned();
    assert!(msg.contains("dropped"), "{msg}");
    let list = clips(&core, &key);
    assert_eq!(list.len(), 1, "the take still lands");
    let data = bundle_of(&core).load(asset_of(&list[0]).0).unwrap();
    // Every frame is either aligned or silent. Whether the dropped frames fall inside the take
    // depends on how fast the writer thread drains (under load the overflow can happen after
    // the stop only), so their presence is asserted deterministically at the writer level
    // instead (`audio_writer::tests::an_overflow_is_counted_and_the_gap_is_written_as_silence`).
    for (j, s) in data.channels[0].iter().enumerate() {
        let want = sample(0, f_start + 64 + j as u64);
        if s.to_bits() != want.to_bits() {
            assert_eq!(s.to_bits(), 0, "frame {j} is either aligned or silent");
        }
    }
}

#[test]
fn the_in_progress_file_is_in_the_bundle_while_recording_and_gone_after() {
    let mut core = core_with(2);
    let key = add_audio_track(&mut core, "A");
    arm(&mut core, &key, true);
    core.transport_action(TransportAction::ToggleRecord);
    pump(&mut core, 9_600, 480);
    std::thread::sleep(std::time::Duration::from_millis(50));
    let audio = core.bundle_root().unwrap().join("Audio");
    let partials = |dir: &Path| -> Vec<PathBuf> {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with("rec-"))
            })
            .collect()
    };
    assert_eq!(
        partials(&audio).len(),
        1,
        "written into the bundle as it goes"
    );
    core.transport_action(TransportAction::Stop);
    pump(&mut core, 2_400, 480);
    core.wait_background();
    assert!(
        partials(&audio).is_empty(),
        "removed once the laps are stored"
    );
}

#[test]
fn restarting_the_engine_cancels_a_running_take_without_a_clip() {
    let mut core = core_with(2);
    let key = add_audio_track(&mut core, "A");
    arm(&mut core, &key, true);
    core.transport_action(TransportAction::ToggleRecord);
    pump(&mut core, 4_800, 480);
    core.rebuild_engine();
    assert!(!core.is_recording());
    core.wait_background();
    assert!(clips(&core, &key).is_empty());
    let audio = core.bundle_root().unwrap().join("Audio");
    // `wait_background` joined the cancelled writer, which deleted its partial file.
    let left = std::fs::read_dir(&audio)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with("rec-"))
        .count();
    assert_eq!(left, 0);
}

// ---- host inputs ---------------------------------------------------------------------------

#[test]
fn changing_the_input_count_restarts_the_engine_only_when_it_differs() {
    let mut core = core_with(2);
    assert_eq!(core.sink().resets, vec![2]);
    core.apply_command(Command::SetInputChannels(4)).unwrap();
    assert_eq!(
        core.sink().resets,
        vec![2, 4],
        "the stream restarts with the new count"
    );
    // A routing edit does not restart anything.
    let key = add_audio_track(&mut core, "A");
    core.apply_command(Command::Disconnect {
        to: atomik_model::SocketRef {
            device: key,
            socket: "in_r".into(),
        },
    })
    .unwrap();
    assert_eq!(core.sink().resets, vec![2, 4]);
    assert!(core.undo());
    assert!(core.undo());
    assert!(core.undo());
    assert_eq!(
        *core.sink().resets.last().unwrap(),
        2,
        "undo restarts with the old count"
    );
}

// ---- monitoring ----------------------------------------------------------------------------

fn monitor_params(core: &mut AppCore<RecordingSink>) -> Vec<(DeviceInstanceId, f32)> {
    let pid = core
        .registry()
        .get(AUDIO_TRACK_TYPE)
        .unwrap()
        .descriptor
        .param_by_key("internal.monitor_live")
        .unwrap()
        .id;
    core.sink_mut()
        .take()
        .into_iter()
        .filter_map(|c| match c {
            SinkCall::Param { id, param, plain } if param == pid => Some((id, plain)),
            _ => None,
        })
        .collect()
}

#[test]
fn monitor_live_follows_the_arm_flag_without_touching_the_document() {
    let mut core = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    let key = add_audio_track(&mut core, "A");
    let id = core.device_id(&key).unwrap();
    // On device add: not armed, so 0.
    assert_eq!(monitor_params(&mut core), vec![(id, 0.0)]);
    let dirty = core.is_dirty();
    arm(&mut core, &key, true);
    assert_eq!(monitor_params(&mut core), vec![(id, 1.0)]);
    arm(&mut core, &key, false);
    assert_eq!(monitor_params(&mut core), vec![(id, 0.0)]);
    // Unrelated edits send nothing.
    core.apply_command(Command::Song(SongCommand::SetSongEnd(PPQ * 64)))
        .unwrap();
    assert!(monitor_params(&mut core).is_empty());
    assert!(dirty || core.is_dirty());
    // The param is not stored.
    let device = core.doc().device(&key).unwrap();
    assert!(!device.params.contains_key("internal.monitor_live"));
}

#[test]
fn monitor_live_is_sent_again_after_a_load_an_engine_restart_and_a_re_add() {
    let mut core = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    let key = add_audio_track(&mut core, "A");
    arm(&mut core, &key, true);
    monitor_params(&mut core);
    // Engine restart (audio settings change): the fresh engine is told again.
    core.rebuild_engine();
    let id = core.device_id(&key).unwrap();
    assert_eq!(monitor_params(&mut core), vec![(id, 1.0)]);
    // Load: a project with an armed audio track starts with live monitoring.
    let text = atomik_model::save_project(core.doc());
    core.new_document();
    monitor_params(&mut core);
    core.open_str(&text, None).unwrap();
    let id = core.device_id(&key).unwrap();
    assert_eq!(monitor_params(&mut core), vec![(id, 1.0)]);
    // Removing the track and undoing the removal re-adds the device under a new engine id.
    core.apply_command(Command::RemoveDevice { key: key.clone() })
        .unwrap();
    monitor_params(&mut core);
    assert!(core.undo());
    let id = core.device_id(&key).unwrap();
    assert!(monitor_params(&mut core).contains(&(id, 1.0)));
}

// ---- streaming -----------------------------------------------------------------------------

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("atomik-rec-audio-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_ramp(dir: &Path, name: &str, frames: usize) -> PathBuf {
    let data: Vec<f32> = (0..frames)
        .map(|i| ((i % 977) as f32 / 977.0) - 0.5)
        .collect();
    let path = dir.join(name);
    write_wav_f32(
        &path,
        &AudioData::new(48_000, vec![data.clone(), data]).unwrap(),
    )
    .unwrap();
    path
}

#[test]
fn imports_stream_above_the_threshold_and_load_into_ram_below_it() {
    let dir = tmp("threshold");
    let small = write_ramp(&dir, "small.wav", 1_000);
    let big = write_ramp(&dir, "big.wav", 4_000);
    // 1 000 stereo frames are 8 000 bytes, 4 000 frames 32 000.
    let mut sink = RecordingSink::new();
    sink.stream_threshold = Some(16_000);
    let mut core = AppCore::with_doc(sink, empty_doc(), None);
    core.import_audio(vec![small, big], atomik_ui::ImportTarget::NewTracks, 0);
    core.wait_background();
    let assets = core.song_assets();
    assert_eq!(assets.len(), 2);
    let streamed: Vec<bool> = {
        let mut v: Vec<_> = assets
            .values()
            .map(|a| (a.frames(), a.is_stream()))
            .collect();
        v.sort_unstable();
        v.into_iter().map(|(_, s)| s).collect()
    };
    assert_eq!(streamed, vec![false, true], "small in RAM, big streamed");
    // The default threshold is 64 MiB: nothing here streams.
    let mut core = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    assert_eq!(
        core.stream_threshold(),
        crate::stream_source::STREAM_THRESHOLD_BYTES
    );
    core.import_audio(
        vec![write_ramp(&dir, "again.wav", 4_000)],
        atomik_ui::ImportTarget::NewTracks,
        0,
    );
    core.wait_background();
    assert!(core.song_assets().values().all(|a| !a.is_stream()));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_streamed_project_opens_streamed_and_plays_like_the_in_memory_one() {
    let dir = tmp("play");
    let wav = write_ramp(&dir, "tone.wav", 48_000);
    let mut outputs = Vec::new();
    for threshold in [None, Some(0u64)] {
        let mut sink = OfflineSink::new();
        sink.threshold = threshold;
        let mut core = AppCore::with_doc(sink, empty_doc(), None);
        core.import_audio(vec![wav.clone()], atomik_ui::ImportTarget::NewTracks, 0);
        core.wait_background();
        assert_eq!(
            core.song_assets().values().all(|a| a.is_stream()),
            threshold.is_some()
        );
        core.sink_mut().render(64 * 8);
        core.frame();
        core.transport_action(TransportAction::Play);
        let mut captured = Vec::new();
        for _ in 0..40 {
            captured.push(core.sink_mut().render(960));
            core.frame();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(
            core.sink().stream_underruns(),
            0,
            "the streamer kept up (threshold {threshold:?})"
        );
        outputs.push(captured);
    }
    let flat = |v: &Vec<Vec<Vec<f32>>>| -> Vec<u32> {
        v.iter()
            .flat_map(|blk| blk[0].iter().map(|s| s.to_bits()))
            .collect()
    };
    // Play may start a few frames later when it waited for the read-ahead: compare from the
    // first audible sample.
    let audible = |v: Vec<u32>| -> Vec<u32> {
        let at = v.iter().position(|b| *b != 0).expect("it is audible");
        v[at..].to_vec()
    };
    let (mem, stream) = (audible(flat(&outputs[0])), audible(flat(&outputs[1])));
    let n = mem.len().min(stream.len()).min(20_000);
    assert!(n > 10_000);
    assert_eq!(
        mem[..n],
        stream[..n],
        "streaming is bit-identical to RAM playback"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn play_waits_for_the_streamer_to_prime_but_never_forever() {
    let mut sink = RecordingSink::new();
    sink.unprimed = true;
    let mut core = AppCore::with_doc(sink, empty_doc(), None);
    core.sink_mut().take();
    core.transport_action(TransportAction::Play);
    let plays = |c: &mut AppCore<RecordingSink>| {
        c.sink_mut()
            .calls
            .iter()
            .filter(|c| matches!(c, SinkCall::Transport(TransportCommand::Play)))
            .count()
    };
    assert_eq!(plays(&mut core), 0, "waiting for the read-ahead");
    assert!(core.is_playing());
    core.frame();
    assert_eq!(plays(&mut core), 0);
    core.sink_mut().unprimed = false;
    core.frame();
    assert_eq!(plays(&mut core), 1, "primed: Play goes out");
    core.frame();
    assert_eq!(plays(&mut core), 1, "once");
    // Never primed: it plays anyway after a short wait.
    let mut sink = RecordingSink::new();
    sink.unprimed = true;
    let mut core = AppCore::with_doc(sink, empty_doc(), None);
    core.sink_mut().take();
    core.transport_action(TransportAction::Play);
    for _ in 0..300 {
        core.frame();
    }
    assert_eq!(plays(&mut core), 1);
    // Stop while waiting cancels the pending play.
    let mut sink = RecordingSink::new();
    sink.unprimed = true;
    let mut core = AppCore::with_doc(sink, empty_doc(), None);
    core.sink_mut().take();
    core.transport_action(TransportAction::Play);
    core.transport_action(TransportAction::Stop);
    core.sink_mut().unprimed = false;
    core.frame();
    assert_eq!(plays(&mut core), 0);
    assert!(!core.is_playing());
}

struct SlowDisk;

impl crate::stream_source::FrameReader for SlowDisk {
    fn read_frames(&mut self, start: u64, out: &mut [Vec<f32>]) -> Result<usize, String> {
        std::thread::sleep(std::time::Duration::from_millis(400));
        for ch in out.iter_mut() {
            for (i, s) in ch.iter_mut().enumerate() {
                *s = 0.25 + (start + i as u64) as f32 * 1e-9;
            }
        }
        Ok(out[0].len())
    }
}

#[test]
fn a_slow_disk_costs_counted_underruns_but_never_blocks_the_audio_thread() {
    let dir = tmp("slow");
    let wav = write_ramp(&dir, "slow.wav", 96_000);
    let mut core = AppCore::with_doc(OfflineSink::new(), empty_doc(), None);
    core.import_audio(vec![wav], atomik_ui::ImportTarget::NewTracks, 0);
    core.wait_background();
    // Swap the engine's copy for the same asset behind a disk far too slow to keep up.
    let hash = core.song_assets().keys().next().unwrap().clone();
    let slow = SongAsset::stream(
        96_000,
        2,
        Arc::new(crate::stream_source::ReaderSource::new(SlowDisk)),
    );
    core.audio.assets.insert(hash, slow);
    core.submit_song(false);
    core.sink_mut().render(64 * 8);
    core.frame();
    core.transport_action(TransportAction::Play);
    let mut worst = std::time::Duration::ZERO;
    // Play waits up to 120 UI frames for a read-ahead that never comes, then goes.
    for _ in 0..200 {
        let t0 = test_now();
        core.sink_mut().render(960);
        worst = worst.max(t0.elapsed());
        core.frame();
    }
    assert!(
        core.sink().stream_underruns() > 0,
        "the starved reads are counted"
    );
    // Well under one 400 ms read, with room for a slow or instrumented build.
    assert!(
        worst < std::time::Duration::from_millis(200),
        "a render call never waits for the disk: {worst:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[allow(clippy::disallowed_methods)] // Justification: test-only timing of blocking behaviour.
fn test_now() -> std::time::Instant {
    std::time::Instant::now()
}

// ---------------------------------------------------------------- input gain and clip-safe

fn set_input_param(core: &mut AppCore<impl EngineSink>, key: &DeviceKey, param: &str, v: f32) {
    core.apply_command(Command::SetParam {
        key: key.clone(),
        param: param.to_owned(),
        value: v,
        merge: None,
    })
    .unwrap();
}

/// Records `frames` frames of the mono input `input` on the armed tracks of `core` and returns
/// each track's clip data in song order.
fn record_mono(
    core: &mut AppCore<OfflineSink>,
    keys: &[DeviceKey],
    frames: usize,
    mut during: impl FnMut(&mut AppCore<OfflineSink>, usize),
) -> Vec<AudioData> {
    for k in keys {
        arm(core, k, true);
    }
    core.transport_action(TransportAction::ToggleRecord);
    pump(core, 4_800, 480);
    let mut done = 0;
    while done < frames {
        during(core, done);
        pump(core, 480, 480);
        done += 480;
    }
    core.transport_action(TransportAction::Stop);
    pump(core, 4_800, 480);
    core.wait_background();
    keys.iter()
        .map(|k| {
            let list = clips(core, k);
            let (hash, _) = asset_of(&list[0]);
            bundle_of(core).load(hash).unwrap()
        })
        .collect()
}

fn mono_core(input: impl Fn(usize, u64) -> f32 + 'static) -> AppCore<OfflineSink> {
    let mut doc = empty_doc();
    doc.input_channels = 1;
    let mut sink = OfflineSink::new();
    sink.input = Box::new(input);
    AppCore::with_doc(sink, doc, None)
}

#[test]
fn the_take_holds_the_input_gain_exactly() {
    let mut core = mono_core(|_, _| 0.25);
    let key = add_audio_track(&mut core, "Mic");
    set_input_param(&mut core, &key, "rec_gain", 6.0);
    let take = record_mono(&mut core, &[key], 24_000, |_, _| {}).remove(0);
    assert!(take.frames() > 24_000);
    let want = (0.25f32 * atomik_core::db_to_gain(6.0)).to_bits();
    assert!(
        take.channels[0].iter().all(|s| s.to_bits() == want),
        "every sample is the input times the gain, from the first frame"
    );
}

#[test]
fn a_gain_turned_during_the_take_glides_and_is_in_the_file_from_the_next_batch() {
    let mut core = mono_core(|_, _| 0.1);
    let key = add_audio_track(&mut core, "Mic");
    let k = key.clone();
    let take = record_mono(&mut core, &[key], 48_000, |c, done| {
        if done == 24_000 {
            set_input_param(c, &k, "rec_gain", 12.0);
        }
    })
    .remove(0);
    let y = &take.channels[0];
    let to = 0.1 * atomik_core::db_to_gain(12.0);
    assert!(y[..16_000].iter().all(|s| s.to_bits() == 0.1f32.to_bits()));
    assert!(y[y.len() - 8_000..].iter().all(|s| (s - to).abs() < 1.0e-4));
    // Monotone and no jump bigger than a hundredth of the move between two frames.
    let step = (to - 0.1) / 100.0;
    for w in y.windows(2) {
        assert!(
            w[1] >= w[0] - 1.0e-7 && w[1] - w[0] < step,
            "{} -> {}",
            w[0],
            w[1]
        );
    }
}

#[test]
fn clip_safe_takes_never_pass_full_scale_and_the_plain_take_keeps_its_overs() {
    let hot = |_: usize, f: u64| 0.9 * (f as f32 * 0.03).sin();
    let mut core = mono_core(hot);
    let safe = add_audio_track(&mut core, "Safe");
    let plain = add_audio_track(&mut core, "Plain");
    for k in [&safe, &plain] {
        set_input_param(&mut core, k, "rec_gain", 12.0);
    }
    set_input_param(&mut core, &safe, "rec_clip_safe", 1.0);
    let takes = record_mono(&mut core, &[safe, plain], 24_000, |_, _| {});
    let peak = |d: &AudioData| d.channels[0].iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(peak(&takes[0]) <= 1.0, "clip-safe: {}", peak(&takes[0]));
    assert!(peak(&takes[0]) > 0.95);
    assert!(
        peak(&takes[1]) > 3.0,
        "the same input at +12 dB with clip-safe off keeps its overs: {}",
        peak(&takes[1])
    );
}

#[test]
fn two_tracks_on_the_same_input_each_keep_their_own_gain() {
    let mut core = mono_core(|_, _| 0.2);
    let a = add_audio_track(&mut core, "A");
    let b = add_audio_track(&mut core, "B");
    set_input_param(&mut core, &b, "rec_gain", -6.0);
    let takes = record_mono(&mut core, &[a, b], 12_000, |_, _| {});
    assert!(
        takes[0].channels[0]
            .iter()
            .all(|s| s.to_bits() == 0.2f32.to_bits())
    );
    let want = (0.2f32 * atomik_core::db_to_gain(-6.0)).to_bits();
    assert!(takes[1].channels[0].iter().all(|s| s.to_bits() == want));
}

// ---------------------------------------------------------------- the input meter

#[test]
fn the_transport_bar_shows_the_post_gain_input_level_before_any_take() {
    let mut core = mono_core(|_, _| 0.5);
    let key = add_audio_track(&mut core, "Mic");
    set_input_param(&mut core, &key, "rec_gain", 6.0);
    // Not armed: no section, nothing metered.
    pump(&mut core, 4_800, 480);
    core.feed_rec_input(0.0);
    assert!(core.transport_bar().rec_input().is_none());
    arm(&mut core, &key, true);
    pump(&mut core, 4_800, 480);
    core.feed_rec_input(0.1);
    let bar = core.transport_bar();
    let shown = bar.rec_input().expect("the armed track");
    assert_eq!(
        (shown.device.clone(), shown.name.as_str()),
        (key.clone(), "Mic")
    );
    assert_eq!((shown.gain_db, shown.clip_safe), (6.0, false));
    // 0.5 at +6 dB is 0.9976: -0.02 dBFS, just under the lamp.
    let (level, hold) = bar.rec_level_db();
    assert!(
        (level + 0.02).abs() < 0.01 && (hold + 0.02).abs() < 0.01,
        "{level}"
    );
    assert!(!bar.rec_clip_latched());
    // Turn it up: the meter reads the new gain and the lamp latches, and it stays.
    set_input_param(&mut core, &key, "rec_gain", 8.0);
    pump(&mut core, 4_800, 480);
    core.feed_rec_input(0.2);
    assert!(core.transport_bar().rec_clip_latched());
    set_input_param(&mut core, &key, "rec_gain", -20.0);
    pump(&mut core, 4_800, 480);
    core.feed_rec_input(0.3);
    assert!(
        core.transport_bar().rec_clip_latched(),
        "until it is clicked"
    );
    // Disarmed: the section goes.
    arm(&mut core, &key, false);
    core.feed_rec_input(0.4);
    assert!(core.transport_bar().rec_input().is_none());
}

#[test]
fn the_input_shown_is_the_selected_armed_track_else_the_first_armed_one() {
    let mut core = mono_core(|_, _| 0.1);
    let a = add_audio_track(&mut core, "A");
    let b = add_audio_track(&mut core, "B");
    let c = add_audio_track(&mut core, "C");
    arm(&mut core, &b, true);
    arm(&mut core, &c, true);
    core.feed_rec_input(0.0);
    let shown =
        |core: &AppCore<OfflineSink>| core.transport_bar().rec_input().unwrap().device.clone();
    assert_eq!(shown(&core), b, "the first armed track");
    core.arrangement_mut().set_selected_track(Some(c.clone()));
    core.feed_rec_input(0.1);
    assert_eq!(shown(&core), c, "the selected armed track");
    core.arrangement_mut().set_selected_track(Some(a));
    core.feed_rec_input(0.2);
    assert_eq!(shown(&core), b, "an unarmed selection does not count");
}

#[test]
fn a_track_without_input_cables_reads_silence() {
    let mut core = mono_core(|_, _| 0.9);
    let key = add_audio_track(&mut core, "Mic");
    for socket in ["in_l", "in_r"] {
        core.apply_command(Command::Disconnect {
            to: atomik_model::SocketRef {
                device: key.clone(),
                socket: socket.to_owned(),
            },
        })
        .unwrap();
    }
    arm(&mut core, &key, true);
    pump(&mut core, 4_800, 480);
    core.feed_rec_input(0.0);
    assert_eq!(core.transport_bar().rec_level_db().0, -60.0);
}
