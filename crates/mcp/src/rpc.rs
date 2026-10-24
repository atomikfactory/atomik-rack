//! The protocol: JSON-RPC 2.0 messages, one per line, with the MCP methods a tool server needs.

use crate::session::Session;
use crate::tool::{Args, Content, Paths, ToolOutput};
use crate::{b64, session, tier1};
use serde_json::{Value, json};

/// The protocol revision this server speaks; a client asking for another is answered with this
/// one and decides whether to continue.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

/// What the model reads once, when it connects.
const INSTRUCTIONS: &str = "Atomik Rack QA tools. Stateless: list_devices, project_inspect, \
validate, bounce (project or patch to WAV, with the audio report), analyze_audio, compare_audio \
(against a reference render), list_plugins. Stateful (one headless app at a time): session_open, \
then session_commands / session_command (menu commands), session_pointer and session_key (input), \
session_texts (every text the UI draws, with positions), session_screenshot (PNG), session_status, \
session_meters (what the spectrum and phase meter devices show at a position), session_bounce (render what the session holds), session_save, session_close. The session has no \
audio device: it checks the model, the UI and the bounce, not live playback. Paths are relative to \
the server's working directory and must lie inside its allowed folders.";

/// A tool: its definition for `tools/list` and the function a call runs.
pub struct Tool {
    /// The tool's name.
    pub name: &'static str,
    /// Builds the definition (name, description, input schema).
    pub definition: fn() -> Value,
    /// Runs a call; `Err` is a failed call with that message.
    pub run: fn(&mut Server, &Args) -> Result<ToolOutput, String>,
}

/// Every tool, in the order `tools/list` shows them.
pub fn all_tools() -> impl Iterator<Item = &'static Tool> {
    tier1::TOOLS.iter().chain(session::TOOLS.iter())
}

/// The server state: the folders it may touch and the open session.
pub struct Server {
    paths: Paths,
    session: Option<Session>,
}

impl Server {
    /// A server confined to `paths`.
    pub fn new(paths: Paths) -> Self {
        Self {
            paths,
            session: None,
        }
    }

    /// The path policy.
    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    /// The open session, if any.
    pub fn session_mut(&mut self) -> Option<&mut Session> {
        self.session.as_mut()
    }

    /// The open session, or the error a tool returns when there is none.
    ///
    /// # Errors
    /// A message that says to call `session_open`.
    pub fn require_session(&mut self) -> Result<&mut Session, String> {
        self.session
            .as_mut()
            .ok_or_else(|| "no open session: call session_open first".to_string())
    }

    /// Replaces the session (the old one is dropped).
    pub fn set_session(&mut self, session: Option<Session>) {
        self.session = session;
    }

    /// Runs the tool `name`; every failure is a result with `is_error`.
    pub fn call_tool(&mut self, name: &str, args: &Args) -> ToolOutput {
        match all_tools().find(|t| t.name == name) {
            None => ToolOutput::error(format!("unknown tool `{name}`")),
            Some(tool) => (tool.run)(self, args).unwrap_or_else(ToolOutput::error),
        }
    }

    /// Handles one decoded message; `None` for a notification (nothing is sent back).
    pub fn handle(&mut self, message: &Value) -> Option<Value> {
        let Some(object) = message.as_object() else {
            return Some(error_reply(
                Value::Null,
                INVALID_REQUEST,
                "a message is a JSON object (batches are not supported)",
            ));
        };
        let id = object.get("id").cloned();
        let Some(method) = object.get("method").and_then(Value::as_str) else {
            // A response to a request of ours (there are none) or garbage.
            return id.map(|id| error_reply(id, INVALID_REQUEST, "no method"));
        };
        let params = object.get("params");
        let result = match method {
            "initialize" => Ok(initialize(params)),
            "ping" => Ok(json!({})),
            "tools/list" => {
                Ok(json!({ "tools": all_tools().map(|t| (t.definition)()).collect::<Vec<_>>() }))
            }
            "tools/call" => self.tools_call(params),
            m if m.starts_with("notifications/") => return None,
            other => Err((METHOD_NOT_FOUND, format!("method not found: {other}"))),
        };
        // A request without an id is a notification: never answered.
        let id = id?;
        Some(match result {
            Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
            Err((code, message)) => error_reply(id, code, &message),
        })
    }

    /// Handles one line of input: parses it and returns the reply line, if any.
    pub fn handle_line(&mut self, line: &str) -> Option<String> {
        let reply = match serde_json::from_str::<Value>(line) {
            Ok(message) => self.handle(&message)?,
            Err(e) => error_reply(Value::Null, PARSE_ERROR, &format!("parse error: {e}")),
        };
        Some(reply.to_string())
    }

    fn tools_call(&mut self, params: Option<&Value>) -> Result<Value, (i64, String)> {
        let name = params
            .and_then(|p| p.get("name"))
            .and_then(Value::as_str)
            .ok_or((INVALID_PARAMS, "tools/call needs a tool name".to_string()))?;
        if !all_tools().any(|t| t.name == name) {
            return Err((INVALID_PARAMS, format!("unknown tool `{name}`")));
        }
        let output = self.call_tool(name, &Args::new(params.and_then(|p| p.get("arguments"))));
        Ok(output_json(&output))
    }
}

fn initialize(params: Option<&Value>) -> Value {
    let _ = params;
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "atomik-mcp", "version": env!("CARGO_PKG_VERSION") },
        "instructions": INSTRUCTIONS,
    })
}

fn error_reply(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// The `tools/call` result of an output.
pub fn output_json(output: &ToolOutput) -> Value {
    let content: Vec<Value> = output
        .content
        .iter()
        .map(|c| match c {
            Content::Text(t) => json!({"type": "text", "text": t}),
            Content::Png(bytes) => {
                json!({"type": "image", "data": b64::encode(bytes), "mimeType": "image/png"})
            }
        })
        .collect();
    json!({"content": content, "isError": output.is_error})
}
