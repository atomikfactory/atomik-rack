//! The MIDI control tools of the headless session: send MIDI messages to the control path,
//! learn and edit mappings, and play a control surface with an in-memory port.
//!
//! * `session_midi_input` hands note, controller and pitch bend messages to the mappings as the
//!   live MIDI input does (the session has no MIDI port), one frame each, at a clock the session
//!   keeps, so soft takeover, gestures and learning behave as with hardware.
//! * `session_midi_learn` arms and forgets targets, switches MIDI Learn mode on, edits the
//!   options of a mapping and exports or imports a preset file.
//! * `session_surface` sets up the control surface driver on an in-memory port, plays the
//!   surface's messages (a fader, a key, a SysEx) and returns what the host sent back.

use crate::rpc::{Server, Tool};
use crate::session::Session;
use crate::tool::{Args, ToolOutput, definition};
use atomik_app::surface::LoopbackSurface;
use atomik_model::{
    MapMode, MidiMapCommand, MidiMapping, MidiTarget, RelativeKind, TransportTarget,
};
use serde_json::{Value, json};

/// The MIDI control tools.
pub const TOOLS: &[Tool] = &[
    Tool {
        name: "session_midi_input",
        definition: input_def,
        run: input,
    },
    Tool {
        name: "session_midi_learn",
        definition: learn_def,
        run: learn,
    },
    Tool {
        name: "session_surface",
        definition: surface_def,
        run: surface,
    },
];

/// Seconds between two messages of one `session_midi_input` call.
const DEFAULT_INTERVAL: f64 = 0.02;
/// Seconds between the frames of the surface (the feedback is throttled to 1/30 s).
const SURFACE_STEP: f64 = 0.1;

/// The state kept by the MIDI tools for one session.
#[derive(Default)]
pub struct MidiTools {
    /// The monotonic time of the control path, in seconds.
    pub clock: f64,
    /// The in-memory port of the control surface, while one is attached.
    pub surface: Option<LoopbackSurface>,
}

/// Parses `F0 00 ...`: hex bytes separated by spaces.
fn hex_bytes(text: &str) -> Result<Vec<u8>, String> {
    text.split_whitespace()
        .map(|t| {
            let t = t.trim_start_matches("0x");
            u8::from_str_radix(t, 16).map_err(|_| format!("`{t}` is not a hex byte"))
        })
        .collect()
}

/// Parses messages separated by `;` or newlines, each a list of hex bytes.
fn messages(text: &str) -> Result<Vec<Vec<u8>>, String> {
    let out: Result<Vec<Vec<u8>>, String> = text
        .split([';', '\n'])
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(hex_bytes)
        .collect();
    let out = out?;
    if out.is_empty() {
        return Err("`messages` holds no message".into());
    }
    Ok(out)
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The learn and mapping state, for every result of these tools.
fn midi_state(session: &Session) -> Value {
    let core = session.shell().core();
    json!({
        "learn_mode": core.midi_learn_mode(),
        "armed": core.midi_armed().map(|t| core.target_text(t)),
        "received": core.midi_received(),
        "last": core.midi_last_message(),
        "mappings": core.doc().midi_maps.len(),
        "message": core.last_rejection(),
        "can_undo": core.document().can_undo(),
    })
}

/// The `midi` block of `session_status`.
pub fn status_json(session: &Session) -> Value {
    let mut v = midi_state(session);
    let core = session.shell().core();
    v["surface"] = match core.surface() {
        Some(s) => json!({
            "driver": s.driver_name(),
            "link": s.describe(),
            "sent": s.sent(),
            "received": s.received(),
        }),
        None => Value::Null,
    };
    v
}

// ------------------------------------------------------------ session_midi_input

fn input_def() -> Value {
    definition(
        "session_midi_input",
        "Sends MIDI messages to the control path of the session, as the live MIDI input would (a headless session has no port): the project's mappings see them, MIDI Learn learns from them. `messages` is hex, one message per `;` (`B0 07 7F; 90 3C 64`): a note on or off, a controller or a pitch bend. Each message runs one frame, `interval` seconds after the last; `wait` seconds then pass, which ends a held move (a mapped control is let go after half a second without a message). Returns the learn state, the last message, the number of mappings, whether the document changed, and the last message the app showed.",
        &[
            (
                "messages",
                "string",
                "Hex messages separated by `;`, e.g. `B0 07 40; 90 3C 64`.",
            ),
            (
                "port",
                "string",
                "The input port name the messages come from (default `mcp`); a mapping may be restricted to one.",
            ),
            (
                "interval",
                "number",
                "Seconds between messages (default 0.02).",
            ),
            (
                "wait",
                "number",
                "Seconds to let pass after the last message (default 0).",
            ),
        ],
        &["messages"],
    )
}

fn input(server: &mut Server, args: &Args) -> Result<ToolOutput, String> {
    let list = messages(args.str("messages")?)?;
    let port = args.opt_str("port")?.unwrap_or("mcp").to_owned();
    let interval = args
        .opt_f64("interval")?
        .unwrap_or(DEFAULT_INTERVAL)
        .max(0.0);
    let wait = args.opt_f64("wait")?.unwrap_or(0.0).max(0.0);
    let session = server.require_session()?;
    let before = session.shell().core().doc().clone();
    for bytes in &list {
        session
            .shell()
            .core()
            .inject_midi_bytes(&port, bytes)
            .map_err(|e| e.to_string())?;
        session.midi.clock += interval;
        let t = session.midi.clock;
        session.shell_mut().core_mut().midi_control_frame(t);
    }
    if wait > 0.0 {
        session.midi.clock += wait;
        let t = session.midi.clock;
        session.shell_mut().core_mut().midi_control_frame(t);
    }
    session.shell_mut().frame();
    session.shell_mut().poll_status();
    let mut v = midi_state(session);
    v["handled"] = json!(list.len());
    v["document_changed"] = json!(session.shell().core().doc() != &before);
    Ok(ToolOutput::json(&v))
}

// ------------------------------------------------------------ session_midi_learn

fn learn_def() -> Value {
    definition(
        "session_midi_learn",
        "MIDI learn and mappings in the session. `action`: `mode` (`on` true or false: MIDI Learn mode, the same as the Mixer menu item), `arm` (a `target`: the next control moved on `session_midi_input` becomes its source; a controller below 64 waits a tenth of a second for a 14 bit pair, so follow it with a `wait` or a second message), `cancel` (drop the armed target), `forget` (every mapping of a `target`), `edit` (mapping `id` with any of `min`, `max`, `invert`, `mode`, `step`, `pickup`, `consume`), `remove` (mapping `id`), `clear`, `export` (a preset `file`, by device type and position), `import` (a preset `file`, applied to this project's devices). A `target` is `<device key> <param key>` (`channel fader`), `vca <id>`, `tempo`, `selected <param key>` (the selected strip) or `transport <play|stop|toggle_play|record|loop|return_to_zero>`. A `mode` is absolute, relative_twos, relative_offset, relative_sign, toggle or momentary. Returns the learn state and the mappings.",
        &[
            (
                "action",
                "string",
                "mode, arm, cancel, forget, edit, remove, clear, export or import.",
            ),
            ("target", "string", "arm, forget: the target, see above."),
            (
                "on",
                "boolean",
                "mode: switch MIDI Learn mode on (true) or off.",
            ),
            (
                "id",
                "number",
                "edit, remove: the mapping id (`session_status` lists them under `midi_maps`).",
            ),
            (
                "min",
                "number",
                "edit: the value the control's bottom gives, 0 to 1 of the target's range.",
            ),
            (
                "max",
                "number",
                "edit: the value the control's top gives, 0 to 1.",
            ),
            ("invert", "boolean", "edit: swap the ends."),
            (
                "mode",
                "string",
                "edit: absolute, relative_twos, relative_offset, relative_sign, toggle or momentary.",
            ),
            (
                "step",
                "number",
                "edit: a relative detent as a fraction of the range, above 0 and at most 1.",
            ),
            (
                "pickup",
                "boolean",
                "edit: soft takeover for an absolute control.",
            ),
            (
                "consume",
                "boolean",
                "edit: keep the message away from the notes of the armed instrument.",
            ),
            ("file", "string", "export, import: the preset file."),
        ],
        &["action"],
    )
}

/// Parses a target as the tool documents it.
fn target(text: &str) -> Result<MidiTarget, String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    match words.as_slice() {
        ["tempo"] => Ok(MidiTarget::Tempo),
        ["vca", id] => id
            .parse()
            .map(|id| MidiTarget::Vca { id })
            .map_err(|_| format!("`{id}` is not a VCA group id")),
        ["selected", param] => Ok(MidiTarget::SelectedStrip {
            param: (*param).to_owned(),
        }),
        ["transport", action] => TransportTarget::from_id(action)
            .map(MidiTarget::Transport)
            .ok_or_else(|| format!("`{action}` is not a transport action")),
        [device, param] => Ok(MidiTarget::Param {
            device: atomik_model::DeviceKey((*device).to_owned()),
            param: (*param).to_owned(),
        }),
        _ => Err(format!(
            "`{text}` is not a target: use `<device> <param>`, `vca <id>`, `tempo`, `selected <param>` or `transport <action>`"
        )),
    }
}

fn mapping_json(
    core: &atomik_app::AppCore<impl atomik_app::sink::EngineSink>,
    m: &MidiMapping,
) -> Value {
    let o = &m.options;
    json!({
        "id": m.id,
        "source": m.source.describe(),
        "target": core.target_text(&m.target),
        "min": o.min, "max": o.max, "invert": o.invert, "mode": o.mode.id(),
        "step": o.sensitivity, "pickup": o.pickup, "consume": o.consume,
    })
}

fn learn(server: &mut Server, args: &Args) -> Result<ToolOutput, String> {
    let action = args.str("action")?;
    let file = args.opt_str("file")?;
    let path = match (action, file) {
        ("export", Some(f)) => Some(server.paths().write(f)?),
        ("import", Some(f)) => Some(server.paths().read(f)?),
        ("export" | "import", None) => return Err("`file` is needed".into()),
        _ => None,
    };
    let session = server.require_session()?;
    let core = session.shell_mut().core_mut();
    let done = match action {
        "mode" => {
            core.set_midi_learn(args.opt_bool("on")?.unwrap_or(true));
            true
        }
        "arm" => core.midi_arm(target(args.str("target")?)?),
        "cancel" => {
            core.midi_cancel_arm();
            true
        }
        "forget" => core.midi_forget(target(args.str("target")?)?),
        "remove" => core.remove_midi_map(id(args)?),
        "clear" => core.clear_midi_maps(),
        "export" => core.export_midi_preset(path.as_deref().ok_or("`file` is needed")?, &[]),
        "import" => core.import_midi_preset(path.as_deref().ok_or("`file` is needed")?),
        "edit" => {
            let mut m = core
                .doc()
                .midi_map(id(args)?)
                .cloned()
                .ok_or("no such mapping")?;
            let o = &mut m.options;
            if let Some(x) = args.opt_f64("min")? {
                o.min = x as f32;
            }
            if let Some(x) = args.opt_f64("max")? {
                o.max = x as f32;
            }
            if let Some(x) = args.opt_bool("invert")? {
                o.invert = x;
            }
            if let Some(x) = args.opt_str("mode")? {
                o.mode = MapMode::from_id(x).ok_or_else(|| {
                    format!("`{x}` is not a mode (absolute, {}, relative_offset, relative_sign, toggle, momentary)", RelativeKind::TwosComplement.id())
                })?;
            }
            if let Some(x) = args.opt_f64("step")? {
                o.sensitivity = x as f32;
            }
            if let Some(x) = args.opt_bool("pickup")? {
                o.pickup = x;
            }
            if let Some(x) = args.opt_bool("consume")? {
                o.consume = x;
            }
            core.apply_command(atomik_model::Command::MidiMap(MidiMapCommand::Set(m)))
                .map(|()| true)
                .map_err(|e| e.to_string())?
        }
        other => {
            return Err(format!(
                "action {other:?} is not mode, arm, cancel, forget, edit, remove, clear, export or import"
            ));
        }
    };
    if !done {
        return Err(core
            .last_rejection()
            .unwrap_or("the action was refused")
            .to_owned());
    }
    session.shell_mut().frame();
    session.shell_mut().poll_status();
    let core = session.shell().core();
    let mut v = midi_state(session);
    v["midi_maps"] = Value::Array(
        core.doc()
            .midi_maps
            .iter()
            .map(|m| mapping_json(core, m))
            .collect(),
    );
    Ok(ToolOutput::json(&v))
}

fn id(args: &Args) -> Result<u32, String> {
    let n = args
        .opt_u64("id")?
        .ok_or_else(|| "missing argument `id`".to_owned())?;
    u32::try_from(n).map_err(|_| "`id` is too large".to_owned())
}

// --------------------------------------------------------------- session_surface

fn surface_def() -> Value {
    definition(
        "session_surface",
        "Plays a control surface (the eight-fader protocol) on an in-memory port, so a headless session can test the surface driver. `action`: `attach` (the driver `mcu`; returns what the host sends first: the device query, then the faders, LEDs, rings, scribble strips and time display for the console), `send` (`messages`: the surface's messages as hex separated by `;`: a fader is pitch bend `E0 <lsb> <msb>` on channel 0 to 7 and 8 for the master, a key is a note on `90 <note> 7F`, a V-pot is `B0 10 <value>`, a SysEx is `F0 ... F7`; the host handles them and answers), `frame` (just run a frame, to see the feedback for a change made by another tool) and `detach` (the host lets the surface go offline). Returns the messages the host sent to the surface as hex (`sent`), and the surface counters.",
        &[
            ("action", "string", "attach, send, frame or detach."),
            ("driver", "string", "attach: the driver id (default `mcu`)."),
            ("messages", "string", "send: hex messages separated by `;`."),
        ],
        &["action"],
    )
}

fn surface(server: &mut Server, args: &Args) -> Result<ToolOutput, String> {
    let action = args.str("action")?;
    let session = server.require_session()?;
    match action {
        "attach" => {
            let driver = args.opt_str("driver")?.unwrap_or("mcu").to_owned();
            let port = session
                .shell_mut()
                .core_mut()
                .attach_loopback_surface(&driver)?;
            session.midi.surface = Some(port);
        }
        "send" => {
            let list = messages(args.str("messages")?)?;
            let port = session
                .midi
                .surface
                .clone()
                .ok_or("no surface is attached: use action attach")?;
            for m in &list {
                port.feed(m);
            }
        }
        "frame" => {
            if session.midi.surface.is_none() {
                return Err("no surface is attached: use action attach".into());
            }
        }
        "detach" => {
            session.shell_mut().core_mut().set_surface(None);
        }
        other => {
            return Err(format!(
                "action {other:?} is not attach, send, frame or detach"
            ));
        }
    }
    // Two frames: the first handles the messages and applies them, the second is the feedback
    // pass that is due after the throttle.
    for _ in 0..2 {
        session.midi.clock += SURFACE_STEP;
        let t = session.midi.clock;
        session.shell_mut().core_mut().surface_frame(t);
    }
    session.shell_mut().frame();
    let sent: Vec<String> = session
        .midi
        .surface
        .as_ref()
        .map(|p| p.take_sent().iter().map(|m| hex(m)).collect())
        .unwrap_or_default();
    if action == "detach" {
        session.midi.surface = None;
    }
    let core = session.shell().core();
    Ok(ToolOutput::json(&json!({
        "sent": sent,
        "sent_count": sent.len(),
        "surface": core.surface().map(|s| json!({
            "driver": s.driver_name(),
            "sent": s.sent(),
            "received": s.received(),
        })),
        "message": core.last_rejection(),
        "can_undo": core.document().can_undo(),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_messages_parse_and_bad_ones_are_named() {
        assert_eq!(
            messages("B0 07 7F; 90 3C 64").unwrap(),
            [vec![0xB0, 7, 0x7F], vec![0x90, 0x3C, 0x64]]
        );
        assert_eq!(messages("e0 00 40\n90 0x3c 7f").unwrap().len(), 2);
        assert!(messages("  ; ").is_err());
        assert!(messages("B0 zz").unwrap_err().contains("zz"));
        assert_eq!(hex(&[0xF0, 0, 0x7F]), "F0 00 7F");
    }

    #[test]
    fn targets_parse_in_every_form() {
        assert_eq!(target("tempo").unwrap(), MidiTarget::Tempo);
        assert_eq!(target("vca 3").unwrap(), MidiTarget::Vca { id: 3 });
        assert_eq!(
            target("selected fader").unwrap(),
            MidiTarget::SelectedStrip {
                param: "fader".into()
            }
        );
        assert_eq!(
            target("transport loop").unwrap(),
            MidiTarget::Transport(TransportTarget::Loop)
        );
        assert!(matches!(
            target("channel fader").unwrap(),
            MidiTarget::Param { .. }
        ));
        assert!(target("vca x").is_err());
        assert!(target("transport fly").is_err());
        assert!(target("a b c").is_err());
    }
}
