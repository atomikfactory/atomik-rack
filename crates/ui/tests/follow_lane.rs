//! The arrangement's followed lane: a lane that follows the selected parameter is drawn
//! highlighted and stays open although the project folds it (the fold in the project is not
//! touched), a reveal shows the track's lanes and scrolls to the lane, and with nothing followed the
//! view is as it was.

mod support;

use atomik_model::{Lane, LaneKind, RackDoc};
use atomik_ui::{ArrangementView, DisplayList};
use support::song::*;
use support::*;

const CUTOFF: &str = "filter.cutoff";

fn folded_doc() -> RackDoc {
    let mut doc = song_doc();
    doc.song
        .org
        .folded_lanes
        .insert((key("bass"), CUTOFF.to_owned()));
    doc
}

/// Twelve tracks, the last with a cutoff lane: its lane is far below a short view.
fn tall_doc() -> RackDoc {
    let mut doc = song_doc();
    for n in 0..12 {
        let k = format!("extra{n}");
        doc.devices.push(device(&k, "atomik.poly-synth", &k));
        let mut lanes = vec![Lane {
            kind: LaneKind::Notes,
            clips: Vec::new(),
            take_groups: Vec::new(),
        }];
        if n == 11 {
            lanes.push(Lane {
                kind: LaneKind::Automation(CUTOFF.into()),
                clips: Vec::new(),
                take_groups: Vec::new(),
            });
        }
        doc.song.tracks.push(track(&k, lanes));
    }
    doc
}

fn view_of(doc: &RackDoc) -> ArrangementView {
    arrangement(doc, &registry())
}

fn painted(v: &mut ArrangementView, doc: &RackDoc) -> DisplayList {
    let mut dl = DisplayList::new();
    v.paint(&mut dl, doc, &registry());
    dl
}

#[test]
fn a_followed_lane_stays_open_though_the_project_folds_it() {
    let doc = folded_doc();
    let mut v = view_of(&doc);
    let thin = v.lane_rect(&key("bass"), 1).unwrap().height();
    assert!(thin < 20.0, "folded: {thin}");
    v.follow_lane(Some((key("bass"), CUTOFF.to_owned())));
    v.sync(&doc, &registry());
    assert_eq!(v.lane_rect(&key("bass"), 1).unwrap().height(), 52.0);
    assert_eq!(v.followed_lane(), Some(&(key("bass"), CUTOFF.to_owned())));
    // The document is untouched: following again nothing folds it back.
    v.follow_lane(None);
    v.sync(&doc, &registry());
    assert_eq!(v.lane_rect(&key("bass"), 1).unwrap().height(), thin);
    assert!(doc.song.lane_folded(&key("bass"), CUTOFF));
}

#[test]
fn a_followed_lane_is_drawn_highlighted() {
    let doc = song_doc();
    let mut v = view_of(&doc);
    let plain = painted(&mut v, &doc);
    v.follow_lane(Some((key("bass"), CUTOFF.to_owned())));
    v.sync(&doc, &registry());
    let marked = painted(&mut v, &doc);
    assert!(
        marked.ops().len() > plain.ops().len(),
        "{} against {}",
        marked.ops().len(),
        plain.ops().len()
    );
    // Following a lane that is not there marks nothing and changes nothing.
    v.follow_lane(Some((key("lead"), CUTOFF.to_owned())));
    v.sync(&doc, &registry());
    assert_eq!(painted(&mut v, &doc).ops().len(), plain.ops().len());
}

#[test]
fn a_reveal_scrolls_a_lane_out_of_sight_into_view() {
    let doc = tall_doc();
    let mut v = view_of(&doc);
    assert!(
        v.lane_rect(&key("extra11"), 1).is_none_or(|r| r.y0 > 420.0),
        "below the view"
    );
    assert_eq!(v.scroll_y(), 0.0);
    v.reveal_lane(&key("extra11"), CUTOFF);
    v.sync(&doc, &registry());
    assert!(v.scroll_y() > 0.0);
    let r = v.lane_rect(&key("extra11"), 1).unwrap();
    assert!(r.y0 >= 0.0 && r.y1 <= 420.0, "{r:?}");
    // A lane already in view does not move the list.
    let before = v.scroll_y();
    v.reveal_lane(&key("extra11"), CUTOFF);
    v.sync(&doc, &registry());
    assert_eq!(v.scroll_y(), before);
}

#[test]
fn a_reveal_shows_the_lanes_of_a_track_that_was_collapsed() {
    let doc = song_doc();
    let mut v = view_of(&doc);
    v.set_expanded(&key("bass"), false);
    v.sync(&doc, &registry());
    assert!(v.lane_rect(&key("bass"), 1).is_none());
    v.reveal_lane(&key("bass"), CUTOFF);
    v.sync(&doc, &registry());
    assert!(v.lane_rect(&key("bass"), 1).is_some());
}
