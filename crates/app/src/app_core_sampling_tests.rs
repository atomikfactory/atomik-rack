//! Sampling into devices through the core: a real offline engine is rendered by hand with
//! scripted input, so the record tap, the 30 s ring and the bundle are the real ones.

use super::*;
use crate::offline_sink::OfflineSink;
use crate::sink::RecordingSink;
use atomik_audio_file::Bundle;
use atomik_model::{ZoneDoc, empty_doc, pad_key};
use atomik_ui::TransportAction;

const SAMPLER: &str = "atomik.sampler";
/// Frames of silence the scripted input plays before the signal.
const LEAD: u64 = 1_000;

pub(super) fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

/// The scripted signal of channel `c` at host frame `f`: silent for `LEAD` frames after
/// `from`, then a repeating exactly representable pattern (the right channel inverted).
fn signal(from: u64) -> impl Fn(usize, u64) -> f32 {
    move |c, f| {
        if f < from + LEAD {
            return 0.0;
        }
        let v = ((f % 251) as f32 + 1.0) / 256.0;
        if c == 0 { v } else { -v }
    }
}

pub(super) fn core_with(inputs: u16, type_id: &str, from: u64) -> AppCore<OfflineSink> {
    let mut doc = empty_doc();
    doc.input_channels = inputs;
    let mut sink = OfflineSink::new();
    sink.input = Box::new(signal(from));
    let mut core = AppCore::with_doc(sink, doc, None);
    core.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: type_id.to_owned(),
        key: Some(key("dev")),
    })
    .unwrap();
    core
}

pub(super) fn pump(core: &mut AppCore<OfflineSink>, frames: usize, block: usize) {
    let mut done = 0;
    while done < frames {
        let n = block.min(frames - done);
        core.sink_mut().render(n);
        core.frame();
        done += n;
    }
}

pub(super) fn sample_into(core: &mut AppCore<OfflineSink>, at_key: Option<u8>) {
    core.handle_action(UiAction::SampleInto {
        device: key("dev"),
        at_key,
    });
}

pub(super) fn zones(core: &AppCore<impl EngineSink>) -> Vec<ZoneDoc> {
    core.doc().device(&key("dev")).unwrap().zones.clone()
}

fn bits(v: &[f32]) -> Vec<u32> {
    v.iter().map(|s| s.to_bits()).collect()
}

/// A core whose engine has rendered 512 frames, sampling from the start of the next batch.
pub(super) fn ready(inputs: u16, type_id: &str) -> AppCore<OfflineSink> {
    let mut core = core_with(inputs, type_id, 512);
    pump(&mut core, 512, 64);
    core
}

#[test]
fn a_known_signal_lands_as_one_zone_minus_the_leading_silence() {
    let mut core = ready(2, SAMPLER);
    let depth = core.document().undo_depth();
    sample_into(&mut core, None);
    assert_eq!(core.sampling_device(), Some(&key("dev")));
    pump(&mut core, 4_096, 64);
    assert!(core.sampling_seconds() > 0.05);
    sample_into(&mut core, None);
    assert!(core.sampling_device().is_none());
    assert!(
        core.last_rejection().is_none(),
        "{:?}",
        core.last_rejection()
    );

    // The tap covered engine frames 512..4608; the signal starts LEAD frames in.
    let z = zones(&core);
    assert_eq!(z.len(), 1);
    let frames = 4_096 - LEAD;
    assert_eq!((z[0].start, z[0].end), (0, frames));
    assert_eq!(z[0].root_key, 60, "the asset name does not pick a root key");
    assert_eq!(core.document().undo_depth(), depth + 1, "one undo step");
    let bundle = Bundle::open(core.bundle_root().expect("a bundle")).unwrap();
    let data = bundle.load(&z[0].asset).unwrap();
    assert_eq!(data.channels.len(), 2);
    for (c, ch) in data.channels.iter().enumerate() {
        let expect: Vec<u32> = (0..frames)
            .map(|j| signal(512)(c, 512 + LEAD + j).to_bits())
            .collect();
        assert_eq!(
            bits(ch),
            expect,
            "channel {c} equals the signal without its lead"
        );
    }
    let meta = &core.doc().assets[&z[0].asset];
    assert_eq!((meta.frames, meta.channels), (frames, 2));
    assert_eq!(meta.original_name, "Sample 1");

    // Undo removes the zone and keeps the asset for redo.
    assert!(core.undo());
    assert!(zones(&core).is_empty());
    assert!(core.doc().assets.contains_key(&z[0].asset));
    assert!(core.redo());
    assert_eq!(zones(&core)[0].asset, z[0].asset);
}

#[test]
fn cancel_and_a_silent_input_leave_the_document_untouched() {
    let mut core = ready(2, SAMPLER);
    let before = core.doc().clone();
    let depth = core.document().undo_depth();
    sample_into(&mut core, None);
    pump(&mut core, 2_048, 64);
    core.cancel_sampling();
    assert!(core.sampling_device().is_none());
    assert_eq!(core.doc(), &before);
    assert_eq!(core.document().undo_depth(), depth);
    // Nothing is running: the next frames do nothing either.
    pump(&mut core, 256, 64);
    assert_eq!(core.doc(), &before);

    // An input that never rises above the threshold.
    let mut quiet = core_with(2, SAMPLER, u64::MAX / 4);
    pump(&mut quiet, 512, 64);
    let before = quiet.doc().clone();
    let quiet_depth = quiet.document().undo_depth();
    sample_into(&mut quiet, None);
    pump(&mut quiet, 2_048, 64);
    sample_into(&mut quiet, None);
    assert_eq!(quiet.doc(), &before);
    assert!(quiet.last_rejection().unwrap().contains("silent"));
    assert_eq!(quiet.document().undo_depth(), quiet_depth);
}

#[test]
fn the_ring_holds_thirty_seconds_and_stops_the_sampling_there() {
    let mut core = ready(2, SAMPLER);
    sample_into(&mut core, None);
    let rate = core.doc().sample_rate as usize;
    pump(&mut core, (SAMPLE_SECONDS + 2) * rate, 480);
    assert!(core.sampling_device().is_none(), "the limit ended it");
    let z = zones(&core);
    assert_eq!(z.len(), 1);
    let kept = SAMPLE_SECONDS * rate - LEAD as usize;
    assert!(
        (kept - 480..=kept).contains(&(z[0].end as usize)),
        "{} frames against {kept}",
        z[0].end
    );
    assert!(core.last_rejection().unwrap().contains("30 s"));
}

#[test]
fn the_input_pair_is_chosen_with_a_cycle_and_a_mono_input_makes_a_mono_sample() {
    let mut core = ready(4, SAMPLER);
    let first = core.sampling_input().unwrap();
    assert_eq!((first.first, first.mono), (0, false));
    assert_eq!(first.label(), "Inputs 1-2");
    core.handle_action(UiAction::CycleSamplingInput);
    assert_eq!(core.sampling_input().unwrap().label(), "Inputs 3-4");
    assert_eq!(core.last_rejection().unwrap(), "Sampling from Inputs 3-4");
    core.handle_action(UiAction::CycleSamplingInput);
    assert_eq!(core.sampling_input().unwrap().label(), "Input 1");
    assert_eq!(
        sampling_inputs(4).len(),
        2 + 4,
        "two pairs and four single channels"
    );
    sample_into(&mut core, None);
    pump(&mut core, 2_048, 64);
    sample_into(&mut core, None);
    let z = zones(&core);
    let data = Bundle::open(core.bundle_root().unwrap())
        .unwrap()
        .load(&z[0].asset)
        .unwrap();
    assert_eq!(data.channels.len(), 1);
    // The choice does not change while it runs.
    sample_into(&mut core, None);
    core.handle_action(UiAction::CycleSamplingInput);
    assert!(core.last_rejection().unwrap().contains("Stop sampling"));
    core.cancel_sampling();
    // An interface with one input offers it alone; none offers nothing.
    assert_eq!(sampling_inputs(1).len(), 1);
    assert!(sampling_inputs(0).is_empty());
}

#[test]
fn a_drum_machine_takes_the_sample_on_the_pad_of_the_key() {
    let mut core = ready(2, "atomik.drum-machine");
    sample_into(&mut core, Some(pad_key(3)));
    pump(&mut core, 2_048, 64);
    sample_into(&mut core, Some(pad_key(3)));
    let z = zones(&core);
    assert_eq!(z.len(), 1);
    assert_eq!((z[0].key_lo, z[0].key_hi), (pad_key(3), pad_key(3)));
}

#[test]
fn a_granular_sampler_takes_the_sample_as_its_one_sample() {
    let mut core = ready(2, "atomik.granular");
    for _ in 0..2 {
        sample_into(&mut core, None);
        pump(&mut core, 2_048, 64);
        sample_into(&mut core, None);
    }
    let z = zones(&core);
    assert_eq!(z.len(), 1, "the second sample replaced the first");
    assert_eq!(core.doc().assets.len(), 2);
    assert_eq!(core.doc().assets[&z[0].asset].original_name, "Sample 2");
}

#[test]
fn sampling_and_recording_exclude_each_other_and_refusals_say_why() {
    let mut core = ready(2, SAMPLER);
    core.apply_command(Command::Song(atomik_model::SongCommand::AddAudioTrack {
        name: None,
    }))
    .unwrap();
    let track = core.doc().song.tracks.last().unwrap().device.clone();
    core.apply_command(Command::Song(atomik_model::SongCommand::SetTrackFlags {
        device: track,
        muted: false,
        solo: false,
        armed: true,
    }))
    .unwrap();
    sample_into(&mut core, None);
    core.transport_action(TransportAction::ToggleRecord);
    assert!(!core.is_recording(), "the take did not start");
    assert!(core.last_rejection().unwrap().contains("sampling"));
    core.cancel_sampling();

    // Not a sampler.
    core.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: "atomik.poly-synth".into(),
        key: Some(key("syn")),
    })
    .unwrap();
    core.handle_action(UiAction::SampleInto {
        device: key("syn"),
        at_key: None,
    });
    assert!(core.sampling_device().is_none());
    assert!(
        core.last_rejection()
            .unwrap()
            .contains("does not take samples")
    );

    // No audio engine: the recording sink has no tap.
    let mut quiet = AppCore::with_doc(RecordingSink::new(), empty_doc(), None);
    quiet
        .apply_command(Command::AddDevice {
            index: usize::MAX,
            type_id: SAMPLER.into(),
            key: Some(key("dev")),
        })
        .unwrap();
    quiet.handle_action(UiAction::SampleInto {
        device: key("dev"),
        at_key: None,
    });
    assert!(quiet.sampling_device().is_none());
    assert!(
        quiet
            .last_rejection()
            .unwrap()
            .contains("running audio engine")
    );

    // An engine restart drops a running sampling.
    let mut core = ready(2, SAMPLER);
    sample_into(&mut core, None);
    core.rebuild_engine();
    assert!(core.sampling_device().is_none());
}
