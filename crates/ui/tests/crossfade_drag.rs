//! Dragging the crossfade zone at the start of an audio clip: the gesture emits trial steps
//! (`CrossfadeStep`) that the host applies without an undo step, the release keeps the last one
//! (`CrossfadeEnd`), Escape drops it. A press outside the zone keeps moving the clip.

mod support;

use atomik_model::{AudioClipContent, ClipContent, ClipId, FadeCurve, RackDoc};
use atomik_ui::kurbo::{Point, Vec2};
use atomik_ui::timeline::Snap;
use atomik_ui::{ArrangementView, Key, Modifiers, UiAction};
use support::audio::*;
use support::song::*;
use support::*;

/// Clip 11 starts at bar 2 over the last bar of clip 10 (bars 1..3): the zone is bar 2..3.
fn overlap_doc() -> RackDoc {
    let mut doc = audio_doc();
    doc.song_mut().tracks[1].lanes[0].clips[1].start = 2 * BAR;
    for c in &mut doc.song_mut().tracks[1].lanes[0].clips {
        if c.id == ClipId(11)
            && let ClipContent::Audio(a) = &mut c.content
        {
            *a = AudioClipContent {
                xfade: Some(FadeCurve::EqualPower),
                ..a.clone()
            };
        }
    }
    doc
}

fn setup() -> (ArrangementView, RackDoc, atomik_device_sdk::Registry) {
    let (doc, reg) = (overlap_doc(), registry());
    let mut v = audio_arrangement(&doc, &reg);
    v.set_snap(Snap::Beat);
    (v, doc, reg)
}

fn at_tick(v: &ArrangementView, tick: i64) -> Point {
    let r = v.clip_rect(ClipId(11)).unwrap();
    Point::new(v.tick_to_x(tick), r.center().y + 6.0)
}

fn steps(actions: &[UiAction]) -> Vec<(ClipId, i64)> {
    actions
        .iter()
        .filter_map(|a| match a {
            UiAction::CrossfadeStep { later, delta } => Some((*later, *delta)),
            _ => None,
        })
        .collect()
}

#[test]
fn dragging_the_zone_previews_the_move_and_the_release_keeps_it() {
    let (mut v, doc, reg) = setup();
    let p = at_tick(&v, 2 * BAR + BAR / 2);
    let to = at_tick(&v, 2 * BAR + BAR / 2 + BAR / 2);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[down(p), mv(Point::new((p.x + to.x) / 2.0, to.y)), mv(to)],
    );
    let got = steps(&a);
    assert_eq!(got.last(), Some(&(ClipId(11), BAR / 2)), "{a:?}");
    // No ordinary move or resize command: the host previews the crossfade.
    assert!(song_commands(&a).is_empty(), "{a:?}");
    let a = v.handle(&up(to), &doc, &reg);
    assert_eq!(a, vec![UiAction::CrossfadeEnd { commit: true }]);
}

#[test]
fn the_zone_stays_inside_both_clips() {
    let (mut v, doc, reg) = setup();
    let p = at_tick(&v, 2 * BAR + BAR / 2);
    // Far right: the earlier clip would end past the later one's end (it ends at bar 4).
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down(p),
            mv(p + Vec2::new(40.0, 0.0)),
            mv(p + Vec2::new(900.0, 0.0)),
        ],
    );
    assert_eq!(steps(&a).last(), Some(&(ClipId(11), BAR - 1)), "{a:?}");
    v.handle(&up(p), &doc, &reg);
    // Far left: the later clip would start at or before the earlier one's start (bar 1).
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down(p),
            mv(p - Vec2::new(40.0, 0.0)),
            mv(p - Vec2::new(900.0, 0.0)),
        ],
    );
    assert_eq!(steps(&a).last(), Some(&(ClipId(11), -(BAR - 1))), "{a:?}");
}

#[test]
fn escape_and_going_back_to_the_start_drop_the_preview() {
    let (mut v, doc, reg) = setup();
    let p = at_tick(&v, 2 * BAR + BAR / 2);
    let to = at_tick(&v, 2 * BAR + BAR / 2 + BAR / 2);
    let a = run(|e| v.handle(e, &doc, &reg), &[down(p), mv(to)]);
    assert_eq!(steps(&a).len(), 1);
    let a = v.handle(&press(Key::Escape, Modifiers::NONE), &doc, &reg);
    assert_eq!(a, vec![UiAction::CrossfadeEnd { commit: false }]);
    let a = v.handle(&up(to), &doc, &reg);
    assert!(a.is_empty(), "{a:?}");
    // Out and back: the preview is dropped and the release has nothing to keep.
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[down(p), mv(to), mv(p), up(p)],
    );
    assert_eq!(
        a.iter()
            .filter(|x| matches!(x, UiAction::CrossfadeEnd { .. }))
            .collect::<Vec<_>>(),
        vec![&UiAction::CrossfadeEnd { commit: false }]
    );
}

#[test]
fn a_press_outside_the_zone_or_with_shift_does_not_grab_it() {
    let (mut v, doc, reg) = setup();
    // Past the zone, in the body of the later clip: the clip moves as before.
    let p = at_tick(&v, 3 * BAR + BAR / 2);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down(p),
            mv(p + Vec2::new(40.0, 0.0)),
            up(p + Vec2::new(40.0, 0.0)),
        ],
    );
    assert!(steps(&a).is_empty(), "{a:?}");
    assert!(!song_commands(&a).is_empty(), "{a:?}");
    // Inside the zone with Shift held: a selection gesture.
    let p = at_tick(&v, 2 * BAR + BAR / 2);
    let a = run(
        |e| v.handle(e, &doc, &reg),
        &[
            down_mods(
                p,
                Modifiers {
                    shift: true,
                    ..Modifiers::NONE
                },
                1,
            ),
            mv_with(
                p + Vec2::new(60.0, 0.0),
                Modifiers {
                    shift: true,
                    ..Modifiers::NONE
                },
            ),
            up(p + Vec2::new(60.0, 0.0)),
        ],
    );
    assert!(steps(&a).is_empty(), "{a:?}");
}
