//! The MCP protocol of the server: handshake, tool listing, errors, and the real binary over
//! stdio.

use atomik_mcp::{Paths, Server};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn repo() -> PathBuf {
    std::fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")).unwrap()
}

fn server() -> Server {
    Server::new(Paths::new(repo(), &[]))
}

fn rpc(server: &mut Server, message: Value) -> Option<Value> {
    server
        .handle_line(&message.to_string())
        .map(|line| serde_json::from_str(&line).unwrap())
}

const EXPECTED_TOOLS: &[&str] = &[
    "list_devices",
    "project_inspect",
    "validate",
    "bounce",
    "analyze_audio",
    "compare_audio",
    "list_plugins",
    "session_open",
    "session_status",
    "session_commands",
    "session_command",
    "session_pointer",
    "session_key",
    "session_texts",
    "session_screenshot",
    "session_save",
    "session_relink",
    "session_midi",
    "session_audio_notes",
    "session_set_param",
    "session_meters",
    "session_bounce",
    "session_close",
    "session_window",
    "session_midi_input",
    "session_midi_learn",
    "session_surface",
    "param_search",
    "session_macros",
    "session_param_search",
    "library_list",
    "session_library",
];

#[test]
fn initialize_names_the_server_and_offers_tools() {
    let mut s = server();
    let r = rpc(
        &mut s,
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "test", "version": "0"}}}),
    )
    .unwrap();
    assert_eq!(r["id"], 1);
    assert_eq!(r["result"]["serverInfo"]["name"], "atomik-mcp");
    assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
    assert!(r["result"]["capabilities"]["tools"].is_object());
    assert!(
        r["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("session_open")
    );
}

#[test]
fn notifications_are_never_answered_and_ping_is() {
    let mut s = server();
    assert!(
        rpc(
            &mut s,
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
        )
        .is_none()
    );
    let r = rpc(
        &mut s,
        json!({"jsonrpc": "2.0", "id": "p", "method": "ping"}),
    )
    .unwrap();
    assert_eq!(r["id"], "p");
    assert_eq!(r["result"], json!({}));
}

#[test]
fn every_tool_is_listed_once_with_an_object_schema() {
    let mut s = server();
    let r = rpc(
        &mut s,
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
    )
    .unwrap();
    let tools = r["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, EXPECTED_TOOLS);
    for t in tools {
        assert!(!t["description"].as_str().unwrap().is_empty(), "{t}");
        assert_eq!(t["inputSchema"]["type"], "object");
        let props = t["inputSchema"]["properties"].as_object().unwrap();
        for req in t["inputSchema"]["required"].as_array().unwrap() {
            assert!(props.contains_key(req.as_str().unwrap()), "{t}");
        }
        for (name, p) in props {
            assert!(p["description"].as_str().is_some(), "{name} of {t}");
        }
    }
}

#[test]
fn errors_use_the_json_rpc_codes() {
    let mut s = server();
    let bad = s.handle_line("{not json").unwrap();
    let bad: Value = serde_json::from_str(&bad).unwrap();
    assert_eq!(bad["error"]["code"], -32700);
    assert_eq!(bad["id"], Value::Null);

    let unknown = rpc(
        &mut s,
        json!({"jsonrpc": "2.0", "id": 3, "method": "resources/list"}),
    )
    .unwrap();
    assert_eq!(unknown["error"]["code"], -32601);

    let tool = rpc(
        &mut s,
        json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "no_such_tool"}}),
    )
    .unwrap();
    assert_eq!(tool["error"]["code"], -32602);

    let nameless = rpc(
        &mut s,
        json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": {}}),
    )
    .unwrap();
    assert_eq!(nameless["error"]["code"], -32602);

    let batch = rpc(
        &mut s,
        json!([{"jsonrpc": "2.0", "id": 6, "method": "ping"}]),
    )
    .unwrap();
    assert_eq!(batch["error"]["code"], -32600);
}

#[test]
fn a_failed_tool_call_is_a_result_the_model_can_read() {
    let mut s = server();
    let r = rpc(
        &mut s,
        json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call",
               "params": {"name": "validate", "arguments": {}}}),
    )
    .unwrap();
    assert!(r.get("error").is_none());
    assert_eq!(r["result"]["isError"], true);
    assert!(
        r["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("missing argument `path`")
    );
}

#[test]
fn images_are_base64_blocks() {
    let out = atomik_mcp::ToolOutput::text("x").with_png(vec![0x89, b'P', b'N', b'G']);
    let v = atomik_mcp::rpc::output_json(&out);
    assert_eq!(v["content"][1]["type"], "image");
    assert_eq!(v["content"][1]["mimeType"], "image/png");
    assert_eq!(v["content"][1]["data"], "iVBORw==");
}

#[test]
fn the_binary_speaks_the_protocol_over_stdio() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_atomik-mcp"))
        .current_dir(repo())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let lines = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
               "params": {"name": "list_devices", "arguments": {"query": "synth"}}}),
    ];
    for l in &lines {
        writeln!(stdin, "{l}").unwrap();
    }
    writeln!(stdin).unwrap(); // a blank line is skipped
    drop(stdin); // end of input: the server exits
    let replies: Vec<Value> = BufReader::new(child.stdout.take().unwrap())
        .lines()
        .map(|l| serde_json::from_str(&l.unwrap()).unwrap())
        .collect();
    assert!(child.wait().unwrap().success());
    assert_eq!(
        replies.len(),
        2,
        "the notification gets no reply: {replies:?}"
    );
    assert_eq!(replies[0]["id"], 1);
    assert_eq!(replies[1]["id"], 2);
    let text = replies[1]["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("atomik.poly-synth"), "{text}");
}

#[test]
fn the_binary_rejects_unknown_arguments() {
    let out = Command::new(env!("CARGO_BIN_EXE_atomik-mcp"))
        .arg("--bogus")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown argument"));
}
