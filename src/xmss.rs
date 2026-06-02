// XMSS decode/encode — parses FileMaker's clipboard XML format.
// Uses StepShape from steps.rs to drive serialization per step type.
// All calculations are treated as opaque CDATA — never escaped or modified.

use quick_xml::events::Event;
use quick_xml::Reader;
use serde::{Deserialize, Serialize};
use std::io::Cursor;

use crate::steps::{self, StepShape};

// XMSS clipboard payload starts with a little-endian u32 = byte length of the
// XML that follows. Total buffer size = 4 + declared_len. Empirically verified
// against captures from FileMaker (e.g. E2 5C 00 00 → 23778 bytes of XML).

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FmScript {
    pub steps: Vec<ScriptStep>,
}

/// A single criterion inside a Perform Find request: one field = one text value.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FindCriterion {
    pub table: String,
    pub field: String,
    pub text: String,
}

/// One row in a Perform Find query — either Include or Omit, with N criteria.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FindRequest {
    pub operation: String, // "Include" or "Omit"
    pub criteria: Vec<FindCriterion>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScriptStep {
    pub name: String,       // Always English canonical name
    pub enable: bool,
    pub id: u32,
    // Fields populated based on StepShape:
    pub text: Option<String>,
    pub calculation: Option<String>,
    pub var_name: Option<String>,
    pub repetition: Option<String>,
    pub object_name: Option<String>,
    pub function_name: Option<String>,
    pub parameters: Vec<String>,
    pub restore_state: Option<String>,
    pub set_state: Option<String>,
    pub dialog_title: Option<String>,
    pub dialog_message: Option<String>,
    pub dialog_buttons: Vec<String>,
    pub field_result: Option<String>,
    pub field_target: Option<String>,
    // For Set Field (FieldAndCalc shape): preserves <Field table=...> attribute.
    // `field_numeric_id` is kept for backward-compat decode only; we no longer emit it.
    pub field_table: Option<String>,
    pub field_numeric_id: Option<String>,
    // For Perform Script (PerformScript shape): target script + parent mode.
    pub script_target_name: Option<String>,
    pub script_target_id: Option<String>,
    pub current_script_mode: Option<String>,
    // For Go to Record/Request/Page (GoToRecord shape).
    pub goto_location: Option<String>,
    pub goto_exit_after_last: Option<String>,
    pub goto_no_interact: Option<String>,
    // For Select Window and Adjust Window.
    pub window_mode: Option<String>,       // SelectWindow: ByName/Current/First/Last/Next/Previous
    pub window_limit_current_file: Option<String>,  // SelectWindow: True/False
    pub window_state: Option<String>,      // AdjustWindow: ResizeToFit/Maximize/...
    // For Go to Layout (GoToLayoutNamed shape) and New Window (NewWindow shape).
    pub layout_name: Option<String>,
    pub layout_id: Option<String>, // optional numeric FM Layout id; preserves round-trip
    pub layout_destination: Option<String>, // SelectedLayout | OriginalLayout | byCalculation
    // For New Window (NewWindow shape): geometry + style. All values are calc expressions
    // (FM stores them as <Calculation>) so they can be literal numbers or `$variables`.
    pub window_height: Option<String>,
    pub window_width: Option<String>,
    pub window_top: Option<String>,
    pub window_left: Option<String>,
    pub window_style_name: Option<String>,  // Document | Floating | Dialog | Card
    // For Perform Find (PerformFind shape).
    pub find_requests: Vec<FindRequest>,
    // For Insert from URL (InsertFromUrl shape). Boolean flags stored as "True"/"False"
    // strings (matches FM's XML attribute form); curl_options is a calc expression.
    // `goto_no_interact` is reused for NoInteract (same semantics — suppress dialog).
    pub curl_options: Option<String>,
    pub dont_encode_url: Option<String>,
    pub verify_ssl: Option<String>,
    pub select_all_state: Option<String>,
    pub indent_level: u32,
}

/// Strip the platform framing from raw clipboard data and return decoded XML string.
///
/// Two framings exist:
///   - Windows: a 4-byte little-endian u32 length prefix + XML bytes (added by
///     `write_fm_clipboard` and present on data FM puts on the HGLOBAL).
///   - macOS: raw XML bytes with no prefix — NSPasteboard hands us the payload
///     verbatim, so it starts directly with `<`.
/// Falls back to Windows-1252 if the XML is not valid UTF-8.
pub fn strip_header(data: &[u8]) -> Result<String, String> {
    if data.is_empty() {
        return Err("Empty clipboard data".to_string());
    }

    // macOS style: no header, data is XML already.
    if data[0] == b'<' {
        return Ok(decode_xml_bytes(data));
    }

    // Windows style: 4-byte LE length prefix.
    if data.len() < 5 {
        return Err("Data too short to be valid XMSS".to_string());
    }

    let declared_len = u32::from_le_bytes([data[0], data[1], data[2], data[3]]) as usize;
    let xml_start = if declared_len + 4 == data.len() {
        4
    } else if data[4] == b'<' {
        // Header present, length doesn't match (truncation or odd source) — trust offset 4
        4
    } else {
        // No recognizable header — scan for XML start
        data.iter().position(|&b| b == b'<').unwrap_or(4)
    };

    let xml_bytes = &data[xml_start..];
    Ok(decode_xml_bytes(xml_bytes))
}

/// Decode XML bytes as UTF-8, falling back to Windows-1252 for legacy Latin-1 data.
fn decode_xml_bytes(xml_bytes: &[u8]) -> String {
    // Try strict UTF-8 first
    if let Ok(s) = std::str::from_utf8(xml_bytes) {
        return s.to_string();
    }
    // Fallback: decode as Windows-1252 (covers Latin-1 accented chars)
    decode_windows1252(xml_bytes)
}

/// Decode bytes as Windows-1252 (also covers Latin-1 / ISO-8859-1).
/// Bytes 0x00-0x7F → ASCII (same as UTF-8)
/// Bytes 0x80-0x9F → Windows-1252 specific characters
/// Bytes 0xA0-0xFF → Latin-1 direct Unicode mapping
pub fn decode_windows1252(data: &[u8]) -> String {
    data.iter().map(|&b| {
        if b < 0x80 {
            b as char
        } else {
            match b {
                0x80 => '\u{20AC}', // €
                0x82 => '\u{201A}', // ‚
                0x83 => '\u{0192}', // ƒ
                0x84 => '\u{201E}', // „
                0x85 => '\u{2026}', // …
                0x86 => '\u{2020}', // †
                0x87 => '\u{2021}', // ‡
                0x88 => '\u{02C6}', // ˆ
                0x89 => '\u{2030}', // ‰
                0x8A => '\u{0160}', // Š
                0x8B => '\u{2039}', // ‹
                0x8C => '\u{0152}', // Œ
                0x8E => '\u{017D}', // Ž
                0x91 => '\u{2018}', // '
                0x92 => '\u{2019}', // '
                0x93 => '\u{201C}', // "
                0x94 => '\u{201D}', // "
                0x95 => '\u{2022}', // •
                0x96 => '\u{2013}', // –
                0x97 => '\u{2014}', // —
                0x98 => '\u{02DC}', // ˜
                0x99 => '\u{2122}', // ™
                0x9A => '\u{0161}', // š
                0x9B => '\u{203A}', // ›
                0x9C => '\u{0153}', // œ
                0x9E => '\u{017E}', // ž
                0x9F => '\u{0178}', // Ÿ
                _ => b as char, // 0xA0-0xFF: Latin-1 direct mapping
            }
        }
    }).collect()
}

/// Strip BOM (U+FEFF) from a string, commonly found in FM clipboard data.
fn strip_bom(s: &str) -> &str {
    s.strip_prefix('\u{FEFF}').unwrap_or(s)
}

/// FileMaker stores line breaks inside calculations as CR (0x0D) on both Mac and Windows
/// (legacy classic-Mac convention). Editors treat CR-only as one line, which collapses
/// adjacent tokens (e.g. `1\ror\rnot` → `1ornot`). Normalize to `\n` so the text format
/// can detect multi-line content. Encode flips it back to CR before emitting CDATA.
fn normalize_eol(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

/// Inverse of `normalize_eol` for emitting calculations back into CDATA.
/// FM expects CR-only inside calc text regardless of platform.
pub fn cr_for_cdata(s: &str) -> String {
    s.replace("\r\n", "\r").replace('\n', "\r")
}

/// Parse an FM XML snippet string into a structured script.
/// Translates Spanish step names to English using the steps table.
pub fn parse_fmxml_snippet(xml: &str) -> Result<FmScript, String> {
    // Normalize to NFC once, here, so EVERY name, calculation, and comment shares
    // a single canonical Unicode form regardless of the source platform (macOS
    // emits NFD, Windows NFC). Doing it on the whole string — rather than per
    // attribute — keeps names and the references embedded in calculations
    // consistent with each other. Idempotent on already-NFC (Windows) input.
    // The reader and the opaque byte-slice capture both operate on this same
    // normalized string, so buffer offsets stay valid.
    let xml_clean = crate::normalization::to_nfc(strip_bom(xml));
    let mut reader = Reader::from_reader(Cursor::new(xml_clean.as_str()));
    // FM emits some elements as self-closing (<Field .../>) and others as
    // explicit pairs (<Field ...></Field>) inconsistently. Normalize both
    // into Start+End pairs so we only need one handler per element.
    reader.config_mut().expand_empty_elements = true;
    let mut buf = Vec::new();
    let mut steps: Vec<ScriptStep> = Vec::new();
    let mut parser = StepParser::default();
    let mut indent_level: u32 = 0;
    // For Opaque-shaped steps: byte offset where the step's inner XML begins
    // (right after the `<Step ...>` start tag). Some => current step is opaque.
    let mut opaque_inner_start: Option<usize> = None;

    loop {
        // Byte position before this event is read = start of the event's tag.
        let pos_before = reader.buffer_position() as usize;
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                match e.name().as_ref() {
                    b"Step" => {
                        parser = StepParser::default();
                        for attr in e.attributes().flatten() {
                            match attr.key.as_ref() {
                                b"name" => {
                                    let name = String::from_utf8_lossy(&attr.value).to_string();
                                    parser.name = strip_bom(&name).to_string();
                                }
                                b"enable" => parser.enable = attr.value.as_ref() == b"True",
                                b"id" => parser.id = String::from_utf8_lossy(&attr.value).to_string(),
                                _ => {}
                            }
                        }
                        // Opaque steps: remember where the inner XML starts so the
                        // whole thing can be captured verbatim at the </Step> end.
                        let en = steps::translate_to_en(&parser.name);
                        opaque_inner_start = if steps::shape_for_en(&en) == Some(&StepShape::Opaque) {
                            Some(reader.buffer_position() as usize)
                        } else {
                            None
                        };
                    }
                    b"Calculation" => {
                        let parent = parser.current_target().clone();
                        match parent {
                            // Calc is just a wrapper here — keep capturing into the parent field
                            // (FM nests <Title><Calculation>"text"</Calculation></Title> etc.)
                            TextTarget::RepetitionCalc
                            | TextTarget::ValueCalc
                            | TextTarget::DialogTitle
                            | TextTarget::DialogMessage
                            | TextTarget::DialogButton
                            | TextTarget::ObjectName
                            | TextTarget::FunctionName
                            | TextTarget::Param
                            | TextTarget::VarName
                            | TextTarget::CurlOptions
                            | TextTarget::WinHeight
                            | TextTarget::WinWidth
                            | TextTarget::WinTop
                            | TextTarget::WinLeft => {}
                            _ => {
                                parser.push_target(TextTarget::Calculation);
                            }
                        }
                    }
                    b"Text" => {
                        // <Text> inside a Perform Find <Criteria> is the search value, not the
                        // step's comment text. Route it to the current criterion.
                        if parser.in_find_criteria {
                            parser.push_target(TextTarget::FindCriterionText);
                        } else {
                            parser.push_target(TextTarget::Text);
                        }
                    }
                    b"LayoutDestination" => {
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"value" {
                                parser.layout_destination = String::from_utf8_lossy(&attr.value).to_string();
                            }
                        }
                    }
                    b"Layout" => {
                        for attr in e.attributes().flatten() {
                            let val = String::from_utf8_lossy(&attr.value).to_string();
                            match attr.key.as_ref() {
                                b"name" => parser.layout_name = val,
                                b"id" => parser.layout_id = val,
                                _ => {}
                            }
                        }
                    }
                    b"Height" => { parser.push_target(TextTarget::WinHeight); }
                    b"Width" => { parser.push_target(TextTarget::WinWidth); }
                    b"DistanceFromTop" => { parser.push_target(TextTarget::WinTop); }
                    b"DistanceFromLeft" => { parser.push_target(TextTarget::WinLeft); }
                    b"NewWndStyles" => {
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"Style" {
                                parser.window_style_name = String::from_utf8_lossy(&attr.value).to_string();
                            }
                        }
                    }
                    b"Query" => {
                        // Container; per-row state is initialized on RequestRow.
                    }
                    b"RequestRow" => {
                        let mut op = "Include".to_string();
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"operation" {
                                op = String::from_utf8_lossy(&attr.value).to_string();
                            }
                        }
                        parser.current_find_request = Some(FindRequest { operation: op, criteria: Vec::new() });
                    }
                    b"Criteria" => {
                        parser.in_find_criteria = true;
                        parser.current_find_criterion = Some(FindCriterion::default());
                    }
                    b"Name" => { parser.push_target(TextTarget::VarName); }
                    b"ObjectName" => { parser.push_target(TextTarget::ObjectName); }
                    b"FunctionName" => { parser.push_target(TextTarget::FunctionName); }
                    b"P" => {
                        parser.current_param.clear();
                        parser.push_target(TextTarget::Param);
                    }
                    b"Repetition" => {
                        parser.push_target(TextTarget::RepetitionCalc);
                    }
                    b"Value" => {
                        parser.push_target(TextTarget::ValueCalc);
                    }
                    b"Restore" => {
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"state" {
                                parser.restore_state = Some(String::from_utf8_lossy(&attr.value).to_string());
                            }
                        }
                    }
                    b"Field" => {
                        let mut tbl = String::new();
                        let mut nm = String::new();
                        let mut has_name_attr = false;
                        let mut numeric_id = String::new();
                        for attr in e.attributes().flatten() {
                            let val = String::from_utf8_lossy(&attr.value).to_string();
                            match attr.key.as_ref() {
                                b"table" => tbl = val,
                                b"id" => numeric_id = val,
                                b"name" => { nm = val; has_name_attr = true; }
                                _ => {}
                            }
                        }
                        if parser.in_find_criteria {
                            // <Field> inside <Criteria> is the criterion's target — route to it,
                            // NOT to the Set Field target fields.
                            if let Some(c) = parser.current_find_criterion.as_mut() {
                                c.table = tbl;
                                c.field = nm;
                            }
                        } else {
                            parser.field_table = tbl;
                            parser.field_numeric_id = numeric_id;
                            parser.field_target = nm;
                            if !has_name_attr {
                                // <Field>$var</Field> form used by Execute FileMaker Data API.
                                parser.push_target(TextTarget::FieldTextContent);
                            }
                        }
                    }
                    b"SelectAll" => {
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"state" {
                                parser.select_all_state = String::from_utf8_lossy(&attr.value).to_string();
                            }
                        }
                    }
                    b"DontEncodeURL" => {
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"state" {
                                parser.dont_encode_url = String::from_utf8_lossy(&attr.value).to_string();
                            }
                        }
                    }
                    b"VerifySSLCertificates" => {
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"state" {
                                parser.verify_ssl = String::from_utf8_lossy(&attr.value).to_string();
                            }
                        }
                    }
                    b"CURLOptions" => { parser.push_target(TextTarget::CurlOptions); }
                    b"Set" => {
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"state" {
                                parser.set_state = Some(String::from_utf8_lossy(&attr.value).to_string());
                            }
                        }
                        parser.push_target(TextTarget::SetState);
                    }
                    b"Script" => {
                        for attr in e.attributes().flatten() {
                            let val = String::from_utf8_lossy(&attr.value).to_string();
                            match attr.key.as_ref() {
                                b"id" => parser.script_target_id = val,
                                b"name" => parser.script_target_name = val,
                                _ => {}
                            }
                        }
                    }
                    b"CurrentScript" => {
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"value" {
                                parser.current_script_mode = String::from_utf8_lossy(&attr.value).to_string();
                            }
                        }
                    }
                    b"RowPageLocation" => {
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"value" {
                                parser.goto_location = String::from_utf8_lossy(&attr.value).to_string();
                            }
                        }
                    }
                    b"Exit" => {
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"state" {
                                parser.goto_exit_after_last = String::from_utf8_lossy(&attr.value).to_string();
                            }
                        }
                    }
                    b"Window" => {
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"value" {
                                parser.window_mode = String::from_utf8_lossy(&attr.value).to_string();
                            }
                        }
                    }
                    b"LimitToWindowsOfCurrentFile" => {
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"state" {
                                parser.window_limit_current_file = String::from_utf8_lossy(&attr.value).to_string();
                            }
                        }
                    }
                    b"WindowState" => {
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"value" {
                                parser.window_state = String::from_utf8_lossy(&attr.value).to_string();
                            }
                        }
                    }
                    b"NoInteract" => {
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"state" {
                                parser.goto_no_interact = String::from_utf8_lossy(&attr.value).to_string();
                            }
                        }
                    }
                    b"Title" => { parser.push_target(TextTarget::DialogTitle); }
                    b"Message" => { parser.push_target(TextTarget::DialogMessage); }
                    b"Button" => {
                        parser.current_button.clear();
                        parser.push_target(TextTarget::DialogButton);
                    }
                    b"Result" => { parser.push_target(TextTarget::FieldResult); }
                    b"TargetName" => { parser.push_target(TextTarget::FieldTarget); }
                    _ => {}
                }
            }
            Ok(Event::Text(ref e)) => {
                // `unescape()` resolves `&#13;`, `&amp;`, `&gt;` etc. to their literal
                // characters so the round-trip xml_escape on the encode side doesn't
                // double-escape them. CDATA is exempt — it's already literal.
                let raw = e.unescape().map(|c| c.into_owned())
                    .unwrap_or_else(|_| String::from_utf8_lossy(e.as_ref()).to_string());
                let text = normalize_eol(strip_bom(&raw));
                parser.capture(&text);
            }
            Ok(Event::CData(ref e)) => {
                let text = String::from_utf8_lossy(e.as_ref()).to_string();
                let text = normalize_eol(strip_bom(&text));
                parser.capture(&text);
            }
            Ok(Event::End(ref e)) => {
                match e.name().as_ref() {
                    b"Step" => {
                        // Translate Spanish name to English
                        let en_name = steps::translate_to_en(&parser.name);

                        // Opaque step: capture the full inner XML verbatim. `pos_before`
                        // is the start of this `</Step>` tag, so the slice from the
                        // inner-start offset to here is exactly the step's children.
                        if let Some(start) = opaque_inner_start.take() {
                            if pos_before > start {
                                parser.calculation = xml_clean[start..pos_before].to_string();
                            }
                        }

                        // Close block BEFORE the step
                        if steps::closes_block(&en_name) {
                            indent_level = indent_level.saturating_sub(1);
                        }

                        // Check opens_block before moving en_name into to_step
                        let is_opener = steps::opens_block(&en_name);

                        let step = parser.to_step(indent_level, en_name);
                        steps.push(step);

                        // Open block AFTER the step
                        if is_opener {
                            indent_level += 1;
                        }
                    }
                    b"Calculation" => { parser.pop_target(TextTarget::Calculation); }
                    b"Name" => { parser.pop_target(TextTarget::VarName); }
                    b"ObjectName" => { parser.pop_target(TextTarget::ObjectName); }
                    b"FunctionName" => { parser.pop_target(TextTarget::FunctionName); }
                    b"P" => {
                        parser.pop_target(TextTarget::Param);
                        parser.param_values.push(parser.current_param.clone());
                    }
                    b"Repetition" => { parser.pop_target(TextTarget::RepetitionCalc); }
                    b"Value" => { parser.pop_target(TextTarget::ValueCalc); }
                    b"Restore" => {}
                    b"Set" => { parser.pop_target(TextTarget::SetState); }
                    b"Title" => { parser.pop_target(TextTarget::DialogTitle); }
                    b"Message" => { parser.pop_target(TextTarget::DialogMessage); }
                    b"Button" => {
                        parser.pop_target(TextTarget::DialogButton);
                        if !parser.current_button.is_empty() {
                            parser.dialog_buttons.push(parser.current_button.clone());
                            parser.current_button.clear();
                        }
                    }
                    b"Result" => { parser.pop_target(TextTarget::FieldResult); }
                    b"TargetName" => { parser.pop_target(TextTarget::FieldTarget); }
                    b"Field" => { parser.pop_target(TextTarget::FieldTextContent); }
                    b"Text" => {
                        if parser.in_find_criteria {
                            parser.pop_target(TextTarget::FindCriterionText);
                        } else {
                            parser.pop_target(TextTarget::Text);
                        }
                    }
                    b"CURLOptions" => { parser.pop_target(TextTarget::CurlOptions); }
                    b"Height" => { parser.pop_target(TextTarget::WinHeight); }
                    b"Width" => { parser.pop_target(TextTarget::WinWidth); }
                    b"DistanceFromTop" => { parser.pop_target(TextTarget::WinTop); }
                    b"DistanceFromLeft" => { parser.pop_target(TextTarget::WinLeft); }
                    b"Criteria" => {
                        if let (Some(req), Some(c)) = (parser.current_find_request.as_mut(), parser.current_find_criterion.take()) {
                            req.criteria.push(c);
                        }
                        parser.in_find_criteria = false;
                    }
                    b"RequestRow" => {
                        if let Some(req) = parser.current_find_request.take() {
                            parser.find_requests.push(req);
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                eprintln!("Error parsing XML: {}", e);
                break;
            }
            _ => {}
        }
        buf.clear();
    }

    if steps.is_empty() {
        return Err("No script steps found in XML".to_string());
    }

    Ok(FmScript { steps })
}

/// Tracks WHERE we are in the XML tree for proper text capture.
/// Uses a stack so nested elements route text to the correct field.
#[derive(Debug, Clone, PartialEq)]
enum TextTarget {
    None,
    Text,
    Calculation,       // top-level calc (direct child of Step, If, etc)
    ValueCalc,         // calc inside <Value> (Set Variable)
    RepetitionCalc,    // calc inside <Repetition>
    VarName,
    ObjectName,
    FunctionName,
    Param,
    DialogTitle,
    DialogMessage,
    DialogButton,
    FieldResult,
    FieldTarget,
    FieldTextContent,  // <Field>$var</Field> form used by Execute FileMaker Data API
    SetState,
    // New Window geometry calcs
    WinHeight,
    WinWidth,
    WinTop,
    WinLeft,
    // Perform Find <Criteria><Text>...</Text>: routes text to current_find_criterion.text
    FindCriterionText,
    // Insert from URL <CURLOptions><Calculation>...</Calculation></CURLOptions>
    CurlOptions,
}

#[derive(Default)]
struct StepParser {
    name: String,
    enable: bool,
    id: String,
    text: String,
    calculation: String,
    var_name: String,
    repetition: String,
    object_name: String,
    function_name: String,
    param_values: Vec<String>,
    current_param: String,
    restore_state: Option<String>,
    set_state: Option<String>,
    dialog_title: String,
    dialog_message: String,
    dialog_buttons: Vec<String>,
    current_button: String,
    field_result: String,
    field_target: String,
    field_table: String,
    field_numeric_id: String,
    script_target_name: String,
    script_target_id: String,
    current_script_mode: String,
    goto_location: String,
    goto_exit_after_last: String,
    goto_no_interact: String,
    window_mode: String,
    window_limit_current_file: String,
    window_state: String,
    // New Window + Go to Layout
    layout_name: String,
    layout_id: String,
    layout_destination: String,
    window_height: String,
    window_width: String,
    window_top: String,
    window_left: String,
    window_style_name: String,
    // Perform Find
    find_requests: Vec<FindRequest>,
    current_find_request: Option<FindRequest>,
    current_find_criterion: Option<FindCriterion>,
    in_find_criteria: bool,
    // Insert from URL
    curl_options: String,
    dont_encode_url: String,
    verify_ssl: String,
    select_all_state: String,
    context_stack: Vec<TextTarget>,
}

impl StepParser {
    fn current_target(&self) -> &TextTarget {
        self.context_stack.last().unwrap_or(&TextTarget::None)
    }

    fn push_target(&mut self, t: TextTarget) {
        self.context_stack.push(t);
    }

    fn pop_target(&mut self, expected: TextTarget) {
        if self.context_stack.last() == Some(&expected) {
            self.context_stack.pop();
        }
    }

    fn capture(&mut self, text: &str) {
        match self.current_target() {
            TextTarget::Calculation | TextTarget::ValueCalc => self.calculation.push_str(text),
            TextTarget::RepetitionCalc => self.repetition.push_str(text),
            TextTarget::Text => self.text.push_str(text),
            TextTarget::VarName => self.var_name.push_str(text),
            TextTarget::ObjectName => self.object_name.push_str(text),
            TextTarget::FunctionName => self.function_name.push_str(text),
            TextTarget::Param => self.current_param.push_str(text),
            TextTarget::DialogTitle => self.dialog_title.push_str(text),
            TextTarget::DialogMessage => self.dialog_message.push_str(text),
            TextTarget::DialogButton => self.current_button.push_str(text),
            TextTarget::FieldResult => self.field_result.push_str(text),
            TextTarget::FieldTarget => self.field_target.push_str(text),
            TextTarget::FieldTextContent => self.field_target.push_str(text),
            TextTarget::WinHeight => self.window_height.push_str(text),
            TextTarget::WinWidth => self.window_width.push_str(text),
            TextTarget::WinTop => self.window_top.push_str(text),
            TextTarget::WinLeft => self.window_left.push_str(text),
            TextTarget::FindCriterionText => {
                if let Some(c) = self.current_find_criterion.as_mut() {
                    c.text.push_str(text);
                }
            }
            TextTarget::CurlOptions => self.curl_options.push_str(text),
            TextTarget::SetState | TextTarget::None => {}
        }
    }

    fn to_step(&self, indent_level: u32, name: String) -> ScriptStep {
        ScriptStep {
            name,
            enable: self.enable,
            id: self.id.parse().unwrap_or(0),
            text: if self.text.is_empty() { None } else { Some(self.text.clone()) },
            calculation: if self.calculation.is_empty() { None } else { Some(self.calculation.clone()) },
            var_name: if self.var_name.is_empty() { None } else { Some(self.var_name.clone()) },
            repetition: if self.repetition.is_empty() { None } else { Some(self.repetition.clone()) },
            object_name: if self.object_name.is_empty() { None } else { Some(self.object_name.clone()) },
            function_name: if self.function_name.is_empty() { None } else { Some(self.function_name.clone()) },
            parameters: self.param_values.clone(),
            restore_state: self.restore_state.clone(),
            set_state: self.set_state.clone(),
            dialog_title: if self.dialog_title.is_empty() { None } else { Some(self.dialog_title.clone()) },
            dialog_message: if self.dialog_message.is_empty() { None } else { Some(self.dialog_message.clone()) },
            dialog_buttons: self.dialog_buttons.clone(),
            field_result: if self.field_result.is_empty() { None } else { Some(self.field_result.clone()) },
            field_target: if self.field_target.is_empty() { None } else { Some(self.field_target.clone()) },
            field_table: if self.field_table.is_empty() { None } else { Some(self.field_table.clone()) },
            field_numeric_id: if self.field_numeric_id.is_empty() { None } else { Some(self.field_numeric_id.clone()) },
            script_target_name: if self.script_target_name.is_empty() { None } else { Some(self.script_target_name.clone()) },
            script_target_id: if self.script_target_id.is_empty() { None } else { Some(self.script_target_id.clone()) },
            current_script_mode: if self.current_script_mode.is_empty() { None } else { Some(self.current_script_mode.clone()) },
            goto_location: if self.goto_location.is_empty() { None } else { Some(self.goto_location.clone()) },
            goto_exit_after_last: if self.goto_exit_after_last.is_empty() { None } else { Some(self.goto_exit_after_last.clone()) },
            goto_no_interact: if self.goto_no_interact.is_empty() { None } else { Some(self.goto_no_interact.clone()) },
            window_mode: if self.window_mode.is_empty() { None } else { Some(self.window_mode.clone()) },
            window_limit_current_file: if self.window_limit_current_file.is_empty() { None } else { Some(self.window_limit_current_file.clone()) },
            window_state: if self.window_state.is_empty() { None } else { Some(self.window_state.clone()) },
            layout_name: if self.layout_name.is_empty() { None } else { Some(self.layout_name.clone()) },
            layout_id: if self.layout_id.is_empty() { None } else { Some(self.layout_id.clone()) },
            layout_destination: if self.layout_destination.is_empty() { None } else { Some(self.layout_destination.clone()) },
            window_height: if self.window_height.is_empty() { None } else { Some(self.window_height.clone()) },
            window_width: if self.window_width.is_empty() { None } else { Some(self.window_width.clone()) },
            window_top: if self.window_top.is_empty() { None } else { Some(self.window_top.clone()) },
            window_left: if self.window_left.is_empty() { None } else { Some(self.window_left.clone()) },
            window_style_name: if self.window_style_name.is_empty() { None } else { Some(self.window_style_name.clone()) },
            find_requests: self.find_requests.clone(),
            curl_options: if self.curl_options.is_empty() { None } else { Some(self.curl_options.clone()) },
            dont_encode_url: if self.dont_encode_url.is_empty() { None } else { Some(self.dont_encode_url.clone()) },
            verify_ssl: if self.verify_ssl.is_empty() { None } else { Some(self.verify_ssl.clone()) },
            select_all_state: if self.select_all_state.is_empty() { None } else { Some(self.select_all_state.clone()) },
            indent_level,
        }
    }
}

// ─── XML encoding ───

/// Escape special XML characters. Includes apostrophe for completeness.
pub fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Build the full XML document from a script.
/// FM expects XML without declaration — just the <fmxmlsnippet> root.
pub fn build_xml_from_script(script: &FmScript) -> Result<String, String> {
    let mut xml = String::from("<fmxmlsnippet type=\"FMObjectList\">");
    for step in &script.steps {
        xml.push_str(&build_step_xml(step)?);
    }
    xml.push_str("</fmxmlsnippet>");
    Ok(xml)
}

/// Build a single <Step> element. The switch is on StepShape, not on step name.
/// Unknown shapes fall back to generic calculation/text output.
/// Generates compact XML without extra whitespace (matching FM's format).
fn build_step_xml(step: &ScriptStep) -> Result<String, String> {
    let enabled = if step.enable { "True" } else { "False" };
    let mut xml = format!("<Step enable=\"{}\" id=\"{}\" name=\"{}\">", enabled, step.id, xml_escape(&step.name));

    // Helper: wrap a calc body in CDATA, converting `\n` back to `\r` (FM's native EOL
    // inside calculations). Opaque shape bypasses this — its body is already raw XML.
    let cdata = |s: &str| format!("<![CDATA[{}]]>", cr_for_cdata(s));

    let shape = steps::shape_for_en(&step.name);
    match shape {
        Some(StepShape::Comment) => {
            // FM uses `&#13;` for line breaks in comment Text. xml_escape runs first
            // so any literal `&` in the comment is properly escaped; then `\n` is
            // turned into the CR entity so FM displays the comment as multi-line.
            let text = step.text.as_deref().unwrap_or("");
            let escaped = xml_escape(text).replace('\n', "&#13;");
            xml.push_str(&format!("<Text>{}</Text>", escaped));
        }
        Some(StepShape::ValueCalcName) => {
            xml.push_str(&format!("<Value><Calculation>{}</Calculation></Value>", cdata(step.calculation.as_deref().unwrap_or(""))));
            xml.push_str("<Repetition><Calculation><![CDATA[1]]></Calculation></Repetition>");
            if let Some(var_name) = &step.var_name {
                xml.push_str(&format!("<Name>{}</Name>", xml_escape(var_name)));
            }
        }
        Some(StepShape::CalculationWithRestore) => {
            xml.push_str("<Restore state=\"False\"/>");
            if let Some(calc) = &step.calculation {
                xml.push_str(&format!("<Calculation>{}</Calculation>", cdata(calc)));
            }
        }
        Some(StepShape::Calculation) => {
            if let Some(calc) = &step.calculation {
                xml.push_str(&format!("<Calculation>{}</Calculation>", cdata(calc)));
            }
        }
        Some(StepShape::SetState) => {
            let state = step.set_state.as_deref().unwrap_or("True");
            xml.push_str(&format!("<Set state=\"{}\"></Set>", state));
        }
        Some(StepShape::Dialog) => {
            // FM wraps each dialog text in <Calculation><![CDATA[...]]></Calculation>.
            // The bracket content the user typed is the literal calc expression
            // (so "prueba" with quotes is a string literal in FM-calc terms).
            if let Some(title) = &step.dialog_title {
                xml.push_str(&format!("<Title><Calculation>{}</Calculation></Title>", cdata(title)));
            }
            if let Some(msg) = &step.dialog_message {
                xml.push_str(&format!("<Message><Calculation>{}</Calculation></Message>", cdata(msg)));
            }
            if !step.dialog_buttons.is_empty() {
                xml.push_str("<Buttons>");
                for (i, btn) in step.dialog_buttons.iter().enumerate() {
                    // First button defaults to CommitState=True (the OK button), rest False.
                    let commit = if i == 0 { "True" } else { "False" };
                    xml.push_str(&format!(
                        "<Button CommitState=\"{}\"><Calculation>{}</Calculation></Button>",
                        commit, cdata(btn)
                    ));
                }
                xml.push_str("</Buttons>");
            }
        }
        Some(StepShape::FieldByName) => {
            if let Some(result) = &step.field_result {
                xml.push_str(&format!("<Result>{}</Result>", cdata(result)));
            }
            if let Some(target) = &step.field_target {
                xml.push_str(&format!("<TargetName>{}</TargetName>", xml_escape(target)));
            }
        }
        Some(StepShape::DataApi) => {
            // Execute FileMaker Data API. Always emit SelectAll=True (the common case);
            // the user can flip it manually in FM if they need otherwise.
            xml.push_str("<SelectAll state=\"True\"></SelectAll>");
            if let Some(calc) = &step.calculation {
                xml.push_str(&format!("<Calculation>{}</Calculation>", cdata(calc)));
            }
            xml.push_str("<Text></Text>");
            if let Some(target) = &step.field_target {
                xml.push_str(&format!("<Field>{}</Field>", xml_escape(target)));
            }
        }
        Some(StepShape::GoToRecord) => {
            // FM emits the elements in this order; preserve it.
            let no_interact = step.goto_no_interact.as_deref().unwrap_or("False");
            xml.push_str(&format!("<NoInteract state=\"{}\"></NoInteract>", xml_escape(no_interact)));
            if let Some(exit) = &step.goto_exit_after_last {
                xml.push_str(&format!("<Exit state=\"{}\"></Exit>", xml_escape(exit)));
            }
            if let Some(loc) = &step.goto_location {
                xml.push_str(&format!("<RowPageLocation value=\"{}\"></RowPageLocation>", xml_escape(loc)));
                if loc == "byCalculation" {
                    if let Some(calc) = &step.calculation {
                        xml.push_str(&format!("<Calculation>{}</Calculation>", cdata(calc)));
                    }
                }
            }
        }
        Some(StepShape::PerformScript) => {
            if let Some(mode) = &step.current_script_mode {
                xml.push_str(&format!("<CurrentScript value=\"{}\"></CurrentScript>", xml_escape(mode)));
            }
            if let Some(calc) = &step.calculation {
                xml.push_str(&format!("<Calculation>{}</Calculation>", cdata(calc)));
            }
            if step.script_target_name.is_some() || step.script_target_id.is_some() {
                xml.push_str("<Script");
                if let Some(id) = &step.script_target_id {
                    xml.push_str(&format!(" id=\"{}\"", xml_escape(id)));
                }
                if let Some(name) = &step.script_target_name {
                    xml.push_str(&format!(" name=\"{}\"", xml_escape(name)));
                }
                xml.push_str("></Script>");
            }
        }
        Some(StepShape::FieldAndCalc) => {
            if let Some(calc) = &step.calculation {
                xml.push_str(&format!("<Calculation>{}</Calculation>", cdata(calc)));
            }
            if step.field_target.is_some() || step.field_table.is_some() {
                // Emit only table+name. No `id` attribute — FM resolves by name on paste,
                // which is what makes from-scratch authoring possible.
                xml.push_str("<Field");
                if let Some(t) = &step.field_table {
                    xml.push_str(&format!(" table=\"{}\"", xml_escape(t)));
                }
                if let Some(name) = &step.field_target {
                    xml.push_str(&format!(" name=\"{}\"", xml_escape(name)));
                }
                xml.push_str("/>");
            }
        }
        Some(StepShape::WebViewerJs) => {
            // FM nests text inside <Calculation><![CDATA[...]]></Calculation>
            if let Some(obj) = &step.object_name {
                xml.push_str(&format!("<ObjectName><Calculation>{}</Calculation></ObjectName>", cdata(obj)));
            }
            if let Some(func) = &step.function_name {
                xml.push_str(&format!("<FunctionName><Calculation>{}</Calculation></FunctionName>", cdata(func)));
            }
            if !step.parameters.is_empty() {
                xml.push_str(&format!("<Parameters Count=\"{}\">", step.parameters.len()));
                for p in &step.parameters {
                    xml.push_str(&format!("<P><Calculation>{}</Calculation></P>", cdata(p)));
                }
                xml.push_str("</Parameters>");
            }
        }
        Some(StepShape::SelectWindow) => {
            // <LimitToWindowsOfCurrentFile/> + <Window value/> + <Name><Calculation>name</Calculation></Name>
            let limit = step.window_limit_current_file.as_deref().unwrap_or("False");
            xml.push_str(&format!("<LimitToWindowsOfCurrentFile state=\"{}\"></LimitToWindowsOfCurrentFile>", xml_escape(limit)));
            // Mode default: ByName if a name is present, Current otherwise.
            let default_mode = if step.var_name.is_some() { "ByName" } else { "Current" };
            let mode = step.window_mode.as_deref().unwrap_or(default_mode);
            xml.push_str(&format!("<Window value=\"{}\"></Window>", xml_escape(mode)));
            if let Some(name) = &step.var_name {
                xml.push_str(&format!("<Name><Calculation>{}</Calculation></Name>", cdata(name)));
            }
        }
        Some(StepShape::AdjustWindow) => {
            if let Some(state) = &step.window_state {
                xml.push_str(&format!("<WindowState value=\"{}\"></WindowState>", xml_escape(state)));
            }
        }
        Some(StepShape::GoToObject) => {
            if let Some(obj) = &step.object_name {
                xml.push_str(&format!("<ObjectName><Calculation>{}</Calculation></ObjectName>", cdata(obj)));
            }
            // Repetition defaults to "1" — FM emits it even when implicit.
            let rep = step.repetition.as_deref().unwrap_or("1");
            xml.push_str(&format!("<Repetition><Calculation>{}</Calculation></Repetition>", cdata(rep)));
        }
        Some(StepShape::GoToLayoutNamed) => {
            let dest = step.layout_destination.as_deref().unwrap_or("SelectedLayout");
            xml.push_str(&format!("<LayoutDestination value=\"{}\"></LayoutDestination>", xml_escape(dest)));
            if dest == "SelectedLayout" {
                if let Some(name) = &step.layout_name {
                    xml.push_str("<Layout");
                    if let Some(id) = &step.layout_id {
                        xml.push_str(&format!(" id=\"{}\"", xml_escape(id)));
                    }
                    xml.push_str(&format!(" name=\"{}\"></Layout>", xml_escape(name)));
                }
            }
        }
        Some(StepShape::NewWindow) => {
            xml.push_str("<LayoutDestination value=\"SelectedLayout\"></LayoutDestination>");
            // Geometry calcs — emit empty if missing so FM has a parse target.
            let h = step.window_height.as_deref().unwrap_or("");
            let w = step.window_width.as_deref().unwrap_or("");
            let top = step.window_top.as_deref().unwrap_or("");
            let left = step.window_left.as_deref().unwrap_or("");
            xml.push_str(&format!("<Height><Calculation>{}</Calculation></Height>", cdata(h)));
            xml.push_str(&format!("<Width><Calculation>{}</Calculation></Width>", cdata(w)));
            xml.push_str(&format!("<DistanceFromTop><Calculation>{}</Calculation></DistanceFromTop>", cdata(top)));
            xml.push_str(&format!("<DistanceFromLeft><Calculation>{}</Calculation></DistanceFromLeft>", cdata(left)));
            // Style bitfield + named-style attribute. We emit the standard "Document" flag set
            // unless overridden. The `Styles` integer is FM's internal representation; the
            // value 1076299266 matches a Document window with all chrome enabled.
            let style = step.window_style_name.as_deref().unwrap_or("Document");
            xml.push_str(&format!(
                "<NewWndStyles DimParentWindow=\"No\" Toolbars=\"Yes\" MenuBar=\"Yes\" Style=\"{}\" Close=\"Yes\" Minimize=\"Yes\" Maximize=\"Yes\" Resize=\"Yes\" Styles=\"1076299266\"></NewWndStyles>",
                xml_escape(style)
            ));
            if let Some(name) = &step.layout_name {
                xml.push_str(&format!("<Layout name=\"{}\"></Layout>", xml_escape(name)));
            }
        }
        Some(StepShape::InsertFromUrl) => {
            // FM emits these 4 flags in a fixed order; preserve it to match the
            // original byte-for-byte where possible.
            let no_int = step.goto_no_interact.as_deref().unwrap_or("False");
            let dont_enc = step.dont_encode_url.as_deref().unwrap_or("False");
            let sel_all = step.select_all_state.as_deref().unwrap_or("False");
            let verify = step.verify_ssl.as_deref().unwrap_or("False");
            xml.push_str(&format!("<NoInteract state=\"{}\"></NoInteract>", xml_escape(no_int)));
            xml.push_str(&format!("<DontEncodeURL state=\"{}\"></DontEncodeURL>", xml_escape(dont_enc)));
            xml.push_str(&format!("<SelectAll state=\"{}\"></SelectAll>", xml_escape(sel_all)));
            xml.push_str(&format!("<VerifySSLCertificates state=\"{}\"></VerifySSLCertificates>", xml_escape(verify)));
            if let Some(curl) = &step.curl_options {
                xml.push_str(&format!("<CURLOptions><Calculation>{}</Calculation></CURLOptions>", cdata(curl)));
            }
            // URL calc (the "Specify URL" calc in FM's dialog).
            if let Some(url) = &step.calculation {
                xml.push_str(&format!("<Calculation>{}</Calculation>", cdata(url)));
            }
            xml.push_str("<Text></Text>");
            // Target: variable (`<Field>$var</Field>`) or named field (`<Field name="..."/>`).
            if let Some(t) = &step.field_target {
                if t.starts_with('$') {
                    xml.push_str(&format!("<Field>{}</Field>", xml_escape(t)));
                } else {
                    xml.push_str("<Field");
                    if let Some(tbl) = &step.field_table {
                        xml.push_str(&format!(" table=\"{}\"", xml_escape(tbl)));
                    }
                    xml.push_str(&format!(" name=\"{}\"></Field>", xml_escape(t)));
                }
            }
        }
        Some(StepShape::PerformFind) => {
            xml.push_str("<Restore state=\"True\"></Restore>");
            if !step.find_requests.is_empty() {
                xml.push_str("<Query>");
                for req in &step.find_requests {
                    xml.push_str(&format!("<RequestRow operation=\"{}\">", xml_escape(&req.operation)));
                    for c in &req.criteria {
                        xml.push_str("<Criteria><Field");
                        if !c.table.is_empty() {
                            xml.push_str(&format!(" table=\"{}\"", xml_escape(&c.table)));
                        }
                        if !c.field.is_empty() {
                            xml.push_str(&format!(" name=\"{}\"", xml_escape(&c.field)));
                        }
                        xml.push_str(&format!("></Field><Text>{}</Text></Criteria>", xml_escape(&c.text)));
                    }
                    xml.push_str("</RequestRow>");
                }
                xml.push_str("</Query>");
            }
        }
        Some(StepShape::Opaque) => {
            // The raw inner XML was captured verbatim on decode — emit it as-is.
            // Already valid, entity-escaped XML; must NOT be re-escaped or wrapped.
            if let Some(raw) = &step.calculation {
                xml.push_str(raw);
            }
        }
        Some(StepShape::Plain) | None => {
            // Unknown or plain steps: output whatever data we have as fallback
            if let Some(calc) = &step.calculation {
                xml.push_str(&format!("<Calculation>{}</Calculation>", cdata(calc)));
            } else if let Some(text) = &step.text {
                xml.push_str(&format!("<Text>{}</Text>", xml_escape(text)));
            }
        }
    }

    xml.push_str("</Step>");
    Ok(xml)
}

/// Encode a script to XMSS binary format (header + XML bytes).
/// Returns raw XML bytes (no framing). The clipboard layer adds platform-specific
/// framing: Windows prepends a 4-byte LE length; macOS NSPasteboard takes raw XML.
pub fn encode_xmss(text: &str) -> Result<Vec<u8>, String> {
    let script = crate::text_format::parse_text_to_script(text)?;
    let xml = build_xml_from_script(&script)?;
    Ok(xml.into_bytes())
}

/// Decode XMSS binary data to a script.
pub fn decode_xmss(data: &[u8]) -> Result<FmScript, String> {
    let xml_str = strip_header(data)?;
    parse_fmxml_snippet(&xml_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SNIPPET: &str =
        "<fmxmlsnippet type=\"FMObjectList\"><Step enable=\"True\" id=\"1\" name=\"Comment\"><Text>hi</Text></Step></fmxmlsnippet>";

    /// Frame XML as Windows clipboard data: 4-byte LE length prefix + bytes.
    fn windows_frame(xml: &str) -> Vec<u8> {
        let body = xml.as_bytes();
        let mut data = (body.len() as u32).to_le_bytes().to_vec();
        data.extend_from_slice(body);
        data
    }

    #[test]
    fn strip_header_mac_style_no_prefix() {
        // macOS hands us raw XML with no length prefix.
        let xml = strip_header(SNIPPET.as_bytes()).unwrap();
        assert!(xml.starts_with("<fmxmlsnippet"));
        assert!(xml.ends_with("</fmxmlsnippet>"));
    }

    #[test]
    fn strip_header_windows_style_with_prefix() {
        let xml = strip_header(&windows_frame(SNIPPET)).unwrap();
        assert!(xml.starts_with("<fmxmlsnippet"));
        assert!(xml.ends_with("</fmxmlsnippet>"));
    }

    #[test]
    fn strip_header_empty_errors() {
        assert!(strip_header(&[]).is_err());
    }

    #[test]
    fn decode_roundtrip_both_framings_agree() {
        let mac = decode_xmss(SNIPPET.as_bytes()).unwrap();
        let win = decode_xmss(&windows_frame(SNIPPET)).unwrap();
        assert_eq!(mac.steps.len(), 1);
        assert_eq!(win.steps.len(), 1);
        assert_eq!(mac.steps[0].text, win.steps[0].text);
    }

    #[test]
    fn decode_normalizes_names_and_text_to_nfc() {
        // "café" written in NFD (e + combining acute) inside a comment must come
        // back precomposed (NFC, U+00E9) after decode.
        let nfd = "<fmxmlsnippet type=\"FMObjectList\"><Step enable=\"True\" id=\"1\" name=\"Comment\"><Text>cafe\u{0301}</Text></Step></fmxmlsnippet>";
        let script = parse_fmxml_snippet(nfd).unwrap();
        assert_eq!(script.steps[0].text.as_deref(), Some("café"));
    }

    #[test]
    fn parse_tolerates_xml_declaration() {
        // Some FM/MBS variants prepend `<?xml ...?>`. quick-xml surfaces this as a
        // Decl event the parser already ignores — verify, so we know the prompt's
        // proposed strip_xml_declaration() is unnecessary.
        let with_decl = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>{}", SNIPPET);
        let script = parse_fmxml_snippet(&with_decl).unwrap();
        assert_eq!(script.steps.len(), 1);
    }

    #[test]
    fn parse_tolerates_bom() {
        let with_bom = format!("\u{FEFF}{}", SNIPPET);
        let script = parse_fmxml_snippet(&with_bom).unwrap();
        assert_eq!(script.steps.len(), 1);
    }
}
