//! The piano roll and the song's key and chords: a clip without a scale follows the song key at its
//! start (scale highlighting, snap to scale and the fold use it), a clip's own scale wins, a key
//! change moves what the clip follows, the "song chord" stamp drops the chord of the lane at the
//! place being drawn, and "Fit Notes to Chords" moves the selected notes onto the chord tones in
//! one undo step.

mod support;

use atomik_device_sdk::Registry;
use atomik_model::harmony::{ChordEvent, Harmony, Key, KeyChange};
use atomik_model::theory::{ChordKind, Scale, ScaleKind};
use atomik_model::{ClipContent, ClipId, Command, Document, Note, SongCommand};
use atomik_ui::chord_edit::{Stamp, lane_chord_keys};
use atomik_ui::key_rows::Fold;
use atomik_ui::keymap::CommandOutcome;
use atomik_ui::kurbo::Point;
use atomik_ui::{Key as UiKey, Modifiers, PianoRollView, PointerButton, Tool, UiAction, UiEvent};
use support::song::*;
use support::*;

fn a_minor() -> Key {
    Key::new(9, ScaleKind::NaturalMinor)
}

fn setup(harmony: Harmony, notes: Vec<Note>) -> (PianoRollView, Document, Registry) {
    let reg = registry();
    let mut doc = song_doc();
    doc.song_mut().tracks[0].lanes[0].clips[0].content = ClipContent::Notes(notes.into());
    doc.song_mut().harmony = harmony;
    let d = Document::new(doc);
    let v = piano_roll(d.doc(), &reg);
    (v, d, reg)
}

fn set_harmony(v: &mut PianoRollView, d: &mut Document, reg: &Registry, h: Harmony) {
    d.apply(Command::Song(SongCommand::SetHarmony(h)), reg)
        .unwrap();
    v.sync(d.doc(), reg);
}

fn apply(v: &mut PianoRollView, d: &mut Document, reg: &Registry, actions: &[UiAction]) {
    for c in song_commands(actions) {
        d.apply(Command::Song(c), reg).unwrap();
    }
    v.sync(d.doc(), reg);
}

fn notes_of(d: &Document) -> Vec<Note> {
    match &d.doc().song.clip(ClipId(1)).unwrap().2.content {
        ClipContent::Notes(n) => n.to_vec(),
        other => panic!("not notes: {other:?}"),
    }
}

fn keys(notes: &[Note]) -> Vec<u8> {
    notes.iter().map(|n| n.key).collect()
}

fn n(tick: i64, key: u8) -> Note {
    Note::new(tick, PPQ, key, 0.8)
}

fn at(v: &PianoRollView, tick: i64, key: u8) -> Point {
    Point::new(v.tick_to_x(tick), v.key_center_y(key))
}

fn click_draw(v: &mut PianoRollView, d: &mut Document, reg: &Registry, tick: i64, key: u8) {
    let p = at(v, tick + 100, key);
    let a = v.handle(&down(p), d.doc(), reg);
    apply(v, d, reg, &a);
    let a = v.handle(&up(p), d.doc(), reg);
    apply(v, d, reg, &a);
}

fn run_cmd(v: &mut PianoRollView, d: &mut Document, reg: &Registry, id: &str) -> CommandOutcome {
    let out = v.command(id, None, d.doc(), reg);
    if let CommandOutcome::Handled(actions) = &out {
        apply(v, d, reg, actions);
    }
    out
}

fn chord(bar: i64, bars: i64, root: u8, quality: ChordKind) -> ChordEvent {
    ChordEvent::new(bar * BAR, bars * BAR, root, quality)
}

// ---------------------------------------------------------------- following the key

#[test]
fn a_clip_without_a_scale_follows_the_song_key() {
    let (v, _d, _reg) = setup(
        Harmony {
            key: Some(a_minor()),
            ..Harmony::default()
        },
        Vec::new(),
    );
    assert_eq!(v.scale(), Some(Scale::new(9, ScaleKind::NaturalMinor)));
    assert!(v.follows_song_key());
    assert_eq!(v.tools().scale, v.scale(), "the menus show it too");
}

#[test]
fn a_song_without_a_key_leaves_the_clip_chromatic() {
    let (v, _d, _reg) = setup(Harmony::default(), Vec::new());
    assert_eq!(v.scale(), None);
    assert!(!v.follows_song_key());
}

#[test]
fn the_clips_own_scale_wins() {
    let (mut v, mut d, reg) = setup(
        Harmony {
            key: Some(a_minor()),
            ..Harmony::default()
        },
        Vec::new(),
    );
    let dorian = Scale::new(2, ScaleKind::Dorian);
    d.apply(
        Command::Song(SongCommand::SetClipScale {
            clip: ClipId(1),
            scale: Some(dorian),
        }),
        &reg,
    )
    .unwrap();
    v.sync(d.doc(), &reg);
    assert_eq!(v.scale(), Some(dorian));
    assert!(!v.follows_song_key());
    // Clearing it goes back to following the song.
    d.apply(
        Command::Song(SongCommand::SetClipScale {
            clip: ClipId(1),
            scale: None,
        }),
        &reg,
    )
    .unwrap();
    v.sync(d.doc(), &reg);
    assert_eq!(v.scale(), Some(Scale::new(9, ScaleKind::NaturalMinor)));
}

#[test]
fn the_key_at_the_start_of_the_clip_is_the_one_it_follows() {
    // The clip starts at tick 0: a key change there is the key it follows.
    let (mut v, mut d, reg) = setup(
        Harmony {
            key: Some(a_minor()),
            ..Harmony::default()
        },
        Vec::new(),
    );
    let h = Harmony {
        key: Some(a_minor()),
        changes: vec![KeyChange {
            tick: 0,
            key: Key::new(0, ScaleKind::Major),
        }],
        chords: Vec::new(),
    };
    set_harmony(&mut v, &mut d, &reg, h);
    assert_eq!(v.scale(), Some(Scale::new(0, ScaleKind::Major)));
    // A change after the clip's start does not move what it follows.
    let h = Harmony {
        key: Some(a_minor()),
        changes: vec![KeyChange {
            tick: 8 * BAR,
            key: Key::new(0, ScaleKind::Major),
        }],
        chords: Vec::new(),
    };
    set_harmony(&mut v, &mut d, &reg, h);
    assert_eq!(v.scale(), Some(Scale::new(9, ScaleKind::NaturalMinor)));
}

#[test]
fn a_clip_that_moves_follows_the_key_at_its_new_start() {
    let h = Harmony {
        key: Some(a_minor()),
        changes: vec![KeyChange {
            tick: 8 * BAR,
            key: Key::new(7, ScaleKind::Mixolydian),
        }],
        chords: Vec::new(),
    };
    let (mut v, mut d, reg) = setup(h, Vec::new());
    assert_eq!(v.scale().map(|s| s.root), Some(9));
    d.apply(
        Command::Song(SongCommand::SetClipGeometry {
            id: ClipId(1),
            start: 8 * BAR,
            length: 4 * BAR,
            offset: 0,
            loop_length: None,
            muted: false,
            merge: None,
        }),
        &reg,
    )
    .unwrap();
    v.sync(d.doc(), &reg);
    assert_eq!(v.scale(), Some(Scale::new(7, ScaleKind::Mixolydian)));
}

#[test]
fn snap_to_scale_and_the_fold_use_the_song_key() {
    let (mut v, mut d, reg) = setup(
        Harmony {
            key: Some(Key::new(0, ScaleKind::Major)),
            ..Harmony::default()
        },
        Vec::new(),
    );
    v.set_tool(Tool::Pencil);
    v.set_snap_to_scale(true);
    click_draw(&mut v, &mut d, &reg, 0, 61);
    click_draw(&mut v, &mut d, &reg, 4 * PPQ, 66);
    assert_eq!(keys(&notes_of(&d)), [60, 65], "C# and F# land on C and F");
    v.set_fold(Fold::Scale);
    assert!(
        v.rows().len() < 128,
        "the fold hides the keys outside C major"
    );
    // A clip scale of its own is the one snapped to.
    d.apply(
        Command::Song(SongCommand::SetClipScale {
            clip: ClipId(1),
            scale: Some(Scale::new(1, ScaleKind::Major)),
        }),
        &reg,
    )
    .unwrap();
    v.sync(d.doc(), &reg);
    click_draw(&mut v, &mut d, &reg, 8 * PPQ, 61);
    assert_eq!(notes_of(&d).last().unwrap().key, 61, "C# is in C# major");
}

#[test]
fn a_clip_that_opted_out_is_not_highlighted_or_snapped_by_the_song_key() {
    let (mut v, mut d, reg) = setup(
        Harmony {
            key: Some(Key::new(0, ScaleKind::Major)),
            ..Harmony::default()
        },
        Vec::new(),
    );
    assert!(v.clip_follows_key());
    assert!(v.tools().follow_key);
    let out = run_cmd(&mut v, &mut d, &reg, "editor.follow_song_key");
    assert!(matches!(out, CommandOutcome::Handled(_)));
    assert!(!v.clip_follows_key());
    assert!(!v.tools().follow_key, "the menu shows it off");
    assert_eq!(v.scale(), None, "nothing is highlighted");
    assert!(!v.follows_song_key());
    v.set_tool(Tool::Pencil);
    v.set_snap_to_scale(true);
    click_draw(&mut v, &mut d, &reg, 0, 61);
    assert_eq!(keys(&notes_of(&d)), [61], "C# stays where it was drawn");
    v.set_fold(Fold::Scale);
    assert_eq!(v.rows().len(), 128, "the fold has no scale to use");
    // A scale of its own still counts, and the toggle brings the song key back.
    run_cmd(&mut v, &mut d, &reg, "editor.follow_song_key");
    assert!(v.clip_follows_key());
    assert_eq!(v.scale(), Some(Scale::new(0, ScaleKind::Major)));
    d.undo().unwrap();
    v.sync(d.doc(), &reg);
    assert!(!v.clip_follows_key(), "one undo step per toggle");
}

#[test]
fn an_opted_out_clip_keeps_a_scale_of_its_own() {
    let (mut v, mut d, reg) = setup(
        Harmony {
            key: Some(a_minor()),
            ..Harmony::default()
        },
        Vec::new(),
    );
    let dorian = Scale::new(2, ScaleKind::Dorian);
    d.apply(
        Command::Song(SongCommand::SetClipScale {
            clip: ClipId(1),
            scale: Some(dorian),
        }),
        &reg,
    )
    .unwrap();
    run_cmd(&mut v, &mut d, &reg, "editor.follow_song_key");
    assert_eq!(v.scale(), Some(dorian));
}

// ---------------------------------------------------------------- the song chord stamp

#[test]
fn the_song_chord_stamp_drops_the_chord_of_the_lane_at_the_place_drawn() {
    let h = Harmony {
        chords: vec![
            chord(0, 1, 9, ChordKind::Minor),
            chord(1, 1, 5, ChordKind::Major),
        ],
        ..Harmony::default()
    };
    let (mut v, mut d, reg) = setup(h, Vec::new());
    v.set_tool(Tool::Pencil);
    v.set_stamp(Stamp::Lane);
    click_draw(&mut v, &mut d, &reg, 0, 57);
    click_draw(&mut v, &mut d, &reg, BAR, 60);
    let notes = notes_of(&d);
    let first: Vec<u8> = notes
        .iter()
        .filter(|x| x.tick < BAR)
        .map(|x| x.key)
        .collect();
    let second: Vec<u8> = notes
        .iter()
        .filter(|x| x.tick >= BAR)
        .map(|x| x.key)
        .collect();
    assert_eq!(first, [57, 60, 64], "A minor on A3");
    assert_eq!(
        second,
        [65, 69, 72],
        "F major from the F nearest the key pressed"
    );
    // One undo step per chord.
    d.undo().unwrap();
    assert_eq!(notes_of(&d).len(), 3);
}

#[test]
fn the_song_chord_stamp_is_a_single_note_where_the_lane_has_no_chord() {
    let (mut v, mut d, reg) = setup(Harmony::default(), Vec::new());
    v.set_tool(Tool::Pencil);
    v.set_stamp(Stamp::Lane);
    click_draw(&mut v, &mut d, &reg, 0, 62);
    assert_eq!(keys(&notes_of(&d)), [62]);
}

#[test]
fn the_song_chord_stamp_is_in_the_menu_ids_and_cannot_chordify() {
    assert_eq!(Stamp::from_id("lane"), Some(Stamp::Lane));
    assert!(Stamp::all().contains(&Stamp::Lane));
    assert_eq!(Stamp::Lane.label(), "Song Chord");
    assert!(Stamp::Lane.is_active(None));
    assert!(!Stamp::Lane.chordifies(None));
    assert!(Stamp::Lane.keys(60, None, 0).is_empty());
}

#[test]
fn a_slash_chord_puts_its_bass_below_the_chord() {
    let mut c = chord(0, 1, 0, ChordKind::Major);
    c.bass = Some(7);
    assert_eq!(lane_chord_keys(&c, 60, 0), [55, 60, 64, 67]);
    c.bass = Some(4);
    assert_eq!(
        lane_chord_keys(&c, 60, 0),
        [52, 60, 64, 67],
        "E below, not doubled"
    );
    let plain = chord(0, 1, 0, ChordKind::Minor);
    assert_eq!(
        lane_chord_keys(&plain, 66, 0),
        [60, 63, 67],
        "the root nearest the key pressed, a tie going down"
    );
    assert_eq!(lane_chord_keys(&plain, 70, 0), [72, 75, 79]);
    assert_eq!(
        lane_chord_keys(&plain, 70, 1),
        [75, 79, 84],
        "first inversion"
    );
}

// ---------------------------------------------------------------- Fit Notes to Chords

#[test]
fn fit_notes_to_chords_moves_the_selection_onto_chord_tones_in_one_undo_step() {
    let h = Harmony {
        chords: vec![
            chord(0, 1, 0, ChordKind::Major),
            chord(1, 1, 9, ChordKind::Minor),
        ],
        ..Harmony::default()
    };
    let notes = vec![n(0, 61), n(PPQ, 66), n(BAR, 62), n(BAR + PPQ, 66)];
    let (mut v, mut d, reg) = setup(h, notes);
    v.set_selected_notes(&[0, 1, 2]);
    let out = run_cmd(&mut v, &mut d, &reg, "editor.fit_to_chords");
    assert!(matches!(out, CommandOutcome::Handled(_)));
    // The result is a preview first: the notes have not moved.
    assert!(v.fit_preview_shown());
    assert_eq!(keys(&notes_of(&d)), [61, 66, 62, 66]);
    // C# -> C, F# -> G (C major); D -> C (nearest A minor tone, a tie goes down); the fourth note
    // was not selected.
    let preview = v.fit_preview_notes().unwrap();
    assert_eq!(keys(&preview), [60, 67, 60, 66]);
    // Apply, from the bar's button.
    let (apply_btn, _) = v.fit_buttons().unwrap();
    let a = v.handle(
        &UiEvent::PointerDown {
            pos: apply_btn.center(),
            button: PointerButton::Primary,
            modifiers: Modifiers::NONE,
            clicks: 1,
        },
        d.doc(),
        &reg,
    );
    apply(&mut v, &mut d, &reg, &a);
    assert!(!v.fit_preview_shown());
    assert_eq!(keys(&notes_of(&d)), [60, 67, 60, 66]);
    d.undo().unwrap();
    assert_eq!(keys(&notes_of(&d)), [61, 66, 62, 66], "one undo step");
}

#[test]
fn cancelling_the_fit_preview_changes_nothing() {
    let h = Harmony {
        chords: vec![chord(0, 1, 0, ChordKind::Major)],
        ..Harmony::default()
    };
    let (mut v, mut d, reg) = setup(h, vec![n(0, 61)]);
    v.set_selected_notes(&[0]);
    run_cmd(&mut v, &mut d, &reg, "editor.fit_to_chords");
    assert!(v.fit_preview_shown());
    let undo_before = d.undo_label().map(str::to_owned);
    let a = v.handle(
        &UiEvent::Key {
            key: UiKey::Escape,
            modifiers: Modifiers::NONE,
            pressed: true,
        },
        d.doc(),
        &reg,
    );
    apply(&mut v, &mut d, &reg, &a);
    assert!(!v.fit_preview_shown());
    assert_eq!(keys(&notes_of(&d)), [61]);
    assert_eq!(
        d.undo_label().map(str::to_owned),
        undo_before,
        "no undo step"
    );
    // Enter applies a new preview.
    run_cmd(&mut v, &mut d, &reg, "editor.fit_to_chords");
    let a = v.handle(
        &UiEvent::Key {
            key: UiKey::Enter,
            modifiers: Modifiers::NONE,
            pressed: true,
        },
        d.doc(),
        &reg,
    );
    apply(&mut v, &mut d, &reg, &a);
    assert_eq!(keys(&notes_of(&d)), [60]);
}

#[test]
fn fit_notes_to_chords_without_a_selection_is_declined_and_without_chords_changes_nothing() {
    let notes = vec![n(0, 61)];
    let (mut v, mut d, reg) = setup(Harmony::default(), notes.clone());
    assert!(matches!(
        v.command("editor.fit_to_chords", None, d.doc(), &reg),
        CommandOutcome::Declined
    ));
    v.set_selected_notes(&[0]);
    let out = run_cmd(&mut v, &mut d, &reg, "editor.fit_to_chords");
    let CommandOutcome::Handled(actions) = out else {
        panic!("not handled")
    };
    assert!(song_commands(&actions).is_empty(), "no chord, no change");
    assert_eq!(keys(&notes_of(&d)), [61]);
}

#[test]
fn fit_notes_to_chords_follows_the_clip_position_in_the_song() {
    // The clip starts at tick 0 with its content offset by a bar: content tick 0 is bar 2 of the
    // song only for a clip starting there; here the clip starts at 0, so content tick BAR meets the
    // second chord.
    let h = Harmony {
        chords: vec![chord(1, 1, 7, ChordKind::Major)],
        ..Harmony::default()
    };
    let (mut v, mut d, reg) = setup(h, vec![n(0, 61), n(BAR, 61)]);
    v.set_selected_notes(&[0, 1]);
    run_cmd(&mut v, &mut d, &reg, "editor.fit_to_chords");
    let a = v.apply_fit_preview();
    apply(&mut v, &mut d, &reg, &a);
    // Only the note under the G major chord (B, D, G) moves: C# goes to D.
    assert_eq!(keys(&notes_of(&d)), [61, 62]);
}

// ---------------------------------------------------------------- new clips

#[test]
fn a_note_clip_drawn_on_an_empty_part_of_a_track_has_no_scale_and_follows_the_song() {
    use atomik_ui::ArrangementView;
    use atomik_ui::timeline::Snap;
    let reg = registry();
    let mut doc = song_doc();
    doc.song_mut().harmony = Harmony {
        key: Some(a_minor()),
        ..Harmony::default()
    };
    let mut d = Document::new(doc);
    let mut arr: ArrangementView = arrangement(d.doc(), &reg);
    arr.set_tool(Tool::Pencil);
    arr.set_snap(Snap::Bar);
    let y = arr.lane_rect(&key("lead"), 0).unwrap().center().y;
    let (x0, x1) = (arr.tick_to_x(5 * BAR + 10), arr.tick_to_x(6 * BAR + 10));
    let mut actions = Vec::new();
    for e in [
        down(Point::new(x0, y)),
        mv(Point::new(x1, y)),
        up(Point::new(x1, y)),
    ] {
        actions.extend(arr.handle(&e, d.doc(), &reg));
    }
    let cmds = song_commands(&actions);
    let atomik_model::SongCommand::AddClip { clip, .. } = &cmds[0] else {
        panic!("{cmds:?}")
    };
    assert_eq!(clip.scale, None, "a new clip has no scale of its own");
    for c in cmds {
        d.apply(Command::Song(c), &reg).unwrap();
    }
    let new_id = d.doc().song.track(&key("lead")).unwrap().lanes[0]
        .clips
        .iter()
        .find(|c| c.start == 5 * BAR)
        .map(|c| c.id)
        .expect("the new clip");
    let mut roll = PianoRollView::new(atomik_ui::Theme::dark());
    roll.set_bounds(ROLL_BOUNDS);
    roll.open(new_id);
    roll.sync(d.doc(), &reg);
    assert_eq!(roll.scale(), Some(Scale::new(9, ScaleKind::NaturalMinor)));
    assert!(roll.follows_song_key());
}
