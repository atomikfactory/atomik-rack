//! The Open-Unmix network: one per target (vocals, drums, bass, other). It reads the magnitude
//! spectrogram of the mix and returns the magnitude estimate of its target.
//!
//! Per frame: a bandwidth-limited input (normalised) goes through a dense layer, batch
//! normalisation and tanh; three bidirectional LSTM layers run over time; the input and the LSTM
//! output are joined and go through two more dense layers (batch normalisation, ReLU between);
//! the output is scaled, rectified and multiplies the mix magnitude. Inference only: the batch
//! normalisations are folded into the dense layers when the weights are loaded.
//!
//! The shapes come from the weight file, so a small network (used by the tests) loads the same
//! way as the published one.

use crate::gemm::{matmul_bias, matvec_add};
use crate::torch::State;
use crate::{Control, StemError};

/// The batch-norm epsilon the network was trained with.
const BN_EPS: f64 = 1e-5;

/// A dense layer with its bias, row major `n * k`.
#[derive(Debug, Clone)]
struct Dense {
    w: Vec<f32>,
    bias: Vec<f32>,
    n: usize,
    k: usize,
}

/// One direction of one LSTM layer.
#[derive(Debug, Clone)]
struct Lstm {
    w_ih: Vec<f32>,
    w_hh: Vec<f32>,
    bias: Vec<f32>,
}

/// One bidirectional layer.
#[derive(Debug, Clone)]
struct Layer {
    fwd: Lstm,
    bwd: Lstm,
}

/// The network of one target.
#[derive(Debug, Clone)]
pub struct Net {
    /// The FFT size.
    pub(crate) n_fft: usize,
    /// The sample rate the network expects.
    pub(crate) rate: u32,
    /// The window it was trained with.
    pub(crate) window: Vec<f32>,
    max_bin: usize,
    hidden: usize,
    gate: usize,
    in_mean: Vec<f32>,
    in_scale: Vec<f32>,
    fc1: Dense,
    layers: Vec<Layer>,
    fc2: Dense,
    fc3: Dense,
}

fn get<'a>(state: &'a State, name: &str, shape: &[usize]) -> Result<&'a [f32], StemError> {
    let t = state
        .get(name)
        .ok_or_else(|| StemError::Weights(format!("{name} is missing")))?;
    if t.shape != shape {
        return Err(StemError::Weights(format!(
            "{name} has the shape {:?}, expected {shape:?}",
            t.shape
        )));
    }
    Ok(&t.data)
}

fn shape_of(state: &State, name: &str, rank: usize) -> Result<Vec<usize>, StemError> {
    let t = state
        .get(name)
        .ok_or_else(|| StemError::Weights(format!("{name} is missing")))?;
    if t.shape.len() != rank {
        return Err(StemError::Weights(format!("{name} has the wrong rank")));
    }
    Ok(t.shape.clone())
}

/// The scale and shift of a batch normalisation in evaluation mode.
fn batch_norm(state: &State, name: &str, n: usize) -> Result<(Vec<f64>, Vec<f64>), StemError> {
    let w = get(state, &format!("{name}.weight"), &[n])?;
    let b = get(state, &format!("{name}.bias"), &[n])?;
    let m = get(state, &format!("{name}.running_mean"), &[n])?;
    let v = get(state, &format!("{name}.running_var"), &[n])?;
    let mut scale = Vec::with_capacity(n);
    let mut shift = Vec::with_capacity(n);
    for i in 0..n {
        let s = f64::from(w[i]) / (f64::from(v[i]) + BN_EPS).sqrt();
        scale.push(s);
        shift.push(f64::from(b[i]) - f64::from(m[i]) * s);
    }
    Ok((scale, shift))
}

/// A dense layer without bias followed by a batch norm, folded into one layer with a bias.
/// `after` is an optional per-output affine (scale, offset) applied after the norm.
fn folded(
    state: &State,
    fc: &str,
    bn: &str,
    n: usize,
    k: usize,
    after: Option<(&[f32], &[f32], usize)>,
) -> Result<Dense, StemError> {
    let w = get(state, &format!("{fc}.weight"), &[n, k])?;
    let (mut scale, mut shift) = batch_norm(state, bn, n)?;
    if let Some((s, o, bins)) = after {
        for i in 0..n {
            let b = i % bins;
            scale[i] *= f64::from(s[b]);
            shift[i] = shift[i] * f64::from(s[b]) + f64::from(o[b]);
        }
    }
    let mut out = Vec::with_capacity(n * k);
    for r in 0..n {
        out.extend(
            w[r * k..(r + 1) * k]
                .iter()
                .map(|&v| (f64::from(v) * scale[r]) as f32),
        );
    }
    Ok(Dense {
        w: out,
        bias: shift.iter().map(|&v| v as f32).collect(),
        n,
        k,
    })
}

fn lstm(
    state: &State,
    suffix: &str,
    layer: usize,
    input: usize,
    gate: usize,
) -> Result<Lstm, StemError> {
    let w_ih = get(
        state,
        &format!("lstm.weight_ih_l{layer}{suffix}"),
        &[4 * gate, input],
    )?;
    let w_hh = get(
        state,
        &format!("lstm.weight_hh_l{layer}{suffix}"),
        &[4 * gate, gate],
    )?;
    let b_ih = get(
        state,
        &format!("lstm.bias_ih_l{layer}{suffix}"),
        &[4 * gate],
    )?;
    let b_hh = get(
        state,
        &format!("lstm.bias_hh_l{layer}{suffix}"),
        &[4 * gate],
    )?;
    Ok(Lstm {
        w_ih: w_ih.to_vec(),
        w_hh: w_hh.to_vec(),
        bias: b_ih.iter().zip(b_hh).map(|(a, b)| a + b).collect(),
    })
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

impl Net {
    /// Builds the network from a checkpoint's state dictionary.
    pub fn from_state(state: &State) -> Result<Self, StemError> {
        let window = state
            .get("stft.window")
            .ok_or_else(|| StemError::Weights("stft.window is missing".into()))?;
        let n_fft = window.data.len();
        if !n_fft.is_power_of_two() || !(8..=1 << 16).contains(&n_fft) {
            return Err(StemError::Weights("the FFT size is not usable".into()));
        }
        let bins = n_fft / 2 + 1;
        let rate = get(state, "sample_rate", &[])?[0];
        if !(1.0..=384_000.0).contains(&rate) {
            return Err(StemError::Weights("the sample rate is not usable".into()));
        }
        let max_bin = shape_of(state, "input_mean", 1)?[0];
        if max_bin == 0 || max_bin > bins {
            return Err(StemError::Weights("the bandwidth is not usable".into()));
        }
        let hidden = shape_of(state, "fc1.weight", 2)?[0];
        let gate = hidden / 2;
        if hidden == 0 || hidden != 2 * gate || hidden > 4096 {
            return Err(StemError::Weights("the hidden size is not usable".into()));
        }
        let in_mean = get(state, "input_mean", &[max_bin])?.to_vec();
        let in_scale = get(state, "input_scale", &[max_bin])?.to_vec();
        let out_scale = get(state, "output_scale", &[bins])?;
        let out_mean = get(state, "output_mean", &[bins])?;
        let fc1 = folded(state, "fc1", "bn1", hidden, 2 * max_bin, None)?;
        let mut layers = Vec::new();
        for l in 0.. {
            if !state.contains_key(&format!("lstm.weight_ih_l{l}")) {
                break;
            }
            if l >= 8 {
                return Err(StemError::Weights("too many LSTM layers".into()));
            }
            layers.push(Layer {
                fwd: lstm(state, "", l, hidden, gate)?,
                bwd: lstm(state, "_reverse", l, hidden, gate)?,
            });
        }
        if layers.is_empty() {
            return Err(StemError::Weights("the LSTM layers are missing".into()));
        }
        let fc2 = folded(state, "fc2", "bn2", hidden, 2 * hidden, None)?;
        let fc3 = folded(
            state,
            "fc3",
            "bn3",
            2 * bins,
            hidden,
            Some((out_scale, out_mean, bins)),
        )?;
        Ok(Self {
            n_fft,
            rate: rate as u32,
            window: window.data.clone(),
            max_bin,
            hidden,
            gate,
            in_mean,
            in_scale,
            fc1,
            layers,
            fc2,
            fc3,
        })
    }

    /// The bins of a frame.
    pub fn bins(&self) -> usize {
        self.n_fft / 2 + 1
    }

    /// The magnitude estimate of this target. `mag` is `frames * 2 * bins` (the left bins then
    /// the right bins of each frame); the result has the same layout.
    pub fn estimate(
        &self,
        mag: &[f32],
        frames: usize,
        control: &Control,
    ) -> Result<Vec<f32>, StemError> {
        let bins = self.bins();
        let h = self.hidden;
        // The bandwidth-limited, normalised input.
        let mut x = vec![0f32; frames * 2 * self.max_bin];
        for f in 0..frames {
            for c in 0..2 {
                let src = &mag[f * 2 * bins + c * bins..f * 2 * bins + c * bins + self.max_bin];
                let dst = &mut x[(f * 2 + c) * self.max_bin..(f * 2 + c + 1) * self.max_bin];
                for (i, v) in src.iter().enumerate() {
                    dst[i] = (v + self.in_mean[i]) * self.in_scale[i];
                }
            }
        }
        control.check()?;
        let mut h1 = vec![0f32; frames * h];
        matmul_bias(
            &x,
            frames,
            self.fc1.k,
            &self.fc1.w,
            self.fc1.n,
            &self.fc1.bias,
            &mut h1,
        );
        drop(x);
        for v in &mut h1 {
            *v = v.tanh();
        }
        control.check()?;
        // The LSTM layers.
        let mut seq = h1.clone();
        for layer in &self.layers {
            seq = self.run_layer(layer, &seq, frames, control)?;
        }
        // Join the input of the recurrent layers with their output.
        let mut joined = vec![0f32; frames * 2 * h];
        for f in 0..frames {
            joined[f * 2 * h..f * 2 * h + h].copy_from_slice(&h1[f * h..(f + 1) * h]);
            joined[f * 2 * h + h..(f + 1) * 2 * h].copy_from_slice(&seq[f * h..(f + 1) * h]);
        }
        drop((h1, seq));
        let mut h2 = vec![0f32; frames * h];
        matmul_bias(
            &joined,
            frames,
            self.fc2.k,
            &self.fc2.w,
            self.fc2.n,
            &self.fc2.bias,
            &mut h2,
        );
        drop(joined);
        for v in &mut h2 {
            *v = v.max(0.0);
        }
        control.check()?;
        let mut out = vec![0f32; frames * 2 * bins];
        matmul_bias(
            &h2,
            frames,
            self.fc3.k,
            &self.fc3.w,
            self.fc3.n,
            &self.fc3.bias,
            &mut out,
        );
        for (o, m) in out.iter_mut().zip(mag) {
            *o = o.max(0.0) * m;
        }
        Ok(out)
    }

    /// One bidirectional layer over `frames` steps; the two directions run on two threads.
    fn run_layer(
        &self,
        layer: &Layer,
        input: &[f32],
        frames: usize,
        control: &Control,
    ) -> Result<Vec<f32>, StemError> {
        let g = self.gate;
        let (fwd, bwd) = std::thread::scope(|s| {
            let a = s.spawn(|| self.run_direction(&layer.fwd, input, frames, false, control));
            let b = self.run_direction(&layer.bwd, input, frames, true, control);
            (a.join().unwrap_or(Err(StemError::Cancelled)), b)
        });
        let (fwd, bwd) = (fwd?, bwd?);
        let mut out = vec![0f32; frames * 2 * g];
        for f in 0..frames {
            out[f * 2 * g..f * 2 * g + g].copy_from_slice(&fwd[f * g..(f + 1) * g]);
            out[f * 2 * g + g..(f + 1) * 2 * g].copy_from_slice(&bwd[f * g..(f + 1) * g]);
        }
        Ok(out)
    }

    fn run_direction(
        &self,
        l: &Lstm,
        input: &[f32],
        frames: usize,
        reverse: bool,
        control: &Control,
    ) -> Result<Vec<f32>, StemError> {
        let g = self.gate;
        // The input projections of every frame at once.
        let mut gates = vec![0f32; frames * 4 * g];
        matmul_bias(
            input,
            frames,
            self.hidden,
            &l.w_ih,
            4 * g,
            &l.bias,
            &mut gates,
        );
        let mut out = vec![0f32; frames * g];
        let mut h = vec![0f32; g];
        let mut c = vec![0f32; g];
        for step in 0..frames {
            if step % 64 == 0 {
                control.check()?;
            }
            let t = if reverse { frames - 1 - step } else { step };
            let row = &mut gates[t * 4 * g..(t + 1) * 4 * g];
            matvec_add(&h, &l.w_hh, 4 * g, row);
            for j in 0..g {
                let i = sigmoid(row[j]);
                let f = sigmoid(row[g + j]);
                let gg = row[2 * g + j].tanh();
                let o = sigmoid(row[3 * g + j]);
                c[j] = f * c[j] + i * gg;
                h[j] = o * c[j].tanh();
            }
            out[t * g..(t + 1) * g].copy_from_slice(&h);
        }
        Ok(out)
    }
}
