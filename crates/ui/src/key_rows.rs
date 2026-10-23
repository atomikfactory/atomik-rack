//! The rows of the piano roll: which keys are shown and in what order. Normally every key is a
//! row (127 at the top); a **fold** hides rows, so a row index and a key are no longer the same
//! thing and every place that turns a position into a key, or a key into a position, goes
//! through [`KeyRows`].

use atomik_model::theory::Scale;
use serde::{Deserialize, Serialize};

/// Which keys the piano roll shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Fold {
    /// Every key.
    #[default]
    Off,
    /// Only the keys of the clip's scale (and the keys its notes use, so no note is hidden).
    Scale,
    /// Only the keys the clip's notes use.
    Used,
}

impl Fold {
    /// Every mode, in menu order.
    pub const ALL: [Fold; 3] = [Fold::Off, Fold::Scale, Fold::Used];

    /// The stable name used in command arguments and `ui-state.json`.
    pub fn id(self) -> &'static str {
        match self {
            Fold::Off => "off",
            Fold::Scale => "scale",
            Fold::Used => "used",
        }
    }

    /// The name shown in menus.
    pub fn label(self) -> &'static str {
        match self {
            Fold::Off => "Off",
            Fold::Scale => "Fold to Scale",
            Fold::Used => "Fold to Used Notes",
        }
    }

    /// The mode named `id`.
    pub fn from_id(id: &str) -> Option<Fold> {
        Fold::ALL.into_iter().find(|f| f.id() == id)
    }
}

/// The visible keys, top row first (descending), and the way back from a key to its row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRows {
    keys: Vec<u8>,
    row: [i16; 128],
}

impl KeyRows {
    /// All 128 keys: row `r` is key `127 - r`.
    pub fn all() -> Self {
        Self::from_keys((0..=127u8).rev().collect())
    }

    fn from_keys(keys: Vec<u8>) -> Self {
        let mut row = [-1i16; 128];
        for (r, k) in keys.iter().enumerate() {
            row[usize::from(*k)] = r as i16;
        }
        Self { keys, row }
    }

    /// The rows of `fold` for a clip with `scale` whose notes use `used` (any order, repeats
    /// allowed). [`Fold::Scale`] without a scale and [`Fold::Used`] without notes show every
    /// key (there is nothing to fold to).
    pub fn build(fold: Fold, scale: Option<Scale>, used: &[u8]) -> Self {
        let mut is_used = [false; 128];
        for k in used {
            is_used[usize::from(*k).min(127)] = true;
        }
        let keys: Vec<u8> = match fold {
            Fold::Off => return Self::all(),
            Fold::Scale => match scale {
                Some(s) if !s.is_chromatic() => (0..=127u8)
                    .rev()
                    .filter(|k| s.contains(*k) || is_used[usize::from(*k)])
                    .collect(),
                _ => return Self::all(),
            },
            Fold::Used => {
                let keys: Vec<u8> = (0..=127u8)
                    .rev()
                    .filter(|k| is_used[usize::from(*k)])
                    .collect();
                if keys.is_empty() {
                    return Self::all();
                }
                keys
            }
        };
        Self::from_keys(keys)
    }

    /// The number of rows.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Never true: a roll always has at least one row.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Whether every key is a row.
    pub fn is_full(&self) -> bool {
        self.keys.len() == 128
    }

    /// The visible keys, top row first.
    pub fn keys(&self) -> &[u8] {
        &self.keys
    }

    /// The key of a row (clamped to the first and last row).
    pub fn key_at_row(&self, row: i64) -> u8 {
        self.keys[row.clamp(0, self.keys.len() as i64 - 1) as usize]
    }

    /// The row of `key`, `None` when the fold hides it.
    pub fn row_of(&self, key: u8) -> Option<usize> {
        usize::try_from(self.row[usize::from(key.min(127))]).ok()
    }

    /// The row of `key`, or of the nearest visible key (ties go to the lower key).
    pub fn nearest_row(&self, key: u8) -> usize {
        if let Some(r) = self.row_of(key) {
            return r;
        }
        let mut best = (u16::MAX, 0usize);
        for (r, k) in self.keys.iter().enumerate() {
            let d = u16::from(k.abs_diff(key));
            // Keys run downwards, so a later row is a lower key: on a tie it wins.
            if d <= best.0 {
                best = (d, r);
            }
        }
        best.1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_model::theory::ScaleKind;

    #[test]
    fn the_full_roll_has_a_row_per_key_with_127_on_top() {
        let rows = KeyRows::all();
        assert_eq!(rows.len(), 128);
        assert!(rows.is_full() && !rows.is_empty());
        for k in 0..=127u8 {
            assert_eq!(rows.row_of(k), Some(usize::from(127 - k)));
            assert_eq!(rows.key_at_row(i64::from(127 - k)), k);
        }
        assert_eq!(rows.key_at_row(-5), 127);
        assert_eq!(rows.key_at_row(500), 0);
    }

    #[test]
    fn fold_off_and_folds_with_nothing_to_fold_to_show_every_key() {
        let full = KeyRows::all();
        assert_eq!(KeyRows::build(Fold::Off, None, &[60]), full);
        assert_eq!(KeyRows::build(Fold::Scale, None, &[60]), full);
        let chroma = Some(Scale::new(0, ScaleKind::Chromatic));
        assert_eq!(KeyRows::build(Fold::Scale, chroma, &[60]), full);
        assert_eq!(KeyRows::build(Fold::Used, None, &[]), full);
    }

    #[test]
    fn fold_to_scale_shows_the_scale_keys_and_keeps_used_keys_visible() {
        let c_major = Some(Scale::new(0, ScaleKind::Major));
        let rows = KeyRows::build(Fold::Scale, c_major, &[]);
        assert_eq!(rows.len(), 75);
        assert!(rows.keys().windows(2).all(|w| w[0] > w[1]), "descending");
        assert_eq!(rows.row_of(61), None);
        assert_eq!(rows.row_of(127), Some(0));
        assert_eq!(rows.row_of(0), Some(74));
        // A note on C# stays visible, as an extra row between C and D.
        let rows = KeyRows::build(Fold::Scale, c_major, &[61, 61]);
        assert_eq!(rows.len(), 76);
        let r = rows.row_of(61).unwrap();
        assert_eq!(rows.key_at_row(r as i64 - 1), 62);
        assert_eq!(rows.key_at_row(r as i64 + 1), 60);
    }

    #[test]
    fn fold_to_used_shows_only_the_used_keys_once() {
        let rows = KeyRows::build(Fold::Used, None, &[64, 60, 64, 67, 60]);
        assert_eq!(rows.keys(), [67, 64, 60]);
        assert_eq!(rows.row_of(64), Some(1));
        assert_eq!(rows.row_of(65), None);
        assert_eq!(rows.key_at_row(0), 67);
        assert_eq!(rows.key_at_row(2), 60);
        assert_eq!(rows.key_at_row(9), 60, "clamped to the last row");
        let one = KeyRows::build(Fold::Used, None, &[0]);
        assert_eq!(one.keys(), [0]);
        let top = KeyRows::build(Fold::Used, None, &[127, 200]);
        assert_eq!(
            top.keys(),
            [127],
            "keys above 127 are clamped to the last key"
        );
    }

    #[test]
    fn nearest_row_of_a_hidden_key_is_the_closest_visible_one_ties_down() {
        let rows = KeyRows::build(Fold::Used, None, &[60, 64]);
        assert_eq!(rows.keys(), [64, 60]);
        assert_eq!(
            rows.key_at_row(rows.nearest_row(62) as i64),
            60,
            "62 is 2 from both: down"
        );
        assert_eq!(rows.key_at_row(rows.nearest_row(63) as i64), 64);
        assert_eq!(rows.key_at_row(rows.nearest_row(10) as i64), 60);
        assert_eq!(rows.key_at_row(rows.nearest_row(120) as i64), 64);
        assert_eq!(rows.nearest_row(64), 0);
    }

    #[test]
    fn fold_ids_round_trip() {
        for f in Fold::ALL {
            assert_eq!(Fold::from_id(f.id()), Some(f));
            assert!(!f.label().is_empty());
        }
        assert_eq!(Fold::from_id("nope"), None);
        assert_eq!(Fold::default(), Fold::Off);
    }
}
