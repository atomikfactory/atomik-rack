//! The catalogue as the outside world sees it.

use atomik_tab::instrument::{
    Capo, Family, MAX_STRINGS, Tuning, TuningError, catalogue, find_instrument, find_tuning,
    parse_pitch,
};

#[test]
fn spec_instruments_and_tunings_are_all_there() {
    let guitar_tunings = [
        "standard",
        "drop_d",
        "dadgad",
        "open_g",
        "open_d",
        "open_e",
        "half_down",
        "full_down",
        "drop_c",
    ];
    for t in guitar_tunings {
        assert!(find_tuning("guitar6", t).is_some(), "guitar6/{t}");
    }
    let n = |id: &str| find_instrument(id).unwrap().default_tuning().tuning.len();
    assert_eq!(n("guitar7"), 7);
    assert_eq!(n("guitar8"), 8);
    assert_eq!(n("bass4"), 4);
    assert_eq!(n("bass5"), 5);
    assert_eq!(n("bass6"), 6);
    for size in ["soprano", "concert", "tenor"] {
        let id = format!("ukulele_{size}");
        assert!(find_tuning(&id, "reentrant").is_some());
        assert!(find_tuning(&id, "low_g").is_some());
    }
    for t in ["open_g", "double_c", "open_d", "standard_c"] {
        let b = &find_tuning("banjo5", t).unwrap().tuning;
        assert_eq!(b.len(), 5);
        assert_eq!(
            b.strings[4].start_fret, 5,
            "{t}: the drone starts at the fifth fret"
        );
        assert!(b.strings[..4].iter().all(|s| s.start_fret == 0));
    }
    let opens = |i: &str, t: &str| find_tuning(i, t).unwrap().tuning.opens();
    assert_eq!(opens("banjo4_tenor", "standard"), vec![69, 62, 55, 48]);
    assert_eq!(opens("banjo4_plectrum", "standard"), vec![62, 59, 55, 48]);
    assert_eq!(opens("mandolin", "standard"), vec![76, 69, 62, 55]);
    assert_eq!(opens("bouzouki_irish", "standard"), vec![62, 57, 50, 43]);
    assert_eq!(opens("dobro", "open_g"), vec![62, 59, 55, 50, 47, 43]);
    assert_eq!(find_instrument("banjo5").unwrap().family, Family::Banjo);
}

#[test]
fn every_fret_of_every_string_finds_its_pitch_again() {
    for inst in catalogue() {
        for nt in &inst.tunings {
            let t = &nt.tuning;
            for capo in [Capo::NONE, Capo::at(2), Capo::partial(3, 0b11)] {
                for s in 0..t.len() {
                    let lo = t.min_fret(s, capo);
                    let hi = t.max_fret(s, capo);
                    let mut frets = vec![0u8];
                    frets.extend(lo..=hi);
                    for f in frets {
                        let Some(p) = t.pitch(s, f, capo) else {
                            continue;
                        };
                        let back = t
                            .fret_for_pitch(s, p, capo)
                            .unwrap_or_else(|| panic!("{}/{} string {s} fret {f}", inst.id, nt.id));
                        assert_eq!(t.pitch(s, back, capo), Some(p));
                    }
                }
            }
        }
    }
}

#[test]
fn custom_tunings_of_any_size_validate() {
    for n in 1..=MAX_STRINGS {
        let opens: Vec<u8> = (0..n).map(|i| 70 - 3 * i as u8).collect();
        assert_eq!(
            Tuning::from_opens(&opens, 24).validate(),
            Ok(()),
            "{n} strings"
        );
    }
    assert_eq!(
        Tuning::from_opens(&[40; MAX_STRINGS + 1], 24).validate(),
        Err(TuningError::StringCount(MAX_STRINGS + 1))
    );
}

#[test]
fn pitch_names() {
    assert_eq!(parse_pitch("A4"), Some(69));
    assert_eq!(parse_pitch("c#3"), Some(49));
    assert_eq!(parse_pitch("Db3"), Some(49));
    assert_eq!(parse_pitch(""), None);
}

#[test]
fn nothing_assumes_strings_are_sorted() {
    let re_entrant = catalogue()
        .iter()
        .flat_map(|i| i.tunings.iter())
        .filter(|nt| nt.tuning.is_re_entrant())
        .count();
    // Ukuleles (three sizes), banjos and the charango are re-entrant.
    assert!(re_entrant >= 8, "{re_entrant}");
}
