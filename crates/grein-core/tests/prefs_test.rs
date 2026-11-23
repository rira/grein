use grein_core::prefs::resolve_prefs_db_path_from_env;
use grein_core::{default_prefs_db_path, UserPrefsStore};
use rusqlite::Connection;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use uuid::Uuid;

static ENV_LOCK: Mutex<()> = Mutex::new(());

struct TempDirGuard {
    path: PathBuf,
}

impl TempDirGuard {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("grein-prefs-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&path).expect("failed to create test temp root");
        Self { path }
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

struct EnvVarGuard {
    saved: Vec<(&'static str, Option<OsString>)>,
}

impl EnvVarGuard {
    fn capture(keys: &[&'static str]) -> Self {
        let saved = keys
            .iter()
            .map(|&key| (key, std::env::var_os(key)))
            .collect();
        Self { saved }
    }

    fn set(key: &str, value: Option<&str>) {
        match value {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        for (key, value) in &self.saved {
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
    }
}

#[test]
fn test_resolve_prefs_db_path_precedence_and_blank_handling() {
    assert_eq!(resolve_prefs_db_path_from_env(None, None, None), None);
    assert_eq!(
        resolve_prefs_db_path_from_env(Some(""), Some("   "), Some("\t\n")),
        None
    );

    assert_eq!(
        resolve_prefs_db_path_from_env(None, None, Some("/home/alice")),
        Some(PathBuf::from("/home/alice/.config/grein/grein.db"))
    );

    assert_eq!(
        resolve_prefs_db_path_from_env(None, Some("/xdg/config"), Some("/home/alice")),
        Some(PathBuf::from("/xdg/config/grein/grein.db"))
    );

    assert_eq!(
        resolve_prefs_db_path_from_env(None, Some("   "), Some(" /home/alice ")),
        Some(PathBuf::from("/home/alice/.config/grein/grein.db"))
    );

    assert_eq!(
        resolve_prefs_db_path_from_env(
            Some(" /custom/prefs.sqlite "),
            Some("/xdg/config"),
            Some("/home/alice")
        ),
        Some(PathBuf::from("/custom/prefs.sqlite"))
    );

    assert_eq!(
        resolve_prefs_db_path_from_env(Some("   "), Some(" /xdg/config "), Some("/home/alice")),
        Some(PathBuf::from("/xdg/config/grein/grein.db"))
    );
}

#[test]
fn test_default_prefs_db_path_and_open_default_with_env() {
    let _lock = ENV_LOCK.lock().expect("env lock poisoned");
    let _env_guard = EnvVarGuard::capture(&["GREIN_CONFIG_DB", "XDG_CONFIG_HOME", "HOME"]);
    let temp = TempDirGuard::new();

    EnvVarGuard::set("GREIN_CONFIG_DB", None);
    EnvVarGuard::set("XDG_CONFIG_HOME", None);
    EnvVarGuard::set("HOME", None);
    assert_eq!(default_prefs_db_path(), None);
    assert!(UserPrefsStore::open_default()
        .expect("open_default without env")
        .is_none());

    let home_dir = temp.path().join("home");
    let home_str = home_dir.to_str().expect("utf8 path");
    EnvVarGuard::set("HOME", Some(home_str));
    assert_eq!(
        default_prefs_db_path(),
        Some(home_dir.join(".config").join("grein").join("grein.db"))
    );

    let xdg_dir = temp.path().join("xdg");
    let xdg_str = xdg_dir.to_str().expect("utf8 path");
    EnvVarGuard::set("XDG_CONFIG_HOME", Some(xdg_str));
    assert_eq!(
        default_prefs_db_path(),
        Some(xdg_dir.join("grein").join("grein.db"))
    );

    let explicit_db = temp.path().join("explicit").join("custom.db");
    let explicit_str = explicit_db.to_str().expect("utf8 path");
    EnvVarGuard::set("GREIN_CONFIG_DB", Some(explicit_str));
    assert_eq!(default_prefs_db_path(), Some(explicit_db.clone()));

    let store = UserPrefsStore::open_default()
        .expect("open_default should succeed")
        .expect("open_default should return Some(store)");
    store
        .set_theme("solarized")
        .expect("set_theme via open_default");
    drop(store);

    assert!(explicit_db.exists());
    let reopened = UserPrefsStore::open(&explicit_db).expect("reopen explicit db");
    assert_eq!(
        reopened.theme().expect("read theme"),
        Some("solarized".to_string())
    );
}

#[test]
fn test_open_creates_nested_parent_dirs_and_persists_across_reopen() {
    let temp = TempDirGuard::new();
    let db_path = temp
        .path()
        .join("nested")
        .join("dirs")
        .join("config")
        .join("grein.db");

    {
        let store = UserPrefsStore::open(&db_path).expect("open should create parent dirs");
        store
            .set_keybinding("quit", "q")
            .expect("set_keybinding should succeed");
        store.set_theme("nord").expect("set_theme should succeed");
        store
            .set("ui", "density", "compact")
            .expect("set custom pref should succeed");
    }

    assert!(db_path.exists());

    let raw = Connection::open(&db_path).expect("open raw sqlite connection");
    let journal_mode: String = raw
        .query_row("PRAGMA journal_mode;", [], |row| row.get(0))
        .expect("query journal_mode");
    assert_eq!(journal_mode.to_lowercase(), "wal");
    drop(raw);

    let reopened = UserPrefsStore::open(&db_path).expect("reopen existing db");
    assert_eq!(
        reopened.keybindings().expect("read keybindings"),
        BTreeMap::from([("quit".to_string(), "q".to_string())])
    );
    assert_eq!(
        reopened.theme().expect("read theme"),
        Some("nord".to_string())
    );
    assert_eq!(
        reopened.get("ui", "density").expect("read custom pref"),
        Some("compact".to_string())
    );
}

#[test]
fn test_in_memory_crud_upsert_updated_at_and_delete() {
    let mem = UserPrefsStore::in_memory().expect("in_memory store");
    assert_eq!(mem.get("ui", "missing").expect("get missing"), None);
    assert!(!mem.delete("ui", "missing").expect("delete missing"));

    mem.set("ui", "font_size", "14").expect("initial set");
    assert_eq!(
        mem.get("ui", "font_size").expect("get after set"),
        Some("14".to_string())
    );

    mem.set("ui", "font_size", "16").expect("upsert overwrite");
    assert_eq!(
        mem.get("ui", "font_size").expect("get after upsert"),
        Some("16".to_string())
    );

    assert!(mem.delete("ui", "font_size").expect("delete existing"));
    assert_eq!(mem.get("ui", "font_size").expect("get after delete"), None);
    assert!(!mem.delete("ui", "font_size").expect("delete twice"));

    let temp = TempDirGuard::new();
    let db_path = temp.path().join("upsert.db");
    let store = UserPrefsStore::open(&db_path).expect("open file store");
    store.set("editor", "tab_width", "2").expect("first set");

    let raw = Connection::open(&db_path).expect("open raw connection");
    let (val1, ts1): (String, String) = raw
        .query_row(
            "SELECT value, updated_at FROM preferences WHERE namespace = ?1 AND key = ?2",
            ["editor", "tab_width"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("query first row");
    assert_eq!(val1, "2");
    let parsed_ts1 = chrono::DateTime::parse_from_rfc3339(&ts1).expect("valid rfc3339 ts1");

    std::thread::sleep(Duration::from_millis(5));
    store.set("editor", "tab_width", "4").expect("second set");

    let (val2, ts2): (String, String) = raw
        .query_row(
            "SELECT value, updated_at FROM preferences WHERE namespace = ?1 AND key = ?2",
            ["editor", "tab_width"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("query second row");
    assert_eq!(val2, "4");
    let parsed_ts2 = chrono::DateTime::parse_from_rfc3339(&ts2).expect("valid rfc3339 ts2");
    assert!(
        parsed_ts2 > parsed_ts1,
        "expected updated_at to advance on upsert ({ts1} -> {ts2})"
    );
}

#[test]
fn test_list_namespace_isolation() {
    let store = UserPrefsStore::in_memory().expect("in_memory store");

    store.set_keybinding("quit", "q").expect("set keybinding");
    store
        .set_keybinding("name", "ctrl+n")
        .expect("set keybinding named 'name'");
    store.set_theme("gruvbox").expect("set theme");
    store
        .set("custom", "name", "custom_value")
        .expect("set custom 'name'");
    store
        .set("custom", "alpha", "first")
        .expect("set custom 'alpha'");

    assert_eq!(
        store.list_namespace("keybinding").expect("list keybinding"),
        BTreeMap::from([
            ("name".to_string(), "ctrl+n".to_string()),
            ("quit".to_string(), "q".to_string()),
        ])
    );
    assert_eq!(
        store.list_namespace("theme").expect("list theme"),
        BTreeMap::from([("name".to_string(), "gruvbox".to_string())])
    );
    assert_eq!(
        store.list_namespace("custom").expect("list custom"),
        BTreeMap::from([
            ("alpha".to_string(), "first".to_string()),
            ("name".to_string(), "custom_value".to_string()),
        ])
    );
    assert_eq!(
        store.list_namespace("empty").expect("list nonexistent"),
        BTreeMap::new()
    );

    assert!(store.delete("custom", "name").expect("delete custom name"));
    assert_eq!(
        store.theme().expect("theme untouched"),
        Some("gruvbox".to_string())
    );
    assert_eq!(
        store
            .get("keybinding", "name")
            .expect("keybinding 'name' untouched"),
        Some("ctrl+n".to_string())
    );
}

#[test]
fn test_keybinding_helpers() {
    let store = UserPrefsStore::in_memory().expect("in_memory store");
    assert!(store.keybindings().expect("initial empty").is_empty());

    store.set_keybinding("quit", "q").expect("set quit");
    store
        .set_keybinding("branch", "ctrl+b")
        .expect("set branch");
    store.set_keybinding("quit", "ctrl+q").expect("update quit");
    store.set_theme("dracula").expect("set theme");
    store.set("ui", "sidebar", "open").expect("set ui pref");

    assert_eq!(
        store.keybindings().expect("list keybindings"),
        BTreeMap::from([
            ("branch".to_string(), "ctrl+b".to_string()),
            ("quit".to_string(), "ctrl+q".to_string()),
        ])
    );

    assert!(!store
        .remove_keybinding("nonexistent")
        .expect("remove nonexistent"));
    assert!(store.remove_keybinding("branch").expect("remove branch"));
    assert_eq!(
        store.keybindings().expect("after remove"),
        BTreeMap::from([("quit".to_string(), "ctrl+q".to_string())])
    );

    let cleared = store.clear_keybindings().expect("clear keybindings");
    assert_eq!(cleared, 1);
    assert!(store.keybindings().expect("after clear").is_empty());
    assert_eq!(
        store.clear_keybindings().expect("clear when already empty"),
        0
    );

    assert_eq!(
        store.theme().expect("theme preserved"),
        Some("dracula".to_string())
    );
    assert_eq!(
        store.get("ui", "sidebar").expect("ui pref preserved"),
        Some("open".to_string())
    );
}

#[test]
fn test_theme_helpers() {
    let store = UserPrefsStore::in_memory().expect("in_memory store");
    assert_eq!(store.theme().expect("initial theme"), None);

    store.set_theme("tokyo-night").expect("set theme");
    assert_eq!(
        store.theme().expect("read theme"),
        Some("tokyo-night".to_string())
    );
    assert_eq!(
        store.get("theme", "name").expect("read theme via get"),
        Some("tokyo-night".to_string())
    );

    store
        .set_theme("catppuccin-mocha")
        .expect("overwrite theme");
    assert_eq!(
        store.theme().expect("read overwritten theme"),
        Some("catppuccin-mocha".to_string())
    );
}

#[test]
fn test_open_fails_cleanly_when_parent_is_a_file() {
    let temp = TempDirGuard::new();
    let blocking_file = temp.path().join("not_a_dir");
    std::fs::write(&blocking_file, b"occupied").expect("create blocking file");

    let db_path = blocking_file.join("grein.db");
    let err = UserPrefsStore::open(&db_path).expect_err("expected error creating parent dir");
    assert!(
        matches!(err, grein_core::CoreError::SyncError(_)),
        "expected CoreError::SyncError, got {err:?}"
    );
}
