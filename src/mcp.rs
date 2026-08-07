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

use serde_json::{Value, json};
use std::io::{BufRead, Write};

use crate::{Command, handle_command, steps};

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
            out.flush()
                .map_err(|e| format!("stdout flush error: {}", e))?;
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
            "name": "describe_database",
            "description": "Inline overview of a database from a single FMSaveAsXML export: counts plus the names of every table, script, layout, custom function and external source. Writes nothing to disk — the right first call to orient yourself before drilling in with get_table / get_script.",
            "inputSchema": { "type": "object", "properties": { "xml_path": { "type": "string", "description": "Path to the FMSaveAsXML .xml export." } }, "required": ["xml_path"] }
        },
        {
            "name": "get_table",
            "description": "List a base table's fields from a single FMSaveAsXML export (no disk writes). Big tables (>40 fields) auto-summarise to one line per field to stay context-sized; pass summary=true to force that, or fields=[…] to get the full definition (validation, auto-enter, calc, storage) of just some fields. For a single field prefer get_field. Use describe_database first for table names.",
            "inputSchema": { "type": "object", "properties": { "xml_path": { "type": "string", "description": "Path to the FMSaveAsXML .xml export." }, "table": { "type": "string", "description": "Base table name (case-insensitive)." }, "fields": { "type": "array", "items": { "type": "string" }, "description": "Optional: return only these fields' full definitions." }, "summary": { "type": "boolean", "description": "Optional: one compact line per field instead of full definitions." } }, "required": ["xml_path", "table"] }
        },
        {
            "name": "get_field",
            "description": "Return ONE field's full definition inline: type, storage/indexing, auto-enter, and the complete <Validation> block (type, allowOverride, alwaysValidate, notEmpty, unique, existing, calc, message). This is the precise, small-output answer to 'what is this field / why can't I change it / what rule blocks this value' — use it instead of get_table (whole table) or reading the raw XML. Table and field are case-insensitive.",
            "inputSchema": { "type": "object", "properties": { "xml_path": { "type": "string", "description": "Path to the FMSaveAsXML .xml export." }, "table": { "type": "string", "description": "Base table name (case-insensitive)." }, "field": { "type": "string", "description": "Field name (case-insensitive)." } }, "required": ["xml_path", "table", "field"] }
        },
        {
            "name": "get_relationships",
            "description": "Describe relationships inline: base/left and right table occurrences plus every join predicate (leftField <op> rightField). This is how you resolve what a related table occurrence (e.g. one named in a validation or lookup like 'Count(recepMt::x)') actually joins on — without an inspect directory. Omit table_occurrence for all relationships; pass a TO name for every relationship touching it, or '#id' for one.",
            "inputSchema": { "type": "object", "properties": { "xml_path": { "type": "string", "description": "Path to the FMSaveAsXML .xml export." }, "table_occurrence": { "type": "string", "description": "Optional: a table-occurrence name (all relationships touching it) or '#id' (one relationship)." } }, "required": ["xml_path"] }
        },
        {
            "name": "get_script",
            "description": "Return a single script's .fmscript text inline, by name (case-insensitive) or '#id', from a single FMSaveAsXML export. Same rendering as inspect, but no files written. Use describe_database first to get script names.",
            "inputSchema": { "type": "object", "properties": { "xml_path": { "type": "string", "description": "Path to the FMSaveAsXML .xml export." }, "script": { "type": "string", "description": "Script name or #id." } }, "required": ["xml_path", "script"] }
        },
        {
            "name": "get_layout",
            "description": "Return one layout's full structure inline, by name (case-insensitive) or '#id': base table occurrence, recursive objects (fields, buttons→script, portals with their contents), tooltips, web viewer URL/source calculations, and object + layout script triggers. No files written. Use describe_database first to get layout names.",
            "inputSchema": { "type": "object", "properties": { "xml_path": { "type": "string", "description": "Path to the FMSaveAsXML .xml export." }, "layout": { "type": "string", "description": "Layout name or #id." } }, "required": ["xml_path", "layout"] }
        },
        {
            "name": "inspect_database",
            "description": "Parse a FileMaker FMSaveAsXML export into a navigable inspection DIRECTORY ON DISK (tables, fields with calc/index, layouts, table occurrences, relationships, custom functions, scripts in folders) and return counts + output paths. Requires filesystem access to read the result; if you have none, use describe_database / get_table / get_script instead.",
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
            "name": "who_calls",
            "description": "List everything that fires a given script: other scripts (Perform Script), layout triggers, buttons and object triggers. Use before changing a script to know its blast radius.",
            "inputSchema": { "type": "object", "properties": { "xml_path": { "type": "string" }, "script": { "type": "string", "description": "Script name or #id." } }, "required": ["xml_path", "script"] }
        },
        {
            "name": "who_uses_field",
            "description": "Find where a field is referenced: layout placements, relationship join keys, Set Field steps, and calculation mentions (scripts, field calcs, custom functions). Accepts 'TableOccurrence::Field' or a bare 'Field'.",
            "inputSchema": { "type": "object", "properties": { "xml_path": { "type": "string" }, "field": { "type": "string", "description": "Field name, optionally 'TableOccurrence::Field'." } }, "required": ["xml_path", "field"] }
        },
        {
            "name": "list_databases",
            "description": "List the live databases configured in the workspace's .fm-bridge.toml, with the FMSaveAsXML export mapped to each one. Connects to nothing. Call this first on the live-data path: it tells you which logical database names the query tools accept, and which XML export describes each — so you can check a field's storage/calculation in the schema before spending a query on it.",
            "inputSchema": { "type": "object", "properties": { "config_path": { "type": "string", "description": "Optional: a path to start searching for .fm-bridge.toml from (default: current directory)." } } }
        },
        {
            "name": "query_table",
            "description": "Read live rows from a hosted FileMaker database over ODBC, without writing SQL: the engine composes and quotes the statement for you. `table` is a TABLE OCCURRENCE (the name that goes in a FROM clause), not a base table — get it from describe_database on this database's XML export. Read-only. Results are capped by the workspace row limit; check `truncated` before concluding anything about totals.",
            "inputSchema": { "type": "object", "properties": {
                "database": { "type": "string", "description": "Logical database name from .fm-bridge.toml (see list_databases)." },
                "table": { "type": "string", "description": "Table occurrence name." },
                "fields": { "type": "array", "items": { "type": "string" }, "description": "Optional: only these columns (default: all)." },
                "filter": { "type": "string", "description": "Optional WHERE body, without the WHERE keyword, e.g. \"status = 'open' AND qty > 0\". FileMaker often stores numeric-looking keys as text, so compare with quotes when unsure." },
                "order_by": { "type": "string", "description": "Optional ORDER BY body, without the keywords." },
                "limit": { "type": "integer", "description": "Optional row cap for this call (clamped to the workspace maximum)." },
                "config_path": { "type": "string" }
            }, "required": ["database", "table"] }
        },
        {
            "name": "count_rows",
            "description": "COUNT(*) over one table occurrence in a live database, with an optional filter. The cheap way to size a problem before pulling rows — and the right way to check whether a filter matches what you expect.",
            "inputSchema": { "type": "object", "properties": {
                "database": { "type": "string" },
                "table": { "type": "string", "description": "Table occurrence name." },
                "filter": { "type": "string", "description": "Optional WHERE body, without the WHERE keyword." },
                "config_path": { "type": "string" }
            }, "required": ["database", "table"] }
        },
        {
            "name": "query_sql",
            "description": "Run an arbitrary read-only SELECT against a live FileMaker database, for joins and aggregates that query_table cannot express. Rejected unless it is a single SELECT statement. FileMaker SQL notes: quote identifiers containing '_' or spaces with double quotes; row limits are 'FETCH FIRST n ROWS ONLY', never LIMIT; names in FROM are table occurrences. Beware unstored calculation fields — they are very slow over ODBC, and ones that call ExecuteSQL across files return '?' in an ODBC session, so query the source table in its own file instead.",
            "inputSchema": { "type": "object", "properties": {
                "database": { "type": "string" },
                "sql": { "type": "string", "description": "A single SELECT statement." },
                "config_path": { "type": "string" }
            }, "required": ["database", "sql"] }
        },
        {
            "name": "data_doctor",
            "description": "Diagnose the live-data path end to end: sidecar present, credentials resolvable, driver loadable, host reachable, account accepted, ODBC/JDBC sharing enabled on the file. Returns each check with a plain-language fix. Run this whenever a query fails for a reason that is not about the SQL — the answer is usually a missing driver or a missing 'Access via ODBC/JDBC' (fmxdbc) extended privilege, not a bug in the query.",
            "inputSchema": { "type": "object", "properties": {
                "database": { "type": "string", "description": "Optional: check just this one (default: all configured)." },
                "config_path": { "type": "string" }
            } }
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
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    // list_steps is the one tool not backed by handle_command.
    if name == "list_steps" {
        let text =
            serde_json::to_string_pretty(&steps::catalog()).unwrap_or_else(|_| "[]".to_string());
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
        "describe_database" => {
            cmd.command = "describe".to_string();
            cmd.xml_path = Some(arg_str(&args, "xml_path")?);
        }
        "get_table" => {
            cmd.command = "get_table".to_string();
            cmd.xml_path = Some(arg_str(&args, "xml_path")?);
            cmd.table = Some(arg_str(&args, "table")?);
            cmd.fields = args.get("fields").and_then(|v| v.as_array()).map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            });
            cmd.summary = args.get("summary").and_then(|v| v.as_bool());
        }
        "get_field" => {
            cmd.command = "get_field".to_string();
            cmd.xml_path = Some(arg_str(&args, "xml_path")?);
            cmd.table = Some(arg_str(&args, "table")?);
            cmd.field = Some(arg_str(&args, "field")?);
        }
        "get_relationships" => {
            cmd.command = "get_relationships".to_string();
            cmd.xml_path = Some(arg_str(&args, "xml_path")?);
            // Optional filter: a table-occurrence name or "#id".
            cmd.table = args
                .get("table_occurrence")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
        }
        "get_script" => {
            cmd.command = "get_script".to_string();
            cmd.xml_path = Some(arg_str(&args, "xml_path")?);
            cmd.script = Some(arg_str(&args, "script")?);
        }
        "get_layout" => {
            cmd.command = "get_layout".to_string();
            cmd.xml_path = Some(arg_str(&args, "xml_path")?);
            cmd.layout = Some(arg_str(&args, "layout")?);
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
        "who_calls" => {
            cmd.command = "who_calls".to_string();
            cmd.xml_path = Some(arg_str(&args, "xml_path")?);
            cmd.script = Some(arg_str(&args, "script")?);
        }
        "who_uses_field" => {
            cmd.command = "who_uses_field".to_string();
            cmd.xml_path = Some(arg_str(&args, "xml_path")?);
            cmd.field = Some(arg_str(&args, "field")?);
        }
        "list_databases" => {
            cmd.command = "data_databases".to_string();
            cmd.config_path = opt_str(&args, "config_path");
        }
        "data_doctor" => {
            cmd.command = "data_doctor".to_string();
            cmd.database = opt_str(&args, "database");
            cmd.config_path = opt_str(&args, "config_path");
        }
        "query_table" => {
            cmd.command = "data_query".to_string();
            cmd.database = Some(arg_str(&args, "database")?);
            cmd.table = Some(arg_str(&args, "table")?);
            cmd.fields = args.get("fields").and_then(|v| v.as_array()).map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            });
            cmd.filter = opt_str(&args, "filter");
            cmd.order_by = opt_str(&args, "order_by");
            cmd.limit = args
                .get("limit")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize);
            cmd.config_path = opt_str(&args, "config_path");
        }
        "count_rows" => {
            cmd.command = "data_count".to_string();
            cmd.database = Some(arg_str(&args, "database")?);
            cmd.table = Some(arg_str(&args, "table")?);
            cmd.filter = opt_str(&args, "filter");
            cmd.config_path = opt_str(&args, "config_path");
        }
        "query_sql" => {
            cmd.command = "data_sql".to_string();
            cmd.database = Some(arg_str(&args, "database")?);
            cmd.sql = Some(arg_str(&args, "sql")?);
            cmd.config_path = opt_str(&args, "config_path");
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
    Command::default()
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

fn opt_str(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(|s| s.to_string())
}

fn arg_strings(args: &Value, key: &str) -> Result<Vec<String>, RpcError> {
    let arr = args
        .get(key)
        .and_then(|v| v.as_array())
        .ok_or((-32602, format!("Missing or non-array argument: {}", key)))?;
    let out: Vec<String> = arr
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    if out.is_empty() {
        return Err((
            -32602,
            format!("Argument {} must be a non-empty string array", key),
        ));
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
            "describe_database",
            "get_table",
            "get_field",
            "get_relationships",
            "get_script",
            "get_layout",
            "inspect_database",
            "slice_inspect",
            "audit_database",
            "who_calls",
            "who_uses_field",
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
        assert!(
            parsed["errors"]
                .as_array()
                .unwrap()
                .iter()
                .any(|e| e["message"].as_str().unwrap().contains("never closed"))
        );
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
