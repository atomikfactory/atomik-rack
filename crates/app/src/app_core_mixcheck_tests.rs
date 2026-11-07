//! The mix check in the app core: the check runs on a worker over the song and changes nothing, its
//! rows carry the fix and tool labels, and each fix (trim before a device, cut the low mids, make
//! mono) is one undo step that says what it did and refuses what it cannot do.

use super::*;
use crate::mixcheck_worker::CheckAnalysis;
use crate::sink::RecordingSink;
use atomik_core::PPQ;
use atomik_model::{
    Clip, ClipContent, ConsoleCommand, CreateOptions, CreateSource, InsertEffect, Note,
    WorkflowCommand, new_project_doc,
};
use atomik_render::mixcheck::{Finding, Fix, Kind, Tool};

const BAR: i64 = 4 * PPQ;
const SYNTH: &str = "atomik.poly-synth";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn sustained(note: u8) -> Clip {
    Clip {
        id: ClipId(0),
        start: 0,
        length: 4 * BAR,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        param_lanes: Vec::new(),
        midi: Default::default(),
        groove: None,
        controllers: Vec::new(),
        name: None,
        color: None,
        grid: None,
        content: ClipContent::Notes(vec![Note::new(0, 2 * BAR, note, 0.9)].into()),
    }
}

/// The starter project with a synth named `Bass` (D1, with a compressor on its inserts) and one
/// named `Pad`.
fn core() -> AppCore<RecordingSink> {
    let mut c = AppCore::with_doc(RecordingSink::new(), new_project_doc(), None);
    for (k, name, note) in [("bass", "Bass", 26u8), ("pad", "Pad", 52)] {
        c.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
            index: usize::MAX,
            source: CreateSource::Type(SYNTH.into()),
            key: Some(key(k)),
            options: CreateOptions::default(),
        }))
        .unwrap();
        c.apply_command(Command::Rename {
            key: key(k),
            name: name.into(),
        })
        .unwrap();
        for (p, v) in [
            ("osc1.wave", 0.0f32),
            ("env.amp.attack", 5.0),
            ("env.amp.sustain", 1.0),
        ] {
            c.apply_command(Command::SetParam {
                key: key(k),
                param: p.into(),
                value: v,
                merge: None,
            })
            .unwrap();
        }
        c.apply_command(Command::Song(SongCommand::AddClip {
            device: key(k),
            lane: 0,
            clip: sustained(note),
        }))
        .unwrap();
    }
    let bass = channel(&c, "bass");
    c.apply_command(Command::Console(ConsoleCommand::SetInsert {
        channel: bass,
        slot: 0,
        effect: Some(InsertEffect::New {
            type_id: "atomik.compressor".into(),
        }),
    }))
    .unwrap();
    c.apply_command(Command::Song(SongCommand::SetSongEnd(2 * BAR)))
        .unwrap();
    c
}

fn channel(c: &AppCore<RecordingSink>, source: &str) -> DeviceKey {
    c.doc()
        .console
        .channel_of(&key(source))
        .expect("channel")
        .channel
        .clone()
}

fn devices_of(c: &AppCore<RecordingSink>, type_id: &str) -> Vec<DeviceKey> {
    c.doc()
        .devices
        .iter()
        .filter(|d| d.type_id == type_id)
        .map(|d| d.key.clone())
        .collect()
}

fn value(c: &AppCore<RecordingSink>, k: &DeviceKey, p: &str) -> f32 {
    c.doc().param_value(k, p, c.registry()).expect("param")
}

fn finding(kind: Kind, fix: Option<Fix>, tool: Option<Tool>, at: Option<f64>) -> Finding {
    Finding {
        kind,
        track: "Track".into(),
        channel: None,
        measured: "a number".into(),
        value: 1.0,
        why: "a reason",
        at,
        fix,
        tool,
    }
}

fn install(c: &mut AppCore<RecordingSink>, findings: Vec<Finding>) {
    c.set_mixcheck_for_test(CheckAnalysis {
        from_frame: 48_000,
        rate: 48_000,
        strips: 3,
        findings,
    });
}

fn chain(c: &AppCore<RecordingSink>, strip: &DeviceKey) -> Vec<DeviceKey> {
    atomik_model::insert_chain(c.doc(), c.registry(), strip)
}

#[test]
fn the_check_finds_the_low_bass_root_and_changes_nothing() {
    let mut c = core();
    let before = c.doc().clone();
    assert!(c.mixcheck().is_none());
    c.start_mixcheck().expect("starts");
    assert!(c.mixcheck_running());
    assert!(c.start_mixcheck().is_err(), "one at a time");
    c.wait_background();
    assert!(!c.mixcheck_running());
    let found = c.mixcheck().expect("a result");
    assert_eq!(found.strips, 3, "two channels and the master");
    assert!(
        found
            .findings
            .iter()
            .any(|f| f.kind == Kind::BassRoot && f.track == "Bass"),
        "{:?}",
        found.findings
    );
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.starts_with("Mix check")),
        "{:?}",
        c.last_rejection()
    );
    assert_eq!(*c.doc(), before, "a check never changes the song");
    let data = c.mixcheck_data().expect("rows");
    assert!(data.rows.iter().any(|r| r.tag == "BASS ROOT" && !r.done));
}

#[test]
fn the_check_needs_a_channel_and_a_new_project_drops_the_result() {
    let mut c = AppCore::with_doc(RecordingSink::new(), new_project_doc(), None);
    assert!(c.start_mixcheck().is_err());
    assert!(c.last_rejection().is_some_and(|m| m.contains("channels")));
    let mut c = core();
    c.start_mixcheck().unwrap();
    c.wait_background();
    assert!(c.mixcheck().is_some());
    c.new_document();
    assert!(c.mixcheck().is_none());
}

#[test]
fn rows_carry_the_fix_label_and_only_the_tools_that_exist() {
    let mut c = core();
    let (bass, pad) = (channel(&c, "bass"), channel(&c, "pad"));
    let comp = chain(&c, &bass)[0].clone();
    install(
        &mut c,
        vec![
            finding(
                Kind::Level,
                Some(Fix::Trim {
                    strip: bass.clone(),
                    device: comp,
                    db: 12.5,
                }),
                None,
                None,
            ),
            finding(
                Kind::LowMid,
                Some(Fix::CutLowMids {
                    channel: pad,
                    hz: 252.0,
                }),
                Some(Tool::MaskingMap),
                None,
            ),
            finding(Kind::Peaks, None, Some(Tool::Clipper), Some(2.0)),
            finding(Kind::Mono, Some(Fix::MonoLow { channel: bass }), None, None),
        ],
    );
    let rows = c.mixcheck_data().expect("rows").rows;
    assert_eq!(rows[0].fix.as_deref(), Some("TRIM +12.5 dB"));
    assert_eq!(rows[1].fix.as_deref(), Some("CUT 252 Hz"));
    assert_eq!(rows[1].tool.as_deref(), Some("MASKING MAP"));
    assert_eq!(rows[2].tool, None, "the clipper hook has no button yet");
    assert_eq!(rows[3].fix.as_deref(), Some("MAKE MONO"));
    // The peak is at the song position of the range start plus its own time.
    assert_eq!(rows[2].at, Some(3.0));
    assert_eq!(c.mixcheck_tool(1), Some(Tool::MaskingMap));
    assert_eq!(c.mixcheck_tool(2), None);
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.contains("no clipper tool")),
        "{:?}",
        c.last_rejection()
    );
}

#[test]
fn a_trim_puts_a_gain_tool_before_the_device_in_one_undo_step() {
    let mut c = core();
    let bass = channel(&c, "bass");
    let comp = chain(&c, &bass)[0].clone();
    install(
        &mut c,
        vec![finding(
            Kind::Level,
            Some(Fix::Trim {
                strip: bass.clone(),
                device: comp.clone(),
                db: 12.5,
            }),
            None,
            None,
        )],
    );
    let before = c.doc().clone();
    c.fix_mixcheck(0).expect("fixes");
    let gain = devices_of(&c, "atomik.gain-tool");
    assert_eq!(gain.len(), 1);
    assert_eq!(
        chain(&c, &bass),
        vec![gain[0].clone(), comp],
        "the Gain Tool comes first"
    );
    assert_eq!(value(&c, &gain[0], "gain"), 12.5);
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.starts_with("Trimmed +12.5 dB before")),
        "{:?}",
        c.last_rejection()
    );
    assert!(c.mixcheck_data().unwrap().rows[0].done);
    assert!(c.fix_mixcheck(0).is_err(), "not twice");
    assert!(c.undo());
    assert_eq!(*c.doc(), before);
    assert!(c.redo());
    assert_eq!(devices_of(&c, "atomik.gain-tool").len(), 1);
}

#[test]
fn cutting_the_low_mids_sets_a_static_band_and_the_next_cut_reuses_the_device() {
    let mut c = core();
    let pad = channel(&c, "pad");
    let cut = |hz: f32| {
        finding(
            Kind::LowMid,
            Some(Fix::CutLowMids {
                channel: pad.clone(),
                hz,
            }),
            Some(Tool::MaskingMap),
            None,
        )
    };
    install(&mut c, vec![cut(252.0), cut(400.0)]);
    let before = c.doc().clone();
    c.fix_mixcheck(0).expect("fixes");
    let eq = devices_of(&c, "atomik.dynamic-eq");
    assert_eq!(eq.len(), 1);
    assert_eq!(chain(&c, &pad), eq);
    assert_eq!(value(&c, &eq[0], "band2.on"), 1.0);
    assert_eq!(value(&c, &eq[0], "band2.freq"), 252.0);
    assert_eq!(value(&c, &eq[0], "band2.gain"), -3.0);
    assert_eq!(value(&c, &eq[0], "band2.range"), 0.0, "a static cut");
    assert_eq!(value(&c, &eq[0], "band2.source"), 0.0, "no key");
    assert_eq!(value(&c, &eq[0], "band3.on"), 0.0);
    assert!(c.last_rejection().is_some_and(|m| m.contains("new device")));
    assert!(c.undo());
    assert_eq!(*c.doc(), before);
    assert!(c.redo());
    // The second cut takes the next free bell of the same device, as one more undo step.
    let after_first = c.doc().clone();
    c.fix_mixcheck(1).expect("fixes again");
    assert_eq!(devices_of(&c, "atomik.dynamic-eq").len(), 1);
    assert_eq!(value(&c, &eq[0], "band3.on"), 1.0);
    assert_eq!(value(&c, &eq[0], "band3.freq"), 400.0);
    assert!(c.undo());
    assert_eq!(*c.doc(), after_first);
}

#[test]
fn making_mono_adds_a_stereo_tool_with_mono_on_at_the_end_of_the_chain() {
    let mut c = core();
    let bass = channel(&c, "bass");
    let comp = chain(&c, &bass)[0].clone();
    install(
        &mut c,
        vec![finding(
            Kind::Mono,
            Some(Fix::MonoLow {
                channel: bass.clone(),
            }),
            None,
            None,
        )],
    );
    let before = c.doc().clone();
    c.fix_mixcheck(0).expect("fixes");
    let tool = devices_of(&c, "atomik.stereo-tool");
    assert_eq!(tool.len(), 1);
    assert_eq!(chain(&c, &bass), vec![comp, tool[0].clone()]);
    assert_eq!(value(&c, &tool[0], "mono"), 1.0);
    assert!(c.undo());
    assert_eq!(*c.doc(), before);
}

#[test]
fn a_fix_that_cannot_be_done_says_why_and_leaves_the_song_alone() {
    let mut c = core();
    assert!(c.fix_mixcheck(0).is_err(), "no check yet");
    let (bass, pad) = (channel(&c, "bass"), channel(&c, "pad"));
    install(
        &mut c,
        vec![
            finding(Kind::BassRoot, None, None, None),
            finding(
                Kind::Level,
                Some(Fix::Trim {
                    strip: bass.clone(),
                    device: key("not-there"),
                    db: 3.0,
                }),
                None,
                None,
            ),
            finding(
                Kind::Mono,
                Some(Fix::MonoLow {
                    channel: pad.clone(),
                }),
                None,
                None,
            ),
        ],
    );
    let before = c.doc().clone();
    assert!(c.fix_mixcheck(0).is_err(), "no fix on that finding");
    assert!(c.fix_mixcheck(9).is_err(), "no such finding");
    assert!(c.fix_mixcheck(1).is_err());
    assert!(c.last_rejection().is_some_and(|m| m.contains("no longer")));
    assert_eq!(*c.doc(), before);
    // A channel that was removed since the check.
    c.apply_command(Command::RemoveDevice { key: key("pad") })
        .unwrap();
    let removed = c.doc().clone();
    assert!(c.fix_mixcheck(2).is_err());
    assert!(c.last_rejection().is_some_and(|m| m.contains("no longer")));
    assert_eq!(*c.doc(), removed);
    assert!(c.mixcheck_data().unwrap().rows.iter().all(|r| !r.done));
}

#[test]
fn locating_a_finding_moves_the_playhead_to_its_time() {
    let mut c = core();
    install(
        &mut c,
        vec![
            finding(Kind::Peaks, None, Some(Tool::Clipper), Some(1.0)),
            finding(Kind::BassRoot, None, None, None),
        ],
    );
    c.locate_mixcheck(0);
    // The range starts at 1 s and the peak is 1 s into it: 2 s of the song.
    let want = c.doc().song.frame_clock(48_000).tick_at(2 * 48_000);
    assert!(want > 0);
    assert_eq!(c.position(), want);
    // A finding with no time, or none at all, does nothing.
    c.locate_mixcheck(1);
    c.locate_mixcheck(7);
    assert_eq!(c.position(), want);
}
