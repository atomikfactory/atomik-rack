//! The recording source of an audio track (model half): the options the mixer offers, and the
//! cycle rule that refuses a strip the recording track feeds.

mod common;

use atomik_model::console::OutputDest;
use atomik_model::{
    Command, ConsoleCommand, Document, RecSource, source_label, source_options, source_refusal,
};
use common::audio::*;
use common::*;

fn two_tracks() -> (Document, atomik_device_sdk::Registry) {
    let r = audio_registry();
    let mut d = with_assets(&r);
    d.apply(add_audio_track(), &r).unwrap();
    d.apply(add_audio_track(), &r).unwrap();
    (d, r)
}

fn bus(d: &mut Document, r: &atomik_device_sdk::Registry) {
    d.apply(
        Command::Console(ConsoleCommand::CreateBus {
            name: "Drums".into(),
            dest: OutputDest::Master,
        }),
        r,
    )
    .unwrap();
}

#[test]
fn the_options_list_every_channel_then_every_bus_then_the_master() {
    let (mut d, r) = two_tracks();
    bus(&mut d, &r);
    let options = source_options(d.doc());
    let sources: Vec<RecSource> = options.iter().map(|(s, _)| *s).collect();
    assert_eq!(
        sources,
        [
            RecSource::Channel(1),
            RecSource::Channel(2),
            RecSource::Bus(1),
            RecSource::Master
        ]
    );
    assert!(options[0].1.starts_with("Channel: "), "{:?}", options[0].1);
    assert_eq!(options[2].1, "Bus: Drums");
    assert!(options[3].1.starts_with("Master: "));
    assert_eq!(source_label(RecSource::Bus(2)), "Bus 2");
}

#[test]
fn a_track_cannot_record_its_own_channel_or_what_it_feeds() {
    let (mut d, r) = two_tracks();
    bus(&mut d, &r);
    let (first, second) = (key("audio-track"), key("audio-track-2"));
    // Inputs are always fine.
    assert_eq!(source_refusal(d.doc(), &first, RecSource::Interface), None);
    // Its own channel, and the master both channels feed.
    assert!(source_refusal(d.doc(), &first, RecSource::Channel(1)).is_some());
    assert!(source_refusal(d.doc(), &first, RecSource::Master).is_some());
    // The other track's channel is fine.
    assert_eq!(source_refusal(d.doc(), &first, RecSource::Channel(2)), None);
    // The bus is empty: either track may record it.
    assert_eq!(source_refusal(d.doc(), &first, RecSource::Bus(1)), None);
    // Route the second track into the bus: it may no longer record the bus, the first still may.
    let ch2 = d.doc().console.channel_of(&second).unwrap().channel.clone();
    d.apply(
        Command::Console(ConsoleCommand::SetOutput {
            strip: ch2,
            dest: OutputDest::Bus(d.doc().console.buses[0].bus.clone()),
        }),
        &r,
    )
    .unwrap();
    assert!(source_refusal(d.doc(), &second, RecSource::Bus(1)).is_some());
    assert_eq!(source_refusal(d.doc(), &first, RecSource::Bus(1)), None);
    // A strip that does not exist is refused with a reason.
    let reason = source_refusal(d.doc(), &first, RecSource::Channel(9)).unwrap();
    assert!(reason.contains("does not exist"), "{reason}");
}

#[test]
fn sending_a_channel_to_an_audio_pair_frees_the_master_for_it() {
    let (mut d, r) = two_tracks();
    let first = key("audio-track");
    d.apply(Command::SetOutputChannels(4), &r).unwrap();
    let ch1 = d.doc().console.channel_of(&first).unwrap().channel.clone();
    assert!(source_refusal(d.doc(), &first, RecSource::Master).is_some());
    d.apply(
        Command::Console(ConsoleCommand::SetOutput {
            strip: ch1,
            dest: OutputDest::AudioIo(2),
        }),
        &r,
    )
    .unwrap();
    // The master is now fed by the other track only.
    assert_eq!(source_refusal(d.doc(), &first, RecSource::Master), None);
    assert!(source_refusal(d.doc(), &key("audio-track-2"), RecSource::Master).is_some());
}
