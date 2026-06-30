// Readable DSL for opaque steps whose inner XML carries options we'd otherwise
// show as a raw blob (or, before they were made opaque, drop entirely).
//
// Same safety contract as `import_records`: a renderer is only used when the
// round-trip `xml -> dsl -> xml` reproduces the original **byte-for-byte**. If
// anything doesn't model cleanly, we return `None` and the caller keeps the
// verbatim XML — so a readable view is never a lossy one.
//
// This module is the dispatcher: `to_dsl` / `from_dsl` route by step name.
// Import/Export Records keep living in `import_records`; Commit Records and Go
// to Related Record are handled here.

use crate::import_records::{element_attr, element_attrs};

/// Render an opaque step's inner XML as a readable DSL, or `None` to keep raw.
pub fn to_dsl(step_name: &str, xml: &str) -> Option<String> {
    let dsl = match step_name {
        "Import Records" | "Export Records" => return crate::import_records::xml_to_dsl(xml),
        "Commit Records/Requests" => commit_to_dsl(xml)?,
        "Go to Related Record" => gtrr_to_dsl(xml)?,
        _ => return None,
    };
    // Lossless gate: only offer the DSL if it rebuilds the exact XML.
    if from_dsl(step_name, &dsl).as_deref() == Some(xml) {
        Some(dsl)
    } else {
        None
    }
}

/// Rebuild the inner XML from a step's DSL, or `None` if it isn't ours/malformed.
/// Accepts both the indented (newline-separated) and inline (" | "-separated)
/// forms — the inline form is normalized to lines first.
pub fn from_dsl(step_name: &str, dsl: &str) -> Option<String> {
    let normalized = dsl.replace(" | ", "\n");
    let dsl = normalized.as_str();
    match step_name {
        "Import Records" | "Export Records" => crate::import_records::dsl_to_xml(dsl),
        "Commit Records/Requests" => commit_from_dsl(dsl),
        "Go to Related Record" => gtrr_from_dsl(dsl),
        _ => None,
    }
}

// ─── Commit Records/Requests ───────────────────────────────────────────────────
// Inner shape: <NoInteract state=…><Option state=…><ESSForceCommit state=…>
// (each element optional). Order is fixed by FileMaker.

fn commit_to_dsl(xml: &str) -> Option<String> {
    let mut lines = Vec::new();
    if let Some(s) = element_attr(xml, "NoInteract", "state") {
        lines.push(format!("Dialog: {}", if s == "True" { "Off" } else { "On" }));
    }
    if let Some(s) = element_attr(xml, "Option", "state") {
        lines.push(format!("SkipDataEntryValidation: {}", s));
    }
    if let Some(s) = element_attr(xml, "ESSForceCommit", "state") {
        lines.push(format!("ForceCommit: {}", s));
    }
    if lines.is_empty() {
        return None;
    }
    Some(lines.join("\n"))
}

fn commit_from_dsl(dsl: &str) -> Option<String> {
    let mut dialog = None;
    let mut skip = None;
    let mut force = None;
    for raw in dsl.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let (key, value) = line.split_once(':')?;
        let value = value.trim();
        match key.trim() {
            "Dialog" => dialog = Some(if value == "Off" { "True" } else { "False" }),
            "SkipDataEntryValidation" => skip = Some(value.to_string()),
            "ForceCommit" => force = Some(value.to_string()),
            _ => return None,
        }
    }
    let mut xml = String::new();
    if let Some(d) = dialog {
        xml.push_str(&format!("<NoInteract state=\"{}\"></NoInteract>", d));
    }
    if let Some(s) = skip {
        xml.push_str(&format!("<Option state=\"{}\"></Option>", s));
    }
    if let Some(f) = force {
        xml.push_str(&format!("<ESSForceCommit state=\"{}\"></ESSForceCommit>", f));
    }
    if xml.is_empty() {
        return None;
    }
    Some(xml)
}

// ─── Go to Related Record ──────────────────────────────────────────────────────
// Inner shape (each element optional, FileMaker order):
//   <Option state=…><MatchAllRecords state=…><ShowInNewWindow state=…>
//   <Restore state=…><LayoutDestination value=…><NewWndStyles …/>
//   <Table id=… name=…><Layout id=… name=…>
// The Table (related TO) and Layout are the meaningful bits; we surface those
// first, then the flags. NewWndStyles is carried verbatim (it holds localized
// window-style values), but it's one line, not a blob.

fn gtrr_to_dsl(xml: &str) -> Option<String> {
    let mut lines = Vec::new();
    if let (Some(id), Some(name)) =
        (element_attr(xml, "Table", "id"), element_attr(xml, "Table", "name"))
    {
        lines.push(format!("Table: {} #{}", name, id));
    }
    if let (Some(id), Some(name)) =
        (element_attr(xml, "Layout", "id"), element_attr(xml, "Layout", "name"))
    {
        lines.push(format!("Layout: {} #{}", name, id));
    }
    if let Some(s) = element_attr(xml, "Option", "state") {
        lines.push(format!("Option: {}", s));
    }
    if let Some(s) = element_attr(xml, "MatchAllRecords", "state") {
        lines.push(format!("MatchAllRecords: {}", s));
    }
    if let Some(s) = element_attr(xml, "ShowInNewWindow", "state") {
        lines.push(format!("ShowInNewWindow: {}", s));
    }
    if let Some(s) = element_attr(xml, "Restore", "state") {
        lines.push(format!("Restore: {}", s));
    }
    if let Some(v) = element_attr(xml, "LayoutDestination", "value") {
        lines.push(format!("LayoutDestination: {}", v));
    }
    if let Some(a) = element_attrs(xml, "NewWndStyles") {
        lines.push(format!("NewWindowStyles: {}", a));
    }
    if lines.is_empty() {
        return None;
    }
    Some(lines.join("\n"))
}

fn gtrr_from_dsl(dsl: &str) -> Option<String> {
    let mut table = None;
    let mut layout = None;
    let mut option = None;
    let mut match_all = None;
    let mut show_new = None;
    let mut restore = None;
    let mut layout_dest = None;
    let mut styles = None;
    for raw in dsl.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let (key, value) = line.split_once(':')?;
        let value = value.trim();
        match key.trim() {
            "Table" => {
                let hash = value.rfind(" #")?;
                table = Some((value[hash + 2..].to_string(), value[..hash].to_string()));
            }
            "Layout" => {
                let hash = value.rfind(" #")?;
                layout = Some((value[hash + 2..].to_string(), value[..hash].to_string()));
            }
            "Option" => option = Some(value.to_string()),
            "MatchAllRecords" => match_all = Some(value.to_string()),
            "ShowInNewWindow" => show_new = Some(value.to_string()),
            "Restore" => restore = Some(value.to_string()),
            "LayoutDestination" => layout_dest = Some(value.to_string()),
            "NewWindowStyles" => styles = Some(value.to_string()),
            _ => return None,
        }
    }
    // Rebuild in FileMaker's element order, regardless of DSL line order.
    let mut xml = String::new();
    if let Some(s) = option {
        xml.push_str(&format!("<Option state=\"{}\"></Option>", s));
    }
    if let Some(s) = match_all {
        xml.push_str(&format!("<MatchAllRecords state=\"{}\"></MatchAllRecords>", s));
    }
    if let Some(s) = show_new {
        xml.push_str(&format!("<ShowInNewWindow state=\"{}\"></ShowInNewWindow>", s));
    }
    if let Some(s) = restore {
        xml.push_str(&format!("<Restore state=\"{}\"></Restore>", s));
    }
    if let Some(v) = layout_dest {
        xml.push_str(&format!("<LayoutDestination value=\"{}\"></LayoutDestination>", v));
    }
    if let Some(a) = styles {
        xml.push_str(&format!("<NewWndStyles {}></NewWndStyles>", a));
    }
    if let Some((id, name)) = table {
        xml.push_str(&format!("<Table id=\"{}\" name=\"{}\"></Table>", id, name));
    }
    if let Some((id, name)) = layout {
        xml.push_str(&format!("<Layout id=\"{}\" name=\"{}\"></Layout>", id, name));
    }
    if xml.is_empty() {
        return None;
    }
    Some(xml)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_round_trips() {
        let xml = "<NoInteract state=\"False\"></NoInteract><Option state=\"True\"></Option>\
                   <ESSForceCommit state=\"True\"></ESSForceCommit>";
        let dsl = to_dsl("Commit Records/Requests", xml).expect("dsl");
        assert!(dsl.contains("SkipDataEntryValidation: True"));
        assert!(dsl.contains("ForceCommit: True"));
        assert_eq!(from_dsl("Commit Records/Requests", &dsl).as_deref(), Some(xml));
    }

    #[test]
    fn gtrr_round_trips_and_surfaces_table_layout() {
        let xml = "<Option state=\"False\"></Option><MatchAllRecords state=\"True\"></MatchAllRecords>\
                   <ShowInNewWindow state=\"False\"></ShowInNewWindow><Restore state=\"True\"></Restore>\
                   <LayoutDestination value=\"SelectedLayout\"></LayoutDestination>\
                   <NewWndStyles Style=\"Document\" Close=\"Sí\" Minimize=\"Sí\" Maximize=\"Sí\" Resize=\"Sí\" Styles=\"3606018\"></NewWndStyles>\
                   <Table id=\"1068907\" name=\"Ta_i_ProductosVersiones\"></Table>\
                   <Layout id=\"2206\" name=\"Imp_ProductosVersiones_Sin\"></Layout>";
        let dsl = to_dsl("Go to Related Record", xml).expect("dsl");
        assert!(dsl.contains("Table: Ta_i_ProductosVersiones #1068907"));
        assert!(dsl.contains("Layout: Imp_ProductosVersiones_Sin #2206"));
        assert!(dsl.contains("MatchAllRecords: True"));
        assert_eq!(from_dsl("Go to Related Record", &dsl).as_deref(), Some(xml));
    }

    #[test]
    fn unknown_step_is_none() {
        assert_eq!(to_dsl("Set Field", "<x></x>"), None);
    }

    #[test]
    fn inline_form_parses_back() {
        // The inline writer joins DSL fields with " | "; from_dsl must accept it
        // and rebuild the exact same XML as the indented (newline) form.
        let xml = "<NoInteract state=\"False\"></NoInteract><Option state=\"True\"></Option>\
                   <ESSForceCommit state=\"True\"></ESSForceCommit>";
        let indented = to_dsl("Commit Records/Requests", xml).unwrap();
        let inline = indented
            .lines()
            .map(str::trim)
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(!inline.contains('\n'));
        assert_eq!(from_dsl("Commit Records/Requests", &inline).as_deref(), Some(xml));
    }
}
