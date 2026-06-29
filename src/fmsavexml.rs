// Parser for FileMaker's FMSaveAsXML format (full database export).
// Distinct from XMSS (clipboard). Streaming, handles 100MB+ files.
// Extracts: scripts, layouts (with objects), tables, fields, table occurrences,
// relationships, external data sources — enough to map the entire UI/data graph.

use std::collections::{HashMap, HashSet};

use quick_xml::Reader;
use quick_xml::events::Event;
use serde::Serialize;

use crate::text_format::format_script;
use crate::xmss::{ScriptStep, parse_fmxml_snippet};

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

#[derive(Debug, Serialize, Clone, Default)]
pub struct LayoutInfo {
    pub id: u32,
    pub name: String,
    pub hidden: bool,
    /// Base TableOccurrence that the layout shows. None for folders.
    pub table_occurrence: Option<String>,
    pub table_occurrence_id: Option<u32>,
    pub is_folder: bool,
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct ScriptTriggerRef {
    /// "OnObjectEnter", "OnObjectExit", "OnObjectModify", "OnObjectKeystroke",
    /// "OnLayoutEnter", "OnLayoutExit", "OnRecordCommit", "OnRecordLoad", etc.
    pub event: String,
    pub script_id: u32,
    pub script_name: String,
    /// Optional script parameter calculation passed to the trigger.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameter: Option<String>,
    /// Modes in which the trigger is active (browseMode/findMode/previewMode).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub modes: Vec<String>,
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct LayoutObjectRef {
    pub object_type: String, // "Field", "Button", "Portal", "Text", ...
    /// Optional object name (FM lets you name objects for Go to Object etc.)
    #[serde(skip_serializing_if = "String::is_empty")]
    pub object_name: String,
    pub bounds: Option<String>, // "top,left,bottom,right"
    /// For Field objects: the field reference (TO::Field).
    pub field_table_occurrence: Option<String>,
    pub field_name: Option<String>,
    /// For Button objects: script triggered (if any).
    pub button_script_id: Option<u32>,
    pub button_script_name: Option<String>,
    /// For Portal objects: the TO it shows.
    pub portal_table_occurrence: Option<String>,
    /// Tooltip text (calculation expression; usually a literal string).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tooltip: Option<String>,
    /// ScriptTriggers attached to this object — "hidden" functionality that
    /// fires on user interactions with this control.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub script_triggers: Vec<ScriptTriggerRef>,
    /// For Portal objects: the LayoutObjects displayed inside each portal row
    /// (fields, buttons, etc.). Top-level objects use `LayoutFull.objects`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<LayoutObjectRef>,
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct LayoutFull {
    pub id: u32,
    pub name: String,
    pub hidden: bool,
    pub is_folder: bool,
    pub width: u32,
    pub table_occurrence: Option<String>,
    pub table_occurrence_id: Option<u32>,
    pub objects: Vec<LayoutObjectRef>,
    /// Layout-level ScriptTriggers (OnLayoutEnter, OnRecordCommit, etc.)
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub layout_triggers: Vec<ScriptTriggerRef>,
    /// Distinct script ids triggered from this layout's buttons OR triggers
    /// (object-level + layout-level), including those inside portals.
    pub triggered_scripts: Vec<u32>,
    /// Distinct TOs referenced by fields/portals on this layout (any nesting).
    pub referenced_tos: Vec<String>,
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

#[derive(Debug, Serialize, Clone, Default)]
pub struct CustomFunction {
    pub id: u32,
    pub name: String,
    pub access: String,
    /// Signature shown in the FM UI, e.g. "AUDITLOG ( _action ; _param1 ; _param2 ; _param3 )"
    pub display: String,
    pub parameters: Vec<String>,
    /// Body of the function (FM calculation). Filled from CalcsForCustomFunctions.
    pub calculation: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct ExternalDataSource {
    pub id: u32,
    pub name: String,
    pub source_type: String, // "FileMaker", "ODBC", ...
    pub file_path: String,
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct TableOccurrence {
    pub id: u32,
    pub name: String,
    /// "External" or "Internal".
    pub source_type: String,
    /// Name of the ExternalDataSource (empty for internal).
    pub data_source: String,
    /// Name of the base table inside the data source.
    pub base_table: String,
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct JoinPredicate {
    pub op: String, // "Equal", "NotEqual", "Less", ...
    pub left_to: String,
    pub left_field: String,
    pub right_to: String,
    pub right_field: String,
}

#[derive(Debug, Serialize, Clone, Default)]
pub struct Relationship {
    pub id: u32,
    pub left_to: String,
    pub right_to: String,
    pub left_cascade_create: bool,
    pub left_cascade_delete: bool,
    pub right_cascade_create: bool,
    pub right_cascade_delete: bool,
    pub predicates: Vec<JoinPredicate>,
}

#[derive(Debug, Serialize)]
pub struct Manifest {
    pub file_name: String,
    pub script_count: usize,
    pub layout_count: usize,
    pub table_count: usize,
    pub field_count: usize,
    pub table_occurrence_count: usize,
    pub relationship_count: usize,
    pub external_source_count: usize,
    pub custom_function_count: usize,
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
    /// Layouts referenced by Go to Layout / New Window in scripts.
    pub layouts_used_by_scripts: Vec<LayoutUsage>,
    /// Scripts triggered from layout buttons.
    pub scripts_triggered_by_layouts: Vec<LayoutScriptTrigger>,
    /// TOs grouped by external data source.
    pub external_dependencies: HashMap<String, Vec<String>>,
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

#[derive(Debug, Serialize)]
pub struct LayoutUsage {
    pub layout_name: String,
    pub used_by_scripts: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct LayoutScriptTrigger {
    pub layout_id: u32,
    pub layout_name: String,
    pub script_id: u32,
    pub script_name: String,
}

pub struct ParsedDatabase {
    pub file_name: String,
    pub scripts: Vec<ScriptInfo>,
    pub script_steps: HashMap<u32, Vec<ScriptStep>>,
    pub layouts: Vec<LayoutFull>,
    pub tables: Vec<TableInfo>,
    pub external_sources: Vec<ExternalDataSource>,
    pub table_occurrences: Vec<TableOccurrence>,
    pub relationships: Vec<Relationship>,
    pub custom_functions: Vec<CustomFunction>,
    /// caller_id → Vec<callee_id> (from script body).
    pub script_calls: HashMap<u32, Vec<u32>>,
    /// caller_id → Vec<layout_name> (from Go to Layout / New Window).
    pub script_layouts: HashMap<u32, Vec<String>>,
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
    ExternalDataSourceCatalog { depth: u32 },
    TableOccurrenceCatalog { depth: u32 },
    RelationshipCatalog { depth: u32 },
    CustomFunctionsCatalog { depth: u32 },
    CalcsForCustomFunctions { depth: u32 },
}

// ─── Parser ───────────────────────────────────────────────────────────────────

pub fn parse(xml_path: &str) -> Result<ParsedDatabase, String> {
    let raw = std::fs::read(xml_path).map_err(|e| format!("Cannot read {}: {}", xml_path, e))?;

    let owned: String;
    let xml_str: &str = if raw.starts_with(b"\xFF\xFE") {
        let u16s: Vec<u16> = raw[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        owned = String::from_utf16(&u16s).map_err(|e| format!("UTF-16 LE error: {}", e))?;
        &owned
    } else if raw.starts_with(b"\xFE\xFF") {
        let u16s: Vec<u16> = raw[2..]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        owned = String::from_utf16(&u16s).map_err(|e| format!("UTF-16 BE error: {}", e))?;
        &owned
    } else if raw.starts_with(b"\xEF\xBB\xBF") {
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
    let mut layouts: Vec<LayoutFull> = Vec::new();
    let mut tables: Vec<TableInfo> = Vec::new();
    let mut external_sources: Vec<ExternalDataSource> = Vec::new();
    let mut table_occurrences: Vec<TableOccurrence> = Vec::new();
    let mut relationships: Vec<Relationship> = Vec::new();
    let mut custom_functions: Vec<CustomFunction> = Vec::new();

    let mut section = Section::Root;
    let mut depth: u32 = 0;
    let mut script_calls: HashMap<u32, Vec<u32>> = HashMap::new();
    let mut script_layouts: HashMap<u32, Vec<String>> = HashMap::new();

    // ScriptCatalog state
    let mut cur_script: Option<ScriptInfo> = None;
    let mut reading_script_uuid = false;
    let mut reading_script_options = false;

    // StepsForScripts state
    let mut cur_steps_script_id: Option<u32> = None;
    let mut object_list_inner_start: Option<usize> = None;
    let mut object_list_depth: u32 = 0;

    // LayoutCatalog state
    let mut cur_layout: Option<LayoutFull> = None;
    let mut layout_started_depth: u32 = 0;
    let mut in_layout_partslist = false;
    // Stack of (object, depth_at_which_it_opened). Portals contain nested
    // LayoutObjects in their <Portal><ObjectList>; we push on each <LayoutObject>
    // start and pop on </LayoutObject>. On pop, the object goes into the parent's
    // children if the stack isn't empty, otherwise into LayoutFull.objects.
    let mut object_stack: Vec<(LayoutObjectRef, u32)> = Vec::new();
    let mut in_button_action = false;
    // ScriptTriggers parsing: track whether we're inside a <ScriptTriggers> block
    // and at what depth, plus the current <ScriptTrigger> being assembled and
    // whether to attach it to the top of the object stack (object trigger) or
    // the layout (layout trigger).
    let mut script_trigger_depth: Option<u32> = None;
    let mut cur_trigger: Option<ScriptTriggerRef> = None;
    let mut trigger_target_is_object: bool = false;
    let mut in_trigger_calc: bool = false;
    let mut trigger_calc_start: Option<usize> = None;
    let mut trigger_calc_depth: u32 = 0;
    // Tooltip: capture inner calc text.
    let mut in_tooltip: bool = false;
    let mut tooltip_calc_start: Option<usize> = None;
    let mut tooltip_calc_depth: u32 = 0;

    // Tables / Fields state
    let mut cur_table: Option<TableInfo> = None;
    let mut cur_field_table_id: Option<u32> = None;
    let mut cur_field_table_name = String::new();
    let mut cur_field: Option<FieldInfo> = None;

    // ExternalDataSource state
    let mut cur_eds: Option<ExternalDataSource> = None;
    let mut reading_eds_path = false;

    // TableOccurrence state
    let mut cur_to: Option<TableOccurrence> = None;

    // CustomFunction state
    let mut cur_cf: Option<CustomFunction> = None;
    let mut reading_cf_display = false;
    // CalcsForCustomFunctions state
    let mut cur_cfcalc_id: Option<u32> = None;
    let mut cfcalc_inner_start: Option<usize> = None;
    let mut cfcalc_depth: u32 = 0;

    // Relationship state
    let mut cur_rel: Option<Relationship> = None;
    let mut cur_predicate: Option<JoinPredicate> = None;
    let mut in_left_field = false;
    let mut in_right_field = false;
    let mut in_left_table = false;
    let mut in_right_table = false;

    let mut seen_table_ids: HashSet<u32> = HashSet::new();
    let mut seen_layout_ids: HashSet<u32> = HashSet::new();
    let mut seen_to_ids: HashSet<u32> = HashSet::new();

    loop {
        let pos_before = reader.buffer_position() as usize;

        let event = reader.read_event_into(&mut buf);
        match event {
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("XML parse error at pos {}: {}", pos_before, e)),

            Ok(Event::Start(ref e)) => {
                depth += 1;
                let local_vec = e.name().as_ref().to_vec();
                let local = local_vec.as_slice();

                match &section {
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
                        b"ExternalDataSourceCatalog" => {
                            section = Section::ExternalDataSourceCatalog { depth }
                        }
                        b"TableOccurrenceCatalog" => {
                            section = Section::TableOccurrenceCatalog { depth }
                        }
                        b"RelationshipCatalog" => section = Section::RelationshipCatalog { depth },
                        b"CustomFunctionsCatalog" => {
                            section = Section::CustomFunctionsCatalog { depth }
                        }
                        b"CalcsForCustomFunctions" => {
                            section = Section::CalcsForCustomFunctions { depth }
                        }
                        _ => {}
                    },

                    Section::ScriptCatalog { depth: sec_depth } => {
                        let sec_depth = *sec_depth;
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
                        } else if local == b"UUID" && cur_script.is_some() && depth == sec_depth + 2
                        {
                            reading_script_uuid = true;
                        } else if local == b"Options"
                            && cur_script.is_some()
                            && depth == sec_depth + 2
                        {
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
                        let sec_depth = *sec_depth;
                        if local == b"ScriptReference" && depth == sec_depth + 2 {
                            let mut id = 0u32;
                            for attr in e.attributes().flatten() {
                                if attr.key.as_ref() == b"id" {
                                    id = String::from_utf8_lossy(&attr.value).parse().unwrap_or(0);
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
                        let _ = sec_depth;
                        if local == b"Layout" && cur_layout.is_none() {
                            let (id, name) = parse_id_name_attrs(e);
                            let width = parse_u32_attr(e, "width").unwrap_or(0);
                            let is_folder = parse_bool_attr(e, "isFolder");
                            if !seen_layout_ids.contains(&id) {
                                cur_layout = Some(LayoutFull {
                                    id,
                                    name,
                                    hidden: false,
                                    width,
                                    is_folder,
                                    ..Default::default()
                                });
                                layout_started_depth = depth;
                            }
                        } else if cur_layout.is_some() {
                            // ── LayoutObject open: push to stack ────────────────
                            if in_layout_partslist && local == b"LayoutObject" {
                                let mut obj = LayoutObjectRef::default();
                                for attr in e.attributes().flatten() {
                                    match attr.key.as_ref() {
                                        b"type" => {
                                            obj.object_type =
                                                String::from_utf8_lossy(&attr.value).to_string();
                                        }
                                        b"name" => {
                                            obj.object_name =
                                                String::from_utf8_lossy(&attr.value).to_string();
                                        }
                                        _ => {}
                                    }
                                }
                                object_stack.push((obj, depth));
                            } else if local == b"PartsList" {
                                in_layout_partslist = true;
                            } else if local == b"TableOccurrenceReference"
                                && object_stack.is_empty()
                                && depth == layout_started_depth + 1
                            {
                                // Layout's base TO (direct child of <Layout>).
                                let (id, name) = parse_id_name_attrs(e);
                                if let Some(l) = cur_layout.as_mut() {
                                    if id != 0 {
                                        l.table_occurrence_id = Some(id);
                                    }
                                    if !name.is_empty() {
                                        l.table_occurrence = Some(name);
                                    }
                                }
                            } else if local == b"ScriptTriggers" {
                                // Distinguish layout-level vs object-level: if there's
                                // a current object on the stack, it's an object trigger.
                                script_trigger_depth = Some(depth);
                                trigger_target_is_object = !object_stack.is_empty();
                            } else if script_trigger_depth.is_some() && local == b"ScriptTrigger" {
                                let mut event = String::new();
                                let mut modes: Vec<String> = Vec::new();
                                for attr in e.attributes().flatten() {
                                    match attr.key.as_ref() {
                                        b"action" => {
                                            event =
                                                String::from_utf8_lossy(&attr.value).to_string();
                                        }
                                        b"browseMode" if &attr.value[..] == b"True" => {
                                            modes.push("browseMode".to_string());
                                        }
                                        b"findMode" if &attr.value[..] == b"True" => {
                                            modes.push("findMode".to_string());
                                        }
                                        b"previewMode" if &attr.value[..] == b"True" => {
                                            modes.push("previewMode".to_string());
                                        }
                                        _ => {}
                                    }
                                }
                                cur_trigger = Some(ScriptTriggerRef {
                                    event,
                                    modes,
                                    ..Default::default()
                                });
                            } else if cur_trigger.is_some() && local == b"ScriptReference" {
                                let (id, name) = parse_id_name_attrs(e);
                                if let Some(t) = cur_trigger.as_mut() {
                                    t.script_id = id;
                                    t.script_name = name;
                                }
                            } else if cur_trigger.is_some() && local == b"Calculation" {
                                // Trigger parameter calc.
                                in_trigger_calc = true;
                                trigger_calc_start = Some(reader.buffer_position() as usize);
                                trigger_calc_depth = depth;
                            } else if let Some((o, _)) = object_stack.last_mut() {
                                // ── Inside an object: fields, bounds, tooltip, etc. ─
                                if local == b"Bounds" {
                                    let mut t_s = String::new();
                                    let mut l_s = String::new();
                                    let mut b_s = String::new();
                                    let mut r_s = String::new();
                                    for attr in e.attributes().flatten() {
                                        let v = String::from_utf8_lossy(&attr.value).to_string();
                                        match attr.key.as_ref() {
                                            b"top" => t_s = v,
                                            b"left" => l_s = v,
                                            b"bottom" => b_s = v,
                                            b"right" => r_s = v,
                                            _ => {}
                                        }
                                    }
                                    o.bounds = Some(format!("{},{},{},{}", t_s, l_s, b_s, r_s));
                                } else if local == b"FieldReference" {
                                    let (_, name) = parse_id_name_attrs(e);
                                    if o.field_name.is_none() {
                                        o.field_name = Some(name);
                                    }
                                } else if local == b"TableOccurrenceReference" {
                                    let (_, name) = parse_id_name_attrs(e);
                                    if o.object_type == "Portal"
                                        && o.portal_table_occurrence.is_none()
                                    {
                                        o.portal_table_occurrence = Some(name);
                                    } else if o.field_table_occurrence.is_none() {
                                        o.field_table_occurrence = Some(name);
                                    }
                                } else if local == b"action" {
                                    in_button_action = true;
                                } else if in_button_action && local == b"ScriptReference" {
                                    let (id, name) = parse_id_name_attrs(e);
                                    o.button_script_id = Some(id);
                                    o.button_script_name = Some(name);
                                } else if local == b"Tooltip" {
                                    in_tooltip = true;
                                } else if in_tooltip && local == b"Calculation" {
                                    tooltip_calc_start = Some(reader.buffer_position() as usize);
                                    tooltip_calc_depth = depth;
                                }
                            }
                            // Hidden flag from Options on the Layout itself.
                            if local == b"Options"
                                && object_stack.is_empty()
                                && depth == layout_started_depth + 1
                            {
                                for attr in e.attributes().flatten() {
                                    if attr.key.as_ref() == b"hidden" {
                                        if let Some(l) = cur_layout.as_mut() {
                                            l.hidden = &attr.value[..] == b"True";
                                        }
                                    }
                                }
                            }
                        }
                    }

                    Section::BaseTableCatalog { .. } => {
                        if local == b"BaseTable" {
                            let (id, name) = parse_id_name_attrs(e);
                            if !seen_table_ids.contains(&id) {
                                cur_table = Some(TableInfo {
                                    id,
                                    name,
                                    fields: Vec::new(),
                                });
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
                                        data_type = String::from_utf8_lossy(&attr.value).to_string()
                                    }
                                    b"comment" => {
                                        comment = String::from_utf8_lossy(&attr.value).to_string()
                                    }
                                    _ => {}
                                }
                            }
                            cur_field = Some(FieldInfo {
                                id,
                                name,
                                field_type,
                                data_type,
                                comment,
                            });
                        }
                    }

                    Section::ExternalDataSourceCatalog { .. } => {
                        if local == b"ExternalDataSource" {
                            let mut id = 0u32;
                            let mut name = String::new();
                            let mut source_type = String::new();
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
                                    b"type" => {
                                        source_type =
                                            String::from_utf8_lossy(&attr.value).to_string()
                                    }
                                    _ => {}
                                }
                            }
                            cur_eds = Some(ExternalDataSource {
                                id,
                                name,
                                source_type,
                                file_path: String::new(),
                            });
                        } else if local == b"UniversalPathList" && cur_eds.is_some() {
                            reading_eds_path = true;
                        }
                    }

                    Section::TableOccurrenceCatalog { .. } => {
                        if local == b"TableOccurrence" {
                            let (id, name) = parse_id_name_attrs(e);
                            let mut source_type = String::new();
                            for attr in e.attributes().flatten() {
                                if attr.key.as_ref() == b"type" {
                                    source_type = String::from_utf8_lossy(&attr.value).to_string();
                                }
                            }
                            if !seen_to_ids.contains(&id) {
                                cur_to = Some(TableOccurrence {
                                    id,
                                    name,
                                    source_type,
                                    data_source: String::new(),
                                    base_table: String::new(),
                                });
                            }
                        } else if let Some(to) = cur_to.as_mut() {
                            if local == b"DataSourceReference" {
                                for attr in e.attributes().flatten() {
                                    if attr.key.as_ref() == b"name" {
                                        to.data_source =
                                            String::from_utf8_lossy(&attr.value).to_string();
                                    }
                                }
                            } else if local == b"BaseTableReference" {
                                for attr in e.attributes().flatten() {
                                    if attr.key.as_ref() == b"name" {
                                        to.base_table =
                                            String::from_utf8_lossy(&attr.value).to_string();
                                    }
                                }
                            }
                        }
                    }

                    Section::RelationshipCatalog { .. } => {
                        if local == b"Relationship" {
                            let mut id = 0u32;
                            for attr in e.attributes().flatten() {
                                if attr.key.as_ref() == b"id" {
                                    id = String::from_utf8_lossy(&attr.value).parse().unwrap_or(0);
                                }
                            }
                            cur_rel = Some(Relationship {
                                id,
                                ..Default::default()
                            });
                        } else if let Some(r) = cur_rel.as_mut() {
                            if local == b"LeftTable" {
                                in_left_table = true;
                                for attr in e.attributes().flatten() {
                                    match attr.key.as_ref() {
                                        b"cascadeCreate" => {
                                            r.left_cascade_create = &attr.value[..] == b"True"
                                        }
                                        b"cascadeDelete" => {
                                            r.left_cascade_delete = &attr.value[..] == b"True"
                                        }
                                        _ => {}
                                    }
                                }
                            } else if local == b"RightTable" {
                                in_right_table = true;
                                for attr in e.attributes().flatten() {
                                    match attr.key.as_ref() {
                                        b"cascadeCreate" => {
                                            r.right_cascade_create = &attr.value[..] == b"True"
                                        }
                                        b"cascadeDelete" => {
                                            r.right_cascade_delete = &attr.value[..] == b"True"
                                        }
                                        _ => {}
                                    }
                                }
                            } else if local == b"JoinPredicate" {
                                let mut op = String::new();
                                for attr in e.attributes().flatten() {
                                    if attr.key.as_ref() == b"type" {
                                        op = String::from_utf8_lossy(&attr.value).to_string();
                                    }
                                }
                                cur_predicate = Some(JoinPredicate {
                                    op,
                                    ..Default::default()
                                });
                            } else if local == b"LeftField" {
                                in_left_field = true;
                            } else if local == b"RightField" {
                                in_right_field = true;
                            } else if local == b"TableOccurrenceReference" {
                                let (_, name) = parse_id_name_attrs(e);
                                if in_left_table && r.left_to.is_empty() {
                                    r.left_to = name;
                                } else if in_right_table && r.right_to.is_empty() {
                                    r.right_to = name;
                                } else if let Some(p) = cur_predicate.as_mut() {
                                    if in_left_field && p.left_to.is_empty() {
                                        p.left_to = name;
                                    } else if in_right_field && p.right_to.is_empty() {
                                        p.right_to = name;
                                    }
                                }
                            } else if local == b"FieldReference" {
                                let (_, name) = parse_id_name_attrs(e);
                                if let Some(p) = cur_predicate.as_mut() {
                                    if in_left_field && p.left_field.is_empty() {
                                        p.left_field = name;
                                    } else if in_right_field && p.right_field.is_empty() {
                                        p.right_field = name;
                                    }
                                }
                            }
                        }
                    }

                    Section::CustomFunctionsCatalog { .. } => {
                        if local == b"CustomFunction" {
                            let mut id = 0u32;
                            let mut name = String::new();
                            let mut access = String::new();
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
                                    b"access" => {
                                        access = String::from_utf8_lossy(&attr.value).to_string()
                                    }
                                    _ => {}
                                }
                            }
                            cur_cf = Some(CustomFunction {
                                id,
                                name,
                                access,
                                ..Default::default()
                            });
                        } else if let Some(cf) = cur_cf.as_mut() {
                            if local == b"Display" {
                                reading_cf_display = true;
                            } else if local == b"Parameter" {
                                for attr in e.attributes().flatten() {
                                    if attr.key.as_ref() == b"name" {
                                        cf.parameters
                                            .push(String::from_utf8_lossy(&attr.value).to_string());
                                    }
                                }
                            }
                        }
                    }

                    Section::CalcsForCustomFunctions { .. } => {
                        if local == b"CustomFunctionReference" {
                            for attr in e.attributes().flatten() {
                                if attr.key.as_ref() == b"id" {
                                    cur_cfcalc_id =
                                        String::from_utf8_lossy(&attr.value).parse().ok();
                                }
                            }
                        } else if local == b"Calculation"
                            && cur_cfcalc_id.is_some()
                            && cfcalc_inner_start.is_none()
                        {
                            cfcalc_inner_start = Some(reader.buffer_position() as usize);
                            cfcalc_depth = depth;
                        }
                    }
                }
            }

            Ok(Event::End(ref e)) => {
                let local_vec = e.name().as_ref().to_vec();
                let local = local_vec.as_slice();

                match &section {
                    Section::ScriptCatalog { depth: sec_depth } => {
                        let sec_depth = *sec_depth;
                        if local == b"UUID" && reading_script_uuid {
                            reading_script_uuid = false;
                        } else if local == b"Options" && reading_script_options {
                            reading_script_options = false;
                        } else if local == b"Script" {
                            if let Some(s) = cur_script.take() {
                                scripts.push(s);
                            }
                        }
                        if depth == sec_depth {
                            section = Section::Root;
                        }
                    }

                    Section::StepsForScripts { depth: sec_depth } => {
                        let sec_depth = *sec_depth;
                        if local == b"ObjectList"
                            && depth == object_list_depth
                            && object_list_inner_start.is_some()
                        {
                            if let (Some(inner_start), Some(script_id)) =
                                (object_list_inner_start.take(), cur_steps_script_id)
                            {
                                let inner_xml = &xml_str[inner_start..pos_before];
                                // Call graph (Perform Script targets).
                                let calls = extract_script_refs(inner_xml);
                                if !calls.is_empty() {
                                    script_calls.insert(script_id, calls);
                                }
                                // Layout refs (Go to Layout, New Window).
                                let lays = extract_layout_refs(inner_xml);
                                if !lays.is_empty() {
                                    script_layouts.insert(script_id, lays);
                                }
                                let normalized = fmsavexml_to_xmss(inner_xml);
                                let wrapped =
                                    format!("<FMScriptStep>{}</FMScriptStep>", normalized);
                                if let Ok(parsed) = parse_fmxml_snippet(&wrapped) {
                                    script_steps.insert(script_id, parsed.steps);
                                }
                            }
                        } else if local == b"Script" {
                            cur_steps_script_id = None;
                        }
                        if depth == sec_depth {
                            section = Section::Root;
                        }
                    }

                    Section::LayoutCatalog { depth: sec_depth } => {
                        let sec_depth = *sec_depth;
                        if local == b"PartsList" {
                            in_layout_partslist = false;
                        } else if local == b"action" {
                            in_button_action = false;
                        } else if local == b"Tooltip" {
                            in_tooltip = false;
                        } else if local == b"Calculation"
                            && in_tooltip
                            && depth == tooltip_calc_depth
                        {
                            if let (Some(start), Some((o, _))) =
                                (tooltip_calc_start.take(), object_stack.last_mut())
                            {
                                let inner = &xml_str[start..pos_before];
                                o.tooltip = Some(extract_cdata_or_text(inner));
                            }
                        } else if local == b"Calculation"
                            && in_trigger_calc
                            && depth == trigger_calc_depth
                        {
                            if let (Some(start), Some(t)) =
                                (trigger_calc_start.take(), cur_trigger.as_mut())
                            {
                                let inner = &xml_str[start..pos_before];
                                let body = extract_cdata_or_text(inner);
                                if !body.is_empty() {
                                    t.parameter = Some(body);
                                }
                            }
                            in_trigger_calc = false;
                        } else if local == b"ScriptTrigger" {
                            if let Some(t) = cur_trigger.take() {
                                if trigger_target_is_object {
                                    if let Some((o, _)) = object_stack.last_mut() {
                                        o.script_triggers.push(t);
                                    }
                                } else if let Some(l) = cur_layout.as_mut() {
                                    l.layout_triggers.push(t);
                                }
                            }
                        } else if local == b"ScriptTriggers" {
                            if Some(depth) == script_trigger_depth {
                                script_trigger_depth = None;
                            }
                        } else if local == b"LayoutObject" {
                            // Pop the matching object from the stack. The depth check
                            // ensures we only pop when we're closing the same object
                            // we opened (defensive against unexpected XML).
                            if let Some((o, _open_depth)) = object_stack.pop() {
                                if let Some((parent, _)) = object_stack.last_mut() {
                                    parent.children.push(o);
                                } else if let Some(l) = cur_layout.as_mut() {
                                    l.objects.push(o);
                                }
                            }
                        } else if local == b"Layout" && depth == layout_started_depth {
                            if let Some(mut l) = cur_layout.take() {
                                // Aggregate scripts & TOs from the whole tree.
                                let mut triggered: HashSet<u32> = HashSet::new();
                                let mut tos: HashSet<String> = HashSet::new();
                                collect_aggregates(&l.objects, &mut triggered, &mut tos);
                                for t in &l.layout_triggers {
                                    if t.script_id != 0 {
                                        triggered.insert(t.script_id);
                                    }
                                }
                                l.triggered_scripts = triggered.into_iter().collect();
                                l.triggered_scripts.sort();
                                l.referenced_tos = tos.into_iter().collect();
                                l.referenced_tos.sort();
                                seen_layout_ids.insert(l.id);
                                layouts.push(l);
                            }
                        }
                        if depth == sec_depth {
                            section = Section::Root;
                        }
                    }

                    Section::BaseTableCatalog { depth: sec_depth } => {
                        let sec_depth = *sec_depth;
                        if local == b"BaseTable" {
                            if let Some(t) = cur_table.take() {
                                seen_table_ids.insert(t.id);
                                tables.push(t);
                            }
                        }
                        if depth == sec_depth {
                            section = Section::Root;
                        }
                    }

                    Section::FieldsForTables { depth: sec_depth } => {
                        let sec_depth = *sec_depth;
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
                        if depth == sec_depth {
                            section = Section::Root;
                            cur_field_table_id = None;
                        }
                    }

                    Section::ExternalDataSourceCatalog { depth: sec_depth } => {
                        let sec_depth = *sec_depth;
                        if local == b"UniversalPathList" {
                            reading_eds_path = false;
                        } else if local == b"ExternalDataSource" {
                            if let Some(eds) = cur_eds.take() {
                                external_sources.push(eds);
                            }
                        }
                        if depth == sec_depth {
                            section = Section::Root;
                        }
                    }

                    Section::TableOccurrenceCatalog { depth: sec_depth } => {
                        let sec_depth = *sec_depth;
                        if local == b"TableOccurrence" {
                            if let Some(to) = cur_to.take() {
                                seen_to_ids.insert(to.id);
                                table_occurrences.push(to);
                            }
                        }
                        if depth == sec_depth {
                            section = Section::Root;
                        }
                    }

                    Section::RelationshipCatalog { depth: sec_depth } => {
                        let sec_depth = *sec_depth;
                        if local == b"LeftTable" {
                            in_left_table = false;
                        } else if local == b"RightTable" {
                            in_right_table = false;
                        } else if local == b"LeftField" {
                            in_left_field = false;
                        } else if local == b"RightField" {
                            in_right_field = false;
                        } else if local == b"JoinPredicate" {
                            if let Some(p) = cur_predicate.take() {
                                if let Some(r) = cur_rel.as_mut() {
                                    r.predicates.push(p);
                                }
                            }
                        } else if local == b"Relationship" {
                            if let Some(r) = cur_rel.take() {
                                relationships.push(r);
                            }
                        }
                        if depth == sec_depth {
                            section = Section::Root;
                        }
                    }

                    Section::CustomFunctionsCatalog { depth: sec_depth } => {
                        let sec_depth = *sec_depth;
                        if local == b"Display" {
                            reading_cf_display = false;
                        } else if local == b"CustomFunction" {
                            if let Some(cf) = cur_cf.take() {
                                custom_functions.push(cf);
                            }
                        }
                        if depth == sec_depth {
                            section = Section::Root;
                        }
                    }

                    Section::CalcsForCustomFunctions { depth: sec_depth } => {
                        let sec_depth = *sec_depth;
                        if local == b"Calculation"
                            && depth == cfcalc_depth
                            && cfcalc_inner_start.is_some()
                        {
                            if let (Some(start), Some(cfid)) =
                                (cfcalc_inner_start.take(), cur_cfcalc_id)
                            {
                                // pos_before is start of </Calculation>. Extract inner.
                                let inner = &xml_str[start..pos_before];
                                // Strip <Text>CDATA</Text> wrapper if present.
                                let body = extract_cdata_or_text(inner);
                                if let Some(cf) = custom_functions.iter_mut().find(|c| c.id == cfid)
                                {
                                    cf.calculation = body;
                                }
                            }
                        } else if local == b"CustomFunctionCalc" {
                            cur_cfcalc_id = None;
                        }
                        if depth == sec_depth {
                            section = Section::Root;
                        }
                    }

                    Section::Root => {}
                }

                depth -= 1;
            }

            Ok(Event::Text(ref e)) => {
                if reading_script_uuid {
                    if let Some(s) = cur_script.as_mut() {
                        let text = e.unescape().unwrap_or_default().to_string();
                        s.uuid = text.trim().to_string();
                    }
                } else if reading_eds_path {
                    if let Some(eds) = cur_eds.as_mut() {
                        let text = e.unescape().unwrap_or_default().to_string();
                        eds.file_path.push_str(text.trim());
                    }
                } else if reading_cf_display {
                    if let Some(cf) = cur_cf.as_mut() {
                        let text = e.unescape().unwrap_or_default().to_string();
                        cf.display.push_str(text.trim());
                    }
                }
            }

            _ => {}
        }

        buf.clear();
    }

    for s in &mut scripts {
        if let Some(steps) = script_steps.get(&s.id) {
            s.step_count = steps.len();
        }
    }

    Ok(ParsedDatabase {
        file_name,
        scripts,
        script_steps,
        layouts,
        tables,
        external_sources,
        table_occurrences,
        relationships,
        custom_functions,
        script_calls,
        script_layouts,
    })
}

/// Extract the inner text from a snippet like `<Text><![CDATA[…]]></Text>` (FMSaveAsXML
/// custom function bodies always use this shape). Falls back to the trimmed raw text
/// if no Text/CDATA wrappers are found.
fn extract_cdata_or_text(s: &str) -> String {
    let trimmed = s.trim();
    let inner = if let Some(stripped) = trimmed.strip_prefix("<Text>") {
        stripped.strip_suffix("</Text>").unwrap_or(stripped)
    } else {
        trimmed
    };
    let inner = inner.trim();
    let cdata = if let Some(stripped) = inner.strip_prefix("<![CDATA[") {
        stripped.strip_suffix("]]>").unwrap_or(stripped)
    } else {
        inner
    };
    // FileMaker stores calculation line breaks as bare CR (\r). Normalize to LF so
    // the resulting .fmcalc file reads correctly in standard editors.
    cdata.replace("\r\n", "\n").replace('\r', "\n")
}

// LayoutFull needs a couple of "compat" fields so we can use ..Default::default().
// Keep them out of the public Default by using #[serde(skip)] for the compat-only field.
impl LayoutFull {
    // (placeholder for future use; intentionally empty)
}

// ─── Output generation ────────────────────────────────────────────────────────

pub fn write_inspection(db: &ParsedDatabase, output_dir: &str) -> Result<InspectionStats, String> {
    let out = std::path::Path::new(output_dir);
    std::fs::create_dir_all(out).map_err(|e| format!("Cannot create {}: {}", output_dir, e))?;

    let scripts_dir = out.join("scripts");
    let layouts_dir = out.join("layouts");
    let tables_dir = out.join("tables");
    let analysis_dir = out.join("analysis");
    std::fs::create_dir_all(&scripts_dir).map_err(|e| format!("mkdir scripts: {}", e))?;
    std::fs::create_dir_all(&layouts_dir).map_err(|e| format!("mkdir layouts: {}", e))?;
    std::fs::create_dir_all(&tables_dir).map_err(|e| format!("mkdir tables: {}", e))?;
    std::fs::create_dir_all(&analysis_dir).map_err(|e| format!("mkdir analysis: {}", e))?;

    // ── Scripts ──────────────────────────────────────────────────────────────
    let mut scripts_written = 0usize;
    let mut script_summaries: Vec<ScriptSummary> = Vec::new();
    for script in &db.scripts {
        let file = if !script.is_folder && !script.is_separator {
            if let Some(steps) = db.script_steps.get(&script.id) {
                let fmscript = crate::xmss::FmScript {
                    steps: steps.clone(),
                };
                let text = format_script(&fmscript);
                let safe_name = sanitize_filename(&script.name);
                let filename = format!("{:04}_{}.fmscript", script.id, safe_name);
                let path = scripts_dir.join(&filename);
                std::fs::write(&path, &text)
                    .map_err(|e| format!("write {}: {}", path.display(), e))?;
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

    // ── Layouts (one JSON per layout + summary) ──────────────────────────────
    let mut layout_summaries: Vec<LayoutInfo> = Vec::new();
    for layout in &db.layouts {
        layout_summaries.push(LayoutInfo {
            id: layout.id,
            name: layout.name.clone(),
            hidden: layout.hidden,
            table_occurrence: layout.table_occurrence.clone(),
            table_occurrence_id: layout.table_occurrence_id,
            is_folder: layout.is_folder,
        });
        let safe = sanitize_filename(&layout.name);
        let filename = format!("{:04}_{}.json", layout.id, safe);
        let path = layouts_dir.join(filename);
        let json = serde_json::to_string_pretty(layout).map_err(|e| format!("json: {}", e))?;
        std::fs::write(&path, &json).map_err(|e| format!("write {}: {}", path.display(), e))?;
    }
    let layouts_index =
        serde_json::to_string_pretty(&layout_summaries).map_err(|e| format!("json: {}", e))?;
    std::fs::write(out.join("layouts.json"), &layouts_index)
        .map_err(|e| format!("write layouts.json: {}", e))?;

    // ── Tables ────────────────────────────────────────────────────────────────
    let total_fields: usize = db.tables.iter().map(|t| t.fields.len()).sum();
    for table in &db.tables {
        let safe = sanitize_filename(&table.name);
        let path = tables_dir.join(format!("{}.json", safe));
        let json = serde_json::to_string_pretty(table).map_err(|e| format!("json: {}", e))?;
        std::fs::write(&path, &json).map_err(|e| format!("write {}: {}", path.display(), e))?;
    }

    // ── External sources, TOs, relationships ─────────────────────────────────
    let eds_json =
        serde_json::to_string_pretty(&db.external_sources).map_err(|e| format!("json: {}", e))?;
    std::fs::write(out.join("external_sources.json"), &eds_json)
        .map_err(|e| format!("write external_sources.json: {}", e))?;

    let tos_json =
        serde_json::to_string_pretty(&db.table_occurrences).map_err(|e| format!("json: {}", e))?;
    std::fs::write(out.join("table_occurrences.json"), &tos_json)
        .map_err(|e| format!("write table_occurrences.json: {}", e))?;

    let rels_json =
        serde_json::to_string_pretty(&db.relationships).map_err(|e| format!("json: {}", e))?;
    std::fs::write(out.join("relationships.json"), &rels_json)
        .map_err(|e| format!("write relationships.json: {}", e))?;

    // Mermaid ER diagram — opens in any Markdown viewer with Mermaid support
    // (VSCode + Markdown Preview Enhanced, GitHub, mermaid.live).
    let mermaid = build_mermaid_diagram(&db.relationships, &db.table_occurrences);
    std::fs::write(out.join("relationships.mmd"), &mermaid)
        .map_err(|e| format!("write relationships.mmd: {}", e))?;

    // ── Custom Functions: one .fmcalc per function + index ──────────────────
    let cfs_dir = out.join("custom_functions");
    std::fs::create_dir_all(&cfs_dir).map_err(|e| format!("mkdir custom_functions: {}", e))?;
    for cf in &db.custom_functions {
        let safe = sanitize_filename(&cf.name);
        let filename = format!("{:04}_{}.fmcalc", cf.id, safe);
        let header = format!(
            "// {}\n// Parameters: {}\n\n",
            cf.display,
            cf.parameters.join(", ")
        );
        let body = format!("{}{}", header, cf.calculation);
        std::fs::write(cfs_dir.join(&filename), &body)
            .map_err(|e| format!("write {}: {}", filename, e))?;
    }
    let cfs_json =
        serde_json::to_string_pretty(&db.custom_functions).map_err(|e| format!("json: {}", e))?;
    std::fs::write(out.join("custom_functions.json"), &cfs_json)
        .map_err(|e| format!("write custom_functions.json: {}", e))?;

    // ── Analysis ─────────────────────────────────────────────────────────────
    let analysis = build_analysis(db);
    let analysis_json =
        serde_json::to_string_pretty(&analysis).map_err(|e| format!("json: {}", e))?;
    std::fs::write(analysis_dir.join("analysis.json"), &analysis_json)
        .map_err(|e| format!("write analysis.json: {}", e))?;

    // ── Manifest ──────────────────────────────────────────────────────────────
    let manifest = Manifest {
        file_name: db.file_name.clone(),
        script_count: db
            .scripts
            .iter()
            .filter(|s| !s.is_folder && !s.is_separator)
            .count(),
        layout_count: db.layouts.len(),
        table_count: db.tables.len(),
        field_count: total_fields,
        table_occurrence_count: db.table_occurrences.len(),
        relationship_count: db.relationships.len(),
        external_source_count: db.external_sources.len(),
        custom_function_count: db.custom_functions.len(),
        scripts: script_summaries,
        layouts: layout_summaries,
    };
    let manifest_json =
        serde_json::to_string_pretty(&manifest).map_err(|e| format!("json: {}", e))?;
    std::fs::write(out.join("manifest.json"), &manifest_json)
        .map_err(|e| format!("write manifest.json: {}", e))?;

    Ok(InspectionStats {
        scripts_written,
        layouts: db.layouts.len(),
        tables: db.tables.len(),
        fields: total_fields,
        table_occurrences: db.table_occurrences.len(),
        relationships: db.relationships.len(),
        external_sources: db.external_sources.len(),
        custom_functions: db.custom_functions.len(),
        unreferenced_scripts: analysis.unreferenced_scripts.len(),
    })
}

pub struct InspectionStats {
    pub scripts_written: usize,
    pub layouts: usize,
    pub tables: usize,
    pub fields: usize,
    pub table_occurrences: usize,
    pub relationships: usize,
    pub external_sources: usize,
    pub custom_functions: usize,
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

    let mut referenced_ids: HashSet<u32> = HashSet::new();
    let mut call_graph: Vec<CallGraphEntry> = Vec::new();

    for (caller_id, callees) in &db.script_calls {
        let caller_name = id_to_name.get(caller_id).copied().unwrap_or("").to_string();
        for callee_id in callees {
            if all_ids.contains(callee_id) {
                referenced_ids.insert(*callee_id);
                call_graph.push(CallGraphEntry {
                    caller_id: *caller_id,
                    caller_name: caller_name.clone(),
                    callee_id: *callee_id,
                    callee_name: id_to_name.get(callee_id).copied().unwrap_or("").to_string(),
                });
            }
        }
    }

    // Scripts triggered from layouts (button actions + object ScriptTriggers +
    // layout-level ScriptTriggers + anything inside portals) count as referenced.
    // layout.triggered_scripts is the precomputed aggregate from collect_aggregates.
    let mut scripts_triggered_by_layouts: Vec<LayoutScriptTrigger> = Vec::new();
    for layout in &db.layouts {
        for sid in &layout.triggered_scripts {
            if all_ids.contains(sid) {
                referenced_ids.insert(*sid);
            }
            let name = id_to_name.get(sid).copied().unwrap_or("").to_string();
            scripts_triggered_by_layouts.push(LayoutScriptTrigger {
                layout_id: layout.id,
                layout_name: layout.name.clone(),
                script_id: *sid,
                script_name: name,
            });
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

    // Layouts referenced by scripts (Go to Layout / New Window).
    let mut layout_to_scripts: HashMap<String, HashSet<String>> = HashMap::new();
    for (caller_id, layouts) in &db.script_layouts {
        let caller_name = id_to_name.get(caller_id).copied().unwrap_or("").to_string();
        for lname in layouts {
            layout_to_scripts
                .entry(lname.clone())
                .or_default()
                .insert(caller_name.clone());
        }
    }
    let mut layouts_used_by_scripts: Vec<LayoutUsage> = layout_to_scripts
        .into_iter()
        .map(|(lname, scripts)| {
            let mut v: Vec<String> = scripts.into_iter().collect();
            v.sort();
            LayoutUsage {
                layout_name: lname,
                used_by_scripts: v,
            }
        })
        .collect();
    layouts_used_by_scripts.sort_by(|a, b| a.layout_name.cmp(&b.layout_name));

    // External dependencies: TOs grouped by data source file.
    let mut external_dependencies: HashMap<String, Vec<String>> = HashMap::new();
    for to in &db.table_occurrences {
        if !to.data_source.is_empty() {
            external_dependencies
                .entry(to.data_source.clone())
                .or_default()
                .push(to.name.clone());
        }
    }

    AnalysisReport {
        unreferenced_scripts,
        call_graph,
        layouts_used_by_scripts,
        scripts_triggered_by_layouts,
        external_dependencies,
    }
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

fn parse_u32_attr(e: &quick_xml::events::BytesStart, name: &str) -> Option<u32> {
    let nb = name.as_bytes();
    for attr in e.attributes().flatten() {
        if attr.key.as_ref() == nb {
            return String::from_utf8_lossy(&attr.value).parse().ok();
        }
    }
    None
}

fn parse_bool_attr(e: &quick_xml::events::BytesStart, name: &str) -> bool {
    let nb = name.as_bytes();
    for attr in e.attributes().flatten() {
        if attr.key.as_ref() == nb {
            return &attr.value[..] == b"True";
        }
    }
    false
}

/// Walk a layout's object tree (top-level objects + nested portal children)
/// and aggregate every script id triggered and every TO referenced — by any
/// kind of attachment: button action, object ScriptTrigger, field, portal.
/// Build a Mermaid `erDiagram` document from relationships. Each TO becomes
/// a node, each relationship an edge labelled with the join predicates.
/// Sanitizes names because Mermaid IDs can't have special chars.
fn build_mermaid_diagram(rels: &[Relationship], tos: &[TableOccurrence]) -> String {
    let mut out = String::from("erDiagram\n");
    // Track which TOs participate so we can show their base table as a comment.
    let mut used: HashSet<String> = HashSet::new();
    for r in rels {
        used.insert(r.left_to.clone());
        used.insert(r.right_to.clone());
    }
    let to_by_name: HashMap<&str, &TableOccurrence> =
        tos.iter().map(|t| (t.name.as_str(), t)).collect();
    for r in rels {
        let l = mermaid_id(&r.left_to);
        let rt = mermaid_id(&r.right_to);
        let label = if r.predicates.is_empty() {
            "rel".to_string()
        } else {
            let p = &r.predicates[0];
            let extra = if r.predicates.len() > 1 {
                format!(" +{}", r.predicates.len() - 1)
            } else {
                String::new()
            };
            mermaid_label(&format!("{}={}{}", p.left_field, p.right_field, extra))
        };
        out.push_str(&format!("    {} ||--o{{ {} : \"{}\"\n", l, rt, label));
    }
    // Node decorations: show base table name for each TO used.
    for to_name in &used {
        if let Some(to) = to_by_name.get(to_name.as_str()) {
            let id = mermaid_id(to_name);
            let table = if to.data_source.is_empty() {
                to.base_table.clone()
            } else {
                format!("{}__{}", to.data_source, to.base_table)
            };
            out.push_str(&format!(
                "    {} {{\n        string {}\n    }}\n",
                id,
                mermaid_id(&table)
            ));
        }
    }
    out
}

fn mermaid_id(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn mermaid_label(s: &str) -> String {
    s.replace('"', "'").replace('\n', " ")
}

fn collect_aggregates(
    objects: &[LayoutObjectRef],
    scripts: &mut HashSet<u32>,
    tos: &mut HashSet<String>,
) {
    for o in objects {
        if let Some(sid) = o.button_script_id {
            scripts.insert(sid);
        }
        for t in &o.script_triggers {
            if t.script_id != 0 {
                scripts.insert(t.script_id);
            }
        }
        if let Some(ref t) = o.field_table_occurrence {
            tos.insert(t.clone());
        }
        if let Some(ref t) = o.portal_table_occurrence {
            tos.insert(t.clone());
        }
        if !o.children.is_empty() {
            collect_aggregates(&o.children, scripts, tos);
        }
    }
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

/// Extract all ScriptReference id values from a chunk of XML.
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

/// Extract layout names referenced in a chunk of XML (LayoutReference name="...").
fn extract_layout_refs(xml: &str) -> Vec<String> {
    let mut refs = Vec::new();
    let mut pos = 0;
    while let Some(i) = xml[pos..].find("<LayoutReference ") {
        let tag_start = pos + i;
        let after = &xml[tag_start..];
        if let Some(gt) = after.find('>') {
            let tag = &after[..gt];
            if let Some(name) = extract_xml_attr(tag, "name") {
                if !name.is_empty() {
                    refs.push(name.to_string());
                }
            }
            pos = tag_start + gt + 1;
        } else {
            break;
        }
    }
    refs
}

fn extract_xml_attr<'a>(tag: &'a str, attr_name: &str) -> Option<&'a str> {
    let needle = format!(" {}=\"", attr_name);
    let start = tag.find(&needle)? + needle.len();
    let end = tag[start..].find('"')? + start;
    Some(&tag[start..end])
}

/// Transform a FMSaveAsXML ObjectList payload into the XMSS clipboard format that
/// `parse_fmxml_snippet` understands. Two phases:
///   1. Collapse double Calculation wrappers
///      `<Calculation datatype="N" position="M"><Calculation>…</Calculation></Calculation>`
///      → `<Calculation>…</Calculation>`
///   2. Rewrite step parameter wrappers:
///      - `<Parameter type="Boolean"><Boolean id="X" value="True"/></Parameter>` → `<Set state="True"/>`
///        (skip Boolean with type="Collapsed", which is UI-state for If/Loop)
///      - `<Parameter type="List"><List …><ScriptReference id name UUID/></List></Parameter>`
///        → `<Script id name/>` (Perform Script target)
///      - `<Parameter type="LayoutReferenceContainer">…<LayoutReference id name UUID/>…</Parameter>`
///        → `<Layout id name/>` (Go to Layout / New Window destination)
///      - Drop the `<ParameterValues>`, `<Parameter type="…">`, `<List>`,
///        `<LayoutReferenceContainer>`, `<Animation/>` wrappers
///      - Lowercase `<value>` / `<repetition>` (Set Variable wrappers) → `<Value>` / `<Repetition>`
///
/// Safe to run on a Script ObjectList: ScriptReference/LayoutReference inside there
/// can only be step targets (the catalog-level <Script>/<Layout> references live outside).
fn fmsavexml_to_xmss(xml: &str) -> String {
    let step1 = normalize_calculations(xml);
    transform_step_tags(&step1)
}

fn transform_step_tags(xml: &str) -> String {
    let bytes = xml.as_bytes();
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] != b'<' {
            out.push(bytes[i] as char);
            i += 1;
            continue;
        }
        let tag_start = i;
        let mut j = i + 1;
        while j < bytes.len() && bytes[j] != b'>' {
            j += 1;
        }
        let tag_end = if j < bytes.len() { j + 1 } else { j };
        let tag = &xml[tag_start..tag_end];

        // ── Drop wrappers entirely (both open and close) ──────────────────
        // Note: </Boolean>, </ScriptReference>, </LayoutReference> end up here too
        // because we rewrite the corresponding opens to self-closing tags, so the
        // close tag must be consumed or the XML becomes unbalanced.
        if tag.starts_with("<ParameterValues")
            || tag == "</ParameterValues>"
            || tag.starts_with("<Parameter type=")
            || tag == "<Parameter>"
            || tag == "</Parameter>"
            || tag.starts_with("<List ")
            || tag == "</List>"
            || tag.starts_with("<LayoutReferenceContainer")
            || tag == "</LayoutReferenceContainer>"
            || tag.starts_with("<Animation ")
            || tag.starts_with("<Animation/>")
            || tag == "</Animation>"
            || tag == "</Boolean>"
            || tag == "</ScriptReference>"
            || tag == "</LayoutReference>"
            // Unwrap <Text> inside <Calculation>: in FMSaveAsXML scripts, <Text> only
            // ever appears as a direct child of <Calculation> (verified across 24955
            // occurrences). XMSS clipboard parser captures Calculation text directly
            // into the `.calculation` field, so dropping the <Text> wrapper makes the
            // calc text route correctly instead of landing in `.text`.
            || tag == "<Text>"
            || tag == "</Text>"
        {
            i = tag_end;
            continue;
        }

        // ── Boolean: state vs UI-collapsed flag ──────────────────────────
        // <Boolean type="Collapsed" .../> is If/Loop's "is the block collapsed in the UI"
        // flag — irrelevant to logic, drop it.
        // <Boolean id="..." value="True/False"/> (no type="Collapsed") is the actual state
        // for Set Error Capture, Allow User Abort, etc. Rewrite as <Set state="X"/>.
        if tag.starts_with("<Boolean") {
            if tag.contains("type=\"Collapsed\"") {
                i = tag_end;
                continue;
            }
            if let Some(value) = extract_xml_attr(tag, "value") {
                out.push_str(&format!("<Set state=\"{}\"/>", value));
                i = tag_end;
                continue;
            }
            // Unknown shape — fall through and emit raw.
        }

        // ── ScriptReference → Script (Perform Script target) ─────────────
        if tag.starts_with("<ScriptReference") {
            let id = extract_xml_attr(tag, "id").unwrap_or("");
            let name = extract_xml_attr(tag, "name").unwrap_or("");
            out.push_str(&format!(
                "<Script id=\"{}\" name=\"{}\"/>",
                id,
                xml_escape_attr(name)
            ));
            i = tag_end;
            continue;
        }

        // ── LayoutReference → Layout (Go to Layout / New Window) ─────────
        if tag.starts_with("<LayoutReference") {
            let id = extract_xml_attr(tag, "id").unwrap_or("");
            let name = extract_xml_attr(tag, "name").unwrap_or("");
            out.push_str(&format!(
                "<Layout id=\"{}\" name=\"{}\"/>",
                id,
                xml_escape_attr(name)
            ));
            i = tag_end;
            continue;
        }

        // ── Lowercase Set Variable wrappers ──────────────────────────────
        if tag == "<value>" {
            out.push_str("<Value>");
            i = tag_end;
            continue;
        }
        if tag == "</value>" {
            out.push_str("</Value>");
            i = tag_end;
            continue;
        }
        if tag == "<repetition>" {
            out.push_str("<Repetition>");
            i = tag_end;
            continue;
        }
        if tag == "</repetition>" {
            out.push_str("</Repetition>");
            i = tag_end;
            continue;
        }

        // Default: emit as-is.
        out.push_str(tag);
        i = tag_end;
    }

    out
}

/// Escape a string for use as an XML attribute value.
fn xml_escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

/// Collapse FMSaveAsXML's double-nested Calculation wrapper to XMSS single-level form.
fn normalize_calculations(xml: &str) -> String {
    let bytes = xml.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] != b'<' {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        let tag_start = i;
        let mut j = i + 1;
        while j < bytes.len() && bytes[j] != b'>' {
            j += 1;
        }
        let tag_end = if j < bytes.len() { j + 1 } else { j };
        let tag = &xml[tag_start..tag_end];

        if tag.starts_with("<Calculation ") && !tag.starts_with("</") {
            i = tag_end;
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
                            i = k_end;
                            break;
                        }
                    } else if inner_tag.starts_with("<Calculation") && !inner_tag.starts_with("</")
                    {
                        depth += 1;
                    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal-but-real FMSaveAsXML: one base table with two fields, and one
    /// script with a single step. Exercises the full parse → write_inspection
    /// path end-to-end and guards the CLI integration against future refactors.
    const FIXTURE: &str = r#"<FMSaveAsXML File="Test.fmp12">
  <BaseTableCatalog>
    <BaseTable id="1" name="Contacts"/>
  </BaseTableCatalog>
  <FieldsForTables>
    <BaseTableReference id="1" name="Contacts"/>
    <Field id="1" name="Name" fieldtype="Normal" datatype="Text"/>
    <Field id="2" name="Email" fieldtype="Normal" datatype="Text"/>
  </FieldsForTables>
  <ScriptCatalog>
    <Script id="10" name="DoThing"/>
  </ScriptCatalog>
  <StepsForScripts>
    <StepsForScript>
      <ScriptReference id="10" name="DoThing"/>
      <ObjectList>
        <Step enable="True" id="1" name="Comment"><Text>hi</Text></Step>
      </ObjectList>
    </StepsForScript>
  </StepsForScripts>
</FMSaveAsXML>"#;

    /// Unique temp dir under the OS temp, so parallel test runs don't collide.
    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("fmbridge-test-{}-{}", tag, nanos));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn inspect_parses_tables_fields_and_scripts() {
        let dir = temp_dir("parse");
        let xml_path = dir.join("export.xml");
        std::fs::write(&xml_path, FIXTURE).unwrap();

        let db = parse(xml_path.to_str().unwrap()).unwrap();
        assert_eq!(db.file_name, "Test.fmp12");
        assert_eq!(db.tables.len(), 1);
        assert_eq!(db.tables[0].name, "Contacts");
        assert_eq!(db.tables[0].fields.len(), 2);
        let real_scripts = db
            .scripts
            .iter()
            .filter(|s| !s.is_folder && !s.is_separator)
            .count();
        assert_eq!(real_scripts, 1);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn inspect_writes_manifest_and_script_files() {
        let dir = temp_dir("write");
        let xml_path = dir.join("export.xml");
        std::fs::write(&xml_path, FIXTURE).unwrap();
        let out_dir = dir.join("out");

        let db = parse(xml_path.to_str().unwrap()).unwrap();
        let stats = write_inspection(&db, out_dir.to_str().unwrap()).unwrap();

        assert_eq!(stats.scripts_written, 1);
        assert_eq!(stats.tables, 1);
        assert_eq!(stats.fields, 2);
        assert!(out_dir.join("manifest.json").exists());
        assert!(
            out_dir
                .join("scripts")
                .join("0010_DoThing.fmscript")
                .exists()
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
