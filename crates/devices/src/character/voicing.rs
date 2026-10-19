//! The voicing tables of `atomik.character`: data, not code, carries
//! the tuning of every preset.
//!
//! * `character-voicing.csv` (schema `preset,stage,param,t0,t50,t100,interp,source,note`): one row
//!   per stage parameter of a preset. A row whose three values are equal is a constant; a row
//!   whose values differ is a **morph value**: the Transform control moves it along the three
//!   points (`t0` at Transform 0, `t50` at 0.5, `t100` at 1) with the interpolation tag inside
//!   each half. A preset has at most [`MAX_MORPH`] morph values (parameters and route gains
//!   together).
//! * `character-routes.csv` (schema `preset,from,send,to,gain_t0,gain_t50,gain_t100,interp,
//!   source,note`): one row per send of a preset built on the generic rack
//!   (`super::rack::RackChain`). `from` is a stage name or `in`, `to` a stage name or `out`.
//!
//! The loader checks every row against the [`ChainSpec`] the chain declares (known stage and
//! param, value inside the declared limits) and that every parameter of an implemented preset has
//! a row. Everything is parsed once, in the constructor or `prepare`, into fixed structures; the
//! per-sample path only reads [`Val`]s.

use atomik_dsp::{exp2_det, log2_det};

/// The 16 preset keys in index order.
pub const PRESET_KEYS: [&str; 16] = [
    "reel",
    "loudness",
    "sparkle",
    "low-end",
    "wide",
    "pocket-radio",
    "camcorder",
    "dusty-groove",
    "lossy",
    "swirl",
    "torn-cone",
    "tin-robot",
    "bit-bender",
    "spin-cycle",
    "ion-sweep",
    "ghost-pipes",
];

/// Most morph values (voicing rows plus route gains whose three points differ) of one preset.
pub const MAX_MORPH: usize = 8;

/// Header of a voicing table.
pub const VOICING_HEADER: [&str; 9] = [
    "preset", "stage", "param", "t0", "t50", "t100", "interp", "source", "note",
];
/// Header of a route table.
pub const ROUTES_HEADER: [&str; 10] = [
    "preset",
    "from",
    "send",
    "to",
    "gain_t0",
    "gain_t50",
    "gain_t100",
    "interp",
    "source",
    "note",
];

/// How a morph value moves between two table points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interp {
    /// Straight line.
    Lin,
    /// Geometric (a straight line in the logarithm); linear when an end is not positive.
    Log,
    /// Slow start: `u^2`.
    Square,
    /// Flat-start exponential: `(2^(4u) - 1) / 15`.
    Exp,
    /// Holds the lower point until the upper one is reached.
    Step,
}

impl Interp {
    /// Parses a table tag (`lin`, `log`, `square`, `exp`, `step`).
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "lin" => Self::Lin,
            "log" => Self::Log,
            "square" => Self::Square,
            "exp" => Self::Exp,
            "step" => Self::Step,
            _ => return None,
        })
    }

    /// The value between `a` (at `u = 0`) and `b` (at `u = 1`).
    pub fn eval(self, a: f32, b: f32, u: f32) -> f32 {
        let u = if u.is_nan() { 0.0 } else { u.clamp(0.0, 1.0) };
        match self {
            Self::Lin => a + (b - a) * u,
            Self::Log => {
                if a > 0.0 && b > 0.0 {
                    a * exp2_det(u * log2_det(b / a))
                } else {
                    a + (b - a) * u
                }
            }
            Self::Square => a + (b - a) * u * u,
            Self::Exp => a + (b - a) * (exp2_det(4.0 * u) - 1.0) * (1.0 / 15.0),
            Self::Step => {
                if u < 1.0 {
                    a
                } else {
                    b
                }
            }
        }
    }
}

/// Evaluates a three-point morph value at Transform `x` (clamped to 0..1): the first half
/// interpolates `t[0]` to `t[1]`, the second `t[1]` to `t[2]`.
pub fn eval3(t: [f32; 3], interp: Interp, x: f32) -> f32 {
    let x = if x.is_nan() { 0.0 } else { x.clamp(0.0, 1.0) };
    if x < 0.5 {
        interp.eval(t[0], t[1], x * 2.0)
    } else {
        interp.eval(t[1], t[2], (x - 0.5) * 2.0)
    }
}

/// The resolved morph values of one preset (see [`PresetVoicing::resolve`]).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Morph {
    /// One value per morph slot.
    pub v: [f32; MAX_MORPH],
}

const NO_SLOT: u8 = u8::MAX;

/// A voicing value: a constant or a reference to a morph slot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Val {
    base: f32,
    slot: u8,
}

impl Val {
    /// A constant.
    pub const fn constant(v: f32) -> Self {
        Self {
            base: v,
            slot: NO_SLOT,
        }
    }

    /// The value for a resolved [`Morph`].
    #[inline]
    pub fn at(self, m: &Morph) -> f32 {
        if self.slot == NO_SLOT {
            self.base
        } else {
            m.v[self.slot as usize]
        }
    }

    /// The constant value, or the Transform 0.5 point of a morph value.
    pub fn base(self) -> f32 {
        self.base
    }

    /// The morph slot, or `None` for a constant.
    pub fn slot(self) -> Option<usize> {
        (self.slot != NO_SLOT).then_some(self.slot as usize)
    }
}

/// One declared parameter of a stage: its name and limits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamSpec {
    /// The `param` column.
    pub name: &'static str,
    /// Lowest accepted value.
    pub min: f32,
    /// Highest accepted value.
    pub max: f32,
}

/// One declared stage of a chain.
#[derive(Debug, Clone, Copy)]
pub struct StageSpec {
    /// The `stage` column.
    pub name: &'static str,
    /// Its parameters.
    pub params: &'static [ParamSpec],
}

/// What a chain accepts from the tables.
#[derive(Debug, Clone)]
pub struct ChainSpec {
    /// The stages (including the universal `chain` stage).
    pub stages: Vec<StageSpec>,
    /// True when the preset is really voiced (all params need rows); false for a stub.
    pub implemented: bool,
}

impl ChainSpec {
    fn find(&self, stage: &str, param: &str) -> Option<&ParamSpec> {
        self.stages
            .iter()
            .find(|s| s.name == stage)?
            .params
            .iter()
            .find(|p| p.name == param)
    }

    fn has_stage(&self, stage: &str) -> bool {
        self.stages.iter().any(|s| s.name == stage)
    }
}

/// A rejected table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoicingError {
    /// The first line is not the schema header.
    Header {
        /// Which table.
        table: &'static str,
    },
    /// A row has the wrong number of columns.
    Columns {
        /// 1-based line.
        line: usize,
    },
    /// The `preset` cell is not one of the 16 keys.
    UnknownPreset {
        /// 1-based line.
        line: usize,
        /// The cell.
        key: String,
    },
    /// A number cell does not parse or is not finite.
    BadNumber {
        /// 1-based line.
        line: usize,
    },
    /// The `interp` cell is not a known tag.
    BadInterp {
        /// 1-based line.
        line: usize,
    },
    /// The `source` cell is not `original`.
    BadSource {
        /// 1-based line.
        line: usize,
    },
    /// The same key appears twice.
    Duplicate {
        /// 1-based line of the second occurrence.
        line: usize,
    },
    /// The row names a stage or param the chain does not declare.
    UnknownParam {
        /// Preset index.
        preset: usize,
        /// Stage cell.
        stage: String,
        /// Param cell.
        param: String,
    },
    /// A value is outside the declared limits.
    OutOfRange {
        /// Preset index.
        preset: usize,
        /// Stage cell.
        stage: String,
        /// Param cell.
        param: String,
    },
    /// An implemented preset has no row for a declared param.
    Incomplete {
        /// Preset index.
        preset: usize,
        /// Stage name.
        stage: String,
        /// Param name.
        param: String,
    },
    /// A preset has more than [`MAX_MORPH`] morph values.
    TooManyMorph {
        /// Preset index.
        preset: usize,
        /// How many.
        count: usize,
    },
    /// A route row is malformed (send, gain range or stage names).
    BadRoute {
        /// 1-based line.
        line: usize,
    },
}

#[derive(Debug, Clone)]
struct Row {
    line: usize,
    preset: usize,
    stage: String,
    param: String,
    t: [f32; 3],
    interp: Interp,
}

#[derive(Debug, Clone)]
struct RawRoute {
    line: usize,
    preset: usize,
    from: String,
    send: u8,
    to: String,
    t: [f32; 3],
    interp: Interp,
}

fn finite(s: &str, line: usize) -> Result<f32, VoicingError> {
    s.trim()
        .parse::<f32>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or(VoicingError::BadNumber { line })
}

fn data_lines<'a>(
    text: &'a str,
    header: &[&str],
    table: &'static str,
) -> Result<Vec<(usize, Vec<&'a str>)>, VoicingError> {
    let mut lines = text
        .lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l.trim()))
        .filter(|(_, l)| !l.is_empty() && !l.starts_with('#'));
    let Some((_, first)) = lines.next() else {
        return Ok(Vec::new());
    };
    let cols: Vec<&str> = first.split(',').map(str::trim).collect();
    if cols != header {
        return Err(VoicingError::Header { table });
    }
    let mut out = Vec::new();
    for (line, l) in lines {
        let cells: Vec<&str> = l.split(',').map(str::trim).collect();
        if cells.len() != header.len() {
            return Err(VoicingError::Columns { line });
        }
        out.push((line, cells));
    }
    Ok(out)
}

fn preset_index(key: &str, line: usize) -> Result<usize, VoicingError> {
    PRESET_KEYS
        .iter()
        .position(|k| *k == key)
        .ok_or_else(|| VoicingError::UnknownPreset {
            line,
            key: key.to_string(),
        })
}

fn check_source(s: &str, line: usize) -> Result<(), VoicingError> {
    if s == "original" {
        Ok(())
    } else {
        Err(VoicingError::BadSource { line })
    }
}

fn parse_voicing(text: &str) -> Result<Vec<Row>, VoicingError> {
    let mut rows: Vec<Row> = Vec::new();
    for (line, c) in data_lines(text, &VOICING_HEADER, "voicing")? {
        let preset = preset_index(c[0], line)?;
        let t = [
            finite(c[3], line)?,
            finite(c[4], line)?,
            finite(c[5], line)?,
        ];
        let interp = Interp::parse(c[6]).ok_or(VoicingError::BadInterp { line })?;
        check_source(c[7], line)?;
        if rows
            .iter()
            .any(|r| r.preset == preset && r.stage == c[1] && r.param == c[2])
        {
            return Err(VoicingError::Duplicate { line });
        }
        rows.push(Row {
            line,
            preset,
            stage: c[1].to_string(),
            param: c[2].to_string(),
            t,
            interp,
        });
    }
    Ok(rows)
}

fn parse_routes(text: &str) -> Result<Vec<RawRoute>, VoicingError> {
    let mut out: Vec<RawRoute> = Vec::new();
    for (line, c) in data_lines(text, &ROUTES_HEADER, "routes")? {
        let preset = preset_index(c[0], line)?;
        let send = match c[2] {
            "1" => 1,
            "2" => 2,
            _ => return Err(VoicingError::BadRoute { line }),
        };
        let t = [
            finite(c[4], line)?,
            finite(c[5], line)?,
            finite(c[6], line)?,
        ];
        if t.iter().any(|g| !(0.0..=2.0).contains(g)) {
            return Err(VoicingError::BadRoute { line });
        }
        let interp = Interp::parse(c[7]).ok_or(VoicingError::BadInterp { line })?;
        check_source(c[8], line)?;
        out.push(RawRoute {
            line,
            preset,
            from: c[1].to_string(),
            send,
            to: c[3].to_string(),
            t,
            interp,
        });
    }
    Ok(out)
}

/// One send of a preset's rack program.
#[derive(Debug, Clone)]
pub struct RouteRow {
    /// Source stage name or `in`.
    pub from: String,
    /// Which send of the source (1 or 2).
    pub send: u8,
    /// Destination stage name or `out`.
    pub to: String,
    /// The send gain (0..2, morphable).
    pub gain: Val,
}

#[derive(Debug, Clone)]
struct Entry {
    stage: String,
    param: String,
    val: Val,
}

#[derive(Debug, Clone, Copy)]
struct Slot {
    t: [f32; 3],
    interp: Interp,
}

/// The voicing of one preset: its parameter values, route gains and morph slots.
#[derive(Debug, Clone, Default)]
pub struct PresetVoicing {
    entries: Vec<Entry>,
    routes: Vec<RouteRow>,
    slots: Vec<Slot>,
}

fn is_morph(t: [f32; 3]) -> bool {
    t[0].to_bits() != t[1].to_bits() || t[1].to_bits() != t[2].to_bits()
}

impl PresetVoicing {
    fn add_val(&mut self, t: [f32; 3], interp: Interp) -> Val {
        if is_morph(t) {
            let slot = self.slots.len().min(usize::from(NO_SLOT - 1)) as u8;
            self.slots.push(Slot { t, interp });
            Val { base: t[1], slot }
        } else {
            Val::constant(t[1])
        }
    }

    /// The voicing value of `stage.param`; a constant 0 when the table has no such row (a stub).
    pub fn val(&self, stage: &str, param: &str) -> Val {
        self.entries
            .iter()
            .find(|e| e.stage == stage && e.param == param)
            .map_or(Val::constant(0.0), |e| e.val)
    }

    /// The route rows in table order.
    pub fn routes(&self) -> &[RouteRow] {
        &self.routes
    }

    /// Number of morph values (voicing rows and route gains whose points differ).
    pub fn morph_count(&self) -> usize {
        self.slots.len()
    }

    /// True when the preset has any voicing row.
    pub fn has_rows(&self) -> bool {
        !self.entries.is_empty()
    }

    /// Evaluates every morph value at Transform `t` into `m` (call every control block).
    pub fn resolve(&self, t: f32, m: &mut Morph) {
        for (v, s) in m.v.iter_mut().zip(&self.slots) {
            *v = eval3(s.t, s.interp, t);
        }
    }

    /// The three table points and the tag of morph slot `slot`.
    pub fn slot_points(&self, slot: usize) -> Option<([f32; 3], Interp)> {
        self.slots.get(slot).map(|s| (s.t, s.interp))
    }
}

/// The 16 presets' voicings.
#[derive(Debug, Clone)]
pub struct Voicing {
    presets: Vec<PresetVoicing>,
}

impl Voicing {
    /// 16 presets without any row (used when the embedded tables are rejected, which the unit
    /// tests make impossible).
    pub fn empty() -> Self {
        Self {
            presets: vec![PresetVoicing::default(); 16],
        }
    }

    /// The voicing of preset `i` (clamped to 0..=15).
    pub fn preset(&self, i: usize) -> &PresetVoicing {
        &self.presets[i.min(15)]
    }

    /// Loads the tables.
    ///
    /// # Errors
    /// Any malformed row, unknown key, out-of-range value, incomplete implemented preset, preset
    /// with too many morph values or bad route row.
    pub fn load(voicing: &str, routes: &str, specs: &[ChainSpec]) -> Result<Self, VoicingError> {
        let rows = parse_voicing(voicing)?;
        let raw_routes = parse_routes(routes)?;
        let mut presets = vec![PresetVoicing::default(); 16];
        for r in &rows {
            let spec = specs.get(r.preset).ok_or(VoicingError::UnknownPreset {
                line: r.line,
                key: PRESET_KEYS[r.preset].to_string(),
            })?;
            let unknown = || VoicingError::UnknownParam {
                preset: r.preset,
                stage: r.stage.clone(),
                param: r.param.clone(),
            };
            let p = spec.find(&r.stage, &r.param).ok_or_else(unknown)?;
            if r.t.iter().any(|v| *v < p.min - 1e-6 || *v > p.max + 1e-6) {
                return Err(VoicingError::OutOfRange {
                    preset: r.preset,
                    stage: r.stage.clone(),
                    param: r.param.clone(),
                });
            }
            let pv = &mut presets[r.preset];
            let val = pv.add_val(r.t, r.interp);
            pv.entries.push(Entry {
                stage: r.stage.clone(),
                param: r.param.clone(),
                val,
            });
        }
        for rt in &raw_routes {
            let spec = specs
                .get(rt.preset)
                .ok_or(VoicingError::BadRoute { line: rt.line })?;
            let from_ok = rt.from == "in" || (rt.from != "chain" && spec.has_stage(&rt.from));
            let to_ok = rt.to == "out" || (rt.to != "chain" && spec.has_stage(&rt.to));
            if !from_ok || !to_ok {
                return Err(VoicingError::BadRoute { line: rt.line });
            }
            let pv = &mut presets[rt.preset];
            let gain = pv.add_val(rt.t, rt.interp);
            pv.routes.push(RouteRow {
                from: rt.from.clone(),
                send: rt.send,
                to: rt.to.clone(),
                gain,
            });
        }
        for (i, pv) in presets.iter().enumerate() {
            if pv.slots.len() > MAX_MORPH {
                return Err(VoicingError::TooManyMorph {
                    preset: i,
                    count: pv.slots.len(),
                });
            }
            let Some(spec) = specs.get(i) else { continue };
            if spec.implemented {
                for s in &spec.stages {
                    for p in s.params {
                        if !pv
                            .entries
                            .iter()
                            .any(|e| e.stage == s.name && e.param == p.name)
                        {
                            return Err(VoicingError::Incomplete {
                                preset: i,
                                stage: s.name.to_string(),
                                param: p.name.to_string(),
                            });
                        }
                    }
                }
            }
        }
        Ok(Self { presets })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAT_PARAMS: &[ParamSpec] = &[
        ParamSpec {
            name: "drive_db",
            min: 0.0,
            max: 48.0,
        },
        ParamSpec {
            name: "makeup_db",
            min: -24.0,
            max: 12.0,
        },
    ];
    const CHAIN_PARAMS: &[ParamSpec] = &[ParamSpec {
        name: "target_gain_db",
        min: -12.0,
        max: 12.0,
    }];

    fn specs() -> Vec<ChainSpec> {
        (0..16)
            .map(|i| ChainSpec {
                stages: vec![
                    StageSpec {
                        name: "sat",
                        params: SAT_PARAMS,
                    },
                    StageSpec {
                        name: "chain",
                        params: CHAIN_PARAMS,
                    },
                ],
                implemented: i == 0,
            })
            .collect()
    }

    const HEAD: &str = "preset,stage,param,t0,t50,t100,interp,source,note\n";

    fn good() -> String {
        format!(
            "{HEAD}reel,sat,drive_db,3,9,18,lin,original,\nreel,sat,makeup_db,-1,-1,-1,lin,original,\nreel,chain,target_gain_db,0,0,0,lin,original,\n"
        )
    }
    const ROUTES_HEAD: &str = "preset,from,send,to,gain_t0,gain_t50,gain_t100,interp,source,note\n";

    #[test]
    fn a_complete_table_loads_and_resolves_the_morph() {
        let v = Voicing::load(&good(), ROUTES_HEAD, &specs()).unwrap();
        let p = v.preset(0);
        assert_eq!(p.morph_count(), 1);
        let drive = p.val("sat", "drive_db");
        let mut m = Morph::default();
        for (t, want) in [(0.0, 3.0), (0.5, 9.0), (1.0, 18.0), (0.25, 6.0)] {
            p.resolve(t, &mut m);
            assert!((drive.at(&m) - want).abs() < 1e-5, "t={t}");
        }
        assert!((p.val("sat", "makeup_db").at(&m) + 1.0).abs() < 1e-6);
        assert!(p.has_rows());
        assert!(!v.preset(1).has_rows());
    }

    #[test]
    fn interpolation_tags_follow_their_shapes_and_hit_the_table_points() {
        for interp in [
            Interp::Lin,
            Interp::Log,
            Interp::Square,
            Interp::Exp,
            Interp::Step,
        ] {
            let t = [100.0, 400.0, 1600.0];
            for (x, want) in [(0.0, 100.0), (0.5, 400.0), (1.0, 1600.0)] {
                assert!(
                    (eval3(t, interp, x) - want).abs() < 1e-3 * want,
                    "{interp:?} {x}"
                );
            }
        }
        let t = [100.0, 200.0, 400.0];
        assert!((eval3(t, Interp::Lin, 0.25) - 150.0).abs() < 1e-4);
        assert!((eval3(t, Interp::Log, 0.25) - 141.4214).abs() < 0.01);
        assert!((eval3(t, Interp::Square, 0.25) - 125.0).abs() < 1e-4);
        let e = eval3(t, Interp::Exp, 0.25);
        assert!(e > 100.0 && e < 125.0, "flat start: {e}");
        assert!((eval3(t, Interp::Step, 0.49) - 100.0).abs() < 1e-6);
        assert!((eval3(t, Interp::Step, 0.5) - 200.0).abs() < 1e-6);
        assert!((eval3(t, Interp::Step, 0.99) - 200.0).abs() < 1e-6);
        // Log falls back to linear when an end is not positive.
        assert!((eval3([0.0, 1.0, 2.0], Interp::Log, 0.25) - 0.5).abs() < 1e-6);
        // Out-of-range transform clamps.
        assert!((eval3(t, Interp::Lin, -3.0) - 100.0).abs() < 1e-6);
        assert!((eval3(t, Interp::Lin, 3.0) - 400.0).abs() < 1e-6);
    }

    #[test]
    fn malformed_tables_are_rejected_with_their_line() {
        let s = specs();
        let bad = |rows: &str| Voicing::load(&format!("{HEAD}{rows}"), ROUTES_HEAD, &s);
        assert!(matches!(
            Voicing::load("a,b\n", ROUTES_HEAD, &s),
            Err(VoicingError::Header { .. })
        ));
        assert!(matches!(
            bad("reel,sat,drive_db,3,9\n"),
            Err(VoicingError::Columns { line: 2 })
        ));
        assert!(matches!(
            bad("nope,sat,drive_db,3,9,18,lin,original,\n"),
            Err(VoicingError::UnknownPreset { line: 2, .. })
        ));
        assert!(matches!(
            bad("reel,sat,drive_db,x,9,18,lin,original,\n"),
            Err(VoicingError::BadNumber { line: 2 })
        ));
        assert!(matches!(
            bad("reel,sat,drive_db,3,9,18,cubic,original,\n"),
            Err(VoicingError::BadInterp { line: 2 })
        ));
        assert!(matches!(
            bad("reel,sat,drive_db,3,9,18,lin,someone,\n"),
            Err(VoicingError::BadSource { line: 2 })
        ));
        assert!(matches!(
            bad("reel,sat,drive_db,3,9,18,lin,original,\nreel,sat,drive_db,3,9,18,lin,original,\n"),
            Err(VoicingError::Duplicate { line: 3 })
        ));
        assert!(matches!(
            bad("reel,sat,nope,3,9,18,lin,original,\n"),
            Err(VoicingError::UnknownParam { .. })
        ));
        assert!(matches!(
            bad("reel,sat,drive_db,3,9,99,lin,original,\n"),
            Err(VoicingError::OutOfRange { .. })
        ));
        assert!(matches!(
            bad("reel,sat,drive_db,3,NaN,9,lin,original,\n"),
            Err(VoicingError::BadNumber { .. })
        ));
    }

    #[test]
    fn an_incomplete_implemented_preset_is_rejected() {
        let t = format!("{HEAD}reel,sat,drive_db,3,9,18,lin,original,\n");
        assert!(matches!(
            Voicing::load(&t, ROUTES_HEAD, &specs()),
            Err(VoicingError::Incomplete { preset: 0, .. })
        ));
    }

    #[test]
    fn more_than_eight_morph_values_are_rejected() {
        let names: Vec<&'static str> = (0..9)
            .map(|i| &*Box::leak(format!("p{i}").into_boxed_str()))
            .collect();
        let params: &'static [ParamSpec] = Box::leak(
            names
                .iter()
                .map(|n| ParamSpec {
                    name: n,
                    min: 0.0,
                    max: 10.0,
                })
                .collect::<Vec<_>>()
                .into_boxed_slice(),
        );
        let s: Vec<ChainSpec> = (0..16)
            .map(|_| ChainSpec {
                stages: vec![StageSpec { name: "x", params }],
                implemented: true,
            })
            .collect();
        let mut t = String::from(HEAD);
        for key in PRESET_KEYS {
            for i in 0..9 {
                t.push_str(&format!("{key},x,p{i},1,2,3,lin,original,\n"));
            }
        }
        let e = Voicing::load(&t, ROUTES_HEAD, &s).err();
        assert!(
            matches!(
                e,
                Some(VoicingError::TooManyMorph {
                    preset: 0,
                    count: 9
                })
            ),
            "{e:?}"
        );
    }

    #[test]
    fn route_rows_are_parsed_and_their_gains_morph() {
        let routes = format!(
            "{ROUTES_HEAD}reel,in,1,sat,1,1,1,lin,original,\nreel,sat,2,out,0,0.5,1,lin,original,\n"
        );
        let v = Voicing::load(&good(), &routes, &specs()).unwrap();
        let p = v.preset(0);
        assert_eq!(p.routes().len(), 2);
        assert_eq!(p.routes()[1].from, "sat");
        assert_eq!(p.routes()[1].send, 2);
        assert_eq!(p.routes()[1].to, "out");
        assert_eq!(p.morph_count(), 2);
        let mut m = Morph::default();
        p.resolve(1.0, &mut m);
        assert!((p.routes()[1].gain.at(&m) - 1.0).abs() < 1e-6);
        assert!((p.routes()[0].gain.at(&m) - 1.0).abs() < 1e-6);
        for bad in [
            "reel,in,3,sat,1,1,1,lin,original,\n",
            "reel,in,1,sat,3,1,1,lin,original,\n",
            "reel,nowhere,1,sat,1,1,1,lin,original,\n",
            "reel,in,1,out,1,1,1,sinc,original,\n",
        ] {
            let r = Voicing::load(&good(), &format!("{ROUTES_HEAD}{bad}"), &specs());
            assert!(
                matches!(
                    r,
                    Err(VoicingError::BadRoute { .. } | VoicingError::BadInterp { .. })
                ),
                "{bad}"
            );
        }
    }

    #[test]
    fn preset_keys_are_the_sixteen_s11_keys_in_order() {
        assert_eq!(PRESET_KEYS[0], "reel");
        assert_eq!(PRESET_KEYS[7], "dusty-groove");
        assert_eq!(PRESET_KEYS[15], "ghost-pipes");
        assert_eq!(PRESET_KEYS.len(), 16);
    }
}
