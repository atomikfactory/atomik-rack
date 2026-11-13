//! Add Transition in the app core: a riser and an impact are printed on the freeze worker (each
//! one undo step, the riser ending where the section starts, the impact landing on it), the
//! filter down, the silence and the fill are one edit each, a boundary comes from the playhead
//! or the selection, and the count warns when one kind is on most boundaries.

use super::*;
use crate::sink::RecordingSink;
use crate::transition_choice::TransitionChoice;
use atomik_core::PPQ;
use atomik_engine::SongAsset;
use atomik_model::transition::TransitionKind;
use atomik_model::{
    Clip, ClipContent, CreateOptions, CreateSource, Marker, Note, WorkflowCommand, insert_chain,
    new_project_doc,
};

const BAR: i64 = 4 * PPQ;
const DRUMS: &str = "atomik.drum-machine";

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_owned())
}

fn notes(start: i64, length: i64, notes: Vec<Note>) -> Clip {
    Clip {
        id: ClipId(0),
        start,
        length,
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
        content: ClipContent::Notes(notes.into()),
    }
}

fn instrument(c: &mut AppCore<RecordingSink>, type_id: &str, k: &str) {
    c.apply_command(Command::Workflow(WorkflowCommand::CreateInstrument {
        index: usize::MAX,
        source: CreateSource::Type(type_id.into()),
        key: Some(key(k)),
        options: CreateOptions::default(),
    }))
    .unwrap();
    c.apply_command(Command::Rename {
        key: key(k),
        name: k[..1].to_uppercase() + &k[1..],
    })
    .unwrap();
}

fn set(c: &mut AppCore<RecordingSink>, device: &str, param: &str, value: f32) {
    c.apply_command(Command::SetParam {
        key: key(device),
        param: param.to_owned(),
        value,
        merge: None,
    })
    .unwrap();
}

fn markers(c: &mut AppCore<RecordingSink>, bars: &[i64]) {
    let list = bars
        .iter()
        .enumerate()
        .map(|(i, b)| Marker {
            id: i as u32 + 1,
            tick: (b - 1) * BAR,
            name: format!("S{}", i + 1),
            color: None,
        })
        .collect();
    c.apply_command(Command::Song(SongCommand::SetMarkers(list)))
        .unwrap();
}

/// A synth `lead` with a clip of four bars from bar 2 (a note on every bar, then one on each
/// beat of the first bar), a drum machine `drums` with a kick on every beat for six bars, and
/// markers at bars 2 to 5.
fn song() -> AppCore<RecordingSink> {
    let mut c = AppCore::with_doc(RecordingSink::new(), new_project_doc(), None);
    instrument(&mut c, "atomik.poly-synth", "lead");
    instrument(&mut c, DRUMS, "drums");
    // The first pad plays a synthesised kick: the render has something to print.
    set(&mut c, "drums", "pad1.module", 1.0);
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key("lead"),
        lane: 0,
        clip: notes(
            BAR,
            4 * BAR,
            (0..4)
                .map(|b| Note::new(b * BAR, PPQ, 64 + b as u8, 0.8))
                .collect(),
        ),
    }))
    .unwrap();
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key("drums"),
        lane: 0,
        clip: notes(
            BAR,
            6 * BAR,
            (0..24)
                .map(|b| Note::new(b * PPQ, PPQ / 4, 36, 0.9))
                .collect(),
        ),
    }))
    .unwrap();
    markers(&mut c, &[2, 3, 4, 5]);
    c.sink_mut().take();
    c
}

/// Puts the playhead at bar `bar` (1-based): the next transition goes at the boundary there.
fn at_bar(c: &mut AppCore<RecordingSink>, bar: i64) {
    c.arrangement_mut().set_playhead((bar - 1) * BAR);
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|s| s * s).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

fn audio_of(c: &AppCore<RecordingSink>, clip: &Clip) -> Vec<Vec<f32>> {
    let ClipContent::Audio(a) = &clip.content else {
        panic!("not an audio clip");
    };
    let SongAsset::Memory { data, .. } = &c.song_assets()[&a.asset] else {
        panic!("the render is not in memory");
    };
    data.to_vec()
}

fn frame(c: &AppCore<RecordingSink>, tick: i64) -> u64 {
    c.doc().song.frame_at(tick, c.doc().sample_rate)
}

/// The audio track right after `device`.
fn track_after<'a>(c: &'a AppCore<RecordingSink>, device: &str) -> &'a atomik_model::Track {
    let tracks = &c.doc().song.tracks;
    let at = tracks.iter().position(|t| t.device == key(device)).unwrap();
    &tracks[at + 1]
}

fn said(c: &AppCore<RecordingSink>) -> String {
    c.last_rejection().unwrap_or_default().to_owned()
}

#[test]
fn a_riser_ends_where_the_section_starts_and_is_one_undo_step() {
    let mut c = song();
    at_bar(&mut c, 4);
    let tracks = c.doc().song.tracks.len();
    let depth = c.document().undo_depth();
    c.add_transition(TransitionChoice::Riser(2));
    assert!(
        c.freeze_progress()
            .is_some_and(|t| t.starts_with("Printing a riser from Drums")),
        "the drum machine plays the first hit: {:?}",
        c.freeze_progress()
    );
    c.wait_background();
    assert_eq!(c.doc().song.tracks.len(), tracks + 1);
    let track = track_after(&c, "drums");
    assert_eq!(c.doc().device(&track.device).unwrap().name, "Drums (riser)");
    let clip = track.lanes[0].clips[0].clone();
    // It ends exactly at the first frame of bar 2.
    assert!(frame(&c, clip.end()).abs_diff(frame(&c, 3 * BAR)) <= 1);
    // Two bars at 120 BPM are four seconds: the print fills most of it and rises toward its end.
    let audio = audio_of(&c, &clip);
    let frames = audio[0].len();
    assert!((190_000..=192_100).contains(&frames), "{frames} frames");
    let tenth = frames / 10;
    assert!(rms(&audio[0][frames - tenth..]) > 3.0 * rms(&audio[0][..tenth]));
    assert_eq!(c.arrangement().selected_clips(), vec![clip.id]);
    assert!(
        said(&c).starts_with("Printed a riser from Drums (2 bars")
            && said(&c).contains("rising into bar 4"),
        "{}",
        said(&c)
    );
    assert_eq!(c.document().undo_depth(), depth + 1, "one undo step");
    c.undo();
    assert_eq!(c.doc().song.tracks.len(), tracks);
}

#[test]
fn a_riser_with_no_room_before_the_start_or_no_hit_says_so() {
    let mut c = song();
    // A section where nothing plays in the first beat: bar 9 holds no clip.
    markers(&mut c, &[2, 9]);
    c.arrangement_mut().set_playhead(8 * BAR);
    c.add_transition(TransitionChoice::Impact);
    assert!(
        said(&c).starts_with("No track plays a note or an audio clip in the first beat of bar 9"),
        "{}",
        said(&c)
    );
    assert!(!c.freeze_busy());
}

#[test]
fn an_impact_lands_on_the_downbeat_and_rings_on_after_the_hit() {
    let mut c = song();
    at_bar(&mut c, 3);
    let tracks = c.doc().song.tracks.len();
    c.add_transition(TransitionChoice::Impact);
    assert!(
        c.freeze_progress()
            .is_some_and(|t| t.starts_with("Printing an impact from Drums")),
        "{:?}",
        c.freeze_progress()
    );
    c.wait_background();
    assert_eq!(c.doc().song.tracks.len(), tracks + 1, "{}", said(&c));
    let track = track_after(&c, "drums");
    assert_eq!(
        c.doc().device(&track.device).unwrap().name,
        "Drums (impact)"
    );
    let clip = track.lanes[0].clips[0].clone();
    assert_eq!(clip.start, 2 * BAR, "it starts on the boundary");
    // One beat of the hit and three seconds of reverb after it.
    let audio = audio_of(&c, &clip);
    let frames = audio[0].len();
    assert!(frames > 48_000 * 3, "{frames} frames");
    assert_eq!(frames as u64, frame(&c, clip.end()) - frame(&c, clip.start));
    // The hit is loud and the tail goes on past the beat.
    assert!(rms(&audio[0][..2_400]) > 0.05);
    assert!(rms(&audio[0][48_000..96_000]) > 1.0e-4, "the tail rings");
    assert!(
        said(&c).starts_with("Printed an impact from Drums") && said(&c).contains("bar 3"),
        "{}",
        said(&c)
    );
    c.undo();
    assert_eq!(c.doc().song.tracks.len(), tracks, "one undo step");
}

#[test]
fn the_filter_down_and_the_silence_are_one_edit_each_and_say_what_they_did() {
    let mut c = song();
    at_bar(&mut c, 4);
    let depth = c.document().undo_depth();
    c.add_transition(TransitionChoice::FilterDown(2));
    assert_eq!(c.document().undo_depth(), depth + 1, "one undo step");
    let master = c.doc().console.master.clone().unwrap();
    let chain = insert_chain(c.doc(), &c.registry, &master);
    assert_eq!(chain.len(), 1);
    let lane = |c: &AppCore<RecordingSink>, device: &DeviceKey, param: &str| {
        let t = c.doc().song.track(device).unwrap();
        t.lanes[t.automation_lane(param).unwrap()].clips[0].clone()
    };
    let filter = lane(&c, &chain[0], "cutoff");
    assert_eq!((filter.start, filter.length), (BAR, 2 * BAR));
    assert_eq!(
        said(&c),
        "Added a low-pass on the master that closes over the last 2 bars before bar 4"
    );

    let depth = c.document().undo_depth();
    c.add_transition(TransitionChoice::Silence);
    assert_eq!(c.document().undo_depth(), depth + 1);
    let silence = lane(&c, &master, "fader");
    assert_eq!((silence.start, silence.length), (3 * BAR, PPQ));
    assert!(said(&c).starts_with("Silenced the master for beat one of bar 4"));
    c.undo();
    c.undo();
    assert!(
        insert_chain(c.doc(), &c.registry, &master).is_empty(),
        "both gone"
    );
}

#[test]
fn a_fill_plays_the_last_beat_of_the_drums_twice_and_is_one_undo_step() {
    let mut c = song();
    at_bar(&mut c, 3);
    let depth = c.document().undo_depth();
    let before: Vec<Note> =
        match &c.doc().song.track(&key("drums")).unwrap().lanes[0].clips[0].content {
            ClipContent::Notes(n) => n.to_vec(),
            _ => panic!(),
        };
    c.add_transition(TransitionChoice::Fill);
    assert_eq!(c.document().undo_depth(), depth + 1);
    let after: Vec<Note> =
        match &c.doc().song.track(&key("drums")).unwrap().lanes[0].clips[0].content {
            ClipContent::Notes(n) => n.to_vec(),
            _ => panic!(),
        };
    // The clip starts at bar 2: the last beat before bar 3 is its fourth beat.
    let last = 3 * PPQ;
    let in_beat: Vec<i64> = after
        .iter()
        .map(|n| n.tick)
        .filter(|t| (last..last + PPQ).contains(t))
        .collect();
    assert_eq!(in_beat, vec![last, last + PPQ / 2]);
    assert_eq!(after.len(), before.len() + 1);
    assert!(
        said(&c).starts_with("Filled the last beat before bar 3 on Drums"),
        "{}",
        said(&c)
    );
    c.undo();
    assert_eq!(c.document().undo_depth(), depth);
}

#[test]
fn a_fill_goes_to_the_selected_track_when_it_can() {
    let mut c = song();
    // At the boundary of bar 3 the lead has no note in the last beat of bar 2: only the drums.
    at_bar(&mut c, 3);
    c.arrangement_mut().set_selected_track(Some(key("lead")));
    c.add_transition(TransitionChoice::Fill);
    assert!(said(&c).contains("on Drums"), "{}", said(&c));
}

#[test]
fn a_group_filter_needs_a_group_bus_under_the_selected_track() {
    let mut c = song();
    at_bar(&mut c, 4);
    c.add_transition(TransitionChoice::GroupFilterDown(1));
    assert!(
        said(&c).starts_with("Select a track that plays into a group bus"),
        "{}",
        said(&c)
    );
    let channel = c
        .doc()
        .console
        .channel_of(&key("drums"))
        .unwrap()
        .channel
        .clone();
    c.apply_command(Command::Console(atomik_model::ConsoleCommand::CreateVca {
        name: "Drums".into(),
        members: vec![channel],
    }))
    .unwrap();
    c.apply_command(Command::Console(
        atomik_model::ConsoleCommand::RouteVcaToNewBus {
            vca: 1,
            name: "Drum bus".into(),
        },
    ))
    .unwrap();
    c.arrangement_mut().set_selected_track(Some(key("drums")));
    c.add_transition(TransitionChoice::GroupFilterDown(1));
    assert!(
        said(&c).starts_with("Added a low-pass on Drum bus that closes over the last 1 bar"),
        "{}",
        said(&c)
    );
}

#[test]
fn a_kind_on_most_boundaries_is_named_in_the_status_line() {
    let mut c = song();
    for bar in [2, 3] {
        at_bar(&mut c, bar);
        c.add_transition(TransitionChoice::Silence);
        assert!(!said(&c).contains("boundaries"), "{}", said(&c));
    }
    // A different kind on the fourth boundary does not count towards the silence.
    at_bar(&mut c, 5);
    c.add_transition(TransitionChoice::FilterDown(1));
    assert!(!said(&c).contains("boundaries"), "{}", said(&c));
    // The third silence is three of four boundaries.
    at_bar(&mut c, 4);
    c.add_transition(TransitionChoice::Silence);
    assert!(
        said(&c).contains("Silence on beat one is on 3 of 4 boundaries"),
        "{}",
        said(&c)
    );
    // Undone, it no longer counts: the same silence at bar 5 is the third use again, not a fourth.
    c.undo();
    at_bar(&mut c, 5);
    c.add_transition(TransitionChoice::Silence);
    assert!(
        said(&c).contains("Silence on beat one is on 3 of 4 boundaries"),
        "{}",
        said(&c)
    );
    assert_eq!(c.transitions.len(), 4);
    assert!(
        c.transitions
            .iter()
            .filter(|t| t.1 == TransitionKind::Silence)
            .count()
            == 3
    );
}

#[test]
fn a_new_project_starts_the_count_again() {
    let mut c = song();
    at_bar(&mut c, 2);
    c.add_transition(TransitionChoice::Silence);
    assert_eq!(c.transitions.len(), 1);
    c.new_document();
    assert!(c.transitions.is_empty());
}

#[test]
fn a_project_without_markers_uses_its_section_lines_and_one_without_a_hit_says_so() {
    // Every eight bars is a section line when there are no markers: the first is bar 9.
    let mut c = song();
    markers(&mut c, &[]);
    c.arrangement_mut().set_playhead(0);
    c.add_transition(TransitionChoice::Silence);
    assert!(said(&c).contains("beat one of bar 9"), "{}", said(&c));
    // Nothing plays there: a print has no hit to start from.
    c.add_transition(TransitionChoice::Riser(1));
    assert!(
        said(&c).starts_with("No track plays a note or an audio clip in the first beat of bar 9"),
        "{}",
        said(&c)
    );
}
