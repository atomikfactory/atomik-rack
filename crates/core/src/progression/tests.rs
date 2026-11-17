//! Progression tests: names, voicing, the generator's rules and the suggestions.

use super::*;

const C_MAJOR: Key = Key {
    root: 0,
    mode: KeyMode::Major,
};
const A_MINOR: Key = Key {
    root: 9,
    mode: KeyMode::Minor,
};
const NO_LOCKS: [Option<Chord>; MAX_SLOTS] = [None; MAX_SLOTS];

fn text(key: Key, p: &Progression, n: usize) -> String {
    (0..n)
        .map(|i| format!("{}({})", roman(key, p.chords[i]), p.beats[i]))
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn mood_labels_and_indices_line_up() {
    assert_eq!(Mood::ALL.len(), 12);
    for (i, m) in Mood::ALL.iter().enumerate() {
        assert_eq!(m.index(), i);
        assert_eq!(Mood::from_index(i), Some(*m));
        assert_eq!(m.label(), MOOD_LABELS[i]);
    }
    assert_eq!(Mood::from_index(12), None);
    assert_eq!(MOOD_LABELS[6], "Neo Soul");
}

#[test]
fn beats_table_and_index() {
    assert_eq!(BEATS, [1, 2, 3, 4, 6, 8, 12, 16]);
    assert_eq!(beats_index(4), 3);
    assert_eq!(beats_index(16), 7);
    assert_eq!(beats_index(5), 3);
    assert_eq!(beats_index(7), 4);
}

#[test]
fn chord_helpers() {
    let g = Chord::new(7, ChordKind::Major);
    assert_eq!(g.over(11).bass, Some(11));
    assert_eq!(g.over(7).bass, None);
    assert_eq!(Chord::new(19, ChordKind::Minor).root, 7);
    let d = g.over(11).transposed(5);
    assert_eq!((d.root, d.bass), (0, Some(4)));
    let sc = d.to_song_chord();
    assert_eq!(Chord::from(sc), d);
}

#[test]
fn chord_symbols() {
    assert_eq!(symbol(Chord::new(9, ChordKind::MinorSeventh)), "Am7");
    assert_eq!(symbol(Chord::new(7, ChordKind::Major).over(11)), "G/B");
    assert_eq!(symbol(Chord::new(6, ChordKind::Dim)), "F#dim");
    assert_eq!(symbol(Chord::new(0, ChordKind::HalfDim)), "Cm7b5");
    let f = Key::new(5, KeyMode::Major);
    assert_eq!(
        symbol_in(f, Chord::new(10, ChordKind::MajorSeventh)),
        "Bbmaj7"
    );
    assert_eq!(symbol(Chord::new(10, ChordKind::MajorSeventh)), "A#maj7");
    // Every kind has a symbol suffix.
    for k in ChordKind::ALL {
        let _ = symbol(Chord::new(0, k));
    }
}

#[test]
fn roman_numerals_in_major() {
    let r = |root: u8, kind: ChordKind| roman(C_MAJOR, Chord::new(root, kind));
    assert_eq!(r(0, ChordKind::Major), "I");
    assert_eq!(r(2, ChordKind::Minor), "ii");
    assert_eq!(r(4, ChordKind::Minor), "iii");
    assert_eq!(r(5, ChordKind::Major), "IV");
    assert_eq!(r(7, ChordKind::Seventh), "V7");
    assert_eq!(r(9, ChordKind::Minor), "vi");
    assert_eq!(r(11, ChordKind::Dim), "vii\u{b0}");
    assert_eq!(r(11, ChordKind::HalfDim), "vii\u{f8}7");
    assert_eq!(r(10, ChordKind::Major), "bVII");
    assert_eq!(r(8, ChordKind::Major), "bVI");
    assert_eq!(r(3, ChordKind::Major), "bIII");
    assert_eq!(r(5, ChordKind::Minor), "iv");
    assert_eq!(r(0, ChordKind::MajorSeventh), "Imaj7");
    assert_eq!(r(2, ChordKind::MinorSeventh), "ii7");
    assert_eq!(r(1, ChordKind::Major), "bII");
    assert_eq!(r(6, ChordKind::Dim), "#iv\u{b0}");
    // Secondary dominants and the tritone substitute.
    assert_eq!(r(4, ChordKind::Seventh), "V7/vi");
    assert_eq!(r(2, ChordKind::Seventh), "V7/V");
    assert_eq!(r(2, ChordKind::Major), "V/V");
    assert_eq!(r(9, ChordKind::Seventh), "V7/ii");
    assert_eq!(r(1, ChordKind::Seventh), "subV7");
    assert_eq!(r(0, ChordKind::Seventh), "I7");
}

#[test]
fn roman_numerals_in_minor() {
    let r = |root: u8, kind: ChordKind| roman(A_MINOR, Chord::new(root, kind));
    assert_eq!(r(9, ChordKind::Minor), "i");
    assert_eq!(r(11, ChordKind::Dim), "ii\u{b0}");
    assert_eq!(r(0, ChordKind::Major), "bIII");
    assert_eq!(r(2, ChordKind::Minor), "iv");
    assert_eq!(r(4, ChordKind::Minor), "v");
    assert_eq!(r(4, ChordKind::Major), "V");
    assert_eq!(r(4, ChordKind::Seventh), "V7");
    assert_eq!(r(5, ChordKind::Major), "bVI");
    assert_eq!(r(7, ChordKind::Major), "bVII");
    assert_eq!(r(2, ChordKind::Major), "IV");
    assert_eq!(r(8, ChordKind::DimSeventh), "vii\u{b0}7");
    assert_eq!(r(9, ChordKind::MinorSeventh), "i7");
}

#[test]
fn the_song_key_maps_to_the_device_mode() {
    let c_major = Scale::new(0, crate::theory::ScaleKind::Major);
    let a_minor = Scale::new(9, crate::theory::ScaleKind::NaturalMinor);
    let device_minor = Key::new(9, KeyMode::Minor);
    // Same mode: the tonics line up.
    assert_eq!(song_key_shift(C_MAJOR, c_major), Some(0));
    assert_eq!(song_key_shift(A_MINOR, a_minor), Some(0));
    let d_major = Scale::new(2, crate::theory::ScaleKind::Major);
    assert_eq!(song_key_shift(C_MAJOR, d_major), Some(2));
    // A major song key for a minor progression: its relative minor.
    assert_eq!(song_key_shift(device_minor, c_major), Some(0));
    assert_eq!(song_key_shift(device_minor, d_major), Some(2));
    // A minor song key for a major progression: its relative major.
    assert_eq!(song_key_shift(C_MAJOR, a_minor), Some(0));
    // Modes of a scale follow their third.
    let dorian = Scale::new(2, crate::theory::ScaleKind::Dorian);
    assert_eq!(song_key_shift(Key::new(2, KeyMode::Minor), dorian), Some(0));
    // No clear mode.
    assert_eq!(
        song_key_shift(C_MAJOR, Scale::new(0, crate::theory::ScaleKind::Chromatic)),
        None
    );
}

#[test]
fn extending_triads_in_the_key() {
    use ChordKind as K;
    let e = |root: u8, kind: K, x: Extend| extend_chord(C_MAJOR, Chord::new(root, kind), x).kind;
    assert_eq!(e(0, K::Major, Extend::AsWritten), K::Major);
    assert_eq!(e(0, K::Major, Extend::Sevenths), K::MajorSeventh);
    assert_eq!(e(7, K::Major, Extend::Sevenths), K::Seventh);
    assert_eq!(e(2, K::Major, Extend::Sevenths), K::Seventh);
    assert_eq!(e(9, K::Minor, Extend::Sevenths), K::MinorSeventh);
    assert_eq!(e(11, K::Dim, Extend::Sevenths), K::HalfDim);
    assert_eq!(e(0, K::Major, Extend::Ninths), K::Add9);
    assert_eq!(e(7, K::Major, Extend::Ninths), K::Ninth);
    assert_eq!(e(9, K::Minor, Extend::Ninths), K::MinorNinth);
    assert_eq!(e(0, K::MajorSeventh, Extend::Ninths), K::MajorNinth);
    assert_eq!(e(7, K::Seventh, Extend::Ninths), K::Ninth);
    // Sus, power and ninths stay.
    assert_eq!(e(7, K::Sus4, Extend::Ninths), K::Sus4);
    assert_eq!(e(0, K::Power, Extend::Sevenths), K::Power);
    assert_eq!(e(7, K::Ninth, Extend::Ninths), K::Ninth);
}

fn opts(lead: bool, register: u8, spread: Spread) -> VoiceOpts {
    VoiceOpts {
        key: C_MAJOR,
        lead,
        register,
        spread,
        extend: Extend::AsWritten,
    }
}

fn pcs(v: &Voicing) -> u16 {
    v.as_slice().iter().fold(0, |m, k| m | 1 << (k % 12))
}

#[test]
fn voicings_hold_the_chord_tones_around_the_register() {
    for kind in ChordKind::ALL {
        for root in 0..12u8 {
            for register in 0..4u8 {
                for spread in [Spread::Close, Spread::Open, Spread::Wide] {
                    let chord = Chord::new(root, kind);
                    let v = voice(chord, None, opts(false, register, spread));
                    assert!(
                        v.len >= 2 && usize::from(v.len) <= MAX_VOICE_NOTES,
                        "{kind:?}"
                    );
                    assert!(v.as_slice().windows(2).all(|w| w[0] < w[1]));
                    assert!(v.as_slice().iter().all(|k| *k <= 127));
                    let tones = SongChord::new(root, kind).pitch_mask();
                    assert_eq!(pcs(&v), tones, "{kind:?} {spread:?}");
                    let centre = [36, 48, 60, 72][usize::from(register)];
                    let mean =
                        v.as_slice().iter().map(|k| i32::from(*k)).sum::<i32>() / i32::from(v.len);
                    assert!(
                        (mean - centre).abs() <= 8,
                        "{kind:?} {spread:?} {mean} vs {centre}"
                    );
                }
            }
        }
    }
}

#[test]
fn spreads_shape_the_stack() {
    let c = Chord::new(0, ChordKind::Major);
    let close = voice(c, None, opts(false, 2, Spread::Close));
    assert_eq!(close.as_slice(), &[60, 64, 67]);
    let open = voice(c, None, opts(false, 2, Spread::Open));
    // Root, fifth, third an octave up.
    let o = open.as_slice();
    assert_eq!(o[1] - o[0], 7);
    assert_eq!(o[2] - o[0], 16);
    let wide = voice(c, None, opts(false, 2, Spread::Wide));
    let w = wide.as_slice();
    assert_eq!(w.len(), 4);
    assert_eq!(w[1] - w[0], 12);
    assert_eq!(w[0] % 12, 0);
    // Drop-2 of a seventh chord.
    let m7 = voice(
        Chord::new(0, ChordKind::MajorSeventh),
        None,
        opts(false, 2, Spread::Open),
    );
    let o = m7.as_slice();
    assert_eq!(o.len(), 4);
    assert_eq!(o[1] - o[0], 5);
    assert_eq!(o[3] - o[0], 16);
}

#[test]
fn voicing_with_extend_uses_the_key() {
    let mut o = opts(false, 2, Spread::Close);
    o.extend = Extend::Sevenths;
    let g = voice(Chord::new(7, ChordKind::Major), None, o);
    assert_eq!(pcs(&g), SongChord::new(7, ChordKind::Seventh).pitch_mask());
    let c = voice(Chord::new(0, ChordKind::Major), None, o);
    assert_eq!(
        pcs(&c),
        SongChord::new(0, ChordKind::MajorSeventh).pitch_mask()
    );
}

fn total_move(a: &Voicing, b: &Voicing) -> i32 {
    let a: Vec<i32> = a.as_slice().iter().map(|k| i32::from(*k)).collect();
    let b: Vec<i32> = b.as_slice().iter().map(|k| i32::from(*k)).collect();
    movement(&a, &b)
}

#[test]
fn voice_leading_never_moves_more_than_root_position() {
    // Every mood's chords in a row, in every register and spread.
    for mood in Mood::ALL {
        for key in [C_MAJOR, A_MINOR] {
            for seed in 0..20u32 {
                let p = generate(key, mood, 0.6, seed, 8, &NO_LOCKS);
                for register in 0..4u8 {
                    for spread in [Spread::Close, Spread::Open, Spread::Wide] {
                        let led = opts(true, register, spread);
                        let flat = opts(false, register, spread);
                        let mut prev: Option<Voicing> = None;
                        for c in &p.chords {
                            let v = voice(*c, prev.as_ref(), led);
                            if let Some(pv) = &prev {
                                let root_pos = voice(*c, None, flat);
                                assert!(
                                    total_move(pv, &v) <= total_move(pv, &root_pos),
                                    "{mood:?} {seed} {c:?} {pv:?} led {v:?} flat {root_pos:?}"
                                );
                            }
                            assert_eq!(pcs(&v), SongChord::new(c.root, c.kind).pitch_mask());
                            prev = Some(v);
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn voice_leading_keeps_common_tones_still() {
    let led = opts(true, 2, Spread::Close);
    let c = voice(Chord::new(0, ChordKind::Major), None, led);
    let am = voice(Chord::new(9, ChordKind::Minor), Some(&c), led);
    // C and E are common tones of C and Am: they stay.
    assert!(
        am.as_slice().contains(&60) && am.as_slice().contains(&64),
        "{am:?}"
    );
    assert!(total_move(&c, &am) <= 6);
}

#[test]
fn every_chord_of_every_mood_voices() {
    for mood in Mood::ALL {
        for key in [C_MAJOR, A_MINOR] {
            for c in vocabulary(key, mood, 1.0) {
                for spread in [Spread::Close, Spread::Wide] {
                    let v = voice(c, None, opts(true, 1, spread));
                    assert!(v.len >= 2, "{mood:?} {c:?}");
                }
            }
        }
    }
}

fn keys() -> [Key; 4] {
    [
        C_MAJOR,
        A_MINOR,
        Key::new(6, KeyMode::Major),
        Key::new(3, KeyMode::Minor),
    ]
}

#[test]
fn the_vocabulary_is_never_empty_and_signature_progressions_stay_in_it() {
    for mood in Mood::ALL {
        for minor in [false, true] {
            let prof = moods::profile(mood.index(), minor);
            let key = Key::new(
                0,
                if minor {
                    KeyMode::Minor
                } else {
                    KeyMode::Major
                },
            );
            let core = vocabulary(key, mood, 0.0);
            let all = vocabulary(key, mood, 1.0);
            assert!(core.len() >= 3, "{mood:?} {minor}");
            assert!(all.len() > core.len() || mood == Mood::Blues, "{mood:?}");
            for t in prof.templates {
                assert!(
                    matches!(t.steps.len(), 3..=8),
                    "{mood:?} template length {}",
                    t.steps.len()
                );
                let universe = if t.min_spice == 0 { &core } else { &all };
                for (d, k, b) in t.steps {
                    assert!(BEATS.contains(b));
                    let c = key.chord_at(*d, *k);
                    assert!(
                        universe.contains(&c),
                        "{mood:?} minor={minor}: {c:?} not in vocabulary"
                    );
                }
                for w in t.steps.windows(2) {
                    assert!(
                        (w[0].0, w[0].1) != (w[1].0, w[1].1),
                        "{mood:?} repeats in a template"
                    );
                }
                let first = t.steps[0];
                let last = t.steps[t.steps.len() - 1];
                assert!(
                    (first.0, first.1) != (last.0, last.1),
                    "{mood:?} loops onto itself"
                );
            }
            for n in prof.nodes {
                assert!((1..=10).contains(&n.w));
            }
        }
    }
}

#[test]
fn generated_chords_stay_in_the_vocabulary_at_zero_spice() {
    for key in keys() {
        for mood in Mood::ALL {
            let allowed = vocabulary(key, mood, 0.0);
            let allowed_roman: Vec<String> = allowed.iter().map(|c| roman(key, *c)).collect();
            for n in [1usize, 2, 3, 4, 5, 6, 7, 8] {
                for seed in 0..200u32 {
                    let p = generate(key, mood, 0.0, seed, n, &NO_LOCKS);
                    for c in &p.chords[..n] {
                        assert!(
                            allowed.contains(c),
                            "{key:?} {mood:?} n={n} seed={seed}: {c:?}"
                        );
                        assert!(allowed_roman.contains(&roman(key, *c)));
                    }
                }
            }
        }
    }
}

#[test]
fn colour_chords_appear_with_spice() {
    for mood in Mood::ALL {
        for key in [C_MAJOR, A_MINOR] {
            let core = vocabulary(key, mood, 0.0);
            let mut seen_colour = false;
            for seed in 0..300u32 {
                let p = generate(key, mood, 1.0, seed, 4, &NO_LOCKS);
                seen_colour |= p.chords[..4].iter().any(|c| !core.contains(c));
            }
            assert!(
                seen_colour,
                "{mood:?} {key:?} never used its colour at full spice"
            );
        }
    }
}

#[test]
fn locked_chords_stay_and_slots_are_always_valid() {
    let lock = Chord::new(6, ChordKind::Dim);
    for mood in Mood::ALL {
        for key in [C_MAJOR, A_MINOR] {
            for slot in 0..4usize {
                for seed in 0..40u32 {
                    let mut locks = NO_LOCKS;
                    locks[slot] = Some(lock);
                    let p = generate(key, mood, 0.5, seed, 4, &locks);
                    assert_eq!(p.chords[slot], lock, "{mood:?} {slot}");
                    for i in 0..3 {
                        assert_ne!(p.chords[i], p.chords[i + 1], "{mood:?} repeat at {i}");
                    }
                    assert!(p.beats.iter().all(|b| BEATS.contains(b)));
                }
            }
            // A lock past the count survives and nothing before it changes its meaning.
            let mut locks = NO_LOCKS;
            locks[6] = Some(lock);
            let p = generate(key, mood, 0.5, 1, 4, &locks);
            assert_eq!(p.chords[6], lock);
        }
    }
}

#[test]
fn neighbours_of_a_lock_lead_into_it() {
    // A locked I after a free chord: the free chord is mostly a dominant or predominant.
    let mut led = 0;
    let mut total = 0;
    for seed in 0..200u32 {
        let mut locks = NO_LOCKS;
        locks[2] = Some(Chord::new(0, ChordKind::Major));
        let p = generate(C_MAJOR, Mood::Pop, 0.0, seed, 4, &locks);
        let before = roman(C_MAJOR, p.chords[1]);
        total += 1;
        if ["V", "IV", "ii", "vii\u{b0}", "vi"].contains(&before.as_str()) {
            led += 1;
        }
    }
    assert!(led * 10 >= total * 9, "{led}/{total}");
}

#[test]
fn the_generator_is_deterministic_and_transposes() {
    for mood in Mood::ALL {
        let a = generate(C_MAJOR, mood, 0.4, 123, 6, &NO_LOCKS);
        let b = generate(C_MAJOR, mood, 0.4, 123, 6, &NO_LOCKS);
        assert_eq!(a, b);
        let d = generate(Key::new(2, KeyMode::Major), mood, 0.4, 123, 6, &NO_LOCKS);
        for i in 0..8 {
            assert_eq!(d.chords[i], a.chords[i].transposed(2), "{mood:?} {i}");
        }
        assert_eq!(d.beats, a.beats);
    }
}

#[test]
fn seeds_change_the_result_and_moods_differ() {
    for mood in Mood::ALL {
        let mut distinct = Vec::new();
        for seed in 0..60u32 {
            let p = generate(C_MAJOR, mood, 0.4, seed, 4, &NO_LOCKS);
            if !distinct.contains(&p.chords[..4].to_vec()) {
                distinct.push(p.chords[..4].to_vec());
            }
        }
        assert!(
            distinct.len() >= 6,
            "{mood:?} gives only {} progressions",
            distinct.len()
        );
    }
    let a = generate(C_MAJOR, Mood::Pop, 0.0, 0, 4, &NO_LOCKS);
    let b = generate(C_MAJOR, Mood::Jazz, 0.0, 0, 4, &NO_LOCKS);
    assert_ne!(a.chords[..4], b.chords[..4]);
}

#[test]
fn progressions_start_on_the_tonic_and_close_the_loop() {
    for mood in Mood::ALL {
        for key in [C_MAJOR, A_MINOR] {
            let mut tonic_first = 0;
            for seed in 0..200u32 {
                let p = generate(key, mood, 0.3, seed, 4, &NO_LOCKS);
                if p.chords[0].root == key.root {
                    tonic_first += 1;
                }
                // The loop never repeats a chord across its seam.
                assert_ne!(p.chords[3], p.chords[0], "{mood:?} {seed}");
            }
            assert!(
                tonic_first >= 50,
                "{mood:?} {key:?} starts on the tonic {tonic_first}/200"
            );
        }
    }
}

#[test]
fn lengths_are_valid_and_bar_aligned() {
    for mood in Mood::ALL {
        for n in 1..=8usize {
            for seed in 0..100u32 {
                let p = generate(C_MAJOR, mood, 0.5, seed, n, &NO_LOCKS);
                assert!(p.beats.iter().all(|b| BEATS.contains(b)));
                let sum: u32 = p.beats[..n].iter().map(|b| u32::from(*b)).sum();
                assert_eq!(
                    sum % 4,
                    0,
                    "{mood:?} n={n} seed={seed}: {:?}",
                    &p.beats[..n]
                );
            }
        }
    }
}

#[test]
fn blues_is_twelve_bars_in_seven_chords_most_of_the_time() {
    let mut twelve_bar = 0;
    for seed in 0..100u32 {
        let p = generate(C_MAJOR, Mood::Blues, 0.2, seed, 7, &NO_LOCKS);
        let sum: u32 = p.beats[..7].iter().map(|b| u32::from(*b)).sum();
        if sum == 48 && p.chords[0].kind == ChordKind::Seventh {
            twelve_bar += 1;
        }
    }
    assert!(twelve_bar >= 70, "{twelve_bar}");
}

#[test]
fn suggestions_after_the_dominant_resolve_first() {
    for key in [C_MAJOR, A_MINOR] {
        for mood in Mood::ALL {
            for spice in [0.0, 0.3, 1.0] {
                let prev = Chord::new(key.root + 7, ChordKind::Major);
                let s = suggest_next(key, mood, prev, spice);
                assert_eq!(
                    s[0].0.root, key.root,
                    "{mood:?} {key:?} {spice}: {:?}",
                    s[0]
                );
                // Never the chord itself.
                assert!(s.iter().all(|(c, _)| *c != prev));
                // The tonic family is strong.
                assert_eq!(s[0].1, Strength::Strong, "{mood:?}");
            }
        }
    }
}

#[test]
fn suggestions_carry_their_strengths() {
    let prev = Chord::new(0, ChordKind::Major);
    let s = suggest_next(C_MAJOR, Mood::Pop, prev, 0.5);
    assert!(s.iter().any(|(_, st)| *st == Strength::Strong));
    assert!(s.iter().any(|(_, st)| *st == Strength::Colour));
    assert!(s.iter().any(|(_, st)| *st == Strength::Surprise));
    // The model's own moves come before the surprises.
    let first_surprise = s
        .iter()
        .position(|(_, st)| *st == Strength::Surprise)
        .unwrap();
    let last_strong = s
        .iter()
        .rposition(|(_, st)| *st == Strength::Strong)
        .unwrap();
    assert!(last_strong < first_surprise, "{s:?}");
    // I -> IV, V, vi lead the way in pop.
    let top: Vec<u8> = s.iter().take(3).map(|(c, _)| c.root).collect();
    for r in [5, 7, 9] {
        assert!(top.contains(&r), "{top:?}");
    }
    // No duplicates.
    for (i, (a, _)) in s.iter().enumerate() {
        assert!(!s[i + 1..].iter().any(|(b, _)| a == b));
    }
}

#[test]
fn suggestions_work_from_any_chord() {
    for mood in Mood::ALL {
        for key in [C_MAJOR, A_MINOR] {
            for root in 0..12u8 {
                for kind in [
                    ChordKind::Major,
                    ChordKind::Seventh,
                    ChordKind::Dim,
                    ChordKind::Power,
                ] {
                    let s = suggest_next(key, mood, Chord::new(root, kind), 0.5);
                    assert!(s.len() >= 8);
                }
            }
        }
    }
}

#[test]
fn pinned_progressions_never_change() {
    // Same seed, same chords on every platform and every release of the tables: a change here
    // is a change of what users' fixed seeds sound like.
    let cases: [(Key, Mood, f32, u32, usize, &str); 6] = [
        (C_MAJOR, Mood::Pop, 0.3, 0, 4, "I(4) V(4) vi(4) IV(4)"),
        (
            C_MAJOR,
            Mood::Jazz,
            0.5,
            7,
            8,
            "ii7(2) V13(2) Imaj7(4) vi7(4) V7(4) Imaj7(4) V7/V(2) V7(2)",
        ),
        (
            C_MAJOR,
            Mood::Blues,
            0.2,
            3,
            7,
            "I7(16) IV7(8) I7(8) V7(4) IV7(4) I7(4) V7(4)",
        ),
        (
            A_MINOR,
            Mood::Dark,
            0.5,
            11,
            6,
            "i(4) bVI(4) vii°(4) bIII(4) v(4) subV7(4)",
        ),
        (
            A_MINOR,
            Mood::NeoSoul,
            0.8,
            42,
            5,
            "i9(4) bVII9(4) i9(4) V7(4) bVII9(4)",
        ),
        (
            Key::new(2, KeyMode::Major),
            Mood::Cinematic,
            1.0,
            9999,
            3,
            "I(4) bVII(4) V/ii(8)",
        ),
    ];
    for (key, mood, spice, seed, n, expect) in cases {
        let p = generate(key, mood, spice, seed, n, &NO_LOCKS);
        assert_eq!(text(key, &p, n), expect, "{mood:?} seed {seed}");
    }
}
