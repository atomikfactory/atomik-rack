//! SoundFont 2 files: samples and their mapping in one RIFF file.
//!
//! One preset is read (the first, or the one asked for): its zones are walked through the
//! instruments they use, and each zone that plays a sample becomes a region. Read: key and
//! velocity ranges, the root key (`overridingRootKey`, else the sample's own), coarse and fine
//! tune plus the sample's pitch correction, attenuation, pan, the start/end and loop offsets,
//! the loop mode, the volume envelope (attack, decay, sustain, release) and the low-pass filter
//! (cutoff, Q). Generators of the preset add to those of the instrument, ranges intersect.
//! Modulators, the hold stage, the modulation envelope, LFOs, effects sends and `ROM` samples
//! are not used. A stereo sample is two mono samples, each placed by its own pan.
//!
//! Reading is bounded and never panics: every chunk, record and sample index is checked against
//! the file, a record that points outside it is skipped, and the file size is capped
//! ([`InstrumentLimits`]).

use crate::data::AudioData;
use crate::error::{AudioFileError, Result};
use crate::instrument::{
    Instrument, InstrumentLimits, InstrumentRegion, InstrumentSample, RegionEnv, RegionFilter,
    SampleSource,
};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Reads the first preset of the SoundFont at `path`.
///
/// # Errors
/// The file cannot be read, is over the size limit, or is not a SoundFont 2.
pub fn read_sf2(path: &Path, limits: InstrumentLimits) -> Result<Instrument> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(limits.max_file_bytes.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limits.max_file_bytes {
        return Err(AudioFileError::Unsupported(format!(
            "SoundFont too large (more than {} bytes)",
            limits.max_file_bytes
        )));
    }
    parse_sf2(&bytes, 0, limits)
}

/// The names of the presets of a SoundFont, in file order.
///
/// # Errors
/// The bytes are not a SoundFont 2.
pub fn sf2_presets(bytes: &[u8]) -> Result<Vec<String>> {
    Ok(Sf2::open(bytes)?
        .phdr
        .iter()
        .map(|p| p.name.clone())
        .collect())
}

/// Parses SoundFont `bytes` and builds the instrument of preset number `preset` (in file order).
///
/// # Errors
/// The bytes are not a SoundFont 2, or `preset` does not exist.
pub fn parse_sf2(bytes: &[u8], preset: usize, limits: InstrumentLimits) -> Result<Instrument> {
    let sf = Sf2::open(bytes)?;
    let Some(p) = sf.phdr.get(preset) else {
        return Err(AudioFileError::InvalidData(format!(
            "the SoundFont has {} preset(s), not {}",
            sf.phdr.len(),
            preset + 1
        )));
    };
    let mut warnings = Vec::new();
    if sf.phdr.len() > 1 {
        warnings.push(format!(
            "the SoundFont has {} presets; only \"{}\" was imported",
            sf.phdr.len(),
            p.name
        ));
    }
    let mut out = Instrument {
        name: p.name.clone(),
        ..Instrument::default()
    };
    let mut by_sample: BTreeMap<usize, usize> = BTreeMap::new();
    let (mut rom, mut bad, mut over) = (0usize, 0usize, 0usize);
    for pz in sf.zones(preset, Level::Preset) {
        let Some(inst_no) = pz.instrument else {
            continue;
        };
        for iz in sf.zones(inst_no, Level::Instrument) {
            let Some(sample_id) = iz.sample else {
                continue;
            };
            let Some(gens) = combine(&iz, &pz) else {
                continue;
            };
            if out.regions.len() >= limits.max_regions {
                over += 1;
                continue;
            }
            let Some(hdr) = sf.shdr.get(sample_id) else {
                bad += 1;
                continue;
            };
            if hdr.kind & 0x8000 != 0 {
                rom += 1;
                continue;
            }
            let index = match by_sample.get(&sample_id) {
                Some(i) => *i,
                None => {
                    if out.samples.len() >= limits.max_samples {
                        over += 1;
                        continue;
                    }
                    let Some(data) = sf.sample_audio(hdr) else {
                        bad += 1;
                        continue;
                    };
                    out.samples.push(InstrumentSample {
                        name: hdr.name.clone(),
                        source: SampleSource::Embedded(Box::new(data)),
                    });
                    by_sample.insert(sample_id, out.samples.len() - 1);
                    out.samples.len() - 1
                }
            };
            out.regions.push(region(index, hdr, &gens));
        }
    }
    if rom > 0 {
        warnings.push(format!("{rom} region(s) left out: they use ROM samples"));
    }
    if bad > 0 {
        warnings.push(format!(
            "{bad} region(s) left out: their sample data is not in the file"
        ));
    }
    if over > 0 {
        warnings.push(format!(
            "{over} region(s) left out: more than {} regions or {} samples",
            limits.max_regions, limits.max_samples
        ));
    }
    if out.regions.is_empty() {
        warnings.push("the preset has no playable regions".to_owned());
    }
    out.warnings = warnings;
    Ok(out)
}

// ---------------------------------------------------------------------------- generators

const GEN_START: u16 = 0;
const GEN_END: u16 = 1;
const GEN_START_LOOP: u16 = 2;
const GEN_END_LOOP: u16 = 3;
const GEN_START_COARSE: u16 = 4;
const GEN_END_COARSE: u16 = 12;
const GEN_FILTER_FC: u16 = 8;
const GEN_FILTER_Q: u16 = 9;
const GEN_PAN: u16 = 17;
const GEN_ATTACK: u16 = 34;
const GEN_DECAY: u16 = 36;
const GEN_SUSTAIN: u16 = 37;
const GEN_RELEASE: u16 = 38;
const GEN_INSTRUMENT: u16 = 41;
const GEN_KEY_RANGE: u16 = 43;
const GEN_VEL_RANGE: u16 = 44;
const GEN_START_LOOP_COARSE: u16 = 45;
const GEN_END_LOOP_COARSE: u16 = 46;
const GEN_ATTENUATION: u16 = 48;
const GEN_COARSE_TUNE: u16 = 51;
const GEN_FINE_TUNE: u16 = 52;
const GEN_SAMPLE_ID: u16 = 53;
const GEN_SAMPLE_MODES: u16 = 54;
const GEN_ROOT_KEY: u16 = 58;

/// The generators of one zone.
#[derive(Debug, Clone, Default)]
struct Gens {
    values: BTreeMap<u16, i32>,
    key: Option<(u8, u8)>,
    vel: Option<(u8, u8)>,
}

impl Gens {
    fn get(&self, g: u16) -> Option<i32> {
        self.values.get(&g).copied()
    }

    /// `over` replaces what this holds.
    fn layer(&mut self, over: &Gens) {
        self.values
            .extend(over.values.iter().map(|(k, v)| (*k, *v)));
        self.key = over.key.or(self.key);
        self.vel = over.vel.or(self.vel);
    }
}

/// A zone of a preset or an instrument, with the global zone laid under it.
struct Zone {
    gens: Gens,
    instrument: Option<usize>,
    sample: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Level {
    Preset,
    Instrument,
}

/// The instrument zone's generators with the preset's added; `None` when the ranges do not
/// meet.
fn combine(inst: &Zone, preset: &Zone) -> Option<Gens> {
    let mut out = inst.gens.clone();
    for (&g, &v) in &preset.gens.values {
        // Offsets add; the ones a preset cannot change are not touched.
        let own = matches!(
            g,
            GEN_SAMPLE_MODES | GEN_ROOT_KEY | GEN_SAMPLE_ID | GEN_INSTRUMENT
        ) || matches!(
            g,
            GEN_START..=GEN_END_LOOP | GEN_START_COARSE | GEN_END_COARSE
        ) || matches!(g, GEN_START_LOOP_COARSE | GEN_END_LOOP_COARSE);
        if !own {
            let base = out.values.get(&g).copied().unwrap_or(default_of(g));
            out.values.insert(g, base.saturating_add(v));
        }
    }
    let meet = |a: Option<(u8, u8)>, b: Option<(u8, u8)>| -> Option<Option<(u8, u8)>> {
        match (a, b) {
            (Some((al, ah)), Some((bl, bh))) => {
                let (lo, hi) = (al.max(bl), ah.min(bh));
                (lo <= hi).then_some(Some((lo, hi)))
            }
            (x, None) | (None, x) => Some(x),
        }
    };
    out.key = meet(inst.gens.key, preset.gens.key)?;
    out.vel = meet(inst.gens.vel, preset.gens.vel)?;
    Some(out)
}

/// The value of a generator a zone does not set.
fn default_of(g: u16) -> i32 {
    match g {
        GEN_FILTER_FC => 13_500,
        GEN_ATTACK | GEN_DECAY | GEN_RELEASE | 33 | 35 | 25 | 26 | 27 | 28 | 29 | 30 => -12_000,
        _ => 0,
    }
}

fn region(sample: usize, hdr: &SampleHeader, g: &Gens) -> InstrumentRegion {
    let value = |id: u16| g.get(id).unwrap_or_else(|| default_of(id));
    let coarse = |lo: u16, hi: u16| i64::from(value(lo)) + 32_768 * i64::from(value(hi));
    let length = i64::from(hdr.end.saturating_sub(hdr.start));
    let offset = |base: i64, add: i64| u64::try_from((base + add).max(0)).unwrap_or(0);
    let start = offset(0, coarse(GEN_START, GEN_START_COARSE));
    let end = offset(length, coarse(GEN_END, GEN_END_COARSE));
    let loop_start = offset(
        i64::from(hdr.start_loop) - i64::from(hdr.start),
        coarse(GEN_START_LOOP, GEN_START_LOOP_COARSE),
    );
    let loop_end = offset(
        i64::from(hdr.end_loop) - i64::from(hdr.start),
        coarse(GEN_END_LOOP, GEN_END_LOOP_COARSE),
    );
    let looped = g.get(GEN_SAMPLE_MODES).unwrap_or(0) & 1 == 1;
    let root = g
        .get(GEN_ROOT_KEY)
        .and_then(|k| u8::try_from(k).ok())
        .filter(|k| *k <= 127)
        .unwrap_or(if hdr.pitch <= 127 { hdr.pitch } else { 60 });
    let (key_lo, key_hi) = g.key.unwrap_or((0, 127));
    let (vel_lo, vel_hi) = g.vel.unwrap_or((1, 127));
    let seconds = |tc: i32| 2f32.powf(tc as f32 / 1200.0);
    let has_env = [GEN_ATTACK, GEN_DECAY, GEN_SUSTAIN, GEN_RELEASE]
        .iter()
        .any(|k| g.get(*k).is_some());
    let env = has_env.then(|| RegionEnv {
        attack: seconds(value(GEN_ATTACK)),
        decay: seconds(value(GEN_DECAY)),
        // Centibels of attenuation below the peak.
        sustain: 10f32.powf(-(value(GEN_SUSTAIN).clamp(0, 1440) as f32) / 200.0),
        release: seconds(value(GEN_RELEASE)),
    });
    let cutoff = value(GEN_FILTER_FC);
    let filter = (g.get(GEN_FILTER_FC).is_some() && cutoff < 13_500).then(|| RegionFilter {
        mode: 0,
        cutoff: 8.176 * 2f32.powf(cutoff as f32 / 1200.0),
        resonance: (value(GEN_FILTER_Q).clamp(0, 960) as f32 / 960.0).clamp(0.0, 1.0),
        key_track: 0.0,
        env_amount: 0.0,
    });
    InstrumentRegion {
        sample,
        key_lo: key_lo.min(127),
        key_hi: key_hi.min(127).max(key_lo.min(127)),
        vel_lo: vel_lo.clamp(1, 127),
        vel_hi: vel_hi.clamp(vel_lo.clamp(1, 127), 127),
        root_key: root,
        tune_cents: f64::from(value(GEN_COARSE_TUNE)) * 100.0
            + f64::from(value(GEN_FINE_TUNE))
            + f64::from(hdr.correction),
        gain_db: -(value(GEN_ATTENUATION).max(0) as f32) / 10.0,
        pan: (value(GEN_PAN) as f32 / 500.0).clamp(-1.0, 1.0),
        start,
        end: Some(end),
        loop_range: looped.then_some((loop_start, loop_end)),
        one_shot: false,
        reverse: false,
        group: 0,
        xfade: 0,
        round_robin: None,
        env,
        filter,
    }
}

// ---------------------------------------------------------------------------- the file

#[derive(Debug, Clone)]
struct PresetHeader {
    name: String,
    bag: usize,
}

#[derive(Debug, Clone)]
struct InstHeader {
    bag: usize,
}

#[derive(Debug, Clone)]
struct SampleHeader {
    name: String,
    start: u32,
    end: u32,
    start_loop: u32,
    end_loop: u32,
    rate: u32,
    pitch: u8,
    correction: i8,
    kind: u16,
}

struct Sf2<'a> {
    smpl: &'a [u8],
    sm24: &'a [u8],
    phdr: Vec<PresetHeader>,
    pbag: Vec<usize>,
    pgen: Vec<(u16, [u8; 2])>,
    inst: Vec<InstHeader>,
    ibag: Vec<usize>,
    igen: Vec<(u16, [u8; 2])>,
    shdr: Vec<SampleHeader>,
}

/// The chunks of `data` as `(id, body)`; a chunk that runs past the end ends the walk.
fn chunks(data: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + 8 <= data.len() {
        let id = [data[at], data[at + 1], data[at + 2], data[at + 3]];
        let size =
            u32::from_le_bytes([data[at + 4], data[at + 5], data[at + 6], data[at + 7]]) as usize;
        let body_start = at + 8;
        let Some(body_end) = body_start.checked_add(size).filter(|e| *e <= data.len()) else {
            break;
        };
        out.push((id, &data[body_start..body_end]));
        at = body_end + (size & 1);
    }
    out
}

fn text(raw: &[u8]) -> String {
    let end = raw.iter().position(|b| *b == 0).unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..end]).trim().to_owned()
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    b.get(at..at + 2)
        .map_or(0, |s| u16::from_le_bytes([s[0], s[1]]))
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 4)
        .map_or(0, |s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn gens_of(body: &[u8]) -> Vec<(u16, [u8; 2])> {
    body.as_chunks::<4>()
        .0
        .iter()
        .map(|r| (u16::from_le_bytes([r[0], r[1]]), [r[2], r[3]]))
        .collect()
}

impl<'a> Sf2<'a> {
    fn open(bytes: &'a [u8]) -> Result<Self> {
        let not_sf2 = || AudioFileError::Unsupported("not a SoundFont 2 file".into());
        if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"sfbk" {
            return Err(not_sf2());
        }
        let top = chunks(&bytes[12..]);
        let (mut smpl, mut sm24): (&[u8], &[u8]) = (&[], &[]);
        let mut pdta: Option<Vec<([u8; 4], &[u8])>> = None;
        for (id, body) in top {
            if &id != b"LIST" || body.len() < 4 {
                continue;
            }
            match &body[..4] {
                b"sdta" => {
                    for (id, b) in chunks(&body[4..]) {
                        match &id {
                            b"smpl" => smpl = b,
                            b"sm24" => sm24 = b,
                            _ => {}
                        }
                    }
                }
                b"pdta" => pdta = Some(chunks(&body[4..])),
                _ => {}
            }
        }
        let pdta = pdta.ok_or_else(not_sf2)?;
        let find = |name: &[u8; 4]| {
            pdta.iter()
                .find(|(id, _)| id == name)
                .map_or(&[][..], |(_, b)| *b)
        };
        // The last record of each list is a terminator.
        let records = |body: &'a [u8], size: usize| -> Vec<&'a [u8]> {
            let mut r: Vec<&[u8]> = body.chunks_exact(size).collect();
            r.pop();
            r
        };
        let phdr = records(find(b"phdr"), 38)
            .into_iter()
            .map(|r| PresetHeader {
                name: text(&r[..20]),
                bag: usize::from(u16_at(r, 24)),
            })
            .collect();
        let pbag = find(b"pbag")
            .as_chunks::<4>()
            .0
            .iter()
            .map(|r| usize::from(u16::from_le_bytes([r[0], r[1]])))
            .collect();
        let inst = records(find(b"inst"), 22)
            .into_iter()
            .map(|r| InstHeader {
                bag: usize::from(u16_at(r, 20)),
            })
            .collect();
        let ibag = find(b"ibag")
            .as_chunks::<4>()
            .0
            .iter()
            .map(|r| usize::from(u16::from_le_bytes([r[0], r[1]])))
            .collect();
        let shdr = records(find(b"shdr"), 46)
            .into_iter()
            .map(|r| SampleHeader {
                name: text(&r[..20]),
                start: u32_at(r, 20),
                end: u32_at(r, 24),
                start_loop: u32_at(r, 28),
                end_loop: u32_at(r, 32),
                rate: u32_at(r, 36),
                pitch: r[40],
                correction: r[41] as i8,
                kind: u16_at(r, 44),
            })
            .collect();
        Ok(Self {
            smpl,
            sm24,
            phdr,
            pbag,
            pgen: gens_of(find(b"pgen")),
            inst,
            ibag,
            igen: gens_of(find(b"igen")),
            shdr,
        })
    }

    /// The zones of preset or instrument `n`; the global zone (the first, when it names no
    /// instrument or sample) is laid under the others and is not listed.
    fn zones(&self, n: usize, level: Level) -> Vec<Zone> {
        let (bags, gens, first, last) = match level {
            Level::Preset => (
                &self.pbag,
                &self.pgen,
                self.phdr.get(n).map(|h| h.bag),
                self.phdr.get(n + 1).map(|h| h.bag),
            ),
            Level::Instrument => (
                &self.ibag,
                &self.igen,
                self.inst.get(n).map(|h| h.bag),
                self.inst.get(n + 1).map(|h| h.bag),
            ),
        };
        // The terminator record is cut off the header lists, so the last real item ends at the
        // last bag (one short of the bag list's terminator).
        let (Some(first), end) = (first, last.unwrap_or(bags.len().saturating_sub(1))) else {
            return Vec::new();
        };
        let mut global = Gens::default();
        let mut out = Vec::new();
        for (i, bag) in (first..end.min(bags.len().saturating_sub(1))).enumerate() {
            let from = bags[bag].min(gens.len());
            let to = bags
                .get(bag + 1)
                .copied()
                .unwrap_or(gens.len())
                .min(gens.len());
            let mut zone = Gens::default();
            let (mut instrument, mut sample) = (None, None);
            for (oper, amount) in gens.get(from..to.max(from)).unwrap_or(&[]) {
                let signed = i32::from(i16::from_le_bytes(*amount));
                let unsigned = usize::from(u16::from_le_bytes(*amount));
                match *oper {
                    GEN_KEY_RANGE => zone.key = Some((amount[0].min(127), amount[1].min(127))),
                    GEN_VEL_RANGE => zone.vel = Some((amount[0], amount[1])),
                    GEN_INSTRUMENT if level == Level::Preset => instrument = Some(unsigned),
                    GEN_SAMPLE_ID if level == Level::Instrument => sample = Some(unsigned),
                    other => {
                        zone.values.insert(other, signed);
                    }
                }
            }
            if i == 0 && instrument.is_none() && sample.is_none() {
                global = zone;
                continue;
            }
            let mut gens = global.clone();
            gens.layer(&zone);
            if instrument.is_some() || sample.is_some() {
                out.push(Zone {
                    gens,
                    instrument,
                    sample,
                });
            }
        }
        out
    }

    /// The audio of a sample header, `None` when it points outside the file's sample data.
    fn sample_audio(&self, h: &SampleHeader) -> Option<AudioData> {
        let (start, end) = (h.start as usize, h.end as usize);
        if end <= start || h.rate == 0 || end.checked_mul(2)? > self.smpl.len() {
            return None;
        }
        let wide = self.sm24.len() >= end;
        let data: Vec<f32> = (start..end)
            .map(|i| {
                let hi = i16::from_le_bytes([self.smpl[2 * i], self.smpl[2 * i + 1]]);
                if wide {
                    (f32::from(hi) * 256.0 + f32::from(self.sm24[i])) / 8_388_608.0
                } else {
                    f32::from(hi) / 32_768.0
                }
            })
            .collect();
        AudioData::new(h.rate, vec![data]).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A SoundFont builder for tests: one preset, one instrument, the given samples (16-bit,
    /// 100 frames of a ramp each) and instrument zones `(generators, sample)`.
    pub(crate) fn build(zones: &[(Vec<(u16, i16)>, u16)], preset_zone: &[(u16, i16)]) -> Vec<u8> {
        fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
            let mut v = id.to_vec();
            v.extend((body.len() as u32).to_le_bytes());
            v.extend(body);
            if body.len() % 2 == 1 {
                v.push(0);
            }
            v
        }
        fn list(kind: &[u8; 4], parts: &[Vec<u8>]) -> Vec<u8> {
            let mut body = kind.to_vec();
            for p in parts {
                body.extend(p);
            }
            chunk(b"LIST", &body)
        }
        fn name(s: &str) -> Vec<u8> {
            let mut v = s.as_bytes().to_vec();
            v.resize(20, 0);
            v
        }
        fn gens(list: &[(u16, i16)]) -> Vec<u8> {
            list.iter()
                .flat_map(|(o, a)| {
                    let mut v = o.to_le_bytes().to_vec();
                    v.extend(a.to_le_bytes());
                    v
                })
                .collect()
        }
        let frames = 100usize;
        let samples = 2usize;
        let mut smpl = Vec::new();
        for s in 0..samples {
            for i in 0..frames {
                let v = ((i as i32 * 300) - 15_000 + s as i32 * 1000) as i16;
                smpl.extend(v.to_le_bytes());
            }
            // 46 zero frames of padding, as the format asks.
            smpl.extend(std::iter::repeat_n(0u8, 92));
        }
        let mut shdr = Vec::new();
        for s in 0..samples {
            let start = (s * (frames + 46)) as u32;
            shdr.extend(name(&format!("smp{s}")));
            for v in [start, start + frames as u32, start + 20, start + 80, 22_050] {
                shdr.extend(v.to_le_bytes());
            }
            shdr.extend([60u8, 0]);
            shdr.extend(0u16.to_le_bytes());
            shdr.extend(1u16.to_le_bytes());
        }
        shdr.extend(name("EOS"));
        shdr.extend([0u8; 26]);
        // Preset: one zone naming instrument 0 (plus the extra generators).
        let mut phdr = name("Test preset");
        phdr.extend([0u8; 2]);
        phdr.extend([0u8; 2]);
        phdr.extend(0u16.to_le_bytes());
        phdr.extend([0u8; 12]);
        phdr.extend(name("EOP"));
        phdr.extend([0u8; 4]);
        phdr.extend(1u16.to_le_bytes());
        phdr.extend([0u8; 12]);
        let mut pbag = Vec::new();
        pbag.extend([0u8; 4]);
        pbag.extend(
            [
                (preset_zone.len() as u16 + 1).to_le_bytes(),
                0u16.to_le_bytes(),
            ]
            .concat(),
        );
        let mut pg = preset_zone.to_vec();
        pg.push((GEN_INSTRUMENT, 0));
        let mut pgen = gens(&pg);
        pgen.extend([0u8; 4]);
        let mut inst = name("Test instrument");
        inst.extend(0u16.to_le_bytes());
        inst.extend(name("EOI"));
        inst.extend((zones.len() as u16).to_le_bytes());
        let mut ibag = Vec::new();
        let mut igen = Vec::new();
        let mut count = 0u16;
        for (g, sample) in zones {
            ibag.extend(count.to_le_bytes());
            ibag.extend([0u8; 2]);
            let mut all = g.clone();
            all.push((GEN_SAMPLE_ID, *sample as i16));
            count += all.len() as u16;
            igen.extend(gens(&all));
        }
        ibag.extend(count.to_le_bytes());
        ibag.extend([0u8; 2]);
        igen.extend([0u8; 4]);
        let pdta = list(
            b"pdta",
            &[
                chunk(b"phdr", &phdr),
                chunk(b"pbag", &pbag),
                chunk(b"pmod", &[0u8; 10]),
                chunk(b"pgen", &pgen),
                chunk(b"inst", &inst),
                chunk(b"ibag", &ibag),
                chunk(b"imod", &[0u8; 10]),
                chunk(b"igen", &igen),
                chunk(b"shdr", &shdr),
            ],
        );
        let sdta = list(b"sdta", &[chunk(b"smpl", &smpl)]);
        let info = list(b"INFO", &[chunk(b"ifil", &[2, 0, 1, 0])]);
        let mut body = b"sfbk".to_vec();
        for part in [info, sdta, pdta] {
            body.extend(part);
        }
        let mut file = b"RIFF".to_vec();
        file.extend((body.len() as u32).to_le_bytes());
        file.extend(body);
        file
    }

    fn pack(lo: u8, hi: u8) -> i16 {
        i16::from_le_bytes([lo, hi])
    }

    #[test]
    fn a_small_soundfont_reads_into_regions_and_samples() {
        let file = build(
            &[
                (
                    vec![
                        (GEN_KEY_RANGE, pack(0, 59)),
                        (GEN_SAMPLE_MODES, 1),
                        (GEN_PAN, -250),
                        (GEN_ATTENUATION, 60),
                        (GEN_COARSE_TUNE, 1),
                        (GEN_FINE_TUNE, 20),
                    ],
                    0,
                ),
                (
                    vec![
                        (GEN_KEY_RANGE, pack(60, 127)),
                        (GEN_VEL_RANGE, pack(1, 100)),
                        (GEN_ROOT_KEY, 64),
                        (GEN_ATTACK, -1200),
                        (GEN_RELEASE, 0),
                        (GEN_FILTER_FC, 9600),
                    ],
                    1,
                ),
            ],
            &[],
        );
        let inst = parse_sf2(&file, 0, InstrumentLimits::default()).expect("parses");
        assert_eq!(inst.name, "Test preset");
        assert_eq!(inst.samples.len(), 2);
        assert_eq!(inst.regions.len(), 2);
        let SampleSource::Embedded(a) = &inst.samples[0].source else {
            panic!("embedded");
        };
        assert_eq!((a.sample_rate, a.frames()), (22_050, 100));
        let (r0, r1) = (&inst.regions[0], &inst.regions[1]);
        assert_eq!((r0.key_lo, r0.key_hi, r0.root_key), (0, 59, 60));
        assert_eq!(r0.loop_range, Some((20, 80)));
        assert!((r0.pan + 0.5).abs() < 1e-6 && (r0.gain_db + 6.0).abs() < 1e-6);
        assert!((r0.tune_cents - 120.0).abs() < 1e-9);
        assert_eq!((r1.key_lo, r1.vel_hi, r1.root_key), (60, 100, 64));
        assert!(r1.loop_range.is_none());
        let env = r1.env.expect("an envelope");
        assert!((env.attack - 0.5).abs() < 1e-3 && (env.release - 1.0).abs() < 1e-3);
        let f = r1.filter.expect("a filter");
        assert!((f.cutoff - 8.176 * 2f32.powf(8.0)).abs() < 1.0);
    }

    #[test]
    fn preset_generators_add_and_ranges_intersect() {
        let file = build(
            &[(vec![(GEN_KEY_RANGE, pack(0, 80)), (GEN_FINE_TUNE, 10)], 0)],
            &[(GEN_KEY_RANGE, pack(40, 127)), (GEN_FINE_TUNE, 5)],
        );
        let inst = parse_sf2(&file, 0, InstrumentLimits::default()).expect("parses");
        let r = &inst.regions[0];
        assert_eq!((r.key_lo, r.key_hi), (40, 80));
        assert!((r.tune_cents - 15.0).abs() < 1e-9);
    }

    #[test]
    fn a_preset_zone_outside_the_instrument_range_leaves_no_region() {
        let file = build(
            &[(vec![(GEN_KEY_RANGE, pack(0, 10))], 0)],
            &[(GEN_KEY_RANGE, pack(50, 60))],
        );
        let inst = parse_sf2(&file, 0, InstrumentLimits::default()).expect("parses");
        assert!(inst.regions.is_empty());
        assert!(inst.warnings.iter().any(|w| w.contains("no playable")));
    }

    #[test]
    fn presets_are_listed_and_a_missing_one_is_an_error() {
        let file = build(&[(vec![], 0)], &[]);
        assert_eq!(sf2_presets(&file).expect("lists"), ["Test preset"]);
        assert!(parse_sf2(&file, 3, InstrumentLimits::default()).is_err());
    }

    #[test]
    fn not_a_soundfont_is_an_error() {
        assert!(parse_sf2(b"", 0, InstrumentLimits::default()).is_err());
        assert!(parse_sf2(b"RIFF\0\0\0\0WAVEfmt ", 0, InstrumentLimits::default()).is_err());
    }

    #[test]
    fn a_truncated_or_corrupt_file_never_panics() {
        let file = build(
            &[(vec![(GEN_KEY_RANGE, pack(0, 127))], 0), (vec![], 1)],
            &[],
        );
        for cut in (0..file.len()).step_by(7) {
            let _ = parse_sf2(&file[..cut], 0, InstrumentLimits::default());
        }
        for at in (0..file.len()).step_by(5) {
            let mut bad = file.clone();
            bad[at] ^= 0xff;
            let _ = parse_sf2(&bad, 0, InstrumentLimits::default());
        }
    }

    #[test]
    fn the_limits_bound_the_regions() {
        let file = build(&[(vec![], 0), (vec![], 1)], &[]);
        let limits = InstrumentLimits {
            max_regions: 1,
            ..InstrumentLimits::default()
        };
        let inst = parse_sf2(&file, 0, limits).expect("parses");
        assert_eq!(inst.regions.len(), 1);
        assert!(inst.warnings.iter().any(|w| w.contains("more than")));
    }

    /// The committed fixture (`tests/fixtures/small.sf2`): two zones over two 100-frame samples.
    /// `ATOMIK_UPDATE_GOLDEN=1` rewrites it.
    fn fixture_bytes() -> Vec<u8> {
        build(
            &[
                (
                    vec![
                        (GEN_KEY_RANGE, pack(0, 59)),
                        (GEN_SAMPLE_MODES, 1),
                        (GEN_ATTENUATION, 60),
                    ],
                    0,
                ),
                (vec![(GEN_KEY_RANGE, pack(60, 127)), (GEN_ROOT_KEY, 64)], 1),
            ],
            &[],
        )
    }

    #[test]
    fn the_committed_fixture_is_what_the_builder_writes() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/small.sf2");
        let bytes = fixture_bytes();
        if std::env::var_os("ATOMIK_UPDATE_GOLDEN").is_some() {
            std::fs::write(&path, &bytes).expect("writes the fixture");
        }
        assert_eq!(std::fs::read(&path).expect("reads the fixture"), bytes);
        let inst = read_sf2(&path, InstrumentLimits::default()).expect("reads");
        assert_eq!(inst.regions.len(), 2);
    }
}
