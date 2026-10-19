//! The JSON shape of scan results and cache entries. Private: the public types are the plain
//! data of the SDK and the host; this module is the only place that knows the text format.

use crate::backend::ScannedPlugin;
use atomik_device_sdk::{
    Channels, PluginFormat, PluginKindHint, PluginLayout, PluginParamInfo, PluginRef,
};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub(crate) struct ParamDto {
    id: u32,
    name: String,
    #[serde(default)]
    module: String,
    min: f64,
    max: f64,
    default: f64,
    #[serde(default)]
    stepped: bool,
    #[serde(default)]
    steps: u32,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct PluginDto {
    id: String,
    name: String,
    vendor: String,
    version: String,
    kind: String,
    main_in: Option<String>,
    main_out: Option<String>,
    sidechain: bool,
    notes_in: bool,
    notes_out: bool,
    params: Vec<ParamDto>,
    #[serde(default)]
    unsupported: Option<String>,
    #[serde(default)]
    params_truncated: bool,
}

fn finite(v: f64, fallback: f64) -> f64 {
    if v.is_finite() { v } else { fallback }
}

fn channels(c: Option<Channels>) -> Option<String> {
    c.map(|c| match c {
        Channels::Mono => "mono".to_string(),
        Channels::Stereo => "stereo".to_string(),
    })
}

fn parse_channels(s: Option<&str>) -> Result<Option<Channels>, String> {
    match s {
        None => Ok(None),
        Some("mono") => Ok(Some(Channels::Mono)),
        Some("stereo") => Ok(Some(Channels::Stereo)),
        Some(other) => Err(format!("unknown channel layout `{other}`")),
    }
}

pub(crate) fn format_name(f: PluginFormat) -> String {
    f.name().to_string()
}

pub(crate) fn parse_format(s: &str) -> Result<PluginFormat, String> {
    match s {
        "clap" => Ok(PluginFormat::Clap),
        "vst3" => Ok(PluginFormat::Vst3),
        other => Err(format!("unknown plugin format `{other}`")),
    }
}

impl PluginDto {
    pub(crate) fn from_scanned(p: &ScannedPlugin) -> Self {
        let l = &p.layout;
        Self {
            id: p.plugin.id.clone(),
            name: l.name.clone(),
            vendor: l.vendor.clone(),
            version: l.version.clone(),
            kind: match l.kind_hint {
                PluginKindHint::Instrument => "instrument",
                PluginKindHint::Effect => "effect",
                PluginKindHint::NoteEffect => "note_effect",
            }
            .to_string(),
            main_in: channels(l.main_in),
            main_out: channels(l.main_out),
            sidechain: l.sidechain,
            notes_in: l.notes_in,
            notes_out: l.notes_out,
            params: l
                .params
                .iter()
                .map(|r| {
                    let min = finite(r.min, 0.0);
                    ParamDto {
                        id: r.id,
                        name: r.name.clone(),
                        module: r.module.clone(),
                        min,
                        max: finite(r.max, min + 1.0),
                        default: finite(r.default, min),
                        stepped: r.stepped,
                        steps: r.steps,
                    }
                })
                .collect(),
            unsupported: p.unsupported.clone(),
            params_truncated: p.params_truncated,
        }
    }

    pub(crate) fn into_scanned(self, format: PluginFormat) -> Result<ScannedPlugin, String> {
        let kind_hint = match self.kind.as_str() {
            "instrument" => PluginKindHint::Instrument,
            "effect" => PluginKindHint::Effect,
            "note_effect" => PluginKindHint::NoteEffect,
            other => return Err(format!("unknown plugin kind `{other}`")),
        };
        if self.id.is_empty() {
            return Err("a plugin has an empty id".to_string());
        }
        Ok(ScannedPlugin {
            plugin: PluginRef {
                format,
                id: self.id,
            },
            layout: PluginLayout {
                name: self.name,
                vendor: self.vendor,
                version: self.version,
                kind_hint,
                main_in: parse_channels(self.main_in.as_deref())?,
                main_out: parse_channels(self.main_out.as_deref())?,
                sidechain: self.sidechain,
                notes_in: self.notes_in,
                notes_out: self.notes_out,
                params: self
                    .params
                    .into_iter()
                    .map(|p| PluginParamInfo {
                        id: p.id,
                        name: p.name,
                        module: p.module,
                        min: p.min,
                        max: p.max,
                        default: p.default,
                        stepped: p.stepped,
                        steps: p.steps,
                    })
                    .collect(),
            },
            unsupported: self.unsupported,
            params_truncated: self.params_truncated,
        })
    }
}

#[derive(Serialize, Deserialize)]
pub(crate) struct ResultDto {
    pub(crate) path: String,
    pub(crate) format: String,
    pub(crate) plugins: Vec<PluginDto>,
    #[serde(default)]
    pub(crate) error: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct FileDto {
    pub(crate) path: String,
    pub(crate) format: String,
    pub(crate) size: u64,
    pub(crate) mtime_ns: i64,
    pub(crate) scanner_version: u32,
    pub(crate) plugins: Vec<PluginDto>,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct BlacklistDto {
    pub(crate) path: String,
    pub(crate) format: String,
    pub(crate) size: u64,
    pub(crate) mtime_ns: i64,
    pub(crate) reason: String,
    pub(crate) date: u64,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct CacheDto {
    pub(crate) version: u32,
    pub(crate) files: Vec<FileDto>,
    pub(crate) blacklist: Vec<BlacklistDto>,
}
