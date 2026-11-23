use crate::error::CoreError;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const KEYBINDING_NAMESPACE: &str = "keybinding";
const THEME_NAMESPACE: &str = "theme";
const THEME_NAME_KEY: &str = "name";

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
        let updated_at = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO preferences (namespace, key, value, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(namespace, key) DO UPDATE SET
                value = excluded.value,
                updated_at = excluded.updated_at",
            params![namespace, key, value, updated_at],
        )?;
        Ok(())
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
        let affected = self.conn.execute(
            "DELETE FROM preferences WHERE namespace = ?1",
            params![KEYBINDING_NAMESPACE],
        )?;
        Ok(affected)
    }

    pub fn theme(&self) -> Result<Option<String>, CoreError> {
        self.get(THEME_NAMESPACE, THEME_NAME_KEY)
    }

    pub fn set_theme(&self, name: &str) -> Result<(), CoreError> {
        self.set(THEME_NAMESPACE, THEME_NAME_KEY, name)
    }
}
