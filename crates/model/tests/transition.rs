//! `WorkflowCommand::AddTransition`: the silence, the filter down and the fill each in one undo
//! step with the clips or automation they promise; which track holds the next section's first
//! hit; the length of a riser; and the count that warns when one kind is on most boundaries.

mod common;

use atomik_device_sdk::Registry;
use atomik_model::transition::{
    CUTOFF_LANE, FADER_LANE, FLOOR_HZ, MIN_USES, OPEN_HZ, TransitionEdit, TransitionKind,
    boundaries, fill_tracks, first_hit, hit_span, nearest_boundary, output_bus, overuse,
    riser_seconds,
};
use atomik_model::{
    ClipContent, Command, ConsoleCommand, Curve, DeviceKey, Document, Marker, RackDoc, SongCommand,
    WorkflowCommand, insert_chain,
};
use common::song::*;
use common::workflow::*;
use common::*;

const FILTER: &str = "atomik.auto-filter";
const DRUMS: &str = "atomik.drum-machine";

fn reg() -> Registry {
    console_registry()
}

/// A synth `lead` and a drum machine `drums`, each with a track: the drums play a note on every
/// beat for 8 bars, the lead one note per bar.
fn project(r: &Registry) -> Document {
    let mut d = Document::new(atomik_model::empty_doc());
    d.apply(add(0, SYNTH, Some("lead")), r).unwrap();
    d.apply(add(1, DRUMS, Some("drums")), r).unwrap();
    d.apply(add_track("lead"), r).unwrap();
    d.apply(add_track("drums"), r).unwrap();
    let beats = (0..32)
        .flat_map(|b| {
            [
                note(b * PPQ, PPQ / 4, 36, 0.9),
                note(b * PPQ + PPQ / 2, PPQ / 4, 38, 0.7),
            ]
        })
        .collect();
    d.apply(add_clip("drums", 0, notes_clip(0, 8 * BAR, beats)), r)
        .unwrap();
    let bars = (0..8).map(|b| note(b * BAR, BAR / 2, 60, 0.8)).collect();
    d.apply(add_clip("lead", 0, notes_clip(0, 8 * BAR, bars)), r)
        .unwrap();
    d
}

fn add_transition(at: i64, edit: TransitionEdit) -> Command {
    wf(WorkflowCommand::AddTransition { at, edit })
}

fn master(doc: &RackDoc) -> DeviceKey {
    doc.console.master.clone().unwrap()
}

/// The value a lane gives `param` of `device` at `tick`.
fn shown(doc: &RackDoc, device: &DeviceKey, param: &str, tick: i64) -> Option<f32> {
    doc.automation_curves()
        .values_at(tick as f64)
        .get(&(device.clone(), param.to_owned()))
        .copied()
}

fn lane_clips(doc: &RackDoc, device: &DeviceKey, param: &str) -> Vec<(i64, i64, usize)> {
    let t = doc.song.track(device).unwrap();
    let lane = &t.lanes[t.automation_lane(param).unwrap()];
    lane.clips
        .iter()
        .map(|c| match &c.content {
            ClipContent::Automation(p) => (c.start, c.length, p.len()),
            other => panic!("{other:?}"),
        })
        .collect()
}

#[test]
fn silence_is_one_undo_step_with_the_master_fader_at_zero_for_one_beat() {
    let r = reg();
    let mut d = project(&r);
    let at = 4 * BAR;
    let tracks = d.doc().song.tracks.len();
    one_step(&mut d, &r, add_transition(at, TransitionEdit::Silence));
    let doc = d.doc();
    let master = master(doc);
    // A track for the master's lane appeared, with one clip of one beat from the boundary.
    assert_eq!(doc.song.tracks.len(), tracks + 1);
    assert_eq!(lane_clips(doc, &master, FADER_LANE), vec![(at, PPQ, 1)]);
    let unity = shown(doc, &master, FADER_LANE, at - 1).unwrap();
    assert!(unity > 0.5, "the mix is at its fader before: {unity}");
    assert_eq!(shown(doc, &master, FADER_LANE, at), Some(0.0));
    assert_eq!(shown(doc, &master, FADER_LANE, at + PPQ - 1), Some(0.0));
    assert_eq!(shown(doc, &master, FADER_LANE, at + PPQ), Some(unity));
    assert_eq!(hit_span(&doc.song, at), (at, at + PPQ));
}

#[test]
fn a_second_silence_goes_on_the_same_lane() {
    let r = reg();
    let mut d = project(&r);
    d.apply(add_transition(4 * BAR, TransitionEdit::Silence), &r)
        .unwrap();
    one_step(&mut d, &r, add_transition(6 * BAR, TransitionEdit::Silence));
    let doc = d.doc();
    let clips = lane_clips(doc, &master(doc), FADER_LANE);
    assert_eq!(clips, vec![(4 * BAR, PPQ, 1), (6 * BAR, PPQ, 1)]);
    // Between them the fader is where it was.
    let between = shown(doc, &master(doc), FADER_LANE, 5 * BAR).unwrap();
    assert!(between > 0.5);
}

#[test]
fn a_silence_over_another_is_refused_and_changes_nothing() {
    let r = reg();
    let mut d = project(&r);
    d.apply(add_transition(4 * BAR, TransitionEdit::Silence), &r)
        .unwrap();
    let err = rejected(
        &mut d,
        &r,
        add_transition(4 * BAR + PPQ / 2, TransitionEdit::Silence),
    );
    assert!(!err.to_string().is_empty());
}

#[test]
fn filter_down_adds_an_open_low_pass_and_closes_it_over_the_last_bars() {
    let r = reg();
    let mut d = project(&r);
    let at = 4 * BAR;
    let master = master(d.doc());
    let edit = TransitionEdit::FilterDown {
        target: master.clone(),
        bars: 2,
    };
    one_step(&mut d, &r, add_transition(at, edit));
    let doc = d.doc();
    let chain = insert_chain(doc, &r, &master);
    let filter = chain.last().unwrap();
    assert_eq!(doc.device(filter).unwrap().type_id, FILTER);
    // A clip over the last two bars with the whole curve in it.
    let clips = lane_clips(doc, filter, CUTOFF_LANE);
    assert_eq!(clips.len(), 1);
    assert_eq!((clips[0].0, clips[0].1), (at - 2 * BAR, 2 * BAR));
    assert!(clips[0].2 >= 5);
    // Open at the start, falling all the way, open again at the boundary.
    let hz = |tick| shown(doc, filter, CUTOFF_LANE, tick).unwrap();
    assert!((hz(at - 2 * BAR) - OPEN_HZ).abs() < 1.0);
    assert!(hz(at - BAR) < OPEN_HZ && hz(at - BAR) > FLOOR_HZ);
    assert!(hz(at - 2) < hz(at - BAR));
    assert!((hz(at - 1) - FLOOR_HZ).abs() < 10.0, "{}", hz(at - 1));
    assert!((hz(at) - OPEN_HZ).abs() < 1.0, "back open at the boundary");
    // It falls by the same share per octave: the middle is the geometric mean.
    let mid = hz(at - BAR);
    let geometric = (OPEN_HZ * FLOOR_HZ).sqrt();
    assert!((mid / geometric - 1.0).abs() < 0.05, "{mid} vs {geometric}");
}

#[test]
fn a_second_filter_down_reuses_the_filter_and_its_lane() {
    let r = reg();
    let mut d = project(&r);
    let master = master(d.doc());
    let edit = |bars| TransitionEdit::FilterDown {
        target: master.clone(),
        bars,
    };
    d.apply(add_transition(4 * BAR, edit(1)), &r).unwrap();
    one_step(&mut d, &r, add_transition(8 * BAR, edit(4)));
    let doc = d.doc();
    let chain = insert_chain(doc, &r, &master);
    assert_eq!(chain.len(), 1, "one filter, not two");
    let clips = lane_clips(doc, &chain[0], CUTOFF_LANE);
    assert_eq!(clips.len(), 2);
    assert_eq!((clips[1].0, clips[1].1), (4 * BAR, 4 * BAR));
}

#[test]
fn filter_down_works_on_a_group_bus_and_refuses_other_strips_and_lengths() {
    let r = reg();
    let mut d = project(&r);
    d.apply(
        Command::Console(ConsoleCommand::CreateVca {
            name: "Drums".into(),
            members: vec![key("channel-2")],
        }),
        &r,
    )
    .unwrap();
    d.apply(
        Command::Console(ConsoleCommand::RouteVcaToNewBus {
            vca: 1,
            name: "Drum bus".into(),
        }),
        &r,
    )
    .unwrap();
    let bus = output_bus(d.doc(), &key("drums")).expect("the drums play into the bus");
    assert_eq!(output_bus(d.doc(), &key("lead")), None);
    one_step(
        &mut d,
        &r,
        add_transition(
            4 * BAR,
            TransitionEdit::FilterDown {
                target: bus.clone(),
                bars: 1,
            },
        ),
    );
    assert_eq!(insert_chain(d.doc(), &r, &bus).len(), 1);
    // A channel is neither the master nor a bus; five bars are too many; bar zero has no room.
    for edit in [
        TransitionEdit::FilterDown {
            target: key("channel"),
            bars: 1,
        },
        TransitionEdit::FilterDown {
            target: bus.clone(),
            bars: 5,
        },
        TransitionEdit::FilterDown {
            target: bus,
            bars: 0,
        },
    ] {
        rejected(&mut d, &r, add_transition(6 * BAR, edit));
    }
    rejected(&mut d, &r, add_transition(0, TransitionEdit::Silence));
}

#[test]
fn a_filter_down_near_the_start_is_kept_inside_the_song() {
    let r = reg();
    let mut d = project(&r);
    let master = master(d.doc());
    d.apply(
        add_transition(
            BAR,
            TransitionEdit::FilterDown {
                target: master.clone(),
                bars: 4,
            },
        ),
        &r,
    )
    .unwrap();
    let doc = d.doc();
    let filter = insert_chain(doc, &r, &master).pop().unwrap();
    let clips = lane_clips(doc, &filter, CUTOFF_LANE);
    assert_eq!((clips[0].0, clips[0].1), (0, BAR), "only one bar before it");
}

/// The notes of the first clip of the track of `device`.
fn notes_of(doc: &RackDoc, device: &str) -> Vec<(i64, i64, u8)> {
    let t = doc.song.track(&key(device)).unwrap();
    match &t.lanes[0].clips[0].content {
        ClipContent::Notes(n) => n.iter().map(|n| (n.tick, n.length, n.key)).collect(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_fill_plays_the_last_beat_twice_at_double_rate() {
    let r = reg();
    let mut d = project(&r);
    let at = 4 * BAR;
    let before = notes_of(d.doc(), "drums");
    let last = at - PPQ;
    assert!(before.contains(&(last, PPQ / 4, 36)));
    one_step(
        &mut d,
        &r,
        add_transition(
            at,
            TransitionEdit::Fill {
                track: key("drums"),
            },
        ),
    );
    let after = notes_of(d.doc(), "drums");
    // The kick of the last beat is twice as fast: at the start, and half a beat later, half as long.
    let in_window: Vec<(i64, i64, u8)> = after
        .iter()
        .copied()
        .filter(|(t, ..)| (last..at).contains(t))
        .collect();
    assert_eq!(
        in_window,
        vec![
            (last, PPQ / 8, 36),
            (last + PPQ / 4, PPQ / 8, 38),
            (last + PPQ / 2, PPQ / 8, 36),
            (last + 3 * PPQ / 4, PPQ / 8, 38),
        ]
    );
    // Everything outside the last beat is as it was.
    let outside = |v: &[(i64, i64, u8)]| -> Vec<(i64, i64, u8)> {
        v.iter()
            .copied()
            .filter(|(t, ..)| !(last..at).contains(t))
            .collect()
    };
    assert_eq!(outside(&before), outside(&after));
}

#[test]
fn a_fill_is_refused_where_it_has_nothing_to_play() {
    let r = reg();
    let mut d = project(&r);
    // The clips end at bar 9: nothing plays in the beat before bar 12.
    rejected(
        &mut d,
        &r,
        add_transition(
            11 * BAR,
            TransitionEdit::Fill {
                track: key("drums"),
            },
        ),
    );
    // A track that is not there.
    rejected(
        &mut d,
        &r,
        add_transition(
            4 * BAR,
            TransitionEdit::Fill {
                track: key("nothing"),
            },
        ),
    );
}

#[test]
fn a_fill_leaves_a_looped_or_muted_clip_alone() {
    let r = reg();
    let mut d = Document::new(atomik_model::empty_doc());
    d.apply(add(0, DRUMS, Some("drums")), &r).unwrap();
    d.apply(add_track("drums"), &r).unwrap();
    let mut looped = notes_clip(0, 4 * BAR, vec![note(PPQ, PPQ / 4, 36, 0.9)]);
    looped.loop_length = Some(BAR);
    d.apply(add_clip("drums", 0, looped), &r).unwrap();
    assert!(fill_tracks(d.doc(), 2 * BAR).is_empty());
    rejected(
        &mut d,
        &r,
        add_transition(
            2 * BAR,
            TransitionEdit::Fill {
                track: key("drums"),
            },
        ),
    );
}

#[test]
fn the_fill_tracks_are_the_ones_that_play_through_the_last_beat_drums_first() {
    let r = reg();
    let d = project(&r);
    // The lead plays on the first beat of a bar only: the beat before the end of bar 4 is the
    // drums', the beat that starts bar 6 is both.
    assert_eq!(fill_tracks(d.doc(), 4 * BAR), vec![key("drums")]);
    assert_eq!(
        fill_tracks(d.doc(), 5 * BAR + PPQ),
        vec![key("drums"), key("lead")]
    );
}

#[test]
fn the_first_hit_belongs_to_the_earliest_track_and_drums_win_a_tie() {
    let r = reg();
    let mut d = project(&r);
    // Both play on the downbeat of bar 3: the drum machine wins.
    let (track, tick) = first_hit(d.doc(), 2 * BAR).unwrap();
    assert_eq!((track, tick), (key("drums"), 2 * BAR));
    // A section where only the lead starts on the downbeat's beat.
    let mut e = Document::new(atomik_model::empty_doc());
    e.apply(add(0, SYNTH, Some("pad")), &r).unwrap();
    e.apply(add_track("pad"), &r).unwrap();
    e.apply(
        add_clip(
            "pad",
            0,
            notes_clip(0, 4 * BAR, vec![note(BAR + PPQ / 2, PPQ / 4, 60, 0.8)]),
        ),
        &r,
    )
    .unwrap();
    assert_eq!(
        first_hit(e.doc(), BAR).unwrap(),
        (key("pad"), BAR + PPQ / 2)
    );
    // Nothing starts in the first beat of a later section.
    assert!(first_hit(e.doc(), 3 * BAR).is_none());
    // The drums lead the first beat of every bar.
    d.apply(add_transition(BAR, TransitionEdit::Silence), &r)
        .unwrap();
    assert_eq!(first_hit(d.doc(), BAR).unwrap().0, key("drums"));
}

#[test]
fn a_riser_lasts_the_bars_before_the_boundary_at_the_tempo_there() {
    let r = reg();
    let d = project(&r);
    // 120 BPM, 4/4: a bar is two seconds.
    let doc = d.doc();
    assert!((riser_seconds(doc, 4 * BAR, 1) - 2.0).abs() < 0.01);
    assert!((riser_seconds(doc, 4 * BAR, 2) - 4.0).abs() < 0.01);
    assert!((riser_seconds(doc, 4 * BAR, 4) - 8.0).abs() < 0.01);
    // Only one bar exists before bar 2.
    assert!((riser_seconds(doc, BAR, 4) - 2.0).abs() < 0.01);
    assert!((riser_seconds(doc, 100 * BAR, 4) - 8.0).abs() < 0.01);
}

fn marker(id: u32, tick: i64) -> Marker {
    Marker {
        id,
        tick,
        name: format!("S{id}"),
        color: None,
    }
}

#[test]
fn the_boundaries_are_the_marker_ticks_after_the_start() {
    let r = reg();
    let mut d = project(&r);
    d.apply(
        Command::Song(SongCommand::SetMarkers(vec![
            marker(1, 0),
            marker(2, 2 * BAR),
            marker(3, 6 * BAR),
        ])),
        &r,
    )
    .unwrap();
    let song = &d.doc().song;
    assert_eq!(boundaries(song), vec![2 * BAR, 6 * BAR]);
    assert_eq!(nearest_boundary(song, 3 * BAR), Some(6 * BAR));
    assert_eq!(nearest_boundary(song, 2 * BAR), Some(2 * BAR));
    assert_eq!(
        nearest_boundary(song, 7 * BAR),
        Some(6 * BAR),
        "past the last one: the last"
    );
}

#[test]
fn a_kind_on_most_boundaries_is_overuse_and_a_mix_is_not() {
    let r = reg();
    let mut d = project(&r);
    d.apply(
        Command::Song(SongCommand::SetMarkers(
            (1..=4)
                .map(|i| marker(i, i64::from(i as i32) * BAR))
                .collect(),
        )),
        &r,
    )
    .unwrap();
    let song = &d.doc().song;
    let uses = |kinds: &[(i64, TransitionKind)]| overuse(song, kinds, kinds[0].1);
    let riser = TransitionKind::Riser;
    // Two of four is half: not most. Three of four is.
    assert_eq!(uses(&[(BAR, riser), (2 * BAR, riser)]), None);
    let three = [(BAR, riser), (2 * BAR, riser), (3 * BAR, riser)];
    let found = uses(&three).expect("three of four");
    assert_eq!((found.used, found.of), (3, 4));
    assert!(found.message().contains("Riser is on 3 of 4 boundaries"));
    // The same boundary twice counts once; another kind does not count.
    let mixed = [
        (BAR, riser),
        (BAR, riser),
        (2 * BAR, riser),
        (3 * BAR, TransitionKind::Impact),
        (4 * BAR, TransitionKind::Fill),
    ];
    assert_eq!(uses(&mixed), None);
    const { assert!(MIN_USES >= 3) };
    // Few boundaries never warn: two of two is too few uses.
    let mut e = project(&r);
    e.apply(
        Command::Song(SongCommand::SetMarkers(vec![
            marker(1, BAR),
            marker(2, 2 * BAR),
        ])),
        &r,
    )
    .unwrap();
    let two = [(BAR, riser), (2 * BAR, riser)];
    assert_eq!(overuse(&e.doc().song, &two, riser), None);
}

#[test]
fn silence_without_a_master_is_refused() {
    let r = reg();
    // No console, no master.
    let mut d = Document::new(atomik_model::empty_doc());
    let err = rejected(&mut d, &r, add_transition(BAR, TransitionEdit::Silence));
    assert!(err.to_string().contains("master"), "{err}");
}

#[test]
fn the_curve_of_a_filter_down_is_made_of_linear_segments() {
    let r = reg();
    let mut d = project(&r);
    let master = master(d.doc());
    d.apply(
        add_transition(
            4 * BAR,
            TransitionEdit::FilterDown {
                target: master.clone(),
                bars: 1,
            },
        ),
        &r,
    )
    .unwrap();
    let doc = d.doc();
    let filter = insert_chain(doc, &r, &master).pop().unwrap();
    let t = doc.song.track(&filter).unwrap();
    let clip = &t.lanes[t.automation_lane(CUTOFF_LANE).unwrap()].clips[0];
    let ClipContent::Automation(points) = &clip.content else {
        panic!()
    };
    assert!(points.iter().all(|p| p.curve == Curve::Linear));
    assert!(points.windows(2).all(|w| w[0].tick < w[1].tick));
    assert!(points.windows(2).all(|w| w[0].value > w[1].value));
    assert_eq!(points[0].tick, 0);
    assert_eq!(points.last().unwrap().tick, clip.length);
    assert!(doc.validate().is_ok());
}
