//! The one matrix product the network needs, written so that the result never depends on the
//! machine's thread count or on the compiler's choices: every sum is a fixed tree of eight lane
//! accumulators, then a fixed horizontal add, then the scalar tail.

/// Frames computed together against one weight row.
const FRAMES: usize = 4;
/// Weight rows kept together so a block of them stays in cache while every frame passes.
const ROWS: usize = 32;

/// The dot products of up to four rows of `x` with one weight row, in a fixed order.
#[inline]
fn dots<const N: usize>(x: [&[f32]; N], w: &[f32]) -> [f32; N] {
    let main = w.len() / 8 * 8;
    let mut acc = [[0f32; 8]; N];
    for (c, wc) in w[..main].as_chunks::<8>().0.iter().enumerate() {
        for j in 0..N {
            let xc = &x[j][c * 8..c * 8 + 8];
            for l in 0..8 {
                acc[j][l] += xc[l] * wc[l];
            }
        }
    }
    let mut out = [0f32; N];
    for j in 0..N {
        let a = &acc[j];
        let mut s = ((a[0] + a[4]) + (a[1] + a[5])) + ((a[2] + a[6]) + (a[3] + a[7]));
        for i in main..w.len() {
            s += x[j][i] * w[i];
        }
        out[j] = s;
    }
    out
}

/// `out[t][n] = bias[n] + sum_k x[t][k] * w[n][k]` for `t` frames of `k` values and `n` weight
/// rows. `x` is `t * k`, `w` is `n * k`, `out` is `t * n`.
pub(crate) fn matmul_bias(
    x: &[f32],
    t: usize,
    k: usize,
    w: &[f32],
    n: usize,
    bias: &[f32],
    out: &mut [f32],
) {
    debug_assert_eq!(x.len(), t * k);
    debug_assert_eq!(w.len(), n * k);
    debug_assert_eq!(bias.len(), n);
    debug_assert_eq!(out.len(), t * n);
    let mut row0 = 0;
    while row0 < n {
        let rows = ROWS.min(n - row0);
        let mut t0 = 0;
        while t0 + FRAMES <= t {
            let xs = [
                &x[t0 * k..(t0 + 1) * k],
                &x[(t0 + 1) * k..(t0 + 2) * k],
                &x[(t0 + 2) * k..(t0 + 3) * k],
                &x[(t0 + 3) * k..(t0 + 4) * k],
            ];
            for r in row0..row0 + rows {
                let d = dots(xs, &w[r * k..(r + 1) * k]);
                for (j, v) in d.iter().enumerate() {
                    out[(t0 + j) * n + r] = bias[r] + v;
                }
            }
            t0 += FRAMES;
        }
        while t0 < t {
            let xs = [&x[t0 * k..(t0 + 1) * k]];
            for r in row0..row0 + rows {
                let d = dots(xs, &w[r * k..(r + 1) * k]);
                out[t0 * n + r] = bias[r] + d[0];
            }
            t0 += 1;
        }
        row0 += rows;
    }
}

/// `out[n] = sum_k x[k] * w[n][k]`: one frame against a weight matrix, in the same fixed order
/// as [`matmul_bias`] (the recurrent step of the network).
pub(crate) fn matvec_add(x: &[f32], w: &[f32], n: usize, out: &mut [f32]) {
    let k = x.len();
    for r in 0..n {
        let d = dots([x], &w[r * k..(r + 1) * k]);
        out[r] += d[0];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn naive(x: &[f32], t: usize, k: usize, w: &[f32], n: usize, bias: &[f32]) -> Vec<f64> {
        let mut out = vec![0f64; t * n];
        for i in 0..t {
            for r in 0..n {
                let mut s = f64::from(bias[r]);
                for c in 0..k {
                    s += f64::from(x[i * k + c]) * f64::from(w[r * k + c]);
                }
                out[i * n + r] = s;
            }
        }
        out
    }

    fn fill(len: usize, seed: u32) -> Vec<f32> {
        let mut s = seed;
        (0..len)
            .map(|_| {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (s >> 8) as f32 / (1u32 << 24) as f32 - 0.5
            })
            .collect()
    }

    #[test]
    fn the_product_matches_a_naive_sum_for_awkward_sizes() {
        for &(t, k, n) in &[(1, 1, 1), (5, 13, 7), (9, 8, 33), (4, 21, 65), (3, 70, 2)] {
            let x = fill(t * k, 1);
            let w = fill(n * k, 2);
            let b = fill(n, 3);
            let mut out = vec![0f32; t * n];
            matmul_bias(&x, t, k, &w, n, &b, &mut out);
            let want = naive(&x, t, k, &w, n, &b);
            for (a, b) in out.iter().zip(&want) {
                assert!(
                    (f64::from(*a) - b).abs() < 1e-4,
                    "{a} vs {b} at {t}x{k}x{n}"
                );
            }
        }
    }

    #[test]
    fn a_frame_gives_the_same_bits_alone_or_in_a_group() {
        let (t, k, n) = (8, 37, 11);
        let x = fill(t * k, 4);
        let w = fill(n * k, 5);
        let b = fill(n, 6);
        let mut all = vec![0f32; t * n];
        matmul_bias(&x, t, k, &w, n, &b, &mut all);
        for i in 0..t {
            let mut one = vec![0f32; n];
            matmul_bias(&x[i * k..(i + 1) * k], 1, k, &w, n, &b, &mut one);
            assert_eq!(&all[i * n..(i + 1) * n], &one[..]);
        }
    }
}
