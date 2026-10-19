//! The pure parts of the host side: value mapping between the rack's plain values and VST3
//! normalised values, `restartComponent` flags, and VST3 transport.

use atomik_plugin_host::PluginTransport;

/// The step counts of the stepped params of a plugin, sorted by param id.
pub(crate) type Steps = Vec<(u32, u32)>;

/// The rack's value of a normalised parameter value: the step index for a stepped param, the
/// normalised value itself otherwise.
pub(crate) fn host_value(steps: &Steps, id: u32, normalized: f64) -> f64 {
    match steps.binary_search_by_key(&id, |s| s.0) {
        Ok(i) => (normalized.clamp(0.0, 1.0) * f64::from(steps[i].1)).round(),
        Err(_) => normalized,
    }
}

/// The normalised value of a rack value: `index / stepCount` for a stepped param, the value
/// itself otherwise; always in 0 to 1 (NaN becomes 0).
pub(crate) fn normalized(steps: &Steps, id: u32, host: f64) -> f64 {
    let v = match steps.binary_search_by_key(&id, |s| s.0) {
        Ok(i) => host / f64::from(steps[i].1.max(1)),
        Err(_) => host,
    };
    if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) }
}

/// What a `restartComponent` call asks the host to do.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct RestartActions {
    /// `kLatencyChanged`: read the latency again (a restart re-activates the plugin).
    pub latency: bool,
    /// `kReloadComponent` or `kIoChanged`: buses may have changed, re-activate and re-read.
    pub reload: bool,
    /// `kParamTitlesChanged` (or a reload): the param table changed.
    pub params: bool,
    /// `kMidiCCAssignmentChanged` or `kNoteExpressionChanged`: the processor needs the new maps.
    pub midi: bool,
    /// `kParamValuesChanged`: values changed inside the plugin; its state is dirty.
    pub values: bool,
}

/// Decodes `RestartFlags`.
pub(crate) fn restart_actions(flags: i32) -> RestartActions {
    let has = |f: i32| flags & f != 0;
    let reload = has(1) || has(2); // kReloadComponent, kIoChanged
    RestartActions {
        latency: has(8),
        reload,
        params: has(16) || reload, // kParamTitlesChanged
        midi: has(32) || has(64),  // kMidiCCAssignmentChanged, kNoteExpressionChanged
        values: has(4),
    }
}

/// `ProcessContext::state` bits (VST3 `StatesAndFlags`).
pub(crate) mod state {
    pub const PLAYING: u32 = 1 << 1;
    pub const CYCLE_ACTIVE: u32 = 1 << 2;
    pub const PROJECT_TIME_MUSIC_VALID: u32 = 1 << 9;
    pub const TEMPO_VALID: u32 = 1 << 10;
    pub const BAR_POSITION_VALID: u32 = 1 << 11;
    pub const CYCLE_VALID: u32 = 1 << 12;
    pub const TIME_SIG_VALID: u32 = 1 << 13;
    pub const CONT_TIME_VALID: u32 = 1 << 17;
}

/// The fields of a VST3 `ProcessContext` that the host sets. `systemTime` is zero and
/// its valid bit stays clear.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ContextValues {
    pub state: u32,
    pub sample_rate: f64,
    pub project_time_samples: i64,
    pub continuous_time_samples: i64,
    pub project_time_music: f64,
    pub bar_position_music: f64,
    pub cycle_start_music: f64,
    pub cycle_end_music: f64,
    pub tempo: f64,
    pub time_sig_numerator: i32,
    pub time_sig_denominator: i32,
}

/// Turns the batch's transport into VST3 context values: `projectTimeSamples` is the position in
/// seconds times the sample rate, `continousTimeSamples` the engine's frame counter.
pub(crate) fn context_values(t: &PluginTransport) -> ContextValues {
    let per_bar = f64::from(t.time_signature.0) * 4.0 / f64::from(t.time_signature.1.max(1));
    let bar = if per_bar > 0.0 {
        (t.song_pos_beats / per_bar).floor()
    } else {
        0.0
    };
    let mut flags = state::PROJECT_TIME_MUSIC_VALID
        | state::TEMPO_VALID
        | state::BAR_POSITION_VALID
        | state::CYCLE_VALID
        | state::TIME_SIG_VALID
        | state::CONT_TIME_VALID;
    if t.playing {
        flags |= state::PLAYING;
    }
    if t.loop_active {
        flags |= state::CYCLE_ACTIVE;
    }
    ContextValues {
        state: flags,
        sample_rate: t.sample_rate,
        project_time_samples: (t.song_pos_seconds * t.sample_rate).round() as i64,
        continuous_time_samples: t.steady_frames as i64,
        project_time_music: t.song_pos_beats,
        bar_position_music: bar * per_bar,
        cycle_start_music: t.loop_start_beats,
        cycle_end_music: t.loop_end_beats,
        tempo: t.tempo_bpm,
        time_sig_numerator: i32::from(t.time_signature.0),
        time_sig_denominator: i32::from(t.time_signature.1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stepped_params_are_step_indexes_and_the_rest_pass_through() {
        let steps: Steps = vec![(2, 1), (3, 512)];
        assert_eq!(host_value(&steps, 3, 37.0 / 512.0), 37.0);
        assert_eq!(host_value(&steps, 2, 1.0), 1.0);
        assert_eq!(host_value(&steps, 1, 0.3), 0.3, "not stepped");
        assert_eq!(host_value(&steps, 3, 7.0), 512.0, "clamped");
        assert_eq!(normalized(&steps, 3, 37.0), 37.0 / 512.0);
        assert_eq!(normalized(&steps, 3, 9_999.0), 1.0);
        assert_eq!(normalized(&steps, 3, -4.0), 0.0);
        assert_eq!(normalized(&steps, 9, 0.5), 0.5);
        assert_eq!(normalized(&steps, 9, f64::NAN), 0.0);
        assert_eq!(normalized(&steps, 9, 9.0), 1.0, "clamped");
    }

    #[test]
    fn an_inexact_normalised_step_still_rounds_to_its_index() {
        // 15 / 22 * 22 is 14.999999999999998 in f64: a floor would hand the rack step 14.
        let steps: Steps = vec![(7, 22)];
        assert_eq!(host_value(&steps, 7, 15.0 / 22.0), 15.0);
        assert_eq!(host_value(&steps, 7, 1.0), 22.0);
        assert_eq!(host_value(&steps, 7, -0.2), 0.0, "clamped below");
    }

    #[test]
    fn a_step_round_trips_through_normalised() {
        let steps: Steps = vec![(5, 48)];
        for i in 0..=48 {
            let n = normalized(&steps, 5, f64::from(i));
            assert_eq!(host_value(&steps, 5, n), f64::from(i));
        }
    }

    #[test]
    fn restart_flags_decode_to_actions() {
        assert_eq!(restart_actions(0), RestartActions::default());
        assert!(restart_actions(8).latency);
        assert!(!restart_actions(8).reload);
        let r = restart_actions(1);
        assert!(r.reload && r.params, "a reload re-reads the table");
        assert!(restart_actions(2).reload);
        assert!(restart_actions(16).params && !restart_actions(16).reload);
        assert!(restart_actions(32).midi && restart_actions(64).midi);
        assert!(restart_actions(4).values);
        // Flags that need nothing from us.
        assert_eq!(
            restart_actions(128 | 256 | 512 | 1024 | 2048),
            RestartActions::default()
        );
    }

    #[test]
    fn the_context_carries_tempo_position_bar_loop_and_the_valid_bits() {
        let t = PluginTransport {
            playing: true,
            tempo_bpm: 90.0,
            song_pos_beats: 9.5,
            song_pos_seconds: 6.0,
            steady_frames: 12_345,
            time_signature: (3, 4),
            loop_active: true,
            loop_start_beats: 4.0,
            loop_end_beats: 12.0,
            sample_rate: 48_000.0,
        };
        let c = context_values(&t);
        assert_eq!(c.tempo, 90.0);
        assert_eq!(c.project_time_music, 9.5);
        assert_eq!(
            c.bar_position_music, 9.0,
            "bars of 3 beats: bar 3 starts at 9"
        );
        assert_eq!(c.project_time_samples, 288_000);
        assert_eq!(c.continuous_time_samples, 12_345);
        assert_eq!((c.time_sig_numerator, c.time_sig_denominator), (3, 4));
        assert_eq!((c.cycle_start_music, c.cycle_end_music), (4.0, 12.0));
        assert_ne!(c.state & state::PLAYING, 0);
        assert_ne!(c.state & state::CYCLE_ACTIVE, 0);
        for bit in [
            state::TEMPO_VALID,
            state::TIME_SIG_VALID,
            state::PROJECT_TIME_MUSIC_VALID,
            state::BAR_POSITION_VALID,
            state::CYCLE_VALID,
            state::CONT_TIME_VALID,
        ] {
            assert_ne!(c.state & bit, 0);
        }
        assert_eq!(c.state & (1 << 8), 0, "the system time is never valid");
        let stopped = context_values(&PluginTransport::default());
        assert_eq!(stopped.state & (state::PLAYING | state::CYCLE_ACTIVE), 0);
    }
}
