//! Sampled instruments (SFZ, SoundFont) as sampler zones.
//!
//! [`atomik_audio_file`] reads an instrument file into an [`Instrument`]: regions that name
//! samples. This module does the rest off the UI thread ([`load_instrument`]): every sample is
//! converted to the session rate and stored in the project's bundle (an SFZ names files, a
//! SoundFont holds its samples), and the regions become [`ZoneDoc`]s ([`zones_from`]). Frame
//! positions in an instrument are in the source sample's own rate, so they scale with the
//! length the stored asset ended up with.
//!
//! A region that cannot be made valid is left out and counted, never a failure; the
//! instrument's own warnings (opcodes it does not know, ...) travel along for one message. The
//! UI thread then registers the assets and replaces the sampler's zones in one undo step, see
//! `app_core_instrument.rs`.

use crate::audio_import::{Loaded, load_asset};
use atomik_audio_file::{
    Bundle, Instrument, InstrumentLimits, InstrumentRegion, SampleSource, read_instrument,
};
use atomik_device_sdk::{
    LoopMode, MAX_RR_LEN, MAX_VEL_XFADE, MAX_ZONE_GROUPS, SampleBuffer, SampleMap, TriggerMode,
    ZONE_CUTOFF_RANGE, ZONE_ENV_MS, ZONE_GAIN_RANGE, ZONE_TRANSPOSE_RANGE, ZONE_TUNE_RANGE,
    ZoneEnv, ZoneExt, ZoneFilter,
};
use atomik_model::ZoneDoc;
use std::path::Path;
use std::sync::Arc;

/// An instrument imported into the bundle, ready for the UI thread.
pub(crate) struct ImportedInstrument {
    /// The instrument's name.
    pub name: String,
    /// The stored samples, one per distinct asset, ready for the engine and the views.
    pub loaded: Vec<Loaded>,
    /// The zones, naming the assets of `loaded`.
    pub zones: Vec<ZoneDoc>,
    /// What was left out or changed, for one message.
    pub warnings: Vec<String>,
}

/// A sample of an instrument once it is stored.
#[derive(Debug, Clone)]
pub(crate) struct Stored {
    /// Asset id.
    pub hash: String,
    /// Frames of the stored asset.
    pub frames: u64,
    /// Channels of the stored asset.
    pub channels: u16,
    /// Frames the sample had in its source, before the conversion to the session rate.
    pub original_frames: u64,
}

/// Reads the instrument file at `path`, stores its samples in `bundle` at `rate` and builds the
/// zones.
pub(crate) fn load_instrument(
    bundle: &mut Bundle,
    path: &Path,
    rate: u32,
    threshold: u64,
) -> Result<ImportedInstrument, String> {
    let instrument =
        read_instrument(path, InstrumentLimits::default()).map_err(|e| e.to_string())?;
    let mut warnings = instrument.warnings.clone();
    let mut loaded: Vec<Loaded> = Vec::new();
    let mut stored: Vec<Option<Stored>> = Vec::with_capacity(instrument.samples.len());
    let mut failed: Vec<String> = Vec::new();
    for sample in &instrument.samples {
        let result = match &sample.source {
            SampleSource::File(file) => bundle.import_counted(file, rate),
            SampleSource::Embedded(data) => bundle.add_sample((**data).clone(), rate, &sample.name),
        };
        let (info, original_frames) = match result {
            Ok(r) => r,
            Err(e) => {
                failed.push(format!("{}: {e}", sample.name));
                stored.push(None);
                continue;
            }
        };
        stored.push(Some(Stored {
            hash: info.hash.clone(),
            frames: info.frames,
            channels: info.channels,
            original_frames,
        }));
        if !loaded.iter().any(|l| l.hash == info.hash) {
            let mut one = load_asset(bundle, &info.hash, threshold)?;
            one.info = Some(info);
            loaded.push(one);
        }
    }
    if !failed.is_empty() {
        const SHOWN: usize = 4;
        let more = failed.len().saturating_sub(SHOWN);
        let mut line = format!(
            "{} sample(s) could not be read: {}",
            failed.len(),
            failed
                .iter()
                .take(SHOWN)
                .cloned()
                .collect::<Vec<_>>()
                .join("; ")
        );
        if more > 0 {
            line.push_str(&format!(" (and {more} more)"));
        }
        warnings.push(line);
    }
    let (zones, mut notes) = zones_from(&instrument, &stored);
    warnings.append(&mut notes);
    if zones.is_empty() {
        return Err(if warnings.is_empty() {
            "no playable regions".to_owned()
        } else {
            format!("no playable regions ({})", warnings.join("; "))
        });
    }
    Ok(ImportedInstrument {
        name: instrument.name,
        loaded,
        zones,
        warnings,
    })
}

/// The zones of `instrument`'s regions; `stored[i]` is sample `i` once stored (`None`: it could
/// not be read, so its regions are left out). The second value is what was changed or left out,
/// as messages.
pub(crate) fn zones_from(
    instrument: &Instrument,
    stored: &[Option<Stored>],
) -> (Vec<ZoneDoc>, Vec<String>) {
    let mut zones = Vec::with_capacity(instrument.regions.len());
    let mut counts = Counts::default();
    for region in &instrument.regions {
        let Some(Some(sample)) = stored.get(region.sample) else {
            counts.unreadable += 1;
            continue;
        };
        match zone_of(region, sample, &mut counts) {
            Some(zone) => zones.push(zone),
            None => counts.invalid += 1,
        }
    }
    (zones, counts.messages())
}

#[derive(Default)]
struct Counts {
    unreadable: usize,
    invalid: usize,
    tuning: usize,
    groups: usize,
    loops: usize,
}

impl Counts {
    fn messages(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut line = |n: usize, text: &str| {
            if n > 0 {
                out.push(format!("{n} region(s) {text}"));
            }
        };
        line(self.unreadable, "left out: their sample could not be read");
        line(self.invalid, "left out: they do not fit a zone");
        line(self.tuning, "tuned beyond four octaves: clamped");
        line(
            self.loops,
            "had loop points outside the sample: played without a loop",
        );
        line(
            self.groups,
            &format!("share a group with others: more than {MAX_ZONE_GROUPS} groups"),
        );
        out
    }
}

/// One region as a zone for the stored `sample`, `None` when it cannot be a valid zone.
fn zone_of(region: &InstrumentRegion, sample: &Stored, counts: &mut Counts) -> Option<ZoneDoc> {
    let frames = sample.frames;
    if frames == 0 || sample.original_frames == 0 {
        return None;
    }
    // Positions are in frames of the source: they scale with the conversion to the session rate.
    let scale = frames as f64 / sample.original_frames as f64;
    let at = |p: u64| -> u64 {
        if p == u64::MAX {
            frames
        } else {
            ((p as f64 * scale).round() as u64).min(frames)
        }
    };
    let mut zone = ZoneDoc::new(sample.hash.clone(), frames);
    zone.key_lo = region.key_lo.min(127);
    zone.key_hi = region.key_hi.min(127);
    zone.vel_lo = region.vel_lo.clamp(1, 127);
    zone.vel_hi = region.vel_hi.clamp(zone.vel_lo, 127);
    zone.root_key = region.root_key.min(127);
    // Whole semitones into the transpose, the rest into the fine tune.
    let semis = (region.tune_cents / 100.0).round();
    let limit = f64::from(ZONE_TRANSPOSE_RANGE.1);
    if semis.abs() > limit {
        counts.tuning += 1;
    }
    let semis = semis.clamp(-limit, limit);
    zone.transpose = semis as i8;
    zone.tune_cents =
        ((region.tune_cents - semis * 100.0) as f32).clamp(ZONE_TUNE_RANGE.0, ZONE_TUNE_RANGE.1);
    zone.gain_db = region.gain_db.clamp(ZONE_GAIN_RANGE.0, ZONE_GAIN_RANGE.1);
    zone.pan = region.pan.clamp(-1.0, 1.0);
    zone.start = at(region.start);
    zone.end = region.end.map_or(frames, at).min(frames);
    if zone.end <= zone.start {
        return None;
    }
    zone.reverse = region.reverse;
    zone.trigger = if region.one_shot {
        TriggerMode::OneShot
    } else {
        TriggerMode::Gate
    };
    if let Some((ls, le)) = region.loop_range
        && !region.reverse
    {
        let (ls, le) = (at(ls).max(zone.start), at(le).min(zone.end));
        if ls < le {
            zone.loop_mode = LoopMode::Forward;
            zone.loop_start = ls;
            zone.loop_end = le;
        } else {
            counts.loops += 1;
        }
    }
    let group = if region.group == 0 {
        0
    } else {
        if region.group as usize >= MAX_ZONE_GROUPS {
            counts.groups += 1;
        }
        // Groups 1.. share the ids 1..MAX_ZONE_GROUPS; 0 is the default group.
        ((region.group as usize - 1) % (MAX_ZONE_GROUPS - 1) + 1) as u8
    };
    let round_robin = region
        .round_robin
        .filter(|(len, pos)| (2..=MAX_RR_LEN).contains(len) && pos < len);
    let ms = |s: f32| (s * 1000.0).clamp(ZONE_ENV_MS.0, ZONE_ENV_MS.1);
    zone.ext = ZoneExt {
        group,
        vel_xfade: region
            .xfade
            .min(MAX_VEL_XFADE)
            .min(zone.vel_hi - zone.vel_lo),
        rr_len: round_robin.map_or(0, |(len, _)| len),
        rr_pos: round_robin.map_or(0, |(_, pos)| pos),
        env: region.env.map(|e| ZoneEnv {
            attack_ms: ms(e.attack),
            decay_ms: ms(e.decay),
            sustain: e.sustain.clamp(0.0, 1.0),
            release_ms: ms(e.release),
        }),
        filter: region.filter.map(|f| ZoneFilter {
            mode: f.mode.min(2),
            cutoff: f.cutoff.clamp(ZONE_CUTOFF_RANGE.0, ZONE_CUTOFF_RANGE.1),
            resonance: f.resonance.clamp(0.0, 1.0),
            key_track: f.key_track.clamp(0.0, 1.0),
            env_amount: f.env_amount.clamp(-1.0, 1.0),
        }),
    };
    // The SDK's own rules, so what the map builder accepts is exactly what is stored.
    let data: Arc<[Vec<f32>]> = Arc::new([Vec::new()]);
    let buffer = Arc::new(SampleBuffer {
        data,
        frames,
        channels: sample.channels,
    });
    SampleMap::new(vec![buffer], vec![zone.to_zone(0)]).ok()?;
    Some(zone)
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_audio_file::{InstrumentSample, RegionEnv, RegionFilter};

    fn region(sample: usize) -> InstrumentRegion {
        InstrumentRegion {
            sample,
            key_lo: 0,
            key_hi: 127,
            vel_lo: 1,
            vel_hi: 127,
            root_key: 60,
            tune_cents: 0.0,
            gain_db: 0.0,
            pan: 0.0,
            start: 0,
            end: None,
            loop_range: None,
            one_shot: false,
            reverse: false,
            group: 0,
            xfade: 0,
            round_robin: None,
            env: None,
            filter: None,
        }
    }

    fn stored(frames: u64, original: u64) -> Option<Stored> {
        Some(Stored {
            hash: "a".repeat(64),
            frames,
            channels: 1,
            original_frames: original,
        })
    }

    fn instrument(regions: Vec<InstrumentRegion>) -> Instrument {
        Instrument {
            name: "t".into(),
            samples: vec![InstrumentSample {
                name: "s".into(),
                source: SampleSource::File("s.wav".into()),
            }],
            regions,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn positions_scale_with_the_converted_length() {
        let mut r = region(0);
        r.start = 100;
        r.end = Some(500);
        r.loop_range = Some((200, 400));
        let (zones, notes) = zones_from(&instrument(vec![r]), &[stored(2000, 1000)]);
        assert!(notes.is_empty(), "{notes:?}");
        let z = &zones[0];
        assert_eq!((z.start, z.end), (200, 1000));
        assert_eq!(z.loop_mode, LoopMode::Forward);
        assert_eq!((z.loop_start, z.loop_end), (400, 800));
    }

    #[test]
    fn an_open_loop_end_runs_to_the_region_end() {
        let mut r = region(0);
        r.end = Some(800);
        r.loop_range = Some((100, u64::MAX));
        let (zones, _) = zones_from(&instrument(vec![r]), &[stored(1000, 1000)]);
        assert_eq!((zones[0].loop_start, zones[0].loop_end), (100, 800));
    }

    #[test]
    fn tuning_splits_into_semitones_and_cents_and_clamps() {
        let mut a = region(0);
        a.tune_cents = 1_270.0;
        let mut b = region(0);
        b.tune_cents = -10_000.0;
        let (zones, notes) = zones_from(&instrument(vec![a, b]), &[stored(10, 10)]);
        assert_eq!(zones[0].transpose, 13);
        assert!((zones[0].tune_cents + 30.0).abs() < 1e-3);
        assert_eq!(zones[1].transpose, -48);
        assert!(notes.iter().any(|n| n.contains("tuned")), "{notes:?}");
    }

    #[test]
    fn groups_round_robin_crossfade_envelope_and_filter_carry_over() {
        let mut r = region(0);
        r.vel_lo = 20;
        r.vel_hi = 60;
        r.xfade = 200;
        r.group = 2;
        r.round_robin = Some((4, 1));
        r.env = Some(RegionEnv {
            attack: 0.0,
            decay: 100.0,
            sustain: 0.5,
            release: 0.25,
        });
        r.filter = Some(RegionFilter {
            mode: 2,
            cutoff: 5.0,
            resonance: 0.3,
            key_track: 0.5,
            env_amount: 0.1,
        });
        let (zones, notes) = zones_from(&instrument(vec![r]), &[stored(10, 10)]);
        assert!(notes.is_empty(), "{notes:?}");
        let e = zones[0].ext;
        assert_eq!((e.group, e.vel_xfade, e.rr_len, e.rr_pos), (2, 40, 4, 1));
        let env = e.env.expect("env");
        assert!((env.attack_ms - ZONE_ENV_MS.0).abs() < 1e-6);
        assert!((env.decay_ms - ZONE_ENV_MS.1).abs() < 1e-6);
        assert!((env.release_ms - 250.0).abs() < 1e-3);
        let f = e.filter.expect("filter");
        assert_eq!(f.mode, 2);
        assert!((f.cutoff - ZONE_CUTOFF_RANGE.0).abs() < 1e-6);
    }

    #[test]
    fn many_groups_share_ids_and_say_so() {
        let regions: Vec<InstrumentRegion> = (1..=40)
            .map(|g| {
                let mut r = region(0);
                r.group = g;
                r
            })
            .collect();
        let (zones, notes) = zones_from(&instrument(regions), &[stored(10, 10)]);
        assert_eq!(zones.len(), 40);
        assert!(
            zones
                .iter()
                .all(|z| usize::from(z.ext.group) < MAX_ZONE_GROUPS)
        );
        assert!(zones.iter().all(|z| z.ext.group >= 1));
        assert!(notes.iter().any(|n| n.contains("group")), "{notes:?}");
    }

    #[test]
    fn bad_loops_empty_ranges_and_missing_samples_are_left_out_or_repaired() {
        let mut bad_loop = region(0);
        bad_loop.loop_range = Some((900, 950));
        bad_loop.end = Some(100);
        let mut empty = region(0);
        empty.start = 500;
        empty.end = Some(500);
        let missing = region(1);
        let mut inst = instrument(vec![bad_loop, empty, missing]);
        inst.samples.push(inst.samples[0].clone());
        let (zones, notes) = zones_from(&inst, &[stored(1000, 1000), None]);
        assert_eq!(zones.len(), 1);
        assert_eq!(zones[0].loop_mode, LoopMode::Off);
        assert!(notes.iter().any(|n| n.contains("loop points")));
        assert!(notes.iter().any(|n| n.contains("do not fit")));
        assert!(notes.iter().any(|n| n.contains("could not be read")));
    }

    #[test]
    fn a_one_shot_and_a_reversed_region_keep_their_flags() {
        let mut a = region(0);
        a.one_shot = true;
        let mut b = region(0);
        b.reverse = true;
        b.loop_range = Some((1, 5));
        let (zones, _) = zones_from(&instrument(vec![a, b]), &[stored(10, 10)]);
        assert_eq!(zones[0].trigger, TriggerMode::OneShot);
        assert!(zones[1].reverse);
        assert_eq!(zones[1].loop_mode, LoopMode::Off);
    }
}
