//! The rack panel of the console master: the send
//! and return rows carry the name of the effect on the slot, each return has an EDIT button
//! that selects that effect, the control-room source is a pick list, and the key switch shows
//! whether the `key_in` jack is cabled and where the key comes from.
//!
//! The panel itself is the generated layout of the descriptor (`send{k}.level` faders,
//! `return{k}.*` rows, `monitor.source` choice); this module adds the document-derived data
//! ([`master_data`]), the geometry of what is drawn on top ([`edit_rects`], [`key_lamp`]) and
//! the painters, so the rack view, the painter and the tests share one set of rectangles.

use crate::autolayout::PanelSpec;
use crate::canvas::{Canvas, CanvasExt, pt};
use crate::paint::{edit_button, fit_label};
use crate::theme::Theme;
use atomik_core::ParamDescriptor;
use atomik_device_sdk::{ControlKind, DeviceDescriptor};
use atomik_model::{DeviceKey, FX_SLOTS, RackDoc};
use vello::kurbo::{Circle, Rect};

/// Type id of the console master.
pub const MASTER_TYPE: &str = crate::console_layout::MASTER_TYPE;
/// Socket key of the master's sidechain input.
pub const KEY_IN: &str = "key_in";
/// Param key of the key switch.
pub const KEY_PARAM: &str = "comp.key";
/// Choice params with at least this many entries open a pick list instead of cycling.
pub const PICK_LIST_MIN: usize = 16;
/// Label of an unused slot.
pub const FREE_LABEL: &str = "free";

/// Whether a click on `p` opens a pick list (a long choice such as the control-room source).
pub fn is_pick_list(p: &ParamDescriptor) -> bool {
    matches!(p.unit, atomik_core::Unit::Choice(names) if names.len() >= PICK_LIST_MIN)
}

/// What the master panel shows that the descriptor does not hold.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MasterData {
    /// The effect on each return slot: its device key and display name.
    pub returns: Vec<Option<(DeviceKey, String)>>,
    /// Whether a cable ends at `key_in`.
    pub key_cabled: bool,
    /// The display name of what feeds `key_in`.
    pub key_from: Option<String>,
}

/// What the painter reads.
#[derive(Debug, Clone, Copy, Default)]
pub struct MasterInfo<'a> {
    /// The effect on each return slot (`None` for a free slot or no data).
    pub returns: &'a [Option<(DeviceKey, String)>],
    /// Whether a cable ends at `key_in`.
    pub key_cabled: bool,
    /// The name of what feeds `key_in`.
    pub key_from: Option<&'a str>,
}

impl MasterData {
    /// The painter's view of this data.
    pub fn info(&self) -> MasterInfo<'_> {
        MasterInfo {
            returns: &self.returns,
            key_cabled: self.key_cabled,
            key_from: self.key_from.as_deref(),
        }
    }
}

/// Reads the master panel data from `doc`: `console.returns` resolved to device names, and the
/// cable into `key_in` of the console master.
pub fn master_data(doc: &RackDoc) -> MasterData {
    let name_of = |key: &DeviceKey| {
        doc.devices
            .iter()
            .find(|d| d.key == *key)
            .map_or_else(|| key.0.clone(), |d| d.name.clone())
    };
    let returns = doc
        .console
        .returns
        .iter()
        .map(|slot| slot.as_ref().map(|k| (k.clone(), name_of(k))))
        .collect();
    let feeder = doc.console.master.as_ref().and_then(|m| {
        doc.cables
            .iter()
            .find(|c| c.to.device == *m && c.to.socket == KEY_IN)
    });
    let key_from = feeder.map(|c| {
        if c.from.device.0 == atomik_model::INTERFACE_KEY {
            format!("Audio I/O {}", c.from.socket.trim_start_matches("in_"))
        } else {
            name_of(&c.from.device)
        }
    });
    MasterData {
        returns,
        key_cabled: feeder.is_some(),
        key_from,
    }
}

/// The 0-based slot of a `return{n}.*` or `send{n}.level` param key.
pub fn slot_of_param(key: &str) -> Option<usize> {
    let rest = key
        .strip_prefix("return")
        .or_else(|| key.strip_prefix("send"))?;
    let n: usize = rest.split('.').next()?.parse().ok()?;
    (1..=FX_SLOTS).contains(&n).then(|| n - 1)
}

/// The label under a send fader: the name of the effect on its slot, `None` (the generic
/// "Level") for a free slot or another param.
pub fn send_label(info: &MasterInfo<'_>, key: &str) -> Option<String> {
    if !(key.starts_with("send") && key.ends_with(".level")) {
        return None;
    }
    let slot = slot_of_param(key)?;
    info.returns
        .get(slot)?
        .as_ref()
        .map(|(_, name)| name.clone())
}

/// The name shown after a section title (`RETURN 3`): the effect on the slot.
pub fn section_suffix(info: &MasterInfo<'_>, title: &str) -> Option<String> {
    let n: usize = title.strip_prefix("RETURN ")?.trim().parse().ok()?;
    let slot = info.returns.get(n.checked_sub(1)?)?;
    Some(
        slot.as_ref()
            .map_or_else(|| FREE_LABEL.to_string(), |(_, name)| name.clone()),
    )
}

fn placed<'a>(
    spec: &'a PanelSpec,
    desc: &DeviceDescriptor,
    key: &str,
) -> Option<&'a crate::autolayout::PlacedControl> {
    spec.front
        .iter()
        .find(|pc| desc.params[pc.param].key == key)
}

/// The EDIT button of each return: below the mute switch of the row, 30 px wide, as
/// `(slot, rectangle)` in panel coordinates. Slots whose mute switch is not placed are skipped.
pub fn edit_rects(spec: &PanelSpec, desc: &DeviceDescriptor) -> Vec<(usize, Rect)> {
    (0..FX_SLOTS)
        .filter_map(|slot| {
            let pc = placed(spec, desc, &format!("return{}.mute", slot + 1))?;
            (pc.kind == ControlKind::Toggle).then(|| {
                let cx = pc.rect.center().x;
                (
                    slot,
                    Rect::new(cx - 15.0, pc.rect.y0 + 31.0, cx + 15.0, pc.rect.y0 + 44.0),
                )
            })
        })
        .collect()
}

/// The lamp left of the key switch: lit when a cable ends at `key_in`.
pub fn key_lamp(spec: &PanelSpec, desc: &DeviceDescriptor) -> Option<Circle> {
    let pc = placed(spec, desc, KEY_PARAM)?;
    Some(Circle::new(pt(pc.rect.x0 + 3.0, pc.rect.y0 + 8.5), 2.5))
}

/// Where the "key from" name is written: under the key switch label.
fn key_text_pos(spec: &PanelSpec, desc: &DeviceDescriptor) -> Option<(f64, f64, f64)> {
    let pc = placed(spec, desc, KEY_PARAM)?;
    Some((pc.rect.center().x, pc.rect.y0 + 36.0, pc.rect.width() + 8.0))
}

/// Paints what the master adds to the generated panel: the EDIT buttons, the key lamp and the
/// name of what feeds the key.
pub fn paint_front<C: Canvas + ?Sized>(
    c: &mut C,
    theme: &Theme,
    spec: &PanelSpec,
    desc: &DeviceDescriptor,
    info: &MasterInfo<'_>,
) {
    for (slot, r) in edit_rects(spec, desc) {
        let used = info.returns.get(slot).is_some_and(Option::is_some);
        edit_button(c, theme, r, "EDIT", used);
    }
    if let Some(lamp) = key_lamp(spec, desc) {
        if info.key_cabled {
            c.fill_with(
                Circle::new(lamp.center, lamp.radius + 3.0),
                theme.io_connected.with_alpha(0.25),
            );
            c.fill_with(lamp, theme.io_connected);
        } else {
            c.fill_with(lamp, theme.led_off);
        }
    }
    if let (Some(from), Some((x, y, w))) = (info.key_from, key_text_pos(spec, desc)) {
        let text = fit_label(&format!("from {from}"), w, theme.label_size - 1.0);
        c.text(
            &crate::canvas::TextRun::new(text, pt(x, y), theme.label_size - 1.0, theme.text_muted)
                .align(crate::canvas::Align::Center),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atomik_model::{Cable, RackDevice, SocketRef, empty_doc};

    fn dev(key: &str, name: &str) -> RackDevice {
        RackDevice {
            key: DeviceKey(key.into()),
            type_id: "x".into(),
            name: name.into(),
            params: Default::default(),
            folded: false,
            color: None,
            zones: Vec::new(),
            steps: None,
            inner: None,
            plugin: None,
        }
    }

    fn sref(d: &str, s: &str) -> SocketRef {
        SocketRef {
            device: DeviceKey(d.into()),
            socket: s.into(),
        }
    }

    #[test]
    fn the_return_labels_come_from_the_console_returns() {
        let mut d = empty_doc();
        d.devices.push(dev("verb", "Reverb"));
        d.devices.push(dev("echo", "Delay 2"));
        d.console.returns[0] = Some(DeviceKey("verb".into()));
        d.console.returns[2] = Some(DeviceKey("echo".into()));
        let data = master_data(&d);
        assert_eq!(data.returns.len(), FX_SLOTS);
        let info = data.info();
        assert_eq!(section_suffix(&info, "RETURN 1").as_deref(), Some("Reverb"));
        assert_eq!(section_suffix(&info, "RETURN 2").as_deref(), Some("free"));
        assert_eq!(
            section_suffix(&info, "RETURN 3").as_deref(),
            Some("Delay 2")
        );
        assert_eq!(section_suffix(&info, "RETURN 9"), None);
        assert_eq!(section_suffix(&info, "SEND 1"), None);
        assert_eq!(send_label(&info, "send3.level").as_deref(), Some("Delay 2"));
        assert_eq!(send_label(&info, "send2.level"), None);
        assert_eq!(send_label(&info, "return1.level"), None);
    }

    #[test]
    fn a_dangling_return_shows_its_key() {
        let mut d = empty_doc();
        d.console.returns[0] = Some(DeviceKey("gone".into()));
        let data = master_data(&d);
        assert_eq!(data.returns[0].as_ref().unwrap().1, "gone");
    }

    #[test]
    fn the_key_state_follows_the_cable() {
        let mut d = empty_doc();
        d.devices.push(dev("master", "Master"));
        d.devices.push(dev("drums", "Drums"));
        d.console.master = Some(DeviceKey("master".into()));
        assert!(!master_data(&d).key_cabled);
        assert_eq!(master_data(&d).key_from, None);
        d.cables.push(Cable {
            from: sref("drums", "out_l"),
            to: sref("master", "key_in"),
        });
        let data = master_data(&d);
        assert!(data.key_cabled);
        assert_eq!(data.key_from.as_deref(), Some("Drums"));
        d.cables[0].from = sref("interface", "in_3");
        assert_eq!(
            master_data(&d).key_from.as_deref(),
            Some("Audio I/O 3"),
            "the interface is called by its one name"
        );
        // A key cable of another device does not count.
        d.cables[0].to = sref("drums", "key_in");
        assert!(!master_data(&d).key_cabled);
    }

    #[test]
    fn slots_come_from_param_keys() {
        assert_eq!(slot_of_param("return1.level"), Some(0));
        assert_eq!(slot_of_param("send8.level"), Some(7));
        assert_eq!(slot_of_param("send9.level"), None);
        assert_eq!(slot_of_param("fader"), None);
    }
}
