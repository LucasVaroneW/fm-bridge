//! What kind of FileMaker object is this snippet?
//!
//! Everything FileMaker puts on the clipboard is an `<fmxmlsnippet>`, but the
//! contents are wildly different: script steps, whole scripts, base tables,
//! loose fields, custom functions, value lists, layout objects. The codec in
//! `xmss.rs` only understands script steps, and until now anything else came
//! back as "No script steps found in XML" — technically true and completely
//! useless, since the clipboard *did* hold FileMaker data.
//!
//! This module answers the prior question: **what have we got?** It is a
//! sniffer, not a parser — it looks at which elements appear directly under the
//! snippet root and counts them. Deliberately shallow, so it can label objects
//! the engine cannot yet decode, which is exactly the case worth naming.

use quick_xml::Reader;
use quick_xml::events::Event;
use serde::Serialize;

/// The kind of object a snippet holds. `Unknown` carries the element name we
/// actually saw, because a snippet we cannot classify is a thing to report, not
/// to swallow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SnippetKind {
    /// Loose script steps — the only kind the codec decodes today.
    ScriptSteps {
        steps: usize,
    },
    /// One or more whole scripts.
    Scripts {
        names: Vec<String>,
    },
    /// One or more base tables, each with its fields.
    BaseTables {
        names: Vec<String>,
        fields: usize,
    },
    /// Loose fields, copied from the Fields tab of a table.
    Fields {
        names: Vec<String>,
    },
    CustomFunctions {
        names: Vec<String>,
    },
    ValueLists {
        names: Vec<String>,
    },
    LayoutObjects {
        objects: usize,
    },
    /// Something we do not recognise. `element` is the first child element seen.
    Unknown {
        element: String,
    },
    /// Not an `<fmxmlsnippet>` at all.
    NotASnippet,
}

impl SnippetKind {
    /// A short human label, for messages the user reads.
    pub fn label(&self) -> String {
        match self {
            SnippetKind::ScriptSteps { steps } => format!("{} paso(s) de script", steps),
            SnippetKind::Scripts { names } => {
                format!("{} script(s): {}", names.len(), names.join(", "))
            }
            SnippetKind::BaseTables { names, fields } => format!(
                "{} tabla(s) con {} campo(s): {}",
                names.len(),
                fields,
                names.join(", ")
            ),
            SnippetKind::Fields { names } => {
                format!("{} campo(s): {}", names.len(), names.join(", "))
            }
            SnippetKind::CustomFunctions { names } => {
                format!("{} custom function(s): {}", names.len(), names.join(", "))
            }
            SnippetKind::ValueLists { names } => {
                format!("{} lista(s) de valores: {}", names.len(), names.join(", "))
            }
            SnippetKind::LayoutObjects { objects } => format!("{} objeto(s) de layout", objects),
            SnippetKind::Unknown { element } => format!("objeto no reconocido (<{}>)", element),
            SnippetKind::NotASnippet => "no es un fmxmlsnippet".to_string(),
        }
    }

    /// Whether `xmss.rs` can decode this to `.fmscript` text today.
    pub fn is_decodable_script(&self) -> bool {
        matches!(self, SnippetKind::ScriptSteps { .. })
    }
}

/// Classify a snippet by its **direct children**, with a real XML walk.
///
/// A shallow text scan is not good enough here, and the reason is worth
/// keeping: a copied table contains `<Field>` elements at two different
/// depths — the table's own fields, and the lookup source inside
/// `<AutoEnter><Lookup><Field/>`. Counting every `<Field>` in the document
/// reported 635 fields for a table that has 589. A wrong number nobody can
/// see is exactly the failure mode principle 5 exists to prevent, so the
/// counting is done by parent, not by pattern.
pub fn detect(xml: &str) -> SnippetKind {
    if !xml.contains("<fmxmlsnippet") {
        return SnippetKind::NotASnippet;
    }

    let mut reader = Reader::from_str(xml);
    reader.config_mut().expand_empty_elements = true;

    // Element names from the snippet root down; `stack[0]` is `fmxmlsnippet`.
    let mut stack: Vec<String> = Vec::new();
    let mut tables: Vec<String> = Vec::new();
    let mut scripts: Vec<String> = Vec::new();
    let mut functions: Vec<String> = Vec::new();
    let mut value_lists: Vec<String> = Vec::new();
    let mut loose_fields: Vec<String> = Vec::new();
    let mut table_fields = 0usize;
    let mut steps = 0usize;
    let mut layout_objects = 0usize;
    let mut first_child: Option<String> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                let parent = stack.last().map(|s| s.as_str()).unwrap_or("");
                let depth = stack.len();

                if depth == 1 && first_child.is_none() {
                    first_child = Some(name.clone());
                }

                let named = || {
                    e.attributes()
                        .flatten()
                        .find(|a| a.key.local_name().as_ref() == b"name")
                        .map(|a| {
                            String::from_utf8_lossy(a.value.as_ref())
                                .into_owned()
                                .replace("&amp;", "&")
                        })
                        .unwrap_or_default()
                };

                // Only direct children of the snippet root count as objects.
                if depth == 1 {
                    match name.as_str() {
                        "BaseTable" => tables.push(named()),
                        "Script" => scripts.push(named()),
                        "CustomFunction" => functions.push(named()),
                        "ValueList" => value_lists.push(named()),
                        "Field" => loose_fields.push(named()),
                        "Step" => steps += 1,
                        "Object" | "Layout" => layout_objects += 1,
                        _ => {}
                    }
                } else if name == "Field" && parent == "BaseTable" {
                    // A table's own field — not the `<Field>` that names a
                    // lookup's source inside `<AutoEnter><Lookup>`.
                    table_fields += 1;
                } else if name == "Step" && parent == "Script" {
                    steps += 1;
                }

                stack.push(name);
            }
            Ok(Event::End(_)) => {
                stack.pop();
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }

    if !tables.is_empty() {
        return SnippetKind::BaseTables {
            names: tables,
            fields: table_fields,
        };
    }
    if !scripts.is_empty() {
        return SnippetKind::Scripts { names: scripts };
    }
    if !functions.is_empty() {
        return SnippetKind::CustomFunctions { names: functions };
    }
    if !value_lists.is_empty() {
        return SnippetKind::ValueLists { names: value_lists };
    }
    if steps > 0 {
        return SnippetKind::ScriptSteps { steps };
    }
    if !loose_fields.is_empty() {
        return SnippetKind::Fields {
            names: loose_fields,
        };
    }
    if layout_objects > 0 {
        return SnippetKind::LayoutObjects {
            objects: layout_objects,
        };
    }

    SnippetKind::Unknown {
        element: first_child.unwrap_or_else(|| "(vacío)".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_copied_table_is_recognised_with_its_fields() {
        // Shape taken from a real Manage Database copy.
        let xml = r#"<fmxmlsnippet type="FMObjectList"><BaseTable comment="" name="Pedidos"><Field id="1" dataType="Number" fieldType="Normal" name="id"><AutoEnter/></Field></BaseTable></fmxmlsnippet>"#;
        assert_eq!(
            detect(xml),
            SnippetKind::BaseTables {
                names: vec!["Pedidos".to_string()],
                fields: 1
            }
        );
    }

    #[test]
    fn several_tables_copied_together_all_show_up() {
        // The case that makes multi-table a requirement and not a convenience:
        // FileMaker really does put N tables in one snippet.
        let xml = r#"<fmxmlsnippet type="FMObjectList">
            <BaseTable comment="" name="Pedidos"><Field name="a"/></BaseTable>
            <BaseTable comment="" name="PedidosItems"><Field name="b"/><Field name="c"/></BaseTable>
            <BaseTable comment="" name="PedidosItemsFechasJT"><Field name="d"/></BaseTable>
        </fmxmlsnippet>"#;
        match detect(xml) {
            SnippetKind::BaseTables { names, fields } => {
                assert_eq!(names.len(), 3);
                assert_eq!(names[0], "Pedidos");
                assert_eq!(fields, 4);
            }
            other => panic!("expected BaseTables, got {:?}", other),
        }
    }

    #[test]
    fn a_lookup_source_field_is_not_counted_as_a_table_field() {
        // Regression: the first version counted every <Field> in the document,
        // so the <Field> naming a lookup's source inflated the count. A real
        // 3-table copy reported 635 fields instead of 589 — wrong, and
        // invisible. Shape taken verbatim from a Manage Database copy.
        let xml = r#"<fmxmlsnippet type="FMObjectList"><BaseTable name="Pedidos">
            <Field id="1" name="cliente_nombre" dataType="Text" fieldType="Normal">
              <AutoEnter lookup="True">
                <Lookup>
                  <Table id="9" name="Ta_d_Clientes"/>
                  <Field id="4" name="nombre" table="Ta_d_Clientes"/>
                  <CopyEmptyContent value="False"/>
                </Lookup>
              </AutoEnter>
            </Field>
        </BaseTable></fmxmlsnippet>"#;
        match detect(xml) {
            SnippetKind::BaseTables { fields, .. } => {
                assert_eq!(
                    fields, 1,
                    "the lookup source is not one of the table's fields"
                )
            }
            other => panic!("expected BaseTables, got {:?}", other),
        }
    }

    #[test]
    fn a_table_is_never_mistaken_for_loose_fields() {
        // A table snippet contains <Field> too. Classifying by "first tag that
        // matches" would silently downgrade a table to a pile of fields.
        let xml = r#"<fmxmlsnippet type="FMObjectList"><BaseTable name="T"><Field name="a"/></BaseTable></fmxmlsnippet>"#;
        assert!(matches!(detect(xml), SnippetKind::BaseTables { .. }));
    }

    #[test]
    fn loose_fields_without_a_table_are_fields() {
        let xml = r#"<fmxmlsnippet type="FMObjectList"><Field id="1" name="nombre"/><Field id="2" name="edad"/></fmxmlsnippet>"#;
        match detect(xml) {
            SnippetKind::Fields { names } => assert_eq!(names, vec!["nombre", "edad"]),
            other => panic!("expected Fields, got {:?}", other),
        }
    }

    #[test]
    fn script_steps_still_classify_as_before() {
        let xml = r#"<fmxmlsnippet type="FMObjectList"><Step id="89" name="Set Variable"></Step><Step id="68" name="Go to Layout"></Step></fmxmlsnippet>"#;
        assert_eq!(detect(xml), SnippetKind::ScriptSteps { steps: 2 });
        assert!(detect(xml).is_decodable_script());
    }

    #[test]
    fn a_whole_script_outranks_the_steps_inside_it() {
        let xml = r#"<fmxmlsnippet type="FMObjectList"><Script id="7" name="Crear pedido"><Step id="89" name="Set Variable"/></Script></fmxmlsnippet>"#;
        match detect(xml) {
            SnippetKind::Scripts { names } => assert_eq!(names, vec!["Crear pedido"]),
            other => panic!("expected Scripts, got {:?}", other),
        }
        assert!(!detect(xml).is_decodable_script());
    }

    #[test]
    fn an_unrecognised_object_names_what_it_saw() {
        // The whole point of principle 5: something we cannot classify must be
        // reported by name, never reported as nothing.
        let xml = r#"<fmxmlsnippet type="FMObjectList"><Theme name="Enlightened"/></fmxmlsnippet>"#;
        assert_eq!(
            detect(xml),
            SnippetKind::Unknown {
                element: "Theme".to_string()
            }
        );
    }

    #[test]
    fn non_snippet_input_is_not_guessed_at() {
        assert_eq!(detect("<html><body/></html>"), SnippetKind::NotASnippet);
    }

    #[test]
    fn escaped_names_are_shown_readably() {
        let xml = r#"<fmxmlsnippet type="FMObjectList"><BaseTable name="Pedidos &amp; Albaranes"><Field name="a"/></BaseTable></fmxmlsnippet>"#;
        match detect(xml) {
            SnippetKind::BaseTables { names, .. } => assert_eq!(names[0], "Pedidos & Albaranes"),
            other => panic!("expected BaseTables, got {:?}", other),
        }
    }
}
