use grein_core::prefs::resolve_prefs_db_path_from_env;
use grein_core::{
    default_prefs_db_path, CoreError, ImportReport, SectionPatch, SettingsFormat, SettingsPatch,
    SettingsSnapshot, UserPrefsStore,
};
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

#[test]
fn test_theme_and_ui_namespace_helpers() {
    let store = UserPrefsStore::in_memory().expect("in_memory store");
    assert!(store.theme_prefs().expect("initial theme_prefs").is_empty());
    assert!(store.ui_prefs().expect("initial ui_prefs").is_empty());

    store.set_keybinding("quit", "q").expect("set keybinding");
    store
        .set_theme_pref("name", "tokyo-night")
        .expect("set theme name");
    store
        .set_theme_pref("accent", "#7dcfff")
        .expect("set theme accent");
    store
        .set_ui_pref("density", "compact")
        .expect("set ui density");
    store
        .set_ui_pref("sidebar", "open")
        .expect("set ui sidebar");

    assert_eq!(
        store.theme_prefs().expect("read theme_prefs"),
        BTreeMap::from([
            ("accent".to_string(), "#7dcfff".to_string()),
            ("name".to_string(), "tokyo-night".to_string()),
        ])
    );
    assert_eq!(
        store.ui_prefs().expect("read ui_prefs"),
        BTreeMap::from([
            ("density".to_string(), "compact".to_string()),
            ("sidebar".to_string(), "open".to_string()),
        ])
    );

    assert_eq!(store.clear_theme_prefs().expect("clear theme_prefs"), 2);
    assert!(store.theme_prefs().expect("empty theme_prefs").is_empty());
    assert_eq!(
        store.clear_theme_prefs().expect("clear empty theme_prefs"),
        0
    );
    assert_eq!(store.ui_prefs().expect("ui_prefs preserved").len(), 2);
    assert_eq!(store.keybindings().expect("keybindings preserved").len(), 1);

    assert_eq!(store.clear_ui_prefs().expect("clear ui_prefs"), 2);
    assert!(store.ui_prefs().expect("empty ui_prefs").is_empty());
    assert_eq!(store.clear_ui_prefs().expect("clear empty ui_prefs"), 0);
    assert_eq!(
        store.keybindings().expect("keybindings still preserved"),
        BTreeMap::from([("quit".to_string(), "q".to_string())])
    );
}

#[test]
fn test_export_snapshot_and_only_sections_filtering() {
    let store = UserPrefsStore::in_memory().expect("in_memory store");
    store.set_ui_pref("sidebar", "open").expect("set ui");
    store
        .set_theme_pref("name", "nord")
        .expect("set theme name");
    store
        .set_theme_pref("accent", "#88c0d0")
        .expect("set theme accent");
    store.set_keybinding("quit", "q").expect("set keybinding");

    let full = store.export_snapshot(None).expect("export full snapshot");
    assert_eq!(full.version, 1);
    assert_eq!(
        full,
        SettingsSnapshot {
            version: 1,
            preferences: BTreeMap::from([
                (
                    "keybinding".to_string(),
                    BTreeMap::from([("quit".to_string(), "q".to_string())]),
                ),
                (
                    "theme".to_string(),
                    BTreeMap::from([
                        ("accent".to_string(), "#88c0d0".to_string()),
                        ("name".to_string(), "nord".to_string()),
                    ]),
                ),
                (
                    "ui".to_string(),
                    BTreeMap::from([("sidebar".to_string(), "open".to_string())]),
                ),
            ]),
        }
    );

    let filtered = store
        .export_snapshot(Some(&[" Theme ".to_string(), "UI".to_string()]))
        .expect("export filtered snapshot");
    assert_eq!(
        filtered.preferences.keys().cloned().collect::<Vec<_>>(),
        vec!["theme".to_string(), "ui".to_string()]
    );

    let empty_filter = store
        .export_snapshot(Some(&[]))
        .expect("export with empty filter");
    assert!(empty_filter.preferences.is_empty());

    store.clear_ui_prefs().expect("clear ui");
    let after_clear = store.export_snapshot(None).expect("export after clear");
    assert!(!after_clear.preferences.contains_key("ui"));
}

#[test]
fn test_export_string_and_format_single_section_roundtrip() {
    let store = UserPrefsStore::in_memory().expect("in_memory store");
    store
        .set_keybinding("alt+z", "toggle\n\"zoom\"\t\\")
        .expect("set special keybinding");
    store.set_keybinding("quit", "q").expect("set quit");
    store
        .set_theme_pref("name", "nord")
        .expect("set theme name");
    store
        .set_theme_pref("accent", "#88c0d0")
        .expect("set theme accent");

    let json_out = store
        .export_string(SettingsFormat::Json, None)
        .expect("export json");
    assert!(json_out.ends_with('\n'));
    let parsed_json: SettingsSnapshot =
        serde_json::from_str(&json_out).expect("valid exported json");
    assert_eq!(parsed_json, store.export_snapshot(None).expect("snapshot"));

    let toml_out = store
        .export_string(SettingsFormat::Toml, None)
        .expect("export toml");
    assert!(toml_out.starts_with("# grein configuration\nversion = 1\n"));
    assert!(toml_out.contains("[keybinding]\n"));
    assert!(toml_out.contains("\"alt+z\" = \"toggle\\n\\\"zoom\\\"\\t\\\\\"\n"));
    assert!(toml_out.contains("[theme]\n"));

    let single_toml = UserPrefsStore::format_single_section(
        "theme",
        &BTreeMap::from([
            ("accent".to_string(), "#7dcfff".to_string()),
            ("name".to_string(), "tokyo-night".to_string()),
        ]),
        SettingsFormat::Toml,
    )
    .expect("format single section toml");
    assert_eq!(
        single_toml,
        "[theme]\naccent = \"#7dcfff\"\nname = \"tokyo-night\"\n"
    );

    let single_json = UserPrefsStore::format_single_section(
        "theme",
        &BTreeMap::from([("name".to_string(), "tokyo-night".to_string())]),
        SettingsFormat::Json,
    )
    .expect("format single section json");
    assert!(single_json.ends_with('\n'));
    let single_snap: SettingsSnapshot =
        serde_json::from_str(&single_json).expect("valid single section json");
    assert_eq!(single_snap.version, 1);
    assert_eq!(
        single_snap.preferences.get("theme"),
        Some(&BTreeMap::from([(
            "name".to_string(),
            "tokyo-night".to_string()
        )]))
    );

    let empty_section_err =
        UserPrefsStore::format_single_section("   ", &BTreeMap::new(), SettingsFormat::Toml)
            .expect_err("empty section name should fail");
    assert!(matches!(empty_section_err, CoreError::SyncError(_)));

    let restored = UserPrefsStore::in_memory().expect("restored store");
    restored
        .import_str(&toml_out, None, None, false)
        .expect("re-import exported toml");
    assert_eq!(
        restored.export_snapshot(None).expect("restored snapshot"),
        store.export_snapshot(None).expect("original snapshot")
    );
}

#[test]
fn test_parse_settings_patch_json_variants_and_null_resets() {
    let wrapped = r#"{
        "version": 1,
        "preferences": {
            "theme": {
                "name": "nord",
                "accent": null,
                "font_size": 14,
                "bold": true
            },
            "keybinding": null
        }
    }"#;
    let patch = UserPrefsStore::parse_settings_patch(wrapped, None).expect("parse wrapped json");
    assert_eq!(patch.version, 1);
    assert_eq!(
        patch.preferences.get("keybinding"),
        Some(&SectionPatch::ResetToDefault)
    );
    assert_eq!(
        patch.preferences.get("theme"),
        Some(&SectionPatch::Entries(BTreeMap::from([
            ("accent".to_string(), None),
            ("bold".to_string(), Some("true".to_string())),
            ("font_size".to_string(), Some("14".to_string())),
            ("name".to_string(), Some("nord".to_string())),
        ])))
    );
    assert!(!patch.preferences.contains_key("ui"));

    let direct = r#"{
        "version": 2,
        "theme": { "name": "gruvbox" },
        "keybinding": null
    }"#;
    let direct_patch = UserPrefsStore::parse_settings_patch(direct, Some(SettingsFormat::Json))
        .expect("parse direct json");
    assert_eq!(direct_patch.version, 2);
    assert_eq!(
        direct_patch.preferences.get("theme"),
        Some(&SectionPatch::Entries(BTreeMap::from([(
            "name".to_string(),
            Some("gruvbox".to_string())
        )])))
    );
    assert_eq!(
        direct_patch.preferences.get("keybinding"),
        Some(&SectionPatch::ResetToDefault)
    );
}

#[test]
fn test_parse_settings_patch_toml_comments_escapes_and_null_resets() {
    let toml_raw = r##"
# Grein settings patch
version = 1
keybinding = null # reset all keybindings

[theme]
name = "tokyo-night" # comment after double quote
accent = "#7dcfff" # '#' inside quotes must not start a comment
literal_path = 'C:\themes\tokyo\#1' # literal single-quoted string
escaped = "line1\nline2\t\"quoted\"\\end\r"
contrast = 1.5
high_contrast = false
old_override = null

[ui]
enter_manage_mode = "m"
ui.density = "compact"
"alt+z" = "zoom"

[empty_reset]
# only a comment inside empty table -> ResetToDefault
"##;

    let patch = UserPrefsStore::parse_settings_patch(toml_raw, None).expect("parse toml patch");
    assert_eq!(patch.version, 1);
    assert_eq!(
        patch.preferences.get("keybinding"),
        Some(&SectionPatch::ResetToDefault)
    );
    assert_eq!(
        patch.preferences.get("empty_reset"),
        Some(&SectionPatch::ResetToDefault)
    );
    assert_eq!(
        patch.preferences.get("theme"),
        Some(&SectionPatch::Entries(BTreeMap::from([
            ("accent".to_string(), Some("#7dcfff".to_string())),
            ("contrast".to_string(), Some("1.5".to_string())),
            (
                "escaped".to_string(),
                Some("line1\nline2\t\"quoted\"\\end\r".to_string()),
            ),
            ("high_contrast".to_string(), Some("false".to_string())),
            (
                "literal_path".to_string(),
                Some("C:\\themes\\tokyo\\#1".to_string()),
            ),
            ("name".to_string(), Some("tokyo-night".to_string())),
            ("old_override".to_string(), None),
        ])))
    );
    assert_eq!(
        patch.preferences.get("ui"),
        Some(&SectionPatch::Entries(BTreeMap::from([
            ("alt+z".to_string(), Some("zoom".to_string())),
            ("enter_manage_mode".to_string(), Some("m".to_string())),
            ("ui.density".to_string(), Some("compact".to_string())),
        ])))
    );
}

#[test]
fn test_import_colleague_theme_keeps_keybindings_and_ui_untouched() {
    let store = UserPrefsStore::in_memory().expect("in_memory store");
    store.set_keybinding("quit", "q").expect("set quit");
    store
        .set_keybinding("branch", "ctrl+b")
        .expect("set branch");
    store.set_ui_pref("density", "compact").expect("set ui");
    store
        .set_theme_pref("name", "old-theme")
        .expect("set old theme");
    store
        .set_theme_pref("legacy_bg", "#000000")
        .expect("set legacy theme key");

    let colleague_flat_toml = r##"
# Colleague's custom Tokyo Night palette
name = "tokyo-night"
accent = "#7dcfff"
bg = '#1a1b26'
border = "#414868"
"##;
    let report = store
        .import_theme_str(colleague_flat_toml, None, false)
        .expect("import flat theme toml");
    assert_eq!(
        report,
        ImportReport {
            updated: 4,
            reset_keys: 0,
            reset_sections: 0,
        }
    );
    assert_eq!(
        store.theme_prefs().expect("merged theme"),
        BTreeMap::from([
            ("accent".to_string(), "#7dcfff".to_string()),
            ("bg".to_string(), "#1a1b26".to_string()),
            ("border".to_string(), "#414868".to_string()),
            ("legacy_bg".to_string(), "#000000".to_string()),
            ("name".to_string(), "tokyo-night".to_string()),
        ])
    );
    assert_eq!(
        store.keybindings().expect("keybindings untouched"),
        BTreeMap::from([
            ("branch".to_string(), "ctrl+b".to_string()),
            ("quit".to_string(), "q".to_string()),
        ])
    );
    assert_eq!(
        store.ui_prefs().expect("ui untouched"),
        BTreeMap::from([("density".to_string(), "compact".to_string())])
    );

    let full_json_from_colleague = r##"{
        "version": 1,
        "preferences": {
            "keybinding": { "quit": "ctrl+x" },
            "ui": { "density": "spacious" },
            "theme": { "name": "nord", "accent": "#88c0d0" }
        }
    }"##;
    let replace_report = store
        .import_theme_str(full_json_from_colleague, None, true)
        .expect("replace theme from full settings json");
    assert_eq!(
        replace_report,
        ImportReport {
            updated: 2,
            reset_keys: 0,
            reset_sections: 0,
        }
    );
    assert_eq!(
        store.theme_prefs().expect("replaced theme only"),
        BTreeMap::from([
            ("accent".to_string(), "#88c0d0".to_string()),
            ("name".to_string(), "nord".to_string()),
        ])
    );
    assert_eq!(
        store.keybindings().expect("keybindings still untouched"),
        BTreeMap::from([
            ("branch".to_string(), "ctrl+b".to_string()),
            ("quit".to_string(), "q".to_string()),
        ])
    );
    assert_eq!(
        store.ui_prefs().expect("ui still untouched"),
        BTreeMap::from([("density".to_string(), "compact".to_string())])
    );

    let flat_json_theme = r##"{
        "name": "gruvbox",
        "accent": null,
        "fg": "#ebdbb2"
    }"##;
    let flat_json_report = store
        .import_theme_str(flat_json_theme, None, false)
        .expect("import flat json theme");
    assert_eq!(
        flat_json_report,
        ImportReport {
            updated: 2,
            reset_keys: 1,
            reset_sections: 0,
        }
    );
    assert_eq!(
        store.theme_prefs().expect("theme after flat json patch"),
        BTreeMap::from([
            ("fg".to_string(), "#ebdbb2".to_string()),
            ("name".to_string(), "gruvbox".to_string()),
        ])
    );

    let sectioned_toml_theme = "[keybinding]\nquit = \"z\"\n[theme]\nname = \"catppuccin\"\n";
    store
        .import_theme_str(sectioned_toml_theme, None, true)
        .expect("import [theme] from multi-section toml");
    assert_eq!(
        store.theme_prefs().expect("catppuccin theme"),
        BTreeMap::from([("name".to_string(), "catppuccin".to_string())])
    );
    assert_eq!(
        store.get("keybinding", "quit").expect("quit unchanged"),
        Some("q".to_string())
    );
}

#[test]
fn test_import_section_reset_and_per_key_reset_semantics() {
    let store = UserPrefsStore::in_memory().expect("in_memory store");
    store.set_keybinding("quit", "q").expect("set quit");
    store
        .set_keybinding("branch", "ctrl+b")
        .expect("set branch");
    store
        .set_theme_pref("name", "nord")
        .expect("set theme name");
    store
        .set_theme_pref("accent", "#88c0d0")
        .expect("set theme accent");
    store.set_ui_pref("density", "compact").expect("set ui");

    let json_reset = r#"{"keybinding": null, "theme": {"accent": null}}"#;
    let r1 = store
        .import_str(json_reset, Some(SettingsFormat::Json), None, false)
        .expect("import json null resets");
    assert_eq!(
        r1,
        ImportReport {
            updated: 0,
            reset_keys: 1,
            reset_sections: 1,
        }
    );
    assert!(store.keybindings().expect("keybindings reset").is_empty());
    assert_eq!(
        store
            .theme_prefs()
            .expect("theme accent removed, name kept"),
        BTreeMap::from([("name".to_string(), "nord".to_string())])
    );
    assert_eq!(
        store.ui_prefs().expect("ui untouched"),
        BTreeMap::from([("density".to_string(), "compact".to_string())])
    );

    store.set_keybinding("quit", "q").expect("restore quit");
    let toml_null_reset = "keybinding = null\n";
    let r2 = store
        .import_str(toml_null_reset, Some(SettingsFormat::Toml), None, false)
        .expect("import toml keybinding = null");
    assert_eq!(r2.reset_sections, 1);
    assert!(store
        .keybindings()
        .expect("keybindings reset via toml null")
        .is_empty());
    assert_eq!(
        store.theme().expect("theme preserved"),
        Some("nord".to_string())
    );

    store
        .set_keybinding("quit", "q")
        .expect("restore quit again");
    let toml_empty_table_reset = "[keybinding]\n";
    let r3 = store
        .import_str(toml_empty_table_reset, None, None, false)
        .expect("import empty [keybinding] reset");
    assert_eq!(r3.reset_sections, 1);
    assert!(store
        .keybindings()
        .expect("keybindings reset via empty table")
        .is_empty());
    assert_eq!(
        store.theme().expect("theme still preserved"),
        Some("nord".to_string())
    );
}

#[test]
fn test_import_only_sections_and_replace_all_scoped_vs_unscoped() {
    let store = UserPrefsStore::in_memory().expect("in_memory store");
    store.set_keybinding("quit", "q").expect("set quit");
    store
        .set_theme_pref("name", "nord")
        .expect("set theme name");
    store
        .set_theme_pref("stale", "yes")
        .expect("set stale theme key");
    store.set_ui_pref("density", "compact").expect("set ui");

    let multi_toml = r#"
[keybinding]
quit = "ctrl+q"

[theme]
name = "dracula"

[ui]
density = "spacious"
"#;

    let scoped_report = store
        .import_str(multi_toml, None, Some(&["theme".to_string()]), true)
        .expect("scoped replace_all import");
    assert_eq!(
        scoped_report,
        ImportReport {
            updated: 1,
            reset_keys: 0,
            reset_sections: 0,
        }
    );
    assert_eq!(
        store.theme_prefs().expect("theme replaced cleanly"),
        BTreeMap::from([("name".to_string(), "dracula".to_string())])
    );
    assert_eq!(
        store
            .keybindings()
            .expect("keybinding untouched by scoped import"),
        BTreeMap::from([("quit".to_string(), "q".to_string())])
    );
    assert_eq!(
        store.ui_prefs().expect("ui untouched by scoped import"),
        BTreeMap::from([("density".to_string(), "compact".to_string())])
    );

    let theme_only_toml = "[theme]\nname = \"solarized\"\n";
    let unscoped_report = store
        .import_str(theme_only_toml, None, None, true)
        .expect("unscoped replace_all import");
    assert_eq!(unscoped_report.updated, 1);
    assert_eq!(
        store.theme_prefs().expect("theme updated"),
        BTreeMap::from([("name".to_string(), "solarized".to_string())])
    );
    assert!(store
        .keybindings()
        .expect("keybindings wiped by unscoped replace_all")
        .is_empty());
    assert!(store
        .ui_prefs()
        .expect("ui wiped by unscoped replace_all")
        .is_empty());
}

#[test]
fn test_malformed_inputs_return_core_error_and_import_rolls_back_atomically() {
    let malformed_json_cases = [
        "{bad json",
        "[1, 2, 3]",
        "\"just a string\"",
        r#"{"preferences": "not-an-object"}"#,
        r#"{"version": "1", "preferences": {}}"#,
        r#"{"version": -1, "preferences": {}}"#,
        r#"{"version": 1, "preferences": {}, "extra": {}}"#,
        r#"{"theme": 42}"#,
        r##"{"theme": {"colors": ["#fff", "#000"]}}"##,
        r#"{"theme": {"nested": {"a": 1}}}"#,
    ];
    for raw in malformed_json_cases {
        assert!(
            UserPrefsStore::parse_settings_patch(raw, Some(SettingsFormat::Json)).is_err(),
            "expected JSON parse error for: {raw}"
        );
    }

    let malformed_toml_cases = [
        "name = \"nord\"\n",
        "version = \"1\"\n[theme]\nname = \"nord\"\n",
        "[theme]\nname = \"unclosed\n",
        "[theme]\nname = 'unclosed\n",
        "[theme]\nname = \"bad\\qescape\"\n",
        "[]\nname = \"nord\"\n",
        "[unclosed\n",
        "[a[b]]\n",
        "[[theme]]\n",
        "[theme] extra\n",
        "[theme]\njust_a_key\n",
        "[theme]\nname = \n",
        "[theme]\nname = unquoted_word\n",
        "[theme]\narr = [1, 2]\n",
        "[theme]\nname = \"nord\" trailing\n",
        "theme = null\n[theme]\nname = \"nord\"\n",
        "[theme]\nname = \"a\"\n[theme]\naccent = \"b\"\n",
    ];
    for raw in malformed_toml_cases {
        assert!(
            UserPrefsStore::parse_settings_patch(raw, Some(SettingsFormat::Toml)).is_err(),
            "expected TOML parse error for: {raw}"
        );
    }

    let no_theme_cases = [
        ("", None),
        ("# comment only\n", Some(SettingsFormat::Toml)),
        ("version = 1\n", Some(SettingsFormat::Toml)),
        ("[keybinding]\nquit = \"q\"\n", Some(SettingsFormat::Toml)),
        (
            r#"{"preferences": {"keybinding": {"quit": "q"}}}"#,
            Some(SettingsFormat::Json),
        ),
        (
            r#"{"keybinding": {"quit": "q"}}"#,
            Some(SettingsFormat::Json),
        ),
        (r#"{}"#, Some(SettingsFormat::Json)),
        (r#"{"version": 1}"#, Some(SettingsFormat::Json)),
    ];
    for (raw, fmt) in no_theme_cases {
        assert!(
            UserPrefsStore::parse_theme_patch(raw, fmt).is_err(),
            "expected parse_theme_patch error for: {raw}"
        );
    }

    let store = UserPrefsStore::in_memory().expect("in_memory store");
    store.set_theme_pref("name", "nord").expect("initial theme");

    let bad_patch = SettingsPatch {
        version: 1,
        preferences: BTreeMap::from([
            (
                "theme".to_string(),
                SectionPatch::Entries(BTreeMap::from([(
                    "name".to_string(),
                    Some("dracula".to_string()),
                )])),
            ),
            (
                "ui".to_string(),
                SectionPatch::Entries(BTreeMap::from([(
                    "".to_string(),
                    Some("invalid-empty-key".to_string()),
                )])),
            ),
        ]),
    };
    let err = store
        .import_patch(&bad_patch, None, false)
        .expect_err("expected empty key rejection in import_patch");
    assert!(matches!(err, CoreError::SyncError(_)));
    assert_eq!(
        store.theme().expect("transaction rolled back"),
        Some("nord".to_string())
    );
}

#[test]
fn test_edge_cases_serde_crlf_unicode_escapes_and_theme_null_resets() {
    assert_eq!(
        serde_json::to_string(&SettingsFormat::Toml).expect("serialize toml format"),
        "\"toml\""
    );
    assert_eq!(
        serde_json::to_string(&SettingsFormat::Json).expect("serialize json format"),
        "\"json\""
    );

    let crlf_toml = "[theme]\r\nname = \"\\u004Eord\"\r\naccent = \"#88c0d0\"\r\n";
    let patch = UserPrefsStore::parse_settings_patch(crlf_toml, None).expect("parse crlf toml");
    assert_eq!(
        patch.preferences.get("theme"),
        Some(&SectionPatch::Entries(BTreeMap::from([
            ("accent".to_string(), Some("#88c0d0".to_string())),
            ("name".to_string(), Some("Nord".to_string())),
        ])))
    );

    let empty_section_toml =
        UserPrefsStore::format_single_section("keybinding", &BTreeMap::new(), SettingsFormat::Toml)
            .expect("format empty section toml");
    assert_eq!(empty_section_toml, "[keybinding]\n");

    let theme_null_toml = "version = 1\ntheme = null\n[keybinding]\nquit = \"q\"\n";
    let theme_patch_toml =
        UserPrefsStore::parse_theme_patch(theme_null_toml, None).expect("theme = null toml");
    assert_eq!(
        theme_patch_toml.preferences,
        BTreeMap::from([("theme".to_string(), SectionPatch::ResetToDefault)])
    );

    let theme_null_json = r#"{"theme": null, "keybinding": {"quit": "q"}}"#;
    let theme_patch_json =
        UserPrefsStore::parse_theme_patch(theme_null_json, None).expect("theme: null json");
    assert_eq!(
        theme_patch_json.preferences,
        BTreeMap::from([("theme".to_string(), SectionPatch::ResetToDefault)])
    );
}
