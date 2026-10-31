//! Copying a strip's settings onto other strips, resetting a strip and clearing its inserts.
//!
//! A [`ChannelSettingsClip`] holds what a copy took from one channel or bus: the params of the
//! sections that were copied (the same sections a mixer snapshot recalls, see
//! [`SnapshotScope`]) and, when the inserts are one of them, the effects of the insert chain
//! with their params. It is plain data: the app keeps it on its clipboard, nothing is saved in
//! the project.
//!
//! Pasting sets every param of the clip's sections on the target, a param the clip does not
//! hold going to its default, and rebuilds the chain from new effect devices; the routing, the
//! name, the source and the VCA membership are never touched. Everything is planned on a
//! scratch document, so a refusal changes nothing, and is one undo step.

use crate::console::{ConsoleCommand, InsertEffect, OutputDest, is_console_type};
use crate::console_cmd::{Planner, bad};
use crate::console_insert::{insert_chain, stereo_effect_type};
use crate::console_snapshot::{SnapshotScope, kept_params};
use crate::history::Edit;
use crate::{DeviceKey, ModelError, RackDoc, rules};
use atomik_core::ParamDescriptor;
use atomik_device_sdk::{DeviceKind, Registry};
use std::collections::BTreeMap;

/// One effect of a copied insert chain.
#[derive(Debug, Clone, PartialEq)]
pub struct InsertClip {
    /// The effect's registry type id.
    pub type_id: String,
    /// Its params that are not at their default.
    pub params: BTreeMap<String, f32>,
}

/// The settings copied from a channel or a bus (see the module documentation).
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelSettingsClip {
    /// The sections the copy took.
    pub sections: SnapshotScope,
    /// The strip's params of those sections that are not at their default.
    pub params: BTreeMap<String, f32>,
    /// The insert chain, in signal order (empty when the inserts were not copied).
    pub inserts: Vec<InsertClip>,
}

impl ChannelSettingsClip {
    /// The same settings restricted to `sections` (what a copy of one section keeps).
    #[must_use]
    pub fn restricted(&self, sections: SnapshotScope) -> Self {
        let keep = |p: &str| sections.covers_param(p) && self.sections.covers_param(p);
        Self {
            sections,
            params: self
                .params
                .iter()
                .filter(|(k, _)| keep(k))
                .map(|(k, v)| (k.clone(), *v))
                .collect(),
            inserts: if sections.covers_inserts() {
                self.inserts.clone()
            } else {
                Vec::new()
            },
        }
    }
}

/// `strip` must be a console channel or an output bus.
fn check_strip(doc: &RackDoc, strip: &DeviceKey) -> Result<(), ModelError> {
    if doc.console.channel_index(strip).is_none() && doc.console.bus_index(strip).is_none() {
        return Err(bad(format!("{strip} is not a console channel or bus")));
    }
    Ok(())
}

impl RackDoc {
    /// Copies the settings of `strip` (a channel or a bus) in `sections`.
    ///
    /// # Errors
    /// `strip` is no channel or bus, or its type is unknown.
    pub fn copy_channel_settings(
        &self,
        registry: &Registry,
        strip: &DeviceKey,
        sections: SnapshotScope,
    ) -> Result<ChannelSettingsClip, ModelError> {
        check_strip(self, strip)?;
        let device = rules::device(self, strip)?;
        let desc = rules::descriptor(registry, &device.type_id)?;
        let params = kept_params(desc, true)
            .filter(|p| sections.covers_param(p.key))
            .filter_map(|p| {
                let v = p.clamp(device.params.get(p.key).copied().unwrap_or(p.default));
                (v.to_bits() != p.default.to_bits()).then(|| (p.key.to_owned(), v))
            })
            .collect();
        let mut inserts = Vec::new();
        if sections.covers_inserts() {
            for key in insert_chain(self, registry, strip) {
                let Some(effect) = self.device(&key) else {
                    continue;
                };
                let Some(f) = registry.get(&effect.type_id) else {
                    continue;
                };
                // Plugins and containers hold more than params: they are not copied.
                if f.descriptor.kind != DeviceKind::Effect
                    || atomik_device_sdk::is_plugin_type_id(&effect.type_id)
                {
                    continue;
                }
                let params = f
                    .descriptor
                    .params
                    .iter()
                    .filter_map(|p| {
                        let v = p.clamp(effect.params.get(p.key).copied().unwrap_or(p.default));
                        (v.to_bits() != p.default.to_bits()).then(|| (p.key.to_owned(), v))
                    })
                    .collect();
                inserts.push(InsertClip {
                    type_id: effect.type_id.clone(),
                    params,
                });
            }
        }
        Ok(ChannelSettingsClip {
            sections,
            params,
            inserts,
        })
    }
}

/// Sets `param` of `key` to `target` (clamped) with a param edit; nothing when it is there.
fn set_param(p: &mut Planner<'_>, key: &DeviceKey, param: &ParamDescriptor, target: f32) {
    let Some(device) = p.doc().device(key) else {
        return;
    };
    let target = param.clamp(target);
    let old = device.params.get(param.key).copied();
    let now = param.clamp(old.unwrap_or(param.default));
    if now.to_bits() == target.to_bits() {
        return;
    }
    let new = (target.to_bits() != param.default.to_bits()).then_some(target);
    p.push(Edit::Param {
        key: key.clone(),
        param: param.key.to_owned(),
        old,
        new,
        default: param.default,
    });
}

/// Empties the insert chain of `strip` (the effects stay in the rack).
fn clear_chain(p: &mut Planner<'_>, strip: &DeviceKey) -> Result<(), ModelError> {
    while !insert_chain(p.doc(), p.registry, strip).is_empty() {
        crate::console_insert::set_insert(p, strip, 0, None)?;
    }
    Ok(())
}

/// Sets the strip's params in `sections` from `values` (a missing one goes to its default).
fn set_strip_params(
    p: &mut Planner<'_>,
    strip: &DeviceKey,
    sections: SnapshotScope,
    values: &BTreeMap<String, f32>,
) -> Result<(), ModelError> {
    let desc = rules::descriptor(p.registry, p.type_of(strip)?)?;
    for param in kept_params(desc, true).filter(|q| sections.covers_param(q.key)) {
        let target = values.get(param.key).copied().unwrap_or(param.default);
        set_param(p, strip, param, target);
    }
    Ok(())
}

fn paste_inserts(
    p: &mut Planner<'_>,
    strip: &DeviceKey,
    inserts: &[InsertClip],
) -> Result<(), ModelError> {
    clear_chain(p, strip)?;
    for (slot, insert) in inserts.iter().enumerate() {
        let desc = rules::descriptor(p.registry, &insert.type_id)?;
        if !stereo_effect_type(desc) {
            return Err(bad(format!("{} is not a stereo effect", insert.type_id)));
        }
        crate::console_insert::set_insert(
            p,
            strip,
            slot,
            Some(InsertEffect::New {
                type_id: insert.type_id.clone(),
            }),
        )?;
        let chain = insert_chain(p.doc(), p.registry, strip);
        let Some(key) = chain.get(slot).cloned() else {
            return Err(bad("the pasted insert did not join the chain"));
        };
        for param in desc.params {
            if let Some(v) = insert.params.get(param.key) {
                set_param(p, &key, param, *v);
            }
        }
    }
    Ok(())
}

/// Plans the commands of this module.
pub(crate) fn plan(p: &mut Planner<'_>, command: ConsoleCommand) -> Result<(), ModelError> {
    match command {
        ConsoleCommand::PasteSettings { strips, clip } => {
            if strips.is_empty() {
                return Err(bad("there is no strip to paste the settings onto"));
            }
            for strip in &strips {
                check_strip(p.doc(), strip)?;
                if !is_console_type(p.type_of(strip)?) {
                    return Err(bad(format!("{strip} is not a console strip")));
                }
                set_strip_params(p, strip, clip.sections, &clip.params)?;
                if clip.sections.covers_inserts() {
                    paste_inserts(p, strip, &clip.inserts)?;
                }
            }
        }
        ConsoleCommand::ResetStrip {
            strip,
            keep_routing,
        } => {
            check_strip(p.doc(), &strip)?;
            set_strip_params(p, &strip, SnapshotScope::ALL, &BTreeMap::new())?;
            clear_chain(p, &strip)?;
            if !keep_routing {
                crate::console_bus::set_output(p, &strip, &OutputDest::Master)?;
            }
        }
        ConsoleCommand::ClearInserts { strip } => {
            check_strip(p.doc(), &strip)?;
            clear_chain(p, &strip)?;
        }
        _ => unreachable!("not a settings command"),
    }
    Ok(())
}
