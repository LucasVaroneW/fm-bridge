//! Census of a FMSaveAsXML export: every element path, every attribute, counted.
//!
//! This is the measuring tape behind principle 5 of `docs/VISION.md` ("nunca
//! perder nada en silencio"). Before we can promise that nothing is dropped
//! silently, we need to know what is actually *in* an export — not what we
//! remember modelling. `census` walks the file and reports the raw inventory;
//! comparing that inventory against what the parser consumes is what turns the
//! promise into a number.
//!
//! It is deliberately dumb: no knowledge of FileMaker semantics, no parsing of
//! anything. Just paths, attributes and text. That is the point — a smarter
//! walker would share the parser's blind spots, and blind spots are what we are
//! hunting.
//!
//! Exports of 100+ MB are normal, and they arrive **UTF-16 LE**, so decoding
//! goes through `fmsavexml::read_export_to_string` — the same door the parser
//! uses. Reading the raw bytes as UTF-8 finds no markup at all.

use std::collections::HashMap;

use quick_xml::Reader;
use quick_xml::events::Event;
use serde::Serialize;

/// One element path (e.g. `FMPXMLRESULT/FieldCatalog/Field`) and everything
/// seen at it.
#[derive(Debug, Serialize, Clone)]
pub struct PathStat {
    /// Slash-joined local names, from the document root down.
    pub path: String,
    /// How many times an element at this path opened.
    pub count: u64,
    /// How many of those carried non-whitespace text directly.
    #[serde(skip_serializing_if = "is_zero")]
    pub with_text: u64,
    /// Attributes seen at this path, with their own counts, most frequent first.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attrs: Vec<AttrStat>,
}

#[derive(Debug, Serialize, Clone)]
pub struct AttrStat {
    pub name: String,
    pub count: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

#[derive(Debug, Serialize)]
pub struct Census {
    pub file_name: String,
    /// Total elements opened in the document.
    pub total_elements: u64,
    /// Total attributes seen (counting repeats across elements).
    pub total_attributes: u64,
    /// Distinct element paths.
    pub distinct_paths: usize,
    /// Deepest nesting reached. Layouts nest hard; this is the number that
    /// decides whether a recursive model is workable.
    pub max_depth: usize,
    /// Every path, sorted by descending count.
    pub paths: Vec<PathStat>,
}

#[derive(Default)]
struct Entry {
    count: u64,
    with_text: u64,
    attrs: HashMap<String, u64>,
}

/// Walk `path` and count everything.
pub fn census(path: &str) -> Result<Census, String> {
    let xml = crate::fmsavexml::read_export_to_string(path)?;
    let mut reader = Reader::from_str(&xml);
    // Empty elements are counted like any other element: `<Field/>` and
    // `<Field></Field>` are the same thing to FileMaker, and treating them
    // differently would split every path in two.
    reader.config_mut().expand_empty_elements = true;
    reader.config_mut().trim_text(true);

    let mut stats: HashMap<String, Entry> = HashMap::new();
    let mut stack: Vec<String> = Vec::new();
    let mut cur_path = String::new();
    let mut total_elements: u64 = 0;
    let mut total_attributes: u64 = 0;
    let mut max_depth: usize = 0;
    // Whether each open element has already been credited with text. Parallel
    // to `stack`. An element can emit several Text events (before and after
    // each child); it must still count once — hence per-element state rather
    // than a single flag, which would re-credit the parent after every child.
    let mut credited: Vec<bool> = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                stack.push(name);
                cur_path = stack.join("/");
                max_depth = max_depth.max(stack.len());
                total_elements += 1;
                credited.push(false);

                let entry = stats.entry(cur_path.clone()).or_default();
                entry.count += 1;
                for attr in e.attributes().flatten() {
                    let key = String::from_utf8_lossy(attr.key.local_name().as_ref()).into_owned();
                    *entry.attrs.entry(key).or_insert(0) += 1;
                    total_attributes += 1;
                }
            }
            Ok(Event::End(_)) => {
                stack.pop();
                credited.pop();
                cur_path = stack.join("/");
            }
            Ok(Event::Text(t)) => {
                if cur_path.is_empty() || credited.last() == Some(&true) {
                    continue;
                }
                let is_blank = t.iter().all(|b| b.is_ascii_whitespace());
                if !is_blank {
                    if let Some(entry) = stats.get_mut(&cur_path) {
                        entry.with_text += 1;
                    }
                    if let Some(last) = credited.last_mut() {
                        *last = true;
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(format!(
                    "XML error at byte {}: {}",
                    reader.buffer_position(),
                    e
                ));
            }
            _ => {}
        }
    }

    let mut paths: Vec<PathStat> = stats
        .into_iter()
        .map(|(path, entry)| {
            let mut attrs: Vec<AttrStat> = entry
                .attrs
                .into_iter()
                .map(|(name, count)| AttrStat { name, count })
                .collect();
            attrs.sort_by(|a, b| b.count.cmp(&a.count).then(a.name.cmp(&b.name)));
            PathStat {
                path,
                count: entry.count,
                with_text: entry.with_text,
                attrs,
            }
        })
        .collect();
    paths.sort_by(|a, b| b.count.cmp(&a.count).then(a.path.cmp(&b.path)));

    let file_name = std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string());

    Ok(Census {
        distinct_paths: paths.len(),
        file_name,
        total_elements,
        total_attributes,
        max_depth,
        paths,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp(name: &str, xml: &str) -> String {
        let mut p = std::env::temp_dir();
        p.push(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(xml.as_bytes()).unwrap();
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn counts_paths_attributes_and_depth() {
        let path = write_temp(
            "fmb_census_basic.xml",
            r#"<FMSaveAsXML>
                 <BaseTableCatalog>
                   <BaseTable id="1" name="Contacts">
                     <Field id="1" name="nombre" dataType="Text"/>
                     <Field id="2" name="edad" dataType="Number"/>
                   </BaseTable>
                 </BaseTableCatalog>
               </FMSaveAsXML>"#,
        );
        let c = census(&path).unwrap();

        let field = c
            .paths
            .iter()
            .find(|p| p.path == "FMSaveAsXML/BaseTableCatalog/BaseTable/Field")
            .expect("Field path present");
        assert_eq!(field.count, 2);
        // Each of the two fields carries all three attributes.
        for want in ["id", "name", "dataType"] {
            let a = field.attrs.iter().find(|a| a.name == want).unwrap();
            assert_eq!(a.count, 2, "attr {}", want);
        }
        assert_eq!(c.total_elements, 5);
        assert_eq!(c.max_depth, 4);
        assert_eq!(c.distinct_paths, 4);
    }

    #[test]
    fn self_closing_and_paired_elements_share_a_path() {
        let path = write_temp(
            "fmb_census_selfclosing.xml",
            r#"<Root><Field a="1"/><Field a="2"></Field></Root>"#,
        );
        let c = census(&path).unwrap();
        let field = c.paths.iter().find(|p| p.path == "Root/Field").unwrap();
        assert_eq!(field.count, 2, "both spellings collapse to one path");
    }

    #[test]
    fn text_is_credited_once_per_element_and_blanks_ignored() {
        let path = write_temp(
            "fmb_census_text.xml",
            r#"<Root>
                 <Calc>Let ( x = 1 ; <Ref>CLIENTES::id</Ref> + x )</Calc>
                 <Empty>   </Empty>
               </Root>"#,
        );
        let c = census(&path).unwrap();

        // <Calc> emits text before AND after <Ref>, but counts as one element
        // with text.
        let calc = c.paths.iter().find(|p| p.path == "Root/Calc").unwrap();
        assert_eq!(calc.with_text, 1);

        let empty = c.paths.iter().find(|p| p.path == "Root/Empty").unwrap();
        assert_eq!(empty.with_text, 0, "whitespace is not content");
    }

    #[test]
    fn same_element_under_different_parents_stays_separate() {
        // This is the whole reason paths are counted instead of tag names:
        // <Calculation> means something different under <AutoEnter> than under
        // <Validation>, and a census that merged them would hide a gap.
        let path = write_temp(
            "fmb_census_paths.xml",
            r#"<Root>
                 <Field><AutoEnter><Calculation>a</Calculation></AutoEnter></Field>
                 <Field><Validation><Calculation>b</Calculation></Validation></Field>
               </Root>"#,
        );
        let c = census(&path).unwrap();
        assert!(
            c.paths
                .iter()
                .any(|p| p.path == "Root/Field/AutoEnter/Calculation")
        );
        assert!(
            c.paths
                .iter()
                .any(|p| p.path == "Root/Field/Validation/Calculation")
        );
    }

    #[test]
    fn a_truncated_document_is_an_error_not_a_partial_count() {
        // Silence is the bug we are hunting: a half-read export must not come
        // back looking like a complete census.
        let path = write_temp("fmb_census_broken.xml", r#"<Root><Field></Root>"#);
        assert!(census(&path).is_err());
    }
}
