//! The macro and parameter tools: macro banks and their assignments in a headless session, and the
//! search over every parameter of a project (stateless on a file, or in the session where its
//! rows can be acted on).
//!
//! * `session_macros` makes banks, assigns a param to a macro (with a range and the invert flag),
//!   edits and removes assignments, names and moves macros, chooses the bank new assignments go to,
//!   switches *automation follows selection* on and shows the lane of a param.
//! * `session_param_search` searches the session's project and can reveal, show the lane of,
//!   assign to a macro, arm for MIDI learn or adjust a row of the result.
//! * `param_search` searches a project file, with the values the project stores.

use crate::rpc::{Server, Tool};
use crate::session::Session;
use crate::tier1::read_project_text;
use crate::tool::{Args, ToolOutput, definition};
use atomik_app::app_core::SEARCH_ROWS;
use atomik_model::{
    Command, DeviceKey, MacroCommand, ParamIndex, RackDoc, SearchTarget, macro_key,
};
use atomik_render::default_registry;
use serde_json::{Value, json};

/// The macro and parameter tools.
pub const TOOLS: &[Tool] = &[
    Tool {
        name: "param_search",
        definition: param_search_def,
        run: param_search,
    },
    Tool {
        name: "session_macros",
        definition: macros_def,
        run: macros,
    },
    Tool {
        name: "session_param_search",
        definition: session_search_def,
        run: session_search,
    },
];

fn r6(x: f32) -> f64 {
    (f64::from(x) * 1e6).round() / 1e6
}

/// The macro banks of a document: each bank with its eight macros (name, value) and their
/// assignments (id, device, param, range, invert, and `live` false when the bank or the target is
/// gone). Empty when the project has none; shared by `project_inspect` and `session_status`.
pub fn macros_json(doc: &RackDoc) -> Vec<Value> {
    doc.banks()
        .map(|bank| {
            let macros: Vec<Value> = (0..8u8)
                .map(|i| {
                    let assigns: Vec<Value> = doc
                        .macros
                        .of_macro(&bank.key, i)
                        .map(|a| {
                            let mut v = json!({
                                "id": a.id,
                                "device": a.device.0,
                                "param": a.param,
                                "min": r6(a.min),
                                "max": r6(a.max),
                            });
                            if a.invert {
                                v["invert"] = json!(true);
                            }
                            if !doc.macro_assign_live(a) {
                                v["live"] = json!(false);
                            }
                            v
                        })
                        .collect();
                    let mut m = json!({
                        "macro": u32::from(i) + 1,
                        "name": doc.macro_name(&bank.key, i),
                        "value": r6(doc.bank_macro_value(&bank.key, i)),
                    });
                    if !assigns.is_empty() {
                        m["assigns"] = Value::Array(assigns);
                    }
                    m
                })
                .collect();
            json!({"bank": bank.key.0, "name": bank.name, "macros": macros})
        })
        .collect()
}

// ------------------------------------------------------------------ session_macros

fn macros_def() -> Value {
    definition(
        "session_macros",
        "Macro banks in the session. A bank is a rack device (`atomik.macro-bank`) with eight macro knobs; an assignment makes one macro drive a param of another top-level device between `min` and `max` (plain values of the param), optionally inverted. A param follows at most one macro (assigning it again moves it); a driven param refuses a direct edit. `action`: `new_bank`, `bank` (choose the bank new assignments go to: `bank`), `assign` (`device`, `param`, `macro` 1 to 8, optional `bank` or `new_bank` true, `min`, `max`, `invert`), `range` (assignment `id` with `min`, `max`, `invert`), `remove` (assignment `id`, or `device` and `param`; the param keeps its value), `clear` (every assignment of `macro` of `bank`), `rename` (`bank`, `macro`, `name`; empty resets it), `set` (`bank`, `macro`, `value` 0 to 1: the macro's value, one undo step), `follow` (`on`: automation follows the selection, as the View menu item) and `lane` (`device`, `param`: show the automation lane of a param, adding the empty lane if it is missing; a param a macro drives shows the macro's lane). Returns the banks with their macros and assignments, the last message the app showed and whether there is something to undo.",
        &[
            (
                "action",
                "string",
                "new_bank, bank, assign, range, remove, clear, rename, set, follow or lane.",
            ),
            (
                "bank",
                "string",
                "The macro bank device key (the default: the current bank).",
            ),
            ("macro", "number", "The macro, 1 to 8."),
            ("device", "string", "assign, remove, lane: the device key."),
            ("param", "string", "assign, remove, lane: the param key."),
            (
                "id",
                "number",
                "range, remove: the assignment id (`macros` in `session_status` lists them).",
            ),
            (
                "min",
                "number",
                "assign, range: the plain value at macro 0 (default: the param's minimum).",
            ),
            (
                "max",
                "number",
                "assign, range: the plain value at macro 1 (default: the param's maximum).",
            ),
            (
                "invert",
                "boolean",
                "assign, range: the macro runs backwards.",
            ),
            (
                "new_bank",
                "boolean",
                "assign: make a new bank for this assignment.",
            ),
            (
                "name",
                "string",
                "rename: the macro's name, at most 24 characters.",
            ),
            ("value", "number", "set: the macro's value, 0 to 1."),
            (
                "on",
                "boolean",
                "follow: switch following on (true) or off.",
            ),
        ],
        &["action"],
    )
}

fn macro_index(args: &Args) -> Result<u8, String> {
    let n = args
        .opt_u64("macro")?
        .ok_or("missing argument `macro` (1 to 8)")?;
    if (1..=8).contains(&n) {
        Ok((n - 1) as u8)
    } else {
        Err(format!("`macro` is 1 to 8, not {n}"))
    }
}

fn bank_arg(session: &Session, args: &Args) -> Result<DeviceKey, String> {
    match args.opt_str("bank")? {
        Some(k) => Ok(DeviceKey(k.to_owned())),
        None => session
            .shell()
            .core()
            .macro_bank()
            .ok_or_else(|| "the project has no macro bank (action new_bank)".to_owned()),
    }
}

fn macros_state(session: &Session) -> Value {
    let core = session.shell().core();
    json!({
        "banks": macros_json(core.doc()),
        "current_bank": core.macro_bank().map(|k| k.0),
        "follow_selection": core.follow_selection(),
        "message": core.last_rejection(),
        "can_undo": core.document().can_undo(),
    })
}

fn macros(server: &mut Server, args: &Args) -> Result<ToolOutput, String> {
    let action = args.str("action")?;
    let session = server.require_session()?;
    let done = match action {
        "new_bank" => session.shell_mut().core_mut().new_macro_bank(),
        "bank" => {
            let bank = bank_arg(session, args)?;
            session.shell_mut().core_mut().set_macro_bank(bank);
            true
        }
        "assign" => {
            let device = DeviceKey(args.str("device")?.to_owned());
            let param = args.str("param")?.to_owned();
            let index = macro_index(args)?;
            // No bank named and none in the project: the assignment makes one.
            let bank = if args.opt_bool("new_bank")?.unwrap_or(false) {
                None
            } else if let Some(k) = args.opt_str("bank")? {
                Some(DeviceKey(k.to_owned()))
            } else {
                session.shell().core().macro_bank()
            };
            let made = bank.is_none();
            let command = Command::Macro(MacroCommand::Assign {
                bank,
                index,
                device,
                param,
                min: args.opt_f64("min")?.map(|v| v as f32),
                max: args.opt_f64("max")?.map(|v| v as f32),
                invert: args.opt_bool("invert")?.unwrap_or(false),
            });
            let core = session.shell_mut().core_mut();
            let ok = core.apply_command(command).is_ok();
            if ok
                && made
                && let Some(last) = core.doc().banks().last().map(|d| d.key.clone())
            {
                core.set_macro_bank(last);
            }
            ok
        }
        "range" => {
            let id = args.opt_u64("id")?.ok_or("missing argument `id`")? as u32;
            let current = session
                .shell()
                .core()
                .doc()
                .macros
                .assigns
                .iter()
                .find(|a| a.id == id)
                .cloned()
                .ok_or_else(|| format!("no assignment {id}"))?;
            let command = Command::Macro(MacroCommand::SetRange {
                id,
                min: args.opt_f64("min")?.map_or(current.min, |v| v as f32),
                max: args.opt_f64("max")?.map_or(current.max, |v| v as f32),
                invert: args.opt_bool("invert")?.unwrap_or(current.invert),
            });
            session
                .shell_mut()
                .core_mut()
                .apply_command(command)
                .is_ok()
        }
        "remove" => {
            let core = session.shell_mut().core_mut();
            match args.opt_u64("id")? {
                Some(id) => core
                    .apply_command(Command::Macro(MacroCommand::Remove { id: id as u32 }))
                    .is_ok(),
                None => {
                    let device = DeviceKey(args.str("device")?.to_owned());
                    core.remove_from_macro(&device, args.str("param")?)
                }
            }
        }
        "clear" => {
            let command = Command::Macro(MacroCommand::Clear {
                bank: bank_arg(session, args)?,
                index: macro_index(args)?,
            });
            session
                .shell_mut()
                .core_mut()
                .apply_command(command)
                .is_ok()
        }
        "rename" => {
            let command = Command::Macro(MacroCommand::Rename {
                bank: bank_arg(session, args)?,
                index: macro_index(args)?,
                name: args.opt_str("name")?.unwrap_or("").to_owned(),
            });
            session
                .shell_mut()
                .core_mut()
                .apply_command(command)
                .is_ok()
        }
        "set" => {
            let command = Command::SetParam {
                key: bank_arg(session, args)?,
                param: macro_key(macro_index(args)?),
                value: args.opt_f64("value")?.ok_or("missing argument `value`")? as f32,
                merge: None,
            };
            session
                .shell_mut()
                .core_mut()
                .apply_command(command)
                .is_ok()
        }
        "follow" => {
            let on = args.opt_bool("on")?.unwrap_or(true);
            session.shell_mut().core_mut().set_follow_selection(on);
            true
        }
        "lane" => {
            let device = DeviceKey(args.str("device")?.to_owned());
            session
                .shell_mut()
                .core_mut()
                .show_lane_of(&device, args.str("param")?)
        }
        other => {
            return Err(format!(
                "`{other}` is not an action: new_bank, bank, assign, range, remove, clear, rename, set, follow or lane"
            ));
        }
    };
    session.shell_mut().frame();
    let mut v = macros_state(session);
    v["done"] = json!(done);
    Ok(ToolOutput::json(&v))
}

// ------------------------------------------------------------------ the search

fn target_json(t: &SearchTarget) -> Value {
    match t {
        SearchTarget::Param { device, param } => json!({"device": device.0, "param": param}),
        SearchTarget::Vca { id } => json!({"vca": id}),
    }
}

fn param_search_def() -> Value {
    definition(
        "param_search",
        "Searches every parameter of a project (all devices, the inside of containers, channel strips, buses, the master, plugin params the project knows, macros and the VCA levels) by track, device, param name, key and unit: every word of `query` must match, a prefix beats a substring beats a fuzzy match, and `>` separates words (`lead > filter > cut`). Each result is `track > device > param` with the target (`device` and `param` keys, or `vca` id), the unit and the value the project stores. Results are best first and cut at `limit`.",
        &[
            ("path", "string", "Project file or bundle folder."),
            (
                "query",
                "string",
                "The words to find; empty lists the first params in rack order.",
            ),
            (
                "limit",
                "number",
                "At most this many results (default 25, at most 500).",
            ),
        ],
        &["path"],
    )
}

fn param_search(server: &mut Server, args: &Args) -> Result<ToolOutput, String> {
    let (file, text) = read_project_text(server, args.str("path")?)?;
    let registry = default_registry();
    let doc = atomik_model::load_project(&text, &registry).map_err(|e| e.to_string())?;
    let index = ParamIndex::build(&doc, &registry);
    let limit = args.opt_u64("limit")?.unwrap_or(25) as usize;
    let query = args.opt_str("query")?.unwrap_or("");
    let rows: Vec<Value> = index
        .search(query, limit)
        .into_iter()
        .map(|e| {
            let mut v = json!({
                "track": e.track,
                "device": e.device,
                "param": e.param,
                "target": target_json(&e.target),
            });
            if !e.unit.is_empty() {
                v["unit"] = json!(e.unit);
            }
            match &e.target {
                SearchTarget::Param { device, param } => {
                    if let Some(x) = doc.effective_param(&registry, device, param) {
                        v["value"] = json!(r6(x));
                    }
                    if let Some(a) = doc.bank_driver(device, param) {
                        v["macro"] = json!({"bank": a.bank.0, "macro": u32::from(a.index) + 1});
                    }
                }
                SearchTarget::Vca { id } => {
                    if let Some(g) = doc.console.vca(*id) {
                        v["value"] = json!(r6(g.level_db));
                    }
                }
            }
            v
        })
        .collect();
    Ok(ToolOutput::json(&json!({
        "file": file.to_string_lossy(),
        "indexed": index.len(),
        "results": rows,
    })))
}

fn session_search_def() -> Value {
    definition(
        "session_param_search",
        "Searches every parameter of the session's project like the Find Parameter panel (see `param_search`: the words, the ranking) and can act on a row of the result: `action` `reveal` (select the device in the rack, or the strips in the console), `lane` (show its automation lane in the arrangement, adding the empty lane), `assign` (to the next free macro of the current bank), `learn` (arm it for MIDI learn) or `adjust` (`steps` of a fiftieth of its range, or of one step for a stepped param; `fine` makes them a tenth). `row` is the position in the result (default 0). Returns the rows with the value now, the macro that drives each, its MIDI mark and its lane, what was done and the last message the app showed.",
        &[
            (
                "query",
                "string",
                "The words to find (empty lists the first params).",
            ),
            (
                "limit",
                "number",
                "At most this many rows (default 25, at most 100).",
            ),
            (
                "action",
                "string",
                "reveal, lane, assign, learn or adjust, on row `row`.",
            ),
            ("row", "number", "The row to act on (default 0)."),
            (
                "steps",
                "number",
                "adjust: how many steps, negative for down (default 1).",
            ),
            ("fine", "boolean", "adjust: a tenth of a step."),
        ],
        &[],
    )
}

fn session_search(server: &mut Server, args: &Args) -> Result<ToolOutput, String> {
    let query = args.opt_str("query")?.unwrap_or("").to_owned();
    let limit = (args.opt_u64("limit")?.unwrap_or(25) as usize).min(SEARCH_ROWS);
    let row = args.opt_u64("row")?.unwrap_or(0) as usize;
    let session = server.require_session()?;
    let core = session.shell_mut().core_mut();
    let _ = core.param_search(&query);
    let done = match args.opt_str("action")? {
        None => None,
        Some("reveal") => Some(core.search_reveal(row).is_some()),
        Some("lane") => Some(core.search_show_lane(row)),
        Some("assign") => Some(core.search_assign(row)),
        Some("learn") => Some(core.search_learn(row)),
        Some("adjust") => {
            let steps = args.opt_f64("steps")?.unwrap_or(1.0) as i32;
            Some(core.search_adjust(row, steps, args.opt_bool("fine")?.unwrap_or(false)))
        }
        Some(other) => {
            return Err(format!(
                "`{other}` is not an action: reveal, lane, assign, learn or adjust"
            ));
        }
    };
    let (rows, total) = core.param_search_refresh();
    let targets = core.param_search_targets();
    let rows: Vec<Value> = rows
        .into_iter()
        .zip(targets)
        .take(limit)
        .enumerate()
        .map(|(i, (r, t))| {
            let mut v = json!({
                "row": i,
                "track": r.track,
                "device": r.device,
                "param": r.param,
                "target": target_json(&t),
                "value": r.value,
            });
            if let Some(m) = r.macro_tag {
                v["macro"] = json!(m);
            }
            if r.midi {
                v["midi"] = json!(true);
            }
            if r.lane {
                v["lane"] = json!(true);
            }
            v
        })
        .collect();
    let mut out = json!({
        "total": total,
        "results": rows,
        "message": core.last_rejection(),
        "can_undo": core.document().can_undo(),
    });
    if let Some(d) = done {
        out["done"] = json!(d);
    }
    Ok(ToolOutput::json(&out))
}
