// SQL safety and composition for the live-data path.
//
// Two ideas drive this module.
//
// 1. **Allowlist, not blocklist.** We never search for forbidden words —
//    `DELETE` hides in string literals, in `/**/` comments and behind `;`
//    chaining, and a blocklist fails *open* on whatever the author did not
//    foresee. Instead we scan the statement and accept it only if it is a
//    single statement whose first keyword is SELECT. Anything unrecognised
//    fails *closed*, which is the behaviour you want with a language model on
//    the other end.
//
// 2. **Validate the text, then send the text.** We do not rewrite the user's
//    SQL beyond appending a row limit: FileMaker's dialect has its own quirks
//    and a well-meaning rewrite is a great way to break a working query.
//
// The limits imposed here are not security theatre against the model — reads
// are free and unrestricted by design. They exist so a runaway `SELECT` cannot
// wedge a production server or flood the caller's context window.

/// Bookkeeping from one pass over a statement, ignoring anything inside string
/// literals, quoted identifiers and comments.
struct Scan {
    /// Statement separators found in code position (a single trailing one is
    /// tolerated and stripped by the caller).
    semicolons: Vec<usize>,
    /// First bare word in code position — the statement's verb.
    first_word: String,
    /// Whether a bare `FETCH` appears, so we don't stack two row limits.
    has_fetch: bool,
    /// Byte offset just past the last non-whitespace character.
    end_of_code: usize,
}

/// Single pass over `sql`, skipping literals and comments.
fn scan(sql: &str) -> Result<Scan, String> {
    let b = sql.as_bytes();
    let mut i = 0usize;
    let mut semicolons = Vec::new();
    let mut first_word = String::new();
    let mut has_fetch = false;
    let mut end_of_code = 0usize;

    while i < b.len() {
        let c = b[i] as char;

        // ── comments ──
        if c == '-' && i + 1 < b.len() && b[i + 1] == b'-' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && i + 1 < b.len() && b[i + 1] == b'*' {
            let start = i;
            i += 2;
            loop {
                if i + 1 >= b.len() {
                    return Err(format!(
                        "Unterminated block comment starting at byte {}.",
                        start
                    ));
                }
                if b[i] == b'*' && b[i + 1] == b'/' {
                    i += 2;
                    break;
                }
                i += 1;
            }
            continue;
        }

        // ── string literal / quoted identifier (doubled quote escapes) ──
        if c == '\'' || c == '"' {
            let quote = b[i];
            let start = i;
            i += 1;
            loop {
                if i >= b.len() {
                    return Err(format!(
                        "Unterminated {} starting at byte {}.",
                        if quote == b'\'' {
                            "string literal"
                        } else {
                            "quoted identifier"
                        },
                        start
                    ));
                }
                if b[i] == quote {
                    if i + 1 < b.len() && b[i + 1] == quote {
                        i += 2; // escaped quote, keep going
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            end_of_code = i;
            continue;
        }

        // ── code position ──
        if c == ';' {
            semicolons.push(i);
            i += 1;
            end_of_code = i;
            continue;
        }
        if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < b.len() && ((b[i] as char).is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let word = &sql[start..i];
            if first_word.is_empty() {
                first_word = word.to_ascii_uppercase();
            }
            if word.eq_ignore_ascii_case("fetch") {
                has_fetch = true;
            }
            end_of_code = i;
            continue;
        }
        if !c.is_whitespace() {
            end_of_code = i + 1;
        }
        i += 1;
    }

    Ok(Scan {
        semicolons,
        first_word,
        has_fetch,
        end_of_code,
    })
}

/// Accept a read-only statement, or explain precisely why not.
///
/// Returns the statement to send: unchanged except that a trailing `;` is
/// removed and a row limit is appended when the query has none.
pub fn sanitize_select(sql: &str, max_rows: usize) -> Result<String, String> {
    let trimmed = sql.trim();
    if trimmed.is_empty() {
        return Err("Empty SQL statement.".to_string());
    }
    let s = scan(trimmed)?;

    // A single trailing separator is fine; anything else means more than one
    // statement, which is how a read turns into a write.
    let body_end = s.end_of_code.min(trimmed.len());
    let trailing_only = match s.semicolons.len() {
        0 => true,
        1 => s.semicolons[0] + 1 >= body_end,
        _ => false,
    };
    if !trailing_only {
        return Err(
            "Only one statement is allowed. Multiple statements separated by ';' are rejected."
                .to_string(),
        );
    }

    if !s.first_word.eq_ignore_ascii_case("SELECT") {
        let verb = if s.first_word.is_empty() {
            "this".to_string()
        } else {
            s.first_word.clone()
        };
        return Err(format!(
            "Only SELECT statements are allowed here; got {}. Live queries are read-only \
             by design: reads are unrestricted, writes go through a separate reviewed path.",
            verb
        ));
    }

    // Drop the trailing separator so we can append to the statement.
    let mut out = trimmed.to_string();
    if let Some(pos) = s.semicolons.first() {
        out.truncate(*pos);
    }
    let out = out.trim_end().to_string();

    if s.has_fetch {
        return Ok(out);
    }
    Ok(format!("{} FETCH FIRST {} ROWS ONLY", out, max_rows))
}

/// Validate a SQL *fragment* supplied for a structured query (a WHERE or ORDER
/// BY body). The fragment is embedded in a statement we build ourselves and
/// the whole thing is validated again by `sanitize_select`, so the job here is
/// to reject the constructs that would let a fragment escape its clause.
pub fn check_fragment(fragment: &str, what: &str) -> Result<(), String> {
    let s = scan(fragment)?;
    if !s.semicolons.is_empty() {
        return Err(format!("The {} must not contain ';'.", what));
    }
    Ok(())
}

/// Quote an identifier for FileMaker SQL. Always quoted, never conditionally:
/// FileMaker table occurrences and field names routinely contain `_`, spaces
/// and accented characters, and unquoted forms fail in ways that are hard to
/// read. An embedded quote is doubled.
pub fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Compose a SELECT from structured parts. This is the surface the AI uses by
/// default: with no SQL string to assemble there is no injection to reason
/// about, and the engine gets to apply FileMaker's quoting rules for you.
pub fn build_select(
    table: &str,
    fields: Option<&[String]>,
    filter: Option<&str>,
    order_by: Option<&str>,
    max_rows: usize,
) -> Result<String, String> {
    if table.trim().is_empty() {
        return Err("A table (occurrence) name is required.".to_string());
    }
    let projection = match fields {
        Some(f) if !f.is_empty() => f
            .iter()
            .map(|c| quote_ident(c))
            .collect::<Vec<_>>()
            .join(", "),
        _ => "*".to_string(),
    };
    let mut sql = format!("SELECT {} FROM {}", projection, quote_ident(table));
    if let Some(f) = filter {
        let f = f.trim();
        if !f.is_empty() {
            check_fragment(f, "filter")?;
            sql.push_str(" WHERE ");
            sql.push_str(f);
        }
    }
    if let Some(o) = order_by {
        let o = o.trim();
        if !o.is_empty() {
            check_fragment(o, "order_by")?;
            sql.push_str(" ORDER BY ");
            sql.push_str(o);
        }
    }
    sanitize_select(&sql, max_rows)
}

/// `SELECT COUNT(*)` over the same shape — used for `count_rows` and, later,
/// as the dry-run behind a proposed write.
pub fn build_count(table: &str, filter: Option<&str>) -> Result<String, String> {
    if table.trim().is_empty() {
        return Err("A table (occurrence) name is required.".to_string());
    }
    // Aliased: an unnamed COUNT(*) comes back with an empty column name, which
    // reads as a blank header in the CLI and as "" as a JSON key.
    let mut sql = format!(
        "SELECT COUNT(*) AS \"row_count\" FROM {}",
        quote_ident(table)
    );
    if let Some(f) = filter {
        let f = f.trim();
        if !f.is_empty() {
            check_fragment(f, "filter")?;
            sql.push_str(" WHERE ");
            sql.push_str(f);
        }
    }
    // A count returns one row; no limit needed, but validate the shape anyway.
    sanitize_select(&sql, 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_select_gets_a_row_limit() {
        let out = sanitize_select("SELECT a FROM t", 100).unwrap();
        assert_eq!(out, "SELECT a FROM t FETCH FIRST 100 ROWS ONLY");
    }

    #[test]
    fn existing_fetch_is_respected() {
        let sql = "SELECT a FROM t FETCH FIRST 5 ROWS ONLY";
        assert_eq!(sanitize_select(sql, 100).unwrap(), sql);
    }

    #[test]
    fn trailing_semicolon_is_stripped_not_rejected() {
        let out = sanitize_select("SELECT a FROM t;", 10).unwrap();
        assert_eq!(out, "SELECT a FROM t FETCH FIRST 10 ROWS ONLY");
    }

    #[test]
    fn chained_statements_are_rejected() {
        let err = sanitize_select("SELECT a FROM t; DELETE FROM t", 10).unwrap_err();
        assert!(err.contains("Only one statement"), "{}", err);
    }

    #[test]
    fn writes_are_rejected_by_verb_not_by_word_search() {
        for sql in [
            "DELETE FROM t",
            "UPDATE t SET a = 1",
            "INSERT INTO t VALUES (1)",
            "DROP TABLE t",
        ] {
            assert!(sanitize_select(sql, 10).is_err(), "should reject: {}", sql);
        }
    }

    #[test]
    fn a_literal_containing_delete_is_still_a_valid_read() {
        // The exact case a blocklist gets wrong.
        let out = sanitize_select("SELECT a FROM t WHERE note = 'DELETE ME'", 10).unwrap();
        assert!(out.starts_with("SELECT a FROM t WHERE note = 'DELETE ME'"));
    }

    #[test]
    fn a_semicolon_inside_a_literal_is_not_a_statement_separator() {
        let out = sanitize_select("SELECT a FROM t WHERE s = 'a;b'", 10).unwrap();
        assert!(out.contains("'a;b'"));
    }

    #[test]
    fn comments_cannot_smuggle_a_verb() {
        // Leading comment: the verb is still SELECT, and this must work.
        let out = sanitize_select("/* DELETE */ SELECT a FROM t", 10).unwrap();
        assert!(out.contains("SELECT a FROM t"));
        // A comment cannot make a write look like a read either.
        assert!(sanitize_select("/* SELECT */ DELETE FROM t", 10).is_err());
    }

    #[test]
    fn a_commented_out_semicolon_does_not_split_the_statement() {
        let out = sanitize_select("SELECT a FROM t -- ; DELETE FROM t\n", 10).unwrap();
        assert!(out.starts_with("SELECT a FROM t"));
    }

    #[test]
    fn unterminated_literal_is_an_error_not_a_pass() {
        assert!(sanitize_select("SELECT a FROM t WHERE s = 'oops", 10).is_err());
        assert!(sanitize_select("SELECT a FROM t /* oops", 10).is_err());
    }

    #[test]
    fn doubled_quotes_escape_correctly() {
        let out = sanitize_select("SELECT a FROM t WHERE s = 'it''s ok'", 10).unwrap();
        assert!(out.contains("'it''s ok'"));
    }

    #[test]
    fn identifiers_are_always_quoted_and_escaped() {
        assert_eq!(quote_ident("Ta_d_Stock"), "\"Ta_d_Stock\"");
        assert_eq!(quote_ident("we\"ird"), "\"we\"\"ird\"");
    }

    #[test]
    fn structured_select_quotes_everything() {
        let sql = build_select(
            "_stockRemoto",
            Some(&["id".to_string(), "cantidad".to_string()]),
            Some("cantidad > 0"),
            Some("id DESC"),
            50,
        )
        .unwrap();
        assert_eq!(
            sql,
            "SELECT \"id\", \"cantidad\" FROM \"_stockRemoto\" WHERE cantidad > 0 \
             ORDER BY id DESC FETCH FIRST 50 ROWS ONLY"
        );
    }

    #[test]
    fn structured_select_defaults_to_star() {
        let sql = build_select("t", None, None, None, 5).unwrap();
        assert_eq!(sql, "SELECT * FROM \"t\" FETCH FIRST 5 ROWS ONLY");
    }

    #[test]
    fn a_filter_cannot_escape_its_clause() {
        let err = build_select("t", None, Some("1=1; DROP TABLE t"), None, 5).unwrap_err();
        assert!(err.contains("must not contain ';'"), "{}", err);
    }

    #[test]
    fn count_builds_a_single_row_query() {
        let sql = build_count("t", Some("a = 1")).unwrap();
        assert!(
            sql.starts_with("SELECT COUNT(*) AS \"row_count\" FROM \"t\" WHERE a = 1"),
            "{}",
            sql
        );
    }
}
