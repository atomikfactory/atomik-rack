//! The voicing generator across the whole catalogue.

use atomik_tab::chord::ChordSymbol;
use atomik_tab::instrument::{Capo, catalogue, find_tuning};
use atomik_tab::voicing::{VoicingCache, VoicingOptions, generate};

#[test]
fn triads_have_voicings_on_every_instrument() {
    let mut missing = Vec::new();
    for inst in catalogue() {
        for nt in &inst.tunings {
            for name in ["C", "Am", "G", "D7", "F#m", "Bb"] {
                let c = ChordSymbol::parse(name).unwrap();
                let v = generate(&nt.tuning, Capo::NONE, &c, &VoicingOptions::default());
                if v.is_empty() {
                    missing.push(format!("{}/{} {name}", inst.id, nt.id));
                }
            }
        }
    }
    assert!(missing.is_empty(), "{missing:?}");
}

#[test]
fn voicings_hold_the_essential_tones_when_they_can() {
    let c = ChordSymbol::parse("G7").unwrap();
    let t = &find_tuning("guitar6", "standard").unwrap().tuning;
    let list = generate(t, Capo::NONE, &c, &VoicingOptions::default());
    assert!(list.len() >= 6);
    for v in list.iter().take(3) {
        let m = v.pc_mask(t, Capo::NONE);
        // G, B and F are essential (the fifth, D, is optional).
        for pc in [7u8, 11, 5] {
            assert!(m & (1 << pc) != 0, "{v:?}");
        }
    }
}

#[test]
fn ranking_is_stable_and_cached() {
    let t = &find_tuning("ukulele_concert", "reentrant").unwrap().tuning;
    let c = ChordSymbol::parse("Em").unwrap();
    let a = generate(t, Capo::NONE, &c, &VoicingOptions::default());
    let b = generate(t, Capo::NONE, &c, &VoicingOptions::default());
    assert_eq!(a, b);
    let mut cache = VoicingCache::new();
    let x = cache.get(t, Capo::NONE, &c, &VoicingOptions::default());
    assert_eq!(*x, a);
}

#[test]
fn any_custom_tuning_works() {
    for opens in [
        vec![64u8, 59, 57, 52, 48, 43, 38],
        vec![60u8],
        vec![60u8, 67],
    ] {
        let t = atomik_tab::Tuning::from_opens(&opens, 22);
        let c = ChordSymbol::parse("Am7").unwrap();
        let v = generate(&t, Capo::NONE, &c, &VoicingOptions::default());
        for x in &v {
            assert_eq!(x.frets.len(), opens.len());
        }
    }
}

#[test]
fn diagrams_start_at_a_base_fret_for_high_shapes() {
    let t = &find_tuning("guitar6", "standard").unwrap().tuning;
    let c = ChordSymbol::parse("F#m").unwrap();
    let list = generate(t, Capo::NONE, &c, &VoicingOptions::default());
    let high = list
        .iter()
        .find(|v| v.frets.iter().flatten().any(|f| *f > 5));
    if let Some(v) = high {
        let min = v
            .frets
            .iter()
            .flatten()
            .copied()
            .filter(|f| *f > 0)
            .min()
            .unwrap();
        assert_eq!(v.base_fret, min);
    }
    assert!(list.iter().all(|v| v.base_fret >= 1));
}
