// Readable DSL for the Import Records / Export Records steps.
//
// FileMaker serializes these steps with a rich inner XML payload (data source,
// profile, options, target table, and an ORDERED field map). fm-bridge used to
// carry that XML verbatim inside the `.fmscript` brackets — lossless but
// impossible to read, diff, or edit. This module converts that payload to an
// indented, line-per-concept DSL and back.
//
// Safety: the conversion is only used when it is **provably lossless** — the
// caller round-trips `xml -> dsl -> xml` and only emits the DSL when it
// reproduces the original XML byte-for-byte. Anything we don't fully model
// (Export's `<Output>`, exotic sources, unknown elements) fails that check and
// stays as raw XML, so we never corrupt a step.
//
// DSL shape:
//   Dialog: Off
//   Restore: True
//   VerifySSL: False
//   Source: File
//   Path: $file
//   Profile: FileName="$file" WorksheetName="" ... DataType="XLSX"
//   Options: CharacterSet="Windows" ... method="Add"
//   Target: Ta_d_ProyectosVentas #1068202
//   Mapping:
//     PryVen_PK #4 Import
//     PryVen_RK_OfeVerConJT_Sel Copia #1022 DoNotImport
//     PryVen_SelecBoo #69 DoNotImport opts=2

/// Convert the Import/Export Records inner XML to the indented DSL, but only if
/// the conversion round-trips exactly. Returns `None` to signal "keep the raw
/// XML" (unrecognized shape — never risk a lossy edit).
pub fn xml_to_dsl(xml: &str) -> Option<String> {
    let dsl = build_dsl(xml)?;
    // Lossless gate: only offer the DSL if we can rebuild the exact XML from it.
    if dsl_to_xml(&dsl).as_deref() == Some(xml) {
        Some(dsl)
    } else {
        None
    }
}

/// Element in the canonical FM order, with how we render/rebuild it.
fn build_dsl(xml: &str) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();

    // Flags (each is `<Tag state="True|False"></Tag>`).
    let no_interact = element_attr(xml, "NoInteract", "state")?;
    lines.push(format!(
        "Dialog: {}",
        if no_interact == "True" { "Off" } else { "On" }
    ));
    lines.push(format!(
        "Restore: {}",
        element_attr(xml, "Restore", "state")?
    ));
    lines.push(format!(
        "VerifySSL: {}",
        element_attr(xml, "VerifySSLCertificates", "state")?
    ));

    lines.push(format!(
        "Source: {}",
        element_attr(xml, "DataSourceType", "value")?
    ));
    lines.push(format!(
        "Path: {}",
        element_inner(xml, "UniversalPathList")?
    ));
    lines.push(format!("Profile: {}", element_attrs(xml, "Profile")?));
    // A "File" source (importing from another FM file) nests a <List> of source
    // <InputField> ids inside <Profile>. Capture it compactly so the step still
    // round-trips; without this the nested List was dropped and the step fell
    // back to raw XML. If the inner markup isn't exactly that List shape, bail
    // (return None) so we keep the verbatim XML rather than risk a lossy edit.
    let profile_inner = element_inner(xml, "Profile")?;
    if !profile_inner.trim().is_empty() {
        let list_attrs = element_attrs(profile_inner, "List")?;
        let list_inner = element_inner(profile_inner, "List")?;
        let ids: Vec<&str> = iter_tags(list_inner, "InputField")
            .map(|t| tag_attr(t, "id"))
            .collect::<Option<Vec<_>>>()?;
        // Reject if there's markup we didn't account for (gate will also catch it).
        lines.push(format!("SourceList: {} fields={}", list_attrs, ids.join(",")));
    }
    lines.push(format!("Options: {}", element_attrs(xml, "ImportOptions")?));

    let table_id = element_attr(xml, "Table", "id")?;
    let table_name = element_attr(xml, "Table", "name")?;
    lines.push(format!("Target: {} #{}", table_name, table_id));

    // Ordered field map.
    lines.push("Mapping:".to_string());
    let fields_inner = element_inner(xml, "TargetFields")?;
    for field_tag in iter_tags(&fields_inner, "Field") {
        let name = tag_attr(field_tag, "name")?;
        let id = tag_attr(field_tag, "id")?;
        let map = tag_attr(field_tag, "map")?;
        let opts = tag_attr(field_tag, "FieldOptions").unwrap_or("0");
        let mut line = format!("  {} #{} {}", name, id, map);
        if opts != "0" {
            line.push_str(&format!(" opts={}", opts));
        }
        lines.push(line);
    }

    Some(lines.join("\n"))
}

/// Rebuild the exact FM inner XML from the DSL. Returns `None` if the DSL is
/// malformed (so the caller can reject it).
pub fn dsl_to_xml(dsl: &str) -> Option<String> {
    let mut dialog = None;
    let mut restore = None;
    let mut verify = None;
    let mut source = None;
    let mut path = None;
    let mut profile = None;
    let mut source_list = None;
    let mut options = None;
    let mut target_name = None;
    let mut target_id = None;
    let mut fields: Vec<(String, String, String, String)> = Vec::new();
    let mut in_mapping = false;

    for raw_line in dsl.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if in_mapping {
            // `<name> #<id> <map> [opts=N]` — anchor on " #" so names may have spaces.
            let hash = line.rfind(" #")?;
            let name = line[..hash].to_string();
            let rest: Vec<&str> = line[hash + 2..].split_whitespace().collect();
            let id = rest.first()?.to_string();
            let map = rest.get(1)?.to_string();
            let opts = rest
                .iter()
                .find_map(|t| t.strip_prefix("opts="))
                .unwrap_or("0")
                .to_string();
            fields.push((opts, map, id, name));
            continue;
        }
        let (key, value) = line.split_once(':')?;
        let value = value.trim();
        match key.trim() {
            "Dialog" => dialog = Some(if value == "Off" { "True" } else { "False" }),
            "Restore" => restore = Some(value.to_string()),
            "VerifySSL" => verify = Some(value.to_string()),
            "Source" => source = Some(value.to_string()),
            "Path" => path = Some(value.to_string()),
            "Profile" => profile = Some(value.to_string()),
            "SourceList" => source_list = Some(value.to_string()),
            "Options" => options = Some(value.to_string()),
            "Target" => {
                let hash = value.rfind(" #")?;
                target_name = Some(value[..hash].to_string());
                target_id = Some(value[hash + 2..].to_string());
            }
            "Mapping" => in_mapping = true,
            _ => return None,
        }
    }

    let mut xml = String::new();
    xml.push_str(&format!("<NoInteract state=\"{}\"></NoInteract>", dialog?));
    xml.push_str(&format!("<Restore state=\"{}\"></Restore>", restore?));
    xml.push_str(&format!(
        "<VerifySSLCertificates state=\"{}\"></VerifySSLCertificates>",
        verify?
    ));
    xml.push_str(&format!(
        "<DataSourceType value=\"{}\"></DataSourceType>",
        source?
    ));
    match &source_list {
        // `<attrs> fields=1,2,3` → <Profile attrs><List attrs><InputField…/></List></Profile>
        Some(sl) => {
            let (list_attrs, fields) = sl.rsplit_once(" fields=")?;
            let inputs: String = fields
                .split(',')
                .filter(|s| !s.is_empty())
                .map(|id| format!("<InputField id=\"{}\"></InputField>", id))
                .collect();
            xml.push_str(&format!(
                "<Profile {}><List {}>{}</List></Profile>",
                profile?, list_attrs, inputs
            ));
        }
        None => xml.push_str(&format!("<Profile {}></Profile>", profile?)),
    }
    xml.push_str(&format!("<UniversalPathList>{}</UniversalPathList>", path?));
    xml.push_str(&format!("<ImportOptions {}></ImportOptions>", options?));
    xml.push_str(&format!(
        "<Table id=\"{}\" name=\"{}\"></Table>",
        target_id?, target_name?
    ));
    xml.push_str("<TargetFields>");
    for (opts, map, id, name) in &fields {
        xml.push_str(&format!(
            "<Field FieldOptions=\"{}\" map=\"{}\" id=\"{}\" name=\"{}\"></Field>",
            opts, map, id, name
        ));
    }
    xml.push_str("</TargetFields>");
    Some(xml)
}

// ─── tiny XML helpers (string-level; the payload is flat and well-formed) ─────

/// Attribute string of the first `<Tag ...>`: everything between `<Tag ` and `>`.
pub(crate) fn element_attrs<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{} ", tag);
    let start = xml.find(&open)? + open.len();
    let end = xml[start..].find('>')? + start;
    // Strip a trailing `/` for self-closing forms (FM uses paired, but be safe).
    Some(xml[start..end].trim_end_matches('/').trim_end())
}

/// A single attribute value from the first `<Tag ...>`.
pub(crate) fn element_attr<'a>(xml: &'a str, tag: &str, attr: &str) -> Option<&'a str> {
    let open = format!("<{} ", tag);
    let start = xml.find(&open)?;
    let end = xml[start..].find('>')? + start + 1;
    tag_attr(&xml[start..end], attr)
}

/// Inner text/markup between `<Tag>`/`<Tag ...>` and `</Tag>`.
pub(crate) fn element_inner<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{}", tag);
    let p = xml.find(&open)?;
    let inner_start = xml[p..].find('>')? + p + 1;
    let close = format!("</{}>", tag);
    let inner_end = xml[inner_start..].find(&close)? + inner_start;
    Some(&xml[inner_start..inner_end])
}

/// Read an attribute value from a single tag like `<Field a="1" b="2">`.
pub(crate) fn tag_attr<'a>(tag: &'a str, attr: &str) -> Option<&'a str> {
    let needle = format!(" {}=\"", attr);
    let start = tag.find(&needle)? + needle.len();
    let end = tag[start..].find('"')? + start;
    Some(&tag[start..end])
}

/// Iterate the opening tags `<Name ...>` within `xml` (one per element).
fn iter_tags<'a>(xml: &'a str, name: &'a str) -> impl Iterator<Item = &'a str> {
    let open = format!("<{} ", name);
    let mut pos = 0;
    std::iter::from_fn(move || {
        let rel = xml[pos..].find(&open)?;
        let start = pos + rel;
        let end = xml[start..].find('>')? + start + 1;
        pos = end;
        Some(&xml[start..end])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = concat!(
        "<NoInteract state=\"True\"></NoInteract><Restore state=\"True\"></Restore>",
        "<VerifySSLCertificates state=\"False\"></VerifySSLCertificates>",
        "<DataSourceType value=\"File\"></DataSourceType>",
        "<Profile FileName=\"$file\" WorksheetName=\"\" SelectedSheet=\"0\" FieldDelimiter=\"&#09;\" IsPredefined=\"-1\" FieldNameRow=\"0\" DataType=\"XLSX\"></Profile>",
        "<UniversalPathList>$file</UniversalPathList>",
        "<ImportOptions CharacterSet=\"Windows\" PreserveContainer=\"False\" MatchFieldNames=\"True\" AutoEnter=\"False\" SplitRepetitions=\"False\" method=\"Add\"></ImportOptions>",
        "<Table id=\"1068202\" name=\"Ta_d_ProyectosVentas\"></Table>",
        "<TargetFields>",
        "<Field FieldOptions=\"0\" map=\"Import\" id=\"4\" name=\"PryVen_PK\"></Field>",
        "<Field FieldOptions=\"2\" map=\"DoNotImport\" id=\"69\" name=\"PryVen_SelecBoo\"></Field>",
        "<Field FieldOptions=\"0\" map=\"DoNotImport\" id=\"1022\" name=\"PryVen_RK_OfeVerConJT_Sel Copia\"></Field>",
        "</TargetFields>",
    );

    #[test]
    fn dsl_round_trips_exactly() {
        let dsl = xml_to_dsl(XML).expect("should produce DSL");
        // Human-readable: mapping lines present, field with a space preserved.
        assert!(dsl.contains("Mapping:"));
        assert!(dsl.contains("PryVen_PK #4 Import"));
        assert!(dsl.contains("PryVen_SelecBoo #69 DoNotImport opts=2"));
        assert!(dsl.contains("PryVen_RK_OfeVerConJT_Sel Copia #1022 DoNotImport"));
        // Byte-exact rebuild.
        assert_eq!(dsl_to_xml(&dsl).as_deref(), Some(XML));
    }

    #[test]
    fn unrecognized_payload_is_rejected() {
        // Export-style payload without the modeled elements → no DSL (stay opaque).
        let xml = "<Output something=\"x\"></Output>";
        assert_eq!(xml_to_dsl(xml), None);
    }
}
