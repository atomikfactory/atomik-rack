//! Quick sidechain: `DuckFrom`, `GentlePump` and `RemoveDucking` make a track duck under another
//! in one undo step (a compressor at the end of the insert chain keyed from the source), refuse
//! loops and repeats with plain messages, and take it all out again.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::duck_from::{
    ATTACK_MS, DUCKER_TYPE, NAME_PREFIX, PUMP_NAME, PUMP_TYPE, RATIO, RELEASE_MS, THRESHOLD_DB,
    duck_sources, duckers_of, ducking_from, pump_of,
};
use atomik_model::{
    Command, ConsoleCommand, Document, OutputDest, RackDoc, WorkflowCommand, insert_chain,
};
use common::song::*;
use common::workflow::*;
use common::*;

fn reg() -> Registry {
    console_registry()
}

/// Tracks `kick`, `bass` and `pad`, each a synth with a track and a console channel.
fn project() -> Document {
    let r = reg();
    let mut d = Document::new(legacy_project_doc());
    for (i, k) in ["kick", "bass", "pad"].into_iter().enumerate() {
        d.apply(add(i, SYNTH, Some(k)), &r).unwrap();
    }
    for k in ["kick", "bass", "pad"] {
        d.apply(add_track(k), &r).unwrap();
    }
    d
}

fn duck(target: &str, source: &str, pre_fader: bool) -> Command {
    wf(WorkflowCommand::DuckFrom {
        target: key(target),
        source: key(source),
        pre_fader,
    })
}

fn channel_of(doc: &RackDoc, track: &str) -> String {
    doc.console
        .channel_of(&key(track))
        .unwrap()
        .channel
        .0
        .clone()
}

fn has(doc: &RackDoc, from: &str, to: &str) -> bool {
    doc.cables.contains(&cable(from, to))
}

fn param(doc: &RackDoc, device: &str, name: &str) -> f32 {
    let d = doc.device(&key(device)).unwrap();
    d.params.get(name).copied().unwrap_or_else(|| {
        reg()
            .get(&d.type_id)
            .unwrap()
            .descriptor
            .param_by_key(name)
            .unwrap()
            .default
    })
}

/// The one ducker of `target` as `(device, source)`.
fn the_ducker(doc: &RackDoc, target: &str) -> (String, Option<String>) {
    let all = duckers_of(doc, &reg(), &key(target));
    assert_eq!(all.len(), 1, "{all:?}");
    (
        all[0].device.0.clone(),
        all[0].source.as_ref().map(|s| s.0.clone()),
    )
}

#[test]
fn duck_from_is_one_undo_step_with_the_device_the_key_cable_and_the_settings() {
    let r = reg();
    let mut d = project();
    let before = d.doc().clone();
    one_step(&mut d, &r, duck("bass", "kick", false));
    let doc = d.doc();
    assert_eq!(
        doc.devices.len(),
        before.devices.len() + 1,
        "one compressor"
    );
    let (ducker, source) = the_ducker(doc, "bass");
    assert_eq!(source.as_deref(), Some("kick"));
    let device = doc.device(&key(&ducker)).unwrap();
    assert_eq!(device.type_id, DUCKER_TYPE);
    assert_eq!(
        device.name,
        format!("{NAME_PREFIX}{}", doc.device(&key("kick")).unwrap().name)
    );

    // The key is the kick channel's post-fader output; the ducker is the end of the bass's chain.
    let (kick, bass) = (channel_of(doc, "kick"), channel_of(doc, "bass"));
    for side in ["l", "r"] {
        assert!(has(
            doc,
            &format!("{kick}:out_{side}"),
            &format!("{ducker}:sc_in_{side}")
        ));
        assert!(has(
            doc,
            &format!("{bass}:insert_send_{side}"),
            &format!("{ducker}:in_{side}")
        ));
        assert!(has(
            doc,
            &format!("{ducker}:out_{side}"),
            &format!("{bass}:insert_return_{side}")
        ));
    }
    assert_eq!(insert_chain(doc, &r, &key(&bass)), [key(&ducker)]);

    assert_eq!(param(doc, &ducker, "sc_enable"), 1.0);
    assert_eq!(param(doc, &ducker, "ratio"), RATIO);
    assert_eq!(param(doc, &ducker, "attack"), ATTACK_MS);
    assert_eq!(param(doc, &ducker, "release"), RELEASE_MS);
    assert_eq!(param(doc, &ducker, "threshold"), THRESHOLD_DB);
    assert_eq!(param(doc, &ducker, "lookahead"), 0.0);
    assert!(doc.validate().is_ok());
}

#[test]
fn pre_fader_takes_the_key_before_the_fader() {
    let r = reg();
    let mut d = project();
    one_step(&mut d, &r, duck("bass", "kick", true));
    let doc = d.doc();
    let (ducker, source) = the_ducker(doc, "bass");
    assert_eq!(source.as_deref(), Some("kick"));
    let kick = channel_of(doc, "kick");
    for side in ["l", "r"] {
        assert!(has(
            doc,
            &format!("{kick}:parallel_out_{side}"),
            &format!("{ducker}:sc_in_{side}")
        ));
        assert!(!has(
            doc,
            &format!("{kick}:out_{side}"),
            &format!("{ducker}:sc_in_{side}")
        ));
    }
    assert!(duckers_of(doc, &r, &key("bass"))[0].pre_fader);
}

#[test]
fn a_second_source_adds_a_second_ducker_after_the_first() {
    let r = reg();
    let mut d = project();
    d.apply(duck("pad", "kick", false), &r).unwrap();
    one_step(&mut d, &r, duck("pad", "bass", false));
    let doc = d.doc();
    let all = duckers_of(doc, &r, &key("pad"));
    assert_eq!(
        all.iter()
            .map(|x| x.source.clone().unwrap().0)
            .collect::<Vec<_>>(),
        ["kick", "bass"]
    );
    assert!(doc.validate().is_ok());
}

#[test]
fn ducking_again_from_the_same_track_is_refused_and_the_source_list_marks_it() {
    let r = reg();
    let mut d = project();
    d.apply(duck("bass", "kick", false), &r).unwrap();
    let e = rejected(&mut d, &r, duck("bass", "kick", false));
    assert!(e.to_string().contains("Already ducking from"), "{e}");
    let found = ducking_from(d.doc(), &r, &key("bass"), &key("kick")).unwrap();
    assert_eq!(
        Some(found.device),
        duckers_of(d.doc(), &r, &key("bass"))
            .first()
            .map(|x| x.device.clone())
    );
    assert!(ducking_from(d.doc(), &r, &key("bass"), &key("pad")).is_none());

    let sources = duck_sources(d.doc(), &r, &key("bass"));
    assert_eq!(
        sources
            .iter()
            .map(|s| (s.device.0.as_str(), s.current))
            .collect::<Vec<_>>(),
        [("kick", true), ("pad", false)],
        "every other track, in song order, the one it ducks from marked"
    );
}

#[test]
fn a_track_cannot_duck_from_itself_or_from_what_is_not_a_track() {
    let r = reg();
    let mut d = project();
    let e = rejected(&mut d, &r, duck("bass", "bass", false));
    assert!(e.to_string().contains("itself"), "{e}");
    let e = rejected(&mut d, &r, duck("bass", "nothing", false));
    assert!(e.to_string().contains("not a track"), "{e}");
    let e = rejected(&mut d, &r, duck("nothing", "kick", false));
    assert!(e.to_string().contains("no track"), "{e}");
}

#[test]
fn ducking_from_a_bus_the_track_plays_into_is_refused_as_a_loop() {
    let r = reg();
    let mut d = project();
    run(
        &mut d,
        &r,
        [Command::Console(ConsoleCommand::CreateBus {
            name: "Drums".into(),
            dest: OutputDest::Master,
        })],
    );
    let bus = d.doc().console.buses[0].bus.clone();
    let bass = key(&channel_of(d.doc(), "bass"));
    run(
        &mut d,
        &r,
        [Command::Console(ConsoleCommand::SetOutput {
            strip: bass,
            dest: OutputDest::Bus(bus.clone()),
        })],
    );
    let e = rejected(&mut d, &r, duck("bass", &bus.0, false));
    let text = e.to_string();
    assert!(text.contains("loop"), "a plain message: {text}");
    assert!(!text.contains("->"), "no cable jargon: {text}");

    // The bus is fed by the bass, not by the kick: the kick can duck from it.
    one_step(&mut d, &r, duck("kick", &bus.0, false));
    assert_eq!(
        the_ducker(d.doc(), "kick").1.as_deref(),
        Some(bus.0.as_str())
    );
}

#[test]
fn a_full_insert_chain_is_refused() {
    let r = reg();
    let mut d = project();
    let max = atomik_model::INSERT_SLOTS;
    // One source per ducker is not available, so fill the chain with pumps and effects.
    let bass = key(&channel_of(d.doc(), "bass"));
    for _ in 0..max {
        let slot = insert_chain(d.doc(), &r, &bass).len();
        d.apply(
            Command::Console(ConsoleCommand::AddInsert {
                channel: bass.clone(),
                slot,
                effect: atomik_model::InsertEffect::New {
                    type_id: "atomik.stereo-delay".into(),
                },
            }),
            &r,
        )
        .unwrap();
    }
    let e = rejected(&mut d, &r, duck("bass", "kick", false));
    assert!(e.to_string().contains("full"), "{e}");
}

#[test]
fn gentle_pump_is_a_sidechain_free_quarter_note_pump_and_only_one() {
    let r = reg();
    let mut d = project();
    one_step(
        &mut d,
        &r,
        wf(WorkflowCommand::GentlePump { target: key("pad") }),
    );
    let doc = d.doc();
    let pump = pump_of(doc, &r, &key("pad")).unwrap();
    let device = doc.device(&pump.device).unwrap();
    assert_eq!(
        (device.type_id.as_str(), device.name.as_str()),
        (PUMP_TYPE, PUMP_NAME)
    );
    assert_eq!(param(doc, &pump.device.0, "mode"), 0.0);
    assert_eq!(param(doc, &pump.device.0, "rate"), 1.0, "quarter notes");
    assert!(
        !doc.cables
            .iter()
            .any(|c| c.to.device == pump.device && c.to.socket.starts_with("sc_in")),
        "no key"
    );
    assert!(doc.validate().is_ok());
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::GentlePump { target: key("pad") }),
    );
    assert!(e.to_string().contains("already has"), "{e}");
}

#[test]
fn remove_ducking_takes_the_devices_and_the_key_cables_out_in_one_step() {
    let r = reg();
    let mut d = project();
    let plain = text(&d);
    d.apply(duck("bass", "kick", false), &r).unwrap();
    d.apply(duck("bass", "pad", true), &r).unwrap();
    d.apply(
        wf(WorkflowCommand::GentlePump {
            target: key("bass"),
        }),
        &r,
    )
    .unwrap();
    assert_eq!(duckers_of(d.doc(), &r, &key("bass")).len(), 3);
    let with = text(&d);
    d.apply(
        wf(WorkflowCommand::RemoveDucking {
            target: key("bass"),
        }),
        &r,
    )
    .unwrap();
    assert!(duckers_of(d.doc(), &r, &key("bass")).is_empty());
    assert_eq!(
        text(&d),
        plain,
        "the song is as it was before the first duck"
    );
    assert!(d.doc().validate().is_ok());
    d.undo().unwrap();
    assert_eq!(text(&d), with, "one undo brings the three back");
    d.redo().unwrap();
    assert_eq!(text(&d), plain);
}

#[test]
fn remove_ducking_leaves_other_inserts_and_refuses_when_there_is_nothing() {
    let r = reg();
    let mut d = project();
    let bass = key(&channel_of(d.doc(), "bass"));
    d.apply(
        Command::Console(ConsoleCommand::AddInsert {
            channel: bass.clone(),
            slot: 0,
            effect: atomik_model::InsertEffect::New {
                type_id: "atomik.stereo-delay".into(),
            },
        }),
        &r,
    )
    .unwrap();
    let delay = insert_chain(d.doc(), &r, &bass)[0].clone();
    let e = rejected(
        &mut d,
        &r,
        wf(WorkflowCommand::RemoveDucking {
            target: key("bass"),
        }),
    );
    assert!(e.to_string().contains("no ducking"), "{e}");
    d.apply(duck("bass", "kick", false), &r).unwrap();
    assert_eq!(insert_chain(d.doc(), &r, &bass).len(), 2);
    d.apply(
        wf(WorkflowCommand::RemoveDucking {
            target: key("bass"),
        }),
        &r,
    )
    .unwrap();
    assert_eq!(
        insert_chain(d.doc(), &r, &bass),
        [delay],
        "the delay stays, in the loop"
    );
}

#[test]
fn a_ducker_follows_its_source_when_the_track_is_renamed() {
    let r = reg();
    let mut d = project();
    d.apply(duck("bass", "kick", false), &r).unwrap();
    d.apply(
        Command::Rename {
            key: key("kick"),
            name: "Big Kick".into(),
        },
        &r,
    )
    .unwrap();
    assert!(ducking_from(d.doc(), &r, &key("bass"), &key("kick")).is_some());
    let e = rejected(&mut d, &r, duck("bass", "kick", false));
    assert!(e.to_string().contains("Big Kick"), "{e}");
}

#[test]
fn removing_the_source_track_leaves_the_ducker_without_a_key() {
    let r = reg();
    let mut d = project();
    d.apply(duck("bass", "kick", false), &r).unwrap();
    d.apply(Command::RemoveDevice { key: key("kick") }, &r)
        .unwrap();
    assert!(d.doc().validate().is_ok());
    // The cable went with the channel: what is left is a compressor with no key, still listed so
    // the user can remove it.
    let all = duckers_of(d.doc(), &r, &key("bass"));
    assert_eq!(all.len(), 1);
    assert!(all[0].source.is_none() && !all[0].pump);
    d.apply(
        wf(WorkflowCommand::RemoveDucking {
            target: key("bass"),
        }),
        &r,
    )
    .unwrap();
    assert!(duckers_of(d.doc(), &r, &key("bass")).is_empty());
}
