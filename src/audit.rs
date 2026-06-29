// Referential-integrity audit over a parsed FMSaveAsXML database.
//
// This is Phase 3 ("the gold"): instead of just exposing the schema, cross the
// scripts against the schema and flag *dangling references* — the bugs a human
// would hunt by hand. Everything here is structural (id/name lookups against the
// catalogs already parsed by fmsavexml), so there's no calculation parsing and
// no false positives from dynamic/cross-file targets (those are skipped).

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::fmsavexml::{LayoutObjectRef, ParsedDatabase};

/// One problem found in the database.
#[derive(Debug, Serialize)]
pub struct Issue {
    /// Machine-readable category, e.g. "broken-script-call".
    pub kind: String,
    pub severity: String, // "error" for dangling refs
    /// Where it lives, e.g. "script: Main" or "relationship #5".
    pub location: String,
    pub detail: String,
}

#[derive(Debug, Serialize)]
pub struct AuditReport {
    pub file_name: String,
    pub issue_count: usize,
    /// Count per `kind`, for a quick human summary.
    pub by_kind: HashMap<String, usize>,
    pub issues: Vec<Issue>,
}

/// A literal name we can actually verify — not a `$variable` or a calculation.
fn is_literal_name(s: &str) -> bool {
    let s = s.trim().trim_matches('"');
    !s.is_empty()
        && !s.starts_with('$')
        && !s.contains(['(', ')', '&', ';', '"', '$'])
}

/// Run all integrity checks and return every issue found (sorted, stable order).
pub fn audit(db: &ParsedDatabase) -> AuditReport {
    let mut issues: Vec<Issue> = Vec::new();

    // ── Catalogs (what legitimately exists) ──
    let real_scripts: Vec<_> = db
        .scripts
        .iter()
        .filter(|s| !s.is_folder && !s.is_separator)
        .collect();
    let script_ids: HashSet<u32> = real_scripts.iter().map(|s| s.id).collect();
    let script_names: HashSet<&str> = real_scripts.iter().map(|s| s.name.as_str()).collect();
    let script_name_by_id: HashMap<u32, &str> =
        db.scripts.iter().map(|s| (s.id, s.name.as_str())).collect();

    let layout_ids: HashSet<u32> = db.layouts.iter().filter(|l| !l.is_folder).map(|l| l.id).collect();
    let layout_names: HashSet<&str> = db
        .layouts
        .iter()
        .filter(|l| !l.is_folder)
        .map(|l| l.name.as_str())
        .collect();

    let to_names: HashSet<&str> = db.table_occurrences.iter().map(|t| t.name.as_str()).collect();
    let to_ids: HashSet<u32> = db.table_occurrences.iter().map(|t| t.id).collect();
    let table_names: HashSet<&str> = db.tables.iter().map(|t| t.name.as_str()).collect();

    // ── A & B: per-script step references (Perform Script, Go to Layout) ──
    for (caller_id, steps) in &db.script_steps {
        let caller = script_name_by_id.get(caller_id).copied().unwrap_or("(unknown)");
        for st in steps {
            match st.name.as_str() {
                "Perform Script" | "Perform Script on Server" => {
                    // Cross-file calls target another database — can't verify here.
                    if st.script_target_file.is_some() {
                        continue;
                    }
                    let id_ok = st
                        .script_target_id
                        .as_deref()
                        .and_then(|s| s.parse::<u32>().ok())
                        .map(|id| script_ids.contains(&id))
                        .unwrap_or(false);
                    let name = st.script_target_name.as_deref().unwrap_or("");
                    let name_ok = !name.is_empty() && script_names.contains(name);
                    // Dynamic (by-calculation) targets aren't verifiable — skip.
                    let verifiable = st.script_target_id.is_some() || is_literal_name(name);
                    if verifiable && !id_ok && !name_ok {
                        issues.push(Issue {
                            kind: "broken-script-call".to_string(),
                            severity: "error".to_string(),
                            location: format!("script: {}", caller),
                            detail: format!(
                                "Perform Script targets '{}'{} which doesn't exist in this file",
                                if name.is_empty() { "?" } else { name },
                                st.script_target_id
                                    .as_deref()
                                    .map(|i| format!(" (#{})", i))
                                    .unwrap_or_default(),
                            ),
                        });
                    }
                }
                "Go to Layout" | "New Window" => {
                    if st.layout_destination.as_deref() == Some("OriginalLayout") {
                        continue;
                    }
                    let id = st.layout_id.as_deref().and_then(|s| s.parse::<u32>().ok());
                    let name = st.layout_name.as_deref().map(|n| n.trim().trim_matches('"'));
                    let id_ok = id.map(|i| layout_ids.contains(&i)).unwrap_or(false);
                    let name_ok = name.map(|n| layout_names.contains(n)).unwrap_or(false);
                    let broken = if id.is_some() {
                        !id_ok && !name_ok
                    } else {
                        // No id: only flag a clearly-literal name we can trust.
                        name.map(|n| is_literal_name(n) && !name_ok).unwrap_or(false)
                    };
                    if broken {
                        issues.push(Issue {
                            kind: "broken-layout-ref".to_string(),
                            severity: "error".to_string(),
                            location: format!("script: {}", caller),
                            detail: format!(
                                "{} targets layout '{}'{} which doesn't exist",
                                st.name,
                                name.unwrap_or("?"),
                                id.map(|i| format!(" (#{})", i)).unwrap_or_default(),
                            ),
                        });
                    }
                }
                _ => {}
            }
        }
    }

    // ── C: relationships referencing missing table occurrences ──
    for r in &db.relationships {
        for (side, to) in [("left", &r.left_to), ("right", &r.right_to)] {
            if !to.is_empty() && !to_names.contains(to.as_str()) {
                issues.push(Issue {
                    kind: "broken-relationship-to".to_string(),
                    severity: "error".to_string(),
                    location: format!("relationship #{}", r.id),
                    detail: format!(
                        "{} side references table occurrence '{}' which doesn't exist",
                        side, to
                    ),
                });
            }
        }
    }

    // ── D: internal table occurrences whose base table is gone ──
    for to in &db.table_occurrences {
        // data_source empty == this file; external TOs point elsewhere — skip.
        if to.data_source.is_empty()
            && !to.base_table.is_empty()
            && !table_names.contains(to.base_table.as_str())
        {
            issues.push(Issue {
                kind: "broken-to-base-table".to_string(),
                severity: "error".to_string(),
                location: format!("table occurrence: {}", to.name),
                detail: format!(
                    "based on table '{}' which doesn't exist in this file",
                    to.base_table
                ),
            });
        }
    }

    // ── E & F: layout-level references ──
    for l in &db.layouts {
        if l.is_folder {
            continue;
        }
        // E: the layout's own table occurrence.
        if let Some(toid) = l.table_occurrence_id {
            let name_ok = l
                .table_occurrence
                .as_deref()
                .map(|n| to_names.contains(n))
                .unwrap_or(false);
            if toid != 0 && !to_ids.contains(&toid) && !name_ok {
                issues.push(Issue {
                    kind: "broken-layout-to".to_string(),
                    severity: "error".to_string(),
                    location: format!("layout: {}", l.name),
                    detail: format!(
                        "shows table occurrence '{}' (#{}) which doesn't exist",
                        l.table_occurrence.as_deref().unwrap_or("?"),
                        toid
                    ),
                });
            }
        }
        // F: objects placed on the layout (fields, portals) — recurse.
        check_objects(&l.objects, &l.name, &to_names, &mut issues);
    }

    issues.sort_by(|a, b| {
        (a.location.as_str(), a.kind.as_str(), a.detail.as_str()).cmp(&(
            b.location.as_str(),
            b.kind.as_str(),
            b.detail.as_str(),
        ))
    });

    let mut by_kind: HashMap<String, usize> = HashMap::new();
    for i in &issues {
        *by_kind.entry(i.kind.clone()).or_insert(0) += 1;
    }

    AuditReport {
        file_name: db.file_name.clone(),
        issue_count: issues.len(),
        by_kind,
        issues,
    }
}

/// Recursively flag fields/portals placed on a table occurrence that no longer
/// exists (a classic "ghost field on the layout" after a TO is deleted).
fn check_objects(
    objects: &[LayoutObjectRef],
    layout: &str,
    to_names: &HashSet<&str>,
    issues: &mut Vec<Issue>,
) {
    for o in objects {
        if let Some(to) = &o.field_table_occurrence {
            if !to.is_empty() && !to_names.contains(to.as_str()) {
                issues.push(Issue {
                    kind: "broken-layout-field-to".to_string(),
                    severity: "error".to_string(),
                    location: format!("layout: {}", layout),
                    detail: format!(
                        "field '{}' sits on table occurrence '{}' which doesn't exist",
                        o.field_name.as_deref().unwrap_or("?"),
                        to
                    ),
                });
            }
        }
        if let Some(to) = &o.portal_table_occurrence {
            if !to.is_empty() && !to_names.contains(to.as_str()) {
                issues.push(Issue {
                    kind: "broken-portal-to".to_string(),
                    severity: "error".to_string(),
                    location: format!("layout: {}", layout),
                    detail: format!("portal bound to table occurrence '{}' which doesn't exist", to),
                });
            }
        }
        check_objects(&o.children, layout, to_names, issues);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fmsavexml::{LayoutFull, Relationship, ScriptInfo, TableInfo, TableOccurrence};
    use crate::xmss::ScriptStep;

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
    fn flags_broken_refs_and_passes_valid_ones() {
        let mut script_steps = HashMap::new();
        script_steps.insert(
            10u32,
            vec![
                // broken: no such script id/name
                ScriptStep {
                    name: "Perform Script".to_string(),
                    script_target_id: Some("999".to_string()),
                    script_target_name: Some("Ghost Script".to_string()),
                    ..Default::default()
                },
                // valid: targets the existing script #11
                ScriptStep {
                    name: "Perform Script".to_string(),
                    script_target_id: Some("11".to_string()),
                    script_target_name: Some("Helper".to_string()),
                    ..Default::default()
                },
                // cross-file: must be skipped (not a bug)
                ScriptStep {
                    name: "Perform Script".to_string(),
                    script_target_id: Some("3".to_string()),
                    script_target_name: Some("Remote".to_string()),
                    script_target_file: Some("Other.fmp12".to_string()),
                    ..Default::default()
                },
                // broken layout
                ScriptStep {
                    name: "Go to Layout".to_string(),
                    layout_name: Some("Ghost Layout".to_string()),
                    ..Default::default()
                },
            ],
        );

        let db = ParsedDatabase {
            file_name: "T.fmp12".to_string(),
            scripts: vec![script(10, "Main"), script(11, "Helper")],
            script_steps,
            layouts: vec![LayoutFull {
                id: 1,
                name: "Home".to_string(),
                table_occurrence_id: Some(99), // missing TO
                table_occurrence: Some("Ghost".to_string()),
                ..Default::default()
            }],
            tables: vec![TableInfo { id: 1, name: "Contacts".to_string(), fields: vec![] }],
            table_occurrences: vec![
                TableOccurrence { id: 1, name: "Contacts".to_string(), base_table: "Contacts".to_string(), ..Default::default() },
                // internal TO whose base table is gone
                TableOccurrence { id: 2, name: "Orphan_TO".to_string(), base_table: "DeletedTable".to_string(), ..Default::default() },
            ],
            relationships: vec![Relationship {
                id: 5,
                left_to: "Contacts".to_string(),
                right_to: "GHOST_TO".to_string(), // missing
                ..Default::default()
            }],
            ..Default::default()
        };

        let report = audit(&db);
        let kinds: Vec<&str> = report.issues.iter().map(|i| i.kind.as_str()).collect();

        assert!(kinds.contains(&"broken-script-call"));
        assert!(kinds.contains(&"broken-layout-ref"));
        assert!(kinds.contains(&"broken-relationship-to"));
        assert!(kinds.contains(&"broken-to-base-table"));
        assert!(kinds.contains(&"broken-layout-to"));
        // The valid Perform Script (#11) and cross-file one must NOT be flagged:
        // exactly one broken-script-call.
        assert_eq!(report.by_kind["broken-script-call"], 1);
    }

    #[test]
    fn clean_database_has_no_issues() {
        let db = ParsedDatabase {
            file_name: "Clean.fmp12".to_string(),
            tables: vec![TableInfo { id: 1, name: "T".to_string(), fields: vec![] }],
            table_occurrences: vec![TableOccurrence {
                id: 1,
                name: "T".to_string(),
                base_table: "T".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(audit(&db).issue_count, 0);
    }
}
