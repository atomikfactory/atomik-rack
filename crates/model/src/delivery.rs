//! Delivery targets: the loudness and true-peak limits a finished master is judged against.
//!
//! A delivery target is a named pair, an integrated loudness in LUFS and a true-peak ceiling in
//! dBTP. [`TARGETS`] lists the usual ones under generic names (no service is named: a streaming
//! target is "Streaming -14 LUFS / -1 dBTP", a broadcast one "Broadcast -23 LUFS / -1 dBTP").
//! [`assess`] says what a playback chain that normalises to a target would do to a master of a
//! measured loudness and peak:
//!
//! * louder than the target: the chain turns it **down** by the difference; the waveform is
//!   untouched and nothing is lost;
//! * about as loud: nothing happens;
//! * quieter than the target and with room under the ceiling: it is turned **up** and stays
//!   under the ceiling;
//! * quieter than the target with peaks that the extra gain would push past the ceiling (a quiet
//!   master whose peaks stand near 0 dBFS): the chain turns it up **into a limiter**, which
//!   treats the peaks outside the user's control. This is the case worth avoiding.
//!
//! # The project's choice
//!
//! [`RackDoc::delivery`] holds the target the user works to, for the export's loudness report to
//! compare against. It is `None` for a new project, saved as the root field `"delivery"` only
//! when set (project format 44, ignorable) and edited with [`crate::Command::SetDelivery`], one
//! undo step that reports [`crate::SyncOp::ViewChanged`]; the Delivery panel's Set Ceiling batches
//! it with the ceiling of the master's limiter, so the two are one step. The name is one line of
//! at most [`MAX_DELIVERY_NAME_CHARS`] characters without control characters; the loudness is
//! within [`LUFS_RANGE`] and the ceiling within [`CEILING_RANGE`].

use crate::history::{Edit, Swap};
use crate::patch::{JsonPath, f32_text, json_str, number, object, string};
use crate::{ModelError, RackDoc};
use serde_json::Value;
use std::fmt::Write as _;
use std::ops::RangeInclusive;

/// Longest name of a stored delivery target, in characters.
pub const MAX_DELIVERY_NAME_CHARS: usize = 64;
/// The integrated loudness a target may ask for, in LUFS.
pub const LUFS_RANGE: RangeInclusive<f32> = -50.0..=0.0;
/// The true-peak ceiling a target may ask for, in dBTP.
pub const CEILING_RANGE: RangeInclusive<f32> = -20.0..=0.0;
/// A difference smaller than this (in dB) is no change at all.
pub const UNCHANGED_DB: f32 = 0.05;

const FIELDS: &[&str] = &["name", "lufs", "ceiling"];

/// One entry of the table of common targets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeliveryTarget {
    /// The generic name, with its numbers, as the panel lists it.
    pub name: &'static str,
    /// The integrated loudness it normalises to, in LUFS.
    pub lufs: f32,
    /// The true-peak ceiling it wants, in dBTP.
    pub ceiling_dbtp: f32,
}

/// The common streaming, podcast, broadcast and club targets, quietest last.
pub const TARGETS: [DeliveryTarget; 8] = [
    DeliveryTarget {
        name: "Streaming loud -11 LUFS / -1 dBTP",
        lufs: -11.0,
        ceiling_dbtp: -1.0,
    },
    DeliveryTarget {
        name: "Streaming -14 LUFS / -1 dBTP",
        lufs: -14.0,
        ceiling_dbtp: -1.0,
    },
    DeliveryTarget {
        name: "Streaming quiet -16 LUFS / -1 dBTP",
        lufs: -16.0,
        ceiling_dbtp: -1.0,
    },
    DeliveryTarget {
        name: "Podcast -16 LUFS / -1 dBTP",
        lufs: -16.0,
        ceiling_dbtp: -1.0,
    },
    DeliveryTarget {
        name: "Broadcast -23 LUFS / -1 dBTP",
        lufs: -23.0,
        ceiling_dbtp: -1.0,
    },
    DeliveryTarget {
        name: "Broadcast -24 LUFS / -2 dBTP",
        lufs: -24.0,
        ceiling_dbtp: -2.0,
    },
    DeliveryTarget {
        name: "Club -9 LUFS / -0.5 dBTP",
        lufs: -9.0,
        ceiling_dbtp: -0.5,
    },
    DeliveryTarget {
        name: "Film mix -27 LUFS / -2 dBTP",
        lufs: -27.0,
        ceiling_dbtp: -2.0,
    },
];

/// What a normalising chain does to a master.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The master is as loud as the target.
    Unchanged,
    /// The master is louder: it is turned down, nothing is lost.
    TurnedDown,
    /// The master is quieter and its peaks stay under the ceiling after the gain.
    TurnedUp,
    /// The master is quieter and the gain would push its peaks past the ceiling: a limiter would
    /// act on it.
    IntoLimiter,
}

/// A target applied to a measured master.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Assessment {
    /// The gain the chain applies, in dB (negative turns down).
    pub gain_db: f32,
    /// The true peak after that gain, in dBTP (`None` without a measured peak).
    pub peak_after_dbtp: Option<f32>,
    /// What the gain amounts to.
    pub verdict: Verdict,
}

impl Assessment {
    /// How far the peaks would pass the ceiling after the gain, in dB; 0 when they stay under it.
    pub fn over_dbtp(&self, target: &DeliveryTarget) -> f32 {
        self.peak_after_dbtp
            .map_or(0.0, |p| (p - target.ceiling_dbtp).max(0.0))
    }

    /// One line on it, for the panel: the gain and, for a limiter, how much it would take.
    pub fn sentence(&self, target: &DeliveryTarget) -> String {
        match self.verdict {
            Verdict::Unchanged => "no change".to_owned(),
            Verdict::TurnedDown => format!("turned down {:.1} dB", -self.gain_db),
            Verdict::TurnedUp => format!("turned up {:.1} dB, peaks stay under", self.gain_db),
            Verdict::IntoLimiter => format!(
                "turned up {:.1} dB into its limiter ({:.1} dB past the ceiling)",
                self.gain_db,
                self.over_dbtp(target)
            ),
        }
    }
}

/// Applies `target` to a master of integrated loudness `lufs` and true peak `peak_dbtp`; `None`
/// when the master has no loudness (it is silent).
pub fn assess(
    target: &DeliveryTarget,
    lufs: Option<f32>,
    peak_dbtp: Option<f32>,
) -> Option<Assessment> {
    let gain_db = target.lufs - lufs?;
    let peak_after_dbtp = peak_dbtp.map(|p| p + gain_db);
    let verdict = if gain_db.abs() < UNCHANGED_DB {
        Verdict::Unchanged
    } else if gain_db < 0.0 {
        Verdict::TurnedDown
    } else if peak_after_dbtp.is_some_and(|p| p > target.ceiling_dbtp + UNCHANGED_DB) {
        Verdict::IntoLimiter
    } else {
        Verdict::TurnedUp
    };
    Some(Assessment {
        gain_db,
        peak_after_dbtp,
        verdict,
    })
}

/// The target a project works to (see the module docs).
#[derive(Debug, Clone, PartialEq)]
pub struct Delivery {
    /// The name shown in the panel and in the export's report.
    pub name: String,
    /// The integrated loudness aimed for, in LUFS.
    pub lufs: f32,
    /// The true-peak ceiling, in dBTP.
    pub ceiling_dbtp: f32,
}

impl Delivery {
    /// The project's choice for an entry of [`TARGETS`].
    pub fn of(target: &DeliveryTarget) -> Self {
        Self {
            name: target.name.to_owned(),
            lufs: target.lufs,
            ceiling_dbtp: target.ceiling_dbtp,
        }
    }

    /// The target as a table entry's numbers.
    pub fn as_target(&self) -> DeliveryTarget {
        DeliveryTarget {
            name: "",
            lufs: self.lufs,
            ceiling_dbtp: self.ceiling_dbtp,
        }
    }

    /// How a measured export compares, for the report: the loudness off the target and the
    /// true peak against the ceiling, as `-14.2 LUFS (0.2 LU under "Streaming ..."), peak 0.3 dB
    /// over the ceiling`.
    pub fn compare(&self, lufs: Option<f32>, peak_dbtp: Option<f32>) -> String {
        let mut parts = Vec::new();
        if let Some(l) = lufs {
            let off = l - self.lufs;
            parts.push(if off.abs() < UNCHANGED_DB {
                "on the loudness target".to_owned()
            } else if off > 0.0 {
                format!("{off:.1} LU over the loudness target")
            } else {
                format!("{:.1} LU under the loudness target", -off)
            });
        }
        if let Some(p) = peak_dbtp {
            let over = p - self.ceiling_dbtp;
            parts.push(if over > UNCHANGED_DB {
                format!("peak {over:.1} dB over the ceiling")
            } else {
                "peak under the ceiling".to_owned()
            });
        }
        format!("{}: {}", self.name, parts.join(", "))
    }
}

/// Checks a stored target against the limits.
///
/// # Errors
/// [`ModelError::OutOfRange`] naming the first value that breaks one.
pub fn check_delivery(d: &Delivery) -> Result<(), ModelError> {
    check_one(d).map_err(|m| ModelError::OutOfRange(format!("delivery target: {m}")))
}

fn check_one(d: &Delivery) -> Result<(), String> {
    let chars = d.name.chars().count();
    if chars == 0 {
        return Err("the name is empty".into());
    }
    if chars > MAX_DELIVERY_NAME_CHARS {
        return Err(format!(
            "the name is {chars} characters, at most {MAX_DELIVERY_NAME_CHARS} are allowed"
        ));
    }
    if d.name.chars().any(char::is_control) {
        return Err("the name must not hold control characters".into());
    }
    if !d.lufs.is_finite() || !LUFS_RANGE.contains(&d.lufs) {
        return Err(format!(
            "{} LUFS is outside {}..={}",
            d.lufs,
            LUFS_RANGE.start(),
            LUFS_RANGE.end()
        ));
    }
    if !d.ceiling_dbtp.is_finite() || !CEILING_RANGE.contains(&d.ceiling_dbtp) {
        return Err(format!(
            "{} dBTP is outside {}..={}",
            d.ceiling_dbtp,
            CEILING_RANGE.start(),
            CEILING_RANGE.end()
        ));
    }
    Ok(())
}

/// Plans `Command::SetDelivery`: the target must be within the limits and differ from the
/// current one.
pub(crate) fn plan_set(doc: &RackDoc, new: &Option<Delivery>) -> Result<Edit, ModelError> {
    if let Some(d) = new {
        check_delivery(d)?;
    }
    if &doc.delivery == new {
        return Err(ModelError::BadScope(
            "the delivery target is already that".into(),
        ));
    }
    Ok(Edit::Swap(Swap::Delivery {
        old: doc.delivery.clone(),
        new: new.clone(),
    }))
}

/// Writes `"delivery"` (with its trailing comma and newline) after `"references"`; nothing when
/// no target is set.
pub(crate) fn write_delivery(w: &mut String, delivery: &Option<Delivery>) {
    if let Some(d) = delivery {
        let _ = writeln!(
            w,
            "  \"delivery\": {{\"name\": {}, \"lufs\": {}, \"ceiling\": {}}},",
            json_str(&d.name),
            f32_text(d.lufs),
            f32_text(d.ceiling_dbtp)
        );
    }
}

/// Reads the project's `"delivery"` object.
pub(crate) fn load_delivery(v: &Value, path: &JsonPath) -> Result<Delivery, ModelError> {
    let o = object(v, path, FIELDS, FIELDS)?;
    let d = Delivery {
        name: string(&o["name"], &path.field("name"))?.to_owned(),
        lufs: number(&o["lufs"], &path.field("lufs"))? as f32,
        ceiling_dbtp: number(&o["ceiling"], &path.field("ceiling"))? as f32,
    };
    check_one(&d).map_err(|m| path.error(m))?;
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(lufs: f32, ceiling: f32) -> DeliveryTarget {
        DeliveryTarget {
            name: "t",
            lufs,
            ceiling_dbtp: ceiling,
        }
    }

    #[test]
    fn the_table_is_generic_and_within_the_limits() {
        for t in &TARGETS {
            assert!(LUFS_RANGE.contains(&t.lufs) && CEILING_RANGE.contains(&t.ceiling_dbtp));
            assert!(check_delivery(&Delivery::of(t)).is_ok(), "{}", t.name);
            assert!(
                t.name.contains("LUFS") && t.name.contains("dBTP"),
                "{}",
                t.name
            );
        }
        let names: Vec<&str> = TARGETS.iter().map(|t| t.name).collect();
        for (i, a) in names.iter().enumerate() {
            assert!(!names[..i].contains(a), "{a} is listed twice");
        }
    }

    #[test]
    fn a_louder_master_is_turned_down_without_loss() {
        let a = assess(&target(-14.0, -1.0), Some(-9.0), Some(-0.3)).unwrap();
        assert_eq!(a.verdict, Verdict::TurnedDown);
        assert!((a.gain_db + 5.0).abs() < 1e-5);
        assert!((a.peak_after_dbtp.unwrap() + 5.3).abs() < 1e-5);
        assert_eq!(a.sentence(&target(-14.0, -1.0)), "turned down 5.0 dB");
    }

    #[test]
    fn a_quiet_master_with_hot_peaks_is_turned_up_into_the_limiter() {
        let t = target(-14.0, -1.0);
        let a = assess(&t, Some(-18.0), Some(-0.2)).unwrap();
        assert_eq!(a.verdict, Verdict::IntoLimiter);
        assert!((a.gain_db - 4.0).abs() < 1e-5);
        assert!((a.over_dbtp(&t) - 4.8).abs() < 1e-4);
        assert!(a.sentence(&t).contains("limiter"));
        // The same loudness with headroom stays under the ceiling.
        let ok = assess(&t, Some(-18.0), Some(-6.0)).unwrap();
        assert_eq!(ok.verdict, Verdict::TurnedUp);
        assert_eq!(ok.over_dbtp(&t), 0.0);
    }

    #[test]
    fn a_master_on_target_is_left_alone_and_silence_has_no_verdict() {
        let t = target(-14.0, -1.0);
        let a = assess(&t, Some(-14.02), Some(-0.5)).unwrap();
        assert_eq!(a.verdict, Verdict::Unchanged);
        assert!(assess(&t, None, None).is_none());
        // Without a measured peak a quiet master is only turned up.
        assert_eq!(
            assess(&t, Some(-20.0), None).unwrap().verdict,
            Verdict::TurnedUp
        );
    }

    #[test]
    fn the_comparison_names_the_loudness_and_the_peak() {
        let d = Delivery::of(&TARGETS[1]);
        let line = d.compare(Some(-14.2), Some(-0.4));
        assert!(line.starts_with("Streaming -14 LUFS / -1 dBTP: "), "{line}");
        assert!(line.contains("0.2 LU under"), "{line}");
        assert!(line.contains("0.6 dB over the ceiling"), "{line}");
        assert!(d.compare(Some(-14.0), Some(-2.0)).contains("peak under"));
    }

    #[test]
    fn limits_are_enforced() {
        let mut d = Delivery::of(&TARGETS[0]);
        assert!(check_delivery(&d).is_ok());
        d.name.clear();
        assert!(check_delivery(&d).is_err());
        d.name = "a\nb".into();
        assert!(check_delivery(&d).is_err());
        d.name = "x".repeat(MAX_DELIVERY_NAME_CHARS + 1);
        assert!(check_delivery(&d).is_err());
        d.name = "ok".into();
        d.lufs = 3.0;
        assert!(check_delivery(&d).is_err());
        d.lufs = -14.0;
        d.ceiling_dbtp = f32::NAN;
        assert!(check_delivery(&d).is_err());
    }

    #[test]
    fn writing_and_reading_round_trip() {
        let d = Delivery {
            name: "My \"mix\" target".into(),
            lufs: -13.5,
            ceiling_dbtp: -1.5,
        };
        let mut text = String::from("{\n");
        write_delivery(&mut text, &Some(d.clone()));
        text.push_str("  \"end\": 0\n}");
        let value: Value = serde_json::from_str(&text).unwrap();
        let back = load_delivery(&value["delivery"], &JsonPath::lazy()).unwrap();
        assert_eq!(back, d);
        let mut empty = String::new();
        write_delivery(&mut empty, &None);
        assert!(empty.is_empty());
    }
}
