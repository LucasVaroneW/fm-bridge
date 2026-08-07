// Workspace configuration for the live-data path (`.fm-bridge.toml`).
//
// This is the bridge between the two halves of the engine: it maps a
// FMSaveAsXML export (dead structure) to the live database it was exported
// from (rows without meaning). With that mapping the engine can resolve table
// occurrences, warn that a field is an unstored calculation *before* querying
// it, and generate integrity SQL from relationship predicates — none of which
// either half can do alone.
//
// Nothing here is specific to any organisation: servers, databases and file
// names all come from the user's own project file.
//
// Secrets never live in this file. It is meant to be committed and shared with
// a team, so a `password` key is rejected outright; passwords come from the
// environment or from a credentials file in the OS config directory.

use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const CONFIG_FILE: &str = ".fm-bridge.toml";

#[derive(Debug, Deserialize)]
pub struct DataConfig {
    #[serde(default)]
    pub server: Vec<ServerConfig>,
    #[serde(default)]
    pub database: Vec<DatabaseConfig>,
    #[serde(default)]
    pub limits: Limits,
    /// Explicit sidecar path. Normally auto-detected next to the binary.
    #[serde(default)]
    pub sidecar: Option<String>,
    /// Absolute path this config was loaded from; relative paths in it
    /// (notably `xml`) resolve against its directory.
    #[serde(skip)]
    pub root: PathBuf,
}

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    pub name: String,
    pub host: String,
    pub user: String,
    /// ODBC driver name as registered on this machine. Defaults to the name
    /// Claris registers ("FileMaker ODBC"); override for a renamed install.
    #[serde(default)]
    pub driver: Option<String>,
    /// Per-server overrides of `[limits]`.
    ///
    /// Servers are not equal: an old FileMaker Server on a busy LAN can take
    /// half a minute just to accept a connection, while a modern one answers in
    /// under a second. One global timeout therefore has to be either too slow to
    /// protect the fast server or too tight for the slow one. These let a single
    /// workspace hold both without compromise.
    #[serde(default)]
    pub connect_timeout_s: Option<u32>,
    #[serde(default)]
    pub kill_timeout_s: Option<u64>,
    /// Rejected on load — present only so we can produce a good error.
    #[serde(default)]
    password: Option<String>,
}

impl ServerConfig {
    /// This server's effective limits: its own overrides on top of `[limits]`.
    pub fn limits(&self, base: &Limits) -> Limits {
        Limits {
            connect_timeout_s: self.connect_timeout_s.unwrap_or(base.connect_timeout_s),
            kill_timeout_s: self.kill_timeout_s.unwrap_or(base.kill_timeout_s),
            max_rows: base.max_rows,
            max_cell_chars: base.max_cell_chars,
            max_total_chars: base.max_total_chars,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct DatabaseConfig {
    /// Logical name the tools use.
    pub name: String,
    /// Which `[[server]]` this lives on.
    pub server: String,
    /// The FileMaker file name as ODBC exposes it. Defaults to `name`.
    #[serde(default)]
    pub odbc: Option<String>,
    /// Optional FMSaveAsXML export for this same database — the mapping that
    /// lets schema knowledge inform live queries.
    #[serde(default)]
    pub xml: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Limits {
    pub max_rows: usize,
    pub connect_timeout_s: u32,
    /// Wall-clock deadline after which the parent KILLS the sidecar.
    ///
    /// This is the *only* query deadline, deliberately: a statement-level
    /// timeout is honoured by the driver, and a driver stuck mid-query is the
    /// one case where that cannot be relied on. Killing the process always
    /// works, and takes the connection down with it.
    pub kill_timeout_s: u64,
    pub max_cell_chars: usize,
    /// Budget for the whole result set. The row and cell caps do not bound the
    /// payload on their own — FileMaker table occurrences routinely carry 60+
    /// columns — so this is what actually keeps a `SELECT *` from flooding the
    /// caller's context window.
    #[serde(default = "default_max_total_chars")]
    pub max_total_chars: usize,
}

fn default_max_total_chars() -> usize {
    20_000
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_rows: 500,
            connect_timeout_s: 15,
            kill_timeout_s: 45,
            max_cell_chars: 500,
            max_total_chars: default_max_total_chars(),
        }
    }
}

impl DataConfig {
    /// Find and load `.fm-bridge.toml`, walking up from `start` (or the current
    /// directory) to the filesystem root — the usual "project file" search, so
    /// tools work from any subdirectory of the workspace.
    pub fn discover(start: Option<&str>) -> Result<DataConfig, String> {
        // An explicit pointer wins over any search. This is what makes the MCP
        // path reliable: the AI client launches the server from its own
        // directory, which is never the user's project, so the config is told
        // rather than guessed.
        if start.is_none() {
            if let Some(env) = std::env::var_os("FMBRIDGE_CONFIG") {
                let p = PathBuf::from(env);
                if !p.as_os_str().is_empty() {
                    let file = if p.is_dir() { p.join(CONFIG_FILE) } else { p };
                    if !file.is_file() {
                        return Err(format!(
                            "FMBRIDGE_CONFIG points at {}, which does not exist.",
                            file.display()
                        ));
                    }
                    return DataConfig::load(&file);
                }
            }
        }
        let begin = match start {
            Some(s) => PathBuf::from(s),
            None => std::env::current_dir()
                .map_err(|e| format!("cannot read current directory: {}", e))?,
        };
        let begin = if begin.is_file() {
            begin.parent().map(Path::to_path_buf).unwrap_or(begin)
        } else {
            begin
        };

        let mut dir = begin.as_path();
        loop {
            let candidate = dir.join(CONFIG_FILE);
            if candidate.is_file() {
                return DataConfig::load(&candidate);
            }
            match dir.parent() {
                Some(p) => dir = p,
                None => break,
            }
        }
        // Worth being explicit: an MCP server is started by the AI client, so
        // its working directory is usually NOT the user's project. Without this
        // nudge the caller sees "not found" and concludes the feature is broken,
        // when the file exists one `config_path` away.
        Err(format!(
            "No {} found in {} or any parent directory.\n\n\
             If the project is somewhere else, pass `config_path` with the project \
             folder — the working directory of an MCP server is not the user's \
             workspace. If no connection has been set up yet, the user can run \
             \"fm-bridge: Connect to a database (live data)…\" in VS Code, or see \
             docs/DATA.md.",
            CONFIG_FILE,
            begin.display()
        ))
    }

    pub fn load(path: &Path) -> Result<DataConfig, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
        let mut cfg: DataConfig =
            toml::from_str(&text).map_err(|e| format!("cannot parse {}: {}", path.display(), e))?;
        cfg.root = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        cfg.validate(path)?;
        Ok(cfg)
    }

    fn validate(&self, path: &Path) -> Result<(), String> {
        for s in &self.server {
            if s.password.is_some() {
                return Err(format!(
                    "{}: server '{}' has a `password` key. Passwords must not be stored in \
                     this file — it is meant to be shared/committed. Use the environment \
                     variable {} or run `fm-bridge data login {}`.",
                    path.display(),
                    s.name,
                    password_env_var(&s.name),
                    s.name
                ));
            }
        }
        for d in &self.database {
            if !self.server.iter().any(|s| s.name == d.server) {
                return Err(format!(
                    "{}: database '{}' refers to server '{}', which is not defined.",
                    path.display(),
                    d.name,
                    d.server
                ));
            }
        }
        for s in &self.server {
            let l = s.limits(&self.limits);
            if l.kill_timeout_s <= l.connect_timeout_s as u64 {
                return Err(format!(
                    "{}: server '{}' ends up with kill_timeout_s ({}) <= connect_timeout_s ({}); \
                     the connection would be killed before it could be established.",
                    path.display(),
                    s.name,
                    l.kill_timeout_s,
                    l.connect_timeout_s
                ));
            }
        }
        if self.limits.kill_timeout_s <= self.limits.connect_timeout_s as u64 {
            return Err(format!(
                "{}: limits.kill_timeout_s ({}) must be greater than limits.connect_timeout_s ({}); \
                 otherwise a slow connection is killed before it can even be established.",
                path.display(),
                self.limits.kill_timeout_s,
                self.limits.connect_timeout_s
            ));
        }
        Ok(())
    }

    /// Look up a database by its logical name (case-insensitive).
    pub fn database(&self, name: &str) -> Result<&DatabaseConfig, String> {
        self.database
            .iter()
            .find(|d| d.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| {
                let known: Vec<&str> = self.database.iter().map(|d| d.name.as_str()).collect();
                if known.is_empty() {
                    format!(
                        "No databases configured in {}. Add a [[database]] entry.",
                        CONFIG_FILE
                    )
                } else {
                    format!(
                        "Unknown database '{}'. Configured: {}.",
                        name,
                        known.join(", ")
                    )
                }
            })
    }

    pub fn server(&self, name: &str) -> Result<&ServerConfig, String> {
        self.server
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| format!("Unknown server '{}'.", name))
    }

    /// Absolute path of the FMSaveAsXML export mapped to a database, if any.
    pub fn xml_for(&self, db: &DatabaseConfig) -> Option<PathBuf> {
        db.xml.as_ref().map(|x| {
            let p = PathBuf::from(x);
            if p.is_absolute() {
                p
            } else {
                self.root.join(p)
            }
        })
    }
}

impl DatabaseConfig {
    /// The name to put in the ODBC connection string.
    pub fn odbc_name(&self) -> &str {
        self.odbc.as_deref().unwrap_or(&self.name)
    }
}

/// `FMBRIDGE_PASSWORD_<SERVER>`, upper-cased with non-alphanumerics as `_`.
pub fn password_env_var(server: &str) -> String {
    let sanitized: String = server
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("FMBRIDGE_PASSWORD_{}", sanitized)
}

/// Per-user credentials file, outside any project directory so it is never
/// committed by accident.
pub fn credentials_path() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|h| h.join("Library").join("Application Support"))
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    };
    base.map(|b| b.join("fm-bridge").join("credentials.toml"))
}

/// Resolve a server password: environment first (handy for CI and for shells
/// that already export it), then the per-user credentials file.
pub fn resolve_password(server: &str) -> Result<String, String> {
    if let Some(v) = std::env::var_os(password_env_var(server)) {
        let s = v.to_string_lossy().into_owned();
        if !s.is_empty() {
            return Ok(s);
        }
    }
    if let Some(path) = credentials_path() {
        if path.is_file() {
            let text = std::fs::read_to_string(&path)
                .map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
            let parsed: toml::Value = toml::from_str(&text)
                .map_err(|e| format!("cannot parse {}: {}", path.display(), e))?;
            // Match the server key case-insensitively, like everything else.
            if let Some(table) = parsed.as_table() {
                for (k, v) in table {
                    if k.eq_ignore_ascii_case(server) {
                        if let Some(pw) = v.get("password").and_then(|p| p.as_str()) {
                            return Ok(pw.to_string());
                        }
                    }
                }
            }
        }
    }
    Err(format!(
        "No password for server '{}'. Set {} or run `fm-bridge data login {}`.",
        server,
        password_env_var(server),
        server
    ))
}

/// Build a DSN-less ODBC connection string. DSN-less on purpose: the user
/// never has to open the ODBC Data Source Administrator or know what a DSN is.
pub fn connection_string(srv: &ServerConfig, database: &str, password: &str) -> String {
    // Braces around values that may contain spaces or `;`.
    format!(
        "Driver={{{}}};Server={};Database={};UID={};PWD={};",
        srv.driver.as_deref().unwrap_or("FileMaker ODBC"),
        srv.host,
        database,
        srv.user,
        password
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, body: &str) -> PathBuf {
        let p = dir.join(CONFIG_FILE);
        std::fs::write(&p, body).unwrap();
        p
    }

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fmbridge-cfg-{}-{:?}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn loads_servers_and_databases() {
        let d = tmpdir("ok");
        let p = write(
            &d,
            r#"
[[server]]
name = "prod"
host = "10.0.0.5"
user = "reader"

[[database]]
name = "Stock"
server = "prod"
odbc = "Warehouse_Stock"
xml = "fm/Stock.xml"
"#,
        );
        let cfg = DataConfig::load(&p).unwrap();
        let db = cfg.database("stock").unwrap();
        assert_eq!(db.odbc_name(), "Warehouse_Stock");
        assert_eq!(cfg.server(&db.server).unwrap().host, "10.0.0.5");
        assert!(cfg.xml_for(db).unwrap().ends_with("fm/Stock.xml"));
        // Defaults apply when [limits] is absent.
        assert_eq!(cfg.limits.max_rows, 500);
    }

    #[test]
    fn password_in_project_file_is_rejected() {
        let d = tmpdir("pw");
        let p = write(
            &d,
            r#"
[[server]]
name = "prod"
host = "h"
user = "u"
password = "secret"
"#,
        );
        let err = DataConfig::load(&p).unwrap_err();
        assert!(err.contains("must not be stored"), "{}", err);
        assert!(err.contains("FMBRIDGE_PASSWORD_PROD"), "{}", err);
    }

    #[test]
    fn database_pointing_at_unknown_server_is_rejected() {
        let d = tmpdir("srv");
        let p = write(
            &d,
            r#"
[[database]]
name = "Stock"
server = "ghost"
"#,
        );
        assert!(DataConfig::load(&p).unwrap_err().contains("not defined"));
    }

    #[test]
    fn a_server_can_override_the_global_timeouts() {
        let d = tmpdir("perserver");
        let p = write(
            &d,
            r#"
[[server]]
name = "fast"
host = "h1"
user = "u"

[[server]]
name = "old"
host = "h2"
user = "u"
connect_timeout_s = 60
kill_timeout_s = 180

[limits]
max_rows = 500
connect_timeout_s = 15
kill_timeout_s = 45
max_cell_chars = 500
"#,
        );
        let cfg = DataConfig::load(&p).unwrap();
        let fast = cfg.server("fast").unwrap().limits(&cfg.limits);
        assert_eq!(fast.connect_timeout_s, 15);
        assert_eq!(fast.kill_timeout_s, 45);

        let old = cfg.server("old").unwrap().limits(&cfg.limits);
        assert_eq!(old.connect_timeout_s, 60);
        assert_eq!(old.kill_timeout_s, 180);
        // Non-timeout limits keep coming from the global block.
        assert_eq!(old.max_rows, 500);
    }

    #[test]
    fn kill_timeout_must_exceed_connect_timeout() {
        let d = tmpdir("to");
        let p = write(
            &d,
            r#"
[limits]
max_rows = 10
connect_timeout_s = 30
kill_timeout_s = 30
max_cell_chars = 100
"#,
        );
        assert!(DataConfig::load(&p).unwrap_err().contains("kill_timeout_s"));
    }

    #[test]
    fn discover_walks_up_from_a_subdirectory() {
        let d = tmpdir("walk");
        write(&d, "[[server]]\nname = \"s\"\nhost = \"h\"\nuser = \"u\"\n");
        let sub = d.join("a").join("b");
        std::fs::create_dir_all(&sub).unwrap();
        let cfg = DataConfig::discover(Some(sub.to_str().unwrap())).unwrap();
        assert_eq!(cfg.server.len(), 1);
    }

    #[test]
    fn env_var_name_is_sanitised() {
        assert_eq!(password_env_var("prod"), "FMBRIDGE_PASSWORD_PROD");
        assert_eq!(
            password_env_var("my server-1"),
            "FMBRIDGE_PASSWORD_MY_SERVER_1"
        );
    }

    #[test]
    fn connection_string_is_dsn_less() {
        let srv = ServerConfig {
            name: "p".into(),
            host: "10.0.0.5".into(),
            user: "reader".into(),
            driver: None,
            connect_timeout_s: None,
            kill_timeout_s: None,
            password: None,
        };
        let cs = connection_string(&srv, "Stock", "pw");
        assert!(cs.starts_with("Driver={FileMaker ODBC};"));
        assert!(cs.contains("Server=10.0.0.5;"));
        assert!(cs.contains("Database=Stock;"));
    }
}
