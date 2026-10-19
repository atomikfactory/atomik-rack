//! Console meter ballistics: pure maths, no time source.
//!
//! The engine publishes per-channel peak/RMS (linear amplitude) and the dynamics gain;
//! the app reads them once per frame and passes them in with
//! [`crate::ConsoleView::set_meters`], then passes the frame time with
//! [`crate::ConsoleView::set_frame_time`]. The view turns them into what is drawn:
//!
//! - **Peak and RMS bars:** instant attack; release at [`PEAK_FALL_DB_PER_S`] and
//!   [`RMS_FALL_DB_PER_S`]. Levels are in dBFS, floored at [`METER_FLOOR_DB`].
//! - **Peak hold:** the highest peak stays for [`PEAK_HOLD_S`], then falls at the peak rate
//!   (never below the peak bar).
//! - **Clip:** lit for [`CLIP_HOLD_S`] after any peak at or above 0 dBFS.
//! - **Gain reduction:** shown in dB (`-20·log10(gr)`), instant attack, release at
//!   [`GR_RELEASE_DB_PER_S`].
//! - **Scale:** [`meter_norm`] maps dB to 0..1 bar height with an IEC 60268-18 style
//!   piecewise deflection, extended linearly up to [`METER_TOP_DB`].

/// Lowest level shown (dBFS); anything quieter reads as silence.
pub const METER_FLOOR_DB: f32 = -60.0;
/// Top of the meter scale (dBFS).
pub const METER_TOP_DB: f32 = 6.0;
/// Peak bar release rate.
pub const PEAK_FALL_DB_PER_S: f32 = 24.0;
/// RMS bar release rate.
pub const RMS_FALL_DB_PER_S: f32 = 12.0;
/// Peak hold time.
pub const PEAK_HOLD_S: f32 = 1.5;
/// Clip indicator hold time.
pub const CLIP_HOLD_S: f32 = 2.0;
/// Gain-reduction release rate.
pub const GR_RELEASE_DB_PER_S: f32 = 30.0;
/// Largest frame step applied at once (a stalled app does not make meters jump).
pub const MAX_FRAME_DT: f64 = 0.25;
/// Largest gain reduction shown (dB).
pub const GR_MAX_DB: f32 = 24.0;

/// One frame of meter values for a channel (or the master), in linear amplitude.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChannelMeters {
    /// Output peak, left.
    pub out_peak_l: f32,
    /// Output peak, right.
    pub out_peak_r: f32,
    /// Output RMS, left.
    pub out_rms_l: f32,
    /// Output RMS, right.
    pub out_rms_r: f32,
    /// Dynamics gain as a linear factor: 1.0 = no gain reduction, 0.5 = 6 dB of reduction.
    pub gr: f32,
}

impl Default for ChannelMeters {
    fn default() -> Self {
        Self::SILENT
    }
}

impl ChannelMeters {
    /// Silence and no gain reduction.
    pub const SILENT: Self = Self {
        out_peak_l: 0.0,
        out_peak_r: 0.0,
        out_rms_l: 0.0,
        out_rms_r: 0.0,
        gr: 1.0,
    };
}

/// Linear amplitude to meter dB, floored at [`METER_FLOOR_DB`] (NaN and negatives read as
/// silence).
pub fn amp_to_meter_db(amp: f32) -> f32 {
    if amp.is_nan() || amp <= 0.0 {
        return METER_FLOOR_DB;
    }
    (20.0 * amp.log10()).max(METER_FLOOR_DB)
}

/// Bar height (0..1) of a level in dB: IEC 60268-18 style deflection, 0 at
/// [`METER_FLOOR_DB`] and below, 1 at [`METER_TOP_DB`].
pub fn meter_norm(db: f32) -> f64 {
    if db.is_nan() || db <= METER_FLOOR_DB {
        return 0.0;
    }
    let d = f64::from(db.min(METER_TOP_DB));
    // Deflection in percent of the 0 dB point (piecewise linear in dB).
    let pct = if d < -50.0 {
        (d + 60.0) * 0.5 + 2.5
    } else if d < -40.0 {
        (d + 50.0) * 0.75 + 7.5
    } else if d < -30.0 {
        (d + 40.0) * 1.5 + 15.0
    } else if d < -20.0 {
        (d + 30.0) * 2.0 + 30.0
    } else {
        (d + 20.0) * 2.5 + 50.0
    };
    // Floor maps to 0: lift the -60 dB start (2.5 %) to zero smoothly over the first decade.
    let pct = if d < -50.0 {
        pct * (d + 60.0) / 10.0
    } else {
        pct
    };
    let top = (f64::from(METER_TOP_DB) + 20.0) * 2.5 + 50.0;
    (pct / top).clamp(0.0, 1.0)
}

/// One meter bar: level with instant attack and linear-in-dB release, plus an optional hold.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ballistic {
    /// Displayed level (dB).
    pub level_db: f32,
    /// Held peak (dB).
    pub hold_db: f32,
    hold_age: f32,
}

impl Default for Ballistic {
    fn default() -> Self {
        Self::new()
    }
}

impl Ballistic {
    /// At the floor.
    pub fn new() -> Self {
        Self {
            level_db: METER_FLOOR_DB,
            hold_db: METER_FLOOR_DB,
            hold_age: 0.0,
        }
    }

    /// Advances by `dt` seconds towards `input_db`, releasing at `fall` dB/s.
    pub fn step(&mut self, input_db: f32, dt: f32, fall: f32) {
        let input = input_db.max(METER_FLOOR_DB);
        let dt = dt.max(0.0);
        self.level_db = input.max(self.level_db - fall * dt).max(METER_FLOOR_DB);
        self.hold_age += dt;
        if input >= self.hold_db {
            self.hold_db = input;
            self.hold_age = 0.0;
        } else if self.hold_age > PEAK_HOLD_S {
            // Only the part of this step past the hold time falls.
            self.hold_db -= fall * (self.hold_age - PEAK_HOLD_S).min(dt);
        }
        self.hold_db = self.hold_db.max(self.level_db);
    }
}

/// The displayed state of one stereo meter with gain reduction.
#[derive(Debug, Clone, PartialEq)]
pub struct MeterBallistics {
    /// Peak bars (L, R).
    pub peak: [Ballistic; 2],
    /// RMS bars (L, R).
    pub rms: [Ballistic; 2],
    /// Displayed gain reduction (dB, ≥ 0).
    pub gr_db: f32,
    clip_age: [f32; 2],
    input: ChannelMeters,
    merging: bool,
}

impl Default for MeterBallistics {
    fn default() -> Self {
        Self::new()
    }
}

impl MeterBallistics {
    /// Silent meter.
    pub fn new() -> Self {
        Self {
            peak: [Ballistic::new(); 2],
            rms: [Ballistic::new(); 2],
            gr_db: 0.0,
            clip_age: [f32::INFINITY; 2],
            input: ChannelMeters::SILENT,
            merging: false,
        }
    }

    /// Records the latest reading. Several readings before one [`Self::advance`] keep the
    /// highest peaks and the most gain reduction, and the latest RMS.
    pub fn set_input(&mut self, m: ChannelMeters) {
        if self.merging {
            let i = &mut self.input;
            i.out_peak_l = max_amp(i.out_peak_l, m.out_peak_l);
            i.out_peak_r = max_amp(i.out_peak_r, m.out_peak_r);
            i.out_rms_l = m.out_rms_l;
            i.out_rms_r = m.out_rms_r;
            i.gr = if m.gr.is_nan() || i.gr.is_nan() {
                m.gr
            } else {
                i.gr.min(m.gr)
            };
        } else {
            self.input = m;
            self.merging = true;
        }
    }

    /// Applies the recorded reading over `dt` seconds (clamped to 0..=[`MAX_FRAME_DT`]).
    /// Returns true when anything drawn changed.
    pub fn advance(&mut self, dt: f64) -> bool {
        let dt = if dt.is_nan() {
            0.0
        } else {
            dt.clamp(0.0, MAX_FRAME_DT)
        } as f32;
        let before = self.snapshot();
        let m = self.input;
        for (ch, (peak, rms)) in [(m.out_peak_l, m.out_rms_l), (m.out_peak_r, m.out_rms_r)]
            .into_iter()
            .enumerate()
        {
            self.peak[ch].step(amp_to_meter_db(peak), dt, PEAK_FALL_DB_PER_S);
            self.rms[ch].step(amp_to_meter_db(rms), dt, RMS_FALL_DB_PER_S);
            self.clip_age[ch] = if peak >= 1.0 {
                0.0
            } else {
                self.clip_age[ch] + dt
            };
        }
        let gr_in = if m.gr.is_nan() || m.gr >= 1.0 {
            0.0
        } else if m.gr <= 0.0 {
            GR_MAX_DB
        } else {
            (-20.0 * m.gr.log10()).min(GR_MAX_DB)
        };
        self.gr_db = gr_in.max(self.gr_db - GR_RELEASE_DB_PER_S * dt).max(0.0);
        self.merging = false;
        let after = self.snapshot();
        before
            .iter()
            .zip(after.iter())
            .any(|(a, b)| (a - b).abs() > 1e-3)
    }

    fn snapshot(&self) -> [f32; 9] {
        let flag = |b: bool| if b { 1.0 } else { 0.0 };
        [
            self.peak[0].level_db,
            self.peak[1].level_db,
            self.peak[0].hold_db,
            self.peak[1].hold_db,
            self.rms[0].level_db,
            self.rms[1].level_db,
            self.gr_db,
            flag(self.clip(0)),
            flag(self.clip(1)),
        ]
    }

    /// Whether channel `ch` (0 = L, 1 = R) clipped within the last [`CLIP_HOLD_S`].
    pub fn clip(&self, ch: usize) -> bool {
        self.clip_age.get(ch).is_some_and(|a| *a < CLIP_HOLD_S)
    }
}

fn max_amp(a: f32, b: f32) -> f32 {
    if a.is_nan() {
        b
    } else if b.is_nan() {
        a
    } else {
        a.max(b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() <= tol
    }

    fn meters(peak: f32, rms: f32, gr: f32) -> ChannelMeters {
        ChannelMeters {
            out_peak_l: peak,
            out_peak_r: peak * 0.5,
            out_rms_l: rms,
            out_rms_r: rms * 0.5,
            gr,
        }
    }

    #[test]
    fn amplitude_converts_to_floored_db() {
        assert!(close(amp_to_meter_db(1.0), 0.0, 1e-4));
        assert!(close(amp_to_meter_db(0.5), -6.0206, 1e-3));
        assert_eq!(amp_to_meter_db(0.0), METER_FLOOR_DB);
        assert_eq!(amp_to_meter_db(1e-9), METER_FLOOR_DB);
        assert_eq!(amp_to_meter_db(-0.5), METER_FLOOR_DB);
        assert_eq!(amp_to_meter_db(f32::NAN), METER_FLOOR_DB);
        assert!(close(amp_to_meter_db(2.0), 6.0206, 1e-3));
    }

    #[test]
    fn meter_scale_is_monotonic_and_spans_zero_to_one() {
        assert_eq!(meter_norm(METER_FLOOR_DB), 0.0);
        assert_eq!(meter_norm(-90.0), 0.0);
        assert!((meter_norm(METER_TOP_DB) - 1.0).abs() < 1e-9);
        assert!((meter_norm(20.0) - 1.0).abs() < 1e-9);
        // 0 dBFS sits at 100 / 115 of the bar.
        assert!((meter_norm(0.0) - 100.0 / 115.0).abs() < 1e-6);
        // -20 dB at 50 % of the 0 dB deflection.
        assert!((meter_norm(-20.0) - 50.0 / 115.0).abs() < 1e-6);
        let mut prev = -1.0;
        let mut db = -60.0;
        while db <= 6.0 {
            let n = meter_norm(db);
            assert!(n >= prev, "{db}");
            prev = n;
            db += 0.25;
        }
    }

    #[test]
    fn attack_is_instant_and_release_is_linear_in_db() {
        let mut b = Ballistic::new();
        b.step(-6.0, 0.0, PEAK_FALL_DB_PER_S);
        assert_eq!(b.level_db, -6.0);
        b.step(METER_FLOOR_DB, 0.5, PEAK_FALL_DB_PER_S);
        assert!(close(b.level_db, -18.0, 1e-4), "{}", b.level_db);
        b.step(METER_FLOOR_DB, 10.0, PEAK_FALL_DB_PER_S);
        assert_eq!(b.level_db, METER_FLOOR_DB);
        // A louder input during the release jumps straight up.
        b.step(-3.0, 0.1, PEAK_FALL_DB_PER_S);
        assert_eq!(b.level_db, -3.0);
    }

    #[test]
    fn peak_hold_stays_then_falls_but_never_below_the_bar() {
        let mut b = Ballistic::new();
        b.step(-1.0, 0.0, PEAK_FALL_DB_PER_S);
        b.step(-40.0, 1.0, PEAK_FALL_DB_PER_S);
        assert_eq!(b.hold_db, -1.0, "held during the hold time");
        b.step(-40.0, 0.5, PEAK_FALL_DB_PER_S);
        // 1.5 s reached: hold time over, no fall yet beyond the boundary.
        assert!(close(b.hold_db, -1.0, 1e-3), "{}", b.hold_db);
        b.step(-40.0, 0.5, PEAK_FALL_DB_PER_S);
        assert!(close(b.hold_db, -13.0, 1e-3), "{}", b.hold_db);
        b.step(-40.0, 5.0, PEAK_FALL_DB_PER_S);
        assert_eq!(b.hold_db, b.level_db, "the hold never sinks below the bar");
        // A new higher peak restarts the hold.
        b.step(-2.0, 0.0, PEAK_FALL_DB_PER_S);
        assert_eq!(b.hold_db, -2.0);
    }

    #[test]
    fn stereo_meter_tracks_inputs_with_ballistics() {
        let mut m = MeterBallistics::new();
        m.set_input(meters(0.5, 0.25, 1.0));
        assert!(m.advance(1.0 / 30.0));
        assert!(close(m.peak[0].level_db, -6.02, 0.01));
        assert!(close(m.peak[1].level_db, -12.04, 0.01));
        assert!(close(m.rms[0].level_db, -12.04, 0.01));
        // Silence: peak falls at 24 dB/s, RMS at 12 dB/s.
        m.set_input(ChannelMeters::SILENT);
        m.advance(0.25);
        assert!(close(m.peak[0].level_db, -12.02, 0.01));
        assert!(close(m.rms[0].level_db, -15.04, 0.01));
        // Nothing changes once everything rests at the floor.
        for _ in 0..40 {
            m.advance(0.25);
        }
        assert!(!m.advance(0.25));
    }

    #[test]
    fn peaks_between_frames_are_not_lost() {
        let mut m = MeterBallistics::new();
        m.set_input(meters(0.9, 0.1, 1.0));
        m.set_input(meters(0.1, 0.2, 1.0));
        m.advance(0.0);
        assert!(close(m.peak[0].level_db, amp_to_meter_db(0.9), 1e-4));
        assert!(close(m.rms[0].level_db, amp_to_meter_db(0.2), 1e-4));
        // The next frame starts from the new reading only.
        m.set_input(meters(0.1, 0.2, 1.0));
        m.advance(0.0);
        assert!(close(m.peak[0].level_db, amp_to_meter_db(0.9), 1e-4));
        for _ in 0..40 {
            m.advance(0.25);
        }
        assert!(close(m.peak[0].level_db, amp_to_meter_db(0.1), 1e-4));
    }

    #[test]
    fn frame_steps_are_clamped() {
        let mut m = MeterBallistics::new();
        m.set_input(meters(1.0, 1.0, 1.0));
        m.advance(0.0);
        m.set_input(ChannelMeters::SILENT);
        m.advance(100.0);
        // At most MAX_FRAME_DT applied: 0.25 s * 24 dB/s = 6 dB.
        assert!(
            close(m.peak[0].level_db, -6.0, 1e-3),
            "{}",
            m.peak[0].level_db
        );
        m.set_input(ChannelMeters::SILENT);
        m.advance(-1.0);
        assert!(close(m.peak[0].level_db, -6.0, 1e-3));
    }

    #[test]
    fn clip_lights_at_full_scale_and_holds() {
        let mut m = MeterBallistics::new();
        m.set_input(meters(0.99, 0.5, 1.0));
        m.advance(0.03);
        assert!(!m.clip(0));
        m.set_input(meters(1.0, 0.5, 1.0));
        m.advance(0.03);
        assert!(m.clip(0));
        assert!(!m.clip(1), "right peak is 0.5");
        for _ in 0..7 {
            m.set_input(ChannelMeters::SILENT);
            m.advance(0.25);
        }
        assert!(m.clip(0), "still lit at 1.75 s");
        m.set_input(ChannelMeters::SILENT);
        m.advance(0.25);
        assert!(!m.clip(0), "off after the hold");
    }

    #[test]
    fn gain_reduction_attacks_instantly_and_releases() {
        let mut m = MeterBallistics::new();
        m.set_input(meters(0.5, 0.5, 0.5));
        m.advance(0.03);
        assert!(close(m.gr_db, 6.02, 0.01));
        m.set_input(meters(0.5, 0.5, 1.0));
        m.advance(0.1);
        assert!(close(m.gr_db, 3.02, 0.01));
        m.advance(1.0);
        assert_eq!(m.gr_db, 0.0);
        // Deep reduction is capped; nonsense reads as none.
        m.set_input(meters(0.5, 0.5, 0.0));
        m.advance(0.0);
        assert_eq!(m.gr_db, GR_MAX_DB);
        let mut n = MeterBallistics::new();
        n.set_input(meters(0.5, 0.5, f32::NAN));
        n.advance(0.0);
        assert_eq!(n.gr_db, 0.0);
    }
}
