//! FLAC output: 16-bit and 24-bit PCM, any number of channels from one to eight.
//!
//! A small encoder of our own rather than a dependency: the format is simple at the level a
//! mastering export needs (lossless, fixed block size, no seek table), the decoder side already
//! lives in the symphonia reader the project uses, and an encoder crate would add a licence and
//! an advisory surface for a few hundred lines of code. The cost is compression a little behind
//! the reference encoder's best settings; every file is still plain FLAC that any decoder reads.
//!
//! * **Samples.** The audio goes through the quantiser of the WAV writer
//!   ([`atomik_dsp::Dither`]), so a FLAC file holds the same codes as the WAV file of the same
//!   audio, dither and seed, and a 16-bit file is quantised exactly once. 32-bit float has no
//!   FLAC form and is refused.
//! * **Blocks.** Fixed blocks of [`BLOCK`] frames (the last one shorter). Per block and channel
//!   the encoder takes the smallest of a constant subframe (silence costs a few bytes), a
//!   verbatim one and the fixed polynomial predictors of order 0 to 4 with Rice coding of the
//!   residual, each over the best partition order up to [`MAX_PARTITION_ORDER`]. A stereo block
//!   is stored as left/right, left/side, side/right or mid/side, whichever is smallest.
//! * **Deterministic.** Every choice is a pure function of the codes, with ties going to the
//!   first candidate, so the same audio gives the same bytes on every machine.
//! * **Stream info.** The header carries the length, the frame sizes and the MD5 of the
//!   unencoded samples, so a decoder or `flac -t` can verify the file end to end.
//!
//! The frames are encoded in memory and written after the header (the stream info needs their
//! sizes); a file is never larger than the WAV of the same audio plus a few kilobytes.

use crate::wav::{SampleFormat, WavError};
use atomik_dsp::{Dither, DitherMode};
use std::io::Write;
use std::path::Path;

/// Frames per block (the last block of a stream holds what is left).
pub const BLOCK: usize = 4096;

/// The deepest Rice partitioning tried: 2^8 partitions of a block.
pub const MAX_PARTITION_ORDER: u32 = 8;

/// The most channels a FLAC stream holds.
pub const MAX_CHANNELS: usize = 8;

/// The highest fixed predictor order.
const MAX_FIXED_ORDER: usize = 4;

/// The largest Rice parameter of the wide (5-bit) residual coding: 31 is its escape.
const MAX_RICE: u32 = 30;

/// The largest Rice parameter of the narrow (4-bit) coding: 15 is its escape.
const MAX_RICE_NARROW: u32 = 14;

/// The CRC-8 (polynomial 0x07) and CRC-16 (polynomial 0x8005) of the frame header and frame.
const CRC8: [u8; 256] = crc8_table();
const CRC16: [u16; 256] = crc16_table();

const fn crc8_table() -> [u8; 256] {
    let mut table = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u8;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 0x80 != 0 {
                (crc << 1) ^ 0x07
            } else {
                crc << 1
            };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

const fn crc16_table() -> [u16; 256] {
    let mut table = [0u16; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = (i as u16) << 8;
        let mut bit = 0;
        while bit < 8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x8005
            } else {
                crc << 1
            };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

fn crc8(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |crc, &b| CRC8[usize::from(crc ^ b)])
}

fn crc16(bytes: &[u8]) -> u16 {
    bytes.iter().fold(0u16, |crc, &b| {
        (crc << 8) ^ CRC16[usize::from((crc >> 8) as u8 ^ b)]
    })
}

/// A bit sink, most significant bit first.
#[derive(Default)]
struct Bits {
    out: Vec<u8>,
    acc: u64,
    pending: u32,
}

impl Bits {
    /// Appends the low `bits` (at most 56) of `value`.
    fn put(&mut self, value: u64, bits: u32) {
        debug_assert!(bits <= 56);
        if bits == 0 {
            return;
        }
        self.acc = (self.acc << bits) | (value & ((1u64 << bits) - 1));
        self.pending += bits;
        while self.pending >= 8 {
            self.pending -= 8;
            self.out.push((self.acc >> self.pending) as u8);
        }
        self.acc &= (1u64 << self.pending) - 1;
    }

    /// Appends `value` as a two's-complement number of `bits` bits.
    fn put_signed(&mut self, value: i32, bits: u32) {
        self.put(value as u32 as u64, bits.min(32));
    }

    /// Appends `zeros` zero bits and a one.
    fn put_unary(&mut self, mut zeros: u64) {
        while zeros >= 32 {
            self.put(0, 32);
            zeros -= 32;
        }
        self.put(1, zeros as u32 + 1);
    }

    /// Pads to a byte boundary with zeros.
    fn align(&mut self) {
        if self.pending > 0 {
            self.put(0, 8 - self.pending);
        }
    }
}

/// How the samples of one subframe are stored.
enum Coding {
    /// Every sample has this value.
    Constant(i32),
    /// The samples as they are.
    Verbatim,
    /// A fixed polynomial predictor with Rice-coded residual.
    Fixed {
        order: usize,
        part_order: u32,
        params: Vec<u32>,
        wide: bool,
        residual: Vec<i32>,
    },
}

/// One channel of one block, coded.
struct Sub {
    coding: Coding,
    /// The exact size in bits, subframe header included.
    bits: u64,
}

/// The residual of the fixed predictor of `order` over `x` (`x.len() - order` values).
fn fixed_residual(x: &[i32], order: usize) -> Vec<i32> {
    let at = |i: usize| i64::from(x[i]);
    (order..x.len())
        .map(|i| {
            let r = match order {
                0 => at(i),
                1 => at(i) - at(i - 1),
                2 => at(i) - 2 * at(i - 1) + at(i - 2),
                3 => at(i) - 3 * at(i - 1) + 3 * at(i - 2) - at(i - 3),
                _ => at(i) - 4 * at(i - 1) + 6 * at(i - 2) - 4 * at(i - 3) + at(i - 4),
            };
            r as i32
        })
        .collect()
}

/// The residual folded to an unsigned number: 0, -1, 1, -2, 2 become 0, 1, 2, 3, 4.
fn fold(r: i32) -> u64 {
    u64::from(((r << 1) ^ (r >> 31)) as u32)
}

/// The cheapest Rice parameter of a partition with these folded sums, and its estimated bits.
fn best_param(sum: u64, count: u64) -> (u32, u64) {
    (0..=MAX_RICE)
        .map(|k| (k, count * u64::from(k + 1) + (sum >> k)))
        .min_by_key(|&(_, bits)| bits)
        .unwrap_or((0, 0))
}

/// The partition order and the Rice parameters that code `residual` (the residual of a block of
/// `n` samples at predictor `order`) in the fewest estimated bits, and that estimate.
fn plan_partitions(residual: &[i32], n: usize, order: usize) -> (u32, Vec<u32>, u64) {
    let max_order = (0..=MAX_PARTITION_ORDER)
        .rev()
        .find(|&p| n.is_multiple_of(1usize << p) && (n >> p) > order)
        .unwrap_or(0);
    let size = n >> max_order;
    // The folded sums at the finest partitioning; the first partition holds `order` samples
    // fewer (they are the warm-up).
    let mut sums: Vec<(u64, u64)> = (0..1usize << max_order)
        .map(|j| {
            let from = (j * size).saturating_sub(order);
            let to = (j + 1) * size - order;
            let part = &residual[from..to];
            (part.iter().map(|&r| fold(r)).sum(), part.len() as u64)
        })
        .collect();
    let mut best: Option<(u32, Vec<u32>, u64)> = None;
    for p in (0..=max_order).rev() {
        let costs: Vec<(u32, u64)> = sums.iter().map(|&(s, c)| best_param(s, c)).collect();
        let bits = costs.iter().map(|&(_, b)| b + 5).sum::<u64>();
        if best.as_ref().is_none_or(|b| bits < b.2) {
            best = Some((p, costs.iter().map(|&(k, _)| k).collect(), bits));
        }
        if p > 0 {
            sums = sums
                .chunks(2)
                .map(|pair| (pair[0].0 + pair[1].0, pair[0].1 + pair[1].1))
                .collect();
        }
    }
    best.unwrap_or((0, vec![0], 0))
}

/// The exact bits of the Rice-coded residual (the method, partition order and parameters
/// included) for the chosen parameters.
fn rice_bits(
    residual: &[i32],
    n: usize,
    order: usize,
    part_order: u32,
    params: &[u32],
) -> (u64, bool) {
    let wide = params.iter().any(|&k| k > MAX_RICE_NARROW);
    let size = n >> part_order;
    let mut bits = 2 + 4;
    for (j, &k) in params.iter().enumerate() {
        let from = (j * size).saturating_sub(order);
        let to = (j + 1) * size - order;
        bits += if wide { 5 } else { 4 };
        for &r in &residual[from..to] {
            bits += (fold(r) >> k) + 1 + u64::from(k);
        }
    }
    (bits, wide)
}

/// Codes one channel of a block in the fewest bits of the codings above. `bps` is the sample
/// size of this channel.
fn analyse(x: &[i32], bps: u32) -> Sub {
    let n = x.len();
    let mut best = Sub {
        coding: Coding::Verbatim,
        bits: 8 + n as u64 * u64::from(bps),
    };
    if x.iter().all(|&v| v == x[0]) {
        let constant = Sub {
            coding: Coding::Constant(x[0]),
            bits: 8 + u64::from(bps),
        };
        if constant.bits < best.bits {
            return constant;
        }
    }
    // The order is chosen on the estimate; only the winner is measured exactly.
    let mut chosen: Option<(u64, Coding)> = None;
    for order in 0..=MAX_FIXED_ORDER.min(n.saturating_sub(1)) {
        let residual = fixed_residual(x, order);
        let (part_order, params, estimate) = plan_partitions(&residual, n, order);
        let total = estimate + order as u64 * u64::from(bps);
        if chosen.as_ref().is_none_or(|c| total < c.0) {
            let coding = Coding::Fixed {
                order,
                part_order,
                params,
                wide: false,
                residual,
            };
            chosen = Some((total, coding));
        }
    }
    if let Some((
        _,
        Coding::Fixed {
            order,
            part_order,
            params,
            residual,
            ..
        },
    )) = chosen
    {
        let (rice, wide) = rice_bits(&residual, n, order, part_order, &params);
        let bits = 8 + order as u64 * u64::from(bps) + rice;
        if bits < best.bits {
            best = Sub {
                coding: Coding::Fixed {
                    order,
                    part_order,
                    params,
                    wide,
                    residual,
                },
                bits,
            };
        }
    }
    best
}

/// Writes the subframe of `x` coded as `sub`.
fn write_subframe(out: &mut Bits, x: &[i32], bps: u32, sub: &Sub) {
    match &sub.coding {
        Coding::Constant(v) => {
            out.put(0, 8);
            out.put_signed(*v, bps);
        }
        Coding::Verbatim => {
            out.put(0b0000_0010, 8);
            for &v in x {
                out.put_signed(v, bps);
            }
        }
        Coding::Fixed {
            order,
            part_order,
            params,
            wide,
            residual,
        } => {
            // 0, the type (001 and the order), no wasted bits.
            out.put(0, 1);
            out.put(0b001_000 | *order as u64, 6);
            out.put(0, 1);
            for &v in &x[..*order] {
                out.put_signed(v, bps);
            }
            out.put(u64::from(*wide), 2);
            out.put(u64::from(*part_order), 4);
            let size = x.len() >> part_order;
            for (j, &k) in params.iter().enumerate() {
                out.put(u64::from(k), if *wide { 5 } else { 4 });
                let from = (j * size).saturating_sub(*order);
                let to = (j + 1) * size - order;
                for &r in &residual[from..to] {
                    let u = fold(r);
                    out.put_unary(u >> k);
                    out.put(u & ((1u64 << k) - 1), k);
                }
            }
        }
    }
}

/// The frame number in the UTF-8-like form of the frame header.
fn put_frame_number(out: &mut Vec<u8>, n: u64) {
    if n < 0x80 {
        out.push(n as u8);
        return;
    }
    let len: u32 = match n {
        0..0x800 => 2,
        0x800..0x1_0000 => 3,
        0x1_0000..0x20_0000 => 4,
        0x20_0000..0x400_0000 => 5,
        0x400_0000..0x8000_0000 => 6,
        _ => 7,
    };
    let lead = (0xFF00u16 >> len) as u8;
    out.push(lead | (n >> (6 * (len - 1))) as u8);
    for i in (0..len - 1).rev() {
        out.push(0x80 | ((n >> (6 * i)) & 0x3F) as u8);
    }
}

/// How the channels of a block are stored.
#[derive(Clone, Copy)]
enum Assign {
    Independent,
    LeftSide,
    SideRight,
    MidSide,
}

/// Encodes one block (`channels` equal-length slices of codes) as a frame appended to `out`.
fn encode_frame(out: &mut Vec<u8>, number: u64, channels: &[&[i32]], bps: u32) {
    let n = channels[0].len();
    // The channels as stored, their sample sizes, their coding and the assignment code.
    let (assign, stored): (Assign, Vec<Vec<i32>>) = if channels.len() == 2 {
        let (l, r) = (channels[0], channels[1]);
        let side: Vec<i32> = l.iter().zip(r).map(|(a, b)| a - b).collect();
        let mid: Vec<i32> = l.iter().zip(r).map(|(a, b)| (a + b) >> 1).collect();
        let sub = |x: &[i32], extra: u32| analyse(x, bps + extra);
        let (cl, cr) = (sub(l, 0), sub(r, 0));
        let (cs, cm) = (sub(&side, 1), sub(&mid, 0));
        let options = [
            (Assign::Independent, cl.bits + cr.bits),
            (Assign::LeftSide, cl.bits + cs.bits),
            (Assign::SideRight, cs.bits + cr.bits),
            (Assign::MidSide, cm.bits + cs.bits),
        ];
        let mut pick = options[0];
        for o in &options[1..] {
            if o.1 < pick.1 {
                pick = *o;
            }
        }
        let list = match pick.0 {
            Assign::Independent => vec![l.to_vec(), r.to_vec()],
            Assign::LeftSide => vec![l.to_vec(), side],
            Assign::SideRight => vec![side, r.to_vec()],
            Assign::MidSide => vec![mid, side],
        };
        (pick.0, list)
    } else {
        (
            Assign::Independent,
            channels.iter().map(|c| c.to_vec()).collect(),
        )
    };
    let code = match assign {
        Assign::Independent => channels.len() as u8 - 1,
        Assign::LeftSide => 8,
        Assign::SideRight => 9,
        Assign::MidSide => 10,
    };
    let start = out.len();
    out.extend_from_slice(&[0xFF, 0xF8]);
    let size_code: u8 = if n == BLOCK { 12 } else { 7 };
    out.push(size_code << 4);
    let sample_size: u8 = if bps == 16 { 0b100 } else { 0b110 };
    out.push((code << 4) | (sample_size << 1));
    put_frame_number(out, number);
    if size_code == 7 {
        out.extend_from_slice(&((n - 1) as u16).to_be_bytes());
    }
    out.push(crc8(&out[start..]));
    let mut bits = Bits::default();
    for (i, x) in stored.iter().enumerate() {
        // The side channel needs one more bit.
        let wide = match assign {
            Assign::Independent => 0,
            Assign::LeftSide | Assign::MidSide => u32::from(i == 1),
            Assign::SideRight => u32::from(i == 0),
        };
        let sub = analyse(x, bps + wide);
        write_subframe(&mut bits, x, bps + wide, &sub);
    }
    bits.align();
    out.extend_from_slice(&bits.out);
    let crc = crc16(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

/// MD5, for the digest of the unencoded samples the stream info holds (a checksum for the
/// decoder to verify the file with, not a security measure).
struct Md5 {
    state: [u32; 4],
    block: [u8; 64],
    filled: usize,
    length: u64,
}

impl Md5 {
    /// floor(2^32 * abs(sin(i + 1))), the additive constants of the rounds.
    const K: [u32; 64] = [
        0xd76a_a478,
        0xe8c7_b756,
        0x2420_70db,
        0xc1bd_ceee,
        0xf57c_0faf,
        0x4787_c62a,
        0xa830_4613,
        0xfd46_9501,
        0x6980_98d8,
        0x8b44_f7af,
        0xffff_5bb1,
        0x895c_d7be,
        0x6b90_1122,
        0xfd98_7193,
        0xa679_438e,
        0x49b4_0821,
        0xf61e_2562,
        0xc040_b340,
        0x265e_5a51,
        0xe9b6_c7aa,
        0xd62f_105d,
        0x0244_1453,
        0xd8a1_e681,
        0xe7d3_fbc8,
        0x21e1_cde6,
        0xc337_07d6,
        0xf4d5_0d87,
        0x455a_14ed,
        0xa9e3_e905,
        0xfcef_a3f8,
        0x676f_02d9,
        0x8d2a_4c8a,
        0xfffa_3942,
        0x8771_f681,
        0x6d9d_6122,
        0xfde5_380c,
        0xa4be_ea44,
        0x4bde_cfa9,
        0xf6bb_4b60,
        0xbebf_bc70,
        0x289b_7ec6,
        0xeaa1_27fa,
        0xd4ef_3085,
        0x0488_1d05,
        0xd9d4_d039,
        0xe6db_99e5,
        0x1fa2_7cf8,
        0xc4ac_5665,
        0xf429_2244,
        0x432a_ff97,
        0xab94_23a7,
        0xfc93_a039,
        0x655b_59c3,
        0x8f0c_cc92,
        0xffef_f47d,
        0x8584_5dd1,
        0x6fa8_7e4f,
        0xfe2c_e6e0,
        0xa301_4314,
        0x4e08_11a1,
        0xf753_7e82,
        0xbd3a_f235,
        0x2ad7_d2bb,
        0xeb86_d391,
    ];

    const SHIFTS: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];

    fn new() -> Self {
        Self {
            state: [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476],
            block: [0; 64],
            filled: 0,
            length: 0,
        }
    }

    fn compress(&mut self) {
        let words: Vec<u32> = self
            .block
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| u32::from_le_bytes(*b))
            .collect();
        let [mut a, mut b, mut c, mut d] = self.state;
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let sum = a
                .wrapping_add(f)
                .wrapping_add(Self::K[i])
                .wrapping_add(words[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(sum.rotate_left(Self::SHIFTS[i]));
        }
        for (s, v) in self.state.iter_mut().zip([a, b, c, d]) {
            *s = s.wrapping_add(v);
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        self.length += bytes.len() as u64;
        for &byte in bytes {
            self.block[self.filled] = byte;
            self.filled += 1;
            if self.filled == 64 {
                self.compress();
                self.filled = 0;
            }
        }
    }

    fn finish(mut self) -> [u8; 16] {
        let bits = self.length.wrapping_mul(8);
        self.update(&[0x80]);
        while self.filled != 56 {
            self.update(&[0]);
        }
        self.update(&bits.to_le_bytes());
        let mut out = [0u8; 16];
        for (chunk, s) in out.as_chunks_mut::<4>().0.iter_mut().zip(self.state) {
            *chunk = s.to_le_bytes();
        }
        out
    }
}

/// The bytes of the `fLaC` marker and the stream info block.
fn stream_info(
    (min_frame, max_frame): (usize, usize),
    (channels, bps, rate): (usize, u32, u32),
    (frames, digest): (u64, [u8; 16]),
) -> Vec<u8> {
    let block = BLOCK.min(frames as usize).max(16) as u64;
    let mut bits = Bits::default();
    bits.put(0x80, 8);
    bits.put(34, 24);
    bits.put(block, 16);
    bits.put(block, 16);
    bits.put(min_frame as u64, 24);
    bits.put(max_frame as u64, 24);
    bits.put(u64::from(rate), 20);
    bits.put(channels as u64 - 1, 3);
    bits.put(u64::from(bps) - 1, 5);
    bits.put(frames, 36);
    let mut out = b"fLaC".to_vec();
    out.extend_from_slice(&bits.out);
    out.extend_from_slice(&digest);
    out
}

/// Writes `channels` (equal lengths, one to eight of them) as a FLAC stream: `format` is the
/// sample size (16-bit or 24-bit PCM through the dither `(mode, seed)`; 32-bit float has no
/// FLAC form).
///
/// # Errors
/// No channels, more than eight, channels of different lengths, 32-bit float, a sample rate
/// above 1 048 575 Hz, or an I/O error.
pub fn write_flac<W: Write>(
    mut writer: W,
    channels: &[Vec<f32>],
    sample_rate: u32,
    format: SampleFormat,
    (dither, seed): (DitherMode, u64),
) -> Result<(), WavError> {
    let bps = match format {
        SampleFormat::Pcm16 => 16,
        SampleFormat::Pcm24 => 24,
        SampleFormat::F32 => {
            return Err(WavError::Unsupported(
                "FLAC holds 16-bit and 24-bit integer audio, not 32-bit float".to_owned(),
            ));
        }
    };
    let frames = channels.first().ok_or(WavError::NoChannels)?.len();
    if channels.iter().any(|c| c.len() != frames) {
        return Err(WavError::UnequalChannels);
    }
    if channels.len() > MAX_CHANNELS {
        return Err(WavError::Unsupported(format!(
            "FLAC holds at most {MAX_CHANNELS} channels, not {}",
            channels.len()
        )));
    }
    if sample_rate == 0 || sample_rate >= 1 << 20 {
        return Err(WavError::Unsupported(format!(
            "a FLAC file cannot have a sample rate of {sample_rate} Hz"
        )));
    }
    // The codes of every channel, quantised in time order like the WAV writer does.
    let mut quantiser = Dither::new(dither, seed, channels.len());
    let mut codes: Vec<Vec<i32>> = vec![Vec::with_capacity(frames); channels.len()];
    for n in 0..frames {
        for (c, channel) in channels.iter().enumerate() {
            codes[c].push(quantiser.quantise(c, channel[n], bps));
        }
    }
    let mut md5 = Md5::new();
    let width = (bps / 8) as usize;
    let mut bytes = Vec::with_capacity(4096 * channels.len() * width);
    for start in (0..frames).step_by(BLOCK) {
        bytes.clear();
        for n in start..(start + BLOCK).min(frames) {
            for channel in &codes {
                bytes.extend_from_slice(&channel[n].to_le_bytes()[..width]);
            }
        }
        md5.update(&bytes);
    }
    let mut body = Vec::new();
    let (mut smallest, mut largest) = (usize::MAX, 0usize);
    for (number, start) in (0..frames).step_by(BLOCK).enumerate() {
        let end = (start + BLOCK).min(frames);
        let block: Vec<&[i32]> = codes.iter().map(|c| &c[start..end]).collect();
        let before = body.len();
        encode_frame(&mut body, number as u64, &block, bps);
        let size = body.len() - before;
        smallest = smallest.min(size);
        largest = largest.max(size);
    }
    if frames == 0 {
        smallest = 0;
    }
    let head = stream_info(
        (smallest, largest),
        (channels.len(), bps, sample_rate),
        (frames as u64, md5.finish()),
    );
    let io = |e: std::io::Error| WavError::Hound(hound::Error::from(e));
    writer.write_all(&head).map_err(io)?;
    writer.write_all(&body).map_err(io)?;
    writer.flush().map_err(io)
}

/// [`write_flac`] to a file (created or truncated).
///
/// # Errors
/// As [`write_flac`].
pub fn write_flac_file(
    path: &Path,
    channels: &[Vec<f32>],
    sample_rate: u32,
    format: SampleFormat,
    dither: (DitherMode, u64),
) -> Result<(), WavError> {
    let file = std::fs::File::create(path).map_err(|e| WavError::Hound(hound::Error::from(e)))?;
    write_flac(
        std::io::BufWriter::new(file),
        channels,
        sample_rate,
        format,
        dither,
    )
}

#[cfg(test)]
mod tests;
