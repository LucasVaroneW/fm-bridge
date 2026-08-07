// fm-bridge-odbc — the ODBC sidecar.
//
// Reads ONE request as JSON on stdin, runs ONE query, writes ONE response as
// JSON on stdout, exits. That "process per query" shape is the whole point:
// when the process ends the OS closes the socket, so a connection cannot be
// leaked or left half-open on the FileMaker server. There is no pool, no
// daemon and no shared state to go wrong — the parent enforces a wall-clock
// deadline by killing us, which is far more reliable than SQLCancel on a
// driver that is already wedged.
//
// This talks plain ODBC. It never ships or contains a vendor driver: the ODBC
// driver manager (odbc32.dll on Windows, unixODBC/iODBC elsewhere) loads
// whatever driver the user installed themselves.

use odbc_api::{ConnectionOptions, Cursor, Environment, ResultSetMetadata};
use serde::{Deserialize, Serialize};
use std::io::Read;

#[derive(Deserialize)]
struct Request {
    /// Full ODBC connection string, already assembled by the engine.
    connection_string: String,
    sql: String,
    #[serde(default = "default_connect_timeout")]
    connect_timeout_s: u32,
    /// Hard stop on rows read into memory, independent of any SQL row limit.
    #[serde(default = "default_max_rows")]
    max_rows: usize,
    /// Hard stop on the size of a single cell, so one container/blob field
    /// cannot blow up the caller's context window.
    #[serde(default = "default_max_cell")]
    max_cell_chars: usize,
    /// Hard stop on the whole result set. Row and cell caps alone do not bound
    /// the payload: a FileMaker table occurrence routinely has 60+ columns, so
    /// `max_rows × columns × max_cell_chars` can still be enormous. This is the
    /// budget that actually protects the caller's context window.
    #[serde(default = "default_max_total")]
    max_total_chars: usize,
}

fn default_connect_timeout() -> u32 {
    15
}
fn default_max_rows() -> usize {
    500
}
fn default_max_cell() -> usize {
    500
}
fn default_max_total() -> usize {
    20_000
}

#[derive(Serialize)]
struct Response {
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    columns: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rows: Option<Vec<Vec<Option<String>>>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    row_count: Option<usize>,
    /// True when `max_rows` cut the result short — the caller must not treat
    /// the result as complete.
    #[serde(skip_serializing_if = "Option::is_none")]
    truncated: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    elapsed_ms: Option<u128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    /// Raw SQLSTATE when we can recover it, so the engine can map the failure
    /// to an actionable hint instead of echoing driver noise.
    #[serde(skip_serializing_if = "Option::is_none")]
    sqlstate: Option<String>,
}

impl Response {
    fn failure(error: String) -> Self {
        let sqlstate = extract_sqlstate(&error);
        Response {
            status: "error",
            columns: None,
            rows: None,
            row_count: None,
            truncated: None,
            elapsed_ms: None,
            error: Some(error),
            sqlstate,
        }
    }
}

/// Recover the SQLSTATE from a diagnostic so the engine can attach an
/// actionable hint instead of echoing driver noise.
///
/// Two shapes occur in practice: odbc-api renders `State: IM002, Native error:
/// …`, while driver managers and ODBC tooling use the bracketed `[IM002]`
/// form. Both are accepted; the `State:` form is checked first because it is
/// what this sidecar actually produces.
fn extract_sqlstate(msg: &str) -> Option<String> {
    if let Some(pos) = msg.find("State:") {
        let rest = msg[pos + "State:".len()..].trim_start();
        let candidate: String = rest.chars().take(5).collect();
        if is_sqlstate(&candidate) {
            return Some(candidate);
        }
    }
    let chars: Vec<char> = msg.chars().collect();
    for i in 0..chars.len() {
        if chars[i] == '[' && i + 6 < chars.len() && chars[i + 6] == ']' {
            let candidate: String = chars[i + 1..i + 6].iter().collect();
            if is_sqlstate(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

/// A SQLSTATE is exactly five uppercase alphanumerics.
fn is_sqlstate(s: &str) -> bool {
    s.chars().count() == 5
        && s.chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

fn main() {
    let mut input = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut input) {
        emit(Response::failure(format!("stdin read error: {}", e)));
        return;
    }
    let req: Request = match serde_json::from_str(&input) {
        Ok(r) => r,
        Err(e) => {
            emit(Response::failure(format!("bad request JSON: {}", e)));
            return;
        }
    };
    match run(&req) {
        Ok(resp) => emit(resp),
        Err(e) => emit(Response::failure(e)),
    }
}

fn emit(resp: Response) {
    let exit = if resp.status == "error" { 1 } else { 0 };
    println!(
        "{}",
        serde_json::to_string(&resp).unwrap_or_else(|_| r#"{"status":"error"}"#.to_string())
    );
    std::process::exit(exit);
}

fn run(req: &Request) -> Result<Response, String> {
    let started = std::time::Instant::now();

    let env = Environment::new().map_err(|e| format!("ODBC environment: {}", e))?;
    let conn = env
        .connect_with_connection_string(
            &req.connection_string,
            ConnectionOptions {
                login_timeout_sec: Some(req.connect_timeout_s),
                ..Default::default()
            },
        )
        .map_err(|e| format!("connect: {}", e))?;

    // No statement-level timeout is set here: odbc-api 9 exposes none, and a
    // driver wedged mid-query is precisely the one that would ignore it. The
    // parent's wall-clock deadline — which kills this process outright — is the
    // enforcement point, and it works regardless of what the driver is doing.
    let cursor = conn
        .execute(&req.sql, ())
        .map_err(|e| format!("execute: {}", e))?;

    // A statement with no result set (shouldn't happen — the engine only sends
    // SELECT — but never assume).
    let mut cursor = match cursor {
        Some(c) => c,
        None => {
            return Ok(Response {
                status: "ok",
                columns: Some(vec![]),
                rows: Some(vec![]),
                row_count: Some(0),
                truncated: Some(false),
                elapsed_ms: Some(started.elapsed().as_millis()),
                error: None,
                sqlstate: None,
            });
        }
    };

    let n_cols = cursor
        .num_result_cols()
        .map_err(|e| format!("column count: {}", e))? as u16;
    let mut columns = Vec::with_capacity(n_cols as usize);
    for i in 1..=n_cols {
        columns.push(
            cursor
                .col_name(i)
                .map_err(|e| format!("column name {}: {}", i, e))?,
        );
    }

    let mut rows: Vec<Vec<Option<String>>> = Vec::new();
    let mut truncated = false;
    let mut budget = req.max_total_chars;

    // Character data is read through the *wide* (UTF-16) ODBC API wherever we
    // can. The narrow API hands back bytes in the driver's own codepage, which
    // silently mangles every accented character — "Almacén" arrives as
    // "Almac?n". FileMaker databases outside English-speaking shops are full of
    // them, so this is a correctness issue, not a nicety.
    //
    // macOS is the exception: linking iODBC forces odbc-api's `narrow` feature
    // (iODBC's wide API uses 4-byte wchar), so there we read narrow and trust
    // the driver's UTF-8.
    #[cfg(not(target_os = "macos"))]
    let mut wide_buf: Vec<u16> = Vec::new();
    #[cfg(target_os = "macos")]
    let mut narrow_buf: Vec<u8> = Vec::new();

    while let Some(mut row) = cursor.next_row().map_err(|e| format!("fetch: {}", e))? {
        if rows.len() >= req.max_rows {
            truncated = true;
            break;
        }
        let mut out = Vec::with_capacity(n_cols as usize);
        let mut row_chars = 0usize;
        for i in 1..=n_cols {
            #[cfg(not(target_os = "macos"))]
            let cell: Option<String> = {
                wide_buf.clear();
                let not_null = row
                    .get_wide_text(i, &mut wide_buf)
                    .map_err(|e| format!("read column {}: {}", i, e))?;
                not_null.then(|| String::from_utf16_lossy(&wide_buf))
            };
            #[cfg(target_os = "macos")]
            let cell: Option<String> = {
                narrow_buf.clear();
                let not_null = row
                    .get_text(i, &mut narrow_buf)
                    .map_err(|e| format!("read column {}: {}", i, e))?;
                not_null.then(|| String::from_utf8_lossy(&narrow_buf).into_owned())
            };

            match cell {
                Some(mut s) => {
                    if s.chars().count() > req.max_cell_chars {
                        s = s.chars().take(req.max_cell_chars).collect::<String>() + "…";
                    }
                    row_chars += s.chars().count();
                    out.push(Some(s));
                }
                None => out.push(None),
            }
        }
        // Always keep the first row, even if it alone busts the budget: an empty
        // result with `truncated` set would tell the caller nothing about shape.
        if row_chars > budget && !rows.is_empty() {
            truncated = true;
            break;
        }
        budget = budget.saturating_sub(row_chars);
        rows.push(out);
        if budget == 0 {
            truncated = true;
            break;
        }
    }

    Ok(Response {
        status: "ok",
        row_count: Some(rows.len()),
        columns: Some(columns),
        rows: Some(rows),
        truncated: Some(truncated),
        elapsed_ms: Some(started.elapsed().as_millis()),
        error: None,
        sqlstate: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact text this sidecar produced against a real driver manager —
    /// odbc-api uses `State:`, not the bracketed form.
    #[test]
    fn sqlstate_is_pulled_from_the_odbc_api_shape() {
        let msg = "connect: ODBC emitted an error calling 'SQLDriverConnect':\n\
                   State: IM002, Native error: 0, Message: [Microsoft][Administrador de \
                   controladores ODBC] No se encuentra el nombre del origen de datos";
        assert_eq!(extract_sqlstate(msg).as_deref(), Some("IM002"));
    }

    #[test]
    fn sqlstate_is_also_pulled_from_the_bracketed_shape() {
        let msg = "ERROR [IM002] [Microsoft][ODBC Driver Manager] Data source name not found";
        assert_eq!(extract_sqlstate(msg).as_deref(), Some("IM002"));
    }

    #[test]
    fn vendor_prefixes_are_not_mistaken_for_a_sqlstate() {
        assert_eq!(extract_sqlstate("[Microsoft][whatever] boom"), None);
        assert_eq!(extract_sqlstate("no brackets at all"), None);
    }
}
