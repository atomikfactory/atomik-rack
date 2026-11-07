//! `WorkflowCommand::AddReverbSend`: the reverb send wizard of a track, in one undo step, with
//! the devices and cables the rules describe, a decay suggested from the track's note spacing, and
//! rejections that change nothing.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::reverb_send::{
    DEFAULT_DECAY, MAX_DECAY, MAX_VOCAL_DECAY, MIN_DECAY, ReverbSendOptions, median_onset_gap,
    suggested_decay,
};
use atomik_model::{
    Command, CreateOptions, CreateSource, Document, ModelError, RackDoc, WorkflowCommand,
    new_project_doc,
};
use common::song::*;
use common::workflow::*;
use common::*;

const REVERB: &str = "atomik.space-reverb";
const EQ: &str = "atomik.eq";
const DE_ESSER: &str = "atomik.de-esser";

fn reg() -> Registry {
    console_registry()
}

fn send(track: &str, options: ReverbSendOptions) -> Command {
    wf(WorkflowCommand::AddReverbSend {
        track: key(track),
        options,
    })
}

/// A project with a synth track whose clip has one note every `gap` ticks, `count` times.
fn project(r: &Registry, gap: i64, count: usize) -> Document {
    let mut d = Document::new(new_project_doc());
    let notes: Vec<_> = (0..count)
        .map(|i| note(i as i64 * gap, gap / 2, 60, 0.8))
        .collect();
    let mut steps = vec![wf(WorkflowCommand::CreateInstrument {
        index: usize::MAX,
        source: CreateSource::Type(SYNTH.into()),
        key: Some(key("lead")),
        options: CreateOptions::default(),
    })];
    if count > 0 {
        steps.push(add_clip(
            "lead",
            0,
            notes_clip(0, gap * count as i64, notes),
        ));
    }
    run(&mut d, r, steps);
    d
}

fn of_type(doc: &RackDoc, type_id: &str) -> Vec<String> {
    doc.devices
        .iter()
        .filter(|d| d.type_id == type_id)
        .map(|d| d.key.0.clone())
        .collect()
}

fn the(doc: &RackDoc, type_id: &str) -> String {
    let all = of_type(doc, type_id);
    assert_eq!(all.len(), 1, "{type_id}: {all:?}");
    all[0].clone()
}

fn has(doc: &RackDoc, from: &str, to: &str) -> bool {
    doc.cables.contains(&cable(from, to))
}

fn param(doc: &RackDoc, device: &str, name: &str) -> f32 {
    let d = doc.device(&key(device)).unwrap();
    d.params.get(name).copied().unwrap_or_else(|| {
        let r = reg();
        r.get(&d.type_id)
            .unwrap()
            .descriptor
            .param_by_key(name)
            .unwrap()
            .default
    })
}

fn channel_of(doc: &RackDoc, source: &str) -> String {
    doc.console
        .channel_of(&key(source))
        .unwrap()
        .channel
        .0
        .clone()
}

#[test]
fn the_wizard_is_one_undo_step_with_the_expected_devices_and_cables() {
    let r = reg();
    let mut d = project(&r, PPQ, 8);
    let before = d.doc().clone();
    let devices = before.devices.len();
    one_step(&mut d, &r, send("lead", ReverbSendOptions::default()));
    let doc = d.doc();
    assert_eq!(doc.devices.len(), devices + 2, "an EQ and a reverb");
    let (eq, reverb) = (the(doc, EQ), the(doc, REVERB));
    assert!(
        of_type(doc, DE_ESSER).is_empty(),
        "no de-esser unless it is a vocal"
    );
    let channel = channel_of(doc, "lead");

    // The return: send 1 -> EQ -> reverb -> return 1.
    assert_eq!(doc.console.returns[0], Some(key(&eq)));
    for side in ["l", "r"] {
        let master = doc.console.master.as_ref().unwrap().0.clone();
        assert!(has(
            doc,
            &format!("{master}:fx_send1_{side}"),
            &format!("{eq}:in_{side}")
        ));
        assert!(has(
            doc,
            &format!("{eq}:out_{side}"),
            &format!("{reverb}:in_{side}")
        ));
        assert!(has(
            doc,
            &format!("{reverb}:out_{side}"),
            &format!("{master}:return1_{side}")
        ));
        assert!(
            !has(
                doc,
                &format!("{eq}:out_{side}"),
                &format!("{master}:return1_{side}")
            ),
            "the EQ no longer feeds the return directly"
        );
        // The key of the reverb comes from the dry track.
        assert!(has(
            doc,
            &format!("{channel}:out_{side}"),
            &format!("{reverb}:sc_in_{side}")
        ));
    }

    // The send: on, -12 dB, post-fader.
    assert_eq!(param(doc, &channel, "send1.on"), 1.0);
    assert_eq!(param(doc, &channel, "send1.level"), -12.0);
    assert_eq!(param(doc, &channel, "send1.pre"), 0.0);

    // The EQ: high-pass and low-pass on, 24 dB per octave.
    assert_eq!(param(doc, &eq, "band1.on"), 1.0);
    assert_eq!(param(doc, &eq, "band1.freq"), 300.0);
    assert_eq!(param(doc, &eq, "band6.on"), 1.0);
    assert_eq!(param(doc, &eq, "band6.freq"), 8_000.0);
    assert_eq!(param(doc, &eq, "lowcut.slope"), 1.0);
    assert_eq!(param(doc, &eq, "highcut.slope"), 1.0);

    // The reverb: all wet, the duck on, the decay from the track (quarter notes at 120 BPM).
    assert_eq!(param(doc, &reverb, "mix"), 1.0);
    assert_eq!(param(doc, &reverb, "duck"), 0.5);
    assert!((param(doc, &reverb, "decay") - 1.0).abs() < 1e-3);
    assert!(doc.validate().is_ok());
}

#[test]
fn a_vocal_gets_a_de_esser_first_and_the_duck_can_be_left_out() {
    let r = reg();
    let mut d = project(&r, PPQ, 8);
    let options = ReverbSendOptions {
        vocal: true,
        duck: false,
        pre_fader: true,
        send_db: -6.0,
        ..ReverbSendOptions::default()
    };
    one_step(&mut d, &r, send("lead", options));
    let doc = d.doc();
    let (de, eq, reverb) = (the(doc, DE_ESSER), the(doc, EQ), the(doc, REVERB));
    assert_eq!(doc.console.returns[0], Some(key(&de)));
    assert!(has(doc, &format!("{de}:out_l"), &format!("{eq}:in_l")));
    assert!(has(doc, &format!("{eq}:out_r"), &format!("{reverb}:in_r")));
    let channel = channel_of(doc, "lead");
    assert!(
        !doc.cables
            .iter()
            .any(|c| c.to.device.0 == reverb && c.to.socket.starts_with("sc_in")),
        "no key cable without the duck"
    );
    assert_eq!(param(doc, &reverb, "duck"), 0.0);
    assert_eq!(param(doc, &channel, "send1.pre"), 1.0);
    assert_eq!(param(doc, &channel, "send1.level"), -6.0);
}

#[test]
fn an_explicit_decay_wins_and_a_second_wizard_takes_the_next_slot() {
    let r = reg();
    let mut d = project(&r, PPQ, 8);
    let first = ReverbSendOptions {
        decay: Some(5.5),
        ..ReverbSendOptions::default()
    };
    d.apply(send("lead", first), &r).unwrap();
    let reverb = the(d.doc(), REVERB);
    assert_eq!(param(d.doc(), &reverb, "decay"), 5.5);

    one_step(&mut d, &r, send("lead", ReverbSendOptions::default()));
    let doc = d.doc();
    assert!(doc.console.returns[1].is_some(), "the second return");
    let channel = channel_of(doc, "lead");
    assert_eq!(param(doc, &channel, "send2.on"), 1.0);
    assert_eq!(of_type(doc, REVERB).len(), 2);
    assert!(doc.validate().is_ok());
}

#[test]
fn the_decay_follows_the_median_gap_between_onsets() {
    let r = reg();
    let decay = |gap: i64, count: usize, vocal: bool| {
        let d = project(&r, gap, count);
        let track = d.doc().song.track(&key("lead")).unwrap().clone();
        suggested_decay(&d.doc().song, &track, vocal)
    };
    // 120 BPM: a quarter note is 0.5 s, so the decay is 1 s.
    assert!((decay(PPQ, 8, false) - 1.0).abs() < 1e-3);
    // Sixteenths: 0.125 s twice over is under the floor.
    assert_eq!(decay(PPQ / 4, 16, false), MIN_DECAY);
    // Half notes: 1 s, twice is 2 s.
    assert!((decay(2 * PPQ, 6, false) - 2.0).abs() < 1e-3);
    // A bar between notes asks for more than the cap: 4 s, and 3 s for a vocal.
    assert_eq!(decay(4 * PPQ, 6, false), MAX_DECAY);
    assert_eq!(decay(4 * PPQ, 6, true), MAX_VOCAL_DECAY);
    // Too few onsets to read a rhythm: the default.
    assert_eq!(decay(PPQ, 3, false), DEFAULT_DECAY);
    assert_eq!(decay(PPQ, 0, false), DEFAULT_DECAY);
}

#[test]
fn a_chord_is_one_onset_and_the_median_ignores_one_long_gap() {
    let r = reg();
    let mut d = Document::new(new_project_doc());
    // Chords on beats 0, 1, 2, 3 (three notes each) and one note much later.
    let mut notes = Vec::new();
    for beat in 0..4 {
        for key_ in [60u8, 64, 67] {
            notes.push(note(beat * PPQ, PPQ / 2, key_, 0.8));
        }
    }
    notes.push(note(40 * PPQ, PPQ / 2, 60, 0.8));
    run(
        &mut d,
        &r,
        [
            wf(WorkflowCommand::CreateInstrument {
                index: usize::MAX,
                source: CreateSource::Type(SYNTH.into()),
                key: Some(key("pad")),
                options: CreateOptions::default(),
            }),
            add_clip("pad", 0, notes_clip(0, 48 * PPQ, notes)),
        ],
    );
    let track = d.doc().song.track(&key("pad")).unwrap().clone();
    let gap = median_onset_gap(&d.doc().song, &track).unwrap();
    assert!((gap - 0.5).abs() < 1e-4, "{gap}");
}

#[test]
fn an_audio_style_track_without_notes_gets_the_default_decay_and_the_duck_release_stays() {
    let r = reg();
    let mut d = project(&r, PPQ, 0);
    one_step(&mut d, &r, send("lead", ReverbSendOptions::default()));
    let reverb = the(d.doc(), REVERB);
    assert_eq!(param(d.doc(), &reverb, "decay"), DEFAULT_DECAY);
    assert_eq!(
        param(d.doc(), &reverb, "duck_release"),
        300.0,
        "the reverb's own default"
    );
}

#[test]
fn the_duck_release_follows_the_rhythm() {
    let r = reg();
    let mut d = project(&r, PPQ, 8);
    d.apply(send("lead", ReverbSendOptions::default()), &r)
        .unwrap();
    let reverb = the(d.doc(), REVERB);
    // 0.5 s between notes: three quarters of it.
    assert!((param(d.doc(), &reverb, "duck_release") - 375.0).abs() < 1e-2);
}

#[test]
fn refusals_change_nothing() {
    let r = reg();
    let mut d = project(&r, PPQ, 8);
    let bad = |d: &mut Document, c: Command, text: &str| {
        let e = rejected(d, &r, c);
        assert!(e.to_string().contains(text), "{e} should mention {text:?}");
    };
    bad(
        &mut d,
        send("nothing", ReverbSendOptions::default()),
        "no track",
    );
    let out_of_range = [
        ReverbSendOptions {
            high_pass: 5.0,
            ..ReverbSendOptions::default()
        },
        ReverbSendOptions {
            low_pass: 100.0,
            ..ReverbSendOptions::default()
        },
        ReverbSendOptions {
            send_db: 20.0,
            ..ReverbSendOptions::default()
        },
        ReverbSendOptions {
            decay: Some(0.0),
            ..ReverbSendOptions::default()
        },
        ReverbSendOptions {
            send_db: f32::NAN,
            ..ReverbSendOptions::default()
        },
    ];
    for o in out_of_range {
        let e = rejected(&mut d, &r, send("lead", o));
        assert!(matches!(e, ModelError::OutOfRange(_)), "{e}");
    }
    // All eight slots used.
    for _ in 0..8 {
        d.apply(send("lead", ReverbSendOptions::default()), &r)
            .unwrap();
    }
    bad(
        &mut d,
        send("lead", ReverbSendOptions::default()),
        "send and return slots",
    );
}

#[test]
fn a_track_without_a_console_channel_is_refused() {
    let r = reg();
    let mut d = Document::new(new_project_doc());
    run(
        &mut d,
        &r,
        [wf(WorkflowCommand::CreateInstrument {
            index: usize::MAX,
            source: CreateSource::Type(SYNTH.into()),
            key: Some(key("lead")),
            options: CreateOptions::default(),
        })],
    );
    let channel = channel_of(d.doc(), "lead");
    d.apply(
        Command::Console(atomik_model::ConsoleCommand::RemoveChannel {
            channel: key(&channel),
        }),
        &r,
    )
    .unwrap();
    let e = rejected(&mut d, &r, send("lead", ReverbSendOptions::default()));
    assert!(e.to_string().contains("console channel"), "{e}");
}
