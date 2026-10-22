//! Player fixtures: a rack with one step sequencer `seq` holding three patterns
//! and one arpeggiator `arp`, plus a synced grid editor over the sequencer.

use super::*;
use atomik_device_sdk::Registry;
use atomik_model::{
    Command, Document, PatternDoc, RackDoc, SeqResolution, StepDoc, StepSeqDoc, SyncOp,
};
use atomik_ui::kurbo::{Point, Rect};
use atomik_ui::{SeqEditorView, Theme, UiAction};

pub const SEQ_TYPE: &str = "atomik.step-sequencer";
pub const ARP_TYPE: &str = "atomik.arpeggiator";
pub const EDITOR_BOUNDS: Rect = Rect::new(0.0, 0.0, 1000.0, 560.0);

/// A step: on, `key`, velocity, gate, probability, no tie.
pub fn step(key: u8, velocity: u8, gate: u8, prob: u8) -> StepDoc {
    StepDoc {
        on: true,
        key,
        velocity,
        gate,
        prob,
        tie: false,
    }
}

/// "Bass": 16 x 1/16, a four-note line with one tie and one 50 % step.
pub fn bass() -> PatternDoc {
    let mut p = PatternDoc::new("Bass");
    p.steps[0] = step(36, 110, 80, 100);
    p.steps[3] = step(36, 90, 50, 100);
    p.steps[4] = StepDoc {
        tie: true,
        ..step(43, 100, 100, 100)
    };
    p.steps[5] = step(46, 100, 90, 100);
    p.steps[8] = step(36, 120, 60, 50);
    p.steps[12] = step(48, 70, 40, 100);
    p
}

/// "Fill": 12 x 1/8 with swing 20.
pub fn fill() -> PatternDoc {
    let mut p = PatternDoc::new("Fill");
    p.length = 12;
    p.resolution = SeqResolution::Eighth;
    p.swing = 20;
    p.steps[0] = step(60, 100, 80, 100);
    p.steps[6] = step(67, 80, 80, 100);
    p
}

pub fn seq_patterns() -> StepSeqDoc {
    StepSeqDoc {
        patterns: vec![bass(), fill(), PatternDoc::new("Pattern 3")],
    }
}

/// The reference rack plus step sequencer `seq` (3 patterns) and arpeggiator `arp`.
pub fn players_doc() -> RackDoc {
    let mut doc = reference_doc();
    let mut s = device("seq", SEQ_TYPE, "Step Sequencer");
    s.steps = Some(seq_patterns());
    let mut a = device("arp", ARP_TYPE, "Arpeggiator");
    a.params.insert("mode".into(), 2.0);
    a.params.insert("rate".into(), 3.0);
    a.params.insert("octaves".into(), 2.0);
    doc.devices.insert(0, s);
    doc.devices.insert(1, a);
    doc
}

/// Only the sequencer.
pub fn seq_only() -> RackDoc {
    let mut doc = players_doc();
    doc.devices.retain(|d| d.key == key("seq"));
    doc.cables.clear();
    doc
}

/// Only the arpeggiator.
pub fn arp_only() -> RackDoc {
    let mut doc = players_doc();
    doc.devices.retain(|d| d.key == key("arp"));
    doc.cables.clear();
    doc
}

pub const MODSRC_TYPE: &str = "atomik.mod-source";

/// Only a Mod Source `mod` (LFO 1 saw up, LFO 2 unipolar pulse at morph 0.6).
pub fn modsrc_only() -> RackDoc {
    let mut doc = reference_doc();
    let mut m = device("mod", MODSRC_TYPE, "Mod Source");
    m.params.insert("lfo1_shape".into(), 2.0);
    m.params.insert("lfo2_shape".into(), 4.0);
    m.params.insert("lfo2_morph".into(), 0.6);
    m.params.insert("lfo2_polarity".into(), 1.0);
    doc.devices = vec![m];
    doc.cables.clear();
    doc
}

pub fn editor(doc: &RackDoc, reg: &Registry) -> SeqEditorView {
    let mut v = SeqEditorView::new(Theme::dark());
    v.set_bounds(EDITOR_BOUNDS);
    v.open(key("seq"));
    v.sync(doc, reg);
    v
}

/// The pattern `index` of the sequencer in `doc`.
pub fn pattern_of(doc: &RackDoc, index: usize) -> PatternDoc {
    doc.device(&key("seq"))
        .and_then(|d| d.steps.as_ref())
        .expect("steps")
        .patterns[index]
        .clone()
}

pub fn pattern_count(doc: &RackDoc) -> usize {
    doc.device(&key("seq"))
        .and_then(|d| d.steps.as_ref())
        .expect("steps")
        .patterns
        .len()
}

/// Applies every command among `actions` to `document` (asserting the model accepts it), ends
/// merges on `EndMerge`, and returns the reported sync ops and the merge ids of the
/// `SetPattern`s seen.
pub fn apply_all(
    document: &mut Document,
    reg: &Registry,
    actions: &[UiAction],
) -> (Vec<SyncOp>, Vec<Option<u64>>) {
    let mut ops = Vec::new();
    let mut merges = Vec::new();
    for a in actions {
        match a {
            UiAction::Command(c) => {
                if let Command::SetPattern { merge, .. } = c {
                    merges.push(*merge);
                }
                ops.extend(
                    document
                        .apply(c.clone(), reg)
                        .unwrap_or_else(|e| panic!("model rejected {c:?}: {e}")),
                );
            }
            UiAction::EndMerge => document.end_merge(),
            _ => {}
        }
    }
    (ops, merges)
}

/// Every `SetPattern` among `actions`: (index, pattern, merge).
pub fn set_patterns(actions: &[UiAction]) -> Vec<(u8, PatternDoc, Option<u64>)> {
    actions
        .iter()
        .filter_map(|a| match a {
            UiAction::Command(Command::SetPattern {
                index,
                pattern,
                merge,
                ..
            }) => Some((*index, pattern.clone(), *merge)),
            _ => None,
        })
        .collect()
}

/// The `(key, velocity, on)` of the audition actions among `actions`.
pub fn auditions(actions: &[UiAction]) -> Vec<(u8, u8, bool)> {
    actions
        .iter()
        .filter_map(|a| match a {
            UiAction::AuditionVelocity {
                device,
                key,
                velocity,
                on,
            } => {
                assert_eq!(device, &super::key("seq"));
                Some((*key, *velocity, *on))
            }
            _ => None,
        })
        .collect()
}

/// A press, the moves and the release of a drag through `points`, with the pointer events fed
/// to `f`; returns every action.
pub fn drag_through(
    mut f: impl FnMut(&atomik_ui::UiEvent) -> Vec<UiAction>,
    points: &[Point],
) -> Vec<UiAction> {
    let mut out = Vec::new();
    let (first, rest) = points.split_first().expect("points");
    out.extend(f(&down(*first)));
    for p in rest {
        out.extend(f(&mv(*p)));
    }
    out.extend(f(&up(*points.last().expect("points"))));
    out
}
