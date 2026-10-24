//! `atomik-mcp`: a Model Context Protocol server over stdio for QA of Atomik Rack.
//!
//! An agent connected to it can build or load a project, bounce it, measure and compare the
//! audio, and drive the real app shell headlessly (commands, pointer, keys, screenshots), with no
//! window, no GPU and no audio device. It is a development tool: nothing in the app links it.
//!
//! - [`rpc`]: the JSON-RPC 2.0 / MCP protocol (`initialize`, `tools/list`, `tools/call`).
//! - [`tool`]: arguments, results and the folders the tools may touch ([`tool::Paths`]).
//! - [`tier1`]: stateless tools: `list_devices`, `project_inspect`, `validate`, `bounce`,
//!   `analyze_audio`, `compare_audio`, `list_plugins`.
//! - [`session`]: the stateful tools over one headless [`atomik_app::shell::Shell`]:
//!   `session_open`, `session_commands`, `session_command`, `session_pointer`, `session_key`,
//!   `session_texts`, `session_screenshot`, `session_status`, `session_save`, `session_set_param`,
//!   `session_meters`, `session_bounce`, `session_close`.
//! - [`analysis`]: the audio measurements behind `analyze_audio` and `compare_audio`.
//!
//! `docs/MCP.md` is the reference and the QA recipes.

pub mod analysis;
pub mod b64;
pub mod rpc;
pub mod session;
pub mod tier1;
pub mod tool;

pub use rpc::Server;
pub use tool::{Args, Content, Paths, ToolOutput};
