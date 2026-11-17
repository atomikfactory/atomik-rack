//! The neutral score document: bounds and conventions.

use atomik_tab::instrument::find_tuning;
use atomik_tab::score::{
    Bend, BendKind, BendPoint, PPQ, TabNote, TabScore, TabTrack, TechFlags, Techniques, limits,
};
use proptest::prelude::*;

fn guitar() -> atomik_tab::Tuning {
    find_tuning("guitar6", "standard").unwrap().tuning.clone()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn sanitize_is_idempotent_and_bounds_everything(
        notes in proptest::collection::vec((-100i64..PPQ * 100, -5i64..PPQ * 100, 0u8..20, 0u8..80, 0u8..255), 0..60),
        bpm in -10.0f64..5000.0,
    ) {
        let mut s = TabScore::new();
        let mut t = TabTrack::new("g", "guitar6", guitar());
        for (tick, len, string, fret, vel) in notes {
            let mut n = TabNote::new(tick, len, string, fret);
            n.velocity = vel;
            n.tech = Techniques::default();
            n.tech.bend = Some(Bend { points: vec![BendPoint { pos: 5.0, cents: 99_999 }, BendPoint { pos: -1.0, cents: -99_999 }] });
            n.tech.tremolo_division = 7;
            t.notes.push(n);
        }
        s.tracks.push(t);
        s.tempo.push((0, bpm));
        s.sanitize();
        let once = s.clone();
        s.sanitize();
        prop_assert_eq!(&s, &once);
        for n in &s.tracks[0].notes {
            prop_assert!(n.tick >= 0);
            prop_assert!(n.len >= 1);
            prop_assert!((1..=127).contains(&n.velocity));
            prop_assert!(usize::from(n.string) < 6);
            prop_assert_eq!(n.tech.tremolo_division, 0);
            for p in &n.tech.bend.as_ref().unwrap().points {
                prop_assert!((0.0..=1.0).contains(&p.pos) && p.cents.abs() <= 4800);
            }
        }
        prop_assert!(s.tracks[0].notes.windows(2).all(|w| (w[0].tick, w[0].string) <= (w[1].tick, w[1].string)));
        prop_assert!(s.tempo.iter().all(|(_, b)| (10.0..=1000.0).contains(b)));
    }
}

#[test]
fn technique_conventions() {
    // The flag marks the destination of a hammer-on or pull-off: both can be set on a chain.
    let mut t = Techniques::default();
    t.flags.insert(TechFlags::HAMMER_ON | TechFlags::PALM_MUTE);
    assert!(t.has(TechFlags::HAMMER_ON) && t.has(TechFlags::PALM_MUTE));
    assert!(!t.has(TechFlags::PULL_OFF));
    assert_eq!(Bend::up(100).kind(), BendKind::Bend);
    assert_eq!(Bend::pre_bend(100).kind(), BendKind::PreBend);
}

#[test]
fn caps_are_documented_constants() {
    const { assert!(limits::MAX_NOTES >= limits::MAX_NOTES_PER_TRACK) };
    assert_eq!(PPQ, atomik_core::PPQ);
}
