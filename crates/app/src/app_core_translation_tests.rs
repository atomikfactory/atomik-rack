//! The translation lens, "Will the bass read?" and Split Sub in the app core: the lens is monitoring
//! state (sent to the engine, sent again to a new one, never in the project, never an undo step,
//! never in a render), the reading runs on a worker over a strip and changes nothing, and Split Sub
//! is one undo step that selects the container and says what it did.

use super::*;
use crate::sink::{RecordingSink, SinkCall};
use atomik_core::PPQ;
use atomik_dsp::bass_read::Verdict;
use atomik_engine::{LensSettings, LensVoicing};
use atomik_model::{
    Clip, ClipContent, CreateOptions, CreateSource, Note, WorkflowCommand, new_project_doc,
};

const BAR: i64 = 4 * PPQ;
const SYNTH: &str = "atomik.poly-synth";
const BASS: &str = "atomik.mono-bass";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn line() -> Clip {
    Clip {
        id: ClipId(0),
        start: 0,
        length: 2 * BAR,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        param_lanes: Vec::new(),
        midi: Default::default(),
        groove: None,
        follow_song_key: true,
        controllers: Vec::new(),
        name: None,
        color: None,
        grid: None,
        content: ClipContent::Notes(
            [36u8, 41]
                .iter()
                .enumerate()
                .map(|(i, &k)| Note::new(i as i64 * BAR, BAR - PPQ / 2, k, 0.9))
                .collect::<Vec<_>>()
                .into(),
        ),
    }
}

/// A project with a bass track of `type_id` playing a short line, after setting `params`.
fn core_with(type_id: &str, params: &[(&str, f32)]) -> AppCore<RecordingSink> {
    let mut c = AppCore::with_doc(RecordingSink::new(), new_project_doc(), None);
    c.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
        index: usize::MAX,
        source: CreateSource::Type(type_id.into()),
        key: Some(key("bass")),
        options: CreateOptions::default(),
    }))
    .unwrap();
    for &(p, v) in params {
        c.apply_command(Command::SetParam {
            key: key("bass"),
            param: p.into(),
            value: v,
            merge: None,
        })
        .unwrap();
    }
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key("bass"),
        lane: 0,
        clip: line(),
    }))
    .unwrap();
    c.apply_command(Command::Song(SongCommand::SetSongEnd(2 * BAR)))
        .unwrap();
    c
}

/// A bright saw or a pure sine from the first oscillator only.
fn raw(wave: f32) -> Vec<(&'static str, f32)> {
    vec![
        ("osc1.wave", wave),
        ("osc.mix", 0.0),
        ("filter.cutoff", 20_000.0),
        ("filter.env_amount", 0.0),
        ("env.amp.attack", 5.0),
        ("env.amp.sustain", 1.0),
        ("master.level", -6.0),
    ]
}

fn lens_calls(c: &AppCore<RecordingSink>) -> Vec<LensSettings> {
    c.sink()
        .calls
        .iter()
        .filter_map(|call| match call {
            SinkCall::LensSet(s) => Some(*s),
            _ => None,
        })
        .collect()
}

// ------------------------------------------------------------------------------------ the lens

#[test]
fn the_lens_voicings_toggle_and_mono_is_a_switch_of_its_own() {
    let mut c = core_with(SYNTH, &[]);
    assert!(c.lens().is_neutral());
    c.set_lens_voicing(LensVoicing::SmallSpeaker);
    assert_eq!(c.lens().voicing, LensVoicing::SmallSpeaker);
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.contains("Small Speaker"))
    );
    // The other voicing replaces it; the same one again goes back to headphones.
    c.set_lens_voicing(LensVoicing::ClubSub);
    assert_eq!(c.lens().voicing, LensVoicing::ClubSub);
    c.toggle_lens_mono();
    assert_eq!(
        c.lens(),
        LensSettings {
            voicing: LensVoicing::ClubSub,
            mono: true
        }
    );
    assert_eq!(c.lens_status().as_deref(), Some("LENS CLUB SUB + MONO"));
    c.set_lens_voicing(LensVoicing::ClubSub);
    assert_eq!(c.lens().voicing, LensVoicing::Headphones);
    assert!(c.lens().mono, "mono stays");
    // Headphones switches everything off and says so.
    c.lens_off();
    assert!(c.lens().is_neutral());
    assert_eq!(c.lens_status(), None);
    assert!(c.last_rejection().is_some_and(|m| m.contains("lens off")));
}

#[test]
fn every_change_is_sent_once_and_a_new_engine_gets_the_lens_again() {
    let mut c = core_with(SYNTH, &[]);
    assert!(lens_calls(&c).is_empty(), "nothing is sent while it is off");
    c.set_lens_voicing(LensVoicing::SmallSpeaker);
    c.toggle_lens_mono();
    let small = LensSettings {
        voicing: LensVoicing::SmallSpeaker,
        mono: false,
    };
    assert_eq!(
        lens_calls(&c),
        [
            small,
            LensSettings {
                mono: true,
                ..small
            }
        ]
    );
    // A new engine starts with the lens off: it hears the setting again.
    let sent = lens_calls(&c).len();
    c.rebuild_engine();
    let after = lens_calls(&c);
    assert_eq!(after.len(), sent + 1);
    assert_eq!(
        after.last(),
        Some(&LensSettings {
            voicing: LensVoicing::SmallSpeaker,
            mono: true
        })
    );
    // A lens that is off after a rebuild sends nothing.
    c.lens_off();
    let sent = lens_calls(&c).len();
    c.rebuild_engine();
    assert_eq!(lens_calls(&c).len(), sent);
}

#[test]
fn the_lens_is_not_an_undo_step_and_is_not_in_the_project() {
    let mut c = core_with(SYNTH, &[]);
    let text = atomik_model::save_project(c.doc());
    let revision = c.document().revision();
    let undo = c.document().undo_label();
    c.set_lens_voicing(LensVoicing::SmallSpeaker);
    c.toggle_lens_mono();
    assert_eq!(c.document().revision(), revision, "the song did not change");
    assert_eq!(c.document().undo_label(), undo, "nothing to undo");
    assert_eq!(atomik_model::save_project(c.doc()), text);
    assert!(!text.to_lowercase().contains("lens"));
}

#[test]
fn a_bounce_never_hears_the_lens() {
    let mut c = core_with(SYNTH, &raw(0.0));
    let render = |c: &AppCore<RecordingSink>| {
        atomik_render::project::render_doc(
            c.doc(),
            &atomik_render::default_registry(),
            &atomik_render::project::ProjectOptions::default(),
        )
        .expect("renders")
        .channels
    };
    let plain = render(&c);
    c.set_lens_voicing(LensVoicing::SmallSpeaker);
    c.toggle_lens_mono();
    let with_lens = render(&c);
    assert_eq!(plain.len(), with_lens.len());
    for (a, b) in plain.iter().zip(&with_lens) {
        assert!(a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits()));
    }
}

/// The source of every file under `dir`, with its path.
fn sources(dir: &std::path::Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push((
                    path.display().to_string(),
                    std::fs::read_to_string(&path).unwrap(),
                ));
            }
        }
    }
    out
}

#[test]
fn nothing_that_renders_or_saves_knows_the_lens() {
    // The lens is a message to the live engine; the crates that render a document and write a
    // project never name it.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    for krate in ["render", "model"] {
        for (path, text) in sources(&root.join(krate).join("src")) {
            for word in ["lens_set", "LensSettings", "LensVoicing", "dsp::lens"] {
                assert!(!text.contains(word), "{path} names {word}");
            }
        }
    }
}

// ------------------------------------------------------------------------- will the bass read

#[test]
fn the_reading_is_high_for_a_saw_and_low_for_a_sine_and_changes_nothing() {
    for (wave, verdict) in [(0.0, Verdict::Reads), (3.0, Verdict::Lost)] {
        let mut c = core_with(SYNTH, &raw(wave));
        let before = c.doc().clone();
        assert!(c.bass_reading().is_none());
        // The master is first, then the channels.
        let data = c.bass_read_data();
        assert_eq!(data.sources.len(), 2);
        assert_eq!(data.sources[0], "Master");
        c.bass_read_choose(1);
        c.start_bass_read().expect("starts");
        assert!(c.bass_read_running());
        assert!(c.start_bass_read().is_err(), "one at a time");
        c.wait_background();
        assert!(!c.bass_read_running());
        let (reading, stale) = c.bass_reading().expect("a reading");
        let (reading, stale) = (*reading, stale);
        assert!(!stale);
        assert_eq!(reading.verdict, verdict, "{reading:?}");
        assert_eq!(*c.doc(), before, "a reading never changes the song");
        let row = c.bass_read_data().reading.expect("the panel has it");
        assert!((row.share - reading.share).abs() < 1e-6);
        assert!(row.source.contains("Poly Synth") || row.source.contains("Synth"));
        assert!(!row.verdict.is_empty() && !row.advice.is_empty());
    }
}

#[test]
fn the_master_can_be_read_too_and_the_selected_track_is_preselected() {
    let mut c = core_with(SYNTH, &raw(0.0));
    c.bass_read_preselect();
    assert_eq!(
        c.bass_read_data().chosen,
        0,
        "no track selected: the master"
    );
    c.arrangement_mut().set_selected_track(Some(key("bass")));
    c.bass_read_preselect();
    assert_eq!(c.bass_read_data().chosen, 1, "the channel of the track");
    c.bass_read_choose(0);
    c.start_bass_read().unwrap();
    c.wait_background();
    let reading = *c.bass_reading().expect("a reading").0;
    assert_eq!(reading.verdict, Verdict::Reads, "{reading:?}");
    assert_eq!(c.bass_read_data().reading.unwrap().source, "Master");
}

#[test]
fn the_reading_goes_stale_when_the_song_changes_and_a_new_project_drops_it() {
    let mut c = core_with(SYNTH, &raw(0.0));
    c.start_bass_read().unwrap();
    c.wait_background();
    assert!(c.bass_reading().is_some_and(|(_, stale)| !stale));
    c.apply_command(Command::Song(SongCommand::SetSongEnd(3 * BAR)))
        .unwrap();
    assert!(c.bass_reading().is_some_and(|(_, stale)| stale));
    assert!(c.bass_read_data().reading.unwrap().stale);
    c.new_document();
    assert!(c.bass_reading().is_none());
}

#[test]
fn an_empty_song_has_nothing_to_read() {
    let mut c = AppCore::with_doc(RecordingSink::new(), new_project_doc(), None);
    let why = c.start_bass_read().expect_err("nothing to read");
    assert!(why.contains("empty") || why.contains("no master"), "{why}");
    assert!(!c.bass_read_running());
}

#[test]
fn a_cancelled_reading_leaves_no_result() {
    let mut c = core_with(SYNTH, &raw(0.0));
    c.start_bass_read().unwrap();
    c.cancel_bass_read();
    c.wait_background();
    assert!(!c.bass_read_running());
    assert!(c.bass_reading().is_none());
    assert!(c.last_rejection().is_some_and(|m| m.contains("cancelled")));
}

// ------------------------------------------------------------------------------- split sub

#[test]
fn split_sub_is_one_undo_step_that_selects_the_container_and_says_what_it_did() {
    let mut c = core_with(BASS, &[]);
    c.view_mut().set_selection(&[], None);
    assert!(!c.split_sub_menu(), "nothing selected");
    assert!(!c.split_sub());
    assert!(c.last_rejection().is_some_and(|m| m.contains("Mono Bass")));
    c.view_mut()
        .set_selection(&[key("bass")], Some(&key("bass")));
    assert!(c.split_sub_menu());
    assert!(c.can_split_sub(&key("bass")));
    let before = c.doc().clone();
    let undo_before = c.document().undo_label();
    assert!(c.split_sub());
    let container = c
        .doc()
        .devices
        .iter()
        .find(|d| d.inner.is_some())
        .map(|d| d.key.clone())
        .expect("a container");
    assert_eq!(c.view().selection(), vec![container.clone()]);
    assert_eq!(c.document().undo_label(), Some("Split Sub"));
    assert!(
        c.last_rejection()
            .is_some_and(|m| m.starts_with("Split Sub:"))
    );
    // One undo brings the bass back with its track.
    c.undo();
    assert_eq!(c.doc().devices, before.devices);
    assert_eq!(c.doc().song, before.song);
    assert_eq!(c.document().undo_label(), undo_before);
    // The container is no Mono Bass: Split Sub is off for it.
    c.redo();
    c.view_mut()
        .set_selection(std::slice::from_ref(&container), Some(&container));
    assert!(!c.split_sub_menu());
}

#[test]
fn split_sub_works_on_the_selected_track_and_refuses_other_instruments() {
    let mut c = core_with(BASS, &[]);
    c.arrangement_mut().set_selected_track(Some(key("bass")));
    assert!(c.split_sub_menu(), "the selected track's device");
    let mut s = core_with(SYNTH, &[]);
    s.view_mut()
        .set_selection(&[key("bass")], Some(&key("bass")));
    assert!(!s.split_sub_menu());
    assert!(!s.can_split_sub(&key("bass")));
}

#[test]
fn a_split_bass_still_plays_notes_through_the_container() {
    let mut c = core_with(BASS, &[]);
    c.view_mut()
        .set_selection(&[key("bass")], Some(&key("bass")));
    assert!(c.split_sub());
    // The engine got the expanded rack: the bass, the sub layer, the low cut and the sum, each as
    // a device of its own, and the container's split.
    let flat = c.doc().expand(c.registry());
    let types: Vec<&str> = flat.devices.iter().map(|d| d.type_id.as_str()).collect();
    for wanted in [BASS, SYNTH, "atomik.eq", "atomik.line-mixer"] {
        assert!(types.contains(&wanted), "{wanted} in {types:?}");
    }
}
