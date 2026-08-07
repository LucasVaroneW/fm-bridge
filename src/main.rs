// fm-bridge — FileMaker script clipboard bridge.
// Core: XMSS ↔ plain text parsing, clipboard I/O, JSON protocol over stdio.
// No UI, no HTTP, no async. Procedural and minimal.

mod audit;
mod clipboard;
mod data;
mod data_config;
mod data_sql;
mod fmsavexml;
mod import_records;
mod mcp;
mod normalization;
#[cfg(windows)]
mod ole_clipboard;
mod slice;
mod step_dsl;
mod steps;
mod text_format;
mod xmss;
mod xref;

use serde::{Deserialize, Serialize};
use std::io::Read;

// ─── JSON protocol ───
// Stable API for the VS Code extension.
// New fields must be optional with skip_serializing_if.

#[derive(Serialize, Deserialize, Default)]
struct Command {
    command: String,
    #[serde(default)]
    script_text: Option<String>,
    // ── inspect / slice params (file-based commands, for AI/tooling) ──
    #[serde(default)]
    xml_path: Option<String>,
    #[serde(default)]
    output_dir: Option<String>,
    #[serde(default)]
    slice_dir: Option<String>,
    #[serde(default)]
    layouts: Option<Vec<String>>,
    // ── xref params (who_calls / who_uses_field) ──
    #[serde(default)]
    script: Option<String>,
    #[serde(default)]
    field: Option<String>,
    // ── inline-read params (get_table / get_layout / get_field) ──
    #[serde(default)]
    table: Option<String>,
    #[serde(default)]
    layout: Option<String>,
    /// get_table: return only these fields' full definitions (size control).
    #[serde(default)]
    fields: Option<Vec<String>>,
    /// get_table: one compact line per field instead of full definitions.
    #[serde(default)]
    summary: Option<bool>,
    // ── format style (reformat): "inline" | "indented" ──
    #[serde(default)]
    style: Option<String>,
    /// resolve_from: path to a FMSaveAsXML export to resolve layout IDs from.
    #[serde(default)]
    resolve_from: Option<String>,
    // ── live-data params (ODBC sidecar) ──
    /// Logical database name from `.fm-bridge.toml`.
    #[serde(default)]
    database: Option<String>,
    /// Raw SELECT for `data_sql` (validated as read-only before it is sent).
    #[serde(default)]
    sql: Option<String>,
    /// WHERE body for the structured read.
    #[serde(default)]
    filter: Option<String>,
    /// ORDER BY body for the structured read.
    #[serde(default)]
    order_by: Option<String>,
    /// Row cap for one call; clamped to `limits.max_rows`.
    #[serde(default)]
    limit: Option<usize>,
    /// Where to start looking for `.fm-bridge.toml` (default: cwd).
    #[serde(default)]
    config_path: Option<String>,
}

#[derive(Serialize)]
struct Response {
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    script_text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    /// 1-based source line of a parse error, for the editor to place a squiggle.
    #[serde(skip_serializing_if = "Option::is_none")]
    error_line: Option<usize>,
    /// All validation errors found (linter). Each carries its own line + message,
    /// so the editor can squiggle every problem at once. `error`/`error_line`
    /// mirror the first entry for older single-error consumers.
    #[serde(skip_serializing_if = "Option::is_none")]
    errors: Option<Vec<text_format::ParseError>>,
    /// Structured result payload for commands that produce more than text
    /// (e.g. `inspect`/`slice` return counts + output paths for an AI/tooling).
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

impl Response {
    fn ok() -> Self {
        Response {
            status: "ok".to_string(),
            script_text: None,
            error: None,
            version: None,
            error_line: None,
            errors: None,
            data: None,
        }
    }
    fn ok_text(text: String) -> Self {
        Response {
            status: "ok".to_string(),
            script_text: Some(text),
            error: None,
            version: None,
            error_line: None,
            errors: None,
            data: None,
        }
    }
    fn ok_data(data: serde_json::Value) -> Self {
        Response {
            status: "ok".to_string(),
            script_text: None,
            error: None,
            version: None,
            error_line: None,
            errors: None,
            data: Some(data),
        }
    }
    fn version(v: String) -> Self {
        Response {
            status: "ok".to_string(),
            script_text: None,
            error: None,
            version: Some(v),
            error_line: None,
            errors: None,
            data: None,
        }
    }
    fn error(message: String) -> Self {
        Response {
            status: "error".to_string(),
            script_text: None,
            error: Some(message),
            version: None,
            error_line: None,
            errors: None,
            data: None,
        }
    }
    /// Build an error response from a full list of validation errors. The first
    /// error is also mirrored into `error`/`error_line` for single-error clients.
    fn errors(errors: Vec<text_format::ParseError>) -> Self {
        let first = errors.first();
        let error = first.map(|e| e.to_string());
        let error_line = first.map(|e| e.line);
        // If all are warnings (no errors), the status is still ok.
        let has_errors = errors.iter().any(|e| e.severity == "error");
        Response {
            status: if has_errors {
                "error".to_string()
            } else {
                "ok".to_string()
            },
            script_text: None,
            error,
            version: None,
            error_line,
            errors: Some(errors),
            data: None,
        }
    }
}

fn handle_command(cmd: &Command) -> Response {
    match cmd.command.as_str() {
        "version" => Response::version(env!("CARGO_PKG_VERSION").to_string()),
        "read" => match clipboard::read_fm_clipboard() {
            Ok(data) => match xmss::decode_xmss(&data) {
                Ok(script) => Response::ok_text(text_format::format_script(&script)),
                Err(e) => Response::error(e),
            },
            Err(e) => Response::error(e),
        },
        // Validate-only: parse the text and report a positioned error, but do
        // NOT touch the clipboard. The editor calls this on every change (with a
        // debounce) to drive diagnostics, so it must be side-effect free.
        "parse" => {
            let script_text = match &cmd.script_text {
                Some(t) => t,
                None => return Response::error("No script_text provided".to_string()),
            };
            let errors = text_format::lint(script_text);
            if errors.is_empty() {
                // Run post_validate to catch warnings (missing layout IDs, etc.)
                match text_format::parse_text_to_script(script_text) {
                    Ok(script) => {
                        let warnings = text_format::post_validate(&script);
                        if warnings.is_empty() {
                            Response::ok()
                        } else {
                            Response::errors(warnings)
                        }
                    }
                    Err(pe) => Response::errors(vec![pe]),
                }
            } else {
                Response::errors(errors)
            }
        }
        // Parse `.fmscript` text into the structured step tree as JSON (#3), for
        // AI/tooling that wants to reason over fields, not just the flat text.
        // Side-effect free, like `parse`. On a format error, returns the
        // positioned error(s) instead of a tree.
        "to_json" => {
            let script_text = match &cmd.script_text {
                Some(t) => t,
                None => return Response::error("No script_text provided".to_string()),
            };
            match text_format::parse_text_to_script(script_text) {
                Ok(script) => Response::ok_data(
                    serde_json::to_value(&script).unwrap_or(serde_json::Value::Null),
                ),
                Err(pe) => Response::errors(vec![pe]),
            }
        }
        // Re-render a script in a different style without touching the clipboard.
        // `style` = "inline" (one line per step, matches FileMaker line numbers)
        // or "indented" (readable multi-line DSL, the default). Side-effect free.
        "reformat" => {
            let script_text = match &cmd.script_text {
                Some(t) => t,
                None => return Response::error("No script_text provided".to_string()),
            };
            let style = match cmd.style.as_deref() {
                Some("inline") => text_format::FormatStyle::Inline,
                _ => text_format::FormatStyle::Indented,
            };
            match text_format::parse_text_to_script(script_text) {
                Ok(script) => Response::ok_text(text_format::format_script_with(&script, style)),
                Err(pe) => Response::errors(vec![pe]),
            }
        }
        "write" => {
            let mut script_text = match &cmd.script_text {
                Some(t) => t.clone(),
                None => return Response::error("No script_text provided".to_string()),
            };
            // Auto-resolve layout IDs from a FMSaveAsXML export.
            if let Some(xml_path) = &cmd.resolve_from {
                match resolve_layout_ids_in_script(&script_text, xml_path) {
                    Ok(resolved) => script_text = resolved,
                    Err(e) => return Response::error(e),
                }
            }
            // Lint first: surface every format/structure error to the editor and
            // refuse to write a broken script to the clipboard.
            let errors = text_format::lint(&script_text);
            if !errors.is_empty() {
                return Response::errors(errors);
            }
            // Run post_validate for warnings
            match text_format::parse_text_to_script(&script_text) {
                Ok(script) => {
                    let warnings = text_format::post_validate(&script);
                    if !warnings.is_empty() {
                        return Response::errors(warnings);
                    }
                }
                Err(pe) => return Response::errors(vec![pe]),
            }
            match xmss::encode_xmss(&script_text) {
                Ok(xmss_data) => match clipboard::write_fm_clipboard(&xmss_data) {
                    Ok(()) => Response::ok(),
                    Err(e) => Response::error(e),
                },
                Err(e) => Response::error(e),
            }
        }
        // Parse a FMSaveAsXML export into a navigable inspection directory and
        // return the counts + output paths so an AI/agent can drive it headless.
        // Streaming and silent: the human-progress prints live in the CLI path,
        // not here, so stdout stays a single clean JSON object.
        "inspect" => {
            let xml_path = match &cmd.xml_path {
                Some(p) => p,
                None => return Response::error("No xml_path provided".to_string()),
            };
            let output_dir = cmd.output_dir.as_deref().unwrap_or("fm-inspect-output");
            match fmsavexml::parse(xml_path) {
                Ok(db) => match fmsavexml::write_inspection(&db, output_dir) {
                    Ok(stats) => {
                        let real_scripts = db
                            .scripts
                            .iter()
                            .filter(|s| !s.is_folder && !s.is_separator)
                            .count();
                        Response::ok_data(serde_json::json!({
                            "output_dir": output_dir,
                            "manifest": format!("{}/manifest.json", output_dir),
                            "file_name": db.file_name,
                            "scripts": real_scripts,
                            "scripts_written": stats.scripts_written,
                            "layouts": stats.layouts,
                            "tables": stats.tables,
                            "fields": stats.fields,
                            "table_occurrences": stats.table_occurrences,
                            "relationships": stats.relationships,
                            "external_sources": stats.external_sources,
                            "custom_functions": stats.custom_functions,
                            "unreferenced_scripts": stats.unreferenced_scripts,
                        }))
                    }
                    Err(e) => Response::error(e),
                },
                Err(e) => Response::error(e),
            }
        }
        // Audit a FMSaveAsXML export for broken references (dangling Perform
        // Script / Go to Layout targets, relationships and layouts pointing at
        // missing table occurrences, etc.). Returns the structured report for AI.
        "audit" => {
            let xml_path = match &cmd.xml_path {
                Some(p) => p,
                None => return Response::error("No xml_path provided".to_string()),
            };
            match fmsavexml::parse(xml_path) {
                Ok(db) => Response::ok_data(
                    serde_json::to_value(audit::audit(&db)).unwrap_or(serde_json::Value::Null),
                ),
                Err(e) => Response::error(e),
            }
        }
        // Inline read tools (no disk): orient on a single XML and pull one table
        // or one script's text without writing an inspect directory. The fix for
        // MCP clients that have no filesystem access to read inspect output.
        "describe" => {
            let xml_path = match &cmd.xml_path {
                Some(p) => p,
                None => return Response::error("No xml_path provided".to_string()),
            };
            match fmsavexml::parse(xml_path) {
                Ok(db) => Response::ok_data(fmsavexml::describe(&db)),
                Err(e) => Response::error(e),
            }
        }
        "get_table" => {
            let xml_path = match &cmd.xml_path {
                Some(p) => p,
                None => return Response::error("No xml_path provided".to_string()),
            };
            let table = match &cmd.table {
                Some(t) => t,
                None => return Response::error("No table provided".to_string()),
            };
            match fmsavexml::parse(xml_path) {
                Ok(db) => {
                    match fmsavexml::table_inline_opts(
                        &db,
                        table,
                        cmd.fields.as_deref(),
                        cmd.summary.unwrap_or(false),
                    ) {
                        Ok(v) => Response::ok_data(v),
                        Err(e) => Response::error(e),
                    }
                }
                Err(e) => Response::error(e),
            }
        }
        "get_field" => {
            let xml_path = match &cmd.xml_path {
                Some(p) => p,
                None => return Response::error("No xml_path provided".to_string()),
            };
            let table = match &cmd.table {
                Some(t) => t,
                None => return Response::error("No table provided".to_string()),
            };
            let field = match &cmd.field {
                Some(f) => f,
                None => return Response::error("No field provided".to_string()),
            };
            match fmsavexml::parse(xml_path) {
                Ok(db) => match fmsavexml::field_inline(&db, table, field) {
                    Ok(v) => Response::ok_data(v),
                    Err(e) => Response::error(e),
                },
                Err(e) => Response::error(e),
            }
        }
        "get_relationships" => {
            let xml_path = match &cmd.xml_path {
                Some(p) => p,
                None => return Response::error("No xml_path provided".to_string()),
            };
            match fmsavexml::parse(xml_path) {
                Ok(db) => match fmsavexml::relationships_inline(&db, cmd.table.as_deref()) {
                    Ok(v) => Response::ok_data(v),
                    Err(e) => Response::error(e),
                },
                Err(e) => Response::error(e),
            }
        }
        "get_script" => {
            let xml_path = match &cmd.xml_path {
                Some(p) => p,
                None => return Response::error("No xml_path provided".to_string()),
            };
            let script = match &cmd.script {
                Some(s) => s,
                None => return Response::error("No script provided".to_string()),
            };
            match fmsavexml::parse(xml_path) {
                Ok(db) => match fmsavexml::script_text_inline(&db, script) {
                    Ok(v) => Response::ok_data(v),
                    Err(e) => Response::error(e),
                },
                Err(e) => Response::error(e),
            }
        }
        "get_layout" => {
            let xml_path = match &cmd.xml_path {
                Some(p) => p,
                None => return Response::error("No xml_path provided".to_string()),
            };
            let layout = match &cmd.layout {
                Some(l) => l,
                None => return Response::error("No layout provided".to_string()),
            };
            match fmsavexml::parse(xml_path) {
                Ok(db) => match fmsavexml::layout_inline(&db, layout) {
                    Ok(v) => Response::ok_data(v),
                    Err(e) => Response::error(e),
                },
                Err(e) => Response::error(e),
            }
        }
        // Cross-reference queries (Phase 3 bug-hunting): who calls a script, and
        // where a field is used. Both parse the export, then answer structurally.
        "who_calls" => {
            let xml_path = match &cmd.xml_path {
                Some(p) => p,
                None => return Response::error("No xml_path provided".to_string()),
            };
            let script = match &cmd.script {
                Some(s) => s,
                None => return Response::error("No script provided".to_string()),
            };
            match fmsavexml::parse(xml_path) {
                Ok(db) => match xref::who_calls(&db, script) {
                    Ok(report) => Response::ok_data(
                        serde_json::to_value(report).unwrap_or(serde_json::Value::Null),
                    ),
                    Err(e) => Response::error(e),
                },
                Err(e) => Response::error(e),
            }
        }
        "who_uses_field" => {
            let xml_path = match &cmd.xml_path {
                Some(p) => p,
                None => return Response::error("No xml_path provided".to_string()),
            };
            let field = match &cmd.field {
                Some(f) => f,
                None => return Response::error("No field provided".to_string()),
            };
            match fmsavexml::parse(xml_path) {
                Ok(db) => Response::ok_data(
                    serde_json::to_value(xref::who_uses_field(&db, field))
                        .unwrap_or(serde_json::Value::Null),
                ),
                Err(e) => Response::error(e),
            }
        }
        // ── live data (ODBC sidecar) ──
        // Reads are deliberately unrestricted: a SELECT has nothing to undo, so
        // gating it would only get in the way of an investigation. The limits
        // that do apply (timeouts, row caps, one query at a time) protect the
        // server and the caller's context, not the data.
        "data_databases" => match data_config::DataConfig::discover(cmd.config_path.as_deref()) {
            Ok(cfg) => Response::ok_data(data::list_databases(&cfg)),
            Err(e) => Response::error(e),
        },
        "data_doctor" => match data_config::DataConfig::discover(cmd.config_path.as_deref()) {
            Ok(cfg) => Response::ok_data(data::doctor(&cfg, cmd.database.as_deref())),
            Err(e) => Response::error(e),
        },
        "data_query" => {
            let cfg = match data_config::DataConfig::discover(cmd.config_path.as_deref()) {
                Ok(c) => c,
                Err(e) => return Response::error(e),
            };
            let database = match &cmd.database {
                Some(d) => d,
                None => return Response::error("No database provided".to_string()),
            };
            let table = match &cmd.table {
                Some(t) => t,
                None => return Response::error("No table provided".to_string()),
            };
            match data::query_table(
                &cfg,
                database,
                table,
                cmd.fields.as_deref(),
                cmd.filter.as_deref(),
                cmd.order_by.as_deref(),
                cmd.limit,
            ) {
                Ok(v) => Response::ok_data(v),
                Err(e) => Response::error(e),
            }
        }
        "data_count" => {
            let cfg = match data_config::DataConfig::discover(cmd.config_path.as_deref()) {
                Ok(c) => c,
                Err(e) => return Response::error(e),
            };
            let database = match &cmd.database {
                Some(d) => d,
                None => return Response::error("No database provided".to_string()),
            };
            let table = match &cmd.table {
                Some(t) => t,
                None => return Response::error("No table provided".to_string()),
            };
            match data::count_rows(&cfg, database, table, cmd.filter.as_deref()) {
                Ok(v) => Response::ok_data(v),
                Err(e) => Response::error(e),
            }
        }
        "data_sql" => {
            let cfg = match data_config::DataConfig::discover(cmd.config_path.as_deref()) {
                Ok(c) => c,
                Err(e) => return Response::error(e),
            };
            let database = match &cmd.database {
                Some(d) => d,
                None => return Response::error("No database provided".to_string()),
            };
            let sql = match &cmd.sql {
                Some(s) => s,
                None => return Response::error("No sql provided".to_string()),
            };
            match data::query_sql(&cfg, database, sql) {
                Ok(v) => Response::ok_data(v),
                Err(e) => Response::error(e),
            }
        }
        // Build a focused slice from an existing inspect output. Returns the
        // closure counts + the slice_summary.md path for the AI to read next.
        "slice" => {
            let output_dir = match &cmd.output_dir {
                Some(p) => p,
                None => return Response::error("No output_dir provided".to_string()),
            };
            let slice_dir = match &cmd.slice_dir {
                Some(p) => p,
                None => return Response::error("No slice_dir provided".to_string()),
            };
            let layouts = match &cmd.layouts {
                Some(l) if !l.is_empty() => l,
                _ => return Response::error("No layouts provided".to_string()),
            };
            match slice::run_slice(output_dir, slice_dir, layouts) {
                Ok(stats) => {
                    let mut data = serde_json::to_value(&stats).unwrap_or(serde_json::Value::Null);
                    if let serde_json::Value::Object(ref mut m) = data {
                        m.insert("slice_dir".to_string(), slice_dir.clone().into());
                        m.insert(
                            "summary".to_string(),
                            format!("{}/slice_summary.md", slice_dir).into(),
                        );
                    }
                    Response::ok_data(data)
                }
                Err(e) => Response::error(e),
            }
        }
        // Resolve layout IDs in a script using a FMSaveAsXML export.
        "resolve_ids" => {
            let script_text = match &cmd.script_text {
                Some(t) => t,
                None => return Response::error("No script_text provided".to_string()),
            };
            let xml_path = match &cmd.xml_path {
                Some(p) => p,
                None => return Response::error("No xml_path provided".to_string()),
            };
            match resolve_layout_ids_in_script(script_text, xml_path) {
                Ok(resolved) => Response::ok_text(resolved),
                Err(e) => Response::error(e),
            }
        }
        _ => Response::error(format!("Unknown command: {}", cmd.command)),
    }
}

fn run_json_mode() -> Result<(), String> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| format!("Cannot read stdin: {}", e))?;
    let cmd: Command = serde_json::from_str(&input).map_err(|e| format!("Invalid JSON: {}", e))?;
    let response = handle_command(&cmd);
    let output = serde_json::to_string(&response)
        .map_err(|e| format!("Cannot serialize response: {}", e))?;
    print!("{}", output);
    Ok(())
}

/// Scan a FMSaveAsXML export for `<Layout id="..." name="...">` entries in
/// the LayoutCatalog section. Returns a name→id map.
fn scan_layout_catalog(
    xml_path: &str,
) -> Result<std::collections::HashMap<String, String>, String> {
    let bytes = std::fs::read(xml_path).map_err(|e| format!("Cannot read {}: {}", xml_path, e))?;
    let text = if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        let u16s: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16(&u16s).map_err(|e| format!("Invalid UTF-16 in {}: {}", xml_path, e))?
    } else {
        String::from_utf8(bytes).map_err(|e| format!("Invalid UTF-8 in {}: {}", xml_path, e))?
    };
    let mut map = std::collections::HashMap::new();
    let start = text.find("<LayoutCatalog").unwrap_or(0);
    let end = text[start..]
        .find("</LayoutCatalog>")
        .map(|p| start + p)
        .unwrap_or(text.len());
    let section = &text[start..end];
    let mut pos = 0;
    while let Some(tag_start) = section[pos..].find("<Layout ") {
        let abs = pos + tag_start;
        let tag_end = match section[abs..].find('>') {
            Some(p) => abs + p + 1,
            None => break,
        };
        let tag = &section[abs..tag_end];
        if let (Some(id), Some(name)) = (
            extract_xml_attrib(tag, "id"),
            extract_xml_attrib(tag, "name"),
        ) {
            map.insert(name.to_string(), id.to_string());
        }
        pos = abs + 1;
    }
    Ok(map)
}

fn extract_xml_attrib<'a>(tag: &'a str, attr: &str) -> Option<&'a str> {
    let pattern = format!(" {}=\"", attr);
    let start = tag.find(&pattern)?;
    let val_start = start + pattern.len();
    let end = tag[val_start..].find('"')?;
    Some(&tag[val_start..val_start + end])
}

fn resolve_layout_ids_in_script(script_text: &str, xml_path: &str) -> Result<String, String> {
    let catalog = scan_layout_catalog(xml_path)?;
    if catalog.is_empty() {
        return Err(format!("No layouts found in {}", xml_path));
    }
    let script =
        crate::text_format::parse_text_to_script(script_text).map_err(|e| e.to_string())?;
    let mut updated = false;
    let mut steps = script.steps.clone();
    for step in &mut steps {
        if step.layout_name.is_some() && step.layout_id.is_none() {
            let name = step.layout_name.as_ref().unwrap();
            if let Some(id) = catalog.get(name) {
                step.layout_id = Some(id.clone());
                updated = true;
            }
        }
    }
    if updated {
        Ok(crate::text_format::format_script(&crate::xmss::FmScript {
            steps,
        }))
    } else {
        Ok(script_text.to_string())
    }
}

// ─── CLI commands ───

fn run_cli_mode() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        return run_read_cli(None);
    }
    match args[0].as_str() {
        "read" => run_read_cli(args.get(1).map(|s| s.as_str())),
        "write" => {
            if args.len() < 2 {
                return Err("Usage: fm-bridge write <file.fmscript>".to_string());
            }
            run_write_cli(&args[1])
        }
        "json" => run_json_mode(),
        "debug" => run_debug_cli(),
        "test" => run_test_cli(),
        "passthrough" => run_passthrough_cli(),
        "dump-ids" => run_dump_ids_cli(),
        "steps" => run_steps_cli(),
        "encode-text" => {
            if args.len() < 3 {
                return Err("Usage: fm-bridge encode-text <in.fmscript> <out.xml>".to_string());
            }
            let text = read_file_to_string(&args[1])?;
            let xml_bytes = xmss::encode_xmss(&text)?;
            std::fs::write(&args[2], &xml_bytes).map_err(|e| e.to_string())?;
            println!("Wrote {} ({} bytes)", args[2], xml_bytes.len());
            Ok(())
        }
        "decode-xml" => {
            if args.len() < 2 {
                return Err("Usage: fm-bridge decode-xml <file.xml>".to_string());
            }
            let xml = std::fs::read_to_string(&args[1]).map_err(|e| e.to_string())?;
            let script = xmss::parse_fmxml_snippet(&xml)?;
            let text = text_format::format_script(&script);
            if let Some(out) = args.get(2) {
                std::fs::write(out, &text).map_err(|e| e.to_string())?;
                println!("Wrote {}", out);
            } else {
                println!("{}", text);
            }
            Ok(())
        }
        "inspect" => run_inspect_cli(&args[1..]),
        "slice" => run_slice_cli(&args[1..]),
        "audit" => run_audit_cli(&args[1..]),
        "who-calls" => run_who_calls_cli(&args[1..]),
        "who-uses-field" => run_who_uses_field_cli(&args[1..]),
        "reformat" => run_reformat_cli(&args[1..]),
        "describe" => run_describe_cli(&args[1..]),
        "get-table" => run_get_table_cli(&args[1..]),
        "get-field" => run_get_field_cli(&args[1..]),
        "get-relationships" => run_get_relationships_cli(&args[1..]),
        "get-script" => run_get_script_cli(&args[1..]),
        "get-layout" => run_get_layout_cli(&args[1..]),
        "data" => run_data_cli(&args[1..]),
        "mcp" => mcp::run(),
        _ => Err(format!(
            "Unknown command: {}. Use: read, write, json, mcp, steps, debug, test, passthrough, dump-ids, inspect, slice, audit, who-calls, who-uses-field, describe, get-table, get-field, get-relationships, get-script, data",
            args[0]
        )),
    }
}

/// `fm-bridge data …` — the live-data path. Everything here is read-only.
fn run_data_cli(args: &[String]) -> Result<(), String> {
    const USAGE: &str = "Usage:\n  \
        fm-bridge data list\n  \
        fm-bridge data doctor [database]\n  \
        fm-bridge data login <server>\n  \
        fm-bridge data query <database> <table> [filter]\n  \
        fm-bridge data count <database> <table> [filter]\n  \
        fm-bridge data sql <database> \"<SELECT …>\"";
    let sub = match args.first() {
        Some(s) => s.as_str(),
        None => return Err(USAGE.to_string()),
    };

    // `login` writes the credentials file; it never touches a project config.
    if sub == "login" {
        let server = args
            .get(1)
            .ok_or_else(|| "Usage: fm-bridge data login <server>".to_string())?;
        return run_data_login(server);
    }

    let mut cmd = Command::default();
    match sub {
        "list" => cmd.command = "data_databases".to_string(),
        "doctor" => {
            cmd.command = "data_doctor".to_string();
            cmd.database = args.get(1).cloned();
        }
        "query" | "count" => {
            cmd.command = if sub == "query" {
                "data_query".to_string()
            } else {
                "data_count".to_string()
            };
            cmd.database = Some(args.get(1).cloned().ok_or_else(|| USAGE.to_string())?);
            cmd.table = Some(args.get(2).cloned().ok_or_else(|| USAGE.to_string())?);
            cmd.filter = args.get(3).cloned();
        }
        "sql" => {
            cmd.command = "data_sql".to_string();
            cmd.database = Some(args.get(1).cloned().ok_or_else(|| USAGE.to_string())?);
            cmd.sql = Some(args.get(2).cloned().ok_or_else(|| USAGE.to_string())?);
        }
        other => return Err(format!("Unknown data subcommand: {}\n\n{}", other, USAGE)),
    }

    let resp = handle_command(&cmd);
    if resp.status == "error" {
        return Err(resp.error.unwrap_or_else(|| "unknown error".to_string()));
    }
    let data = resp.data.unwrap_or(serde_json::Value::Null);
    // Rows print as a table; anything else prints as JSON.
    if data.get("rows").is_some() {
        print_rows(&data);
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(&data).unwrap_or_else(|_| "{}".to_string())
        );
    }
    Ok(())
}

/// Store a password in the per-user credentials file, so it never lands in a
/// project file that gets committed.
fn run_data_login(server: &str) -> Result<(), String> {
    let path = data_config::credentials_path()
        .ok_or_else(|| "Cannot determine the user config directory.".to_string())?;

    eprint!("Password for server '{}': ", server);
    use std::io::Write as _;
    std::io::stderr().flush().ok();
    let mut password = String::new();
    std::io::stdin()
        .read_line(&mut password)
        .map_err(|e| format!("cannot read password: {}", e))?;
    let password = password.trim_end_matches(['\r', '\n']).to_string();
    if password.is_empty() {
        return Err("Empty password; nothing stored.".to_string());
    }

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {}", dir.display(), e))?;
    }
    // Merge into whatever is already there rather than clobbering other servers.
    let mut doc: toml::Table = if path.is_file() {
        let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        toml::from_str(&text).map_err(|e| format!("cannot parse {}: {}", path.display(), e))?
    } else {
        toml::Table::new()
    };
    let mut entry = toml::Table::new();
    entry.insert("password".to_string(), toml::Value::String(password));
    doc.insert(server.to_string(), toml::Value::Table(entry));
    std::fs::write(
        &path,
        toml::to_string_pretty(&doc).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("cannot write {}: {}", path.display(), e))?;

    println!("Stored the password for '{}' in {}", server, path.display());
    println!(
        "Note: this file is plain text, readable by your user account. \
         Prefer a FileMaker account with read-only privileges."
    );
    Ok(())
}

/// Widest a column is allowed to get on screen. Without this, one 500-char
/// cell pads *every* row in that column to 500 characters, so a wide FileMaker
/// table prints tens of kilobytes of spaces.
const MAX_DISPLAY_WIDTH: usize = 40;

/// Print a result set as an aligned table.
fn print_rows(data: &serde_json::Value) {
    let empty = vec![];
    let columns: Vec<String> = data
        .get("columns")
        .and_then(|c| c.as_array())
        .unwrap_or(&empty)
        .iter()
        .map(|c| c.as_str().unwrap_or("?").to_string())
        .collect();
    let rows: Vec<Vec<String>> = data
        .get("rows")
        .and_then(|r| r.as_array())
        .unwrap_or(&empty)
        .iter()
        .map(|row| {
            row.as_array()
                .unwrap_or(&empty)
                .iter()
                .map(|cell| match cell.as_str() {
                    Some(s) => s.to_string(),
                    None => "<null>".to_string(),
                })
                .collect()
        })
        .collect();

    // Elide anything longer than the display cap before measuring, so a single
    // long value cannot widen the whole column.
    let elide = |s: &str| -> String {
        if s.chars().count() > MAX_DISPLAY_WIDTH {
            s.chars().take(MAX_DISPLAY_WIDTH - 1).collect::<String>() + "…"
        } else {
            s.to_string()
        }
    };
    let columns: Vec<String> = columns.iter().map(|c| elide(c)).collect();
    let rows: Vec<Vec<String>> = rows
        .iter()
        .map(|r| r.iter().map(|c| elide(c)).collect())
        .collect();

    let mut widths: Vec<usize> = columns.iter().map(|c| c.chars().count()).collect();
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(cell.chars().count());
            }
        }
    }
    let line = |cells: &[String]| -> String {
        cells
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{:width$}", c, width = widths.get(i).copied().unwrap_or(0)))
            .collect::<Vec<_>>()
            .join(" | ")
    };
    println!("{}", line(&columns));
    println!(
        "{}",
        "-".repeat(widths.iter().sum::<usize>() + 3 * widths.len())
    );
    for row in &rows {
        println!("{}", line(row));
    }
    let count = data.get("row_count").and_then(|v| v.as_u64()).unwrap_or(0);
    let ms = data.get("elapsed_ms").and_then(|v| v.as_u64()).unwrap_or(0);
    let truncated = data
        .get("truncated")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    println!(
        "{} row(s) in {} ms{}",
        count,
        ms,
        if truncated {
            // Deliberately not naming which limit: it may be the row cap or the
            // total-size budget, and claiming the wrong one sends people to the
            // wrong setting.
            "  [TRUNCATED — a size limit was reached; this is NOT the full result]"
        } else {
            ""
        }
    );
}

/// Parse an `FMSaveAsXML` database export and write a navigable inspection
/// directory (scripts, layouts, tables, TOs, relationships, custom functions,
/// cross-reference analysis). Streaming, handles 100MB+ UTF-16 exports.
fn run_inspect_cli(args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        return Err("Usage: fm-bridge inspect <FMSaveAsXML.xml> [output-dir]".to_string());
    }
    let xml_path = &args[0];
    let output_dir = args
        .get(1)
        .map(|s| s.as_str())
        .unwrap_or("fm-inspect-output");

    println!("Parsing {}...", xml_path);
    let db = fmsavexml::parse(xml_path)?;

    println!(
        "  Scripts: {}  |  Layouts: {}  |  Tables: {}",
        db.scripts
            .iter()
            .filter(|s| !s.is_folder && !s.is_separator)
            .count(),
        db.layouts.len(),
        db.tables.len(),
    );

    println!("Writing to {}...", output_dir);
    let stats = fmsavexml::write_inspection(&db, output_dir)?;

    println!(
        "Done.\n  Scripts exported       : {}\n  Layouts indexed        : {}\n  Tables (base) indexed  : {}\n  Fields (base) indexed  : {}\n  Table occurrences      : {}\n  Relationships          : {}\n  External data sources  : {}\n  Custom functions       : {}\n  Unreferenced scripts   : {}",
        stats.scripts_written,
        stats.layouts,
        stats.tables,
        stats.fields,
        stats.table_occurrences,
        stats.relationships,
        stats.external_sources,
        stats.custom_functions,
        stats.unreferenced_scripts,
    );
    Ok(())
}

/// From an existing `inspect` output, build a focused slice around one or more
/// layouts: transitive closure of triggered scripts, referenced TOs, relations,
/// and custom functions. Pares a 150MB export down to ~30 files for an AI.
fn run_slice_cli(args: &[String]) -> Result<(), String> {
    if args.len() < 3 {
        return Err(
            "Usage: fm-bridge slice <output-dir> <slice-dir> <layout-name> [layout-name…]"
                .to_string(),
        );
    }
    let output_dir = &args[0];
    let slice_dir = &args[1];
    let layouts: Vec<String> = args[2..].to_vec();
    println!("Slicing {} layout(s)...", layouts.len());
    let stats = slice::run_slice(output_dir, slice_dir, &layouts)?;
    println!(
        "Slice written to {}\n  Layouts                : {}\n  Scripts (seed)         : {}\n  Scripts (closure)      : {}\n  Table occurrences      : {}\n  Relationships          : {}\n  Custom functions       : {}\n  External data sources  : {}",
        slice_dir,
        stats.layouts,
        stats.scripts_seed,
        stats.scripts_closure,
        stats.table_occurrences,
        stats.relationships,
        stats.custom_functions,
        stats.external_sources,
    );
    Ok(())
}

/// Audit a FMSaveAsXML export for broken references and print a human report.
/// Exit is still 0 (it's a report, not a failure); the issues are the output.
fn run_audit_cli(args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        return Err("Usage: fm-bridge audit <FMSaveAsXML.xml>".to_string());
    }
    println!("Parsing {}...", args[0]);
    let db = fmsavexml::parse(&args[0])?;
    let report = audit::audit(&db);

    if report.issue_count == 0 {
        println!("No broken references found in {}. ✓", report.file_name);
        return Ok(());
    }

    println!("\n{} issue(s) in {}:", report.issue_count, report.file_name);
    let mut kinds: Vec<(&String, &usize)> = report.by_kind.iter().collect();
    kinds.sort();
    for (kind, n) in kinds {
        println!("  {:4}  {}", n, kind);
    }
    println!();
    for issue in &report.issues {
        println!("  [{}] {} — {}", issue.kind, issue.location, issue.detail);
    }
    Ok(())
}

/// `who-calls`: list everything that fires a given script (Perform Script,
/// layout triggers, buttons, object triggers).
fn run_who_calls_cli(args: &[String]) -> Result<(), String> {
    if args.len() < 2 {
        return Err("Usage: fm-bridge who-calls <FMSaveAsXML.xml> <script-name|#id>".to_string());
    }
    let db = fmsavexml::parse(&args[0])?;
    let report = xref::who_calls(&db, &args[1])?;
    if report.caller_count == 0 {
        println!(
            "Nothing calls '{}' (#{}).",
            report.target_name, report.target_id
        );
        return Ok(());
    }
    println!(
        "{} caller(s) of '{}' (#{}):",
        report.caller_count, report.target_name, report.target_id
    );
    for c in &report.callers {
        println!("  {:24}  {}", c.via, c.location);
    }
    Ok(())
}

/// `who-uses-field`: where a field is referenced — layouts, relationship keys,
/// Set Field steps, and calculation mentions.
fn run_who_uses_field_cli(args: &[String]) -> Result<(), String> {
    if args.len() < 2 {
        return Err(
            "Usage: fm-bridge who-uses-field <FMSaveAsXML.xml> <Field|TableOccurrence::Field>"
                .to_string(),
        );
    }
    let db = fmsavexml::parse(&args[0])?;
    let report = xref::who_uses_field(&db, &args[1]);
    if report.use_count == 0 {
        println!("No uses found for '{}'.", report.field);
        return Ok(());
    }
    println!("{} use(s) of '{}':", report.use_count, report.field);
    for u in &report.uses {
        println!("  [{}] {} — {}", u.kind, u.location, u.detail);
    }
    Ok(())
}

/// `reformat`: re-render a .fmscript in `inline` or `indented` style. Prints to
/// stdout, or writes to a third-arg file. Round-trips to the same clipboard XML.
fn run_reformat_cli(args: &[String]) -> Result<(), String> {
    if args.len() < 2 {
        return Err(
            "Usage: fm-bridge reformat <file.fmscript> <inline|indented> [out.fmscript]"
                .to_string(),
        );
    }
    let style = match args[1].as_str() {
        "inline" => text_format::FormatStyle::Inline,
        "indented" | "indent" => text_format::FormatStyle::Indented,
        other => {
            return Err(format!(
                "Unknown style '{}'. Use inline or indented.",
                other
            ));
        }
    };
    let text = read_file_to_string(&args[0])?;
    let script = text_format::parse_text_to_script(&text).map_err(|e| e.to_string())?;
    let out = text_format::format_script_with(&script, style);
    if let Some(path) = args.get(2) {
        std::fs::write(path, &out).map_err(|e| e.to_string())?;
        println!("Wrote {}", path);
    } else {
        println!("{}", out);
    }
    Ok(())
}

/// `describe`: inline overview of a database (counts + names of tables,
/// scripts, layouts, custom functions, external sources) as pretty JSON.
fn run_describe_cli(args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        return Err("Usage: fm-bridge describe <FMSaveAsXML.xml>".to_string());
    }
    let db = fmsavexml::parse(&args[0])?;
    println!(
        "{}",
        serde_json::to_string_pretty(&fmsavexml::describe(&db)).map_err(|e| e.to_string())?
    );
    Ok(())
}

/// `get-table`: one table's field definitions as pretty JSON. Size control:
/// `--summary` for one line per field, or a list of field names for just those.
fn run_get_table_cli(args: &[String]) -> Result<(), String> {
    if args.len() < 2 {
        return Err(
            "Usage: fm-bridge get-table <FMSaveAsXML.xml> <TableName> [--summary | FieldName…]"
                .to_string(),
        );
    }
    let db = fmsavexml::parse(&args[0])?;
    let rest = &args[2..];
    let summary = rest.iter().any(|a| a == "--summary");
    let field_filter: Vec<String> = rest
        .iter()
        .filter(|a| !a.starts_with("--"))
        .cloned()
        .collect();
    let fields = if field_filter.is_empty() {
        None
    } else {
        Some(field_filter.as_slice())
    };
    let table = fmsavexml::table_inline_opts(&db, &args[1], fields, summary)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&table).map_err(|e| e.to_string())?
    );
    Ok(())
}

/// `get-field`: a single field's full definition (type, storage, auto-enter,
/// validation) as pretty JSON — the small, precise answer to "why does this
/// field behave this way".
fn run_get_field_cli(args: &[String]) -> Result<(), String> {
    if args.len() < 3 {
        return Err(
            "Usage: fm-bridge get-field <FMSaveAsXML.xml> <TableName> <FieldName>".to_string(),
        );
    }
    let db = fmsavexml::parse(&args[0])?;
    let data = fmsavexml::field_inline(&db, &args[1], &args[2])?;
    println!(
        "{}",
        serde_json::to_string_pretty(&data).map_err(|e| e.to_string())?
    );
    Ok(())
}

/// `get-relationships`: relationships as a compact list — all of them, or (with
/// a second arg) those touching one table occurrence, or `#id` for one.
fn run_get_relationships_cli(args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        return Err(
            "Usage: fm-bridge get-relationships <FMSaveAsXML.xml> [TableOccurrence|#id]"
                .to_string(),
        );
    }
    let db = fmsavexml::parse(&args[0])?;
    let data = fmsavexml::relationships_inline(&db, args.get(1).map(|s| s.as_str()))?;
    println!(
        "{}",
        serde_json::to_string_pretty(&data).map_err(|e| e.to_string())?
    );
    Ok(())
}

/// `get-layout`: one layout's full structure (objects, fields, web viewer URLs,
/// triggers) as pretty JSON, by name or `#id`.
fn run_get_layout_cli(args: &[String]) -> Result<(), String> {
    if args.len() < 2 {
        return Err("Usage: fm-bridge get-layout <FMSaveAsXML.xml> <layout-name|#id>".to_string());
    }
    let db = fmsavexml::parse(&args[0])?;
    let data = fmsavexml::layout_inline(&db, &args[1])?;
    println!(
        "{}",
        serde_json::to_string_pretty(&data).map_err(|e| e.to_string())?
    );
    Ok(())
}

/// `get-script`: one script's `.fmscript` text, by name or `#id`.
fn run_get_script_cli(args: &[String]) -> Result<(), String> {
    if args.len() < 2 {
        return Err("Usage: fm-bridge get-script <FMSaveAsXML.xml> <script-name|#id>".to_string());
    }
    let db = fmsavexml::parse(&args[0])?;
    let data = fmsavexml::script_text_inline(&db, &args[1])?;
    // Print just the script text for CLI ergonomics (the JSON is for the protocol).
    if let Some(text) = data.get("script_text").and_then(|v| v.as_str()) {
        println!("{}", text);
    }
    Ok(())
}

/// Read a text file with encoding detection.
/// Tries: UTF-8 (with/without BOM), UTF-16 LE (PowerShell >), UTF-16 BE, then Windows-1252.
fn read_file_to_string(path: &str) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Cannot read file {}: {}", path, e))?;

    if bytes.is_empty() {
        return Ok(String::new());
    }

    // UTF-16 LE BOM (FF FE) — PowerShell's > operator produces this
    if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        let u16s: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16(&u16s)
            .map_err(|e| format!("Invalid UTF-16 LE in {}: {}", path, e));
    }

    // UTF-16 BE BOM (FE FF)
    if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        let u16s: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16(&u16s)
            .map_err(|e| format!("Invalid UTF-16 BE in {}: {}", path, e));
    }

    // Try UTF-8 (handles both with and without BOM)
    if let Ok(s) = std::str::from_utf8(&bytes) {
        return Ok(s.strip_prefix('\u{FEFF}').unwrap_or(s).to_string());
    }

    // Fallback: Windows-1252 (covers Latin-1 accented characters)
    Ok(crate::xmss::decode_windows1252(&bytes))
}

fn run_read_cli(output_path: Option<&str>) -> Result<(), String> {
    let data = clipboard::read_fm_clipboard()?;
    let script = xmss::decode_xmss(&data)?;
    let text = text_format::format_script(&script);
    if let Some(path) = output_path {
        std::fs::write(path, &text).map_err(|e| format!("Cannot write file {}: {}", path, e))?;
        println!("Script written to {}", path);
    } else {
        println!("{}", text);
    }
    Ok(())
}

fn run_write_cli(file_path: &str) -> Result<(), String> {
    let text = read_file_to_string(file_path)?;
    let xmss_data = xmss::encode_xmss(&text)?;
    clipboard::write_fm_clipboard(&xmss_data)?;
    println!("Script written to clipboard from {}", file_path);
    Ok(())
}

fn run_debug_cli() -> Result<(), String> {
    let formats = clipboard::list_clipboard_formats();
    println!("=== Clipboard formats ({} total) ===", formats.len());
    for (fmt, name, fmt_size) in &formats {
        println!(
            "  ID: {:5}  Name: {:30}  Size: {} bytes",
            fmt, name, fmt_size
        );
    }

    let data = clipboard::read_fm_clipboard()?;
    println!("\n=== FM data ({} bytes) ===", data.len());
    println!(
        "Header: {:02x} {:02x} {:02x} {:02x}",
        data[0], data[1], data[2], data[3]
    );

    let xml_str = xmss::strip_header(&data)?;
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let output_path = manifest_dir.join("debug_raw.xml");
    std::fs::write(&output_path, &xml_str)
        .map_err(|e| format!("Cannot write {}: {}", output_path.display(), e))?;
    println!("\nRaw XML saved to: {}", output_path.display());

    let script = xmss::parse_fmxml_snippet(&xml_str)?;
    let built_xml = xmss::build_xml_from_script(&script)?;
    let built_path = manifest_dir.join("debug_built.xml");
    std::fs::write(&built_path, &built_xml)
        .map_err(|e| format!("Cannot write {}: {}", built_path.display(), e))?;
    println!("Built XML saved to: {}", built_path.display());

    println!("\n=== DECODED SCRIPT ===\n");
    println!("{}", text_format::format_script(&script));
    Ok(())
}

fn run_dump_ids_cli() -> Result<(), String> {
    let data = clipboard::read_fm_clipboard()?;
    let script = xmss::decode_xmss(&data)?;
    for step in &script.steps {
        println!("{}\t{}", step.id, step.name);
    }
    Ok(())
}

/// Emit the full step catalog as JSON. This is the single source of truth the
/// VS Code extension reads for autocomplete (step names, shapes, block
/// behavior), so the extension never drifts from the installed binary.
fn run_steps_cli() -> Result<(), String> {
    let catalog = steps::catalog();
    let json = serde_json::to_string_pretty(&catalog)
        .map_err(|e| format!("Cannot serialize step catalog: {}", e))?;
    println!("{}", json);
    Ok(())
}

fn run_passthrough_cli() -> Result<(), String> {
    let data = clipboard::read_fm_clipboard()?;
    println!("Read {} bytes from clipboard", data.len());
    println!(
        "Header: {:02X} {:02X} {:02X} {:02X}",
        data[0], data[1], data[2], data[3]
    );

    // Also save raw bytes for comparison
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let raw_path = manifest_dir.join("clipboard_raw.bin");
    std::fs::write(&raw_path, &data).map_err(|e| format!("Cannot save raw data: {}", e))?;
    println!("Raw bytes saved to: {}", raw_path.display());

    // On Windows, read_fm_clipboard returns bytes WITH the 4-byte LE length header
    // that FM puts on the HGLOBAL, and write_fm_clipboard prepends ITS OWN header —
    // so for a true passthrough we must strip FM's header first, otherwise we'd
    // produce a doubly-framed buffer that FM rejects on paste. On macOS the data is
    // raw XML (no header) and starts with `<`, so stripping 4 bytes would corrupt it.
    // Detect which case we're in by the leading byte.
    let xml_bytes = if data.len() > 4 && data[0] != b'<' {
        &data[4..]
    } else {
        &data[..]
    };
    clipboard::write_fm_clipboard(xml_bytes)?;
    println!(
        "Wrote {} bytes of XML back to clipboard (header re-added by write).",
        xml_bytes.len()
    );
    println!("Now try pasting in FileMaker.");
    Ok(())
}

fn run_test_cli() -> Result<(), String> {
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let test_file = manifest_dir.join("scripts").join("test_script.fmscript");

    println!("=== ROUNDTRIP TEST ===\n");
    println!("Input file: {}\n", test_file.display());

    let input_text = std::fs::read_to_string(&test_file)
        .map_err(|e| format!("Cannot read {}: {}", test_file.display(), e))?;

    println!("--- INPUT TEXT ---");
    println!("{}", input_text);

    let xmss_data = xmss::encode_xmss(&input_text)?;
    let xml_path = manifest_dir.join("test_roundtrip.xml");
    let xml_str = std::str::from_utf8(&xmss_data)
        .map_err(|e| format!("Invalid UTF-8 in generated XML: {}", e))?;
    std::fs::write(&xml_path, xml_str)
        .map_err(|e| format!("Cannot write {}: {}", xml_path.display(), e))?;

    println!("\n--- GENERATED XML ({} bytes) ---", xmss_data.len());
    println!("Saved to: {}", xml_path.display());

    let decoded_script = xmss::decode_xmss(&xmss_data)?;
    let output_text = text_format::format_script(&decoded_script);

    println!("\n--- DECODED TEXT ---");
    println!("{}", output_text);

    let input_lines: Vec<&str> = input_text.lines().collect();
    let output_lines: Vec<&str> = output_text.lines().collect();

    println!("\n--- COMPARISON ---");
    println!("Input lines:  {}", input_lines.len());
    println!("Output lines: {}", output_lines.len());

    let mut all_match = true;
    let max_lines = input_lines.len().max(output_lines.len());
    for i in 0..max_lines {
        let inp = input_lines.get(i).unwrap_or(&"<missing>");
        let out = output_lines.get(i).unwrap_or(&"<missing>");
        if inp.trim() != out.trim() {
            println!("  Line {}: INPUT  >> {}<<", i + 1, inp);
            println!("  Line {}: OUTPUT >> {}<<", i + 1, out);
            all_match = false;
        }
    }

    if all_match {
        println!("\n*** ROUNDTRIP OK - All lines match ***");
    } else {
        println!("\n*** ROUNDTRIP FAILED - Lines differ ***");
    }

    Ok(())
}

#[cfg(windows)]
fn set_console_utf8() {
    // Without this, PowerShell decodes our stdout via the legacy OEM code page
    // (CP850 on Spanish Windows), turning ó → ├│ when captured with `>`.
    // Idempotent and harmless if stdout is already a file or pipe.
    unsafe {
        windows_sys::Win32::System::Console::SetConsoleOutputCP(65001);
    }
}

#[cfg(not(windows))]
fn set_console_utf8() {}

fn main() {
    set_console_utf8();
    let result = run_cli_mode();
    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"<FMSaveAsXML File="Test.fmp12">
  <BaseTableCatalog><BaseTable id="1" name="Contacts"/></BaseTableCatalog>
  <FieldsForTables>
    <BaseTableReference id="1" name="Contacts"/>
    <Field id="1" name="Name" fieldtype="Normal" datatype="Text"/>
  </FieldsForTables>
  <ScriptCatalog><Script id="10" name="DoThing"/></ScriptCatalog>
  <StepsForScripts><StepsForScript>
    <ScriptReference id="10" name="DoThing"/>
    <ObjectList><Step enable="True" id="1" name="Comment"><Text>hi</Text></Step></ObjectList>
  </StepsForScript></StepsForScripts>
</FMSaveAsXML>"#;

    fn cmd(command: &str) -> Command {
        Command {
            command: command.to_string(),
            ..Default::default()
        }
    }

    /// The JSON `inspect` command returns a clean ok+data payload with counts
    /// (and never prints progress to stdout — that's the CLI path's job).
    #[test]
    fn json_inspect_returns_structured_data() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("fmbridge-json-{}", nanos));
        std::fs::create_dir_all(&dir).unwrap();
        let xml = dir.join("export.xml");
        std::fs::write(&xml, FIXTURE).unwrap();
        let out = dir.join("out");

        let resp = handle_command(&Command {
            xml_path: Some(xml.to_string_lossy().into_owned()),
            output_dir: Some(out.to_string_lossy().into_owned()),
            ..cmd("inspect")
        });

        assert_eq!(resp.status, "ok");
        let data = resp.data.expect("inspect should return data");
        assert_eq!(data["file_name"], "Test.fmp12");
        assert_eq!(data["tables"], 1);
        assert_eq!(data["fields"], 1);
        assert_eq!(data["scripts"], 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Missing required params surface as structured errors, not panics.
    #[test]
    fn json_inspect_without_path_errors() {
        let resp = handle_command(&cmd("inspect"));
        assert_eq!(resp.status, "error");
        assert!(resp.error.unwrap().contains("xml_path"));
    }

    /// Write the FIXTURE to a fresh temp file and hand its path to a callback.
    fn with_fixture<T>(f: impl FnOnce(&str) -> T) -> T {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("fmbridge-inline-{}", nanos));
        std::fs::create_dir_all(&dir).unwrap();
        let xml = dir.join("export.xml");
        std::fs::write(&xml, FIXTURE).unwrap();
        let out = f(&xml.to_string_lossy());
        std::fs::remove_dir_all(&dir).ok();
        out
    }

    /// `describe` returns inline counts + names with no disk output.
    #[test]
    fn describe_returns_inline_overview() {
        with_fixture(|path| {
            let resp = handle_command(&Command {
                xml_path: Some(path.to_string()),
                ..cmd("describe")
            });
            assert_eq!(resp.status, "ok");
            let data = resp.data.unwrap();
            assert_eq!(data["file_name"], "Test.fmp12");
            assert_eq!(data["counts"]["tables"], 1);
            assert_eq!(data["tables"][0]["name"], "Contacts");
            assert_eq!(data["scripts"][0]["name"], "DoThing");
        });
    }

    /// `get_table` returns one table's fields inline; a miss suggests near matches.
    #[test]
    fn get_table_returns_fields_and_suggests_on_miss() {
        with_fixture(|path| {
            let ok = handle_command(&Command {
                xml_path: Some(path.to_string()),
                table: Some("contacts".to_string()), // case-insensitive
                ..cmd("get_table")
            });
            assert_eq!(ok.status, "ok");
            assert_eq!(ok.data.unwrap()["fields"][0]["name"], "Name");

            let miss = handle_command(&Command {
                xml_path: Some(path.to_string()),
                table: Some("Contact".to_string()),
                ..cmd("get_table")
            });
            assert_eq!(miss.status, "error");
            assert!(miss.error.unwrap().contains("Contacts"));
        });
    }

    /// `get_script` renders one script's .fmscript text inline, by name or #id.
    #[test]
    fn get_script_returns_text_by_name_and_id() {
        with_fixture(|path| {
            for q in ["DoThing", "#10"] {
                let resp = handle_command(&Command {
                    xml_path: Some(path.to_string()),
                    script: Some(q.to_string()),
                    ..cmd("get_script")
                });
                assert_eq!(resp.status, "ok", "query {}", q);
                let data = resp.data.unwrap();
                assert_eq!(data["name"], "DoThing");
                assert!(data["script_text"].as_str().unwrap().contains("hi"));
            }
        });
    }
}
