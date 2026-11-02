//! SFZ instrument files: a text mapping of sample files onto the keyboard.
//!
//! The common subset is read: `<control>` `default_path`; the `<global>`, `<master>`,
//! `<group>` and `<region>` headers with their usual inheritance; `sample`; key and velocity
//! ranges (`key`, `lokey`, `hikey`, `lovel`, `hivel`, note names such as `c#4`) and
//! `pitch_keycenter`; `transpose`, `tune`, `volume`, `pan`; `offset`, `end`, `loop_mode`,
//! `loop_start`, `loop_end`, `direction`; the amplitude envelope `ampeg_*`; a filter from
//! `cutoff`, `resonance`, `fil_type`, `fil_keytrack`, `fileg_depth`; round-robin from
//! `seq_length` and `seq_position`; velocity crossfades `xfin_*` and `xfout_*`; `#define`
//! variables. Comments (`//`, `/* */`) are skipped; sample paths may hold spaces.
//!
//! Everything else is skipped: an opcode this reader does not know is listed once in one
//! warning for the file, never an error. `#include` is not followed (the file must stand
//! alone), a sample path must be relative, and the sizes of the text and of what it lists are
//! capped ([`InstrumentLimits`], [`MAX_SFZ_BYTES`]). Release-triggered regions (they would
//! sound on note-on) are left out.

use crate::error::{AudioFileError, Result};
use crate::instrument::{
    Instrument, InstrumentLimits, InstrumentRegion, InstrumentSample, MAX_SFZ_BYTES, RegionEnv,
    RegionFilter, SampleSource, listed,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

type Ops = BTreeMap<String, String>;

/// Audio file extensions an SFZ sample may have (what the decoder reads).
const SAMPLE_EXTENSIONS: [&str; 6] = ["wav", "flac", "ogg", "aif", "aiff", "mp3"];

/// Longest round-robin cycle a region keeps (the zone limit).
const MAX_CYCLE: u32 = 16;

/// Reads the SFZ file at `path`; sample paths are resolved against its folder.
///
/// # Errors
/// The file cannot be read or is larger than [`MAX_SFZ_BYTES`].
pub fn read_sfz(path: &Path, limits: InstrumentLimits) -> Result<Instrument> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_SFZ_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_SFZ_BYTES {
        return Err(AudioFileError::Unsupported(format!(
            "SFZ file too large (more than {MAX_SFZ_BYTES} bytes)"
        )));
    }
    let text = String::from_utf8_lossy(&bytes);
    let name = path
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    let base = path.parent().unwrap_or_else(|| Path::new(""));
    Ok(parse_sfz(&text, base, &name, limits))
}

/// Parses SFZ `text`; sample paths are joined to `base`.
pub fn parse_sfz(text: &str, base: &Path, name: &str, limits: InstrumentLimits) -> Instrument {
    let tokens = tokenize(&strip_comments(text));
    let raw = collect_regions(tokens);
    let mut ctx = Ctx {
        base: base.to_path_buf(),
        default_path: raw.default_path,
        limits,
        samples: Vec::new(),
        index: BTreeMap::new(),
        regions: Vec::new(),
        unknown: raw.unknown,
        skipped: Skipped::default(),
    };
    for (ops, group) in &raw.regions {
        if ctx.regions.len() >= limits.max_regions {
            ctx.skipped.over_limit = true;
            break;
        }
        ctx.add_region(ops, *group);
    }
    let mut warnings = Vec::new();
    warnings.extend(listed(
        "ignored opcodes this reader does not know",
        &ctx.unknown,
    ));
    ctx.skipped.warn(&mut warnings, limits);
    if ctx.regions.is_empty() {
        warnings.push("the file has no playable regions".to_owned());
    }
    Instrument {
        name: name.to_owned(),
        samples: ctx.samples,
        regions: ctx.regions,
        warnings,
    }
}

// ---------------------------------------------------------------------------- tokens

enum Token {
    Header(String),
    Opcode(String, String),
}

/// Removes `//` and `/* */` comments, keeping line breaks.
fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut block = false;
    while let Some(c) = chars.next() {
        if block {
            if c == '*' && chars.peek() == Some(&'/') {
                chars.next();
                block = false;
            } else if c == '\n' {
                out.push('\n');
            }
            continue;
        }
        if c == '/' && chars.peek() == Some(&'/') {
            for n in chars.by_ref() {
                if n == '\n' {
                    out.push('\n');
                    break;
                }
            }
        } else if c == '/' && chars.peek() == Some(&'*') {
            chars.next();
            block = true;
        } else {
            out.push(c);
        }
    }
    out
}

/// Headers and opcodes of the text, with `#define` variables substituted.
fn tokenize(text: &str) -> Vec<Token> {
    let mut defines: Vec<(String, String)> = Vec::new();
    let mut tokens = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("#define") {
            let mut parts = rest.trim().splitn(2, char::is_whitespace);
            if let (Some(var), Some(value)) = (parts.next(), parts.next())
                && var.starts_with('$')
                && defines.len() < 256
            {
                defines.push((var.to_owned(), value.trim().to_owned()));
                // Longest names first, so `$KEY2` is not eaten by `$KEY`.
                defines.sort_by_key(|(v, _)| std::cmp::Reverse(v.len()));
            }
            continue;
        }
        if trimmed.starts_with('#') {
            // `#include` is not followed.
            tokens.push(Token::Opcode(
                "#include".to_owned(),
                trimmed.trim().to_owned(),
            ));
            continue;
        }
        let mut line = line.to_owned();
        if line.contains('$') {
            for (var, value) in &defines {
                line = line.replace(var.as_str(), value);
            }
        }
        scan_line(&line, &mut tokens);
    }
    tokens
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '$'
}

/// Splits one line into headers and `name=value` opcodes. A value runs to the next
/// whitespace-separated `name=` or header, so it may hold spaces (a sample path).
fn scan_line(line: &str, out: &mut Vec<Token>) {
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '<' {
            let mut j = i + 1;
            while j < chars.len() && chars[j] != '>' {
                j += 1;
            }
            let name: String = chars[i + 1..j.min(chars.len())].iter().collect();
            out.push(Token::Header(name.trim().to_ascii_lowercase()));
            i = j + 1;
        } else {
            let mut j = i;
            while j < chars.len() && is_name_char(chars[j]) {
                j += 1;
            }
            if j == i || chars.get(j) != Some(&'=') {
                // Not an opcode: skip the word.
                while i < chars.len() && !chars[i].is_whitespace() {
                    i += 1;
                }
                continue;
            }
            let name: String = chars[i..j].iter().collect();
            let start = j + 1;
            let mut end = start;
            while end < chars.len() {
                if chars[end].is_whitespace() {
                    let mut k = end;
                    while k < chars.len() && chars[k].is_whitespace() {
                        k += 1;
                    }
                    if k >= chars.len() || chars[k] == '<' {
                        break;
                    }
                    let mut m = k;
                    while m < chars.len() && is_name_char(chars[m]) {
                        m += 1;
                    }
                    if m > k && chars.get(m) == Some(&'=') {
                        break;
                    }
                    end = k;
                } else {
                    end += 1;
                }
            }
            let value: String = chars[start..end].iter().collect();
            out.push(Token::Opcode(
                name.to_ascii_lowercase(),
                value.trim().to_owned(),
            ));
            i = end;
        }
    }
}

// ---------------------------------------------------------------------------- structure

#[derive(Default)]
struct Raw {
    /// Each region's opcodes with the global, master and group ones under them, and its group
    /// number (0 without a `<group>`).
    regions: Vec<(Ops, u32)>,
    default_path: String,
    unknown: BTreeSet<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Skip,
    Control,
    Global,
    Master,
    Group,
    Region,
}

fn collect_regions(tokens: Vec<Token>) -> Raw {
    let mut raw = Raw::default();
    let (mut global, mut master, mut group, mut region) =
        (Ops::new(), Ops::new(), Ops::new(), Ops::new());
    let mut mode = Mode::Skip;
    let mut group_no = 0u32;
    let mut groups_seen = 0u32;
    let mut in_region = false;
    let finish =
        |raw: &mut Raw, in_region: &mut bool, region: &mut Ops, group_no: u32, under: [&Ops; 3]| {
            if *in_region {
                let mut merged = Ops::new();
                for ops in under {
                    merged.extend(ops.iter().map(|(k, v)| (k.clone(), v.clone())));
                }
                merged.extend(std::mem::take(region));
                raw.regions.push((merged, group_no));
                *in_region = false;
            }
        };
    for token in tokens {
        match token {
            Token::Header(name) => {
                finish(
                    &mut raw,
                    &mut in_region,
                    &mut region,
                    group_no,
                    [&global, &master, &group],
                );
                match name.as_str() {
                    "control" => mode = Mode::Control,
                    "global" => {
                        global.clear();
                        master.clear();
                        group.clear();
                        group_no = 0;
                        mode = Mode::Global;
                    }
                    "master" => {
                        master.clear();
                        group.clear();
                        group_no = 0;
                        mode = Mode::Master;
                    }
                    "group" => {
                        group.clear();
                        groups_seen += 1;
                        group_no = groups_seen;
                        mode = Mode::Group;
                    }
                    "region" => {
                        region.clear();
                        in_region = true;
                        mode = Mode::Region;
                    }
                    other => {
                        raw.unknown.insert(format!("<{other}>"));
                        mode = Mode::Skip;
                    }
                }
            }
            Token::Opcode(name, value) => match mode {
                Mode::Control => {
                    if name == "default_path" {
                        raw.default_path = value;
                    } else if name == "#include" {
                        raw.unknown.insert("#include".to_owned());
                    }
                }
                Mode::Global => {
                    global.insert(name, value);
                }
                Mode::Master => {
                    master.insert(name, value);
                }
                Mode::Group => {
                    group.insert(name, value);
                }
                Mode::Region => {
                    region.insert(name, value);
                }
                Mode::Skip => {
                    if name == "#include" {
                        raw.unknown.insert("#include".to_owned());
                    }
                }
            },
        }
    }
    finish(
        &mut raw,
        &mut in_region,
        &mut region,
        group_no,
        [&global, &master, &group],
    );
    raw
}

// ---------------------------------------------------------------------------- regions

/// Opcodes read (anything else in a region is reported once).
const KNOWN: &[&str] = &[
    "sample",
    "key",
    "lokey",
    "hikey",
    "lovel",
    "hivel",
    "pitch_keycenter",
    "transpose",
    "tune",
    "volume",
    "pan",
    "offset",
    "end",
    "loop_mode",
    "loopmode",
    "loop_start",
    "loopstart",
    "loop_end",
    "loopend",
    "direction",
    "ampeg_attack",
    "ampeg_decay",
    "ampeg_sustain",
    "ampeg_release",
    "cutoff",
    "resonance",
    "fil_type",
    "filtype",
    "fil_keytrack",
    "fileg_depth",
    "seq_length",
    "seq_position",
    "xfin_lovel",
    "xfin_hivel",
    "xfout_lovel",
    "xfout_hivel",
    "trigger",
];

#[derive(Default)]
struct Skipped {
    release: usize,
    absolute: usize,
    formats: BTreeSet<String>,
    bad_range: usize,
    generated: usize,
    long_cycle: usize,
    over_limit: bool,
    sample_limit: usize,
}

impl Skipped {
    fn warn(&self, out: &mut Vec<String>, limits: InstrumentLimits) {
        let count = |n: usize, what: &str| format!("{n} region(s) left out: {what}");
        if self.release > 0 {
            out.push(count(self.release, "triggered on release"));
        }
        if self.absolute > 0 {
            out.push(count(self.absolute, "sample paths must be relative"));
        }
        if self.generated > 0 {
            out.push(count(self.generated, "built-in generators are not samples"));
        }
        if self.bad_range > 0 {
            out.push(count(self.bad_range, "an empty key or velocity range"));
        }
        out.extend(listed(
            "regions left out: sample format not supported",
            &self.formats,
        ));
        if self.long_cycle > 0 {
            out.push(format!(
                "{} region(s) use a round-robin cycle longer than {MAX_CYCLE}: played without it",
                self.long_cycle
            ));
        }
        if self.over_limit {
            out.push(format!(
                "only the first {} regions were read",
                limits.max_regions
            ));
        }
        if self.sample_limit > 0 {
            out.push(count(
                self.sample_limit,
                &format!("more than {} samples", limits.max_samples),
            ));
        }
    }
}

struct Ctx {
    base: PathBuf,
    default_path: String,
    limits: InstrumentLimits,
    samples: Vec<InstrumentSample>,
    index: BTreeMap<PathBuf, usize>,
    regions: Vec<InstrumentRegion>,
    unknown: BTreeSet<String>,
    skipped: Skipped,
}

fn num(ops: &Ops, name: &str) -> Option<f64> {
    ops.get(name)
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite())
}

fn first_num(ops: &Ops, names: &[&str]) -> Option<f64> {
    names.iter().find_map(|n| num(ops, n))
}

fn frames_of(ops: &Ops, names: &[&str]) -> Option<u64> {
    first_num(ops, names).map(|v| if v < 0.0 { 0 } else { v as u64 })
}

/// A key as a number or a note name (`c4` is 60, `c#4`, `db3`, `a-1`).
fn parse_key(text: &str) -> Option<u8> {
    let t = text.trim();
    if let Ok(n) = t.parse::<i32>() {
        return u8::try_from(n).ok().filter(|k| *k <= 127);
    }
    let mut chars = t.chars();
    let semitone = match chars.next()?.to_ascii_lowercase() {
        'c' => 0,
        'd' => 2,
        'e' => 4,
        'f' => 5,
        'g' => 7,
        'a' => 9,
        'b' => 11,
        _ => return None,
    };
    let rest = chars.as_str();
    let (shift, rest) = match rest.chars().next() {
        Some('#') => (1, &rest[1..]),
        Some('b') => (-1, &rest[1..]),
        _ => (0, rest),
    };
    let octave: i32 = rest.parse().ok()?;
    u8::try_from((octave + 1) * 12 + semitone + shift)
        .ok()
        .filter(|k| *k <= 127)
}

fn key_of(ops: &Ops, name: &str) -> Option<u8> {
    ops.get(name).and_then(|v| parse_key(v))
}

impl Ctx {
    fn add_region(&mut self, ops: &Ops, group: u32) {
        for name in ops.keys() {
            if !KNOWN.contains(&name.as_str()) && name != "default_path" {
                self.unknown.insert(name.clone());
            }
        }
        let Some(sample_text) = ops
            .get("sample")
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        else {
            return;
        };
        if sample_text.starts_with('*') {
            self.skipped.generated += 1;
            return;
        }
        if let Some(t) = ops.get("trigger")
            && t.trim().eq_ignore_ascii_case("release")
        {
            self.skipped.release += 1;
            return;
        }
        let Some(sample) = self.sample_index(sample_text) else {
            return;
        };
        let single = key_of(ops, "key");
        let key_lo = key_of(ops, "lokey").or(single).unwrap_or(0);
        let key_hi = key_of(ops, "hikey").or(single).unwrap_or(127);
        let root_key = key_of(ops, "pitch_keycenter").or(single).unwrap_or(60);
        let vel = |name: &str, default: i64| {
            num(ops, name).map_or(default, |v| (v.round() as i64).clamp(0, 127))
        };
        let mut vel_lo = vel("lovel", 1).max(1);
        let mut vel_hi = vel("hivel", 127);
        // A crossfade starts where the zone starts to sound.
        let in_lo = num(ops, "xfin_lovel").map(|v| (v.round() as i64).clamp(1, 127));
        let in_hi = num(ops, "xfin_hivel").map(|v| (v.round() as i64).clamp(1, 127));
        let out_lo = num(ops, "xfout_lovel").map(|v| (v.round() as i64).clamp(1, 127));
        let out_hi = num(ops, "xfout_hivel").map(|v| (v.round() as i64).clamp(1, 127));
        let mut xfade = 0i64;
        if let (Some(lo), Some(hi)) = (in_lo, in_hi) {
            vel_lo = vel_lo.max(lo);
            xfade = xfade.max(hi - lo);
        }
        if let (Some(lo), Some(hi)) = (out_lo, out_hi) {
            vel_hi = vel_hi.min(hi);
            xfade = xfade.max(hi - lo);
        }
        if key_lo > key_hi || vel_lo > vel_hi {
            self.skipped.bad_range += 1;
            return;
        }
        let xfade = u8::try_from(xfade.clamp(0, vel_hi - vel_lo)).unwrap_or(0);

        let looping = ops
            .get("loop_mode")
            .or_else(|| ops.get("loopmode"))
            .map(|v| v.trim().to_ascii_lowercase());
        let one_shot = looping.as_deref() == Some("one_shot");
        let start = frames_of(ops, &["offset"]).unwrap_or(0);
        let end = frames_of(ops, &["end"]).map(|e| e.saturating_add(1));
        let loop_range = matches!(looping.as_deref(), Some("loop_continuous" | "loop_sustain"))
            .then(|| {
                let ls = frames_of(ops, &["loop_start", "loopstart"]).unwrap_or(start);
                // `loop_end` is the last frame of the loop; ours is exclusive. Without it the loop
                // runs to the end of the region.
                let le = frames_of(ops, &["loop_end", "loopend"])
                    .map_or(u64::MAX, |e| e.saturating_add(1));
                (ls, le)
            });
        let reverse = ops
            .get("direction")
            .is_some_and(|d| d.trim().eq_ignore_ascii_case("reverse"));

        let round_robin = match num(ops, "seq_length").map(|v| v.round() as i64) {
            Some(len) if len > 1 => {
                if len as u32 > MAX_CYCLE {
                    self.skipped.long_cycle += 1;
                    None
                } else {
                    let pos = num(ops, "seq_position").map_or(1, |v| v.round() as i64);
                    Some((len as u8, (pos.clamp(1, len) - 1) as u8))
                }
            }
            _ => None,
        };

        let env = [
            "ampeg_attack",
            "ampeg_decay",
            "ampeg_sustain",
            "ampeg_release",
        ]
        .iter()
        .any(|n| ops.contains_key(*n))
        .then(|| RegionEnv {
            attack: num(ops, "ampeg_attack").unwrap_or(0.0).max(0.0) as f32,
            decay: num(ops, "ampeg_decay").unwrap_or(0.0).max(0.0) as f32,
            sustain: (num(ops, "ampeg_sustain").unwrap_or(100.0) / 100.0).clamp(0.0, 1.0) as f32,
            release: num(ops, "ampeg_release").unwrap_or(0.0).max(0.0) as f32,
        });
        let filter = num(ops, "cutoff").map(|cutoff| {
            let kind = ops
                .get("fil_type")
                .or_else(|| ops.get("filtype"))
                .map_or_else(String::new, |v| v.trim().to_ascii_lowercase());
            let mode = if kind.starts_with("hpf") {
                2
            } else if kind.starts_with("bpf") {
                1
            } else {
                0
            };
            RegionFilter {
                mode,
                cutoff: cutoff as f32,
                resonance: (num(ops, "resonance").unwrap_or(0.0) / 40.0).clamp(0.0, 1.0) as f32,
                key_track: (num(ops, "fil_keytrack").unwrap_or(0.0) / 100.0).clamp(0.0, 1.0) as f32,
                env_amount: (num(ops, "fileg_depth").unwrap_or(0.0) / 7200.0).clamp(-1.0, 1.0)
                    as f32,
            }
        });
        self.regions.push(InstrumentRegion {
            sample,
            key_lo,
            key_hi,
            vel_lo: vel_lo as u8,
            vel_hi: vel_hi as u8,
            root_key,
            tune_cents: num(ops, "transpose").unwrap_or(0.0) * 100.0
                + num(ops, "tune").unwrap_or(0.0),
            gain_db: num(ops, "volume").unwrap_or(0.0) as f32,
            pan: (num(ops, "pan").unwrap_or(0.0) / 100.0).clamp(-1.0, 1.0) as f32,
            start,
            end,
            loop_range,
            one_shot,
            reverse,
            group,
            xfade,
            round_robin,
            env,
            filter,
        });
    }

    /// The index of the sample at `text` (added on first use), or `None` when the region has to
    /// be left out.
    fn sample_index(&mut self, text: &str) -> Option<usize> {
        let relative = |s: &str| {
            let s = s.trim().replace('\\', "/");
            let p = Path::new(&s);
            let bad = p.is_absolute() || s.starts_with('/') || s.chars().nth(1) == Some(':');
            (!bad).then_some(s)
        };
        let Some(file) = relative(text) else {
            self.skipped.absolute += 1;
            return None;
        };
        let folder = relative(&self.default_path).unwrap_or_default();
        let ext = Path::new(&file)
            .extension()
            .map_or_else(String::new, |e| e.to_string_lossy().to_ascii_lowercase());
        if !SAMPLE_EXTENSIONS.contains(&ext.as_str()) {
            self.skipped.formats.insert(format!(".{ext}"));
            return None;
        }
        let path = self.base.join(folder).join(file);
        if let Some(i) = self.index.get(&path) {
            return Some(*i);
        }
        if self.samples.len() >= self.limits.max_samples {
            self.skipped.sample_limit += 1;
            return None;
        }
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        self.samples.push(InstrumentSample {
            name,
            source: SampleSource::File(path.clone()),
        });
        self.index.insert(path, self.samples.len() - 1);
        Some(self.samples.len() - 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Instrument {
        parse_sfz(text, Path::new("lib"), "test", InstrumentLimits::default())
    }

    #[test]
    fn keys_read_as_numbers_and_note_names() {
        assert_eq!(parse_key("60"), Some(60));
        assert_eq!(parse_key("c4"), Some(60));
        assert_eq!(parse_key("C#4"), Some(61));
        assert_eq!(parse_key("db4"), Some(61));
        assert_eq!(parse_key("a-1"), Some(9));
        assert_eq!(parse_key("g9"), Some(127));
        assert_eq!(parse_key("g#9"), None);
        assert_eq!(parse_key("200"), None);
        assert_eq!(parse_key("x"), None);
    }

    #[test]
    fn regions_inherit_from_global_master_and_group() {
        let sfz = "<global> volume=-3 lovel=10\n<master> pan=20\n<group> hivel=90 \
                   <region> sample=a.wav key=c4\n<region> sample=b.wav lokey=0 hikey=10 hivel=50\n";
        let inst = parse(sfz);
        assert_eq!(inst.regions.len(), 2);
        let (a, b) = (&inst.regions[0], &inst.regions[1]);
        assert_eq!((a.key_lo, a.key_hi, a.root_key), (60, 60, 60));
        assert_eq!((a.vel_lo, a.vel_hi), (10, 90));
        assert!((a.gain_db + 3.0).abs() < 1e-6);
        assert!((a.pan - 0.2).abs() < 1e-6);
        assert_eq!((b.vel_lo, b.vel_hi, b.group), (10, 50, 1));
        assert_eq!(inst.samples.len(), 2);
    }

    #[test]
    fn sample_paths_keep_their_spaces_and_resolve_against_the_folder() {
        let sfz = "<control> default_path=Samples\\Piano\\\n<region> sample=soft hit 1.wav \
                   key=62 tune=5\n";
        let inst = parse(sfz);
        let SampleSource::File(path) = &inst.samples[0].source else {
            panic!("a file");
        };
        assert_eq!(
            path,
            &Path::new("lib")
                .join("Samples/Piano/")
                .join("soft hit 1.wav")
        );
        assert_eq!(inst.samples[0].name, "soft hit 1.wav");
        assert!((inst.regions[0].tune_cents - 5.0).abs() < 1e-9);
    }

    #[test]
    fn loops_ranges_and_envelope_are_read() {
        let sfz = "<region> sample=a.wav loop_mode=loop_continuous loop_start=100 loop_end=199 \
                   offset=10 end=999 ampeg_attack=0.01 ampeg_release=0.5 ampeg_sustain=50 \
                   direction=reverse transpose=-1 tune=20\n";
        let r = &parse(sfz).regions[0];
        assert_eq!(r.loop_range, Some((100, 200)));
        assert_eq!((r.start, r.end), (10, Some(1000)));
        assert!(r.reverse && !r.one_shot);
        let env = r.env.expect("an envelope");
        assert!((env.attack - 0.01).abs() < 1e-6 && (env.sustain - 0.5).abs() < 1e-6);
        assert!((env.release - 0.5).abs() < 1e-6);
        assert!((r.tune_cents + 80.0).abs() < 1e-9);
        let one = &parse("<region> sample=a.wav loop_mode=one_shot\n").regions[0];
        assert!(one.one_shot && one.loop_range.is_none());
    }

    #[test]
    fn round_robin_crossfade_and_filter_are_read() {
        let sfz = "<region> sample=a.wav seq_length=4 seq_position=3 xfin_lovel=20 xfin_hivel=40 \
                   cutoff=800 resonance=20 fil_type=hpf_2p fil_keytrack=50 fileg_depth=3600\n";
        let r = &parse(sfz).regions[0];
        assert_eq!(r.round_robin, Some((4, 2)));
        assert_eq!((r.vel_lo, r.xfade), (20, 20));
        let f = r.filter.expect("a filter");
        assert_eq!(f.mode, 2);
        assert!((f.cutoff - 800.0).abs() < 1e-3 && (f.resonance - 0.5).abs() < 1e-6);
        assert!((f.key_track - 0.5).abs() < 1e-6 && (f.env_amount - 0.5).abs() < 1e-6);
    }

    #[test]
    fn unknown_opcodes_are_one_warning_and_never_fail() {
        let sfz = "<region> sample=a.wav lorand=0.5 sw_last=36 key=60\n\
                   <region> sample=b.wav lorand=0.5 foo_bar=1 key=61\n";
        let inst = parse(sfz);
        assert_eq!(inst.regions.len(), 2);
        let unknown: Vec<_> = inst
            .warnings
            .iter()
            .filter(|w| w.contains("opcodes"))
            .collect();
        assert_eq!(unknown.len(), 1);
        for name in ["lorand", "sw_last", "foo_bar"] {
            assert!(unknown[0].contains(name), "{unknown:?}");
        }
    }

    #[test]
    fn comments_defines_and_release_regions() {
        let sfz = "// a comment\n#define $K 64\n/* block\n sample=ghost.wav */\n\
                   <region> sample=a.wav key=$K // inline\n\
                   <region> sample=r.wav key=65 trigger=release\n";
        let inst = parse(sfz);
        assert_eq!(inst.regions.len(), 1);
        assert_eq!(inst.regions[0].key_lo, 64);
        assert!(inst.warnings.iter().any(|w| w.contains("release")));
    }

    #[test]
    fn unsafe_and_unreadable_samples_are_left_out() {
        let sfz = "<region> sample=/etc/x.wav\n<region> sample=D\u{3a}\\x.wav\n\
                   <region> sample=a.wv\n<region> sample=*sine\n<region> sample=ok.flac\n";
        let inst = parse(sfz);
        assert_eq!(inst.regions.len(), 1);
        assert_eq!(inst.samples.len(), 1);
        assert!(inst.warnings.iter().any(|w| w.contains("relative")));
        assert!(inst.warnings.iter().any(|w| w.contains(".wv")));
    }

    #[test]
    fn the_limits_bound_regions_and_samples() {
        let mut sfz = String::new();
        for i in 0..50 {
            sfz.push_str(&format!("<region> sample=s{i}.wav\n"));
        }
        let limits = InstrumentLimits {
            max_regions: 10,
            max_samples: 5,
            ..InstrumentLimits::default()
        };
        let inst = parse_sfz(&sfz, Path::new(""), "t", limits);
        assert_eq!(inst.samples.len(), 5);
        assert_eq!(inst.regions.len(), 5);
        assert!(inst.warnings.iter().any(|w| w.contains("more than 5")));
    }

    #[test]
    fn garbage_text_gives_an_empty_instrument() {
        let inst = parse("\u{1}\u{2}<<<>>> ==== <region <region> key=");
        assert!(inst.regions.is_empty());
        assert!(!inst.warnings.is_empty());
    }
}
