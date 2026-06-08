// fm-bridge — FileMaker script clipboard bridge.
// Core: XMSS ↔ plain text parsing, clipboard I/O, JSON protocol over stdio.
// No UI, no HTTP, no async. Procedural and minimal.

mod clipboard;
mod fmsavexml;
mod normalization;
#[cfg(windows)]
mod ole_clipboard;
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
}

fn handle_command(cmd: &Command) -> Response {
    match cmd.command.as_str() {
        "version" => Response {
            status: "ok".to_string(),
            script_text: None,
            error: None,
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
        },
        "read" => {
            match clipboard::read_fm_clipboard() {
                Ok(data) => {
                    match xmss::decode_xmss(&data) {
                        Ok(script) => {
                            let text = text_format::format_script(&script);
                            Response { status: "ok".to_string(), script_text: Some(text), error: None, version: None }
                        }
                        Err(e) => Response { status: "error".to_string(), script_text: None, error: Some(e), version: None }
                    }
                }
                Err(e) => Response { status: "error".to_string(), script_text: None, error: Some(e), version: None }
            }
        }
        "write" => {
            let script_text = match &cmd.script_text {
                Some(t) => t,
                None => return Response { status: "error".to_string(), script_text: None, error: Some("No script_text provided".to_string()), version: None }
            };
            match xmss::encode_xmss(script_text) {
                Ok(xmss_data) => {
                    match clipboard::write_fm_clipboard(&xmss_data) {
                        Ok(()) => Response { status: "ok".to_string(), script_text: None, error: None, version: None },
                        Err(e) => Response { status: "error".to_string(), script_text: None, error: Some(e), version: None }
                    }
                }
                Err(e) => Response { status: "error".to_string(), script_text: None, error: Some(e), version: None }
            }
        }
        _ => Response { status: "error".to_string(), script_text: None, error: Some(format!("Unknown command: {}", cmd.command)), version: None }
    }
}

fn run_json_mode() -> Result<(), String> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)
        .map_err(|e| format!("Cannot read stdin: {}", e))?;
    let cmd: Command = serde_json::from_str(&input)
        .map_err(|e| format!("Invalid JSON: {}", e))?;
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
        "encode-text" => {
            if args.len() < 3 { return Err("Usage: fm-bridge encode-text <in.fmscript> <out.xml>".to_string()); }
            let text = read_file_to_string(&args[1])?;
            let xml_bytes = xmss::encode_xmss(&text)?;
            std::fs::write(&args[2], &xml_bytes).map_err(|e| e.to_string())?;
            println!("Wrote {} ({} bytes)", args[2], xml_bytes.len());
            Ok(())
        }
        "decode-xml" => {
            if args.len() < 2 { return Err("Usage: fm-bridge decode-xml <file.xml>".to_string()); }
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
        _ => Err(format!("Unknown command: {}. Use: read, write, json, debug, test, passthrough, dump-ids, inspect", args[0]))
    }
}

/// Read a text file with encoding detection.
/// Tries: UTF-8 (with/without BOM), UTF-16 LE (PowerShell >), UTF-16 BE, then Windows-1252.
fn read_file_to_string(path: &str) -> Result<String, String> {
    let bytes = std::fs::read(path)
        .map_err(|e| format!("Cannot read file {}: {}", path, e))?;

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
        std::fs::write(path, &text)
            .map_err(|e| format!("Cannot write file {}: {}", path, e))?;
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
        println!("  ID: {:5}  Name: {:30}  Size: {} bytes", fmt, name, fmt_size);
    }

    let data = clipboard::read_fm_clipboard()?;
    println!("\n=== FM data ({} bytes) ===", data.len());
    println!("Header: {:02x} {:02x} {:02x} {:02x}", data[0], data[1], data[2], data[3]);

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

fn run_passthrough_cli() -> Result<(), String> {
    let data = clipboard::read_fm_clipboard()?;
    println!("Read {} bytes from clipboard", data.len());
    println!("Header: {:02X} {:02X} {:02X} {:02X}", data[0], data[1], data[2], data[3]);

    // Also save raw bytes for comparison
    let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let raw_path = manifest_dir.join("clipboard_raw.bin");
    std::fs::write(&raw_path, &data)
        .map_err(|e| format!("Cannot save raw data: {}", e))?;
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
    println!("Wrote {} bytes of XML back to clipboard (header re-added by write).", xml_bytes.len());
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

fn run_inspect_cli(args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        return Err("Usage: fm-bridge inspect <FMSaveAsXML.xml> [output-dir]".to_string());
    }
    let xml_path = &args[0];
    let output_dir = args.get(1).map(|s| s.as_str()).unwrap_or("fm-inspect-output");

    println!("Parsing {}...", xml_path);
    let db = fmsavexml::parse(xml_path)?;

    println!(
        "  Scripts: {}  |  Layouts: {}  |  Tables: {}",
        db.scripts.iter().filter(|s| !s.is_folder && !s.is_separator).count(),
        db.layouts.len(),
        db.tables.len(),
    );

    println!("Writing to {}...", output_dir);
    let stats = fmsavexml::write_inspection(&db, output_dir)?;

    println!(
        "Done.\n  Scripts exported : {}\n  Layouts indexed  : {}\n  Tables indexed   : {}\n  Fields indexed   : {}\n  Unreferenced scripts (analysis): {}",
        stats.scripts_written,
        stats.layouts,
        stats.tables,
        stats.fields,
        stats.unreferenced_scripts,
    );
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
