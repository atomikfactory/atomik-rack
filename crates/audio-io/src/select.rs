//! Pure host/device/config selection logic, independent of cpal so it is unit-testable.

use crate::{HostKind, IoError};

/// Sample formats the stream can drive (f32 preferred; integer formats are converted).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum SampleKind {
    /// 32-bit float (preferred).
    F32,
    /// 32-bit signed integer.
    I32,
    /// 16-bit signed integer.
    I16,
}

/// One supported configuration range, reduced to what selection needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ConfigCandidate {
    pub(crate) channels: u16,
    pub(crate) min_rate: u32,
    pub(crate) max_rate: u32,
    pub(crate) format: SampleKind,
    pub(crate) buffer: Option<(u32, u32)>,
}

/// The cpal host name for a host kind (`None` for [`HostKind::Default`]).
pub(crate) fn host_name(kind: HostKind) -> Option<&'static str> {
    match kind {
        HostKind::Default => None,
        HostKind::Wasapi => Some("WASAPI"),
        HostKind::Asio => Some("ASIO"),
        HostKind::CoreAudio => Some("CoreAudio"),
        HostKind::Jack => Some("JACK"),
        HostKind::Alsa => Some("ALSA"),
    }
}

/// The host kind for a cpal host name, if Atomik Rack knows it.
pub(crate) fn host_kind(name: &str) -> Option<HostKind> {
    [
        HostKind::Wasapi,
        HostKind::Asio,
        HostKind::CoreAudio,
        HostKind::Jack,
        HostKind::Alsa,
    ]
    .into_iter()
    .find(|k| host_name(*k).is_some_and(|n| n.eq_ignore_ascii_case(name)))
}

/// Exact name first, then a unique case-insensitive substring.
pub(crate) fn match_device(names: &[String], query: &str) -> Result<usize, IoError> {
    if let Some(i) = names.iter().position(|n| n == query) {
        return Ok(i);
    }
    let needle = query.to_lowercase();
    let hits: Vec<usize> = names
        .iter()
        .enumerate()
        .filter(|(_, n)| !needle.is_empty() && n.to_lowercase().contains(&needle))
        .map(|(i, _)| i)
        .collect();
    match hits.as_slice() {
        [one] => Ok(*one),
        [] => Err(IoError::DeviceNotFound(format!(
            "no device matches \"{query}\" (available: {names:?})"
        ))),
        many => Err(IoError::DeviceNotFound(format!(
            "\"{query}\" is ambiguous: {:?}",
            many.iter().map(|&i| &names[i]).collect::<Vec<_>>()
        ))),
    }
}

/// The input device a stream opens when the settings name none: the index of the device named
/// exactly like the output device (case-insensitive; ASIO and CoreAudio list one device for both
/// directions), `None` when the host has no such input (then no input opens; the host default input
/// is never used, as it may be an unrelated microphone).
pub(crate) fn default_input_index(input_names: &[String], output_name: &str) -> Option<usize> {
    if output_name.is_empty() {
        return None;
    }
    input_names
        .iter()
        .position(|n| n.eq_ignore_ascii_case(output_name))
}

/// Picks the configuration to open: the rate must be supported; channels prefer an exact match,
/// then the smallest count above, then the largest below; formats prefer f32, then i32, then i16.
pub(crate) fn choose_config(
    candidates: &[ConfigCandidate],
    channels: u16,
    sample_rate: u32,
) -> Option<ConfigCandidate> {
    candidates
        .iter()
        .filter(|c| (c.min_rate..=c.max_rate).contains(&sample_rate))
        .min_by_key(|c| {
            let fit = if c.channels == channels {
                (0, 0)
            } else if c.channels > channels {
                (1, c.channels)
            } else {
                (2, u16::MAX - c.channels)
            };
            (fit, c.format)
        })
        .copied()
}

/// The fixed buffer size to request: `requested` clamped to the supported range, if known.
pub(crate) fn buffer_request(requested: u32, range: Option<(u32, u32)>) -> u32 {
    let requested = requested.max(1);
    match range {
        Some((min, max)) if min <= max => requested.clamp(min.max(1), max.max(1)),
        _ => requested,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_default_input_is_the_output_device_when_the_host_lists_it_as_an_input() {
        let names: Vec<String> = ["Mic (USB)", "Example USB ASIO", "Line"]
            .map(String::from)
            .to_vec();
        assert_eq!(default_input_index(&names, "Example USB ASIO"), Some(1));
        assert_eq!(default_input_index(&names, "example usb asio"), Some(1));
        assert_eq!(default_input_index(&names, "Virtual Cable Input"), None);
        assert_eq!(default_input_index(&names, ""), None);
        assert_eq!(default_input_index(&[], "Example USB ASIO"), None);
    }

    use super::*;

    fn cand(channels: u16, format: SampleKind) -> ConfigCandidate {
        ConfigCandidate {
            channels,
            min_rate: 44_100,
            max_rate: 96_000,
            format,
            buffer: Some((32, 4096)),
        }
    }

    #[test]
    fn host_name_and_kind_round_trip() {
        for kind in [
            HostKind::Wasapi,
            HostKind::Asio,
            HostKind::CoreAudio,
            HostKind::Jack,
            HostKind::Alsa,
        ] {
            let name = host_name(kind).unwrap();
            assert_eq!(host_kind(name), Some(kind));
            assert_eq!(host_kind(&name.to_lowercase()), Some(kind));
        }
        assert_eq!(host_name(HostKind::Default), None);
        assert_eq!(host_kind("PulseAudio"), None);
    }

    #[test]
    fn match_device_exact_then_unique_substring() {
        let names = vec![
            "Speakers (2- Example USB Audio)".to_string(),
            "Virtual Cable Input".to_string(),
            "Speakers (Generic Audio)".to_string(),
        ];
        assert_eq!(match_device(&names, "Virtual Cable Input"), Ok(1));
        assert_eq!(match_device(&names, "cable input"), Ok(1));
        assert_eq!(match_device(&names, "example usb"), Ok(0));
        assert!(
            matches!(match_device(&names, "speakers"), Err(IoError::DeviceNotFound(m)) if m.contains("ambiguous"))
        );
        assert!(matches!(
            match_device(&names, "nope"),
            Err(IoError::DeviceNotFound(_))
        ));
        assert!(matches!(
            match_device(&names, ""),
            Err(IoError::DeviceNotFound(_))
        ));
    }

    #[test]
    fn match_device_exact_match_wins_over_substring_ambiguity() {
        let names = vec!["Out".to_string(), "Out 2".to_string()];
        assert_eq!(match_device(&names, "Out"), Ok(0));
    }

    #[test]
    fn choose_config_prefers_f32_and_exact_channels() {
        let c = [
            cand(2, SampleKind::I16),
            cand(2, SampleKind::F32),
            cand(8, SampleKind::F32),
            cand(2, SampleKind::I32),
        ];
        assert_eq!(choose_config(&c, 2, 48_000), Some(c[1]));
    }

    #[test]
    fn choose_config_falls_back_to_integer_formats() {
        let c = [cand(2, SampleKind::I16), cand(2, SampleKind::I32)];
        assert_eq!(
            choose_config(&c, 2, 48_000).unwrap().format,
            SampleKind::I32
        );
        let c = [cand(2, SampleKind::I16)];
        assert_eq!(
            choose_config(&c, 2, 48_000).unwrap().format,
            SampleKind::I16
        );
    }

    #[test]
    fn choose_config_channels_smallest_above_then_largest_below() {
        let c = [
            cand(8, SampleKind::F32),
            cand(4, SampleKind::F32),
            cand(1, SampleKind::F32),
        ];
        assert_eq!(choose_config(&c, 2, 48_000).unwrap().channels, 4);
        let c = [cand(1, SampleKind::F32), cand(2, SampleKind::I16)];
        assert_eq!(choose_config(&c, 6, 48_000).unwrap().channels, 2);
    }

    #[test]
    fn choose_config_rejects_unsupported_rate() {
        let c = [cand(2, SampleKind::F32)];
        assert_eq!(choose_config(&c, 2, 192_000), None);
        assert_eq!(choose_config(&[], 2, 48_000), None);
    }

    #[test]
    fn buffer_request_clamps_to_range() {
        assert_eq!(buffer_request(128, Some((32, 4096))), 128);
        assert_eq!(buffer_request(16, Some((32, 4096))), 32);
        assert_eq!(buffer_request(9000, Some((32, 4096))), 4096);
        assert_eq!(buffer_request(128, None), 128);
        assert_eq!(buffer_request(0, None), 1);
    }
}
