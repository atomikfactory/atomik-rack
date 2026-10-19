//! The generic rack engine: a preset that fits a stage graph is a
//! list of stages in code ([`RackDef`]: names and kinds) plus the send table in
//! `character-routes.csv` and the numbers in `character-voicing.csv`; one [`RackChain`]
//! interprets it.
//!
//! * **Router.** Every stage has one input bus, the sum of the sends addressed to it, and two
//!   sends (send 1 and send 2; for a [`Kind::Split`] stage send 1 is the low band and send 2 the
//!   high band, for every other stage both carry the stage output). Each route has a gain from
//!   the table, morphable and smoothed (second-order, fast class). `in` is the chain input, `out`
//!   the wet bus. Generators ([`Kind::generator`]) have no audio input.
//! * **Load-time checks** ([`RackError`]): the graph is acyclic, no route enters a generator,
//!   every stage that has routes is reachable from `in` or a generator and reaches `out`;
//!   the execution order is a topological order (ties broken by declaration order).
//! * **Lazy evaluation.** Per control block a stage runs only if some active route (gain above
//!   0) feeds it from a running source and some active route leads from it to `out`; a stage
//!   that does not run does no work and its state stands still (delay lines, LFO phases,
//!   envelopes keep what they hold). A declared stage without any route never runs.
//! * **Path alignment.** The latency of every path to a bus is computed at load; a route whose
//!   source arrives earlier than the latest source of the same bus gets an integer delay, so
//!   parallel branches rejoin without comb filtering. Generators are not delayed. The wet bus is
//!   finally padded up to the class latency ([`class_latency`]: the short class, or the long class
//!   when the preset has a stage that lifts it, the spectral stage); a chain whose natural
//!   latency exceeds it is rejected.

use super::chain::{
    Activity, Bus, ChainCommon, Ctx, PresetChain, Routing, SUB, StereoDelay, chain_stage_spec,
    short_latency,
};
pub use super::stages::Kind;
use super::stages::{BuildInfo, Stage, build};
use super::voicing::{ChainSpec, Morph, PresetVoicing, StageSpec, Val};
use atomik_dsp::rng::hash64;
use atomik_dsp::{Smoother2, SmootherClass};

/// Most stages one rack preset may declare.
pub const MAX_STAGES: usize = 30;

/// The class latency of a preset in frames: [`short_latency`] plus the largest class extra of its
/// stages (the spectral frame for a preset with the spectral stage, so the long class is
/// `short_latency + frame`).
pub fn class_latency(def: &RackDef, sample_rate: f32) -> u32 {
    short_latency(sample_rate)
        + def
            .stages
            .iter()
            .map(|s| s.kind.class_extra(sample_rate))
            .max()
            .unwrap_or(0)
}

/// One stage of a rack preset.
#[derive(Debug, Clone, Copy)]
pub struct StageDef {
    /// The `stage` name used in the tables.
    pub name: &'static str,
    /// What the stage is.
    pub kind: Kind,
}

/// The code half of a rack preset: its stages and how the device feeds it.
#[derive(Debug)]
pub struct RackDef {
    /// The stages in declaration order.
    pub stages: &'static [StageDef],
    /// Input and output routing.
    pub routing: Routing,
}

/// Why a route table was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RackError {
    /// The graph has a cycle.
    Cyclic,
    /// A stage with routes cannot be reached from the input or a generator.
    Unreachable(&'static str),
    /// A stage with routes does not reach the output.
    DeadEnd(&'static str),
    /// A route enters a generator.
    IntoGenerator(&'static str),
    /// A route names a stage the preset does not declare.
    UnknownStage(String),
    /// The natural latency exceeds the class latency.
    Overlong {
        /// Latency of the longest path.
        natural: u32,
        /// The class latency.
        target: u32,
    },
}

impl std::fmt::Display for RackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

/// A compiled route: node 0 is the input, node `i + 1` stage `i`, node `n + 1` the output.
#[derive(Debug, Clone, Copy)]
struct RouteDef {
    from: usize,
    to: usize,
    send: u8,
    delay: u32,
}

#[derive(Debug, Clone)]
struct Program {
    routes: Vec<RouteDef>,
    /// Stage indices in execution order (only stages with routes).
    order: Vec<usize>,
    /// Alignment pad of the output bus.
    pad: u32,
    /// Latency of the chain (natural plus pad).
    latency: u32,
}

impl Program {
    fn compile(
        def: &'static RackDef,
        pv: &PresetVoicing,
        sample_rate: f32,
        target: u32,
    ) -> Result<Self, RackError> {
        let n = def.stages.len();
        if n > MAX_STAGES {
            return Err(RackError::UnknownStage("too many stages".to_string()));
        }
        let out = n + 1;
        let mut routes = Vec::new();
        let node = |name: &str, dest: bool| -> Result<usize, RackError> {
            if name == "in" && !dest {
                Ok(0)
            } else if name == "out" && dest {
                Ok(out)
            } else {
                def.stages
                    .iter()
                    .position(|s| s.name == name)
                    .map(|i| i + 1)
                    .ok_or_else(|| RackError::UnknownStage(name.to_string()))
            }
        };
        for r in pv.routes() {
            let from = node(&r.from, false)?;
            let to = node(&r.to, true)?;
            if to <= n && def.stages[to - 1].kind.generator() {
                return Err(RackError::IntoGenerator(def.stages[to - 1].name));
            }
            routes.push(RouteDef {
                from,
                to,
                send: r.send,
                delay: 0,
            });
        }
        let used: Vec<bool> = (0..n)
            .map(|i| routes.iter().any(|r| r.from == i + 1 || r.to == i + 1))
            .collect();
        // Kahn's algorithm over the used stages, lowest index first.
        let mut indeg = vec![0usize; n];
        for r in &routes {
            if r.from >= 1 && r.to <= n {
                indeg[r.to - 1] += 1;
            }
        }
        let mut order = Vec::new();
        let mut done = vec![false; n];
        loop {
            let next = (0..n).find(|&i| used[i] && !done[i] && indeg[i] == 0);
            let Some(s) = next else { break };
            done[s] = true;
            order.push(s);
            for r in &routes {
                if r.from == s + 1 && r.to <= n {
                    indeg[r.to - 1] -= 1;
                }
            }
        }
        if order.len() != used.iter().filter(|u| **u).count() {
            return Err(RackError::Cyclic);
        }
        // Forward reachability from the input and the generators.
        let mut reached = vec![false; n];
        for &s in &order {
            reached[s] = def.stages[s].kind.generator()
                || routes
                    .iter()
                    .any(|r| r.to == s + 1 && (r.from == 0 || reached[r.from - 1]));
            if !reached[s] {
                return Err(RackError::Unreachable(def.stages[s].name));
            }
        }
        // Backward: every used stage reaches the output.
        let mut leads = vec![false; n];
        for &s in order.iter().rev() {
            leads[s] = routes
                .iter()
                .any(|r| r.from == s + 1 && (r.to == out || leads[r.to - 1]));
            if !leads[s] {
                return Err(RackError::DeadEnd(def.stages[s].name));
            }
        }
        // Arrival latencies (None: a generator-only path, which is never delayed).
        let mut arrival: Vec<Option<u32>> = vec![None; n + 2];
        arrival[0] = Some(0);
        let bus_arrival = |arrival: &[Option<u32>], routes: &[RouteDef], dest: usize| {
            routes
                .iter()
                .filter(|r| r.to == dest)
                .filter_map(|r| arrival[r.from])
                .max()
        };
        for &s in &order {
            let kind = def.stages[s].kind;
            arrival[s + 1] = if kind.generator() {
                None
            } else {
                bus_arrival(&arrival, &routes, s + 1).map(|a| a + kind.latency(sample_rate))
            };
        }
        arrival[out] = bus_arrival(&arrival, &routes, out);
        for i in 0..routes.len() {
            let dest = routes[i].to;
            if let (Some(a), Some(top)) = (
                arrival[routes[i].from],
                bus_arrival(&arrival, &routes, dest),
            ) {
                routes[i].delay = top - a;
            }
        }
        let natural = arrival[out].unwrap_or(0);
        if natural > target {
            return Err(RackError::Overlong { natural, target });
        }
        Ok(Self {
            routes,
            order,
            pad: target - natural,
            latency: target,
        })
    }
}

struct StageRt {
    def: StageDef,
    stage: Box<dyn Stage>,
    params: Vec<Val>,
    limits: &'static [super::voicing::ParamSpec],
    scratch: Vec<f32>,
    out1: Bus,
    out2: Bus,
    two: bool,
    latency: u32,
    evals: u64,
}

struct RouteRt {
    def: RouteDef,
    gain: Val,
    sm: Smoother2,
    delay: Option<StereoDelay>,
    g: [f32; SUB],
    active: bool,
}

struct Rt {
    stages: Vec<StageRt>,
    routes: Vec<RouteRt>,
    order: Vec<usize>,
    pad: StereoDelay,
    latency: u32,
    common: ChainCommon,
    trim: f32,
    fresh: bool,
    activity: Activity,
}

/// A rack preset: the stages of a [`RackDef`] driven by the tables.
pub struct RackChain {
    def: &'static RackDef,
    rt: Option<Rt>,
}

impl RackChain {
    /// A chain for `def` (call [`PresetChain::prepare`] before processing).
    pub fn new(def: &'static RackDef) -> Self {
        Self { def, rt: None }
    }

    /// How often stage `stage` (declaration index) has been evaluated (test and tooling hook).
    pub fn eval_count(&self, stage: usize) -> u64 {
        self.rt
            .as_ref()
            .and_then(|r| r.stages.get(stage))
            .map_or(0, |s| s.evals)
    }

    /// The latency of stage `stage` in frames.
    pub fn stage_latency(&self, stage: usize) -> u32 {
        self.rt
            .as_ref()
            .and_then(|r| r.stages.get(stage))
            .map_or(0, |s| s.latency)
    }

    /// The alignment delay of each route in table order.
    pub fn route_delays(&self) -> Vec<u32> {
        self.rt
            .as_ref()
            .map(|r| r.routes.iter().map(|x| x.def.delay).collect())
            .unwrap_or_default()
    }

    /// Compiles the route table against the declared stages (the load-time checks).
    ///
    /// # Errors
    /// A [`RackError`] for a cycle, an unreachable stage, a dead end, a route into a generator,
    /// an unknown stage name or a latency above the class latency.
    pub fn check_program(&self, v: &PresetVoicing, sample_rate: f32) -> Result<(), RackError> {
        Program::compile(
            self.def,
            v,
            sample_rate,
            class_latency(self.def, sample_rate),
        )
        .map(|_| ())
    }

    /// The names of the stages in execution order.
    pub fn execution_order(&self) -> Vec<&'static str> {
        self.rt
            .as_ref()
            .map(|r| r.order.iter().map(|&i| self.def.stages[i].name).collect())
            .unwrap_or_default()
    }
}

impl PresetChain for RackChain {
    fn spec(&self) -> ChainSpec {
        let mut stages: Vec<StageSpec> = self
            .def
            .stages
            .iter()
            .map(|s| StageSpec {
                name: s.name,
                params: s.kind.params(),
            })
            .collect();
        stages.push(chain_stage_spec());
        ChainSpec {
            stages,
            implemented: true,
        }
    }

    fn check(&self, v: &PresetVoicing, sample_rate: f32) -> Result<(), String> {
        self.check_program(v, sample_rate)
            .map_err(|e| e.to_string())
    }

    fn prepare(&mut self, sample_rate: f32, seed: u64, v: &PresetVoicing) {
        let target = class_latency(self.def, sample_rate);
        let Ok(prog) = Program::compile(self.def, v, sample_rate, target) else {
            // The loader check rejects such tables before they get here; a chain that cannot be
            // compiled stays unprepared and passes its input through.
            self.rt = None;
            return;
        };
        let common = ChainCommon::read(v);
        let follows = common.noise_follows_gain.base() >= 0.5;
        let stages: Vec<StageRt> = self
            .def
            .stages
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let stage = build(
                    d.kind,
                    &BuildInfo {
                        sample_rate,
                        seed: hash64(seed, 0x57A6E, i as u64),
                        noise_follows_gain: follows,
                    },
                );
                let limits = d.kind.params();
                StageRt {
                    def: *d,
                    two: stage.two_outputs(),
                    latency: stage.latency(),
                    stage,
                    params: limits.iter().map(|p| v.val(d.name, p.name)).collect(),
                    limits,
                    scratch: vec![0.0; limits.len()],
                    out1: [[0.0; SUB]; 2],
                    out2: [[0.0; SUB]; 2],
                    evals: 0,
                }
            })
            .collect();
        let routes = prog
            .routes
            .iter()
            .zip(v.routes())
            .map(|(r, row)| RouteRt {
                def: *r,
                gain: row.gain,
                sm: Smoother2::with_class(0.0, SmootherClass::Fast, sample_rate),
                delay: (r.delay > 0).then(|| StereoDelay::new(r.delay as usize)),
                g: [0.0; SUB],
                active: false,
            })
            .collect();
        self.rt = Some(Rt {
            stages,
            routes,
            order: prog.order,
            pad: StereoDelay::new(prog.pad as usize),
            latency: prog.latency,
            trim: common.trim(),
            common,
            fresh: true,
            activity: Activity::new(sample_rate),
        });
    }

    fn reset(&mut self) {
        if let Some(rt) = self.rt.as_mut() {
            rt.reset();
        }
    }

    fn process(&mut self, l: &mut [f32; SUB], r: &mut [f32; SUB], m: &Morph, ctx: &Ctx) {
        if let Some(rt) = self.rt.as_mut() {
            rt.process(l, r, m, ctx);
        }
    }

    fn latency(&self) -> u32 {
        self.rt.as_ref().map_or(0, |r| r.latency)
    }

    fn routing(&self) -> Routing {
        self.def.routing
    }

    fn activity(&self) -> f32 {
        self.rt.as_ref().map_or(0.0, |r| r.activity.value())
    }

    fn presence(&self) -> (f32, f32) {
        self.rt.as_ref().map_or((3.0, 2.0), |r| r.common.presence())
    }
}

/// Sends below this (about -80 dB) are closed once their target is 0.
const GATE_FLOOR: f32 = 1.0e-4;

impl Rt {
    fn reset(&mut self) {
        for s in &mut self.stages {
            s.stage.reset();
            s.out1 = [[0.0; SUB]; 2];
            s.out2 = [[0.0; SUB]; 2];
        }
        for r in &mut self.routes {
            if let Some(d) = r.delay.as_mut() {
                d.reset();
            }
            r.sm.set_immediate(0.0);
            r.g = [0.0; SUB];
            r.active = false;
        }
        self.pad.reset();
        self.fresh = true;
        self.activity.reset();
    }

    fn process(&mut self, l: &mut [f32; SUB], r: &mut [f32; SUB], m: &Morph, ctx: &Ctx) {
        let snap = self.fresh;
        self.fresh = false;
        let in_bus: Bus = [*l, *r];
        let n_stages = self.stages.len();
        let out_node = n_stages + 1;

        for s in &mut self.stages {
            for (i, v) in s.params.iter().enumerate() {
                let lim = &s.limits[i];
                let x = v.at(m);
                s.scratch[i] = if x.is_finite() {
                    x.clamp(lim.min, lim.max)
                } else {
                    lim.min
                };
            }
            s.stage.set(&s.scratch, snap);
        }
        for r in &mut self.routes {
            let target = {
                let g = r.gain.at(m);
                if g.is_finite() {
                    g.clamp(0.0, 2.0)
                } else {
                    0.0
                }
            };
            if snap {
                r.sm.set_immediate(target);
            } else {
                r.sm.set_target(target);
            }
            for g in &mut r.g {
                *g = r.sm.next_value();
            }
            if target <= 0.0 && r.g[SUB - 1] < GATE_FLOOR {
                r.sm.set_immediate(0.0);
                for g in &mut r.g {
                    if *g < GATE_FLOOR {
                        *g = 0.0;
                    }
                }
            }
            r.active = target > 0.0 || r.g[0] > 0.0;
        }

        // Which stages run: fed by a running source and leading to the output.
        let mut needed = [false; 32];
        for &s in self.order.iter().rev() {
            needed[s] = self.routes.iter().any(|r| {
                r.active && r.def.from == s + 1 && (r.def.to == out_node || needed[r.def.to - 1])
            });
        }
        let mut ran = [false; 32];
        for &s in &self.order {
            let fed = self.stages[s].def.kind.generator()
                || self.routes.iter().any(|r| {
                    r.active && r.def.to == s + 1 && (r.def.from == 0 || ran[r.def.from - 1])
                });
            ran[s] = fed && needed[s];
        }

        for oi in 0..self.order.len() {
            let s = self.order[oi];
            if !ran[s] {
                continue;
            }
            let mut inp: Bus = [[0.0; SUB]; 2];
            for ri in 0..self.routes.len() {
                let rd = self.routes[ri].def;
                if rd.to != s + 1 || !self.routes[ri].active {
                    continue;
                }
                let Some(src) = source_bus(&self.stages, &in_bus, &ran, rd) else {
                    continue;
                };
                contribute(&mut self.routes[ri], &src, &mut inp);
            }
            let st = &mut self.stages[s];
            st.stage.process(&inp, &mut st.out1, &mut st.out2, ctx);
            st.evals += 1;
        }

        let mut acc: Bus = [[0.0; SUB]; 2];
        for ri in 0..self.routes.len() {
            let rd = self.routes[ri].def;
            if rd.to != out_node || !self.routes[ri].active {
                continue;
            }
            let Some(src) = source_bus(&self.stages, &in_bus, &ran, rd) else {
                continue;
            };
            contribute(&mut self.routes[ri], &src, &mut acc);
        }

        for n in 0..SUB {
            let (a, b) = self.pad.process(acc[0][n], acc[1][n]);
            let (a, b) = (a * self.trim, b * self.trim);
            let a = if a.is_finite() { a } else { 0.0 };
            let b = if b.is_finite() { b } else { 0.0 };
            l[n] = a;
            r[n] = b;
            self.activity.update(a, b);
        }
    }
}

/// The block a route reads: the chain input or a stage output; `None` when the source stage did
/// not run in this block.
fn source_bus(stages: &[StageRt], in_bus: &Bus, ran: &[bool; 32], rd: RouteDef) -> Option<Bus> {
    if rd.from == 0 {
        return Some(*in_bus);
    }
    let s = rd.from - 1;
    if !ran[s] {
        return None;
    }
    let st = &stages[s];
    Some(if rd.send == 2 && st.two {
        st.out2
    } else {
        st.out1
    })
}

fn contribute(route: &mut RouteRt, src: &Bus, dst: &mut Bus) {
    let mut s = *src;
    if let Some(d) = route.delay.as_mut() {
        let [l, r] = &mut s;
        for (a, b) in l.iter_mut().zip(r.iter_mut()) {
            (*a, *b) = d.process(*a, *b);
        }
    }
    let [dl, dr] = dst;
    for (n, g) in route.g.iter().enumerate() {
        dl[n] += g * s[0][n];
        dr[n] += g * s[1][n];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::character::chain::{Ctx, PresetChain, SUB, short_latency};
    use crate::character::voicing::{Morph, PresetVoicing, Voicing};
    use atomik_dsp::{Rng, ShaperKind};

    const ROUTES_HEAD: &str = "preset,from,send,to,gain_t0,gain_t50,gain_t100,interp,source,note\n";
    const HEAD: &str = "preset,stage,param,t0,t50,t100,interp,source,note\n";
    const CHAIN_ROWS: &str = "reel,chain,target_gain_db,0,0,0,lin,original,\nreel,chain,noise_follows_gain,0,0,0,lin,original,\nreel,chain,presence_hold_s,3,3,3,lin,original,\nreel,chain,presence_fade_s,2,2,2,lin,original,\n";

    macro_rules! def {
        ($name:ident, [$($st:expr),*]) => {
            static $name: RackDef = RackDef {
                stages: &[$($st),*],
                routing: Routing::Stereo,
            };
        };
    }
    const fn st(name: &'static str, kind: Kind) -> StageDef {
        StageDef { name, kind }
    }

    def!(LINEAR, [st("pk", Kind::Peak)]);
    def!(WOB, [st("w", Kind::Wobble)]);
    def!(CYC, [st("a", Kind::Peak), st("b", Kind::Peak)]);
    def!(TWO, [st("a", Kind::Peak), st("b", Kind::Peak)]);
    def!(SPLIT, [st("x", Kind::Split)]);
    def!(NOISE, [st("h", Kind::Hiss)]);
    def!(GENIN, [st("h", Kind::Hiss), st("a", Kind::Peak)]);
    def!(SAT, [st("s", Kind::Sat(ShaperKind::SoftClip))]);
    def!(UNUSED, [st("a", Kind::Peak), st("b", Kind::Peak)]);
    def!(SPEC, [st("s", Kind::Spectral)]);

    fn rows_for(def: &RackDef, over: &[(&str, &str, [f32; 3])]) -> String {
        // Constant rows at neutral values for every declared param, then the overrides.
        let mut out = String::from(HEAD);
        for s in def.stages {
            for p in s.kind.params() {
                let t = over
                    .iter()
                    .find(|o| o.0 == s.name && o.1 == p.name)
                    .map_or_else(|| neutral(p.name), |o| o.2);
                out.push_str(&format!(
                    "reel,{},{},{},{},{},lin,original,\n",
                    s.name, p.name, t[0], t[1], t[2]
                ));
            }
        }
        out.push_str(CHAIN_ROWS);
        out
    }

    fn neutral(name: &str) -> [f32; 3] {
        let v = match name {
            "freq_hz" => 1000.0,
            "q" => 0.7,
            "gain_db" | "drive_db" | "makeup_db" => 0.0,
            "depth_ms" => 0.0,
            "wow_hz" => 0.6,
            "flutter_hz" => 6.0,
            "level_db" => -20.0,
            _ => 0.0,
        };
        [v, v, v]
    }

    struct Fixture {
        chain: RackChain,
        pv: PresetVoicing,
    }

    fn try_build(
        def: &'static RackDef,
        over: &[(&str, &str, [f32; 3])],
        routes: &str,
        sr: f32,
    ) -> Result<Fixture, RackError> {
        let chain0 = RackChain::new(def);
        let mut specs: Vec<_> = (0..16)
            .map(|_| crate::character::voicing::ChainSpec {
                stages: vec![crate::character::chain::chain_stage_spec()],
                implemented: false,
            })
            .collect();
        specs[0] = chain0.spec();
        let v = Voicing::load(
            &rows_for(def, over),
            &format!("{ROUTES_HEAD}{routes}"),
            &specs,
        )
        .expect("fixture tables load");
        let pv = v.preset(0).clone();
        chain0.check_program(&pv, sr)?;
        let mut chain = RackChain::new(def);
        chain.prepare(sr, 7, &pv);
        Ok(Fixture { chain, pv })
    }

    fn build(def: &'static RackDef, over: &[(&str, &str, [f32; 3])], routes: &str) -> Fixture {
        try_build(def, over, routes, 48_000.0).expect("fixture compiles")
    }

    fn run_t(f: &mut Fixture, t: f32, left: &[f32], right: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let presence = [1.0f32; SUB];
        let gain = [1.0f32; SUB];
        let ctx = Ctx {
            presence: &presence,
            gain: &gain,
        };
        let mut m = Morph::default();
        let (mut ol, mut or) = (Vec::new(), Vec::new());
        for (cl, cr) in left.chunks(SUB).zip(right.chunks(SUB)) {
            f.pv.resolve(t, &mut m);
            let mut l = [0.0f32; SUB];
            let mut r = [0.0f32; SUB];
            l[..cl.len()].copy_from_slice(cl);
            r[..cr.len()].copy_from_slice(cr);
            f.chain.process(&mut l, &mut r, &m, &ctx);
            ol.extend_from_slice(&l[..cl.len()]);
            or.extend_from_slice(&r[..cr.len()]);
        }
        (ol, or)
    }

    fn impulse(n: usize, amp: f32) -> Vec<f32> {
        let mut v = vec![0.0; n];
        v[0] = amp;
        v
    }

    fn noise(n: usize, seed: u64) -> Vec<f32> {
        let mut r = Rng::new(seed);
        (0..n).map(|_| r.next_bipolar() * 0.5).collect()
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    fn argmax(x: &[f32]) -> usize {
        let mut best = 0;
        for (i, v) in x.iter().enumerate() {
            if v.abs() > x[best].abs() {
                best = i;
            }
        }
        best
    }

    const LIN_ROUTES: &str =
        "reel,in,1,pk,1,1,1,lin,original,\nreel,pk,1,out,1,1,1,lin,original,\n";

    #[test]
    fn a_linear_chain_with_a_flat_stage_is_transparent_after_its_pad() {
        let mut f = build(&LINEAR, &[], LIN_ROUTES);
        let x = noise(2048, 1);
        let (l, _) = run_t(&mut f, 0.5, &x, &x);
        let lat = f.chain.latency() as usize;
        assert_eq!(lat, short_latency(48_000.0) as usize);
        for n in lat..x.len() {
            assert!((l[n] - x[n - lat]).abs() < 1e-5, "n={n}");
        }
    }

    /// The spectral stage near its identity: the lowest gate, the finest quantiser, no dropout.
    const SPEC_OVER: &[(&str, &str, [f32; 3])] =
        &[("s", "thresh_db", [-100.0; 3]), ("s", "step_db", [0.5; 3])];

    #[test]
    fn a_spectral_stage_puts_the_chain_in_the_long_class_and_the_impulse_arrives_exactly() {
        for (sr, frame) in [(44_100.0f32, 1024u32), (48_000.0, 1024), (96_000.0, 2048)] {
            let routes = "reel,in,1,s,1,1,1,lin,original,
reel,s,1,out,1,1,1,lin,original,
";
            let mut f = try_build(&SPEC, SPEC_OVER, routes, sr).unwrap();
            let lat = short_latency(sr) + frame;
            assert_eq!(class_latency(&SPEC, sr), lat);
            assert_eq!(f.chain.latency(), lat, "sr {sr}");
            assert_eq!(f.chain.stage_latency(0), frame);
            // The class of the presets without the stage is the short one.
            assert_eq!(class_latency(&LINEAR, sr), short_latency(sr));
            let x = impulse(lat as usize + 600, 0.5);
            let (l, _) = run_t(&mut f, 0.5, &x, &x);
            assert_eq!(argmax(&l), lat as usize, "sr {sr}");
            assert!((l[lat as usize] - 0.5).abs() < 0.03, "{}", l[lat as usize]);
        }
    }

    #[test]
    fn a_direct_branch_beside_the_spectral_stage_is_delayed_to_align() {
        // The direct send gets the spectral frame as an alignment delay: both arrive together.
        let routes = "reel,in,1,out,1,1,1,lin,original,
reel,in,2,s,1,1,1,lin,original,
reel,s,1,out,1,1,1,lin,original,
";
        let mut f = try_build(&SPEC, SPEC_OVER, routes, 48_000.0).unwrap();
        assert_eq!(f.chain.route_delays(), vec![1024, 0, 0]);
        let lat = f.chain.latency() as usize;
        let x = impulse(lat + 600, 0.5);
        let (l, _) = run_t(&mut f, 0.5, &x, &x);
        assert_eq!(argmax(&l), lat);
        assert!(
            (l[lat] - 1.0).abs() < 0.04,
            "both branches coincide: {}",
            l[lat]
        );
    }

    #[test]
    fn a_cycle_is_rejected_at_load() {
        let routes = "reel,in,1,a,1,1,1,lin,original,\nreel,a,1,b,1,1,1,lin,original,\nreel,b,1,a,1,1,1,lin,original,\nreel,b,2,out,1,1,1,lin,original,\n";
        let e = try_build(&CYC, &[], routes, 48_000.0).err();
        assert_eq!(e, Some(RackError::Cyclic));
    }

    #[test]
    fn unreachable_and_dead_end_stages_are_rejected() {
        // b has a route to out but nothing feeds it.
        let routes = "reel,in,1,a,1,1,1,lin,original,\nreel,a,1,out,1,1,1,lin,original,\nreel,b,1,out,1,1,1,lin,original,\n";
        assert_eq!(
            try_build(&TWO, &[], routes, 48_000.0).err(),
            Some(RackError::Unreachable("b"))
        );
        // b is fed but never reaches out.
        let routes = "reel,in,1,a,1,1,1,lin,original,\nreel,a,1,out,1,1,1,lin,original,\nreel,a,2,b,1,1,1,lin,original,\n";
        assert_eq!(
            try_build(&TWO, &[], routes, 48_000.0).err(),
            Some(RackError::DeadEnd("b"))
        );
    }

    #[test]
    fn a_route_into_a_generator_is_rejected() {
        let routes = "reel,in,1,h,1,1,1,lin,original,\nreel,h,1,a,1,1,1,lin,original,\nreel,a,1,out,1,1,1,lin,original,\n";
        assert_eq!(
            try_build(&GENIN, &[], routes, 48_000.0).err(),
            Some(RackError::IntoGenerator("h"))
        );
    }

    #[test]
    fn the_execution_order_is_topological() {
        let routes = "reel,in,1,b,1,1,1,lin,original,\nreel,b,1,a,1,1,1,lin,original,\nreel,a,1,out,1,1,1,lin,original,\n";
        let f = build(&TWO, &[], routes);
        assert_eq!(f.chain.execution_order(), vec!["b", "a"]);
    }

    #[test]
    fn parallel_paths_with_different_latencies_are_aligned_and_sum_coherently() {
        // Direct path plus a wobble path (a pure delay at depth 0): the direct path gets the delay.
        let routes = "reel,in,1,out,1,1,1,lin,original,\nreel,in,2,w,1,1,1,lin,original,\nreel,w,1,out,1,1,1,lin,original,\n";
        let mut f = build(&WOB, &[], routes);
        assert_eq!(f.chain.route_delays().iter().filter(|d| **d > 0).count(), 1);
        let x = impulse(512, 0.5);
        let (l, _) = run_t(&mut f, 0.5, &x, &x);
        let lat = f.chain.latency() as usize;
        assert_eq!(argmax(&l), lat);
        assert!(
            (l[lat] - 1.0).abs() < 1e-5,
            "both branches coincide: {}",
            l[lat]
        );
        let rest: f32 = l
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != lat)
            .map(|(_, v)| v.abs())
            .sum();
        assert!(rest < 1e-5, "no comb: {rest}");
    }

    #[test]
    fn a_sat_stage_has_latency_29_at_every_rate_and_the_chain_pads_to_the_class() {
        for sr in [44_100.0f32, 48_000.0, 96_000.0] {
            let routes = "reel,in,1,s,1,1,1,lin,original,\nreel,s,1,out,1,1,1,lin,original,\n";
            let mut f = try_build(&SAT, &[], routes, sr).unwrap();
            let x = impulse(256, 0.1);
            let (l, _) = run_t(&mut f, 0.5, &x, &x);
            // The stage itself delays by 29; the chain pads up to the short class latency.
            assert_eq!(f.chain.stage_latency(0), 29, "sr {sr}");
            assert_eq!(argmax(&l), short_latency(sr) as usize, "sr {sr}");
            assert_eq!(f.chain.latency(), short_latency(sr), "sr {sr}");
        }
    }

    // the gain of the w -> out send falls from 1 (Transform 0) to 0 (Transform 1)
    const LAZY_ROUTES: &str = "reel,in,1,out,1,1,1,lin,original,\nreel,in,2,w,1,1,1,lin,original,\nreel,w,1,out,1,0.5,0,lin,original,\n";

    #[test]
    fn unrouted_stages_are_not_evaluated_and_keep_their_state() {
        let mut f = build(&WOB, &[], LAZY_ROUTES);
        let noise_in = noise(SUB * 6, 11);
        let z = vec![0.0f32; SUB];
        run_t(&mut f, 0.0, &noise_in, &noise_in);
        let active_evals = f.chain.eval_count(0);
        assert!(active_evals >= 6);
        // Transform 1 closes the send; once its smoother has settled the stage stands still.
        let long = noise(SUB * 100, 12);
        run_t(&mut f, 1.0, &long, &long);
        let settled = f.chain.eval_count(0);
        assert!(
            settled < active_evals + 100,
            "it stopped running: {settled}"
        );
        run_t(&mut f, 1.0, &z, &z);
        run_t(&mut f, 1.0, &z, &z);
        assert_eq!(
            f.chain.eval_count(0),
            settled,
            "not evaluated while unrouted"
        );
        // Switching it on again: the stage still holds the last samples it saw and plays them
        // out although its input is silent now (a running stage would have flushed them).
        let mut heard = 0.0f32;
        for _ in 0..4 {
            let (l, _) = run_t(&mut f, 0.0, &z, &z);
            heard = heard.max(l.iter().fold(0.0, |m, v| m.max(v.abs())));
        }
        assert!(heard > 1e-4, "state kept: {heard}");
    }

    #[test]
    fn a_stage_without_any_route_is_inert() {
        let routes = "reel,in,1,a,1,1,1,lin,original,\nreel,a,1,out,1,1,1,lin,original,\n";
        let mut f = build(&UNUSED, &[], routes);
        let x = noise(512, 3);
        let _ = run_t(&mut f, 0.5, &x, &x);
        assert!(f.chain.eval_count(0) > 0);
        assert_eq!(f.chain.eval_count(1), 0);
    }

    // send 1 is the low band, send 2 the high band of a split stage
    const SPLIT_ROUTES: &str = "reel,in,1,x,1,1,1,lin,original,\nreel,x,1,out,1,1,1,lin,original,\nreel,x,2,out,1,1,1,lin,original,\n";

    #[test]
    fn a_split_stage_sends_its_bands_separately_and_they_sum_flat() {
        let mut f = build(&SPLIT, &[("x", "freq_hz", [1000.0; 3])], SPLIT_ROUTES);
        let x = noise(48_000, 5);
        let (l, _) = run_t(&mut f, 0.5, &x, &x);
        let lat = f.chain.latency() as usize;
        let gi = rms(&x[..x.len() - lat]);
        let go = rms(&l[lat + 4096..]);
        let gi2 = rms(&x[4096..x.len() - lat]);
        assert!(
            (20.0 * (go / gi2).log10()).abs() < 0.2,
            "flat sum {go} {gi} {gi2}"
        );
        // Only the high band: a 100 Hz sine is strongly attenuated.
        let high_only = "reel,in,1,x,1,1,1,lin,original,\nreel,x,2,out,1,1,1,lin,original,\n";
        let mut f = build(&SPLIT, &[("x", "freq_hz", [1000.0; 3])], high_only);
        let s: Vec<f32> = (0..9600)
            .map(|n| 0.5 * (std::f32::consts::TAU * 100.0 * n as f32 / 48_000.0).sin())
            .collect();
        let (l, _) = run_t(&mut f, 0.5, &s, &s);
        assert!(rms(&l[4800..]) < 0.5 * 0.707 * 0.02, "{}", rms(&l[4800..]));
    }

    #[test]
    fn a_generator_stage_is_silent_without_presence_and_follows_the_gain_flag() {
        let routes = "reel,h,1,out,1,1,1,lin,original,\n";
        let mut f = build(&NOISE, &[("h", "level_db", [-20.0; 3])], routes);
        let x = vec![0.0f32; 4800];
        let presence = [0.0f32; SUB];
        let gain = [1.0f32; SUB];
        let ctx = Ctx {
            presence: &presence,
            gain: &gain,
        };
        let m = Morph::default();
        let mut peak = 0.0f32;
        for _ in 0..x.len() / SUB {
            let mut l = [0.0; SUB];
            let mut r = [0.0; SUB];
            f.chain.process(&mut l, &mut r, &m, &ctx);
            peak = l.iter().chain(r.iter()).fold(peak, |p, v| p.max(v.abs()));
        }
        assert!(peak.to_bits() == 0f32.to_bits(), "silent when idle: {peak}");
        // With the gate open the level is the table level.
        let (l, r) = run_t(&mut f, 0.5, &x, &x);
        let level = 10f32.powf(-20.0 / 20.0);
        assert!((rms(&l) / level - 1.0).abs() < 0.25, "{}", rms(&l));
        assert!(l != r, "channels are independent");
    }

    #[test]
    fn noise_is_deterministic_by_seed() {
        let routes = "reel,h,1,out,1,1,1,lin,original,\n";
        let run = |seed: u64| {
            let mut f = build(&NOISE, &[], routes);
            f.chain.prepare(48_000.0, seed, &f.pv.clone());
            run_t(&mut f, 0.5, &vec![0.0; 1024], &vec![0.0; 1024]).0
        };
        assert_eq!(run(3), run(3));
        assert_ne!(run(3), run(4));
    }

    #[test]
    fn reset_restores_the_start_state() {
        let mut f = build(&WOB, &[], LAZY_ROUTES);
        let x = noise(2048, 9);
        let a = run_t(&mut f, 0.3, &x, &x);
        f.chain.reset();
        let b = run_t(&mut f, 0.3, &x, &x);
        assert_eq!(a, b);
    }

    #[test]
    fn output_is_finite_for_hostile_input_and_any_transform() {
        let mut f = build(&WOB, &[], LAZY_ROUTES);
        let mut x = noise(4096, 2);
        x[10] = 1.0e6;
        x[11] = -1.0e6;
        for t in [0.0, 0.3, 0.7, 1.0] {
            let (l, r) = run_t(&mut f, t, &x, &x);
            assert!(l.iter().chain(r.iter()).all(|v| v.is_finite()));
        }
    }

    #[test]
    fn activity_follows_the_wet_level_and_falls() {
        let routes = "reel,in,1,pk,1,1,1,lin,original,\nreel,pk,1,out,1,1,1,lin,original,\n";
        let mut f = build(&LINEAR, &[], routes);
        assert!(f.chain.activity() < 1e-6);
        let x = noise(4800, 1);
        run_t(&mut f, 0.5, &x, &x);
        let on = f.chain.activity();
        assert!(on > 0.05 && on <= 1.5, "{on}");
        run_t(&mut f, 0.5, &vec![0.0; 48_000], &vec![0.0; 48_000]);
        assert!(f.chain.activity() < on * 0.01);
    }

    // `pk -> out` is open at Transform 0 and closed at Transform 1  (answers the mutants
    // that cut a closing send at once or snapped it shut at a high level).
    const CLOSING_ROUTES: &str =
        "reel,in,1,pk,1,1,1,lin,original,\nreel,pk,1,out,1,0.5,0,lin,original,\n";

    #[test]
    fn a_send_closed_by_the_morph_fades_out_through_its_smoother_and_then_stops() {
        let mut f = build(&LINEAR, &[], CLOSING_ROUTES);
        let dc = vec![0.5f32; SUB * 300];
        let (open, _) = run_t(&mut f, 0.0, &dc, &dc);
        let settled = open[open.len() - 1];
        assert!((settled - 0.5).abs() < 1e-3, "{settled}");
        let (fade, _) = run_t(&mut f, 1.0, &dc, &dc);
        let (mut worst, mut prev) = (0.0f32, settled);
        for v in &fade {
            worst = worst.max((v - prev).abs());
            prev = *v;
        }
        assert!(worst < 0.01, "the gain ramps, no step: {worst}");
        assert!(
            fade.windows(2).all(|w| w[1] <= w[0] + 1e-6),
            "monotone fall"
        );
        assert_eq!(fade[fade.len() - 1], 0.0, "exactly closed at the end");
        // Opened again it comes back smoothly too.
        let (back, _) = run_t(&mut f, 0.0, &dc, &dc);
        assert!(back[0].abs() < 0.01, "starts from silence: {}", back[0]);
        assert!((back[back.len() - 1] - 0.5).abs() < 1e-3);
    }

    #[test]
    fn a_send_gain_may_reach_twice_unity() {
        for (gain, want) in [(1.5f32, 0.75f32), (2.0, 1.0)] {
            let routes = format!(
                "reel,in,1,pk,1,1,1,lin,original,\nreel,pk,1,out,{gain},{gain},{gain},lin,original,\n"
            );
            let mut f = build(&LINEAR, &[], &routes);
            let dc = vec![0.5f32; SUB * 300];
            let (l, _) = run_t(&mut f, 0.5, &dc, &dc);
            assert!(
                (l[l.len() - 1] - want).abs() < 2e-3,
                "gain {gain}: {}",
                l[l.len() - 1]
            );
        }
    }
}
