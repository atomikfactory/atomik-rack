//! The document revision in the app core: `is_dirty` is the structural compare computed once
//! per change, and the views read the document again only when it changed.

use super::*;
use crate::sink::RecordingSink;
use atomik_core::PPQ;
use atomik_model::{Clip, ClipContent, ClipId, DeviceKey, Note, SongCommand, empty_doc};
use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{DisplayList, Modifiers};

const SYNTH: &str = "atomik.poly-synth";
const BAR: i64 = 4 * PPQ;

fn key(k: &str) -> DeviceKey {
    DeviceKey(k.to_string())
}

fn core() -> AppCore<RecordingSink> {
    AppCore::with_registry(RecordingSink::new(), default_registry(), empty_doc(), None)
}

/// A seeded xorshift.
#[derive(Clone)]
struct Rng(u32);

impl Rng {
    fn next(&mut self, n: u32) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0 % n
    }
}

fn note_clip(start: i64, notes: &[(i64, u8)]) -> Clip {
    Clip {
        id: ClipId(0),
        start,
        length: BAR,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        controllers: Vec::new(),
        grid: None,
        content: ClipContent::Notes(
            notes
                .iter()
                .map(|&(tick, k)| Note::new(tick, PPQ / 2, k, 0.8))
                .collect(),
        ),
    }
}

/// Synth `a` with a track and one note clip (id 1).
fn seed(c: &mut AppCore<RecordingSink>) {
    c.apply_command(Command::AddDevice {
        index: usize::MAX,
        type_id: SYNTH.to_string(),
        key: Some(key("a")),
    })
    .unwrap();
    c.apply_command(Command::Song(SongCommand::AddTrack { device: key("a") }))
        .unwrap();
    c.apply_command(Command::Song(SongCommand::AddClip {
        device: key("a"),
        lane: 0,
        clip: note_clip(0, &[(0, 60), (PPQ, 62)]),
    }))
    .unwrap();
}

/// One random step: a command, an undo or a redo, a save, or a new document.
fn step(c: &mut AppCore<RecordingSink>, rng: &mut Rng, steps: u32) {
    match rng.next(12) {
        0..=3 => {
            let cutoff = [200.0, 500.0, 1000.0, 4000.0][rng.next(4) as usize];
            let command = Command::SetParam {
                key: key("a"),
                param: "filter.cutoff".into(),
                value: cutoff,
                merge: None,
            };
            if rng.next(2) == 0 {
                let _ = c.apply_command(command);
            } else {
                // Behind the views: only the next event reads it.
                let _ = c.document.apply(command, &c.registry);
            }
        }
        4 => {
            let at = i64::from(rng.next(8)) * BAR;
            let _ = c.apply_command(Command::Song(SongCommand::AddClip {
                device: key("a"),
                lane: 0,
                clip: note_clip(at, &[(0, 60 + rng.next(12) as u8)]),
            }));
        }
        5 => {
            let _ = c.apply_command(Command::AddDevice {
                index: usize::MAX,
                type_id: SYNTH.to_string(),
                key: Some(key(&format!("d{steps}"))),
            });
        }
        6 | 7 => {
            // New notes in clip 1 (an undo step of its own).
            let key = 40 + rng.next(40) as u8;
            let command = Command::Song(SongCommand::SetClipContent {
                id: ClipId(1),
                content: ClipContent::Notes(vec![Note::new(0, PPQ / 2, key, 0.7)].into()),
                merge: None,
            });
            if rng.next(2) == 0 {
                let _ = c.apply_command(command);
            } else {
                let _ = c.document.apply(command, &c.registry);
            }
        }
        8 => {
            if rng.next(2) == 0 {
                c.undo();
            } else {
                let _ = c.document.undo(&c.registry);
            }
        }
        9 => {
            if rng.next(2) == 0 {
                c.redo();
            } else {
                let _ = c.document.redo(&c.registry);
            }
        }
        10 => c.set_saved(c.doc().clone()),
        _ => {
            if rng.next(6) == 0 {
                c.new_document();
                seed(c);
            }
        }
    }
}

#[test]
fn the_dirty_flag_equals_the_structural_compare_in_random_sequences() {
    let mut c = core();
    let mut rng = Rng(0x2545_f491);
    let mut cleans = 0;
    for seq in 0..200 {
        c.new_document();
        seed(&mut c);
        for n in 0..50 {
            step(&mut c, &mut rng, seq * 50 + n);
            let reference = c.doc() != &c.saved;
            assert_eq!(c.is_dirty(), reference, "sequence {seq} step {n}");
            // Asked twice, the second time from the memo: the same answer.
            assert_eq!(c.is_dirty(), reference);
            cleans += usize::from(!reference);
        }
    }
    assert!(cleans > 1000, "the sequences reach the saved state often");
}

#[test]
fn undoing_back_to_the_saved_state_is_clean_and_a_new_save_point_is_clean() {
    let mut c = core();
    seed(&mut c);
    c.set_saved(c.doc().clone());
    assert!(!c.is_dirty());
    c.apply_command(Command::SetParam {
        key: key("a"),
        param: "filter.cutoff".into(),
        value: 500.0,
        merge: None,
    })
    .unwrap();
    assert!(c.is_dirty());
    assert!(c.title().ends_with(" *"));
    assert!(c.undo());
    assert!(!c.is_dirty(), "the compare is kept, not a bare counter");
    assert!(c.redo());
    assert!(c.is_dirty());
    c.set_saved(c.doc().clone());
    assert!(!c.is_dirty());
    assert!(!c.title().ends_with(" *"));
}

fn pointer_move() -> UiEvent {
    UiEvent::PointerMove {
        pos: Point::new(20.0, 20.0),
        modifiers: Modifiers::NONE,
    }
}

const PANES: [Pane; 5] = [
    Pane::Rack,
    Pane::Transport,
    Pane::Arrangement,
    Pane::PianoRoll,
    Pane::Console,
];

fn paint_all(c: &mut AppCore<RecordingSink>) -> Vec<DisplayList> {
    PANES
        .iter()
        .map(|&pane| {
            let mut list = DisplayList::new();
            c.paint_pane(pane, &mut list);
            list
        })
        .collect()
}

fn lay_out(c: &mut AppCore<RecordingSink>) {
    c.transport_bar_mut()
        .set_bounds(Rect::new(0.0, 0.0, 1200.0, 60.0));
    c.arrangement_mut()
        .set_bounds(Rect::new(0.0, 60.0, 1200.0, 400.0));
    c.piano_roll_mut()
        .set_bounds(Rect::new(0.0, 400.0, 1200.0, 700.0));
    c.console_mut()
        .set_bounds(Rect::new(0.0, 700.0, 1200.0, 1000.0));
}

fn start(c: &mut AppCore<RecordingSink>) {
    seed(c);
    lay_out(c);
    c.open_piano_roll(ClipId(1));
}

/// The notes of clip 1 in the document (none when it is gone).
fn open_clip_notes(c: &AppCore<RecordingSink>) -> Vec<Note> {
    match c.doc().song.clip(ClipId(1)) {
        Some((_, _, clip)) => match &clip.content {
            ClipContent::Notes(notes) => notes.to_vec(),
            _ => Vec::new(),
        },
        None => Vec::new(),
    }
}

#[test]
fn gated_view_syncs_equal_always_sync_on_a_random_command_stream() {
    let (mut gated, mut forced) = (core(), core());
    start(&mut gated);
    start(&mut forced);
    let mut rng = Rng(0x9e37_79b9);
    for n in 0..400 {
        // The same step on both: a copy of the generator keeps them in step.
        let mut again = rng.clone();
        step(&mut gated, &mut rng, n);
        step(&mut forced, &mut again, n);
        assert_eq!(gated.doc(), forced.doc(), "the cores diverged at step {n}");
        // The gated core only hears one pointer move, over a rotating pane; the reference
        // reads every view after it. What the other views hold is compared before any of
        // them paints (a view that is handed the document reads it itself).
        let pane = PANES[n as usize % PANES.len()];
        let _ = gated.handle_pane_event(pane, &pointer_move());
        let _ = forced.handle_pane_event(pane, &pointer_move());
        forced.after_view_change();
        assert_eq!(
            gated.piano_roll().notes(),
            forced.piano_roll().notes(),
            "the open clip differs after step {n}"
        );
        assert_eq!(gated.piano_roll().clip(), forced.piano_roll().clip());
        assert_eq!(
            gated.console().strip_keys(),
            forced.console().strip_keys(),
            "the strips differ after step {n}"
        );
        if gated.piano_roll().clip().is_some() {
            assert_eq!(
                gated.piano_roll().notes(),
                open_clip_notes(&gated).as_slice(),
                "the open clip is not the document's after step {n}"
            );
        }
        let (a, b) = (paint_all(&mut gated), paint_all(&mut forced));
        for (i, (x, y)) in a.iter().zip(&b).enumerate() {
            assert!(x == y, "pane {:?} differs after step {n}", PANES[i]);
        }
    }
}

#[test]
fn an_idle_move_after_a_change_reads_the_document_and_a_second_one_does_not() {
    let mut c = core();
    start(&mut c);
    let _ = c.handle_pane_event(Pane::Arrangement, &pointer_move());
    assert_eq!(c.views_synced_at, Some(c.document.revision()));
    // The document changes behind the views (no `after_view_change`): the next move reads it.
    c.document
        .apply(
            Command::Song(SongCommand::AddClip {
                device: key("a"),
                lane: 0,
                clip: note_clip(2 * BAR, &[(0, 64)]),
            }),
            &c.registry,
        )
        .unwrap();
    assert_ne!(c.views_synced_at, Some(c.document.revision()));
    let _ = c.handle_pane_event(Pane::Arrangement, &pointer_move());
    assert_eq!(c.views_synced_at, Some(c.document.revision()));
    let seen = c.views_synced_at;
    let _ = c.handle_pane_event(Pane::Arrangement, &pointer_move());
    assert_eq!(c.views_synced_at, seen);
}
