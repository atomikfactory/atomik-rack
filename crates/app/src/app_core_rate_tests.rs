//! The project rate and channel counts in the app core: a new project at a rate, the conversion
//! of an open project with its audio, the tick across an engine rebuild and the channel counts.

use super::*;
use crate::sink::{RecordingSink, SinkCall};
use atomik_audio_file::{AudioData, write_wav_f32};
use atomik_core::PPQ;
use atomik_engine::TransportCommand;
use atomik_model::{ClipContent, empty_doc};
use atomik_ui::ImportTarget;

fn core() -> AppCore<RecordingSink> {
    let mut c = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    c.sink_mut().take();
    c
}

fn tone_file(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("atomik-app-rate-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let data: Vec<f32> = (0..48_000).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
    let path = dir.join("tone.wav");
    write_wav_f32(&path, &AudioData::new(48_000, vec![data]).unwrap()).unwrap();
    path
}

fn clip_of(c: &AppCore<RecordingSink>) -> (i64, i64, String, u64) {
    for t in &c.doc().song.tracks {
        for clip in t.lanes.iter().flat_map(|l| &l.clips) {
            if let ClipContent::Audio(a) = &clip.content {
                return (clip.start, clip.length, a.asset.clone(), a.source_offset);
            }
        }
    }
    panic!("no audio clip");
}

#[test]
fn a_new_project_takes_the_chosen_rate() {
    for rate in [44_100, 48_000, 96_000] {
        let mut c = core();
        c.new_document_at(rate);
        assert_eq!(c.doc().sample_rate, rate);
        let reset = c
            .sink_mut()
            .take()
            .into_iter()
            .rev()
            .find_map(|call| match call {
                SinkCall::Reset { sample_rate, .. } => Some(sample_rate),
                _ => None,
            })
            .expect("the engine was reset");
        assert_eq!(reset, f64::from(rate));
    }
    // An unsupported rate leaves the default.
    let mut c = core();
    c.new_document_at(22_050);
    assert_eq!(c.doc().sample_rate, 48_000);
}

#[test]
fn a_rate_change_keeps_the_tick_and_keeps_playing() {
    let mut c = core();
    c.locate(5 * PPQ);
    c.play();
    c.sink_mut().take();
    c.convert_sample_rate(44_100);
    assert!(c.last_rejection().is_none(), "{:?}", c.last_rejection());
    assert_eq!(c.doc().sample_rate, 44_100);
    let calls = c.sink_mut().take();
    let reset = calls
        .iter()
        .position(|k| matches!(k, SinkCall::Reset { sample_rate, .. } if (*sample_rate - 44_100.0).abs() < 0.5))
        .expect("the engine restarted at the new rate");
    let after = &calls[reset..];
    let locate = after
        .iter()
        .position(
            |k| matches!(k, SinkCall::Transport(TransportCommand::Locate(t)) if t.0 == 5 * PPQ),
        )
        .expect("the transport was put back at its tick");
    let play = after
        .iter()
        .position(|k| matches!(k, SinkCall::Transport(TransportCommand::Play)))
        .expect("the transport kept playing");
    assert!(locate < play);
    // Undo goes back to the old rate, again at the tick.
    c.sink_mut().take();
    assert!(c.undo());
    assert_eq!(c.doc().sample_rate, 48_000);
    assert!(
        c.sink_mut()
            .take()
            .iter()
            .any(|k| matches!(k, SinkCall::Reset { sample_rate, .. } if (*sample_rate - 48_000.0).abs() < 0.5))
    );
}

#[test]
fn converting_a_project_with_audio_is_one_undo_step() {
    let mut c = core();
    c.import_audio(vec![tone_file("convert")], ImportTarget::NewTracks, 4 * PPQ);
    c.wait_background();
    let before = clip_of(&c);
    let steps = c.doc().song.tracks.len();
    c.convert_sample_rate(44_100);
    c.wait_background();
    assert!(c.last_rejection().is_none(), "{:?}", c.last_rejection());
    assert_eq!(c.doc().sample_rate, 44_100);
    let after = clip_of(&c);
    // The clip stays where it is in ticks; the asset is the 44.1 kHz copy.
    assert_eq!((after.0, after.1), (before.0, before.1));
    assert_ne!(after.2, before.2);
    let meta = &c.doc().assets[&after.2];
    assert_eq!((meta.sample_rate, meta.frames), (44_100, 44_100));
    assert!(
        c.audio.assets.contains_key(&after.2),
        "the engine has the copy"
    );
    assert_eq!(c.doc().song.tracks.len(), steps);
    // One undo restores the rate and the original asset.
    assert!(c.undo());
    assert_eq!(c.doc().sample_rate, 48_000);
    assert_eq!(clip_of(&c), before);
    assert!(c.redo());
    assert_eq!(c.doc().sample_rate, 44_100);
    assert_eq!(clip_of(&c).2, after.2);
}

#[test]
fn a_bad_conversion_is_reported_and_changes_nothing() {
    let mut c = core();
    c.convert_sample_rate(22_050);
    assert!(c.last_rejection().is_some());
    c.convert_sample_rate(48_000);
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.contains("already runs"))
    );
    assert_eq!(c.doc().sample_rate, 48_000);
}

#[test]
fn channel_counts_are_one_undo_step_each_and_restart_the_engine() {
    let mut c = core();
    c.sink_mut().take();
    c.set_output_channels(8);
    assert_eq!(c.doc().output_channels, 8);
    assert!(c.sink_mut().take().iter().any(|k| matches!(
        k,
        SinkCall::Reset {
            output_channels: 8,
            ..
        }
    )));
    c.set_input_channels(4);
    assert_eq!(c.doc().input_channels, 4);
    assert!(c.undo());
    assert_eq!(c.doc().input_channels, 2);
    assert!(c.undo());
    assert_eq!(c.doc().output_channels, 2);
    c.set_output_channels(0);
    assert!(c.last_rejection().is_some());
}
