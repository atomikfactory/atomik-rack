//! Reading what a CLAP plugin is: its audio and note ports, its features and its params, and
//! turning that into the format-neutral [`PluginLayout`].
//!
//! The port rules are pure functions ([`classify_audio`]) so they are tested without a plugin.

use crate::hub::ClapHost;
use atomik_device_sdk::{
    Channels, MAX_PLUGIN_PARAMS, PluginKindHint, PluginLayout, PluginParamInfo,
};
use clack_extensions::audio_ports::{AudioPortFlags, AudioPortInfoBuffer, PluginAudioPorts};
use clack_extensions::note_ports::{NoteDialects, NotePortInfoBuffer, PluginNotePorts};
use clack_extensions::params::{ParamInfoBuffer, ParamInfoFlags, PluginParams};
use clack_host::prelude::*;

/// What the descriptor says (read from the factory without creating the plugin).
#[derive(Debug, Clone, Default)]
pub(crate) struct DescInfo {
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub features: Vec<String>,
}

/// What an input port is used for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InRole {
    /// The main input.
    Main,
    /// The sidechain (the first aux input).
    Sidechain,
    /// Another aux input: the host feeds it silence.
    Unused,
}

/// One audio port as the host will fill it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PortPlan {
    /// Channels of the port.
    pub channels: u32,
    /// Role of an input port; `Main` for the main output (an output port that is not main is
    /// `Unused` and gets a scratch buffer).
    pub role: InRole,
}

/// The audio ports of a plugin and how the host drives them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct AudioPlan {
    /// Input ports in the plugin's order.
    pub inputs: Vec<PortPlan>,
    /// Output ports in the plugin's order.
    pub outputs: Vec<PortPlan>,
}

impl AudioPlan {
    /// Channels of the main input, if there is one.
    pub fn main_in(&self) -> Option<u32> {
        self.inputs
            .iter()
            .find(|p| p.role == InRole::Main)
            .map(|p| p.channels)
    }

    /// Channels of the sidechain input, if there is one.
    pub fn sidechain(&self) -> Option<u32> {
        self.inputs
            .iter()
            .find(|p| p.role == InRole::Sidechain)
            .map(|p| p.channels)
    }

    /// Channels of the main output, if there is one.
    pub fn main_out(&self) -> Option<u32> {
        self.outputs
            .iter()
            .find(|p| p.role == InRole::Main)
            .map(|p| p.channels)
    }
}

/// An audio port as the plugin declares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeclaredPort {
    pub channels: u32,
    pub is_main: bool,
}

fn channels_of(n: u32, what: &str) -> Result<Channels, String> {
    match n {
        1 => Ok(Channels::Mono),
        2 => Ok(Channels::Stereo),
        _ => Err(format!(
            "unsupported {what} of {n} channels (mono or stereo only)"
        )),
    }
}

/// Decides which ports the host drives and how: the main bus must be mono or stereo,
/// the first other input is the sidechain, every further port is fed silence or collected into
/// scratch.
///
/// # Errors
/// The reason the plugin cannot be hosted.
pub(crate) fn classify_audio(
    inputs: &[DeclaredPort],
    outputs: &[DeclaredPort],
) -> Result<AudioPlan, String> {
    let flagged = |ports: &[DeclaredPort]| ports.iter().position(|p| p.is_main);
    // A plugin that never flags a main port still has its first port as the main one.
    let main_in = flagged(inputs).or(if inputs.is_empty() { None } else { Some(0) });
    let main_out = flagged(outputs).or(if outputs.is_empty() { None } else { Some(0) });
    let mut plan = AudioPlan::default();
    let mut sidechain_taken = false;
    for (i, p) in inputs.iter().enumerate() {
        let role = if Some(i) == main_in {
            channels_of(p.channels, "main input")?;
            InRole::Main
        } else if !sidechain_taken && (p.channels == 1 || p.channels == 2) {
            sidechain_taken = true;
            InRole::Sidechain
        } else {
            InRole::Unused
        };
        plan.inputs.push(PortPlan {
            channels: p.channels,
            role,
        });
    }
    for (i, p) in outputs.iter().enumerate() {
        let role = if Some(i) == main_out {
            channels_of(p.channels, "main output")?;
            InRole::Main
        } else {
            InRole::Unused
        };
        plan.outputs.push(PortPlan {
            channels: p.channels,
            role,
        });
    }
    Ok(plan)
}

/// The notes of a plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct NotePlan {
    /// The note input accepts these dialects (`None` for no note input).
    pub input: Option<NoteDialects>,
    /// A note output exists.
    pub output: bool,
}

impl NotePlan {
    /// True when the host sends CLAP note events (otherwise MIDI 1.0).
    pub fn clap_dialect(&self) -> bool {
        self.input.is_some_and(|d| d.contains(NoteDialects::CLAP))
    }

    /// True when MIDI events (pitch bend, CC, pressure) may be sent.
    pub fn midi(&self) -> bool {
        self.input.is_some_and(|d| d.contains(NoteDialects::MIDI))
    }
}

/// Everything read from an initialised plugin.
#[derive(Debug, Clone)]
pub(crate) struct Read {
    pub layout: PluginLayout,
    pub audio: AudioPlan,
    pub notes: NotePlan,
    pub params_truncated: bool,
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// The visible, writable params of the plugin in its own order, capped at
/// [`MAX_PLUGIN_PARAMS`] (the second value says it was cut).
pub(crate) fn read_params(instance: &mut PluginInstance<ClapHost>) -> (Vec<PluginParamInfo>, bool) {
    let handle = instance.plugin_handle();
    let Some(ext) = handle.get_extension::<PluginParams>() else {
        return (Vec::new(), false);
    };
    let count = ext.count(&handle);
    let mut out: Vec<PluginParamInfo> =
        Vec::with_capacity(count.min(MAX_PLUGIN_PARAMS as u32 + 1) as usize);
    let mut buffer = ParamInfoBuffer::new();
    let mut truncated = false;
    for index in 0..count {
        let Some(info) = ext.get_info(&handle, index, &mut buffer) else {
            continue;
        };
        if info
            .flags
            .intersects(ParamInfoFlags::IS_HIDDEN | ParamInfoFlags::IS_READONLY)
        {
            continue;
        }
        let id = info.id.get();
        if out.iter().any(|p| p.id == id) {
            continue;
        }
        if out.len() >= MAX_PLUGIN_PARAMS {
            truncated = true;
            break;
        }
        out.push(PluginParamInfo {
            id,
            name: text(info.name),
            module: text(info.module),
            min: info.min_value,
            max: info.max_value,
            default: info.default_value,
            stepped: info.flags.contains(ParamInfoFlags::IS_STEPPED),
            steps: 0,
        });
    }
    (out, truncated)
}

fn read_audio(instance: &mut PluginInstance<ClapHost>) -> Result<AudioPlan, String> {
    let handle = instance.plugin_handle();
    let Some(ext) = handle.get_extension::<PluginAudioPorts>() else {
        return Ok(AudioPlan::default());
    };
    let mut buffer = AudioPortInfoBuffer::new();
    let mut read = |is_input: bool| -> Vec<DeclaredPort> {
        (0..ext.count(&handle, is_input))
            .filter_map(|i| {
                ext.get(&handle, i, is_input, &mut buffer)
                    .map(|info| DeclaredPort {
                        channels: info.channel_count,
                        is_main: info.flags.contains(AudioPortFlags::IS_MAIN),
                    })
            })
            .collect()
    };
    let inputs = read(true);
    let outputs = read(false);
    classify_audio(&inputs, &outputs)
}

fn read_notes(instance: &mut PluginInstance<ClapHost>) -> Result<NotePlan, String> {
    let handle = instance.plugin_handle();
    let Some(ext) = handle.get_extension::<PluginNotePorts>() else {
        return Ok(NotePlan::default());
    };
    let mut buffer = NotePortInfoBuffer::new();
    let mut plan = NotePlan::default();
    if ext.count(&handle, true) > 0
        && let Some(info) = ext.get(&handle, 0, true, &mut buffer)
    {
        if !info
            .supported_dialects
            .intersects(NoteDialects::CLAP | NoteDialects::MIDI)
        {
            return Err("the note input supports neither the CLAP nor the MIDI dialect".into());
        }
        plan.input = Some(info.supported_dialects);
    }
    plan.output = ext.count(&handle, false) > 0;
    Ok(plan)
}

fn kind_hint(
    features: &[String],
    layout_in_notes: bool,
    has_audio_out: bool,
    notes_out: bool,
) -> PluginKindHint {
    let has = |f: &str| features.iter().any(|x| x == f);
    if !has_audio_out && notes_out {
        PluginKindHint::NoteEffect
    } else if has("instrument") && layout_in_notes && has_audio_out {
        PluginKindHint::Instrument
    } else {
        PluginKindHint::Effect
    }
}

/// Reads the layout and the port plans of an initialised plugin.
///
/// # Errors
/// The reason the plugin cannot be hosted.
pub(crate) fn read(
    instance: &mut PluginInstance<ClapHost>,
    desc: &DescInfo,
) -> Result<Read, String> {
    let audio = read_audio(instance)?;
    let notes = read_notes(instance)?;
    let (params, params_truncated) = read_params(instance);
    let to_channels = |n: Option<u32>| n.and_then(|n| channels_of(n, "bus").ok());
    let main_in = to_channels(audio.main_in());
    let main_out = to_channels(audio.main_out());
    if main_out.is_none() && !notes.output {
        return Err("the plugin has neither an audio nor a note output".into());
    }
    let layout = PluginLayout {
        name: desc.name.clone(),
        vendor: desc.vendor.clone(),
        version: desc.version.clone(),
        kind_hint: kind_hint(
            &desc.features,
            notes.input.is_some(),
            main_out.is_some(),
            notes.output,
        ),
        main_in,
        main_out,
        sidechain: audio.sidechain().is_some(),
        notes_in: notes.input.is_some(),
        notes_out: notes.output,
        params,
    };
    Ok(Read {
        layout,
        audio,
        notes,
        params_truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn port(channels: u32, is_main: bool) -> DeclaredPort {
        DeclaredPort { channels, is_main }
    }

    #[test]
    fn a_stereo_effect_with_an_aux_input_has_a_sidechain() {
        let plan = classify_audio(&[port(2, true), port(2, false)], &[port(2, true)]).unwrap();
        assert_eq!(plan.main_in(), Some(2));
        assert_eq!(plan.sidechain(), Some(2));
        assert_eq!(plan.main_out(), Some(2));
        assert_eq!(plan.inputs[1].role, InRole::Sidechain);
    }

    #[test]
    fn mono_main_buses_are_accepted() {
        let plan = classify_audio(&[port(1, true)], &[port(1, true)]).unwrap();
        assert_eq!((plan.main_in(), plan.main_out()), (Some(1), Some(1)));
        assert_eq!(plan.sidechain(), None);
    }

    #[test]
    fn a_surround_main_bus_is_unsupported_with_a_reason_naming_the_channels() {
        let err = classify_audio(&[port(6, true)], &[port(2, true)]).unwrap_err();
        assert!(err.contains("6 channels"), "{err}");
        let err = classify_audio(&[], &[port(4, true)]).unwrap_err();
        assert!(err.contains("main output") && err.contains("4"), "{err}");
    }

    #[test]
    fn extra_aux_ports_are_unused_and_only_the_first_aux_input_is_the_sidechain() {
        let plan = classify_audio(
            &[
                port(2, true),
                port(2, false),
                port(2, false),
                port(8, false),
            ],
            &[port(2, true), port(2, false)],
        )
        .unwrap();
        let roles: Vec<InRole> = plan.inputs.iter().map(|p| p.role).collect();
        assert_eq!(
            roles,
            [
                InRole::Main,
                InRole::Sidechain,
                InRole::Unused,
                InRole::Unused
            ]
        );
        assert_eq!(
            plan.outputs[1].role,
            InRole::Unused,
            "aux output into scratch"
        );
    }

    #[test]
    fn a_plugin_that_flags_no_main_port_has_its_first_port_as_main() {
        let plan = classify_audio(&[port(2, false)], &[port(2, false)]).unwrap();
        assert_eq!(plan.main_in(), Some(2));
        assert_eq!(plan.main_out(), Some(2));
    }

    #[test]
    fn instruments_and_note_effects_have_no_main_input() {
        let plan = classify_audio(&[], &[port(2, true)]).unwrap();
        assert_eq!(plan.main_in(), None);
        let plan = classify_audio(&[], &[]).unwrap();
        assert_eq!(plan.main_out(), None);
    }

    #[test]
    fn the_kind_follows_features_and_ports() {
        let f = |s: &[&str]| s.iter().map(|x| (*x).to_string()).collect::<Vec<_>>();
        assert_eq!(
            kind_hint(&f(&["instrument"]), true, true, false),
            PluginKindHint::Instrument
        );
        assert_eq!(
            kind_hint(&f(&["note-effect"]), true, false, true),
            PluginKindHint::NoteEffect
        );
        assert_eq!(
            kind_hint(&f(&["audio-effect"]), false, true, false),
            PluginKindHint::Effect
        );
        // An instrument feature without notes in is an effect.
        assert_eq!(
            kind_hint(&f(&["instrument"]), false, true, false),
            PluginKindHint::Effect
        );
    }
}
