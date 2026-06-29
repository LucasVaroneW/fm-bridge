// Cross-reference queries over a parsed FMSaveAsXML database.
//
// Phase 3 bug-hunting helpers that answer the two questions a developer asks
// constantly before changing anything:
//   - who_calls(script)      → what fires this script (so I know the blast radius)
//   - who_uses_field(field)  → where a field is referenced (layouts, relationships,
//                              Set Field steps, and calc mentions)
// All structural (reuses the catalogs fmsavexml already parsed); the only fuzzy
// part is "calc mention" (a token search of calculation bodies), clearly labelled.

use std::collections::HashMap;

use serde::Serialize;

use crate::fmsavexml::{LayoutObjectRef, ParsedDatabase};

// ─── who_calls ───

#[derive(Debug, Serialize)]
pub struct Caller {
    /// "Perform Script", "layout trigger (OnRecordLoad)", "button", "object trigger (OnObjectEnter)"
    pub via: String,
    pub location: String,
}

#[derive(Debug, Serialize)]
pub struct WhoCallsReport {
    pub target_id: u32,
    pub target_name: String,
    pub caller_count: usize,
    pub callers: Vec<Caller>,
}

/// Resolve a script query — `#<id>` or an exact name — to (id, name).
fn resolve_script(db: &ParsedDatabase, query: &str) -> Result<(u32, String), String> {
    let q = query.trim();
    if let Some(num) = q.strip_prefix('#') {
        let id: u32 = num.parse().map_err(|_| format!("Invalid script id: {}", q))?;
        return db
            .scripts
            .iter()
            .find(|s| s.id == id && !s.is_folder && !s.is_separator)
            .map(|s| (s.id, s.name.clone()))
            .ok_or_else(|| format!("No script with id #{}", id));
    }
    db.scripts
        .iter()
        .find(|s| s.name == q && !s.is_folder && !s.is_separator)
        .map(|s| (s.id, s.name.clone()))
        .ok_or_else(|| format!("No script named '{}'", q))
}

pub fn who_calls(db: &ParsedDatabase, query: &str) -> Result<WhoCallsReport, String> {
    let (target_id, target_name) = resolve_script(db, query)?;
    let name_by_id: HashMap<u32, &str> =
        db.scripts.iter().map(|s| (s.id, s.name.as_str())).collect();
    let mut callers: Vec<Caller> = Vec::new();

    // Scripts that Perform Script the target.
    for (caller_id, callees) in &db.script_calls {
        if callees.contains(&target_id) {
            callers.push(Caller {
                via: "Perform Script".to_string(),
                location: format!(
                    "script: {}",
                    name_by_id.get(caller_id).copied().unwrap_or("(unknown)")
                ),
            });
        }
    }

    // Layout-level and object-level triggers/buttons.
    for l in &db.layouts {
        for t in &l.layout_triggers {
            if t.script_id == target_id {
                callers.push(Caller {
                    via: format!("layout trigger ({})", t.event),
                    location: format!("layout: {}", l.name),
                });
            }
        }
        collect_layout_callers(&l.objects, target_id, &l.name, &mut callers);
    }

    callers.sort_by(|a, b| (a.location.as_str(), a.via.as_str()).cmp(&(b.location.as_str(), b.via.as_str())));
    callers.dedup_by(|a, b| a.location == b.location && a.via == b.via);

    Ok(WhoCallsReport {
        target_id,
        target_name,
        caller_count: callers.len(),
        callers,
    })
}

fn collect_layout_callers(
    objects: &[LayoutObjectRef],
    target: u32,
    layout: &str,
    out: &mut Vec<Caller>,
) {
    for o in objects {
        if o.button_script_id == Some(target) {
            out.push(Caller {
                via: "button".to_string(),
                location: format!("layout: {}", layout),
            });
        }
        for t in &o.script_triggers {
            if t.script_id == target {
                out.push(Caller {
                    via: format!("object trigger ({})", t.event),
                    location: format!("layout: {}", layout),
                });
            }
        }
        collect_layout_callers(&o.children, target, layout, out);
    }
}

// ─── who_uses_field ───

#[derive(Debug, Serialize)]
pub struct FieldUse {
    /// "layout-field", "relationship-key", "set-field", "calc-mention"
    pub kind: String,
    pub location: String,
    pub detail: String,
}

#[derive(Debug, Serialize)]
pub struct WhoUsesFieldReport {
    pub field: String,
    pub table_occurrence: Option<String>,
    pub use_count: usize,
    pub uses: Vec<FieldUse>,
}

/// True when `field` appears in `text` as a standalone token (not as a substring
/// of a longer identifier). Keeps calc-mention search from matching e.g. `id`
/// inside `valid`.
fn mentions(text: &str, field: &str) -> bool {
    if field.is_empty() {
        return false;
    }
    let bytes = text.as_bytes();
    let fb = field.as_bytes();
    let is_ident = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut i = 0;
    while let Some(pos) = text[i..].find(field) {
        let start = i + pos;
        let end = start + fb.len();
        let before_ok = start == 0 || !is_ident(bytes[start - 1]);
        let after_ok = end >= bytes.len() || !is_ident(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        i = start + 1;
    }
    false
}

pub fn who_uses_field(db: &ParsedDatabase, query: &str) -> WhoUsesFieldReport {
    // Accept "TableOccurrence::Field" or a bare "Field".
    let (to, field) = match query.split_once("::") {
        Some((t, f)) => (Some(t.trim().to_string()), f.trim().to_string()),
        None => (None, query.trim().to_string()),
    };
    let to_matches = |candidate: Option<&str>| -> bool {
        match (&to, candidate) {
            (None, _) => true,
            (Some(want), Some(got)) => want == got,
            (Some(_), None) => false,
        }
    };
    let mut uses: Vec<FieldUse> = Vec::new();

    // Layout placements (fields/portals, any nesting).
    for l in &db.layouts {
        collect_field_placements(&l.objects, &field, &to, &l.name, &mut uses);
    }

    // Relationship join keys.
    for r in &db.relationships {
        for p in &r.predicates {
            if p.left_field == field && to_matches(Some(p.left_to.as_str())) {
                uses.push(FieldUse {
                    kind: "relationship-key".to_string(),
                    location: format!("relationship #{}", r.id),
                    detail: format!("left key {}::{}", p.left_to, p.left_field),
                });
            }
            if p.right_field == field && to_matches(Some(p.right_to.as_str())) {
                uses.push(FieldUse {
                    kind: "relationship-key".to_string(),
                    location: format!("relationship #{}", r.id),
                    detail: format!("right key {}::{}", p.right_to, p.right_field),
                });
            }
        }
    }

    // Script steps: Set Field / Replace targets + calc mentions.
    let name_by_id: HashMap<u32, &str> =
        db.scripts.iter().map(|s| (s.id, s.name.as_str())).collect();
    for (sid, steps) in &db.script_steps {
        let sname = name_by_id.get(sid).copied().unwrap_or("(unknown)");
        for st in steps {
            let is_set = matches!(st.name.as_str(), "Set Field" | "Replace Field Contents");
            if is_set
                && st.field_target.as_deref() == Some(field.as_str())
                && to_matches(st.field_table.as_deref())
            {
                uses.push(FieldUse {
                    kind: "set-field".to_string(),
                    location: format!("script: {}", sname),
                    detail: format!("{} target", st.name),
                });
            }
            if let Some(calc) = &st.calculation {
                if mentions(calc, &field) {
                    uses.push(FieldUse {
                        kind: "calc-mention".to_string(),
                        location: format!("script: {}", sname),
                        detail: format!("mentioned in a {} calculation", st.name),
                    });
                }
            }
        }
    }

    // Field calculations and custom functions that mention it.
    for t in &db.tables {
        for f in &t.fields {
            if let Some(calc) = &f.calculation {
                if mentions(calc, &field) {
                    uses.push(FieldUse {
                        kind: "calc-mention".to_string(),
                        location: format!("field calc: {}::{}", t.name, f.name),
                        detail: "mentioned in a field calculation".to_string(),
                    });
                }
            }
        }
    }
    for cf in &db.custom_functions {
        if mentions(&cf.calculation, &field) {
            uses.push(FieldUse {
                kind: "calc-mention".to_string(),
                location: format!("custom function: {}", cf.name),
                detail: "mentioned in the function body".to_string(),
            });
        }
    }

    uses.sort_by(|a, b| {
        (a.kind.as_str(), a.location.as_str(), a.detail.as_str()).cmp(&(
            b.kind.as_str(),
            b.location.as_str(),
            b.detail.as_str(),
        ))
    });

    WhoUsesFieldReport {
        field,
        table_occurrence: to,
        use_count: uses.len(),
        uses,
    }
}

fn collect_field_placements(
    objects: &[LayoutObjectRef],
    field: &str,
    to: &Option<String>,
    layout: &str,
    out: &mut Vec<FieldUse>,
) {
    for o in objects {
        if o.field_name.as_deref() == Some(field) {
            let to_ok = match (to, &o.field_table_occurrence) {
                (None, _) => true,
                (Some(want), Some(got)) => want == got,
                (Some(_), None) => false,
            };
            if to_ok {
                out.push(FieldUse {
                    kind: "layout-field".to_string(),
                    location: format!("layout: {}", layout),
                    detail: o
                        .field_table_occurrence
                        .as_deref()
                        .map(|t| format!("placed as {}::{}", t, field))
                        .unwrap_or_else(|| format!("placed field {}", field)),
                });
            }
        }
        collect_field_placements(&o.children, field, to, layout, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fmsavexml::{
        JoinPredicate, LayoutFull, Relationship, ScriptInfo, ScriptTriggerRef, TableInfo,
    };
    use crate::xmss::ScriptStep;
    use std::collections::HashMap;

    fn script(id: u32, name: &str) -> ScriptInfo {
        ScriptInfo {
            id,
            name: name.to_string(),
            uuid: String::new(),
            hidden: false,
            is_folder: false,
            is_separator: false,
            run_with_full_access: false,
            step_count: 0,
            folder: String::new(),
        }
    }

    #[test]
    fn who_calls_finds_scripts_and_layout_triggers() {
        let mut script_calls = HashMap::new();
        script_calls.insert(1u32, vec![99u32]); // script 1 calls target 99
        let db = ParsedDatabase {
            scripts: vec![script(1, "Caller"), script(99, "Target")],
            script_calls,
            layouts: vec![LayoutFull {
                id: 5,
                name: "Home".to_string(),
                layout_triggers: vec![ScriptTriggerRef {
                    event: "OnRecordLoad".to_string(),
                    script_id: 99,
                    script_name: "Target".to_string(),
                    parameter: None,
                    modes: vec![],
                }],
                ..Default::default()
            }],
            ..Default::default()
        };

        let r = who_calls(&db, "Target").unwrap();
        assert_eq!(r.target_id, 99);
        assert_eq!(r.caller_count, 2);
        assert!(r.callers.iter().any(|c| c.location == "script: Caller"));
        assert!(r.callers.iter().any(|c| c.via.contains("OnRecordLoad")));

        // Resolve by #id too, and error on unknown.
        assert_eq!(who_calls(&db, "#99").unwrap().target_id, 99);
        assert!(who_calls(&db, "Nope").is_err());
    }

    #[test]
    fn who_uses_field_covers_relationships_setfield_and_calc() {
        let mut script_steps = HashMap::new();
        script_steps.insert(
            1u32,
            vec![
                ScriptStep {
                    name: "Set Field".to_string(),
                    field_target: Some("Status".to_string()),
                    field_table: Some("Orders".to_string()),
                    ..Default::default()
                },
                ScriptStep {
                    name: "Set Variable".to_string(),
                    calculation: Some("If ( Status = 1 ; \"ok\" ; \"no\" )".to_string()),
                    ..Default::default()
                },
            ],
        );
        let db = ParsedDatabase {
            scripts: vec![script(1, "Proc")],
            script_steps,
            relationships: vec![Relationship {
                id: 7,
                predicates: vec![JoinPredicate {
                    op: "Equal".to_string(),
                    left_to: "Orders".to_string(),
                    left_field: "Status".to_string(),
                    right_to: "Lookup".to_string(),
                    right_field: "code".to_string(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
            tables: vec![TableInfo {
                id: 1,
                name: "Orders".to_string(),
                fields: vec![],
            }],
            ..Default::default()
        };

        let r = who_uses_field(&db, "Status");
        let kinds: Vec<&str> = r.uses.iter().map(|u| u.kind.as_str()).collect();
        assert!(kinds.contains(&"set-field"));
        assert!(kinds.contains(&"relationship-key"));
        assert!(kinds.contains(&"calc-mention"));

        // Token search must not match a field name embedded in a longer word.
        assert!(!mentions("Invalidated = 1", "id"));
        assert!(mentions("If ( id = 1 )", "id"));
    }
}
