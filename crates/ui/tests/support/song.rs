//! Song fixtures for the sequencer views: four synths and a delay, three tracks (the bass
//! track has a cutoff automation lane), a loop over bars 1–4.

use super::*;
use atomik_model::{
    AutomationPoint, Clip, ClipContent, ClipId, Curve, Lane, LaneKind, Note, Track,
};
use atomik_ui::keymap::{Os, Roles};
use atomik_ui::kurbo::Rect;
use atomik_ui::{ArrangementView, PianoRollView, TransportBar};

pub const PPQ: i64 = 15_360;
pub const BAR: i64 = 4 * PPQ;

pub fn n(tick: i64, length: i64, key: u8, velocity: f32) -> Note {
    Note::new(tick, length, key, velocity)
}

pub fn notes_clip(id: u64, start: i64, length: i64, notes: Vec<Note>) -> Clip {
    Clip {
        id: ClipId(id),
        start,
        length,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        grid: None,
        content: ClipContent::Notes(notes),
    }
}

pub fn auto_clip(id: u64, start: i64, length: i64, points: Vec<AutomationPoint>) -> Clip {
    Clip {
        id: ClipId(id),
        start,
        length,
        offset: 0,
        loop_length: None,
        muted: false,
        scale: None,
        grid: None,
        content: ClipContent::Automation(points),
    }
}

pub fn ap(tick: i64, value: f32, curve: Curve) -> AutomationPoint {
    AutomationPoint { tick, value, curve }
}

/// A 4-bar bass line (one note per beat, walking up), sorted.
pub fn bass_line() -> Vec<Note> {
    let keys = [36u8, 36, 43, 41, 36, 48, 46, 43];
    (0..16)
        .map(|i| {
            n(
                i * PPQ,
                if i % 4 == 3 { PPQ / 2 } else { PPQ - PPQ / 8 },
                keys[(i % 8) as usize],
                0.45 + 0.035 * i as f32,
            )
        })
        .collect()
}

pub fn track(device: &str, lanes: Vec<Lane>) -> Track {
    Track {
        device: key(device),
        muted: false,
        solo: false,
        armed: false,
        lanes,
    }
}

pub fn song_doc() -> RackDoc {
    let mut doc = reference_doc();
    doc.devices = vec![
        device("bass", "atomik.poly-synth", "Bass"),
        device("lead", "atomik.poly-synth", "Lead"),
        device("keys", "atomik.poly-synth", "Keys"),
        device("pad", "atomik.poly-synth", "Pad"),
        device("delay", "atomik.stereo-delay", "Stereo Delay"),
    ];
    doc.devices[1].color = Some([0x5f, 0xb4, 0xe6]);
    doc.devices[2].color = Some([0xc8, 0x7c, 0xe0]);
    doc.cables.clear();
    let mut looped = notes_clip(
        5,
        4 * BAR,
        2 * BAR,
        vec![
            n(0, PPQ / 2, 60, 0.9),
            n(PPQ, PPQ / 2, 63, 0.7),
            n(2 * PPQ, PPQ, 67, 0.8),
        ],
    );
    looped.loop_length = Some(BAR);
    let mut lead = notes_clip(
        2,
        BAR,
        3 * BAR,
        vec![
            n(0, PPQ * 2, 72, 0.8),
            n(2 * PPQ, PPQ, 74, 0.6),
            n(3 * PPQ, PPQ, 76, 0.7),
            n(BAR, 3 * PPQ, 79, 0.9),
        ],
    );
    lead.loop_length = Some(2 * BAR);
    let keys_clip = notes_clip(
        3,
        0,
        4 * BAR,
        vec![
            n(0, 2 * BAR, 60, 0.6),
            n(0, 2 * BAR, 64, 0.6),
            n(0, 2 * BAR, 67, 0.6),
            n(2 * BAR, 2 * BAR, 62, 0.6),
            n(2 * BAR, 2 * BAR, 65, 0.6),
            n(2 * BAR, 2 * BAR, 69, 0.6),
        ],
    );
    let cutoff = auto_clip(
        4,
        0,
        4 * BAR,
        vec![
            ap(0, 300.0, Curve::Linear),
            ap(2 * BAR, 6000.0, Curve::Step),
            ap(3 * BAR, 1200.0, Curve::Linear),
            ap(4 * BAR - PPQ, 2400.0, Curve::Linear),
        ],
    );
    let mut lead_track = track(
        "lead",
        vec![Lane {
            kind: LaneKind::Notes,
            clips: vec![lead],
        }],
    );
    lead_track.muted = true;
    let mut keys_track = track(
        "keys",
        vec![Lane {
            kind: LaneKind::Notes,
            clips: vec![keys_clip],
        }],
    );
    keys_track.solo = true;
    keys_track.armed = true;
    doc.song.tracks = vec![
        track(
            "bass",
            vec![
                Lane {
                    kind: LaneKind::Notes,
                    clips: vec![notes_clip(1, 0, 4 * BAR, bass_line()), looped],
                },
                Lane {
                    kind: LaneKind::Automation("filter.cutoff".into()),
                    clips: vec![cutoff],
                },
            ],
        ),
        lead_track,
        keys_track,
    ];
    doc.song.loop_range = (0, 4 * BAR);
    doc.song.loop_enabled = true;
    doc.song.next_clip_id = 6;
    doc
}

pub const ARR_BOUNDS: Rect = Rect::new(0.0, 0.0, 1200.0, 420.0);

/// A synced arrangement at 96 px per bar, unscrolled.
pub fn arrangement(doc: &RackDoc, reg: &Registry) -> ArrangementView {
    let mut v = ArrangementView::new(Theme::dark());
    // The tests press Ctrl and Alt: pin the role defaults of an OS where those are the roles
    // (on macOS the defaults use Command and Option).
    v.set_roles(Roles::defaults(Os::Windows));
    v.set_bounds(ARR_BOUNDS);
    v.sync(doc, reg);
    v
}

pub const ROLL_BOUNDS: Rect = Rect::new(0.0, 0.0, 1000.0, 560.0);

/// A piano roll open on the 4-bar bass clip (id 1).
pub fn piano_roll(doc: &RackDoc, reg: &Registry) -> PianoRollView {
    let mut v = PianoRollView::new(Theme::dark());
    v.set_bounds(ROLL_BOUNDS);
    v.open(ClipId(1));
    v.sync(doc, reg);
    v
}

pub const BAR_BOUNDS: Rect = Rect::new(0.0, 0.0, 1000.0, 44.0);

pub fn transport_bar(doc: &RackDoc, reg: &Registry) -> TransportBar {
    let mut v = TransportBar::new(Theme::dark());
    v.set_bounds(BAR_BOUNDS);
    v.sync(doc, reg);
    v
}

pub fn down_mods(pos: atomik_ui::kurbo::Point, modifiers: Modifiers, clicks: u8) -> UiEvent {
    UiEvent::PointerDown {
        pos,
        button: PointerButton::Primary,
        modifiers,
        clicks,
    }
}

pub fn right_down(pos: atomik_ui::kurbo::Point) -> UiEvent {
    UiEvent::PointerDown {
        pos,
        button: PointerButton::Secondary,
        modifiers: Modifiers::NONE,
        clicks: 1,
    }
}

pub fn right_up(pos: atomik_ui::kurbo::Point) -> UiEvent {
    UiEvent::PointerUp {
        pos,
        button: PointerButton::Secondary,
        modifiers: Modifiers::NONE,
    }
}

pub fn press(key: atomik_ui::Key, modifiers: Modifiers) -> UiEvent {
    UiEvent::Key {
        key,
        modifiers,
        pressed: true,
    }
}

pub fn alt() -> Modifiers {
    Modifiers {
        alt: true,
        ..Modifiers::NONE
    }
}

/// Only the song commands among `actions`.
pub fn song_commands(actions: &[UiAction]) -> Vec<atomik_model::SongCommand> {
    commands(actions)
        .into_iter()
        .filter_map(|c| match c {
            atomik_model::Command::Song(s) => Some(s),
            _ => None,
        })
        .collect()
}

/// Feeds a sequence of events to a view-like handler and collects every action.
pub fn run(f: impl FnMut(&UiEvent) -> Vec<UiAction>, events: &[UiEvent]) -> Vec<UiAction> {
    events.iter().flat_map(f).collect()
}
