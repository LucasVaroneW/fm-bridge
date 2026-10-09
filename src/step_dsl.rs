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
        "Import Records" | "Export Records" => {
            // Clipboard form: gated, fully round-trips (the read/write path).
            if let Some(d) = crate::import_records::xml_to_dsl(xml) {
                return Some(d);
            }
            // FMSaveAsXML form (an `inspect` of a whole-DB export): a readable,
            // **read-only** summary instead of the raw XML blob. inspect output is
            // documentation, not something you paste back, so it isn't gated for
            // round-trip — see import_fmsavexml_summary.
            return import_fmsavexml_summary(xml);
        }
        "Commit Records/Requests" => match strip_fmsavexml_envelope(xml) {
            // inspect form: read-only, not gated (see strip_fmsavexml_envelope).
            Some(clean) => return commit_to_dsl(&clean).filter(|_| only_known(&clean)),
            None => commit_to_dsl(xml)?,
        },
        "Go to Related Record" => gtrr_to_dsl(xml)?,
        n if is_flag_step(n) => match strip_fmsavexml_envelope(xml) {
            Some(clean) => return flags_to_dsl(n, &clean, true).filter(|_| only_known(&clean)),
            None => flags_to_dsl(n, xml, false)?,
        },
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
        n if is_flag_step(n) => flags_from_dsl(n, dsl),
        _ => None,
    }
}

/// True when every element left in a (cleaned) inspect payload is one of the
/// option tags we render — anything else (a Query, a SortList…) keeps it raw.
fn only_known(xml: &str) -> bool {
    const KNOWN: &[&str] = &["NoInteract", "Pause", "Restore", "Option", "ESSForceCommit"];
    xml.split('<')
        .filter(|p| !p.is_empty() && !p.starts_with('/'))
        .all(|p| {
            let tag = p
                .split(|c: char| c.is_whitespace() || c == '>' || c == '/')
                .next()
                .unwrap_or("");
            KNOWN.contains(&tag)
        })
}

// ─── Import Records (FMSaveAsXML form — inspect only) ──────────────────────────
// A whole-database export serializes Import Records very differently from the
// clipboard (`<ImportField>`/`<FilePathList>`/`<Map>`/`<FieldReference>` instead
// of `<TargetFields>`/`<Field>`), and the export is pretty-printed with tabs, so
// a byte-exact round-trip isn't practical. inspect output is read-only context,
// so here we render a readable **summary** (source, target TO, field map) — the
// thing a human or AI actually needs: which source column maps to which field.

/// Read-only readable summary of an FMSaveAsXML Import Records payload, or `None`
/// if this isn't that form (so the caller keeps the raw XML).
fn import_fmsavexml_summary(xml: &str) -> Option<String> {
    if !xml.contains("<ImportField") && !xml.contains("<FilePathList") {
        return None;
    }
    let mut lines: Vec<String> = Vec::new();
    lines.push("(inspect view — read-only)".to_string());

    if let Some(path) = crate::import_records::element_inner(xml, "FilePathList") {
        // FileMaker wraps the path in field-separator chars (decode to `â`); strip
        // those and surrounding whitespace.
        let clean = path
            .trim()
            .trim_matches(|c: char| !c.is_ascii_alphanumeric());
        if !clean.is_empty() {
            lines.push(format!("Source: {}", clean));
        }
    }
    if let Some(name) = crate::import_records::element_attr(xml, "TableOccurrenceReference", "name")
    {
        let id = crate::import_records::element_attr(xml, "TableOccurrenceReference", "id")
            .unwrap_or("");
        lines.push(format!("Target: {} #{}", name, id));
    }

    // Field map: each `<Map index="N" …><FieldReference name="F" …>`.
    let mut maps: Vec<String> = Vec::new();
    for chunk in xml.split("<Map ").skip(1) {
        let open_tag = format!("<Map {}>", chunk.split('>').next().unwrap_or(""));
        let index = crate::import_records::tag_attr(&open_tag, "index").unwrap_or("");
        let field = chunk.find("<FieldReference").and_then(|p| {
            let rest = &chunk[p..];
            let end = rest.find('>').map(|e| e + 1).unwrap_or(rest.len());
            crate::import_records::tag_attr(&rest[..end], "name")
        });
        if let Some(f) = field {
            maps.push(format!("  {} -> {}", index, f));
        }
    }
    if !maps.is_empty() {
        lines.push("Mapping:".to_string());
        lines.extend(maps);
    }

    if lines.len() <= 1 {
        return None; // nothing useful extracted → keep raw XML
    }
    Some(lines.join("\n"))
}

// ─── Commit Records/Requests ───────────────────────────────────────────────────
// Inner shape: <NoInteract state=…><Option state=…><ESSForceCommit state=…>
// (each element optional). Order is fixed by FileMaker.

fn commit_to_dsl(xml: &str) -> Option<String> {
    let mut lines = Vec::new();
    if let Some(s) = element_attr(xml, "NoInteract", "state") {
        lines.push(format!(
            "Dialog: {}",
            if s == "True" { "Off" } else { "On" }
        ));
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
    // Lines (indented form) or `;`-separated on one line, the way FileMaker
    // shows the step: `[With dialog: Off; Skip data entry validation]`.
    for line in split_flag_tokens(dsl) {
        if let Some(off) = dialog_flag(line) {
            dialog = Some(if off { "True" } else { "False" });
            continue;
        }
        let (key, value) = match line.split_once(':') {
            Some((k, v)) => (k.trim(), Some(v.trim())),
            None => (line, None), // bare flag = on
        };
        let key = normalize_key(key);
        let state = match value {
            None => "True".to_string(),
            Some(v) => on_off_state(v)?.to_string(),
        };
        match key.as_str() {
            "skipdataentryvalidation" => skip = Some(state),
            "forcecommit" | "overrideesslockingconflicts" => force = Some(state),
            _ => return None,
        }
    }
    // FileMaker always writes the three options; so do we, unstated ones at
    // their default (dialog shown, validate, no force) instead of leaving them
    // to whatever the paste assumes.
    if dialog.is_none() && skip.is_none() && force.is_none() {
        return None;
    }
    Some(format!(
        "<NoInteract state=\"{}\"></NoInteract><Option state=\"{}\"></Option>\
         <ESSForceCommit state=\"{}\"></ESSForceCommit>",
        dialog.unwrap_or("False"),
        skip.as_deref().unwrap_or("False"),
        force.as_deref().unwrap_or("False")
    ))
}

// ─── Shared option helpers ─────────────────────────────────────────────────────

/// `Some(true)` = dialog suppressed, `Some(false)` = dialog shown, for the
/// dialog option in any of its accepted spellings (case-insensitive):
/// `Dialog: Off|On` (fm-bridge's canonical form) or `With dialog: Off|On`
/// (FileMaker's own step text). `None` if `seg` isn't a dialog option.
pub fn dialog_flag(seg: &str) -> Option<bool> {
    let (key, value) = seg.trim().split_once(':')?;
    let key = normalize_key(key);
    if key != "dialog" && key != "withdialog" {
        return None;
    }
    match value.trim().to_ascii_lowercase().as_str() {
        "off" | "false" => Some(true),
        "on" | "true" => Some(false),
        _ => None,
    }
}

/// Lower-case a key and drop spaces/underscores: `Skip data entry validation`
/// and `SkipDataEntryValidation` compare equal.
fn normalize_key(key: &str) -> String {
    key.chars()
        .filter(|c| !c.is_whitespace() && *c != '_')
        .collect::<String>()
        .to_ascii_lowercase()
}

/// `On`/`True` → "True", `Off`/`False` → "False" (case-insensitive).
fn on_off_state(v: &str) -> Option<&'static str> {
    match v.trim().to_ascii_lowercase().as_str() {
        "on" | "true" => Some("True"),
        "off" | "false" => Some("False"),
        _ => None,
    }
}

/// Split a flag DSL into its options: one per line, or `;`-separated.
fn split_flag_tokens(dsl: &str) -> impl Iterator<Item = &str> {
    dsl.split(['\n', ';'])
        .map(str::trim)
        .filter(|t| !t.is_empty())
}

// ─── Flag-only steps (dialog / pause / restore) ────────────────────────────────
// Steps whose whole payload is a fixed sequence of `<Tag state="True|False">`
// options. Before, they were "plain": read dropped the options and write
// emitted none, so FileMaker pasted its defaults — dialog ON for the deletes,
// Restore ON for Enter Find Mode. Element order verified against a FileMaker 22
// DDR (Database Design Report, same step XML as the clipboard).
//
// Text (one line, `; `-separated, FileMaker's own wording):
//   Delete Record/Request [Dialog: Off]          (also `With dialog: Off`)
//   Enter Find Mode [Pause: Off]                 (`Restore` when it stores requests)
//   Sort Records [Dialog: Off]                   (a stored sort order stays raw XML)

#[derive(Clone, Copy, PartialEq)]
enum Flag {
    Dialog,
    Pause,
    Restore,
}

/// The ordered options of a flag-only step, or `None` if `step_name` isn't one.
fn flag_spec(step_name: &str) -> Option<&'static [(&'static str, Flag)]> {
    const DIALOG: &[(&str, Flag)] = &[("NoInteract", Flag::Dialog)];
    const FIND_MODE: &[(&str, Flag)] = &[("Pause", Flag::Pause), ("Restore", Flag::Restore)];
    const SORT: &[(&str, Flag)] = &[("NoInteract", Flag::Dialog), ("Restore", Flag::Restore)];
    match step_name {
        "Delete Record/Request"
        | "Delete Portal Row"
        | "Delete All Records"
        | "Revert Record/Request" => Some(DIALOG),
        "Enter Find Mode" => Some(FIND_MODE),
        "Sort Records" => Some(SORT),
        _ => None,
    }
}

/// True for the steps whose options this module models as flags. Used by the
/// parser so a bare step (no brackets) still gets explicit defaults.
pub fn is_flag_step(step_name: &str) -> bool {
    flag_spec(step_name).is_some()
}

/// The XML a flag step gets when written bare (no brackets): every option at
/// its default — dialog shown, pause off, restore off. Emitting them is what
/// keeps FileMaker from pasting its own defaults (Restore ON on Enter Find Mode).
pub fn default_flag_xml(step_name: &str) -> Option<String> {
    flags_from_dsl(step_name, "")
}

/// `lenient`: a missing option reads as its default (inspect form, where
/// FMSaveAsXML omits some); strict otherwise (the gate needs every element).
fn flags_to_dsl(step_name: &str, xml: &str, lenient: bool) -> Option<String> {
    let spec = flag_spec(step_name)?;
    let mut parts = Vec::new();
    for (tag, flag) in spec {
        let state = match element_attr(xml, tag, "state") {
            Some(s) => s,
            None if lenient => "False",
            None => return None,
        };
        match flag {
            Flag::Dialog => parts.push(format!(
                "Dialog: {}",
                if state == "True" { "Off" } else { "On" }
            )),
            Flag::Pause => parts.push(format!(
                "Pause: {}",
                if state == "True" { "On" } else { "Off" }
            )),
            Flag::Restore => {
                if state == "True" {
                    parts.push("Restore".to_string());
                }
            }
        }
    }
    Some(parts.join("; "))
}

fn flags_from_dsl(step_name: &str, dsl: &str) -> Option<String> {
    let spec = flag_spec(step_name)?;
    let mut dialog = "False"; // NoInteract: dialog shown
    let mut pause = "False";
    let mut restore = "False";
    let has = |f: Flag| spec.iter().any(|(_, g)| *g == f);
    for tok in split_flag_tokens(dsl) {
        if let Some(off) = dialog_flag(tok) {
            if !has(Flag::Dialog) {
                return None;
            }
            dialog = if off { "True" } else { "False" };
            continue;
        }
        let (key, value) = match tok.split_once(':') {
            Some((k, v)) => (normalize_key(k), Some(v)),
            None => (normalize_key(tok), None),
        };
        let state = match value {
            None => "True",
            Some(v) => on_off_state(v)?,
        };
        match key.as_str() {
            "pause" if has(Flag::Pause) => pause = state,
            "restore" if has(Flag::Restore) => restore = state,
            _ => return None,
        }
    }
    let mut xml = String::new();
    for (tag, flag) in spec {
        let state = match flag {
            Flag::Dialog => dialog,
            Flag::Pause => pause,
            Flag::Restore => restore,
        };
        xml.push_str(&format!("<{tag} state=\"{state}\"></{tag}>"));
    }
    Some(xml)
}

// ─── FMSaveAsXML form (inspect / get-script) ───────────────────────────────────
// A whole-database export carries `<UUID>`/`<OwnerID>`/`<Options>` around the
// step's options, and is pretty-printed, so the byte-exact gate never passes and
// the step showed as a raw blob. Strip that envelope and, when what's left is
// only options we model, render the same DSL (read-only context, like the
// Import Records summary).
fn strip_fmsavexml_envelope(xml: &str) -> Option<String> {
    if !xml.contains("<UUID>") {
        return None;
    }
    let mut s = xml.to_string();
    for tag in ["UUID", "OwnerID", "Options"] {
        if let Some(p) = s.find(&format!("<{}", tag)) {
            let close = format!("</{}>", tag);
            if let Some(e) = s[p..].find(&close) {
                s.replace_range(p..p + e + close.len(), "");
            } else if let Some(e) = s[p..].find("/>") {
                s.replace_range(p..p + e + 2, "");
            }
        }
    }
    // Self-closing → paired, and drop inter-element whitespace, so the flag
    // readers see the clipboard shape.
    let mut out = String::new();
    for piece in s.split('<').filter(|p| !p.trim().is_empty()) {
        let piece = piece.trim_end();
        if let Some(body) = piece.strip_suffix("/>") {
            let body = body.trim_end();
            let tag = body.split_whitespace().next().unwrap_or("");
            out.push_str(&format!("<{}></{}>", body, tag));
        } else {
            out.push('<');
            out.push_str(piece);
        }
    }
    Some(out)
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
    if let (Some(id), Some(name)) = (
        element_attr(xml, "Table", "id"),
        element_attr(xml, "Table", "name"),
    ) {
        lines.push(format!("Table: {} #{}", name, id));
    }
    if let (Some(id), Some(name)) = (
        element_attr(xml, "Layout", "id"),
        element_attr(xml, "Layout", "name"),
    ) {
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
        xml.push_str(&format!(
            "<MatchAllRecords state=\"{}\"></MatchAllRecords>",
            s
        ));
    }
    if let Some(s) = show_new {
        xml.push_str(&format!(
            "<ShowInNewWindow state=\"{}\"></ShowInNewWindow>",
            s
        ));
    }
    if let Some(s) = restore {
        xml.push_str(&format!("<Restore state=\"{}\"></Restore>", s));
    }
    if let Some(v) = layout_dest {
        xml.push_str(&format!(
            "<LayoutDestination value=\"{}\"></LayoutDestination>",
            v
        ));
    }
    if let Some(a) = styles {
        xml.push_str(&format!("<NewWndStyles {}></NewWndStyles>", a));
    }
    if let Some((id, name)) = table {
        xml.push_str(&format!("<Table id=\"{}\" name=\"{}\"></Table>", id, name));
    }
    if let Some((id, name)) = layout {
        xml.push_str(&format!(
            "<Layout id=\"{}\" name=\"{}\"></Layout>",
            id, name
        ));
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
        assert_eq!(
            from_dsl("Commit Records/Requests", &dsl).as_deref(),
            Some(xml)
        );
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
    fn import_fmsavexml_form_renders_readable_summary() {
        // The whole-DB export form (inspect), not the clipboard form.
        let xml = "<Options>33587232</Options>\
                   <FilePathList fileType=\"TABS\">âfilemac:/Desktop/SRCâ</FilePathList>\
                   <ImportField><Target>\
                   <TableOccurrenceReference id=\"99\" name=\"MT_PROV\"></TableOccurrenceReference>\
                   </Target><Field membercount=\"2\">\
                   <Map index=\"1\" id=\"5\"><FieldReference id=\"5\" name=\"idPadre\"></FieldReference></Map>\
                   <Map index=\"2\" id=\"6\"><FieldReference id=\"6\" name=\"codProv\"></FieldReference></Map>\
                   </Field></ImportField>";
        let dsl = to_dsl("Import Records", xml).expect("summary");
        assert!(dsl.contains("Source: filemac:/Desktop/SRC")); // â delimiters stripped
        assert!(dsl.contains("Target: MT_PROV #99"));
        assert!(dsl.contains("1 -> idPadre"));
        assert!(dsl.contains("2 -> codProv"));
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
        assert_eq!(
            from_dsl("Commit Records/Requests", &inline).as_deref(),
            Some(xml)
        );
    }
}
