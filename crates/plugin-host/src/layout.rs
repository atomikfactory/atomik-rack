//! Helpers over the SDK's plugin descriptors: param keys, the sockets of a
//! descriptor, the cut of an over-long param table, and the check that a layout still fits the
//! descriptor a device was built from.

use atomik_device_sdk::{
    AUX_OUT_KEYS, CV_AMOUNT_KEYS, CV_DEST_KEYS, CV_IN_KEYS, DeviceDescriptor, MAX_AUX_OUTPUTS,
    MAX_PLUGIN_PARAMS, PLUGIN_CV_INPUTS, PluginLayout, SocketId,
};

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
/// `bypass`, the CV programmer's `cv.*` params excluded).
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
    /// The CV inputs `cv_in_1` to `cv_in_8`.
    pub cv_in: [Option<SocketId>; PLUGIN_CV_INPUTS],
    /// The aux output pairs `out_aux_<k>_l/r` the descriptor has.
    pub aux_out: [[Option<SocketId>; 2]; MAX_AUX_OUTPUTS],
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
            cv_in: std::array::from_fn(|k| id(CV_IN_KEYS[k])),
            aux_out: std::array::from_fn(|k| [id(AUX_OUT_KEYS[k][0]), id(AUX_OUT_KEYS[k][1])]),
        }
    }
}

/// Whether `descriptor` is what [`atomik_device_sdk::descriptor_for`] builds for `layout`: the
/// same sockets in the same order and the same params (by key, in order).
pub fn layout_matches(layout: &PluginLayout, descriptor: &DeviceDescriptor) -> bool {
    let mut want: Vec<&str> = Vec::with_capacity(8 + PLUGIN_CV_INPUTS);
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
    want.extend(CV_IN_KEYS);
    for pair in AUX_OUT_KEYS.iter().take(usize::from(layout.aux_out)) {
        want.extend(pair);
    }
    let sockets_ok = descriptor.sockets.iter().map(|s| s.key).eq(want);
    let n = layout.params.len();
    let params_ok = descriptor.params.len() == n + 1 + 2 * PLUGIN_CV_INPUTS
        && descriptor
            .params
            .iter()
            .skip(1)
            .zip(&layout.params)
            .all(|(d, p)| d.key == param_key(p.id))
        && (0..PLUGIN_CV_INPUTS).all(|k| {
            descriptor.params[1 + n + 2 * k].key == CV_DEST_KEYS[k]
                && descriptor.params[2 + n + 2 * k].key == CV_AMOUNT_KEYS[k]
        });
    sockets_ok && params_ok
}
