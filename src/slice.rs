// `fm-bridge slice` — given an inspect output and a list of layouts, copy out
// the focused subset (those layouts + transitive scripts + referenced TOs +
// joining relationships + custom functions actually used) into a slice dir.
//
// Goal: collapse 100MB+ of database context into ~30 files that fit in an AI
// context window for "rewrite layouts X+Y as a web viewer" tasks.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Counts describing what a slice ended up containing. Returned by `run_slice`
/// so callers (CLI text output, JSON mode) decide how to present it instead of
/// the function printing to stdout itself.
#[derive(Debug, Clone, Serialize)]
pub struct SliceStats {
    pub layouts: usize,
    pub scripts_seed: usize,
    pub scripts_closure: usize,
    pub table_occurrences: usize,
    pub relationships: usize,
    pub custom_functions: usize,
    pub external_sources: usize,
}

// ─── Minimal deserialization types ────────────────────────────────────────────
// We re-declare only the fields we read, so slice doesn't have to import the
// full ParsedDatabase types. JSON inputs come from a previous inspect run.

#[derive(Deserialize)]
struct LayoutFile {
    id: u32,
    name: String,
    table_occurrence: Option<String>,
    objects: Vec<LayoutObject>,
    triggered_scripts: Vec<u32>,
    referenced_tos: Vec<String>,
}

#[derive(Deserialize)]
struct LayoutObject {
    #[allow(dead_code)]
    object_type: String,
    button_script_id: Option<u32>,
    #[allow(dead_code)]
    button_script_name: Option<String>,
    field_table_occurrence: Option<String>,
    portal_table_occurrence: Option<String>,
    #[serde(default)]
    script_triggers: Vec<TriggerRef>,
    #[serde(default)]
    children: Vec<LayoutObject>,
}

#[derive(Deserialize)]
struct TriggerRef {
    script_id: u32,
}

#[derive(Deserialize)]
struct ManifestFile {
    scripts: Vec<ScriptSummaryFile>,
    layouts: Vec<LayoutSummaryFile>,
}

#[derive(Deserialize, Clone)]
struct ScriptSummaryFile {
    id: u32,
    name: String,
    file: Option<String>,
}

#[derive(Deserialize, Clone)]
struct LayoutSummaryFile {
    id: u32,
    name: String,
}

#[derive(Deserialize, Clone, serde::Serialize)]
struct TableOccurrence {
    id: u32,
    name: String,
    source_type: String,
    data_source: String,
    base_table: String,
}

#[derive(Deserialize, Clone, serde::Serialize)]
struct Relationship {
    id: u32,
    left_to: String,
    right_to: String,
    left_cascade_create: bool,
    left_cascade_delete: bool,
    right_cascade_create: bool,
    right_cascade_delete: bool,
    predicates: Vec<JoinPredicate>,
}

#[derive(Deserialize, Clone, serde::Serialize)]
struct JoinPredicate {
    op: String,
    left_to: String,
    left_field: String,
    right_to: String,
    right_field: String,
}

#[derive(Deserialize, Clone, serde::Serialize)]
struct CustomFunctionMeta {
    id: u32,
    name: String,
    #[serde(default)]
    display: String,
    #[serde(default)]
    parameters: Vec<String>,
}

#[derive(Deserialize, Clone, serde::Serialize)]
struct ExternalDataSource {
    id: u32,
    name: String,
    source_type: String,
    file_path: String,
}

#[derive(Deserialize)]
struct AnalysisFile {
    call_graph: Vec<CallGraphEntry>,
}

#[derive(Deserialize, Clone)]
struct CallGraphEntry {
    caller_id: u32,
    callee_id: u32,
}

// ─── Entry point ──────────────────────────────────────────────────────────────

pub fn run_slice(
    output_dir: &str,
    slice_dir: &str,
    layout_names: &[String],
) -> Result<SliceStats, String> {
    let out = Path::new(output_dir);
    let slice = Path::new(slice_dir);

    if !out.exists() {
        return Err(format!("Output dir not found: {}", output_dir));
    }
    fs::create_dir_all(slice).map_err(|e| format!("mkdir {}: {}", slice_dir, e))?;

    // ── Load inputs ──────────────────────────────────────────────────────────
    let manifest: ManifestFile = read_json(&out.join("manifest.json"))?;
    let analysis: AnalysisFile = read_json(&out.join("analysis").join("analysis.json"))?;
    let all_tos: Vec<TableOccurrence> = read_json(&out.join("table_occurrences.json"))?;
    let all_rels: Vec<Relationship> = read_json(&out.join("relationships.json"))?;
    let all_cfs: Vec<CustomFunctionMeta> = read_json(&out.join("custom_functions.json"))?;
    let all_eds: Vec<ExternalDataSource> = read_json(&out.join("external_sources.json"))?;

    // Resolve requested layouts (case-insensitive name match).
    let mut wanted_layouts: Vec<LayoutSummaryFile> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    for requested in layout_names {
        let req_l = requested.to_lowercase();
        let found = manifest
            .layouts
            .iter()
            .find(|l| l.name.to_lowercase() == req_l);
        match found {
            Some(l) => wanted_layouts.push(l.clone()),
            None => missing.push(requested.clone()),
        }
    }
    if !missing.is_empty() {
        return Err(format!(
            "Layouts not found: {}. Available layouts can be listed from manifest.json.",
            missing.join(", ")
        ));
    }
    if wanted_layouts.is_empty() {
        return Err("No layouts specified".to_string());
    }

    // ── Load layout JSONs (the full LayoutFull for each requested) ──────────
    let layouts_dir = out.join("layouts");
    let mut layout_files: Vec<LayoutFile> = Vec::new();
    for l in &wanted_layouts {
        let safe = sanitize_filename(&l.name);
        let filename = format!("{:04}_{}.json", l.id, safe);
        let path = layouts_dir.join(filename);
        let lf: LayoutFile = read_json(&path)?;
        layout_files.push(lf);
    }

    // ── Seed: scripts triggered by these layouts ─────────────────────────────
    let mut script_closure: HashSet<u32> = HashSet::new();
    for lf in &layout_files {
        for sid in &lf.triggered_scripts {
            script_closure.insert(*sid);
        }
        // Also include scripts referenced by individual layout buttons (same data,
        // but defensive).
        for obj in &lf.objects {
            if let Some(sid) = obj.button_script_id {
                script_closure.insert(sid);
            }
        }
    }
    let seed_count = script_closure.len();

    // ── Transitive closure via call_graph ────────────────────────────────────
    let mut caller_to_callees: HashMap<u32, Vec<u32>> = HashMap::new();
    for e in &analysis.call_graph {
        caller_to_callees
            .entry(e.caller_id)
            .or_default()
            .push(e.callee_id);
    }
    loop {
        let mut added = false;
        let current: Vec<u32> = script_closure.iter().copied().collect();
        for caller in current {
            if let Some(callees) = caller_to_callees.get(&caller) {
                for callee in callees {
                    if script_closure.insert(*callee) {
                        added = true;
                    }
                }
            }
        }
        if !added {
            break;
        }
    }

    // ── Build script_id → file lookup and grep their bodies for TOs / CFs ───
    let script_by_id: HashMap<u32, ScriptSummaryFile> =
        manifest.scripts.iter().map(|s| (s.id, s.clone())).collect();
    let scripts_dir = out.join("scripts");
    let slice_scripts_dir = slice.join("scripts");
    fs::create_dir_all(&slice_scripts_dir).map_err(|e| format!("mkdir scripts: {}", e))?;

    let mut copied_scripts: Vec<&ScriptSummaryFile> = Vec::new();
    let mut all_script_text = String::new();
    for sid in &script_closure {
        if let Some(s) = script_by_id.get(sid) {
            if let Some(ref file) = s.file {
                let src = scripts_dir.join(file);
                let dst = slice_scripts_dir.join(file);
                if let Ok(content) = fs::read_to_string(&src) {
                    // `file` may include a script-folder subdir; mirror it.
                    if let Some(parent) = dst.parent() {
                        fs::create_dir_all(parent)
                            .map_err(|e| format!("mkdir {}: {}", parent.display(), e))?;
                    }
                    fs::write(&dst, &content)
                        .map_err(|e| format!("write {}: {}", dst.display(), e))?;
                    all_script_text.push_str(&content);
                    all_script_text.push('\n');
                    copied_scripts.push(s);
                }
            }
        }
    }

    // ── Collect TOs: layout base + layout-referenced + grepped from scripts ─
    let mut wanted_to_names: HashSet<String> = HashSet::new();
    for lf in &layout_files {
        if let Some(ref t) = lf.table_occurrence {
            wanted_to_names.insert(t.clone());
        }
        for t in &lf.referenced_tos {
            wanted_to_names.insert(t.clone());
        }
        for obj in &lf.objects {
            if let Some(ref t) = obj.field_table_occurrence {
                wanted_to_names.insert(t.clone());
            }
            if let Some(ref t) = obj.portal_table_occurrence {
                wanted_to_names.insert(t.clone());
            }
        }
    }
    // Scripts may reference TOs as `TO::Field` — pick those up by scanning script bodies.
    for to in &all_tos {
        if to.name.is_empty() {
            continue;
        }
        let needle = format!("{}::", to.name);
        if all_script_text.contains(&needle) {
            wanted_to_names.insert(to.name.clone());
        }
    }

    let mut wanted_tos: Vec<TableOccurrence> = all_tos
        .iter()
        .filter(|t| wanted_to_names.contains(&t.name))
        .cloned()
        .collect();
    wanted_tos.sort_by_key(|t| t.id);

    // ── Relationships: include if either side is in the TO set ──────────────
    let wanted_rels: Vec<Relationship> = all_rels
        .iter()
        .filter(|r| wanted_to_names.contains(&r.left_to) || wanted_to_names.contains(&r.right_to))
        .cloned()
        .collect();

    // ── Custom functions actually referenced by these scripts ───────────────
    let cfs_dir = out.join("custom_functions");
    let slice_cfs_dir = slice.join("custom_functions");
    fs::create_dir_all(&slice_cfs_dir).map_err(|e| format!("mkdir custom_functions: {}", e))?;
    let mut wanted_cfs: Vec<CustomFunctionMeta> = Vec::new();
    for cf in &all_cfs {
        // Crude but effective: look for "NAME (" (case sensitive — FM is case-insensitive
        // but the canonical casing in scripts matches the CF definition).
        let needle = format!("{} (", cf.name);
        let alt_needle = format!("{}(", cf.name);
        if all_script_text.contains(&needle) || all_script_text.contains(&alt_needle) {
            wanted_cfs.push(cf.clone());
            let safe = sanitize_filename(&cf.name);
            let filename = format!("{:04}_{}.fmcalc", cf.id, safe);
            let src = cfs_dir.join(&filename);
            let dst = slice_cfs_dir.join(&filename);
            if let Ok(content) = fs::read_to_string(&src) {
                fs::write(&dst, &content).map_err(|e| format!("write {}: {}", dst.display(), e))?;
            }
        }
    }

    // ── External data sources referenced by the included TOs ────────────────
    let referenced_eds: HashSet<String> = wanted_tos
        .iter()
        .map(|t| t.data_source.clone())
        .filter(|s| !s.is_empty())
        .collect();
    let wanted_eds: Vec<ExternalDataSource> = all_eds
        .iter()
        .filter(|e| referenced_eds.contains(&e.name))
        .cloned()
        .collect();

    // ── Copy layout JSONs ────────────────────────────────────────────────────
    let slice_layouts_dir = slice.join("layouts");
    fs::create_dir_all(&slice_layouts_dir).map_err(|e| format!("mkdir layouts: {}", e))?;
    for l in &wanted_layouts {
        let safe = sanitize_filename(&l.name);
        let filename = format!("{:04}_{}.json", l.id, safe);
        let src = layouts_dir.join(&filename);
        let dst = slice_layouts_dir.join(&filename);
        if let Ok(content) = fs::read_to_string(&src) {
            fs::write(&dst, &content).map_err(|e| format!("write {}: {}", dst.display(), e))?;
        }
    }

    // ── Write subset JSONs ───────────────────────────────────────────────────
    write_json(&slice.join("table_occurrences.json"), &wanted_tos)?;
    write_json(&slice.join("relationships.json"), &wanted_rels)?;
    write_json(&slice.join("external_sources.json"), &wanted_eds)?;
    write_json(&slice.join("custom_functions.json"), &wanted_cfs)?;

    // Mermaid ER diagram restricted to this slice's TOs — much more readable than
    // the global one. Opens in any Markdown viewer with Mermaid support.
    let mermaid = slice_mermaid(&wanted_tos, &wanted_rels);
    fs::write(slice.join("relationships.mmd"), &mermaid)
        .map_err(|e| format!("write relationships.mmd: {}", e))?;

    // ── Write summary ────────────────────────────────────────────────────────
    let summary = build_summary(
        &wanted_layouts,
        &layout_files,
        seed_count,
        &copied_scripts,
        &wanted_tos,
        &wanted_rels,
        &wanted_cfs,
        &wanted_eds,
    );
    fs::write(slice.join("slice_summary.md"), &summary)
        .map_err(|e| format!("write slice_summary.md: {}", e))?;

    Ok(SliceStats {
        layouts: wanted_layouts.len(),
        scripts_seed: seed_count,
        scripts_closure: copied_scripts.len(),
        table_occurrences: wanted_tos.len(),
        relationships: wanted_rels.len(),
        custom_functions: wanted_cfs.len(),
        external_sources: wanted_eds.len(),
    })
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn read_json<T: for<'de> Deserialize<'de>>(path: &PathBuf) -> Result<T, String> {
    let s = fs::read_to_string(path).map_err(|e| format!("read {}: {}", path.display(), e))?;
    serde_json::from_str(&s).map_err(|e| format!("parse {}: {}", path.display(), e))
}

fn write_json<T: serde::Serialize>(path: &PathBuf, value: &T) -> Result<(), String> {
    let s = serde_json::to_string_pretty(value).map_err(|e| format!("json: {}", e))?;
    fs::write(path, &s).map_err(|e| format!("write {}: {}", path.display(), e))
}

fn slice_mermaid(tos: &[TableOccurrence], rels: &[Relationship]) -> String {
    let mut out = String::from("erDiagram\n");
    let mut used: HashSet<String> = HashSet::new();
    let to_by_name: HashMap<&str, &TableOccurrence> =
        tos.iter().map(|t| (t.name.as_str(), t)).collect();
    for r in rels {
        used.insert(r.left_to.clone());
        used.insert(r.right_to.clone());
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
            format!("{}={}{}", p.left_field, p.right_field, extra)
        };
        out.push_str(&format!("    {} ||--o{{ {} : \"{}\"\n", l, rt, label));
    }
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

#[allow(clippy::too_many_arguments)]
fn build_summary(
    requested: &[LayoutSummaryFile],
    layout_files: &[LayoutFile],
    seed_count: usize,
    scripts: &[&ScriptSummaryFile],
    tos: &[TableOccurrence],
    rels: &[Relationship],
    cfs: &[CustomFunctionMeta],
    eds: &[ExternalDataSource],
) -> String {
    let mut s = String::new();
    s.push_str("# Slice Summary\n\n");
    s.push_str("Focused subset of a FileMaker database, extracted around the requested layouts.\n");
    s.push_str("Includes their triggered scripts (transitively), referenced table occurrences,\n");
    s.push_str("joining relationships, and any custom functions actually used in the scripts.\n\n");
    s.push_str(
        "Each `layouts/*.json` carries: base TO, **all layout objects (recursively — portal\n",
    );
    s.push_str("contents included)** with field/TO refs, button → script links, tooltips,\n");
    s.push_str("object-level ScriptTriggers (OnObjectExit/OnObjectModify/…), and layout-level\n");
    s.push_str("ScriptTriggers (OnLayoutEnter/OnRecordCommit/…). `relationships.mmd` is a\n");
    s.push_str("Mermaid ER diagram of the relationships included.\n\n");

    s.push_str("## Requested layouts\n\n");
    for (l, lf) in requested.iter().zip(layout_files.iter()) {
        s.push_str(&format!("### {} (id {})\n", l.name, l.id));
        if let Some(ref t) = lf.table_occurrence {
            s.push_str(&format!("- Base TO: `{}`\n", t));
        }
        s.push_str(&format!("- Objects: {}\n", lf.objects.len()));
        s.push_str(&format!(
            "- Triggered scripts: {}\n",
            lf.triggered_scripts.len()
        ));
        if !lf.referenced_tos.is_empty() {
            s.push_str(&format!(
                "- Referenced TOs: {}\n",
                lf.referenced_tos.join(", ")
            ));
        }
        s.push('\n');
    }

    s.push_str("## Scripts\n\n");
    s.push_str(&format!(
        "- Seed (directly triggered by layouts): {}\n",
        seed_count
    ));
    s.push_str(&format!(
        "- After transitive call-graph closure: {}\n\n",
        scripts.len()
    ));
    s.push_str("All script bodies are in `scripts/`.\n\n");

    s.push_str("## Table occurrences (");
    s.push_str(&tos.len().to_string());
    s.push_str(")\n\n");
    for to in tos {
        s.push_str(&format!(
            "- `{}` → `{}::{}` ({})\n",
            to.name,
            if to.data_source.is_empty() {
                "internal"
            } else {
                &to.data_source
            },
            to.base_table,
            to.source_type
        ));
    }
    s.push('\n');

    s.push_str(&format!("## Relationships ({})\n\n", rels.len()));
    for r in rels.iter().take(50) {
        s.push_str(&format!("- `{}` ←→ `{}`", r.left_to, r.right_to));
        if !r.predicates.is_empty() {
            let preds: Vec<String> = r
                .predicates
                .iter()
                .map(|p| {
                    format!(
                        "{}.{} {} {}.{}",
                        p.left_to, p.left_field, p.op, p.right_to, p.right_field
                    )
                })
                .collect();
            s.push_str(&format!(" — {}", preds.join(" AND ")));
        }
        s.push('\n');
    }
    if rels.len() > 50 {
        s.push_str(&format!(
            "…and {} more (see relationships.json).\n",
            rels.len() - 50
        ));
    }
    s.push('\n');

    if !cfs.is_empty() {
        s.push_str(&format!("## Custom functions ({})\n\n", cfs.len()));
        for cf in cfs {
            s.push_str(&format!(
                "- `{}`",
                if cf.display.is_empty() {
                    &cf.name
                } else {
                    &cf.display
                }
            ));
            s.push('\n');
        }
        s.push_str("\nBodies are in `custom_functions/`.\n\n");
    }

    if !eds.is_empty() {
        s.push_str(&format!("## External data sources ({})\n\n", eds.len()));
        for e in eds {
            s.push_str(&format!(
                "- `{}` ({}): {}\n",
                e.name, e.source_type, e.file_path
            ));
        }
    }

    s
}
