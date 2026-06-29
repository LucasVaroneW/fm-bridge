// fm-bridge — FileMaker script clipboard bridge.
// Core: XMSS ↔ plain text parsing, clipboard I/O, JSON protocol over stdio.
// No UI, no HTTP, no async. Procedural and minimal.

mod audit;
mod clipboard;
mod fmsavexml;
mod import_records;
mod mcp;
mod normalization;
#[cfg(windows)]
mod ole_clipboard;
mod slice;
mod steps;
mod text_format;
mod xmss;

use serde::{Deserialize, Serialize};
use std::io::Read;

// ─── JSON protocol ───
// Stable API for the VS Code extension.
// New fields must be optional with skip_serializing_if.

#[derive(Serialize, Deserialize)]
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
        Response {
            status: "error".to_string(),
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
                Response::ok()
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
                Ok(script) => {
                    Response::ok_data(serde_json::to_value(&script).unwrap_or(serde_json::Value::Null))
                }
                Err(pe) => Response::errors(vec![pe]),
            }
        }
        "write" => {
            let script_text = match &cmd.script_text {
                Some(t) => t,
                None => return Response::error("No script_text provided".to_string()),
            };
            // Lint first: surface every format/structure error to the editor and
            // refuse to write a broken script to the clipboard.
            let errors = text_format::lint(script_text);
            if !errors.is_empty() {
                return Response::errors(errors);
            }
            match xmss::encode_xmss(script_text) {
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
        "mcp" => mcp::run(),
        _ => Err(format!(
            "Unknown command: {}. Use: read, write, json, mcp, steps, debug, test, passthrough, dump-ids, inspect, slice, audit",
            args[0]
        )),
    }
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

    println!(
        "\n{} issue(s) in {}:",
        report.issue_count, report.file_name
    );
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
            script_text: None,
            xml_path: None,
            output_dir: None,
            slice_dir: None,
            layouts: None,
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
}
