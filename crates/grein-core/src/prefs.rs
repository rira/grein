use crate::error::CoreError;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const KEYBINDING_NAMESPACE: &str = "keybinding";
const THEME_NAMESPACE: &str = "theme";
const THEME_NAME_KEY: &str = "name";
const UI_NAMESPACE: &str = "ui";
const SETTINGS_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SettingsFormat {
    Toml,
    Json,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsSnapshot {
    pub version: u32,
    pub preferences: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SectionPatch {
    ResetToDefault,
    Entries(BTreeMap<String, Option<String>>),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SettingsPatch {
    pub version: u32,
    pub preferences: BTreeMap<String, SectionPatch>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportReport {
    pub updated: usize,
    pub reset_keys: usize,
    pub reset_sections: usize,
}

fn non_empty_trimmed(val: Option<&str>) -> Option<&str> {
    val.map(str::trim).filter(|s| !s.is_empty())
}

pub fn resolve_prefs_db_path_from_env(
    config_db: Option<&str>,
    xdg_config_home: Option<&str>,
    home: Option<&str>,
) -> Option<PathBuf> {
    if let Some(path) = non_empty_trimmed(config_db) {
        return Some(PathBuf::from(path));
    }
    if let Some(xdg) = non_empty_trimmed(xdg_config_home) {
        return Some(PathBuf::from(xdg).join("grein").join("grein.db"));
    }
    if let Some(home) = non_empty_trimmed(home) {
        return Some(
            PathBuf::from(home)
                .join(".config")
                .join("grein")
                .join("grein.db"),
        );
    }
    None
}

pub fn default_prefs_db_path() -> Option<PathBuf> {
    let config_db = std::env::var("GREIN_CONFIG_DB").ok();
    let xdg_config_home = std::env::var("XDG_CONFIG_HOME").ok();
    let home = std::env::var("HOME").ok();
    resolve_prefs_db_path_from_env(
        config_db.as_deref(),
        xdg_config_home.as_deref(),
        home.as_deref(),
    )
}

#[derive(Debug)]
pub struct UserPrefsStore {
    conn: rusqlite::Connection,
}

impl UserPrefsStore {
    fn init_connection(conn: Connection, enable_wal: bool) -> Result<Self, CoreError> {
        if enable_wal {
            conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
        } else {
            conn.execute_batch("PRAGMA busy_timeout=5000;")?;
        }
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS preferences (
                namespace TEXT NOT NULL,
                key TEXT NOT NULL,
                value TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY (namespace, key)
            );",
        )?;
        Ok(Self { conn })
    }

    pub fn open(path: &Path) -> Result<Self, CoreError> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    CoreError::SyncError(format!(
                        "failed to create preferences directory '{}': {e}",
                        parent.display()
                    ))
                })?;
            }
        }
        let conn = Connection::open(path)?;
        Self::init_connection(conn, true)
    }

    pub fn open_default() -> Result<Option<Self>, CoreError> {
        match default_prefs_db_path() {
            Some(path) => Self::open(&path).map(Some),
            None => Ok(None),
        }
    }

    pub fn in_memory() -> Result<Self, CoreError> {
        let conn = Connection::open_in_memory()?;
        Self::init_connection(conn, false)
    }

    pub fn get(&self, namespace: &str, key: &str) -> Result<Option<String>, CoreError> {
        let value = self
            .conn
            .query_row(
                "SELECT value FROM preferences WHERE namespace = ?1 AND key = ?2",
                params![namespace, key],
                |row| row.get(0),
            )
            .optional()?;
        Ok(value)
    }

    pub fn set(&self, namespace: &str, key: &str, value: &str) -> Result<(), CoreError> {
        upsert_preference(&self.conn, namespace, key, value)
    }

    pub fn delete(&self, namespace: &str, key: &str) -> Result<bool, CoreError> {
        let affected = self.conn.execute(
            "DELETE FROM preferences WHERE namespace = ?1 AND key = ?2",
            params![namespace, key],
        )?;
        Ok(affected > 0)
    }

    pub fn list_namespace(&self, namespace: &str) -> Result<BTreeMap<String, String>, CoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT key, value FROM preferences WHERE namespace = ?1 ORDER BY key ASC")?;
        let rows = stmt.query_map(params![namespace], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut entries = BTreeMap::new();
        for row in rows {
            let (key, value) = row?;
            entries.insert(key, value);
        }
        Ok(entries)
    }

    fn clear_namespace(&self, namespace: &str) -> Result<usize, CoreError> {
        let affected = self.conn.execute(
            "DELETE FROM preferences WHERE namespace = ?1",
            params![namespace],
        )?;
        Ok(affected)
    }

    pub fn keybindings(&self) -> Result<BTreeMap<String, String>, CoreError> {
        self.list_namespace(KEYBINDING_NAMESPACE)
    }

    pub fn set_keybinding(&self, action: &str, binding: &str) -> Result<(), CoreError> {
        self.set(KEYBINDING_NAMESPACE, action, binding)
    }

    pub fn remove_keybinding(&self, action: &str) -> Result<bool, CoreError> {
        self.delete(KEYBINDING_NAMESPACE, action)
    }

    pub fn clear_keybindings(&self) -> Result<usize, CoreError> {
        self.clear_namespace(KEYBINDING_NAMESPACE)
    }

    pub fn theme(&self) -> Result<Option<String>, CoreError> {
        self.get(THEME_NAMESPACE, THEME_NAME_KEY)
    }

    pub fn set_theme(&self, name: &str) -> Result<(), CoreError> {
        self.set(THEME_NAMESPACE, THEME_NAME_KEY, name)
    }

    pub fn theme_prefs(&self) -> Result<BTreeMap<String, String>, CoreError> {
        self.list_namespace(THEME_NAMESPACE)
    }

    pub fn set_theme_pref(&self, key: &str, value: &str) -> Result<(), CoreError> {
        self.set(THEME_NAMESPACE, key, value)
    }

    pub fn clear_theme_prefs(&self) -> Result<usize, CoreError> {
        self.clear_namespace(THEME_NAMESPACE)
    }

    pub fn ui_prefs(&self) -> Result<BTreeMap<String, String>, CoreError> {
        self.list_namespace(UI_NAMESPACE)
    }

    pub fn set_ui_pref(&self, key: &str, value: &str) -> Result<(), CoreError> {
        self.set(UI_NAMESPACE, key, value)
    }

    pub fn clear_ui_prefs(&self) -> Result<usize, CoreError> {
        self.clear_namespace(UI_NAMESPACE)
    }

    pub fn export_snapshot(
        &self,
        only_sections: Option<&[String]>,
    ) -> Result<SettingsSnapshot, CoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT namespace, key, value FROM preferences ORDER BY namespace ASC, key ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut preferences: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        for row in rows {
            let (namespace, key, value) = row?;
            if !section_matches_filter(only_sections, &namespace) {
                continue;
            }
            preferences.entry(namespace).or_default().insert(key, value);
        }
        Ok(SettingsSnapshot {
            version: SETTINGS_VERSION,
            preferences,
        })
    }

    pub fn export_string(
        &self,
        format: SettingsFormat,
        only_sections: Option<&[String]>,
    ) -> Result<String, CoreError> {
        let snapshot = self.export_snapshot(only_sections)?;
        match format {
            SettingsFormat::Json => format_json_snapshot(&snapshot),
            SettingsFormat::Toml => {
                let mut out = format!("# grein configuration\nversion = {}\n", snapshot.version);
                for (section, entries) in &snapshot.preferences {
                    if entries.is_empty() {
                        continue;
                    }
                    out.push('\n');
                    format_toml_section(&mut out, section, entries);
                }
                Ok(out)
            }
        }
    }

    pub fn format_single_section(
        section: &str,
        entries: &BTreeMap<String, String>,
        format: SettingsFormat,
    ) -> Result<String, CoreError> {
        let section = section.trim();
        if section.is_empty() {
            return Err(CoreError::SyncError(
                "section name cannot be empty".to_string(),
            ));
        }
        match format {
            SettingsFormat::Json => {
                let mut preferences = BTreeMap::new();
                preferences.insert(section.to_string(), entries.clone());
                format_json_snapshot(&SettingsSnapshot {
                    version: SETTINGS_VERSION,
                    preferences,
                })
            }
            SettingsFormat::Toml => {
                let mut out = String::new();
                format_toml_section(&mut out, section, entries);
                Ok(out)
            }
        }
    }

    pub fn parse_settings_patch(
        raw: &str,
        format: Option<SettingsFormat>,
    ) -> Result<SettingsPatch, CoreError> {
        match detect_format(raw, format) {
            SettingsFormat::Json => {
                let root = parse_json_root(raw)?;
                parse_json_settings_root(&root)
            }
            SettingsFormat::Toml => {
                let doc = parse_toml_document(raw)?;
                let mut preferences = BTreeMap::new();
                for (key, val) in doc.root_entries {
                    if val.is_some() {
                        return Err(CoreError::SyncError(format!(
                            "unexpected top-level key '{key}' outside any [section]"
                        )));
                    }
                    preferences.insert(key, SectionPatch::ResetToDefault);
                }
                for (section, entries) in doc.sections {
                    let patch = if entries.is_empty() {
                        SectionPatch::ResetToDefault
                    } else {
                        SectionPatch::Entries(entries)
                    };
                    preferences.insert(section, patch);
                }
                Ok(SettingsPatch {
                    version: doc.version.unwrap_or(SETTINGS_VERSION),
                    preferences,
                })
            }
        }
    }

    pub fn parse_theme_patch(
        raw: &str,
        format: Option<SettingsFormat>,
    ) -> Result<SettingsPatch, CoreError> {
        match detect_format(raw, format) {
            SettingsFormat::Json => {
                let root = parse_json_root(raw)?;
                parse_json_theme_root(&root)
            }
            SettingsFormat::Toml => {
                let mut doc = parse_toml_document(raw)?;
                let version = doc.version.unwrap_or(SETTINGS_VERSION);
                let mut preferences = BTreeMap::new();

                if let Some(entries) = doc.sections.remove(THEME_NAMESPACE) {
                    ensure_no_root_scalars(&doc.root_entries)?;
                    let patch = if entries.is_empty() {
                        SectionPatch::ResetToDefault
                    } else {
                        SectionPatch::Entries(entries)
                    };
                    preferences.insert(THEME_NAMESPACE.to_string(), patch);
                    return Ok(SettingsPatch {
                        version,
                        preferences,
                    });
                }

                if matches!(doc.root_entries.get(THEME_NAMESPACE), Some(None)) {
                    ensure_no_root_scalars(&doc.root_entries)?;
                    preferences.insert(THEME_NAMESPACE.to_string(), SectionPatch::ResetToDefault);
                    return Ok(SettingsPatch {
                        version,
                        preferences,
                    });
                }

                if !doc.sections.is_empty() {
                    return Err(CoreError::SyncError(
                        "settings TOML does not contain a [theme] section".to_string(),
                    ));
                }

                if doc.root_entries.is_empty() {
                    return Err(CoreError::SyncError(
                        "theme TOML contains neither a [theme] section nor theme keys".to_string(),
                    ));
                }

                preferences.insert(
                    THEME_NAMESPACE.to_string(),
                    SectionPatch::Entries(doc.root_entries),
                );
                Ok(SettingsPatch {
                    version,
                    preferences,
                })
            }
        }
    }

    pub fn import_patch(
        &self,
        patch: &SettingsPatch,
        only_sections: Option<&[String]>,
        replace_all: bool,
    ) -> Result<ImportReport, CoreError> {
        let tx = self.conn.unchecked_transaction()?;

        if replace_all {
            match only_sections {
                Some(secs) => {
                    for ns in list_distinct_namespaces(&tx)? {
                        if section_matches_filter(Some(secs), &ns) {
                            tx.execute(
                                "DELETE FROM preferences WHERE namespace = ?1",
                                params![ns],
                            )?;
                        }
                    }
                }
                None => {
                    tx.execute("DELETE FROM preferences", [])?;
                }
            }
        }

        let mut report = ImportReport::default();
        for (section, sec_patch) in &patch.preferences {
            if !section_matches_filter(only_sections, section) {
                continue;
            }
            if section.trim().is_empty() {
                return Err(CoreError::SyncError(
                    "section name cannot be empty".to_string(),
                ));
            }
            match sec_patch {
                SectionPatch::ResetToDefault => {
                    tx.execute(
                        "DELETE FROM preferences WHERE namespace = ?1",
                        params![section],
                    )?;
                    report.reset_sections += 1;
                }
                SectionPatch::Entries(entries) => {
                    for (key, opt_val) in entries {
                        if key.trim().is_empty() {
                            return Err(CoreError::SyncError(format!(
                                "key cannot be empty in section '{section}'"
                            )));
                        }
                        match opt_val {
                            None => {
                                tx.execute(
                                    "DELETE FROM preferences WHERE namespace = ?1 AND key = ?2",
                                    params![section, key],
                                )?;
                                report.reset_keys += 1;
                            }
                            Some(value) => {
                                upsert_preference(&tx, section, key, value)?;
                                report.updated += 1;
                            }
                        }
                    }
                }
            }
        }

        tx.commit()?;
        Ok(report)
    }

    pub fn import_str(
        &self,
        raw: &str,
        format: Option<SettingsFormat>,
        only_sections: Option<&[String]>,
        replace_all: bool,
    ) -> Result<ImportReport, CoreError> {
        let patch = Self::parse_settings_patch(raw, format)?;
        self.import_patch(&patch, only_sections, replace_all)
    }

    pub fn import_theme_str(
        &self,
        raw: &str,
        format: Option<SettingsFormat>,
        replace_theme: bool,
    ) -> Result<ImportReport, CoreError> {
        let patch = Self::parse_theme_patch(raw, format)?;
        self.import_patch(&patch, Some(&[THEME_NAMESPACE.to_string()]), replace_theme)
    }
}

fn upsert_preference(
    conn: &Connection,
    namespace: &str,
    key: &str,
    value: &str,
) -> Result<(), CoreError> {
    let updated_at = chrono::Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO preferences (namespace, key, value, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(namespace, key) DO UPDATE SET
            value = excluded.value,
            updated_at = excluded.updated_at",
        params![namespace, key, value, updated_at],
    )?;
    Ok(())
}

fn list_distinct_namespaces(conn: &Connection) -> Result<Vec<String>, CoreError> {
    let mut stmt = conn.prepare("SELECT DISTINCT namespace FROM preferences")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut namespaces = Vec::new();
    for row in rows {
        namespaces.push(row?);
    }
    Ok(namespaces)
}

fn section_matches_filter(only_sections: Option<&[String]>, section: &str) -> bool {
    match only_sections {
        None => true,
        Some(secs) => secs.iter().any(|s| {
            let trimmed = s.trim();
            !trimmed.is_empty() && (trimmed == section || trimmed.eq_ignore_ascii_case(section))
        }),
    }
}

fn detect_format(raw: &str, format: Option<SettingsFormat>) -> SettingsFormat {
    if let Some(fmt) = format {
        return fmt;
    }
    let trimmed = raw.trim_start_matches('\u{feff}').trim_start();
    if trimmed.starts_with('{') {
        SettingsFormat::Json
    } else {
        SettingsFormat::Toml
    }
}

fn format_json_snapshot(snapshot: &SettingsSnapshot) -> Result<String, CoreError> {
    let mut json = serde_json::to_string_pretty(snapshot)?;
    json.push('\n');
    Ok(json)
}

fn is_bare_toml_key(key: &str) -> bool {
    !key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn escape_toml_string(val: &str) -> String {
    let mut out = String::with_capacity(val.len());
    for ch in val.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                use std::fmt::Write;
                let _ = write!(out, "\\u{:04X}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

fn format_toml_key(key: &str) -> String {
    if is_bare_toml_key(key) {
        key.to_string()
    } else {
        format!("\"{}\"", escape_toml_string(key))
    }
}

fn format_toml_section(out: &mut String, section: &str, entries: &BTreeMap<String, String>) {
    out.push('[');
    out.push_str(&format_toml_key(section));
    out.push_str("]\n");
    for (key, value) in entries {
        out.push_str(&format_toml_key(key));
        out.push_str(" = \"");
        out.push_str(&escape_toml_string(value));
        out.push_str("\"\n");
    }
}

fn parse_json_root(raw: &str) -> Result<serde_json::Map<String, Value>, CoreError> {
    let value: Value = serde_json::from_str(raw)?;
    match value {
        Value::Object(map) => Ok(map),
        _ => Err(CoreError::SyncError(
            "expected top-level JSON object".to_string(),
        )),
    }
}

fn parse_json_u32(val: &Value) -> Result<u32, CoreError> {
    val.as_u64()
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| {
            CoreError::SyncError(
                "invalid settings 'version': expected unsigned integer".to_string(),
            )
        })
}

fn parse_json_scalar_or_null(
    section: &str,
    key: &str,
    val: &Value,
) -> Result<Option<String>, CoreError> {
    match val {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(s.clone())),
        Value::Bool(b) => Ok(Some(b.to_string())),
        Value::Number(n) => Ok(Some(n.to_string())),
        Value::Array(_) | Value::Object(_) => Err(CoreError::SyncError(format!(
            "invalid value for '{section}.{key}': nested arrays and objects are not supported"
        ))),
    }
}

fn parse_json_section_patch(section: &str, val: &Value) -> Result<SectionPatch, CoreError> {
    if section.trim().is_empty() {
        return Err(CoreError::SyncError(
            "section name cannot be empty".to_string(),
        ));
    }
    match val {
        Value::Null => Ok(SectionPatch::ResetToDefault),
        Value::Object(obj) => {
            let mut entries = BTreeMap::new();
            for (key, item) in obj {
                if key.trim().is_empty() {
                    return Err(CoreError::SyncError(format!(
                        "key cannot be empty in section '{section}'"
                    )));
                }
                entries.insert(key.clone(), parse_json_scalar_or_null(section, key, item)?);
            }
            Ok(SectionPatch::Entries(entries))
        }
        _ => Err(CoreError::SyncError(format!(
            "invalid section '{section}': expected object or null"
        ))),
    }
}

fn parse_json_settings_root(
    root: &serde_json::Map<String, Value>,
) -> Result<SettingsPatch, CoreError> {
    if let Some(prefs_val) = root.get("preferences") {
        for key in root.keys() {
            if key != "preferences" && key != "version" {
                return Err(CoreError::SyncError(format!(
                    "unexpected top-level key '{key}' alongside 'preferences'"
                )));
            }
        }
        let version = root
            .get("version")
            .map(parse_json_u32)
            .transpose()?
            .unwrap_or(SETTINGS_VERSION);
        let prefs_obj = prefs_val.as_object().ok_or_else(|| {
            CoreError::SyncError("'preferences' must be a JSON object".to_string())
        })?;
        let mut preferences = BTreeMap::new();
        for (section, val) in prefs_obj {
            preferences.insert(section.clone(), parse_json_section_patch(section, val)?);
        }
        return Ok(SettingsPatch {
            version,
            preferences,
        });
    }

    let mut version = SETTINGS_VERSION;
    let mut preferences = BTreeMap::new();
    for (key, val) in root {
        if key == "version" {
            version = parse_json_u32(val)?;
            continue;
        }
        preferences.insert(key.clone(), parse_json_section_patch(key, val)?);
    }
    Ok(SettingsPatch {
        version,
        preferences,
    })
}

fn parse_json_theme_root(
    root: &serde_json::Map<String, Value>,
) -> Result<SettingsPatch, CoreError> {
    if root.contains_key("preferences") {
        let full = parse_json_settings_root(root)?;
        let theme_patch = full
            .preferences
            .get(THEME_NAMESPACE)
            .cloned()
            .ok_or_else(|| {
                CoreError::SyncError("settings JSON does not contain a 'theme' section".to_string())
            })?;
        return Ok(SettingsPatch {
            version: full.version,
            preferences: BTreeMap::from([(THEME_NAMESPACE.to_string(), theme_patch)]),
        });
    }

    let version = root
        .get("version")
        .map(parse_json_u32)
        .transpose()?
        .unwrap_or(SETTINGS_VERSION);

    if let Some(theme_val) = root.get(THEME_NAMESPACE) {
        if matches!(theme_val, Value::Object(_) | Value::Null) {
            let patch = parse_json_section_patch(THEME_NAMESPACE, theme_val)?;
            return Ok(SettingsPatch {
                version,
                preferences: BTreeMap::from([(THEME_NAMESPACE.to_string(), patch)]),
            });
        }
    }

    let mut entries = BTreeMap::new();
    for (key, val) in root {
        if key == "version" {
            continue;
        }
        if matches!(val, Value::Object(_)) {
            return Err(CoreError::SyncError(
                "settings JSON does not contain a 'theme' section".to_string(),
            ));
        }
        if key.trim().is_empty() {
            return Err(CoreError::SyncError(
                "theme key cannot be empty".to_string(),
            ));
        }
        entries.insert(
            key.clone(),
            parse_json_scalar_or_null(THEME_NAMESPACE, key, val)?,
        );
    }

    if entries.is_empty() {
        return Err(CoreError::SyncError(
            "theme JSON contains neither a 'theme' section nor theme keys".to_string(),
        ));
    }

    Ok(SettingsPatch {
        version,
        preferences: BTreeMap::from([(
            THEME_NAMESPACE.to_string(),
            SectionPatch::Entries(entries),
        )]),
    })
}

fn ensure_no_root_scalars(
    root_entries: &BTreeMap<String, Option<String>>,
) -> Result<(), CoreError> {
    for (key, val) in root_entries {
        if val.is_some() {
            return Err(CoreError::SyncError(format!(
                "unexpected top-level key '{key}' outside any [section]"
            )));
        }
    }
    Ok(())
}

struct ParsedToml {
    version: Option<u32>,
    root_entries: BTreeMap<String, Option<String>>,
    sections: BTreeMap<String, BTreeMap<String, Option<String>>>,
}

enum TomlVal {
    Null,
    Int(String),
    Scalar(String),
}

impl TomlVal {
    fn into_option_string(self) -> Option<String> {
        match self {
            TomlVal::Null => None,
            TomlVal::Int(s) | TomlVal::Scalar(s) => Some(s),
        }
    }
}

fn parse_toml_document(raw: &str) -> Result<ParsedToml, CoreError> {
    let mut doc = ParsedToml {
        version: None,
        root_entries: BTreeMap::new(),
        sections: BTreeMap::new(),
    };
    let mut current_section: Option<String> = None;

    for (idx, line) in raw.lines().enumerate() {
        let line_no = idx + 1;
        let pos = skip_ws(line, 0);
        if pos >= line.len() || line.as_bytes()[pos] == b'#' {
            continue;
        }

        if line.as_bytes()[pos] == b'[' {
            let section = parse_section_header(line, pos, line_no)?;
            if doc.root_entries.contains_key(&section) || doc.sections.contains_key(&section) {
                return Err(CoreError::SyncError(format!(
                    "line {line_no}: duplicate section '[{section}]'"
                )));
            }
            doc.sections.insert(section.clone(), BTreeMap::new());
            current_section = Some(section);
            continue;
        }

        let (key, next_pos) = parse_toml_key(line, pos, line_no)?;
        let eq_pos = skip_ws(line, next_pos);
        if line.as_bytes().get(eq_pos) != Some(&b'=') {
            return Err(CoreError::SyncError(format!(
                "line {line_no}: expected '=' after key '{key}'"
            )));
        }
        let val = parse_toml_value(line, eq_pos + 1, line_no)?;

        if let Some(sec_name) = current_section.as_deref() {
            let sec_map = doc.sections.get_mut(sec_name).ok_or_else(|| {
                CoreError::SyncError(format!("line {line_no}: missing active section"))
            })?;
            if sec_map.contains_key(&key) {
                return Err(CoreError::SyncError(format!(
                    "line {line_no}: duplicate key '{key}' in section '[{sec_name}]'"
                )));
            }
            sec_map.insert(key, val.into_option_string());
        } else if key == "version" {
            if doc.version.is_some() {
                return Err(CoreError::SyncError(format!(
                    "line {line_no}: duplicate 'version' key"
                )));
            }
            match val {
                TomlVal::Int(int_str) => {
                    let v = int_str.parse::<u32>().map_err(|_| {
                        CoreError::SyncError(format!(
                            "line {line_no}: invalid settings 'version': expected unsigned integer"
                        ))
                    })?;
                    doc.version = Some(v);
                }
                _ => {
                    return Err(CoreError::SyncError(format!(
                        "line {line_no}: invalid settings 'version': expected unsigned integer"
                    )));
                }
            }
        } else {
            if doc.root_entries.contains_key(&key) || doc.sections.contains_key(&key) {
                return Err(CoreError::SyncError(format!(
                    "line {line_no}: duplicate top-level key '{key}'"
                )));
            }
            doc.root_entries.insert(key, val.into_option_string());
        }
    }

    Ok(doc)
}

fn skip_ws(s: &str, mut pos: usize) -> usize {
    let bytes = s.as_bytes();
    while pos < bytes.len() && matches!(bytes[pos], b' ' | b'\t') {
        pos += 1;
    }
    pos
}

fn expect_eol_or_comment(s: &str, pos: usize, line_no: usize) -> Result<(), CoreError> {
    let pos = skip_ws(s, pos);
    if pos >= s.len() || s.as_bytes()[pos] == b'#' {
        Ok(())
    } else {
        Err(CoreError::SyncError(format!(
            "line {line_no}: unexpected trailing content '{}'",
            &s[pos..]
        )))
    }
}

fn parse_unicode_escape(
    s: &str,
    pos: usize,
    digits: usize,
    line_no: usize,
) -> Result<(char, usize), CoreError> {
    let hex = s.get(pos..pos + digits).ok_or_else(|| {
        CoreError::SyncError(format!(
            "line {line_no}: incomplete unicode escape sequence"
        ))
    })?;
    let code = u32::from_str_radix(hex, 16).map_err(|_| {
        CoreError::SyncError(format!("line {line_no}: invalid unicode escape '\\u{hex}'"))
    })?;
    let ch = char::from_u32(code).ok_or_else(|| {
        CoreError::SyncError(format!(
            "line {line_no}: invalid unicode scalar value 0x{code:X}"
        ))
    })?;
    Ok((ch, pos + digits))
}

fn parse_double_quoted(
    s: &str,
    start: usize,
    line_no: usize,
) -> Result<(String, usize), CoreError> {
    let mut pos = start + 1;
    let mut out = String::new();
    while pos < s.len() {
        let ch = s[pos..].chars().next().ok_or_else(|| {
            CoreError::SyncError(format!("line {line_no}: unterminated double-quoted string"))
        })?;
        match ch {
            '"' => return Ok((out, pos + 1)),
            '\\' => {
                pos += 1;
                let esc = s[pos..].chars().next().ok_or_else(|| {
                    CoreError::SyncError(format!(
                        "line {line_no}: unterminated escape sequence in double-quoted string"
                    ))
                })?;
                pos += esc.len_utf8();
                match esc {
                    '"' => out.push('"'),
                    '\\' => out.push('\\'),
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    'b' => out.push('\u{0008}'),
                    'f' => out.push('\u{000C}'),
                    'u' => {
                        let (uc, next) = parse_unicode_escape(s, pos, 4, line_no)?;
                        out.push(uc);
                        pos = next;
                    }
                    'U' => {
                        let (uc, next) = parse_unicode_escape(s, pos, 8, line_no)?;
                        out.push(uc);
                        pos = next;
                    }
                    other => {
                        return Err(CoreError::SyncError(format!(
                            "line {line_no}: invalid TOML escape '\\{other}'"
                        )));
                    }
                }
            }
            c if c.is_control() && c != '\t' => {
                return Err(CoreError::SyncError(format!(
                    "line {line_no}: unescaped control character in string"
                )));
            }
            c => {
                out.push(c);
                pos += c.len_utf8();
            }
        }
    }
    Err(CoreError::SyncError(format!(
        "line {line_no}: unterminated double-quoted string"
    )))
}

fn parse_single_quoted(
    s: &str,
    start: usize,
    line_no: usize,
) -> Result<(String, usize), CoreError> {
    let mut pos = start + 1;
    while pos < s.len() {
        let ch = s[pos..].chars().next().ok_or_else(|| {
            CoreError::SyncError(format!("line {line_no}: unterminated single-quoted string"))
        })?;
        if ch == '\'' {
            return Ok((s[start + 1..pos].to_string(), pos + 1));
        }
        if ch.is_control() && ch != '\t' {
            return Err(CoreError::SyncError(format!(
                "line {line_no}: unescaped control character in literal string"
            )));
        }
        pos += ch.len_utf8();
    }
    Err(CoreError::SyncError(format!(
        "line {line_no}: unterminated single-quoted string"
    )))
}

fn is_bare_toml_parse_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')
}

fn parse_toml_key(s: &str, start: usize, line_no: usize) -> Result<(String, usize), CoreError> {
    let (key, next_pos) = match s.as_bytes().get(start) {
        Some(b'"') => parse_double_quoted(s, start, line_no)?,
        Some(b'\'') => parse_single_quoted(s, start, line_no)?,
        Some(_) => {
            let mut pos = start;
            let bytes = s.as_bytes();
            while pos < bytes.len() && is_bare_toml_parse_byte(bytes[pos]) {
                pos += 1;
            }
            if pos == start {
                return Err(CoreError::SyncError(format!(
                    "line {line_no}: expected key before '{}'",
                    &s[start..]
                )));
            }
            (s[start..pos].to_string(), pos)
        }
        None => {
            return Err(CoreError::SyncError(format!(
                "line {line_no}: expected key"
            )));
        }
    };
    if key.trim().is_empty() {
        return Err(CoreError::SyncError(format!(
            "line {line_no}: key cannot be empty"
        )));
    }
    Ok((key, next_pos))
}

fn parse_section_header(s: &str, start: usize, line_no: usize) -> Result<String, CoreError> {
    if s.as_bytes().get(start + 1) == Some(&b'[') {
        return Err(CoreError::SyncError(format!(
            "line {line_no}: TOML array tables '[[...]]' are not supported"
        )));
    }
    let pos = skip_ws(s, start + 1);
    if pos >= s.len() || s.as_bytes()[pos] == b']' {
        return Err(CoreError::SyncError(format!(
            "line {line_no}: section name cannot be empty"
        )));
    }
    let (section, next_pos) = parse_toml_key(s, pos, line_no)?;
    let close_pos = skip_ws(s, next_pos);
    if s.as_bytes().get(close_pos) != Some(&b']') {
        return Err(CoreError::SyncError(format!(
            "line {line_no}: invalid or unterminated section header"
        )));
    }
    expect_eol_or_comment(s, close_pos + 1, line_no)?;
    Ok(section)
}

fn parse_toml_value(s: &str, start: usize, line_no: usize) -> Result<TomlVal, CoreError> {
    let pos = skip_ws(s, start);
    if pos >= s.len() || s.as_bytes()[pos] == b'#' {
        return Err(CoreError::SyncError(format!(
            "line {line_no}: missing value after '='"
        )));
    }
    match s.as_bytes()[pos] {
        b'"' => {
            let (val, next) = parse_double_quoted(s, pos, line_no)?;
            expect_eol_or_comment(s, next, line_no)?;
            Ok(TomlVal::Scalar(val))
        }
        b'\'' => {
            let (val, next) = parse_single_quoted(s, pos, line_no)?;
            expect_eol_or_comment(s, next, line_no)?;
            Ok(TomlVal::Scalar(val))
        }
        _ => {
            let raw_end = s[pos..].find('#').map(|i| pos + i).unwrap_or(s.len());
            let token = s[pos..raw_end].trim();
            if token.is_empty() {
                return Err(CoreError::SyncError(format!(
                    "line {line_no}: missing value after '='"
                )));
            }
            if token == "null" {
                Ok(TomlVal::Null)
            } else if token == "true" || token == "false" {
                Ok(TomlVal::Scalar(token.to_string()))
            } else if is_valid_toml_int(token) {
                Ok(TomlVal::Int(token.replace('_', "")))
            } else if is_valid_toml_float(token) {
                Ok(TomlVal::Scalar(token.replace('_', "")))
            } else {
                Err(CoreError::SyncError(format!(
                    "line {line_no}: invalid TOML value '{token}'"
                )))
            }
        }
    }
}

fn is_valid_digit_run(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    let bytes = s.as_bytes();
    if bytes[0] == b'_' || bytes[bytes.len() - 1] == b'_' {
        return false;
    }
    let mut prev_underscore = false;
    for &b in bytes {
        if b == b'_' {
            if prev_underscore {
                return false;
            }
            prev_underscore = true;
        } else if b.is_ascii_digit() {
            prev_underscore = false;
        } else {
            return false;
        }
    }
    true
}

fn is_valid_toml_int(token: &str) -> bool {
    let body = token.strip_prefix(['+', '-']).unwrap_or(token);
    is_valid_digit_run(body)
}

fn is_valid_toml_float(token: &str) -> bool {
    let body = token.strip_prefix(['+', '-']).unwrap_or(token);
    let (mantissa, exp_opt) = match body.find(['e', 'E']) {
        Some(idx) => (&body[..idx], Some(&body[idx + 1..])),
        None => (body, None),
    };
    if let Some(exp) = exp_opt {
        let exp_digits = exp.strip_prefix(['+', '-']).unwrap_or(exp);
        if !is_valid_digit_run(exp_digits) {
            return false;
        }
    }
    match mantissa.split_once('.') {
        Some((int_part, frac_part)) => {
            is_valid_digit_run(int_part) && is_valid_digit_run(frac_part)
        }
        None => exp_opt.is_some() && is_valid_digit_run(mantissa),
    }
}
