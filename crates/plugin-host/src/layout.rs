//! Helpers over the SDK's plugin descriptors: param keys, the sockets of a
//! descriptor, the cut of an over-long param table, and the check that a layout still fits the
//! descriptor a device was built from.

use atomik_device_sdk::{DeviceDescriptor, MAX_PLUGIN_PARAMS, PluginLayout, SocketId};

/// The key of a plugin param: `p` and the plugin's own 32-bit id in decimal.
pub fn param_key(id: u32) -> String {
    format!("p{id}")
}

/// The plugin param id behind a key such as `p42`; `None` for `bypass` and anything else.
pub fn parse_param_key(key: &str) -> Option<u32> {
    let digits = key.strip_prefix('p')?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// The plugin param ids of a plugin descriptor, in descriptor order (every param after
/// `bypass`).
pub fn param_ids(descriptor: &DeviceDescriptor) -> Vec<u32> {
    descriptor
        .params
        .iter()
        .skip(1)
        .filter_map(|p| parse_param_key(p.key))
        .collect()
}

/// Cuts a layout's param table to [`MAX_PLUGIN_PARAMS`], keeping the first ones. Returns true
/// when something was cut; the scan result records it as `params_truncated`.
pub fn truncate_params(layout: &mut PluginLayout) -> bool {
    if layout.params.len() > MAX_PLUGIN_PARAMS {
        layout.params.truncate(MAX_PLUGIN_PARAMS);
        true
    } else {
        false
    }
}

/// The socket ids of a plugin descriptor by their fixed names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PluginSockets {
    /// Main input left.
    pub in_l: Option<SocketId>,
    /// Main input right.
    pub in_r: Option<SocketId>,
    /// Sidechain left.
    pub sc_l: Option<SocketId>,
    /// Sidechain right.
    pub sc_r: Option<SocketId>,
    /// Notes in.
    pub notes_in: Option<SocketId>,
    /// Main output left.
    pub out_l: Option<SocketId>,
    /// Main output right.
    pub out_r: Option<SocketId>,
    /// Notes out.
    pub notes_out: Option<SocketId>,
}

impl PluginSockets {
    /// Looks the sockets up in `descriptor`.
    pub fn of(descriptor: &DeviceDescriptor) -> Self {
        let id = |key: &str| descriptor.socket_by_key(key).map(|s| s.id);
        Self {
            in_l: id("in_l"),
            in_r: id("in_r"),
            sc_l: id("sc_in_l"),
            sc_r: id("sc_in_r"),
            notes_in: id("notes_in"),
            out_l: id("out_l"),
            out_r: id("out_r"),
            notes_out: id("notes_out"),
        }
    }
}

/// Whether `descriptor` is what [`atomik_device_sdk::descriptor_for`] builds for `layout`: the
/// same sockets in the same order and the same params (by key, in order).
pub fn layout_matches(layout: &PluginLayout, descriptor: &DeviceDescriptor) -> bool {
    let mut want: Vec<&str> = Vec::with_capacity(8);
    if layout.main_in.is_some() {
        want.extend(["in_l", "in_r"]);
    }
    if layout.sidechain {
        want.extend(["sc_in_l", "sc_in_r"]);
    }
    if layout.notes_in {
        want.push("notes_in");
    }
    if layout.main_out.is_some() {
        want.extend(["out_l", "out_r"]);
    }
    if layout.notes_out {
        want.push("notes_out");
    }
    let sockets_ok = descriptor.sockets.iter().map(|s| s.key).eq(want);
    let params_ok = descriptor.params.len() == layout.params.len() + 1
        && descriptor
            .params
            .iter()
            .skip(1)
            .zip(&layout.params)
            .all(|(d, p)| d.key == param_key(p.id));
    sockets_ok && params_ok
}
