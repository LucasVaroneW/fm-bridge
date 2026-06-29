// Minimal MCP (Model Context Protocol) server over stdio.
//
// Newline-delimited JSON-RPC 2.0, synchronous, zero extra dependencies — same
// "no async, procedural, minimal" ethos as the rest of the binary. This is the
// **AI front door**: any MCP client (Claude Desktop, Cursor, Antigravity, …)
// can drive the exact same engine the human uses, because every tool here just
// forwards to `handle_command` — no logic is duplicated.
//
// Lifecycle handled: `initialize` → `notifications/initialized` (ignored) →
// `tools/list` → `tools/call`. Requests (with `id`) get a reply; notifications
// (no `id`) don't. Anything we can't model is surfaced as a JSON-RPC error, not
// a panic.

use serde_json::{json, Value};
use std::io::{BufRead, Write};

use crate::{handle_command, steps, Command};

/// A JSON-RPC error as (code, message).
type RpcError = (i64, String);

/// Run the stdio server loop until EOF. Never prints anything to stdout except
/// protocol messages (so the channel stays clean).
pub fn run() -> Result<(), String> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    for line in stdin.lock().lines() {
        let line = line.map_err(|e| format!("stdin read error: {}", e))?;
        if line.trim().is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue, // ignore malformed frames rather than crash
        };

        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = msg.get("params");

        let outcome: Result<Value, RpcError> = match method {
            "initialize" => Ok(initialize_result(params)),
            "tools/list" => Ok(tools_list_result()),
            "tools/call" => tools_call(params),
            "ping" => Ok(json!({})),
            other => Err((-32601, format!("Method not found: {}", other))),
        };

        // Only requests (those carrying an `id`) get a reply. Notifications such
        // as `notifications/initialized` are fire-and-forget.
        if let Some(id) = id {
            let envelope = match outcome {
                Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                Err((code, message)) => {
                    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
                }
            };
            writeln!(out, "{}", envelope).map_err(|e| format!("stdout write error: {}", e))?;
            out.flush().map_err(|e| format!("stdout flush error: {}", e))?;
        }
    }
    Ok(())
}

/// `initialize` reply. We echo the client's requested protocol version when
/// present (maximises compatibility) and advertise the `tools` capability.
fn initialize_result(params: Option<&Value>) -> Value {
    let protocol_version = params
        .and_then(|p| p.get("protocolVersion"))
        .and_then(|v| v.as_str())
        .unwrap_or("2024-11-05");
    json!({
        "protocolVersion": protocol_version,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "fm-bridge", "version": env!("CARGO_PKG_VERSION") }
    })
}

/// The tool catalog advertised to the client. Each forwards to a binary command
/// (or, for `list_steps`, to the step catalog).
fn tools_list_result() -> Value {
    json!({ "tools": [
        {
            "name": "read_clipboard_script",
            "description": "Read the FileMaker clipboard and return the decoded .fmscript text. The user must copy script steps in FileMaker first (Cmd/Ctrl+C).",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        },
        {
            "name": "validate_script",
            "description": "Validate .fmscript text and return every format/structure error (unknown steps, unclosed brackets, unbalanced If/Loop blocks). No errors = valid. Does not touch the clipboard.",
            "inputSchema": { "type": "object", "properties": { "script_text": { "type": "string", "description": "The .fmscript source to validate." } }, "required": ["script_text"] }
        },
        {
            "name": "script_to_json",
            "description": "Parse .fmscript text into a structured JSON tree of steps (name, calculation, fields, variables, block nesting) for precise reasoning.",
            "inputSchema": { "type": "object", "properties": { "script_text": { "type": "string", "description": "The .fmscript source to parse." } }, "required": ["script_text"] }
        },
        {
            "name": "inspect_database",
            "description": "Parse a FileMaker FMSaveAsXML export into a navigable inspection directory (tables, fields with calc/index, layouts, table occurrences, relationships, custom functions, scripts in folders) and return counts + output paths.",
            "inputSchema": { "type": "object", "properties": { "xml_path": { "type": "string", "description": "Path to the FMSaveAsXML .xml export." }, "output_dir": { "type": "string", "description": "Where to write the inspection (default: fm-inspect-output)." } }, "required": ["xml_path"] }
        },
        {
            "name": "slice_inspect",
            "description": "From an existing inspect output, build a focused slice around one or more layouts: the transitive closure of triggered scripts, referenced table occurrences, relationships and custom functions.",
            "inputSchema": { "type": "object", "properties": { "output_dir": { "type": "string", "description": "An existing inspect output directory." }, "slice_dir": { "type": "string", "description": "Where to write the slice." }, "layouts": { "type": "array", "items": { "type": "string" }, "description": "Layout name(s) to anchor the slice on." } }, "required": ["output_dir", "slice_dir", "layouts"] }
        },
        {
            "name": "audit_database",
            "description": "Scan a FileMaker FMSaveAsXML export for broken references (dangling Perform Script / Go to Layout targets, relationships and layouts pointing at deleted table occurrences, table occurrences whose base table is gone, ghost fields on layouts). Returns a structured issue list — the fast way to find bugs.",
            "inputSchema": { "type": "object", "properties": { "xml_path": { "type": "string", "description": "Path to the FMSaveAsXML .xml export." } }, "required": ["xml_path"] }
        },
        {
            "name": "list_steps",
            "description": "Return the catalog of supported FileMaker script step types (English/Spanish name, shape, block behavior).",
            "inputSchema": { "type": "object", "properties": {}, "additionalProperties": false }
        }
    ] })
}

/// Dispatch `tools/call`. Builds the matching `Command`, runs `handle_command`,
/// and wraps the `Response` as MCP tool content. `isError` mirrors the engine's
/// status so the model knows when a call failed.
fn tools_call(params: Option<&Value>) -> Result<Value, RpcError> {
    let params = params.ok_or((-32602, "Missing params".to_string()))?;
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or((-32602, "Missing tool name".to_string()))?;
    let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));

    // list_steps is the one tool not backed by handle_command.
    if name == "list_steps" {
        let text = serde_json::to_string_pretty(&steps::catalog())
            .unwrap_or_else(|_| "[]".to_string());
        return Ok(tool_result(&text, false));
    }

    let mut cmd = base_command();
    match name {
        "read_clipboard_script" => cmd.command = "read".to_string(),
        "validate_script" => {
            cmd.command = "parse".to_string();
            cmd.script_text = Some(arg_str(&args, "script_text")?);
        }
        "script_to_json" => {
            cmd.command = "to_json".to_string();
            cmd.script_text = Some(arg_str(&args, "script_text")?);
        }
        "inspect_database" => {
            cmd.command = "inspect".to_string();
            cmd.xml_path = Some(arg_str(&args, "xml_path")?);
            cmd.output_dir = args
                .get("output_dir")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
        }
        "audit_database" => {
            cmd.command = "audit".to_string();
            cmd.xml_path = Some(arg_str(&args, "xml_path")?);
        }
        "slice_inspect" => {
            cmd.command = "slice".to_string();
            cmd.output_dir = Some(arg_str(&args, "output_dir")?);
            cmd.slice_dir = Some(arg_str(&args, "slice_dir")?);
            cmd.layouts = Some(arg_strings(&args, "layouts")?);
        }
        other => return Err((-32602, format!("Unknown tool: {}", other))),
    }

    let response = handle_command(&cmd);
    let is_error = response.status == "error";
    let text = serde_json::to_string_pretty(&response).unwrap_or_else(|_| "{}".to_string());
    Ok(tool_result(&text, is_error))
}

fn base_command() -> Command {
    Command {
        command: String::new(),
        script_text: None,
        xml_path: None,
        output_dir: None,
        slice_dir: None,
        layouts: None,
    }
}

/// Wrap text into the MCP `tools/call` result shape.
fn tool_result(text: &str, is_error: bool) -> Value {
    json!({ "content": [ { "type": "text", "text": text } ], "isError": is_error })
}

fn arg_str(args: &Value, key: &str) -> Result<String, RpcError> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or((-32602, format!("Missing or non-string argument: {}", key)))
}

fn arg_strings(args: &Value, key: &str) -> Result<Vec<String>, RpcError> {
    let arr = args
        .get(key)
        .and_then(|v| v.as_array())
        .ok_or((-32602, format!("Missing or non-array argument: {}", key)))?;
    let out: Vec<String> = arr.iter().filter_map(|v| v.as_str().map(String::from)).collect();
    if out.is_empty() {
        return Err((-32602, format!("Argument {} must be a non-empty string array", key)));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_list_has_the_expected_tools() {
        let list = tools_list_result();
        let names: Vec<&str> = list["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        for expected in [
            "read_clipboard_script",
            "validate_script",
            "script_to_json",
            "inspect_database",
            "slice_inspect",
            "audit_database",
            "list_steps",
        ] {
            assert!(names.contains(&expected), "missing tool {}", expected);
        }
    }

    #[test]
    fn initialize_echoes_protocol_version() {
        let res = initialize_result(Some(&json!({ "protocolVersion": "2025-06-18" })));
        assert_eq!(res["protocolVersion"], "2025-06-18");
        assert_eq!(res["serverInfo"]["name"], "fm-bridge");
        assert!(res["capabilities"]["tools"].is_object());
    }

    #[test]
    fn call_script_to_json_returns_step_tree() {
        let params = json!({
            "name": "script_to_json",
            "arguments": { "script_text": "Set Variable [$x = 1]\nIf [$x = 1]\n  Show All Records\nEnd If" }
        });
        let res = tools_call(Some(&params)).unwrap();
        assert_eq!(res["isError"], false);
        let text = res["content"][0]["text"].as_str().unwrap();
        // The serialized engine Response carries the structured tree under `data`.
        let parsed: Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["status"], "ok");
        assert_eq!(parsed["data"]["steps"][0]["name"], "Set Variable");
    }

    #[test]
    fn call_validate_script_flags_errors() {
        let params = json!({
            "name": "validate_script",
            "arguments": { "script_text": "If [$x = 1]\n  Show All Records" } // missing End If
        });
        let res = tools_call(Some(&params)).unwrap();
        assert_eq!(res["isError"], true);
        let parsed: Value =
            serde_json::from_str(res["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(parsed["status"], "error");
        assert!(parsed["errors"].as_array().unwrap().iter().any(|e| e["message"]
            .as_str()
            .unwrap()
            .contains("never closed")));
    }

    #[test]
    fn call_list_steps_returns_catalog() {
        let res = tools_call(Some(&json!({ "name": "list_steps" }))).unwrap();
        assert_eq!(res["isError"], false);
        let text = res["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("Set Variable"));
    }

    #[test]
    fn unknown_tool_is_an_rpc_error() {
        let err = tools_call(Some(&json!({ "name": "nope" }))).unwrap_err();
        assert_eq!(err.0, -32602);
    }
}
