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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    // For Set Field (FieldAndCalc shape): preserves the full <Field> attributes.
    pub field_table: Option<String>,
    pub field_numeric_id: Option<String>,
    // For Perform Script (PerformScript shape): target script + parent mode.
    pub script_target_name: Option<String>,
    pub script_target_id: Option<String>,
    pub current_script_mode: Option<String>,
    pub indent_level: u32,
}

/// Strip the 4-byte header from raw clipboard data and return decoded XML string.
/// Accepts any known FM header variant. Falls back to Windows-1252 if not valid UTF-8.
pub fn strip_header(data: &[u8]) -> Result<String, String> {
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

    // Try strict UTF-8 first
    if let Ok(s) = std::str::from_utf8(xml_bytes) {
        return Ok(s.to_string());
    }

    // Fallback: decode as Windows-1252 (covers Latin-1 accented chars)
    // Each byte 0x80-0xFF maps to a specific Unicode code point.
    // Bytes 0x00-0x7F are identical in both encodings.
    let decoded: String = xml_bytes.iter().map(|&b| {
        if b < 0x80 {
            // ASCII — same in UTF-8 and Windows-1252
            b as char
        } else {
            // Windows-1252 specific mappings for 0x80-0x9F range
            // 0xA0-0xFF are Latin-1 supplement (direct Unicode mapping)
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
    }).collect();

    Ok(decoded)
}

/// Strip BOM (U+FEFF) from a string, commonly found in FM clipboard data.
fn strip_bom(s: &str) -> &str {
    s.strip_prefix('\u{FEFF}').unwrap_or(s)
}

/// Parse an FM XML snippet string into a structured script.
/// Translates Spanish step names to English using the steps table.
pub fn parse_fmxml_snippet(xml: &str) -> Result<FmScript, String> {
    let xml_clean = strip_bom(xml);
    let mut reader = Reader::from_reader(Cursor::new(xml_clean));
    // FM emits some elements as self-closing (<Field .../>) and others as
    // explicit pairs (<Field ...></Field>) inconsistently. Normalize both
    // into Start+End pairs so we only need one handler per element.
    reader.config_mut().expand_empty_elements = true;
    let mut buf = Vec::new();
    let mut steps: Vec<ScriptStep> = Vec::new();
    let mut parser = StepParser::default();
    let mut indent_level: u32 = 0;

    loop {
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
                    }
                    b"Calculation" => {
                        let parent = parser.current_target().clone();
                        match parent {
                            TextTarget::RepetitionCalc | TextTarget::ValueCalc => {
                                // Already in the right context, don't push another target
                            }
                            _ => {
                                parser.push_target(TextTarget::Calculation);
                            }
                        }
                    }
                    b"Text" => { parser.push_target(TextTarget::Text); }
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
                        for attr in e.attributes().flatten() {
                            let val = String::from_utf8_lossy(&attr.value).to_string();
                            match attr.key.as_ref() {
                                b"table" => parser.field_table = val,
                                b"id" => parser.field_numeric_id = val,
                                b"name" => parser.field_target = val,
                                _ => {}
                            }
                        }
                    }
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
                    b"Title" => { parser.push_target(TextTarget::DialogTitle); }
                    b"Message" => { parser.push_target(TextTarget::DialogMessage); }
                    b"Button" => { parser.push_target(TextTarget::DialogButton); }
                    b"Result" => { parser.push_target(TextTarget::FieldResult); }
                    b"TargetName" => { parser.push_target(TextTarget::FieldTarget); }
                    _ => {}
                }
            }
            Ok(Event::Text(ref e)) => {
                let text = String::from_utf8_lossy(e.as_ref()).to_string();
                let text = strip_bom(&text).to_string();
                parser.capture(&text);
            }
            Ok(Event::CData(ref e)) => {
                let text = String::from_utf8_lossy(e.as_ref()).to_string();
                let text = strip_bom(&text).to_string();
                parser.capture(&text);
            }
            Ok(Event::End(ref e)) => {
                match e.name().as_ref() {
                    b"Step" => {
                        // Translate Spanish name to English
                        let en_name = steps::translate_to_en(&parser.name);

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
                    b"Text" => { parser.pop_target(TextTarget::Text); }
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
                    b"Button" => { parser.pop_target(TextTarget::DialogButton); }
                    b"Result" => { parser.pop_target(TextTarget::FieldResult); }
                    b"TargetName" => { parser.pop_target(TextTarget::FieldTarget); }
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
    SetState,
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
    field_result: String,
    field_target: String,
    field_table: String,
    field_numeric_id: String,
    script_target_name: String,
    script_target_id: String,
    current_script_mode: String,
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
            TextTarget::DialogButton => self.dialog_buttons.push(text.to_string()),
            TextTarget::FieldResult => self.field_result.push_str(text),
            TextTarget::FieldTarget => self.field_target.push_str(text),
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

    let shape = steps::shape_for_en(&step.name);
    match shape {
        Some(StepShape::Comment) => {
            let text = step.text.as_deref().unwrap_or("");
            xml.push_str(&format!("<Text>{}</Text>", xml_escape(text)));
        }
        Some(StepShape::ValueCalcName) => {
            xml.push_str("<Value><Calculation><![CDATA[");
            xml.push_str(step.calculation.as_deref().unwrap_or(""));
            xml.push_str("]]></Calculation></Value>");
            xml.push_str("<Repetition><Calculation><![CDATA[1]]></Calculation></Repetition>");
            if let Some(var_name) = &step.var_name {
                xml.push_str(&format!("<Name>{}</Name>", xml_escape(var_name)));
            }
        }
        Some(StepShape::CalculationWithRestore) => {
            xml.push_str("<Restore state=\"False\"/>");
            if let Some(calc) = &step.calculation {
                xml.push_str(&format!("<Calculation><![CDATA[{}]]></Calculation>", calc));
            }
        }
        Some(StepShape::Calculation) => {
            if let Some(calc) = &step.calculation {
                xml.push_str(&format!("<Calculation><![CDATA[{}]]></Calculation>", calc));
            }
        }
        Some(StepShape::SetState) => {
            let state = step.set_state.as_deref().unwrap_or("True");
            xml.push_str(&format!("<Set state=\"{}\"></Set>", state));
        }
        Some(StepShape::Dialog) => {
            if let Some(title) = &step.dialog_title {
                xml.push_str(&format!("<Title>{}</Title>", xml_escape(title)));
            }
            if let Some(msg) = &step.dialog_message {
                xml.push_str(&format!("<Message><![CDATA[{}]]></Message>", msg));
            }
            for btn in &step.dialog_buttons {
                xml.push_str(&format!("<Button>{}</Button>", xml_escape(btn)));
            }
        }
        Some(StepShape::FieldByName) => {
            if let Some(result) = &step.field_result {
                xml.push_str(&format!("<Result><![CDATA[{}]]></Result>", result));
            }
            if let Some(target) = &step.field_target {
                xml.push_str(&format!("<TargetName>{}</TargetName>", xml_escape(target)));
            }
        }
        Some(StepShape::PerformScript) => {
            if let Some(mode) = &step.current_script_mode {
                xml.push_str(&format!("<CurrentScript value=\"{}\"></CurrentScript>", xml_escape(mode)));
            }
            if let Some(calc) = &step.calculation {
                xml.push_str(&format!("<Calculation><![CDATA[{}]]></Calculation>", calc));
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
                xml.push_str(&format!("<Calculation><![CDATA[{}]]></Calculation>", calc));
            }
            if step.field_target.is_some() || step.field_table.is_some() || step.field_numeric_id.is_some() {
                xml.push_str("<Field");
                if let Some(t) = &step.field_table {
                    xml.push_str(&format!(" table=\"{}\"", xml_escape(t)));
                }
                if let Some(id) = &step.field_numeric_id {
                    xml.push_str(&format!(" id=\"{}\"", xml_escape(id)));
                }
                if let Some(name) = &step.field_target {
                    xml.push_str(&format!(" name=\"{}\"", xml_escape(name)));
                }
                xml.push_str("/>");
            }
        }
        Some(StepShape::WebViewerJs) => {
            if let Some(obj) = &step.object_name {
                xml.push_str(&format!("<ObjectName>{}</ObjectName>", xml_escape(obj)));
            }
            if let Some(func) = &step.function_name {
                xml.push_str(&format!("<FunctionName>{}</FunctionName>", xml_escape(func)));
            }
            for p in &step.parameters {
                xml.push_str(&format!("<P><![CDATA[{}]]></P>", p));
            }
        }
        Some(StepShape::Plain) | None => {
            // Unknown or plain steps: output whatever data we have as fallback
            if let Some(calc) = &step.calculation {
                xml.push_str(&format!("<Calculation><![CDATA[{}]]></Calculation>", calc));
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
