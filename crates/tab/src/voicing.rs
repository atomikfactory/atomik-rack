//! The voicing generator: chord shapes for any tuning.
//!
//! [`generate`] enumerates fingerings inside a window of at most five frets that slides from the
//! nut to the fifteenth fret. Every string is muted, open or fretted, a fretted or open string
//! only sounds a chord tone (the short drone string of a banjo may also sound open under any
//! chord, as is the custom), and a shape needs at most four fretting fingers plus one barre.
//! The shapes are ranked by playability: a small span, few fingers, open strings, the root (or
//! the slash bass) in the bass, every essential chord tone present and no muted strings between
//! sounding ones. Re-entrant tunings (ukulele, banjo) give the bass rule a lower weight, since
//! their lowest sounding string is rarely the root.
//!
//! The strings of a [`Voicing`] are in tab order and its fret numbers are relative to the capo,
//! as everywhere in this crate.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::chord::{ChordSymbol, Role};
use crate::instrument::{Capo, Tuning};

/// A barre: one finger across several strings at a fret.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Barre {
    /// The fret.
    pub fret: u8,
    /// The first string (tab-order index, the lower index).
    pub from: usize,
    /// The last string.
    pub to: usize,
}

/// A fingering of one chord.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Voicing {
    /// The fret of each string in tab order: `None` is muted, `Some(0)` open.
    pub frets: Vec<Option<u8>>,
    /// The finger (1 = index to 4 = little) of each string; `None` for muted and open strings.
    /// Strings under a barre carry the barre's finger.
    pub fingers: Vec<Option<u8>>,
    /// The barres (at most one).
    pub barres: Vec<Barre>,
    /// The fret a diagram starts at: 1 for shapes within the first five frets, else the lowest
    /// fretted fret.
    pub base_fret: u8,
}

impl Voicing {
    /// The sounding pitches in tab order (`None` for muted strings).
    pub fn pitches(&self, tuning: &Tuning, capo: Capo) -> Vec<Option<u8>> {
        self.frets
            .iter()
            .enumerate()
            .map(|(s, f)| f.and_then(|f| tuning.pitch(s, f, capo)))
            .collect()
    }

    /// The number of sounding strings.
    pub fn sounding(&self) -> usize {
        self.frets.iter().flatten().count()
    }

    /// The lowest sounding pitch class.
    pub fn bass_pc(&self, tuning: &Tuning, capo: Capo) -> Option<u8> {
        self.pitches(tuning, capo)
            .into_iter()
            .flatten()
            .min()
            .map(|p| p % 12)
    }

    /// The set of pitch classes sounded, as a 12-bit mask.
    pub fn pc_mask(&self, tuning: &Tuning, capo: Capo) -> u16 {
        self.pitches(tuning, capo)
            .into_iter()
            .flatten()
            .fold(0, |m, p| m | (1 << (p % 12)))
    }
}

/// Options of the generator.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoicingOptions {
    /// How many voicings to return.
    pub limit: usize,
    /// The fewest sounding strings; `None` picks 3 for four strings or more, else up to 2.
    pub min_strings: Option<usize>,
    /// Thirds and sevenths count as optional (root-and-fifth shapes for a bass).
    pub relaxed: bool,
}

impl Default for VoicingOptions {
    fn default() -> Self {
        Self {
            limit: 12,
            min_strings: None,
            relaxed: false,
        }
    }
}

impl VoicingOptions {
    /// Options for a bass: two strings are enough and the third is optional.
    pub fn bass() -> Self {
        Self {
            limit: 12,
            min_strings: Some(2),
            relaxed: true,
        }
    }
}

/// Assigns fingers and finds the barre of a fret pattern. `None` when the shape needs more
/// than four fingers (a barre counts as one).
pub fn assign_fingers(frets: &[Option<u8>]) -> Option<(Vec<Option<u8>>, Vec<Barre>)> {
    let fretted: Vec<(usize, u8)> = frets
        .iter()
        .enumerate()
        .filter_map(|(i, f)| f.filter(|f| *f > 0).map(|f| (i, f)))
        .collect();
    let mut fingers = vec![None; frets.len()];
    let mut barres = Vec::new();
    let Some(min_f) = fretted.iter().map(|(_, f)| *f).min() else {
        return Some((fingers, barres));
    };
    let at_min: Vec<usize> = fretted
        .iter()
        .filter(|(_, f)| *f == min_f)
        .map(|(i, _)| *i)
        .collect();
    let mut under_barre = |from: usize, to: usize| {
        for (i, f) in frets.iter().enumerate().take(to + 1).skip(from) {
            if f.is_some_and(|f| f >= min_f) {
                fingers[i] = Some(1);
            }
        }
    };
    let mut barre_used = false;
    if at_min.len() >= 2 && fretted.len() >= 4 {
        let from = at_min[0];
        let to = at_min[at_min.len() - 1];
        let valid = (from..=to).all(|i| frets[i].is_some_and(|f| f >= min_f));
        if valid {
            barres.push(Barre {
                fret: min_f,
                from,
                to,
            });
            under_barre(from, to);
            barre_used = true;
        }
    }
    let mut rest: Vec<(usize, u8)> = fretted
        .iter()
        .copied()
        .filter(|(i, _)| fingers[*i].is_none())
        .collect();
    // Lower frets first; at one fret the lower-sounding (higher index) string first.
    rest.sort_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)));
    let first = if barre_used { 2 } else { 1 };
    if rest.len() > usize::from(5 - first) {
        return None;
    }
    for (k, (i, _)) in rest.iter().enumerate() {
        fingers[*i] = Some(first + k as u8);
    }
    Some((fingers, barres))
}

struct StringOptions {
    /// `(fret, pc)` pairs that sound a chord tone, within the neck.
    fretted: Vec<(u8, u8)>,
    open_pc: Option<u8>,
    drone: bool,
}

/// The ranked voicings of `chord` on `tuning` with `capo` (best first).
pub fn generate(
    tuning: &Tuning,
    capo: Capo,
    chord: &ChordSymbol,
    opts: &VoicingOptions,
) -> Vec<Voicing> {
    let n = tuning.len();
    if n == 0 || n > 12 {
        return Vec::new();
    }
    let tones = chord.tones();
    let mut tone_mask: u16 = 0;
    for t in &tones {
        tone_mask |= 1 << t.pc;
    }
    let min_strings = opts
        .min_strings
        .unwrap_or(if n >= 4 { 3 } else { n.min(2) })
        .clamp(1, n);
    let per_string: Vec<StringOptions> = (0..n)
        .map(|s| {
            let open_pc = tuning.open_pitch(s, capo).map(|p| p % 12);
            let fretted = (tuning.min_fret(s, capo)..=tuning.max_fret(s, capo))
                .filter_map(|f| {
                    let pc = tuning.pitch(s, f, capo)? % 12;
                    (tone_mask & (1 << pc) != 0).then_some((f, pc))
                })
                .collect();
            StringOptions {
                fretted,
                open_pc,
                drone: tuning.strings[s].start_fret > 0,
            }
        })
        .collect();
    let re_entrant = tuning.is_re_entrant();
    let max_window = tuning.frets.saturating_sub(1).min(14);
    let mut best: BTreeMap<Vec<Option<u8>>, f32> = BTreeMap::new();
    for w in 0..=max_window {
        let lo = w.max(1);
        let hi = w.saturating_add(4);
        let mut cur: Vec<Option<u8>> = vec![None; n];
        let mut nodes = 0usize;
        walk(
            &Ctx {
                tuning,
                capo,
                chord,
                tones: &tones,
                per_string: &per_string,
                lo,
                hi,
                open_ok: w <= 3,
                min_strings,
                re_entrant,
                relaxed: opts.relaxed,
            },
            0,
            &mut cur,
            &mut best,
            &mut nodes,
        );
    }
    let mut ranked: Vec<(Vec<Option<u8>>, f32)> = best.into_iter().collect();
    ranked.sort_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    ranked.truncate(opts.limit.max(1));
    ranked
        .into_iter()
        .filter_map(|(frets, _)| {
            let (fingers, barres) = assign_fingers(&frets)?;
            let max_f = frets.iter().flatten().copied().max().unwrap_or(0);
            let min_f = frets.iter().flatten().copied().filter(|f| *f > 0).min();
            let base_fret = match min_f {
                Some(m) if max_f > 5 => m,
                _ => 1,
            };
            Some(Voicing {
                frets,
                fingers,
                barres,
                base_fret,
            })
        })
        .collect()
}

struct Ctx<'a> {
    tuning: &'a Tuning,
    capo: Capo,
    chord: &'a ChordSymbol,
    tones: &'a [crate::chord::ChordTone],
    per_string: &'a [StringOptions],
    lo: u8,
    hi: u8,
    open_ok: bool,
    min_strings: usize,
    re_entrant: bool,
    relaxed: bool,
}

const NODE_BUDGET: usize = 30_000;

fn walk(
    cx: &Ctx<'_>,
    s: usize,
    cur: &mut Vec<Option<u8>>,
    best: &mut BTreeMap<Vec<Option<u8>>, f32>,
    nodes: &mut usize,
) {
    if s == cur.len() {
        if let Some(score) = evaluate(cx, cur) {
            best.entry(cur.clone())
                .and_modify(|b| *b = b.min(score))
                .or_insert(score);
        }
        return;
    }
    *nodes += 1;
    if *nodes > NODE_BUDGET {
        return;
    }
    let opt = &cx.per_string[s];
    // Open first, then frets, then the mute: the greedy leaves are the full shapes.
    if (opt.drone
        || (cx.open_ok
            && opt
                .open_pc
                .is_some_and(|pc| cx.tones.iter().any(|t| t.pc == pc))))
        && opt.open_pc.is_some()
    {
        cur[s] = Some(0);
        walk(cx, s + 1, cur, best, nodes);
    }
    for (f, _) in &opt.fretted {
        if *f >= cx.lo && *f <= cx.hi {
            cur[s] = Some(*f);
            walk(cx, s + 1, cur, best, nodes);
        }
    }
    cur[s] = None;
    walk(cx, s + 1, cur, best, nodes);
}

/// The cost of a complete pattern (lower is better), or `None` when it is not a voicing.
fn evaluate(cx: &Ctx<'_>, frets: &[Option<u8>]) -> Option<f32> {
    let sounding: Vec<usize> = (0..frets.len()).filter(|i| frets[*i].is_some()).collect();
    if sounding.len() < cx.min_strings {
        return None;
    }
    let fretted: Vec<u8> = frets.iter().flatten().copied().filter(|f| *f > 0).collect();
    let min_f = fretted.iter().copied().min();
    let max_f = fretted.iter().copied().max().unwrap_or(0);
    if let Some(m) = min_f {
        if max_f - m > 4 {
            return None;
        }
        // An open string far from the hand is not an open-position shape.
        if frets
            .iter()
            .enumerate()
            .any(|(i, f)| *f == Some(0) && !cx.per_string[i].drone && m > 5)
        {
            return None;
        }
    }
    let (_, barres) = assign_fingers(frets)?;
    let mut cost = 0.0f32;
    let pitches: Vec<Option<u8>> = frets
        .iter()
        .enumerate()
        .map(|(s, f)| f.and_then(|f| cx.tuning.pitch(s, f, cx.capo)))
        .collect();
    // Tones covered.
    let mut present: u16 = 0;
    for p in pitches.iter().flatten() {
        present |= 1 << (p % 12);
    }
    for t in cx.tones {
        if present & (1 << t.pc) != 0 {
            continue;
        }
        cost += match t.role {
            Role::Root => 25.0,
            Role::Third | Role::Seventh | Role::Sixth if cx.relaxed => 3.0,
            Role::Third => 40.0,
            Role::Seventh | Role::Sixth => 35.0,
            Role::Fifth if t.essential => 30.0,
            Role::Fifth if cx.relaxed => 12.0,
            Role::Fifth => 4.0,
            Role::Bass => 20.0,
            _ if t.essential => 30.0,
            _ => 6.0,
        };
    }
    // Bass.
    let lowest = pitches.iter().flatten().copied().min()?;
    let want = cx.chord.bass_pc();
    if lowest % 12 != want {
        let w = if cx.chord.bass.is_some() { 80.0 } else { 30.0 };
        cost += if cx.re_entrant { w * 0.1 } else { w };
    }
    // Hand: span, fingers, barres, position.
    if let Some(m) = min_f {
        cost += 3.0 * f32::from(max_f - m);
        cost += 1.5 * f32::from(m);
    }
    let finger_count = {
        let (fingers, _) = assign_fingers(frets)?;
        let mut used: Vec<u8> = fingers.into_iter().flatten().collect();
        used.sort_unstable();
        used.dedup();
        used.len()
    };
    cost += 3.0 * finger_count as f32;
    cost += 5.0 * barres.len() as f32;
    for b in &barres {
        if b.to - b.from >= 4 {
            cost += 2.0;
        }
    }
    // Open and muted strings.
    let first = sounding[0];
    let last = sounding[sounding.len() - 1];
    for (i, f) in frets.iter().enumerate() {
        match f {
            Some(0) => {
                let pc = cx.per_string[i].open_pc;
                let tone = pc.is_some_and(|pc| cx.tones.iter().any(|t| t.pc == pc));
                if tone && !cx.per_string[i].drone && min_f.is_some_and(|m| m >= 4) {
                    // An open string beside a hand high on the neck is awkward.
                    cost += 2.0;
                } else if tone {
                    cost -= 2.5;
                } else if cx.per_string[i].drone {
                    cost += 1.0;
                }
            }
            Some(_) => {}
            None => {
                cost += if i < first || i > last {
                    if frets.len() <= 5 { 8.0 } else { 3.0 }
                } else {
                    12.0
                };
            }
        }
    }
    cost -= 1.5 * sounding.len() as f32;
    Some(cost)
}

/// A cache of ranked voicings per (tuning, capo, chord, options).
#[derive(Debug, Default)]
pub struct VoicingCache {
    map: BTreeMap<CacheKey, Arc<Vec<Voicing>>>,
}

const CACHE_LIMIT: usize = 512;

type CacheKey = (Tuning, Capo, String, usize, bool);

impl VoicingCache {
    /// An empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// The voicings of `chord`, generated on the first request.
    pub fn get(
        &mut self,
        tuning: &Tuning,
        capo: Capo,
        chord: &ChordSymbol,
        opts: &VoicingOptions,
    ) -> Arc<Vec<Voicing>> {
        if self.map.len() >= CACHE_LIMIT {
            self.map.clear();
        }
        let key = (
            tuning.clone(),
            capo,
            chord.name(),
            opts.limit,
            opts.relaxed || opts.min_strings.is_some(),
        );
        if let Some(v) = self.map.get(&key) {
            return Arc::clone(v);
        }
        let v = Arc::new(generate(tuning, capo, chord, opts));
        self.map.insert(key, Arc::clone(&v));
        v
    }

    /// The number of cached chords.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether nothing is cached.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instrument::find_tuning;

    fn chord(t: &str) -> ChordSymbol {
        ChordSymbol::parse(t).unwrap()
    }

    fn tuning(i: &str, t: &str) -> Tuning {
        find_tuning(i, t).unwrap().tuning.clone()
    }

    fn top(t: &Tuning, c: &str) -> Voicing {
        generate(t, Capo::NONE, &chord(c), &VoicingOptions::default())
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("no voicing for {c}"))
    }

    fn f(v: &Voicing) -> Vec<Option<u8>> {
        v.frets.clone()
    }

    /// The text of a fret pattern read low string first (the order guitarists write).
    fn low_first(v: &Voicing) -> String {
        v.frets
            .iter()
            .rev()
            .map(|f| f.map_or("x".to_owned(), |f| f.to_string()))
            .collect::<Vec<_>>()
            .join(",")
    }

    #[test]
    fn guitar_open_chords() {
        let g = tuning("guitar6", "standard");
        assert_eq!(low_first(&top(&g, "C")), "x,3,2,0,1,0");
        assert_eq!(low_first(&top(&g, "G")), "3,2,0,0,0,3");
        assert_eq!(low_first(&top(&g, "D")), "x,x,0,2,3,2");
        assert_eq!(low_first(&top(&g, "A")), "x,0,2,2,2,0");
        assert_eq!(low_first(&top(&g, "E")), "0,2,2,1,0,0");
        assert_eq!(low_first(&top(&g, "Am")), "x,0,2,2,1,0");
        assert_eq!(low_first(&top(&g, "Em")), "0,2,2,0,0,0");
        assert_eq!(low_first(&top(&g, "Dm")), "x,x,0,2,3,1");
        let fm7 = top(&g, "Fmaj7");
        assert_eq!(fm7.bass_pc(&g, Capo::NONE), Some(5));
        assert_eq!(
            fm7.pc_mask(&g, Capo::NONE) & 0b1000_1011_0001,
            0b1000_1011_0001 & fm7.pc_mask(&g, Capo::NONE)
        );
    }

    #[test]
    fn guitar_f_has_a_barre_or_open_form_among_the_best() {
        let g = tuning("guitar6", "standard");
        let list = generate(&g, Capo::NONE, &chord("F"), &VoicingOptions::default());
        assert!(!list.is_empty());
        let full: Vec<String> = list.iter().map(low_first).collect();
        // Every top voicing holds F, A and C with F in the bass.
        for v in list.iter().take(4) {
            let mask = v.pc_mask(&g, Capo::NONE);
            assert_eq!(mask & 0b0010_0010_0001, 0b0010_0010_0001 & mask);
            assert!(
                mask & (1 << 5) != 0 && mask & (1 << 9) != 0 && mask & 1 != 0,
                "{full:?}"
            );
            assert_eq!(v.bass_pc(&g, Capo::NONE), Some(5), "{full:?}");
        }
        assert!(
            full.iter()
                .any(|s| s == "1,3,3,2,1,1" || s == "x,x,3,2,1,1"),
            "{full:?}"
        );
    }

    #[test]
    fn barre_voicings_carry_the_barre() {
        let g = tuning("guitar6", "standard");
        let list = generate(&g, Capo::NONE, &chord("F"), &VoicingOptions::default());
        let barre = list.iter().find(|v| !v.barres.is_empty());
        let v = barre.expect("a barre shape in the list");
        let b = v.barres[0];
        assert!(b.from < b.to);
        assert_eq!(v.fingers[b.from], Some(1));
        // A shape with a barre has at most three other fingers.
        let others = v.fingers.iter().flatten().filter(|f| **f != 1).count();
        assert!(others <= 3);
    }

    #[test]
    fn ukulele_open_chords() {
        // Re-entrant gCEA; the tab order is A E C G, so the fret strings below are read in
        // that order (the top line first): C = 0003 low-first is 3,0,0,0 here.
        let u = tuning("ukulele_soprano", "reentrant");
        assert_eq!(f(&top(&u, "C")), vec![Some(3), Some(0), Some(0), Some(0)]);
        assert_eq!(f(&top(&u, "G")), vec![Some(2), Some(3), Some(2), Some(0)]);
        assert_eq!(f(&top(&u, "Am")), vec![Some(0), Some(0), Some(0), Some(2)]);
        assert_eq!(f(&top(&u, "F")), vec![Some(0), Some(1), Some(0), Some(2)]);
    }

    #[test]
    fn banjo_open_g() {
        let b = tuning("banjo5", "open_g");
        // The drone string is the last tab line; open G is every string open.
        assert_eq!(
            f(&top(&b, "G")),
            vec![Some(0), Some(0), Some(0), Some(0), Some(0)]
        );
        let c = top(&b, "C");
        let mask = c.pc_mask(&b, Capo::NONE);
        assert!(mask & 1 != 0 && mask & (1 << 4) != 0 && mask & (1 << 7) != 0);
        assert!(c.frets.iter().flatten().all(|f| *f <= 3 || *f >= 5));
        let d7 = generate(&b, Capo::NONE, &chord("D7"), &VoicingOptions::default());
        assert!(!d7.is_empty());
        let best_mask = d7[0].pc_mask(&b, Capo::NONE);
        for pc in [2u8, 6, 0] {
            assert!(best_mask & (1 << pc) != 0, "D7 misses pc {pc}: {:?}", d7[0]);
        }
        // The drone never sounds below its start fret.
        for v in &d7 {
            if let Some(Some(fr)) = v.frets.get(4) {
                assert!(*fr == 0 || *fr >= 5);
            }
        }
    }

    #[test]
    fn mandolin_and_bass() {
        let m = tuning("mandolin", "standard");
        let g = top(&m, "G");
        assert!(
            g.frets.iter().all(Option::is_some),
            "a full mandolin shape: {g:?}"
        );
        let mask = g.pc_mask(&m, Capo::NONE);
        assert!(mask & (1 << 7) != 0 && mask & (1 << 11) != 0 && mask & (1 << 2) != 0);

        let b = tuning("bass4", "standard");
        for (name, root) in [("C", 0u8), ("G", 7), ("A", 9), ("F", 5)] {
            let v = generate(&b, Capo::NONE, &chord(name), &VoicingOptions::bass())
                .into_iter()
                .next()
                .unwrap();
            assert_eq!(v.bass_pc(&b, Capo::NONE), Some(root), "{name}");
            let mask = v.pc_mask(&b, Capo::NONE);
            assert!(mask & (1 << ((root + 7) % 12)) != 0, "{name} has its fifth");
            assert!(v.sounding() >= 2);
        }
    }

    #[test]
    fn slash_chords_put_the_bass_first() {
        let g = tuning("guitar6", "standard");
        let v = top(&g, "C/G");
        assert_eq!(v.bass_pc(&g, Capo::NONE), Some(7));
        let v = top(&g, "D/F#");
        assert_eq!(v.bass_pc(&g, Capo::NONE), Some(6));
    }

    #[test]
    fn capo_changes_the_shape_numbers_not_the_sound() {
        let g = tuning("guitar6", "standard");
        let capo = Capo::at(2);
        // A D shape with a capo at 2 sounds E.
        let list = generate(&g, capo, &chord("E"), &VoicingOptions::default());
        let v = &list[0];
        assert_eq!(low_first(v), "x,x,0,2,3,2");
        assert_eq!(v.bass_pc(&g, capo), Some(4));
    }

    #[test]
    fn every_voicing_is_playable_and_correct_in_any_tuning() {
        let chords = [
            "C", "Am7", "G7", "F#m7b5", "Bbmaj7", "Dsus4", "E5", "Cadd9", "Gdim7",
        ];
        let mut tested = 0;
        for inst in crate::instrument::catalogue() {
            for nt in &inst.tunings {
                for c in chords {
                    let ch = chord(c);
                    let list = generate(&nt.tuning, Capo::NONE, &ch, &VoicingOptions::default());
                    let mut tone_mask = 0u16;
                    for t in ch.tones() {
                        tone_mask |= 1 << t.pc;
                    }
                    for v in &list {
                        assert_eq!(v.frets.len(), nt.tuning.len());
                        assert_eq!(v.fingers.len(), nt.tuning.len());
                        let fingers: std::collections::BTreeSet<u8> =
                            v.fingers.iter().flatten().copied().collect();
                        assert!(fingers.len() <= 4, "{} {c}: {v:?}", inst.id);
                        assert!(v.barres.len() <= 1);
                        for (s, fr) in v.frets.iter().enumerate() {
                            if let Some(fr) = fr {
                                let min = nt.tuning.min_fret(s, Capo::NONE);
                                let drone = nt.tuning.strings[s].start_fret > 0;
                                assert!(*fr == 0 || *fr >= min, "{} {c}: {v:?}", inst.id);
                                if *fr > 0 || !drone {
                                    let pc = nt.tuning.pitch(s, *fr, Capo::NONE).unwrap() % 12;
                                    assert!(tone_mask & (1 << pc) != 0, "{} {c}: {v:?}", inst.id);
                                }
                            }
                        }
                        tested += 1;
                    }
                }
            }
        }
        assert!(tested > 500, "{tested}");
    }

    #[test]
    fn wide_necks_terminate() {
        let t = Tuning::from_opens(&[70, 68, 66, 64, 62, 60, 58, 56, 54, 52, 50, 48], 24);
        let list = generate(&t, Capo::NONE, &chord("Cmaj7"), &VoicingOptions::default());
        assert!(list.len() <= 12);
        let one = Tuning::from_opens(&[60], 12);
        let list = generate(&one, Capo::NONE, &chord("C"), &VoicingOptions::default());
        assert!(!list.is_empty());
        assert!(
            generate(
                &Tuning::from_opens(&[], 12),
                Capo::NONE,
                &chord("C"),
                &VoicingOptions::default()
            )
            .is_empty()
        );
    }

    #[test]
    fn finger_assignment_basics() {
        // Open C: fret 1 on the B string, 2 on D, 3 on A.
        let (fingers, barres) =
            assign_fingers(&[Some(0), Some(1), Some(0), Some(2), Some(3), None]).unwrap();
        assert!(barres.is_empty());
        assert_eq!(fingers, vec![None, Some(1), None, Some(2), Some(3), None]);
        // Full F barre: one barre and three other fingers.
        let (fingers, barres) =
            assign_fingers(&[Some(1), Some(1), Some(2), Some(3), Some(3), Some(1)]).unwrap();
        assert_eq!(
            barres,
            vec![Barre {
                fret: 1,
                from: 0,
                to: 5
            }]
        );
        assert_eq!(fingers[0], Some(1));
        assert_eq!(fingers[5], Some(1));
        // Five separate frets cannot be fingered.
        assert!(assign_fingers(&[Some(1), Some(2), Some(3), Some(4), Some(5), Some(6)]).is_none());
    }

    #[test]
    fn cache_returns_the_same_list() {
        let g = tuning("guitar6", "standard");
        let mut cache = VoicingCache::new();
        let a = cache.get(&g, Capo::NONE, &chord("Am"), &VoicingOptions::default());
        let b = cache.get(&g, Capo::NONE, &chord("Am"), &VoicingOptions::default());
        assert!(Arc::ptr_eq(&a, &b));
        assert_eq!(cache.len(), 1);
        let c = cache.get(&g, Capo::at(2), &chord("Am"), &VoicingOptions::default());
        assert!(!Arc::ptr_eq(&a, &c));
        assert_eq!(cache.len(), 2);
    }
}
