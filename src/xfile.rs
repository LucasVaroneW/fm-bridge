// Cross-file Perform Script / PSoS: fill in the target script's id before
// writing to the clipboard.
//
// Verified the hard way (FileMaker 22, PR #84 follow-up): on paste FileMaker
// resolves the external data source of a cross-file call BY NAME, but the
// script inside that other file only BY ID — a `<Script name="X">` without
// `id` pastes as <desconocido>. The id is the script's id in the TARGET file's
// own script catalog (the DDR confirms it: the cross-file step in
// By_00_Desk_4_Montaje carries `<Script id="1201">`, and 1201 is
// Api_StepsRRHHJT's id in By_22_Fabricacion's catalog).
//
// When the `.fmscript` gives only the name (`"Script" from file: "File"`), we
// look the id up in FMSaveAsXML exports:
//   1. the target file's own export (ScriptCatalog: name → id), found among the
//      `.fm-bridge.toml` databases (by name / odbc name / xml file stem) or the
//      exports passed explicitly;
//   2. failing that, any other known export that already calls that script
//      cross-file (`<DataSourceReference name=File><ScriptReference id name>`).
// Exports are tried newest first, since an old export misses new scripts.
// The data source id is NOT filled in: FileMaker resolves the file by name, and
// the id differs per calling file (we don't know which file you paste into).
//
// Anything we can't resolve becomes a warning naming the step, never a silent
// <desconocido>.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::data_config::DataConfig;
use crate::xmss::FmScript;

/// What resolution did, for the caller to report.
#[derive(Debug, Default)]
pub struct Outcome {
    /// Human lines: `paso 12: "Fab_Web_Escribir_Caja" → #1219 (By_22_Fabricacion.xml)`.
    pub resolved: Vec<String>,
    /// (1-based step number, message) for every cross-file call left without id.
    pub warnings: Vec<(usize, String)>,
}

/// Where to look for exports.
#[derive(Debug, Default)]
pub struct Sources {
    /// Explicit FMSaveAsXML exports (CLI `--xml`, JSON `xml_paths`).
    pub explicit: Vec<PathBuf>,
    /// Configured databases: (names it answers to, export path).
    pub configured: Vec<(Vec<String>, PathBuf)>,
}

impl Sources {
    /// Explicit exports plus the databases of the first `.fm-bridge.toml`
    /// found: `config` if given, else walking up from `start_dir`, else
    /// FMBRIDGE_CONFIG / the current directory. A missing config is not an
    /// error — resolution just has fewer places to look.
    pub fn discover(explicit: &[String], config: Option<&str>, start_dir: Option<&Path>) -> Self {
        let mut s = Sources {
            explicit: explicit.iter().map(PathBuf::from).collect(),
            configured: Vec::new(),
        };
        let mut attempts: Vec<Option<String>> = Vec::new();
        if let Some(c) = config {
            attempts.push(Some(c.to_string()));
        }
        if let Some(d) = start_dir {
            attempts.push(Some(d.to_string_lossy().to_string()));
        }
        attempts.push(None);
        for a in attempts {
            if let Ok(cfg) = DataConfig::discover(a.as_deref()) {
                s.add_config(&cfg);
                break;
            }
        }
        s
    }

    fn add_config(&mut self, cfg: &DataConfig) {
        for db in &cfg.database {
            if let Some(xml) = cfg.xml_for(db) {
                let mut names = vec![db.name.clone(), db.odbc_name().to_string()];
                if let Some(stem) = xml.file_stem() {
                    names.push(stem.to_string_lossy().to_string());
                }
                self.configured.push((names, xml));
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.explicit.is_empty() && self.configured.is_empty()
    }
}

/// A FileMaker file name for comparison: no case, no `.fmp12`, NFC.
fn norm_file(s: &str) -> String {
    let s = s.trim();
    let s = s
        .strip_suffix(".fmp12")
        .or_else(|| s.strip_suffix(".FMP12"))
        .unwrap_or(s);
    crate::normalization::to_nfc(s).to_lowercase()
}

/// FileMaker file names compare without case and without `.fmp12`.
fn same_file(a: &str, b: &str) -> bool {
    norm_file(a) == norm_file(b)
}

/// Lazily loaded export: its script catalog and its cross-file calls.
struct Export {
    label: String,
    catalog: HashMap<String, String>,
    cross: HashMap<(String, String), String>,
}

fn load_export(path: &Path) -> Option<Export> {
    let bytes = std::fs::read(path).ok()?;
    let text = if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        let u16s: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&u16s)
    } else {
        String::from_utf8_lossy(&bytes).into_owned()
    };
    Some(Export {
        label: path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        catalog: script_catalog(&text),
        cross: cross_file_calls(&text),
    })
}

fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let pat = format!(" {}=\"", name);
    let start = tag.find(&pat)? + pat.len();
    let end = tag[start..].find('"')?;
    Some(&tag[start..start + end])
}

fn unescape(s: &str) -> String {
    let s = s
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&");
    crate::normalization::to_nfc(&s).to_string()
}

/// Script name → id from the `<ScriptCatalog>` section of an FMSaveAsXML.
fn script_catalog(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Some(start) = text.find("<ScriptCatalog") else {
        return map;
    };
    let end = text[start..]
        .find("</ScriptCatalog>")
        .map(|p| start + p)
        .unwrap_or(text.len());
    let section = &text[start..end];
    let mut pos = 0;
    while let Some(rel) = section[pos..].find("<Script ") {
        let abs = pos + rel;
        let Some(close) = section[abs..].find('>') else {
            break;
        };
        let tag = &section[abs..abs + close];
        if let (Some(id), Some(name)) = (attr(tag, "id"), attr(tag, "name")) {
            map.entry(unescape(name)).or_insert_with(|| id.to_string());
        }
        pos = abs + close;
    }
    map
}

/// (file, script) → script id, from the cross-file calls an export already
/// makes: `<DataSourceReference name="F" …></DataSourceReference>` immediately
/// followed by `<ScriptReference id="N" name="S" …>` (Perform Script's
/// "From list").
fn cross_file_calls(text: &str) -> HashMap<(String, String), String> {
    let mut map = HashMap::new();
    let mut pos = 0;
    while let Some(rel) = text[pos..].find("<DataSourceReference ") {
        let abs = pos + rel;
        let Some(close) = text[abs..].find('>') else {
            break;
        };
        let ds_tag = &text[abs..abs + close];
        let mut rest = text[abs + close + 1..].trim_start();
        rest = rest
            .strip_prefix("</DataSourceReference>")
            .unwrap_or(rest)
            .trim_start();
        if rest.starts_with("<ScriptReference ")
            && let Some(end) = rest.find('>')
            && let sr = &rest[..end]
            && let (Some(file), Some(id), Some(name)) =
                (attr(ds_tag, "name"), attr(sr, "id"), attr(sr, "name"))
        {
            map.entry((norm_file(&unescape(file)), unescape(name)))
                .or_insert_with(|| id.to_string());
        }
        pos = abs + close;
    }
    map
}

/// Load (once) and borrow an export.
fn export<'a>(cache: &'a mut HashMap<PathBuf, Option<Export>>, p: &Path) -> Option<&'a Export> {
    cache
        .entry(p.to_path_buf())
        .or_insert_with(|| load_export(p))
        .as_ref()
}

fn newest_first(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut v: Vec<(std::time::SystemTime, PathBuf)> = paths
        .into_iter()
        .map(|p| {
            let t = std::fs::metadata(&p)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            (t, p)
        })
        .collect();
    v.sort_by_key(|(t, _)| std::cmp::Reverse(*t));
    let mut seen = std::collections::HashSet::new();
    v.into_iter()
        .map(|(_, p)| p)
        .filter(|p| seen.insert(p.clone()))
        .collect()
}

/// Fill `script_target_id` on cross-file Perform Script / PSoS steps that lack
/// it. Steps that already carry an id are left alone.
pub fn resolve(script: &mut FmScript, sources: &Sources) -> Outcome {
    let mut out = Outcome::default();
    let pending: Vec<usize> = script
        .steps
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            s.script_target_file.is_some()
                && s.script_target_name.is_some()
                && s.script_target_id.is_none()
        })
        .map(|(i, _)| i)
        .collect();
    if pending.is_empty() {
        return out;
    }

    let mut cache: HashMap<PathBuf, Option<Export>> = HashMap::new();

    for i in pending {
        let step = &script.steps[i];
        let file = step.script_target_file.clone().unwrap_or_default();
        let name = step.script_target_name.clone().unwrap_or_default();
        let key_name = crate::normalization::to_nfc(&name).to_string();
        let n = i + 1;

        // 1. The target file's own export(s).
        let mut targets: Vec<PathBuf> = sources
            .configured
            .iter()
            .filter(|(names, _)| names.iter().any(|x| same_file(x, &file)))
            .map(|(_, p)| p.clone())
            .collect();
        targets.extend(
            sources
                .explicit
                .iter()
                .filter(|p| {
                    p.file_stem()
                        .map(|s| same_file(&s.to_string_lossy(), &file))
                        .unwrap_or(false)
                })
                .cloned(),
        );
        let targets = newest_first(targets);
        let mut found: Option<(String, String)> = None;
        let mut looked: Vec<String> = Vec::new();
        for p in &targets {
            if let Some(e) = export(&mut cache, p) {
                looked.push(e.label.clone());
                if let Some(id) = e.catalog.get(&key_name) {
                    found = Some((id.clone(), e.label.clone()));
                    break;
                }
            }
        }

        // 2. Any other export that already calls it cross-file.
        if found.is_none() {
            let mut others: Vec<PathBuf> = sources.explicit.clone();
            others.extend(sources.configured.iter().map(|(_, p)| p.clone()));
            others.retain(|p| !targets.contains(p));
            for p in newest_first(others) {
                if let Some(e) = export(&mut cache, &p)
                    && let Some(id) = e.cross.get(&(norm_file(&file), key_name.clone()))
                {
                    found = Some((id.clone(), format!("{} (llamada existente)", e.label)));
                    break;
                }
            }
        }

        match found {
            Some((id, from)) => {
                out.resolved.push(format!(
                    "paso {}: \"{}\" from file: \"{}\" → #{} ({})",
                    n, name, file, id, from
                ));
                script.steps[i].script_target_id = Some(id);
            }
            None => {
                let why = if sources.is_empty() {
                    "no hay exports FMSaveAsXML a mano (ni .fm-bridge.toml ni --xml)".to_string()
                } else if looked.is_empty() {
                    format!("ningún export conocido es de \"{}\"", file)
                } else {
                    format!("no está en {} (¿export desactualizado?)", looked.join(", "))
                };
                out.warnings.push((
                    n,
                    format!(
                        "{} paso {}: \"{}\" from file: \"{}\" sin #id — {}. El guion quedará \
                         <desconocido> al pegar: agregá #id (\"{}\" #<id> from file: …) o pasá \
                         el export del archivo con --xml.",
                        script.steps[i].name, n, name, file, why, name
                    ),
                ));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str, content: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fmb_xfile_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, content).unwrap();
        p
    }

    const TARGET: &str = r#"<FMSaveAsXML><Structure><AddAction><ScriptCatalog membercount="2">
        <Script id="1219" name="Fab_Web_Escribir_Caja" UUID="A"></Script>
        <Script id="7" name="Con &amp; signo" UUID="B"></Script>
        </ScriptCatalog></AddAction></Structure></FMSaveAsXML>"#;

    const CALLER: &str = r#"<FMSaveAsXML><Step id="164" name="Perform Script on Server"><ParameterValues>
        <Parameter type="List"><List name="From list" value="1">
        <DataSourceReference id="84" name="By_22_Fabricacion" UUID="X"></DataSourceReference>
        <ScriptReference id="1219" name="Fab_Web_Escribir_Caja" UUID="Y"></ScriptReference>
        </List></Parameter></ParameterValues></Step></FMSaveAsXML>"#;

    fn script(text: &str) -> FmScript {
        crate::text_format::parse_text_to_script(text).unwrap()
    }

    #[test]
    fn resolves_from_target_export_and_writes_script_id() {
        let p = tmp("By_22_Fabricacion.xml", TARGET);
        let mut s = script(
            "Perform Script on Server [\"Fab_Web_Escribir_Caja\" from file: \"By_22_Fabricacion\"; $p; Wait for completion]",
        );
        let out = resolve(
            &mut s,
            &Sources {
                explicit: vec![p],
                configured: vec![],
            },
        );
        assert!(out.warnings.is_empty(), "{:?}", out.warnings);
        assert_eq!(s.steps[0].script_target_id.as_deref(), Some("1219"));
        let xml = crate::xmss::build_xml_from_script(&s).unwrap();
        assert!(
            xml.contains("<Script id=\"1219\" name=\"Fab_Web_Escribir_Caja\">"),
            "{}",
            xml
        );
        // The data source is resolved by name on paste: no id invented.
        assert!(
            xml.contains("<FileReference name=\"By_22_Fabricacion\">"),
            "{}",
            xml
        );
    }

    #[test]
    fn configured_database_matches_by_name_and_unescapes() {
        let p = tmp("export_fab.xml", TARGET);
        let mut s =
            script("Perform Script [\"Con & signo\" from file: \"by_22_fabricacion.fmp12\"]");
        resolve(
            &mut s,
            &Sources {
                explicit: vec![],
                configured: vec![(vec!["By_22_Fabricacion".into()], p)],
            },
        );
        assert_eq!(s.steps[0].script_target_id.as_deref(), Some("7"));
    }

    #[test]
    fn falls_back_to_an_existing_cross_file_call() {
        let p = tmp("By_00_1Launch.xml", CALLER);
        let mut s = script(
            "Perform Script on Server [\"Fab_Web_Escribir_Caja\" from file: \"By_22_Fabricacion\"]",
        );
        let out = resolve(
            &mut s,
            &Sources {
                explicit: vec![p],
                configured: vec![],
            },
        );
        assert_eq!(s.steps[0].script_target_id.as_deref(), Some("1219"));
        assert!(out.resolved[0].contains("#1219"), "{:?}", out.resolved);
    }

    #[test]
    fn unresolved_call_warns_naming_the_step() {
        let p = tmp(
            "By_22_Fabricacion_viejo.xml",
            "<FMSaveAsXML><ScriptCatalog></ScriptCatalog></FMSaveAsXML>",
        );
        let mut s =
            script("# c\nPerform Script [\"Nuevo\" from file: \"By_22_Fabricacion_viejo\"]");
        let out = resolve(
            &mut s,
            &Sources {
                explicit: vec![p],
                configured: vec![],
            },
        );
        assert!(s.steps[1].script_target_id.is_none());
        assert_eq!(out.warnings.len(), 1);
        assert_eq!(out.warnings[0].0, 2);
        assert!(
            out.warnings[0].1.contains("<desconocido>"),
            "{}",
            out.warnings[0].1
        );
        assert!(out.warnings[0].1.contains("paso 2"));
        // No sources at all: still a warning, not an error.
        let mut s = script("Perform Script [\"X\" from file: \"Otro\"]");
        let out = resolve(&mut s, &Sources::default());
        assert_eq!(out.warnings.len(), 1);
    }

    #[test]
    fn explicit_ids_and_local_calls_are_untouched() {
        let mut s =
            script("Perform Script [\"A\" #5 from file: \"F\"]\nPerform Script [\"Local\"]");
        let out = resolve(&mut s, &Sources::default());
        assert!(out.warnings.is_empty() && out.resolved.is_empty());
        assert_eq!(s.steps[0].script_target_id.as_deref(), Some("5"));
    }
}
