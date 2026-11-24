use super::{
    codec_for, ImportReport, SectionPatch, SettingsFormat, SettingsPatch, SettingsSnapshot,
    SETTINGS_VERSION, THEME_NAMESPACE,
};
use crate::error::{CoreError, Result};
use crate::port::outbound::PreferenceRepository;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

const KEYBINDING_NAMESPACE: &str = "keybinding";
const THEME_NAME_KEY: &str = "name";
const UI_NAMESPACE: &str = "ui";

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
    conn: Mutex<rusqlite::Connection>,
}

impl UserPrefsStore {
    fn init_connection(conn: Connection, enable_wal: bool) -> Result<Self> {
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
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> Result<MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| CoreError::SyncError("preferences connection mutex poisoned".to_string()))
    }

    pub fn open(path: &Path) -> Result<Self> {
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

    pub fn open_default() -> Result<Option<Self>> {
        match default_prefs_db_path() {
            Some(path) => Self::open(&path).map(Some),
            None => Ok(None),
        }
    }

    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init_connection(conn, false)
    }

    pub fn get(&self, namespace: &str, key: &str) -> Result<Option<String>> {
        let conn = self.lock()?;
        let value = conn
            .query_row(
                "SELECT value FROM preferences WHERE namespace = ?1 AND key = ?2",
                params![namespace, key],
                |row| row.get(0),
            )
            .optional()?;
        Ok(value)
    }

    pub fn set(&self, namespace: &str, key: &str, value: &str) -> Result<()> {
        let conn = self.lock()?;
        upsert_preference(&conn, namespace, key, value)
    }

    pub fn delete(&self, namespace: &str, key: &str) -> Result<bool> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "DELETE FROM preferences WHERE namespace = ?1 AND key = ?2",
            params![namespace, key],
        )?;
        Ok(affected > 0)
    }

    pub fn list_namespace(&self, namespace: &str) -> Result<BTreeMap<String, String>> {
        let conn = self.lock()?;
        let mut stmt = conn
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

    fn clear_namespace(&self, namespace: &str) -> Result<usize> {
        let conn = self.lock()?;
        let affected = conn.execute(
            "DELETE FROM preferences WHERE namespace = ?1",
            params![namespace],
        )?;
        Ok(affected)
    }

    pub fn keybindings(&self) -> Result<BTreeMap<String, String>> {
        self.list_namespace(KEYBINDING_NAMESPACE)
    }

    pub fn set_keybinding(&self, action: &str, binding: &str) -> Result<()> {
        self.set(KEYBINDING_NAMESPACE, action, binding)
    }

    pub fn remove_keybinding(&self, action: &str) -> Result<bool> {
        self.delete(KEYBINDING_NAMESPACE, action)
    }

    pub fn clear_keybindings(&self) -> Result<usize> {
        self.clear_namespace(KEYBINDING_NAMESPACE)
    }

    pub fn theme(&self) -> Result<Option<String>> {
        self.get(THEME_NAMESPACE, THEME_NAME_KEY)
    }

    pub fn set_theme(&self, name: &str) -> Result<()> {
        self.set(THEME_NAMESPACE, THEME_NAME_KEY, name)
    }

    pub fn theme_prefs(&self) -> Result<BTreeMap<String, String>> {
        self.list_namespace(THEME_NAMESPACE)
    }

    pub fn set_theme_pref(&self, key: &str, value: &str) -> Result<()> {
        self.set(THEME_NAMESPACE, key, value)
    }

    pub fn clear_theme_prefs(&self) -> Result<usize> {
        self.clear_namespace(THEME_NAMESPACE)
    }

    pub fn ui_prefs(&self) -> Result<BTreeMap<String, String>> {
        self.list_namespace(UI_NAMESPACE)
    }

    pub fn set_ui_pref(&self, key: &str, value: &str) -> Result<()> {
        self.set(UI_NAMESPACE, key, value)
    }

    pub fn clear_ui_prefs(&self) -> Result<usize> {
        self.clear_namespace(UI_NAMESPACE)
    }

    pub fn export_snapshot(&self, only_sections: Option<&[String]>) -> Result<SettingsSnapshot> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
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
    ) -> Result<String> {
        let snapshot = self.export_snapshot(only_sections)?;
        codec_for(format).encode(&snapshot)
    }

    pub fn format_single_section(
        section: &str,
        entries: &BTreeMap<String, String>,
        format: SettingsFormat,
    ) -> Result<String> {
        codec_for(format).encode_section(section, entries)
    }

    pub fn parse_settings_patch(
        raw: &str,
        format: Option<SettingsFormat>,
    ) -> Result<SettingsPatch> {
        codec_for(detect_format(raw, format)).decode(raw)
    }

    pub fn parse_theme_patch(raw: &str, format: Option<SettingsFormat>) -> Result<SettingsPatch> {
        codec_for(detect_format(raw, format)).decode_theme(raw)
    }

    pub fn import_patch(
        &self,
        patch: &SettingsPatch,
        only_sections: Option<&[String]>,
        replace_all: bool,
    ) -> Result<ImportReport> {
        let mut conn = self.lock()?;
        let tx = conn.transaction()?;

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
    ) -> Result<ImportReport> {
        let patch = Self::parse_settings_patch(raw, format)?;
        self.import_patch(&patch, only_sections, replace_all)
    }

    pub fn import_theme_str(
        &self,
        raw: &str,
        format: Option<SettingsFormat>,
        replace_theme: bool,
    ) -> Result<ImportReport> {
        let patch = Self::parse_theme_patch(raw, format)?;
        self.import_patch(&patch, Some(&[THEME_NAMESPACE.to_string()]), replace_theme)
    }
}

impl PreferenceRepository for UserPrefsStore {
    fn get(&self, namespace: &str, key: &str) -> Result<Option<String>> {
        UserPrefsStore::get(self, namespace, key)
    }

    fn set(&self, namespace: &str, key: &str, value: &str) -> Result<()> {
        UserPrefsStore::set(self, namespace, key, value)
    }

    fn delete(&self, namespace: &str, key: &str) -> Result<bool> {
        UserPrefsStore::delete(self, namespace, key)
    }

    fn list_namespace(&self, namespace: &str) -> Result<BTreeMap<String, String>> {
        UserPrefsStore::list_namespace(self, namespace)
    }

    fn keybindings(&self) -> Result<BTreeMap<String, String>> {
        UserPrefsStore::keybindings(self)
    }

    fn set_keybinding(&self, action: &str, binding: &str) -> Result<()> {
        UserPrefsStore::set_keybinding(self, action, binding)
    }

    fn remove_keybinding(&self, action: &str) -> Result<bool> {
        UserPrefsStore::remove_keybinding(self, action)
    }

    fn clear_keybindings(&self) -> Result<usize> {
        UserPrefsStore::clear_keybindings(self)
    }

    fn theme(&self) -> Result<Option<String>> {
        UserPrefsStore::theme(self)
    }

    fn set_theme(&self, name: &str) -> Result<()> {
        UserPrefsStore::set_theme(self, name)
    }

    fn theme_prefs(&self) -> Result<BTreeMap<String, String>> {
        UserPrefsStore::theme_prefs(self)
    }

    fn set_theme_pref(&self, key: &str, value: &str) -> Result<()> {
        UserPrefsStore::set_theme_pref(self, key, value)
    }

    fn clear_theme_prefs(&self) -> Result<usize> {
        UserPrefsStore::clear_theme_prefs(self)
    }

    fn ui_prefs(&self) -> Result<BTreeMap<String, String>> {
        UserPrefsStore::ui_prefs(self)
    }

    fn set_ui_pref(&self, key: &str, value: &str) -> Result<()> {
        UserPrefsStore::set_ui_pref(self, key, value)
    }

    fn clear_ui_prefs(&self) -> Result<usize> {
        UserPrefsStore::clear_ui_prefs(self)
    }

    fn export_snapshot(&self, only_sections: Option<&[String]>) -> Result<SettingsSnapshot> {
        UserPrefsStore::export_snapshot(self, only_sections)
    }

    fn export_string(
        &self,
        format: SettingsFormat,
        only_sections: Option<&[String]>,
    ) -> Result<String> {
        UserPrefsStore::export_string(self, format, only_sections)
    }

    fn import_patch(
        &self,
        patch: &SettingsPatch,
        only_sections: Option<&[String]>,
        replace_all: bool,
    ) -> Result<ImportReport> {
        UserPrefsStore::import_patch(self, patch, only_sections, replace_all)
    }

    fn import_str(
        &self,
        raw: &str,
        format: Option<SettingsFormat>,
        only_sections: Option<&[String]>,
        replace_all: bool,
    ) -> Result<ImportReport> {
        UserPrefsStore::import_str(self, raw, format, only_sections, replace_all)
    }

    fn import_theme_str(
        &self,
        raw: &str,
        format: Option<SettingsFormat>,
        replace_theme: bool,
    ) -> Result<ImportReport> {
        UserPrefsStore::import_theme_str(self, raw, format, replace_theme)
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

fn upsert_preference(conn: &Connection, namespace: &str, key: &str, value: &str) -> Result<()> {
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

fn list_distinct_namespaces(conn: &Connection) -> Result<Vec<String>> {
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
