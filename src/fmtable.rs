//! `.fmtable` — FileMaker tables as plain text, both ways.
//!
//! The clipboard hands us `XMTB`: a table with every field, its type, its
//! auto-enter, its validation and its storage. Useful, and unreadable — 280 KB
//! of XML for two tables. This module is the codec that makes it a text file a
//! human can edit in VS Code and an AI can read whole without burning its
//! context window, and then turn back into something FileMaker will paste.
//!
//! ```text
//! table PedidosItems
//!
//! field PedIte_Ref
//!   type       number
//!   comment    Ref. Núm.
//!   auto       serial next=1068893 increment=1 generate=OnCreation
//!   index      all
//!
//! field PedIte_cRef
//!   type       text
//!   calc       stored
//!   formula
//!     | "PedIte - " & PedIte_Ref
//! ```
//!
//! **On fidelity, honestly.** A byte-exact round trip is not achievable here and
//! pretending otherwise would be the silent-loss failure this project exists to
//! avoid. FileMaker keeps *dead* payloads in its XML: a field with
//! `constant="False"` still carries a `<ConstantData>` element, and one with
//! `calculation="False"` can still carry a whole `<Calculation>` from an option
//! that was switched off years ago. `.fmtable` records what FileMaker actually
//! *applies*, and everything dropped is counted and named in the [`Ledger`] —
//! never discarded quietly. See `docs/SCHEMA.md`.

use serde::Serialize;

use crate::text_format::ParseError;

// ─── Model ───

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Table {
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub comment: String,
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Field {
    pub name: String,
    /// FileMaker `dataType`: Text, Number, Date, Time, TimeStamp, Container.
    pub data_type: String,
    /// FileMaker `fieldType`: Normal, Calculated, Summary.
    pub field_type: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub comment: String,
    /// The field's own calculation (`fieldType="Calculated"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub formula: Option<String>,
    /// `storeCalculationResults` for a calc field.
    pub stored: bool,
    pub global: bool,
    /// `maxRepetition`, only meaningful above 1.
    pub repetitions: u32,
    /// "all" | "minimal" | "none".
    pub index: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub index_language: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto: Option<AutoEnter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validation: Option<Validation>,
    /// The table this field's calculations are evaluated from (`Calculation
    /// table=`). Carried so encoding can put it back.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub calc_context: String,
}

/// Auto-entry. FileMaker allows exactly one of these to be active at a time,
/// which is why this is an enum and not a bag of flags — the XML's four
/// independent booleans can express states FileMaker itself cannot.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "auto", rename_all = "snake_case")]
pub enum AutoEnter {
    Serial {
        next: String,
        increment: String,
        /// "OnCreation" | "OnCommit".
        generate: String,
    },
    Constant {
        value: String,
    },
    Calculation {
        formula: String,
        /// `alwaysEvaluate` — re-evaluate even when the field has a value.
        always: bool,
    },
    Lookup {
        /// Table occurrence the lookup starts from.
        from_table: String,
        /// Field copied from the related record.
        from_field: String,
        copy_empty: bool,
        /// "none" | "next" | "prev".
        no_match: String,
    },
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct Validation {
    pub not_empty: bool,
    pub unique: bool,
    pub existing: bool,
    /// `StrictValidation` — "allow only values in the member list".
    pub strict: bool,
    /// "OnlyDuringDataEntry" | "Always".
    #[serde(skip_serializing_if = "String::is_empty")]
    pub when: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub message: String,
}

impl Validation {
    fn is_empty(&self) -> bool {
        !self.not_empty
            && !self.unique
            && !self.existing
            && !self.strict
            && self.when.is_empty()
            && self.message.is_empty()
    }
}

/// What a decode kept and what it left behind. Principle 5 made countable: an
/// empty `dropped` means nothing was dropped, never "we did not look".
#[derive(Debug, Clone, Default, Serialize)]
pub struct Ledger {
    pub tables: usize,
    pub fields: usize,
    /// One line per thing deliberately not carried into `.fmtable`, with the
    /// field it belonged to and why.
    pub dropped: Vec<String>,
}

// ─── Decode: XMTB XML → model ───

/// Parse a `<fmxmlsnippet>` holding one or more `<BaseTable>`.
pub fn decode_xmtb(xml: &str) -> Result<(Vec<Table>, Ledger), String> {
    use quick_xml::Reader;
    use quick_xml::events::Event;

    let mut reader = Reader::from_str(xml);
    reader.config_mut().expand_empty_elements = true;

    let mut tables: Vec<Table> = Vec::new();
    let mut ledger = Ledger::default();
    let mut stack: Vec<String> = Vec::new();
    let mut field: Option<Field> = None;
    // Auto-enter flags, read off `<AutoEnter>` and resolved once the element
    // closes — the payload elements arrive after the attributes.
    let mut ae_flags = (false, false, false, false); // constant, calc, lookup, serial-present
    let mut ae_always = false;
    let mut ae_constant = String::new();
    let mut ae_calc = String::new();
    let mut ae_serial: Option<(String, String, String)> = None;
    let mut ae_lookup: (String, String, bool, String) =
        (String::new(), String::new(), false, String::new());
    let mut validation = Validation::default();
    let mut text_buf = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).into_owned();
                let parent = stack.last().cloned().unwrap_or_default();
                let attr = |k: &str| -> String {
                    e.attributes()
                        .flatten()
                        .find(|a| a.key.local_name().as_ref() == k.as_bytes())
                        .map(|a| decode_entities(&String::from_utf8_lossy(a.value.as_ref())))
                        .unwrap_or_default()
                };
                text_buf.clear();

                match (parent.as_str(), name.as_str()) {
                    ("fmxmlsnippet", "BaseTable") => tables.push(Table {
                        name: attr("name"),
                        comment: attr("comment"),
                        fields: Vec::new(),
                    }),
                    ("BaseTable", "Field") => {
                        field = Some(Field {
                            name: attr("name"),
                            data_type: attr("dataType"),
                            field_type: attr("fieldType"),
                            index: "none".to_string(),
                            repetitions: 1,
                            ..Default::default()
                        });
                        ae_flags = (false, false, false, false);
                        ae_always = false;
                        ae_constant.clear();
                        ae_calc.clear();
                        ae_serial = None;
                        ae_lookup = (String::new(), String::new(), false, String::new());
                        validation = Validation::default();
                    }
                    ("Field", "AutoEnter") => {
                        ae_flags = (
                            attr("constant") == "True",
                            attr("calculation") == "True",
                            attr("lookup") == "True",
                            false,
                        );
                        ae_always = attr("alwaysEvaluate") == "True";
                    }
                    ("AutoEnter", "Serial") => {
                        ae_flags.3 = true;
                        ae_serial = Some((attr("nextValue"), attr("increment"), attr("generate")));
                    }
                    ("Lookup", "Table") => ae_lookup.0 = attr("name"),
                    ("Lookup", "Field") => ae_lookup.1 = attr("name"),
                    ("Lookup", "CopyEmptyContent") => ae_lookup.2 = attr("value") == "True",
                    ("Lookup", "NoMatchCopyOption") => ae_lookup.3 = attr("value"),
                    ("Field", "Validation") => {
                        // "OnlyDuringDataEntry" is FileMaker's default; storing
                        // it would make the text format carry a value that says
                        // nothing, and lose the round trip when it is omitted.
                        validation.when = if attr("type") == "Always" {
                            "Always".to_string()
                        } else {
                            String::new()
                        };
                    }
                    ("Validation", "NotEmpty") => validation.not_empty = attr("value") == "True",
                    ("Validation", "Unique") => validation.unique = attr("value") == "True",
                    ("Validation", "Existing") => validation.existing = attr("value") == "True",
                    ("Validation", "StrictValidation") => {
                        validation.strict = attr("value") == "True"
                    }
                    ("Field", "Storage") => {
                        if let Some(f) = field.as_mut() {
                            f.index = match attr("index").as_str() {
                                "All" => "all",
                                "Minimal" => "minimal",
                                _ => "none",
                            }
                            .to_string();
                            f.index_language = attr("indexLanguage");
                            f.global = attr("global") == "True";
                            f.stored = attr("storeCalculationResults") == "True";
                            f.repetitions = attr("maxRepetition").parse().unwrap_or(1);
                        }
                    }
                    ("Field", "Calculation") => {
                        if let Some(f) = field.as_mut() {
                            f.calc_context = attr("table");
                        }
                    }
                    ("AutoEnter", "Calculation") => {
                        if let Some(f) = field.as_mut() {
                            if f.calc_context.is_empty() {
                                f.calc_context = attr("table");
                            }
                        }
                    }
                    _ => {}
                }
                stack.push(name);
            }
            Ok(Event::Text(t)) => {
                text_buf.push_str(&t.unescape().map(|c| c.into_owned()).unwrap_or_default());
            }
            Ok(Event::CData(t)) => {
                text_buf.push_str(&String::from_utf8_lossy(t.as_ref()));
            }
            Ok(Event::End(_)) => {
                let name = stack.pop().unwrap_or_default();
                let parent = stack.last().cloned().unwrap_or_default();
                let text = std::mem::take(&mut text_buf);

                match (parent.as_str(), name.as_str()) {
                    ("Field", "Comment") => {
                        if let Some(f) = field.as_mut() {
                            f.comment = text.trim().to_string();
                        }
                    }
                    ("Field", "Calculation") => {
                        if let Some(f) = field.as_mut() {
                            f.formula = Some(text);
                        }
                    }
                    ("AutoEnter", "ConstantData") => ae_constant = text,
                    ("AutoEnter", "Calculation") => ae_calc = text,
                    ("Validation", "ErrorMessage") => validation.message = text.trim().to_string(),
                    ("BaseTable", "Field") => {
                        if let Some(mut f) = field.take() {
                            // Resolve the one auto-entry FileMaker actually
                            // applies, and account for the payloads that belong
                            // to switched-off options.
                            let (is_const, is_calc, is_lookup, has_serial) = ae_flags;
                            f.auto = if has_serial {
                                ae_serial.take().map(|(next, increment, generate)| {
                                    AutoEnter::Serial {
                                        next,
                                        increment,
                                        generate,
                                    }
                                })
                            } else if is_lookup {
                                Some(AutoEnter::Lookup {
                                    from_table: ae_lookup.0.clone(),
                                    from_field: ae_lookup.1.clone(),
                                    copy_empty: ae_lookup.2,
                                    no_match: ae_lookup.3.clone(),
                                })
                            } else if is_calc {
                                Some(AutoEnter::Calculation {
                                    formula: ae_calc.clone(),
                                    always: ae_always,
                                })
                            } else if is_const && !ae_constant.is_empty() {
                                Some(AutoEnter::Constant {
                                    value: ae_constant.clone(),
                                })
                            } else {
                                None
                            };

                            // Dead payloads: present in the XML, not applied by
                            // FileMaker. Named, not swallowed.
                            let auto_is_calc =
                                matches!(f.auto, Some(AutoEnter::Calculation { .. }));
                            if !auto_is_calc && !ae_calc.trim().is_empty() {
                                ledger.dropped.push(format!(
                                    "{}: auto-enter calc inactiva (calculation=\"False\") — {}",
                                    f.name,
                                    one_line(&ae_calc, 60)
                                ));
                            }
                            let auto_is_const = matches!(f.auto, Some(AutoEnter::Constant { .. }));
                            if !auto_is_const && !ae_constant.trim().is_empty() {
                                ledger.dropped.push(format!(
                                    "{}: constante de auto-entrada inactiva (constant=\"False\") — {}",
                                    f.name,
                                    one_line(&ae_constant, 60)
                                ));
                            }
                            if f.field_type != "Calculated" && f.formula.is_some() {
                                let dead = f.formula.take().unwrap_or_default();
                                if !dead.trim().is_empty() {
                                    ledger.dropped.push(format!(
                                        "{}: cálculo inactivo (fieldType=\"{}\") — {}",
                                        f.name,
                                        f.field_type,
                                        one_line(&dead, 60)
                                    ));
                                }
                            }

                            // `Calculation table=` only means something when a
                            // calculation survives. FileMaker writes it on dead
                            // payloads too; keeping it there would make a field
                            // differ from itself across a round trip.
                            let has_formula = f.formula.is_some()
                                || matches!(f.auto, Some(AutoEnter::Calculation { .. }));
                            if !has_formula {
                                f.calc_context.clear();
                            }

                            if !validation.is_empty() {
                                f.validation = Some(validation.clone());
                            }
                            ledger.fields += 1;
                            if let Some(t) = tables.last_mut() {
                                t.fields.push(f);
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("XML inválido: {}", e)),
            _ => {}
        }
    }

    if tables.is_empty() {
        return Err("No hay ninguna <BaseTable> en este snippet.".to_string());
    }
    ledger.tables = tables.len();
    Ok((tables, ledger))
}

fn one_line(s: &str, max: usize) -> String {
    let flat: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > max {
        format!("{}…", flat.chars().take(max).collect::<String>())
    } else {
        flat
    }
}

fn decode_entities(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

// ─── Format: model → text ───

const PAD: usize = 10;

/// Render tables as `.fmtable` text.
pub fn format_tables(tables: &[Table]) -> String {
    let mut out = String::new();
    for (i, t) in tables.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&format!("table {}\n", t.name));
        if !t.comment.is_empty() {
            out.push_str(&format!("  comment {}\n", t.comment));
        }
        // FileMaker records an index language on every field, indexed or not.
        // Repeating it 385 times would drown the file, so the common value
        // becomes the table's default and only the odd one out says its own.
        let default_lang = most_common_language(&t.fields);
        if !default_lang.is_empty() {
            out.push_str(&format!(
                "  lang    {}
",
                default_lang
            ));
        }
        for f in &t.fields {
            out.push('\n');
            out.push_str(&format!("field {}\n", f.name));
            kv(&mut out, "type", &f.data_type.to_lowercase());
            if f.field_type == "Calculated" {
                kv(
                    &mut out,
                    "calc",
                    if f.stored { "stored" } else { "unstored" },
                );
            } else if f.field_type == "Summary" {
                kv(&mut out, "calc", "summary");
            }
            if !f.comment.is_empty() {
                kv(&mut out, "comment", &f.comment);
            }
            if let Some(formula) = &f.formula {
                block(&mut out, "formula", formula);
            }
            // Only worth a line when it is not the obvious one: FileMaker
            // evaluates from a table occurrence, which is usually the table
            // itself but need not be.
            if !f.calc_context.is_empty() && f.calc_context != t.name {
                kv(&mut out, "context", &f.calc_context);
            }
            match &f.auto {
                Some(AutoEnter::Serial {
                    next,
                    increment,
                    generate,
                }) => kv(
                    &mut out,
                    "auto",
                    &format!(
                        "serial next={} increment={} generate={}",
                        next, increment, generate
                    ),
                ),
                Some(AutoEnter::Constant { value }) => {
                    kv(&mut out, "auto", &format!("constant {}", value))
                }
                Some(AutoEnter::Calculation { formula, always }) => {
                    kv(
                        &mut out,
                        "auto",
                        if *always { "calc always" } else { "calc" },
                    );
                    block(&mut out, "auto-formula", formula);
                }
                Some(AutoEnter::Lookup {
                    from_table,
                    from_field,
                    copy_empty,
                    no_match,
                }) => kv(
                    &mut out,
                    "auto",
                    &format!(
                        "lookup from {}::{} copy-empty={} no-match={}",
                        from_table,
                        from_field,
                        copy_empty,
                        if no_match.is_empty() {
                            "None"
                        } else {
                            no_match
                        }
                    ),
                ),
                None => {}
            }
            if let Some(v) = &f.validation {
                let mut flags: Vec<&str> = Vec::new();
                if v.not_empty {
                    flags.push("not-empty");
                }
                if v.unique {
                    flags.push("unique");
                }
                if v.existing {
                    flags.push("existing");
                }
                if v.strict {
                    flags.push("strict");
                }
                if v.when == "Always" {
                    flags.push("always");
                }
                if !flags.is_empty() {
                    kv(&mut out, "validate", &flags.join(" "));
                }
                if !v.message.is_empty() {
                    kv(&mut out, "message", &v.message);
                }
            }
            if f.global {
                kv(&mut out, "global", "true");
            }
            if f.repetitions > 1 {
                kv(&mut out, "repetitions", &f.repetitions.to_string());
            }
            kv(&mut out, "index", &f.index);
            if f.index_language != default_lang {
                // `-` is how a field says "no language", which is different
                // from "the table's default".
                kv(
                    &mut out,
                    "lang",
                    if f.index_language.is_empty() {
                        "-"
                    } else {
                        &f.index_language
                    },
                );
            }
        }
    }
    out
}

/// The index language most fields share, which becomes the table's default.
fn most_common_language(fields: &[Field]) -> String {
    let mut counts: Vec<(String, usize)> = Vec::new();
    for f in fields {
        if f.index_language.is_empty() {
            continue;
        }
        match counts.iter_mut().find(|(l, _)| *l == f.index_language) {
            Some((_, n)) => *n += 1,
            None => counts.push((f.index_language.clone(), 1)),
        }
    }
    counts
        .into_iter()
        .max_by_key(|(_, n)| *n)
        .map(|(l, _)| l)
        .unwrap_or_default()
}

fn kv(out: &mut String, key: &str, value: &str) {
    out.push_str(&format!("  {:<width$} {}\n", key, value, width = PAD));
}

/// A multi-line value. Every line is prefixed with `|` so the parser never has
/// to guess where the block ends — a FileMaker calculation can contain blank
/// lines, indentation and anything else.
fn block(out: &mut String, key: &str, value: &str) {
    out.push_str(&format!("  {}\n", key));
    for line in value.trim_end().lines() {
        out.push_str(&format!("    | {}\n", line));
    }
    if value.trim_end().is_empty() {
        out.push_str("    | \n");
    }
}

// ─── Parse: text → model ───

/// Parse `.fmtable` text. Returns every error found, not just the first — the
/// editor underlines them all at once.
pub fn parse_text(text: &str) -> Result<Vec<Table>, Vec<ParseError>> {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let mut errors: Vec<ParseError> = Vec::new();
    let mut tables: Vec<Table> = Vec::new();
    let mut field: Option<Field> = None;
    // The block key currently collecting `|` lines, and what it has so far.
    let mut block_key: Option<String> = None;
    let mut block_lines: Vec<String> = Vec::new();
    let mut auto_kind: Option<String> = None;
    let mut auto_always = false;
    // Index language declared once on the table; each field starts from it.
    let mut table_lang = String::new();

    let err = |errors: &mut Vec<ParseError>, line: usize, msg: String| {
        errors.push(ParseError {
            line,
            message: msg,
            severity: "error".to_string(),
        });
    };

    macro_rules! flush_block {
        ($f:expr) => {
            if let Some(k) = block_key.take() {
                let body = block_lines.join("\n");
                block_lines.clear();
                if let Some(f) = $f.as_mut() {
                    if k == "formula" {
                        f.formula = Some(body);
                    } else {
                        auto_kind = Some(format!("__calc__{}", body));
                    }
                }
            }
        };
    }

    macro_rules! finish_field {
        () => {
            flush_block!(field);
            if let Some(mut f) = field.take() {
                if let Some(kind) = auto_kind.take() {
                    if let Some(body) = kind.strip_prefix("__calc__") {
                        f.auto = Some(AutoEnter::Calculation {
                            formula: body.to_string(),
                            always: auto_always,
                        });
                    }
                }
                auto_always = false;
                if let Some(t) = tables.last_mut() {
                    t.fields.push(f);
                }
            }
        };
    }

    for (i, raw) in text.lines().enumerate() {
        let no = i + 1;
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Block continuation lines belong to whatever block is open.
        if let Some(rest) = trimmed.strip_prefix('|') {
            if block_key.is_none() {
                err(
                    &mut errors,
                    no,
                    "Línea `|` fuera de un bloque. Va debajo de `formula` o `auto-formula`."
                        .to_string(),
                );
                continue;
            }
            block_lines.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
            continue;
        }
        flush_block!(field);

        let (key, value) = match trimmed.split_once(char::is_whitespace) {
            Some((k, v)) => (k, v.trim()),
            None => (trimmed, ""),
        };

        match key {
            "table" => {
                finish_field!();
                table_lang.clear();
                if value.is_empty() {
                    err(&mut errors, no, "`table` necesita un nombre.".to_string());
                }
                tables.push(Table {
                    name: value.to_string(),
                    ..Default::default()
                });
            }
            "field" => {
                finish_field!();
                if tables.is_empty() {
                    err(
                        &mut errors,
                        no,
                        "`field` antes de cualquier `table`. Empezá el archivo con `table <nombre>`."
                            .to_string(),
                    );
                    tables.push(Table::default());
                }
                if value.is_empty() {
                    err(&mut errors, no, "`field` necesita un nombre.".to_string());
                }
                field = Some(Field {
                    name: value.to_string(),
                    data_type: "Text".to_string(),
                    field_type: "Normal".to_string(),
                    index: "none".to_string(),
                    index_language: table_lang.clone(),
                    repetitions: 1,
                    ..Default::default()
                });
            }
            "comment" => {
                if let Some(f) = field.as_mut() {
                    f.comment = value.to_string();
                } else if let Some(t) = tables.last_mut() {
                    t.comment = value.to_string();
                }
            }
            "lang" if field.is_none() => table_lang = value.to_string(),
            "formula" | "auto-formula" => {
                if field.is_none() {
                    err(&mut errors, no, format!("`{}` fuera de un `field`.", key));
                }
                block_key = Some(if key == "formula" {
                    "formula".to_string()
                } else {
                    "auto".to_string()
                });
                // An inline value is allowed for one-liners.
                if !value.is_empty() {
                    block_lines.push(value.to_string());
                }
            }
            _ => {
                let f = match field.as_mut() {
                    Some(f) => f,
                    None => {
                        err(&mut errors, no, format!("`{}` fuera de un `field`.", key));
                        continue;
                    }
                };
                match key {
                    "type" => match normalize_type(value) {
                        Some(t) => f.data_type = t,
                        None => err(
                            &mut errors,
                            no,
                            format!(
                                "Tipo desconocido `{}`. Válidos: text, number, date, time, timestamp, binary (contenedor).",
                                value
                            ),
                        ),
                    },
                    "calc" => match value {
                        "stored" => {
                            f.field_type = "Calculated".to_string();
                            f.stored = true;
                        }
                        "unstored" => {
                            f.field_type = "Calculated".to_string();
                            f.stored = false;
                        }
                        "summary" => f.field_type = "Summary".to_string(),
                        other => err(
                            &mut errors,
                            no,
                            format!(
                                "`calc {}` no es válido. Usá stored, unstored o summary.",
                                other
                            ),
                        ),
                    },
                    "index" => match value {
                        "all" | "minimal" | "none" => f.index = value.to_string(),
                        other => err(
                            &mut errors,
                            no,
                            format!("`index {}` no es válido. Usá all, minimal o none.", other),
                        ),
                    },
                    "lang" => {
                        f.index_language = if value == "-" {
                            String::new()
                        } else {
                            value.to_string()
                        }
                    }
                    "context" => f.calc_context = value.to_string(),
                    "global" => f.global = value != "false",
                    "repetitions" => match value.parse::<u32>() {
                        Ok(n) if n >= 1 => f.repetitions = n,
                        _ => err(
                            &mut errors,
                            no,
                            format!("`repetitions {}` debe ser un entero ≥ 1.", value),
                        ),
                    },
                    "message" => {
                        f.validation.get_or_insert_with(Default::default).message =
                            value.to_string()
                    }
                    "validate" => {
                        let v = f.validation.get_or_insert_with(Default::default);
                        for flag in value.split_whitespace() {
                            match flag {
                                "not-empty" => v.not_empty = true,
                                "unique" => v.unique = true,
                                "existing" => v.existing = true,
                                "strict" => v.strict = true,
                                "always" => v.when = "Always".to_string(),
                                other => err(
                                    &mut errors,
                                    no,
                                    format!(
                                        "`{}` no es una validación conocida. Usá not-empty, unique, existing, strict o always.",
                                        other
                                    ),
                                ),
                            }
                        }
                    }
                    "auto" => match parse_auto(value) {
                        Ok(Some(a)) => f.auto = Some(a),
                        Ok(None) => {
                            // `auto calc [always]` — the formula follows in a block.
                            auto_always = value.split_whitespace().any(|w| w == "always");
                        }
                        Err(m) => err(&mut errors, no, m),
                    },
                    other => err(
                        &mut errors,
                        no,
                        format!(
                            "Clave desconocida `{}`. Válidas: type, calc, comment, formula, context, auto, auto-formula, validate, message, index, lang, global, repetitions.",
                            other
                        ),
                    ),
                }
            }
        }
    }
    finish_field!();

    if tables.is_empty() {
        errors.push(ParseError {
            line: 1,
            message: "El archivo no declara ninguna tabla. Empezá con `table <nombre>`."
                .to_string(),
            severity: "error".to_string(),
        });
    }
    for (ti, t) in tables.iter().enumerate() {
        if t.name.trim().is_empty() {
            errors.push(ParseError {
                line: 1,
                message: format!("La tabla #{} no tiene nombre.", ti + 1),
                severity: "error".to_string(),
            });
        }
        if t.fields.is_empty() {
            errors.push(ParseError {
                line: 1,
                message: format!("La tabla `{}` no tiene ningún campo.", t.name),
                severity: "error".to_string(),
            });
        }
        let mut seen: Vec<&str> = Vec::new();
        for f in &t.fields {
            if seen.contains(&f.name.as_str()) {
                errors.push(ParseError {
                    line: 1,
                    message: format!("Campo duplicado `{}` en la tabla `{}`.", f.name, t.name),
                    severity: "error".to_string(),
                });
            }
            seen.push(&f.name);
            if f.field_type == "Calculated" && f.formula.is_none() {
                errors.push(ParseError {
                    line: 1,
                    message: format!(
                        "`{}` es un campo calculado pero no tiene `formula`.",
                        f.name
                    ),
                    severity: "error".to_string(),
                });
            }
        }
    }

    if errors.is_empty() {
        Ok(tables)
    } else {
        Err(errors)
    }
}

/// Text name → FileMaker `dataType`.
///
/// A container field is `Binary` in the clipboard XML, not "Container" — the
/// name the FileMaker UI shows. Both spellings are accepted on the way in;
/// `binary` is what comes back out, so the round trip is exact.
fn normalize_type(v: &str) -> Option<String> {
    Some(
        match v.to_lowercase().as_str() {
            "text" => "Text",
            "number" => "Number",
            "date" => "Date",
            "time" => "Time",
            "timestamp" => "TimeStamp",
            "binary" | "container" => "Binary",
            _ => return None,
        }
        .to_string(),
    )
}

/// `serial next=… increment=… generate=…` | `constant <v>` | `lookup from
/// TO::field …` | `calc [always]` (the formula arrives as a block, so this
/// returns `Ok(None)`).
fn parse_auto(value: &str) -> Result<Option<AutoEnter>, String> {
    let mut words = value.split_whitespace();
    match words.next() {
        Some("serial") => {
            let mut next = "1".to_string();
            let mut increment = "1".to_string();
            let mut generate = "OnCreation".to_string();
            for w in words {
                match w.split_once('=') {
                    Some(("next", v)) => next = v.to_string(),
                    Some(("increment", v)) => increment = v.to_string(),
                    Some(("generate", v)) => generate = v.to_string(),
                    _ => return Err(format!("`auto serial`: no entiendo `{}`.", w)),
                }
            }
            Ok(Some(AutoEnter::Serial {
                next,
                increment,
                generate,
            }))
        }
        Some("constant") => Ok(Some(AutoEnter::Constant {
            value: value["constant".len()..].trim().to_string(),
        })),
        Some("calc") => Ok(None),
        Some("lookup") => {
            let rest: Vec<&str> = words.collect();
            if rest.first() != Some(&"from") {
                return Err("`auto lookup` necesita `from <TO>::<campo>`.".to_string());
            }
            let target = rest.get(1).copied().unwrap_or_default();
            let (from_table, from_field) = target
                .split_once("::")
                .ok_or("`auto lookup from` necesita la forma `TableOccurrence::campo`.")?;
            let mut copy_empty = false;
            let mut no_match = "None".to_string();
            for w in &rest[2..] {
                match w.split_once('=') {
                    Some(("copy-empty", v)) => copy_empty = v == "true",
                    Some(("no-match", v)) => no_match = v.to_string(),
                    _ => return Err(format!("`auto lookup`: no entiendo `{}`.", w)),
                }
            }
            Ok(Some(AutoEnter::Lookup {
                from_table: from_table.to_string(),
                from_field: from_field.to_string(),
                copy_empty,
                no_match,
            }))
        }
        Some(other) => Err(format!(
            "`auto {}` no es válido. Usá serial, constant, calc o lookup.",
            other
        )),
        None => Err("`auto` necesita un tipo: serial, constant, calc o lookup.".to_string()),
    }
}

/// Validate without building anything — the editor's diagnostics call this.
pub fn lint(text: &str) -> Vec<ParseError> {
    match parse_text(text) {
        Ok(_) => Vec::new(),
        Err(e) => e,
    }
}

// ─── Encode: model → XMTB XML ───

/// Build the `<fmxmlsnippet>` FileMaker accepts on the clipboard.
pub fn encode_xmtb(tables: &[Table]) -> String {
    let mut out = String::from("<fmxmlsnippet type=\"FMObjectList\">");
    for t in tables {
        out.push_str(&format!(
            "<BaseTable comment=\"{}\" name=\"{}\">",
            xml_escape(&t.comment),
            xml_escape(&t.name)
        ));
        for (i, f) in t.fields.iter().enumerate() {
            let ctx = if f.calc_context.is_empty() {
                t.name.clone()
            } else {
                f.calc_context.clone()
            };
            out.push_str(&format!(
                "<Field id=\"{}\" dataType=\"{}\" fieldType=\"{}\" name=\"{}\">",
                i + 1,
                xml_escape(&f.data_type),
                xml_escape(&f.field_type),
                xml_escape(&f.name)
            ));
            if f.field_type == "Calculated" {
                out.push_str(&format!(
                    "<Calculation table=\"{}\"><![CDATA[{}]]></Calculation>",
                    xml_escape(&ctx),
                    f.formula.clone().unwrap_or_default()
                ));
            }
            out.push_str(&format!("<Comment>{}</Comment>", xml_escape(&f.comment)));

            // AutoEnter. The four booleans must agree with the payload: an
            // active flag with no payload is a field FileMaker pastes wrong.
            let (constant, calculation, lookup) = match &f.auto {
                Some(AutoEnter::Constant { .. }) => (true, false, false),
                Some(AutoEnter::Calculation { .. }) => (false, true, false),
                Some(AutoEnter::Lookup { .. }) => (false, false, true),
                _ => (false, false, false),
            };
            let always = matches!(&f.auto, Some(AutoEnter::Calculation { always: true, .. }));
            out.push_str(&format!(
                "<AutoEnter allowEditing=\"True\" constant=\"{}\" furigana=\"False\" lookup=\"{}\" calculation=\"{}\" alwaysEvaluate=\"{}\">",
                bool_str(constant), bool_str(lookup), bool_str(calculation), bool_str(always)
            ));
            match &f.auto {
                Some(AutoEnter::Serial {
                    next,
                    increment,
                    generate,
                }) => out.push_str(&format!(
                    "<Serial increment=\"{}\" nextValue=\"{}\" generate=\"{}\"></Serial>",
                    xml_escape(increment),
                    xml_escape(next),
                    xml_escape(generate)
                )),
                Some(AutoEnter::Constant { value }) => out.push_str(&format!(
                    "<ConstantData>{}</ConstantData>",
                    xml_escape(value)
                )),
                Some(AutoEnter::Calculation { formula, .. }) => out.push_str(&format!(
                    "<Calculation table=\"{}\"><![CDATA[{}]]></Calculation>",
                    xml_escape(&ctx),
                    formula
                )),
                Some(AutoEnter::Lookup {
                    from_table,
                    from_field,
                    copy_empty,
                    no_match,
                }) => out.push_str(&format!(
                    "<Lookup><Table id=\"1\" name=\"{}\"></Table><Field table=\"{}\" id=\"1\" name=\"{}\"></Field><CopyEmptyContent value=\"{}\"></CopyEmptyContent><NoMatchCopyOption value=\"{}\"></NoMatchCopyOption></Lookup>",
                    xml_escape(from_table),
                    xml_escape(from_table),
                    xml_escape(from_field),
                    bool_str(*copy_empty),
                    xml_escape(no_match)
                )),
                None => {}
            }
            out.push_str("</AutoEnter>");

            let v = f.validation.clone().unwrap_or_default();
            out.push_str(&format!(
                "<Validation messageCalc=\"False\" message=\"{}\" maxLength=\"False\" valuelist=\"False\" calculation=\"False\" alwaysValidateCalculation=\"False\" type=\"{}\">",
                bool_str(!v.message.is_empty()),
                if v.when.is_empty() { "OnlyDuringDataEntry" } else { &v.when }
            ));
            out.push_str(&format!(
                "<NotEmpty value=\"{}\"></NotEmpty><Unique value=\"{}\"></Unique><Existing value=\"{}\"></Existing><StrictValidation value=\"{}\"></StrictValidation>",
                bool_str(v.not_empty), bool_str(v.unique), bool_str(v.existing), bool_str(v.strict)
            ));
            if !v.message.is_empty() {
                out.push_str(&format!(
                    "<ErrorMessage>{}</ErrorMessage>",
                    xml_escape(&v.message)
                ));
            }
            out.push_str("</Validation>");

            let index = match f.index.as_str() {
                "all" => "All",
                "minimal" => "Minimal",
                _ => "None",
            };
            out.push_str("<Storage ");
            if f.field_type == "Calculated" {
                out.push_str(&format!(
                    "storeCalculationResults=\"{}\" ",
                    bool_str(f.stored)
                ));
            }
            out.push_str(&format!("index=\"{}\" ", index));
            if !f.index_language.is_empty() {
                out.push_str(&format!(
                    "indexLanguage=\"{}\" ",
                    xml_escape(&f.index_language)
                ));
            }
            out.push_str(&format!(
                "global=\"{}\" maxRepetition=\"{}\"></Storage>",
                bool_str(f.global),
                f.repetitions.max(1)
            ));
            out.push_str("</Field>");
        }
        out.push_str("</BaseTable>");
    }
    out.push_str("</fmxmlsnippet>");
    out
}

fn bool_str(b: bool) -> &'static str {
    if b { "True" } else { "False" }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SERIAL_FIELD: &str = r#"<fmxmlsnippet type="FMObjectList"><BaseTable comment="" name="PedidosItems"><Field id="872" dataType="Number" fieldType="Normal" name="PedIte_Ref"><Comment>Ref. Num.</Comment><AutoEnter allowEditing="True" constant="False" furigana="False" lookup="False" calculation="False"><Serial increment="1" nextValue="1068893" generate="OnCreation"></Serial><ConstantData></ConstantData></AutoEnter><Validation messageCalc="False" message="False" maxLength="False" valuelist="False" calculation="False" alwaysValidateCalculation="False" type="OnlyDuringDataEntry"><NotEmpty value="True"></NotEmpty><Unique value="True"></Unique><Existing value="False"></Existing><StrictValidation value="False"></StrictValidation><ErrorMessage>Verifique la referencia.</ErrorMessage></Validation><Storage index="All" indexLanguage="Spanish_Traditional" global="False" maxRepetition="1"></Storage></Field></BaseTable></fmxmlsnippet>"#;

    #[test]
    fn decodes_a_serial_field_with_its_validation() {
        let (tables, ledger) = decode_xmtb(SERIAL_FIELD).unwrap();
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].name, "PedidosItems");
        let f = &tables[0].fields[0];
        assert_eq!(f.name, "PedIte_Ref");
        assert_eq!(f.data_type, "Number");
        assert_eq!(
            f.auto,
            Some(AutoEnter::Serial {
                next: "1068893".to_string(),
                increment: "1".to_string(),
                generate: "OnCreation".to_string(),
            })
        );
        let v = f.validation.as_ref().unwrap();
        assert!(v.not_empty && v.unique);
        assert_eq!(v.message, "Verifique la referencia.");
        assert_eq!(f.index, "all");
        assert_eq!(ledger.fields, 1);
        assert!(ledger.dropped.is_empty());
    }

    #[test]
    fn a_dead_payload_is_reported_not_swallowed() {
        // FileMaker leaves the calculation of a switched-off option in the XML.
        // Carrying it would invent behaviour; dropping it quietly would hide a
        // change. It gets named in the ledger.
        let xml = SERIAL_FIELD.replace(
            "<ConstantData></ConstantData>",
            r#"<ConstantData>viejo</ConstantData><Calculation table="T"><![CDATA[Get ( UUID )]]></Calculation>"#,
        );
        let (_, ledger) = decode_xmtb(&xml).unwrap();
        assert_eq!(ledger.dropped.len(), 2, "{:?}", ledger.dropped);
        assert!(ledger.dropped.iter().any(|d| d.contains("Get ( UUID )")));
        assert!(ledger.dropped.iter().any(|d| d.contains("viejo")));
    }

    #[test]
    fn text_round_trips_through_the_model() {
        let (tables, _) = decode_xmtb(SERIAL_FIELD).unwrap();
        let text = format_tables(&tables);
        let back = parse_text(&text).expect("el texto que emitimos tiene que parsear");
        assert_eq!(back[0].name, tables[0].name);
        assert_eq!(back[0].fields[0], tables[0].fields[0]);
    }

    #[test]
    fn xml_round_trips_through_the_text_format() {
        // The real contract: clipboard → text → clipboard produces a snippet
        // that means the same thing.
        let (tables, _) = decode_xmtb(SERIAL_FIELD).unwrap();
        let text = format_tables(&tables);
        let parsed = parse_text(&text).unwrap();
        let xml = encode_xmtb(&parsed);
        let (again, _) = decode_xmtb(&xml).unwrap();
        assert_eq!(again[0].fields[0], tables[0].fields[0]);
    }

    #[test]
    fn a_multiline_calculation_survives_both_directions() {
        let xml = r#"<fmxmlsnippet type="FMObjectList"><BaseTable comment="" name="T"><Field id="1" dataType="Text" fieldType="Calculated" name="c"><Calculation table="T"><![CDATA[Let ( [
  ~a = 1 ;
  ~b = 2
] ;
  ~a + ~b )]]></Calculation><Comment></Comment><AutoEnter alwaysEvaluate="False"></AutoEnter><Storage storeCalculationResults="True" index="None" global="False" maxRepetition="1"></Storage></Field></BaseTable></fmxmlsnippet>"#;
        let (tables, _) = decode_xmtb(xml).unwrap();
        assert!(
            tables[0].fields[0]
                .formula
                .as_ref()
                .unwrap()
                .contains("~a + ~b")
        );

        let text = format_tables(&tables);
        assert!(text.contains("  formula\n"), "{}", text);
        assert!(text.contains("    | Let ( ["), "{}", text);

        let back = parse_text(&text).unwrap();
        assert_eq!(back[0].fields[0].formula, tables[0].fields[0].formula);
    }

    #[test]
    fn a_lookup_keeps_its_source() {
        let xml = r#"<fmxmlsnippet type="FMObjectList"><BaseTable comment="" name="T"><Field id="1" dataType="Text" fieldType="Normal" name="f"><Comment></Comment><AutoEnter allowEditing="True" constant="False" furigana="False" lookup="True" calculation="False"><Lookup><Table id="9" name="Clientes"></Table><Field table="Clientes" id="4" name="nombre"></Field><CopyEmptyContent value="False"></CopyEmptyContent><NoMatchCopyOption value="None"></NoMatchCopyOption></Lookup></AutoEnter><Storage index="None" global="False" maxRepetition="1"></Storage></Field></BaseTable></fmxmlsnippet>"#;
        let (tables, _) = decode_xmtb(xml).unwrap();
        assert_eq!(
            tables[0].fields[0].auto,
            Some(AutoEnter::Lookup {
                from_table: "Clientes".to_string(),
                from_field: "nombre".to_string(),
                copy_empty: false,
                no_match: "None".to_string(),
            })
        );
        let text = format_tables(&tables);
        assert!(
            text.contains("auto       lookup from Clientes::nombre"),
            "{}",
            text
        );
        let back = parse_text(&text).unwrap();
        assert_eq!(back[0].fields[0].auto, tables[0].fields[0].auto);
    }

    #[test]
    fn several_tables_stay_separate() {
        let text = "table A\n\nfield a\n  type text\n\ntable B\n\nfield b\n  type number\n";
        let t = parse_text(text).unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].fields.len(), 1);
        assert_eq!(t[1].fields[0].data_type, "Number");
    }

    // ─── The linter ───

    #[test]
    fn an_unknown_type_is_an_error_that_lists_the_valid_ones() {
        let e = lint("table T\n\nfield a\n  type numerico\n");
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].line, 4);
        assert!(e[0].message.contains("timestamp"), "{}", e[0].message);
        // `container` is the FileMaker UI name for `binary`; both must be accepted.
        assert!(
            lint(
                "table T

field a
  type container
"
            )
            .is_empty()
        );
        assert!(
            lint(
                "table T

field a
  type binary
"
            )
            .is_empty()
        );
    }

    #[test]
    fn a_calc_field_without_a_formula_is_caught() {
        let e = lint("table T\n\nfield a\n  type text\n  calc stored\n");
        assert!(e.iter().any(|x| x.message.contains("no tiene `formula`")));
    }

    #[test]
    fn duplicate_field_names_are_caught() {
        let e = lint("table T\n\nfield a\n  type text\n\nfield a\n  type text\n");
        assert!(e.iter().any(|x| x.message.contains("duplicado")));
    }

    #[test]
    fn a_key_outside_a_field_points_at_the_right_line() {
        let e = lint("table T\n  type text\n\nfield a\n  type text\n");
        assert_eq!(e[0].line, 2);
        assert!(e[0].message.contains("fuera de un `field`"));
    }

    #[test]
    fn every_error_is_reported_not_just_the_first() {
        let e = lint("table T\n\nfield a\n  type nope\n  index siempre\n  validate raro\n");
        assert_eq!(e.len(), 3, "{:?}", e);
    }

    #[test]
    fn a_stray_pipe_line_says_where_it_belongs() {
        let e = lint("table T\n\nfield a\n  type text\n  | huerfana\n");
        assert!(e[0].message.contains("fuera de un bloque"));
    }

    #[test]
    fn valid_text_lints_clean() {
        let text = "# un comentario\ntable T\n\nfield id\n  type number\n  auto       serial next=5 increment=1 generate=OnCreation\n  validate   not-empty unique\n  index      all\n";
        assert!(lint(text).is_empty(), "{:?}", lint(text));
    }

    #[test]
    fn an_empty_file_is_an_error_with_a_hint() {
        let e = lint("\n\n");
        assert_eq!(e.len(), 1);
        assert!(e[0].message.contains("table <nombre>"));
    }
}

#[cfg(test)]
mod real_world {
    use super::*;

    /// The only test that touches a real export. It is skipped when the file is
    /// not there (it holds a customer's field names and never enters the repo),
    /// but when it is, it is the one that matters: 385 fields through
    /// XML → text → XML → model, compared field by field.
    #[test]
    fn a_real_two_table_snippet_survives_the_full_round_trip() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("pruebas.xml");
        let Ok(xml) = std::fs::read_to_string(&path) else {
            eprintln!("saltado: no hay {}", path.display());
            return;
        };

        let (tables, ledger) = decode_xmtb(&xml).expect("decodifica");
        let text = format_tables(&tables);
        let parsed = parse_text(&text)
            .unwrap_or_else(|e| panic!("el texto emitido no parsea: {:?}", &e[..e.len().min(5)]));
        let again = encode_xmtb(&parsed);
        let (final_tables, _) = decode_xmtb(&again).expect("re-decodifica");

        assert_eq!(final_tables.len(), tables.len());
        for (a, b) in tables.iter().zip(final_tables.iter()) {
            assert_eq!(a.name, b.name);
            assert_eq!(a.fields.len(), b.fields.len(), "tabla {}", a.name);
            for (fa, fb) in a.fields.iter().zip(b.fields.iter()) {
                assert_eq!(fa, fb, "campo {} de {}", fa.name, a.name);
            }
        }
        eprintln!(
            "round-trip real: {} tablas, {} campos, {} descartes anotados",
            ledger.tables,
            ledger.fields,
            ledger.dropped.len()
        );
    }
}
