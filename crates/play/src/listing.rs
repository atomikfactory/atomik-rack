//! `--list` output.

use atomik_audio_io::{DeviceInfo, HostInfo, HostKind, IoError};
use atomik_midi_io::MidiError;
use std::fmt::Write;

/// Formats the listing. A failed enumeration becomes a `warning:` line (returned separately,
/// for stderr) and never an error: `--list` always succeeds, even with no devices at all.
/// Returns `(stdout text, stderr warnings)`.
pub fn format_listing(
    audio: &Result<Vec<HostInfo>, IoError>,
    midi: &Result<Vec<String>, MidiError>,
) -> (String, Vec<String>) {
    const NONE: &str = "  (none)\n";
    let mut out = String::from("audio hosts:\n");
    let mut warnings = Vec::new();
    match audio {
        Ok(hosts) if !hosts.is_empty() => {
            for host in hosts {
                let _ = writeln!(out, "  {}", host_name(host.kind));
                if host.devices.is_empty() {
                    out.push_str("    (no devices)\n");
                }
                for device in &host.devices {
                    let _ = writeln!(out, "    {}", device_line(device));
                }
            }
        }
        Ok(_) => out.push_str(NONE),
        Err(e) => {
            out.push_str(NONE);
            warnings.push(format!("warning: cannot list audio devices: {e}"));
        }
    }
    out.push_str("midi inputs:\n");
    match midi {
        Ok(ports) if !ports.is_empty() => {
            for port in ports {
                let _ = writeln!(out, "  {port:?}");
            }
        }
        Ok(_) => out.push_str(NONE),
        Err(e) => {
            out.push_str(NONE);
            warnings.push(format!("warning: cannot list MIDI inputs: {e}"));
        }
    }
    (out, warnings)
}

/// The `--host` name of a host kind.
pub fn host_name(kind: HostKind) -> &'static str {
    match kind {
        HostKind::Default => "default",
        HostKind::Wasapi => "wasapi",
        HostKind::Asio => "asio",
        HostKind::CoreAudio => "coreaudio",
        HostKind::Jack => "jack",
        HostKind::Alsa => "alsa",
    }
}

/// `* "name" in=0 out=2 rates=44100,48000 buffer=32..2048 (default output)`; the marker is
/// `*` for the default output device and `-` otherwise.
fn device_line(d: &DeviceInfo) -> String {
    let rates = if d.sample_rates.is_empty() {
        "none".to_string()
    } else {
        let rates: Vec<String> = d.sample_rates.iter().map(u32::to_string).collect();
        rates.join(",")
    };
    let buffer = d
        .buffer_range
        .map_or_else(|| "unknown".to_string(), |(lo, hi)| format!("{lo}..{hi}"));
    format!(
        "{} {:?} in={} out={} rates={rates} buffer={buffer}{}",
        if d.is_default_output { '*' } else { '-' },
        d.name,
        d.max_inputs,
        d.max_outputs,
        if d.is_default_output {
            " (default output)"
        } else {
            ""
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_audio_io::{DeviceInfo, HostKind};

    fn device(name: &str, default: bool) -> DeviceInfo {
        DeviceInfo {
            name: name.into(),
            max_inputs: 0,
            max_outputs: 2,
            sample_rates: vec![44_100, 48_000],
            buffer_range: Some((32, 2048)),
            is_default_output: default,
        }
    }

    #[test]
    fn lists_hosts_devices_and_midi_inputs() {
        let audio = Ok(vec![
            HostInfo {
                kind: HostKind::Wasapi,
                devices: vec![
                    device("Speakers (2- Example USB Audio)", true),
                    DeviceInfo {
                        buffer_range: None,
                        max_inputs: 2,
                        sample_rates: vec![],
                        ..device("Virtual Cable Output", false)
                    },
                ],
            },
            HostInfo {
                kind: HostKind::Asio,
                devices: vec![],
            },
        ]);
        let midi = Ok(vec![
            "Example Keyboard".to_string(),
            "Example USB MIDI".to_string(),
        ]);
        let (out, warnings) = format_listing(&audio, &midi);
        assert!(warnings.is_empty());
        let expected = "\
audio hosts:
  wasapi
    * \"Speakers (2- Example USB Audio)\" in=0 out=2 rates=44100,48000 buffer=32..2048 (default output)
    - \"Virtual Cable Output\" in=2 out=2 rates=none buffer=unknown
  asio
    (no devices)
midi inputs:
  \"Example Keyboard\"
  \"Example USB MIDI\"
";
        assert_eq!(out, expected);
    }

    #[test]
    fn enumeration_failures_are_warnings() {
        let audio = Err(IoError::Backend("no audio subsystem".into()));
        let midi = Err(MidiError::Backend("no midi".into()));
        let (out, warnings) = format_listing(&audio, &midi);
        assert_eq!(out, "audio hosts:\n  (none)\nmidi inputs:\n  (none)\n");
        assert_eq!(
            warnings,
            [
                "warning: cannot list audio devices: audio backend error: no audio subsystem",
                "warning: cannot list MIDI inputs: MIDI backend error: no midi"
            ]
        );
    }

    #[test]
    fn empty_lists_say_none() {
        let (out, warnings) = format_listing(&Ok(vec![]), &Ok(vec![]));
        assert_eq!(out, "audio hosts:\n  (none)\nmidi inputs:\n  (none)\n");
        assert!(warnings.is_empty());
    }
}
