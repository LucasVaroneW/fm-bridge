// Parser for FileMaker's FMSaveAsXML format (full database export).
// Distinct from XMSS (clipboard). Uses streaming quick-xml to handle 100MB+ files.
// Reuses xmss::parse_fmxml_snippet for the step-level parsing inside each script.

use std::collections::{HashMap, HashSet};

use quick_xml::events::Event;
use quick_xml::Reader;
use serde::Serialize;

use crate::text_format::format_script;
use crate::xmss::{parse_fmxml_snippet, ScriptStep};

// ─── Data types ──────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Clone)]
pub struct ScriptInfo {
    pub id: u32,
    pub name: String,
    pub uuid: String,
    pub hidden: bool,
    pub is_folder: bool,
    pub is_separator: bool,
    pub run_with_full_access: bool,
    pub step_count: usize,
}

#[derive(Debug, Serialize, Clone)]
pub struct LayoutInfo {
    pub id: u32,
    pub name: String,
    pub hidden: bool,
}

#[derive(Debug, Serialize, Clone)]
pub struct FieldInfo {
    pub id: u32,
    pub name: String,
    pub field_type: String,
    pub data_type: String,
    pub comment: String,
}

#[derive(Debug, Serialize)]
pub struct TableInfo {
    pub id: u32,
    pub name: String,
    pub fields: Vec<FieldInfo>,
}

#[derive(Debug, Serialize)]
pub struct Manifest {
    pub file_name: String,
    pub script_count: usize,
    pub layout_count: usize,
    pub table_count: usize,
    pub field_count: usize,
    pub scripts: Vec<ScriptSummary>,
    pub layouts: Vec<LayoutInfo>,
}

#[derive(Debug, Serialize)]
pub struct ScriptSummary {
    pub id: u32,
    pub name: String,
    pub hidden: bool,
    pub is_folder: bool,
    pub step_count: usize,
    pub file: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AnalysisReport {
    pub unreferenced_scripts: Vec<ScriptRef>,
    pub call_graph: Vec<CallGraphEntry>,
}

#[derive(Debug, Serialize)]
pub struct ScriptRef {
    pub id: u32,
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct CallGraphEntry {
    pub caller_id: u32,
    pub caller_name: String,
    pub callee_id: u32,
    pub callee_name: String,
}

pub struct ParsedDatabase {
    pub file_name: String,
    pub scripts: Vec<ScriptInfo>,
    pub script_steps: HashMap<u32, Vec<ScriptStep>>,
    pub layouts: Vec<LayoutInfo>,
    pub tables: Vec<TableInfo>,
    /// caller_id → Vec<callee_id>, extracted directly from raw XML (more reliable than
    /// relying on parse_fmxml_snippet for FMSaveAsXML's different Perform Script format).
    pub script_calls: HashMap<u32, Vec<u32>>,
}

// ─── State machine ────────────────────────────────────────────────────────────

#[derive(PartialEq, Debug)]
enum Section {
    Root,
    ScriptCatalog { depth: u32 },
    StepsForScripts { depth: u32 },
    LayoutCatalog { depth: u32 },
    BaseTableCatalog { depth: u32 },
    FieldsForTables { depth: u32 },
    #[allow(dead_code)]
    Skip,
}

// ─── Parser ───────────────────────────────────────────────────────────────────

pub fn parse(xml_path: &str) -> Result<ParsedDatabase, String> {
    let raw = std::fs::read(xml_path)
        .map_err(|e| format!("Cannot read {}: {}", xml_path, e))?;

    let owned: String;
    let xml_str: &str = if raw.starts_with(b"\xFF\xFE") {
        // UTF-16 LE (FileMaker's default on Windows)
        let u16s: Vec<u16> = raw[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        owned = String::from_utf16(&u16s).map_err(|e| format!("UTF-16 LE error: {}", e))?;
        &owned
    } else if raw.starts_with(b"\xFE\xFF") {
        // UTF-16 BE
        let u16s: Vec<u16> = raw[2..]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        owned = String::from_utf16(&u16s).map_err(|e| format!("UTF-16 BE error: {}", e))?;
        &owned
    } else if raw.starts_with(b"\xEF\xBB\xBF") {
        // UTF-8 BOM
        std::str::from_utf8(&raw[3..]).map_err(|e| format!("UTF-8 error: {}", e))?
    } else {
        std::str::from_utf8(&raw).map_err(|e| format!("UTF-8 error: {}", e))?
    };

    let mut reader = Reader::from_str(xml_str);
    reader.config_mut().expand_empty_elements = true;

    let mut buf = Vec::new();

    let mut file_name = String::new();
    let mut scripts: Vec<ScriptInfo> = Vec::new();
    let mut script_steps: HashMap<u32, Vec<ScriptStep>> = HashMap::new();
    let mut layouts: Vec<LayoutInfo> = Vec::new();
    let mut tables: Vec<TableInfo> = Vec::new();

    let mut section = Section::Root;
    let mut depth: u32 = 0;
    // caller_id → Vec<callee_id>, extracted directly from the XML (not via parse_fmxml_snippet)
    let mut script_calls: HashMap<u32, Vec<u32>> = HashMap::new();

    // Per-section working state
    let mut cur_script: Option<ScriptInfo> = None;
    let mut reading_script_uuid = false;
    let mut reading_script_options = false;

    let mut cur_steps_script_id: Option<u32> = None;
    let mut object_list_inner_start: Option<usize> = None;
    let mut object_list_depth: u32 = 0;

    let mut cur_layout: Option<LayoutInfo> = None;
    let mut _cur_layout_options_depth: Option<u32> = None;

    let mut cur_table: Option<TableInfo> = None;
    let mut cur_field_table_id: Option<u32> = None;
    let mut cur_field_table_name = String::new();
    let mut cur_field: Option<FieldInfo> = None;

    let mut seen_layout_ids: HashSet<u32> = HashSet::new();
    let mut seen_table_ids: HashSet<u32> = HashSet::new();

    loop {
        let pos_before = reader.buffer_position() as usize;

        let event = reader.read_event_into(&mut buf);
        match event {
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("XML parse error at pos {}: {}", pos_before, e)),

            Ok(Event::Start(ref e)) => {
                depth += 1;
                let local = e.name().as_ref().to_vec();
                let local = local.as_slice();

                match section {
                    Section::Root => match local {
                        b"FMSaveAsXML" => {
                            for attr in e.attributes().flatten() {
                                if attr.key.as_ref() == b"File" {
                                    file_name = String::from_utf8_lossy(&attr.value).to_string();
                                }
                            }
                        }
                        b"ScriptCatalog" => section = Section::ScriptCatalog { depth },
                        b"StepsForScripts" => section = Section::StepsForScripts { depth },
                        b"LayoutCatalog" => section = Section::LayoutCatalog { depth },
                        b"BaseTableCatalog" => section = Section::BaseTableCatalog { depth },
                        b"FieldsForTables" => section = Section::FieldsForTables { depth },
                        _ => {}
                    },

                    Section::ScriptCatalog { depth: sec_depth } => {
                        if local == b"Script" {
                            let (id, name, is_folder, is_sep) = parse_script_attrs(e);
                            cur_script = Some(ScriptInfo {
                                id,
                                name,
                                uuid: String::new(),
                                hidden: false,
                                is_folder,
                                is_separator: is_sep,
                                run_with_full_access: false,
                                step_count: 0,
                            });
                            reading_script_uuid = false;
                            reading_script_options = false;
                        } else if local == b"UUID" && cur_script.is_some() && depth == sec_depth + 2 {
                            reading_script_uuid = true;
                        } else if local == b"Options" && cur_script.is_some() && depth == sec_depth + 2 {
                            reading_script_options = true;
                            for attr in e.attributes().flatten() {
                                match attr.key.as_ref() {
                                    b"hidden" => {
                                        if let Some(s) = cur_script.as_mut() {
                                            s.hidden = &attr.value[..] == b"True";
                                        }
                                    }
                                    b"runwithfullaccess" => {
                                        if let Some(s) = cur_script.as_mut() {
                                            s.run_with_full_access = &attr.value[..] == b"True";
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }

                    Section::StepsForScripts { depth: sec_depth } => {
                        // Only capture ScriptReference that is a direct child of <Script>
                        // (depth == sec_depth + 2). Nested <ScriptReference> elements appear
                        // inside Perform Script step parameters at greater depth and must be
                        // ignored to avoid overwriting cur_steps_script_id.
                        if local == b"ScriptReference" && depth == sec_depth + 2 {
                            let mut id = 0u32;
                            for attr in e.attributes().flatten() {
                                if attr.key.as_ref() == b"id" {
                                    id = String::from_utf8_lossy(&attr.value)
                                        .parse()
                                        .unwrap_or(0);
                                }
                            }
                            cur_steps_script_id = Some(id);
                        } else if local == b"ObjectList"
                            && cur_steps_script_id.is_some()
                            && object_list_inner_start.is_none()
                        {
                            object_list_inner_start = Some(reader.buffer_position() as usize);
                            object_list_depth = depth;
                        }
                    }

                    Section::LayoutCatalog { depth: sec_depth } => {
                        if local == b"Layout" {
                            let (id, name) = parse_id_name_attrs(e);
                            if !seen_layout_ids.contains(&id) {
                                cur_layout = Some(LayoutInfo { id, name, hidden: false });
                            }
                        } else if local == b"Options" && cur_layout.is_some() {
                            _cur_layout_options_depth = Some(depth);
                            for attr in e.attributes().flatten() {
                                if attr.key.as_ref() == b"hidden" {
                                    if let Some(l) = cur_layout.as_mut() {
                                        l.hidden = &attr.value[..] == b"True";
                                    }
                                }
                            }
                        }
                        let _ = sec_depth;
                    }

                    Section::BaseTableCatalog { .. } => {
                        if local == b"BaseTable" {
                            let (id, name) = parse_id_name_attrs(e);
                            if !seen_table_ids.contains(&id) {
                                cur_table = Some(TableInfo { id, name, fields: Vec::new() });
                            }
                        }
                    }

                    Section::FieldsForTables { .. } => {
                        if local == b"BaseTableReference" {
                            for attr in e.attributes().flatten() {
                                match attr.key.as_ref() {
                                    b"id" => {
                                        cur_field_table_id =
                                            String::from_utf8_lossy(&attr.value).parse().ok()
                                    }
                                    b"name" => {
                                        cur_field_table_name =
                                            String::from_utf8_lossy(&attr.value).to_string()
                                    }
                                    _ => {}
                                }
                            }
                        } else if local == b"Field" {
                            let mut id = 0u32;
                            let mut name = String::new();
                            let mut field_type = String::new();
                            let mut data_type = String::new();
                            let mut comment = String::new();
                            for attr in e.attributes().flatten() {
                                match attr.key.as_ref() {
                                    b"id" => {
                                        id = String::from_utf8_lossy(&attr.value)
                                            .parse()
                                            .unwrap_or(0)
                                    }
                                    b"name" => {
                                        name = String::from_utf8_lossy(&attr.value).to_string()
                                    }
                                    b"fieldtype" => {
                                        field_type =
                                            String::from_utf8_lossy(&attr.value).to_string()
                                    }
                                    b"datatype" => {
                                        data_type =
                                            String::from_utf8_lossy(&attr.value).to_string()
                                    }
                                    b"comment" => {
                                        comment =
                                            String::from_utf8_lossy(&attr.value).to_string()
                                    }
                                    _ => {}
                                }
                            }
                            cur_field = Some(FieldInfo { id, name, field_type, data_type, comment });
                        }
                    }

                    Section::Skip => {}
                }
            }

            Ok(Event::End(ref e)) => {
                let local = e.name().as_ref().to_vec();
                let local = local.as_slice();

                match &section {
                    Section::ScriptCatalog { depth: sec_depth } => {
                        if local == b"UUID" && reading_script_uuid {
                            reading_script_uuid = false;
                        } else if local == b"Options" && reading_script_options {
                            reading_script_options = false;
                        } else if local == b"Script" {
                            if let Some(s) = cur_script.take() {
                                scripts.push(s);
                            }
                        }
                        if depth == *sec_depth {
                            section = Section::Root;
                        }
                    }

                    Section::StepsForScripts { depth: sec_depth } => {
                        if local == b"ObjectList"
                            && depth == object_list_depth
                            && object_list_inner_start.is_some()
                        {
                            if let (Some(inner_start), Some(script_id)) =
                                (object_list_inner_start.take(), cur_steps_script_id)
                            {
                                let inner_xml = &xml_str[inner_start..pos_before];

                                // Extract call targets directly from the raw XML.
                                // FMSaveAsXML wraps Perform Script targets in
                                // <Parameter type="List"><List><ScriptReference id="X">
                                // which parse_fmxml_snippet doesn't handle.
                                let calls = extract_script_refs(inner_xml);
                                if !calls.is_empty() {
                                    script_calls.insert(script_id, calls);
                                }

                                // Normalize FMSaveAsXML's double-nested Calculation wrapper
                                // before passing to parse_fmxml_snippet (which expects XMSS format).
                                let normalized = normalize_calculations(inner_xml);
                                let wrapped = format!("<FMScriptStep>{}</FMScriptStep>", normalized);
                                if let Ok(parsed) = parse_fmxml_snippet(&wrapped) {
                                    script_steps.insert(script_id, parsed.steps);
                                }
                            }
                        } else if local == b"Script" {
                            cur_steps_script_id = None;
                        }
                        if depth == *sec_depth {
                            section = Section::Root;
                        }
                    }

                    Section::LayoutCatalog { depth: sec_depth } => {
                        if local == b"Options" {
                            _cur_layout_options_depth = None;
                        } else if local == b"Layout" {
                            if let Some(l) = cur_layout.take() {
                                seen_layout_ids.insert(l.id);
                                layouts.push(l);
                            }
                        }
                        if depth == *sec_depth {
                            section = Section::Root;
                        }
                    }

                    Section::BaseTableCatalog { depth: sec_depth } => {
                        if local == b"BaseTable" {
                            if let Some(t) = cur_table.take() {
                                seen_table_ids.insert(t.id);
                                tables.push(t);
                            }
                        }
                        if depth == *sec_depth {
                            section = Section::Root;
                        }
                    }

                    Section::FieldsForTables { depth: sec_depth } => {
                        if local == b"Field" {
                            if let Some(f) = cur_field.take() {
                                if let Some(tid) = cur_field_table_id {
                                    if let Some(t) = tables.iter_mut().find(|t| t.id == tid) {
                                        t.fields.push(f);
                                    } else {
                                        tables.push(TableInfo {
                                            id: tid,
                                            name: cur_field_table_name.clone(),
                                            fields: vec![f],
                                        });
                                    }
                                }
                            }
                        }
                        if depth == *sec_depth {
                            section = Section::Root;
                            cur_field_table_id = None;
                        }
                    }

                    Section::Root | Section::Skip => {}
                }

                depth -= 1;
            }

            Ok(Event::Text(ref e)) => {
                if reading_script_uuid {
                    if let Some(s) = cur_script.as_mut() {
                        let text = e.unescape().unwrap_or_default().to_string();
                        s.uuid = text.trim().to_string();
                    }
                }
            }

            _ => {}
        }

        buf.clear();
    }

    // Attach step counts to script metadata.
    for s in &mut scripts {
        if let Some(steps) = script_steps.get(&s.id) {
            s.step_count = steps.len();
        }
    }

    Ok(ParsedDatabase { file_name, scripts, script_steps, layouts, tables, script_calls })
}

// ─── Output generation ────────────────────────────────────────────────────────

pub fn write_inspection(db: &ParsedDatabase, output_dir: &str) -> Result<InspectionStats, String> {
    let out = std::path::Path::new(output_dir);
    std::fs::create_dir_all(out).map_err(|e| format!("Cannot create {}: {}", output_dir, e))?;

    let scripts_dir = out.join("scripts");
    let tables_dir = out.join("tables");
    let analysis_dir = out.join("analysis");

    std::fs::create_dir_all(&scripts_dir)
        .map_err(|e| format!("Cannot create scripts dir: {}", e))?;
    std::fs::create_dir_all(&tables_dir)
        .map_err(|e| format!("Cannot create tables dir: {}", e))?;
    std::fs::create_dir_all(&analysis_dir)
        .map_err(|e| format!("Cannot create analysis dir: {}", e))?;

    // ── Scripts ──────────────────────────────────────────────────────────────
    let mut scripts_written = 0usize;
    let mut script_summaries: Vec<ScriptSummary> = Vec::new();

    for script in &db.scripts {
        let file = if !script.is_folder && !script.is_separator {
            if let Some(steps) = db.script_steps.get(&script.id) {
                let fmscript = crate::xmss::FmScript { steps: steps.clone() };
                let text = format_script(&fmscript);
                let safe_name = sanitize_filename(&script.name);
                let filename = format!("{:04}_{}.fmscript", script.id, safe_name);
                let path = scripts_dir.join(&filename);
                std::fs::write(&path, &text)
                    .map_err(|e| format!("Cannot write {}: {}", path.display(), e))?;
                scripts_written += 1;
                Some(filename)
            } else {
                None
            }
        } else {
            None
        };

        script_summaries.push(ScriptSummary {
            id: script.id,
            name: script.name.clone(),
            hidden: script.hidden,
            is_folder: script.is_folder,
            step_count: script.step_count,
            file,
        });
    }

    // ── Layouts ───────────────────────────────────────────────────────────────
    let layouts_json = serde_json::to_string_pretty(&db.layouts)
        .map_err(|e| format!("JSON error: {}", e))?;
    std::fs::write(out.join("layouts.json"), &layouts_json)
        .map_err(|e| format!("Cannot write layouts.json: {}", e))?;

    // ── Tables ────────────────────────────────────────────────────────────────
    let total_fields: usize = db.tables.iter().map(|t| t.fields.len()).sum();
    for table in &db.tables {
        let safe = sanitize_filename(&table.name);
        let path = tables_dir.join(format!("{}.json", safe));
        let json = serde_json::to_string_pretty(table)
            .map_err(|e| format!("JSON error: {}", e))?;
        std::fs::write(&path, &json)
            .map_err(|e| format!("Cannot write {}: {}", path.display(), e))?;
    }

    // ── Analysis ─────────────────────────────────────────────────────────────
    let analysis = build_analysis(db);
    let analysis_json = serde_json::to_string_pretty(&analysis)
        .map_err(|e| format!("JSON error: {}", e))?;
    std::fs::write(analysis_dir.join("analysis.json"), &analysis_json)
        .map_err(|e| format!("Cannot write analysis.json: {}", e))?;

    // ── Manifest ──────────────────────────────────────────────────────────────
    let manifest = Manifest {
        file_name: db.file_name.clone(),
        script_count: db.scripts.iter().filter(|s| !s.is_folder && !s.is_separator).count(),
        layout_count: db.layouts.len(),
        table_count: db.tables.len(),
        field_count: total_fields,
        scripts: script_summaries,
        layouts: db.layouts.clone(),
    };
    let manifest_json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| format!("JSON error: {}", e))?;
    std::fs::write(out.join("manifest.json"), &manifest_json)
        .map_err(|e| format!("Cannot write manifest.json: {}", e))?;

    Ok(InspectionStats {
        scripts_written,
        layouts: db.layouts.len(),
        tables: db.tables.len(),
        fields: total_fields,
        unreferenced_scripts: analysis.unreferenced_scripts.len(),
    })
}

pub struct InspectionStats {
    pub scripts_written: usize,
    pub layouts: usize,
    pub tables: usize,
    pub fields: usize,
    pub unreferenced_scripts: usize,
}

// ─── Analysis ─────────────────────────────────────────────────────────────────

fn build_analysis(db: &ParsedDatabase) -> AnalysisReport {
    let all_ids: HashSet<u32> = db
        .scripts
        .iter()
        .filter(|s| !s.is_folder && !s.is_separator)
        .map(|s| s.id)
        .collect();

    let id_to_name: HashMap<u32, &str> =
        db.scripts.iter().map(|s| (s.id, s.name.as_str())).collect();

    // Use the directly-extracted call graph (more reliable than parse_fmxml_snippet
    // for FMSaveAsXML's Perform Script parameter format).
    let mut referenced_ids: HashSet<u32> = HashSet::new();
    let mut call_graph: Vec<CallGraphEntry> = Vec::new();

    for (caller_id, callees) in &db.script_calls {
        let caller_name = id_to_name
            .get(caller_id)
            .copied()
            .unwrap_or("")
            .to_string();
        for callee_id in callees {
            if all_ids.contains(callee_id) {
                referenced_ids.insert(*callee_id);
                call_graph.push(CallGraphEntry {
                    caller_id: *caller_id,
                    caller_name: caller_name.clone(),
                    callee_id: *callee_id,
                    callee_name: id_to_name
                        .get(callee_id)
                        .copied()
                        .unwrap_or("")
                        .to_string(),
                });
            }
        }
    }

    let mut unreferenced_scripts: Vec<ScriptRef> = all_ids
        .iter()
        .filter(|id| !referenced_ids.contains(id))
        .filter_map(|id| {
            id_to_name.get(id).map(|name| ScriptRef {
                id: *id,
                name: name.to_string(),
            })
        })
        .collect();
    unreferenced_scripts.sort_by_key(|s| s.id);

    AnalysisReport { unreferenced_scripts, call_graph }
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn parse_script_attrs(e: &quick_xml::events::BytesStart) -> (u32, String, bool, bool) {
    let mut id = 0u32;
    let mut name = String::new();
    let mut is_folder = false;
    let mut is_sep = false;
    for attr in e.attributes().flatten() {
        match attr.key.as_ref() {
            b"id" => id = String::from_utf8_lossy(&attr.value).parse().unwrap_or(0),
            b"name" => name = String::from_utf8_lossy(&attr.value).to_string(),
            b"isFolder" => is_folder = &attr.value[..] == b"True",
            b"isSeparatorItem" => is_sep = &attr.value[..] == b"True",
            _ => {}
        }
    }
    (id, name, is_folder, is_sep)
}

fn parse_id_name_attrs(e: &quick_xml::events::BytesStart) -> (u32, String) {
    let mut id = 0u32;
    let mut name = String::new();
    for attr in e.attributes().flatten() {
        match attr.key.as_ref() {
            b"id" => id = String::from_utf8_lossy(&attr.value).parse().unwrap_or(0),
            b"name" => name = String::from_utf8_lossy(&attr.value).to_string(),
            _ => {}
        }
    }
    (id, name)
}

fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

/// Extract all ScriptReference id values from the inner XML of an ObjectList.
/// These are the script call targets (Perform Script, etc.) embedded in step parameters.
fn extract_script_refs(xml: &str) -> Vec<u32> {
    let mut refs = Vec::new();
    let mut pos = 0;
    while let Some(i) = xml[pos..].find("<ScriptReference ") {
        let tag_start = pos + i;
        let after = &xml[tag_start..];
        if let Some(gt) = after.find('>') {
            let tag = &after[..gt];
            if let Some(id) = extract_xml_attr(tag, "id").and_then(|s| s.parse::<u32>().ok()) {
                if id > 0 {
                    refs.push(id);
                }
            }
            pos = tag_start + gt + 1;
        } else {
            break;
        }
    }
    refs
}

/// Extract a named attribute value from a raw XML tag string (e.g. `<Step id="5" name="Foo">`).
fn extract_xml_attr<'a>(tag: &'a str, attr_name: &str) -> Option<&'a str> {
    let needle = format!(" {}=\"", attr_name);
    let start = tag.find(&needle)? + needle.len();
    let end = tag[start..].find('"')? + start;
    Some(&tag[start..end])
}

/// Normalize FMSaveAsXML's double-nested Calculation elements to the single-level
/// form that parse_fmxml_snippet (built for the XMSS clipboard format) expects.
///
/// FMSaveAsXML:  <Calculation datatype="1" position="0"><Calculation><Text>…</Text></Calculation></Calculation>
/// XMSS expects: <Calculation><Text>…</Text></Calculation>
///
/// Strategy: strip the outer opening tag (which has `datatype` attribute) and the
/// corresponding extra closing tag. We walk the string tracking Calculation depth
/// so we only collapse the outer wrapper, not legitimate inner nesting.
fn normalize_calculations(xml: &str) -> String {
    let bytes = xml.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;

    while i < bytes.len() {
        // Look for '<' to inspect tags.
        if bytes[i] != b'<' {
            out.push(bytes[i]);
            i += 1;
            continue;
        }

        // Find the end of this tag.
        let tag_start = i;
        let mut j = i + 1;
        while j < bytes.len() && bytes[j] != b'>' {
            j += 1;
        }
        // j points to '>' or end of string.
        let tag_end = if j < bytes.len() { j + 1 } else { j };
        let tag = &xml[tag_start..tag_end];

        // Check if this is an outer Calculation wrapper:
        // <Calculation  (with space — meaning it has attributes like datatype/position).
        if tag.starts_with("<Calculation ") && !tag.starts_with("</") {
            // Skip this opening tag entirely (collapse outer wrapper).
            i = tag_end;
            // Now skip the matching closing </Calculation>.
            // We need to skip the LAST </Calculation> in this outer Calculation element.
            // Since the inner <Calculation> is a single level, after emitting the
            // inner content we'll encounter one extra </Calculation>. Track depth.
            let mut depth = 1i32;
            while i < bytes.len() && depth > 0 {
                if bytes[i] == b'<' {
                    let k_start = i;
                    let mut k = i + 1;
                    while k < bytes.len() && bytes[k] != b'>' {
                        k += 1;
                    }
                    let k_end = if k < bytes.len() { k + 1 } else { k };
                    let inner_tag = &xml[k_start..k_end];
                    if inner_tag.starts_with("</Calculation") {
                        depth -= 1;
                        if depth == 0 {
                            // Skip this extra closing tag (it matched the outer wrapper we removed).
                            i = k_end;
                            break;
                        }
                    } else if inner_tag.starts_with("<Calculation") && !inner_tag.starts_with("</") {
                        depth += 1;
                    }
                    // Emit this inner tag.
                    out.extend_from_slice(inner_tag.as_bytes());
                    i = k_end;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
        } else {
            out.extend_from_slice(tag.as_bytes());
            i = tag_end;
        }
    }

    String::from_utf8_lossy(&out).into_owned()
}
