// Live-data provider: drives the ODBC sidecar.
//
// Robustness is the design, not a feature bolted on:
//
//  * **One disposable process per query.** No pool, no daemon. When the child
//    exits the OS closes the socket, so an orphaned or half-open connection to
//    the FileMaker server is not something we have to remember to avoid — it
//    cannot happen. Reconnecting costs a few hundred milliseconds, which is a
//    good trade for never wedging a production server.
//  * **The parent kills, it does not ask.** A wall-clock deadline backstops the
//    driver's own timeouts, because a driver that is already stuck is exactly
//    the one that will ignore them.
//  * **One query at a time.** A model can happily fan out ten calls; serialising
//    them here is what stops that from becoming ten connections on the server.
//
// The engine never links ODBC itself, so all of this degrades to a clear
// message when the sidecar or the vendor driver is absent.

use serde_json::{Value, json};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::data_config::{DataConfig, connection_string, resolve_password};
use crate::data_sql;

/// Serialises every live query in this process (see module docs).
static QUERY_LOCK: Mutex<()> = Mutex::new(());

/// A query that came back from the sidecar.
struct SidecarResult {
    value: Value,
    sqlstate: Option<String>,
    ok: bool,
}

/// Candidate sidecar executables, most specific first.
///
/// Both bitnesses are tried because on Windows the FileMaker ODBC driver is
/// frequently registered 32-bit only (FileMaker Pro installs that one), and a
/// 64-bit process physically cannot load it. Rather than make the user find
/// out what "bitness" means, we try the other build when the first reports a
/// missing driver.
fn sidecar_candidates(cfg: &DataConfig) -> Vec<PathBuf> {
    let exe_name = if cfg!(windows) {
        "fm-bridge-odbc.exe"
    } else {
        "fm-bridge-odbc"
    };
    let exe_name_32 = if cfg!(windows) {
        "fm-bridge-odbc-x86.exe"
    } else {
        "fm-bridge-odbc-x86"
    };

    let mut out = Vec::new();
    if let Some(explicit) = &cfg.sidecar {
        out.push(PathBuf::from(explicit));
    }
    if let Some(env) = std::env::var_os("FMBRIDGE_ODBC_SIDECAR") {
        out.push(PathBuf::from(env));
    }
    // Next to the running binary — how the packaged extension ships it.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            out.push(dir.join(exe_name_32));
            out.push(dir.join(exe_name));
        }
    }
    // Development layout.
    for profile in ["debug", "release"] {
        out.push(PathBuf::from("target").join(profile).join(exe_name));
    }
    out.retain(|p| p.is_file());
    out.dedup();
    out
}

/// Run one statement against one configured database.
///
/// `sql` must already have passed `data_sql::sanitize_select`.
pub fn execute_sql(cfg: &DataConfig, database: &str, sql: &str) -> Result<Value, String> {
    let db = cfg.database(database)?;
    let srv = cfg.server(&db.server)?;
    let password = resolve_password(&srv.name)?;
    let cs = connection_string(srv, db.odbc_name(), &password);
    let limits = srv.limits(&cfg.limits);

    let request = json!({
        "connection_string": cs,
        "sql": sql,
        "connect_timeout_s": limits.connect_timeout_s,
        "max_rows": limits.max_rows,
        "max_cell_chars": limits.max_cell_chars,
        "max_total_chars": limits.max_total_chars,
    });
    let payload = serde_json::to_string(&request).map_err(|e| e.to_string())?;

    let candidates = sidecar_candidates(cfg);
    if candidates.is_empty() {
        return Err(missing_sidecar_message());
    }

    let _guard = QUERY_LOCK.lock().map_err(|_| {
        "The live-data lock was poisoned by an earlier panic; restart the server.".to_string()
    })?;

    let mut last: Option<(String, Option<String>)> = None;
    for (idx, sidecar) in candidates.iter().enumerate() {
        let result = call_sidecar(sidecar, &payload, limits.kill_timeout_s)?;
        if result.ok {
            let mut value = result.value;
            if let Some(obj) = value.as_object_mut() {
                obj.insert("database".to_string(), json!(db.name));
                obj.insert("sql".to_string(), json!(sql));
            }
            return Ok(value);
        }

        let message = result
            .value
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown sidecar error")
            .to_string();

        // A missing driver in *this* architecture is the one failure worth
        // retrying with the other build.
        let arch_retryable = result.sqlstate.as_deref() == Some("IM002");
        last = Some((message, result.sqlstate));
        if !(arch_retryable && idx + 1 < candidates.len()) {
            break;
        }
    }

    let (message, sqlstate) = last.unwrap_or_else(|| ("unknown error".to_string(), None));
    Err(explain(&message, sqlstate.as_deref()))
}

/// Spawn the sidecar, feed it the request, and enforce the kill deadline.
fn call_sidecar(
    sidecar: &PathBuf,
    payload: &str,
    kill_timeout_s: u64,
) -> Result<SidecarResult, String> {
    let mut child = Command::new(sidecar)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start ODBC sidecar {}: {}", sidecar.display(), e))?;

    // Write the request and close stdin so the child sees EOF and starts work.
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "sidecar stdin unavailable".to_string())?;
        stdin
            .write_all(payload.as_bytes())
            .map_err(|e| format!("cannot send request to sidecar: {}", e))?;
    }

    // Drain stdout/stderr on threads: a child that fills a pipe buffer while we
    // wait would deadlock against its own output.
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let out_thread = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(h) = stdout.as_mut() {
            let _ = h.read_to_string(&mut s);
        }
        s
    });
    let err_thread = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(h) = stderr.as_mut() {
            let _ = h.read_to_string(&mut s);
        }
        s
    });

    let deadline = Instant::now() + Duration::from_secs(kill_timeout_s);
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => break,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Ok(SidecarResult {
                        value: json!({
                            "status": "error",
                            "error": format!(
                                "The query exceeded the {}s wall-clock limit and the connection \
                                 was terminated. Nothing is left open on the server. Narrow the \
                                 query, or raise limits.kill_timeout_s if it is legitimately slow.",
                                kill_timeout_s
                            )
                        }),
                        sqlstate: Some("HYT00".to_string()),
                        ok: false,
                    });
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => return Err(format!("waiting for sidecar failed: {}", e)),
        }
    }

    let stdout_text = out_thread.join().unwrap_or_default();
    let stderr_text = err_thread.join().unwrap_or_default();

    let parsed: Value = serde_json::from_str(stdout_text.trim()).map_err(|_| {
        format!(
            "The ODBC sidecar returned something unreadable.\nstdout: {}\nstderr: {}",
            truncate(&stdout_text, 400),
            truncate(&stderr_text, 400)
        )
    })?;

    let ok = parsed.get("status").and_then(|v| v.as_str()) == Some("ok");
    let sqlstate = parsed
        .get("sqlstate")
        .and_then(|v| v.as_str())
        .map(String::from);
    Ok(SidecarResult {
        value: parsed,
        sqlstate,
        ok,
    })
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.trim().to_string()
    } else {
        s.chars().take(n).collect::<String>() + "…"
    }
}

fn missing_sidecar_message() -> String {
    "The ODBC sidecar (fm-bridge-odbc) was not found, so live queries are unavailable. \
     Schema tools that read a FMSaveAsXML export keep working. Build it with \
     `cargo build --release -p fm-bridge-odbc` and place it next to the fm-bridge binary, \
     or set `sidecar` in .fm-bridge.toml."
        .to_string()
}

/// Turn a driver diagnostic into something a FileMaker developer can act on.
///
/// This is the difference between a feature people use and one they abandon:
/// the raw text for a missing driver is "Data source name not found and no
/// default driver specified", which says nothing about what to install.
fn explain(message: &str, sqlstate: Option<&str>) -> String {
    // FileMaker reports SQL-level problems as its own FQL codes under the
    // catch-all SQLSTATE HY000, so the standard 42S02/42S22 states never
    // arrive from a FileMaker server. Check the FQL codes first; the SQLSTATE
    // table below still covers connection/driver failures and other drivers.
    if let Some(hint) = filemaker_hint(message) {
        return format!("{}\n\n{}", message, hint);
    }
    let hint = match sqlstate {
        Some("IM002") | Some("IM003") => Some(
            "The ODBC driver manager could not load a FileMaker driver.\n\
             1. Install the Claris/FileMaker ODBC client driver (a free download from Claris; \
                it is not bundled with fm-bridge).\n\
             2. If it is already installed, this is almost certainly an architecture mismatch: \
                FileMaker Pro commonly registers the 32-bit driver only, and a 64-bit process \
                cannot load it. Build the 32-bit sidecar and name it fm-bridge-odbc-x86.",
        ),
        Some("28000") => Some(
            "The server rejected the account. Check the user and password, and make sure the \
             account's privilege set has the extended privilege 'Access via ODBC/JDBC' (fmxdbc) \
             in File ▸ Manage ▸ Security.",
        ),
        Some("08001") | Some("08S01") | Some("08004") => Some(
            "Could not reach the database. Check the host, that the FileMaker file is hosted and \
             open, and that File ▸ Sharing ▸ Enable ODBC/JDBC is on for it (TCP port 2399).",
        ),
        Some("HYT00") | Some("HYT01") => Some(
            "The query timed out. Unstored calculation fields are very slow over ODBC — prefer \
             stored fields, or narrow the query. Raise limits.kill_timeout_s if it is \
             legitimately slow.",
        ),
        Some("42S02") => Some(
            "No such table. In FileMaker SQL the name in FROM is a *table occurrence*, not a base \
             table — use describe_database on the matching XML export to get the right name.",
        ),
        Some("42S22") => Some(
            "No such column. Field names are per table occurrence; get_table on the XML export \
             lists them.",
        ),
        _ => None,
    };
    match hint {
        Some(h) => format!("{}\n\n{}", message, h),
        None => message.to_string(),
    }
}

/// Hints keyed on FileMaker's own `FQL####` error codes, which arrive inside
/// the message rather than as a SQLSTATE.
fn filemaker_hint(message: &str) -> Option<&'static str> {
    if message.contains("FQL0002") {
        return Some(
            "No such table. In FileMaker SQL the name in FROM is a *table occurrence* (the box in \
             the Relationships graph), not a base table, and it is case-sensitive. Use \
             describe_database on this database's XML export, or query FileMaker_Tables, to get \
             the exact name.",
        );
    }
    if message.contains("FQL0007") {
        return Some(
            "No such column. Field names are resolved through the table occurrence in FROM. Use \
             get_table on the XML export, or query FileMaker_Fields, to list the real names. \
             Remember that a field shown on a layout may live in a related table.",
        );
    }
    if message.contains("FQL0001") {
        return Some(
            "Syntax error. FileMaker SQL is not the dialect you may expect: row limits are \
             'FETCH FIRST n ROWS ONLY' (never LIMIT), identifiers containing '_', spaces or \
             accents must be double-quoted, and string literals use single quotes.",
        );
    }
    None
}

/// Structured read: the default surface, where there is no SQL string for the
/// caller to assemble and therefore nothing to inject.
pub fn query_table(
    cfg: &DataConfig,
    database: &str,
    table: &str,
    fields: Option<&[String]>,
    filter: Option<&str>,
    order_by: Option<&str>,
    limit: Option<usize>,
) -> Result<Value, String> {
    let max = limit
        .unwrap_or(cfg.limits.max_rows)
        .min(cfg.limits.max_rows);
    let sql = data_sql::build_select(table, fields, filter, order_by, max)?;
    execute_sql(cfg, database, &sql)
}

pub fn count_rows(
    cfg: &DataConfig,
    database: &str,
    table: &str,
    filter: Option<&str>,
) -> Result<Value, String> {
    let sql = data_sql::build_count(table, filter)?;
    execute_sql(cfg, database, &sql)
}

/// Free-form read. Still a read: validated as a single SELECT before it leaves.
pub fn query_sql(cfg: &DataConfig, database: &str, sql: &str) -> Result<Value, String> {
    let checked = data_sql::sanitize_select(sql, cfg.limits.max_rows)?;
    execute_sql(cfg, database, &checked)
}

/// What the workspace knows about, without connecting to anything.
pub fn list_databases(cfg: &DataConfig) -> Value {
    let dbs: Vec<Value> = cfg
        .database
        .iter()
        .map(|d| {
            let effective = cfg.server(&d.server).ok().map(|s| s.limits(&cfg.limits));
            json!({
                "name": d.name,
                "server": d.server,
                "odbc_name": d.odbc_name(),
                "xml": cfg.xml_for(d).map(|p| p.display().to_string()),
                "connect_timeout_s": effective.as_ref().map(|l| l.connect_timeout_s),
                "kill_timeout_s": effective.as_ref().map(|l| l.kill_timeout_s),
            })
        })
        .collect();
    json!({
        "config": cfg.root.join(crate::data_config::CONFIG_FILE).display().to_string(),
        "databases": dbs,
        "limits": {
            "max_rows": cfg.limits.max_rows,
            "connect_timeout_s": cfg.limits.connect_timeout_s,
            "kill_timeout_s": cfg.limits.kill_timeout_s,
            "max_total_chars": cfg.limits.max_total_chars,
        }
    })
}

/// Preflight check, in the order things actually break. Stops at the first
/// failure with the fix attached, so the user is never staring at a SQLSTATE.
pub fn doctor(cfg: &DataConfig, database: Option<&str>) -> Value {
    let mut checks: Vec<Value> = Vec::new();
    let mut push = |name: &str, ok: bool, detail: String| {
        checks.push(json!({ "check": name, "ok": ok, "detail": detail }));
    };

    let candidates = sidecar_candidates(cfg);
    if candidates.is_empty() {
        push("odbc_sidecar", false, missing_sidecar_message());
        return json!({ "ok": false, "checks": checks });
    }
    push(
        "odbc_sidecar",
        true,
        format!(
            "Found: {}",
            candidates
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    );

    let targets: Vec<&crate::data_config::DatabaseConfig> = match database {
        Some(name) => match cfg.database(name) {
            Ok(d) => vec![d],
            Err(e) => {
                push("database", false, e);
                return json!({ "ok": false, "checks": checks });
            }
        },
        None => cfg.database.iter().collect(),
    };
    if targets.is_empty() {
        push(
            "database",
            false,
            format!(
                "No [[database]] entries in {}.",
                crate::data_config::CONFIG_FILE
            ),
        );
        return json!({ "ok": false, "checks": checks });
    }

    let mut all_ok = true;
    for db in targets {
        let label = format!("connect:{}", db.name);
        match resolve_password(&db.server) {
            Ok(_) => {}
            Err(e) => {
                all_ok = false;
                push(&label, false, e);
                continue;
            }
        }
        // The system catalog is the cheapest statement that proves the whole
        // chain works: driver loaded, host reachable, account accepted,
        // ODBC/JDBC sharing enabled on the file.
        match execute_sql(
            cfg,
            &db.name,
            "SELECT TableName FROM FileMaker_Tables FETCH FIRST 1 ROWS ONLY",
        ) {
            Ok(v) => {
                let ms = v.get("elapsed_ms").and_then(|m| m.as_u64()).unwrap_or(0);
                push(&label, true, format!("Connected and queried in {} ms.", ms));
            }
            Err(e) => {
                all_ok = false;
                push(&label, false, e);
            }
        }
    }

    json!({ "ok": all_ok, "checks": checks })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_driver_explains_the_bitness_trap() {
        let msg = explain("ERROR [IM002] Data source name not found", Some("IM002"));
        assert!(msg.contains("32-bit"), "{}", msg);
        assert!(msg.contains("Claris"), "{}", msg);
    }

    #[test]
    fn auth_failure_points_at_the_extended_privilege() {
        let msg = explain("rejected", Some("28000"));
        assert!(msg.contains("fmxdbc"), "{}", msg);
    }

    #[test]
    fn table_not_found_explains_table_occurrences() {
        let msg = explain("nope", Some("42S02"));
        assert!(msg.contains("table occurrence"), "{}", msg);
    }

    /// The shape a real FileMaker server returns: SQLSTATE HY000 with the useful
    /// detail hidden in an FQL code. Keyed off a message captured live.
    #[test]
    fn filemaker_fql_codes_win_over_the_generic_sqlstate() {
        let msg = explain(
            "execute: ODBC emitted an error calling 'SQLExecDirect':\nState: HY000, Native error: \
             8309, Message: [FileMaker][FileMaker] FQL0002/(1:14): The table named \"NoExiste\" \
             does not exist.",
            Some("HY000"),
        );
        assert!(msg.contains("table occurrence"), "{}", msg);

        let col = explain(
            "State: HY000, Native error: 8309, Message: [FileMaker][FileMaker] FQL0007/(1:56): \
             The column named \"Cantidad\" does not exist in any table in the column reference's \
             scope.",
            Some("HY000"),
        );
        assert!(col.contains("FileMaker_Fields"), "{}", col);
    }

    #[test]
    fn an_unmapped_sqlstate_passes_the_message_through_unchanged() {
        assert_eq!(
            explain("weird driver noise", Some("ZZZZZ")),
            "weird driver noise"
        );
        assert_eq!(explain("no sqlstate", None), "no sqlstate");
    }

    #[test]
    fn truncate_keeps_short_strings_whole() {
        assert_eq!(truncate("  hi  ", 10), "hi");
        assert_eq!(truncate("abcdef", 3), "abc…");
    }
}
