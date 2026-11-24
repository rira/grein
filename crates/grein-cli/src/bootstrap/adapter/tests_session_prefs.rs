use std::time::Duration;

use clap::Parser;
use crate::args::{Cli, Command};
use crate::engine::InProcessEngine;
use grein_tui::engine_port::{TuiEngineEvent, TuiEnginePort, UserPreferencesSnapshot};

use super::{EngineClientPort, InitialSessionMode};

#[test]
fn test_cli_agent_flag_defaults_false_and_parses_true() {
    let default_cli = Cli::try_parse_from(["grein"]).expect("parse default grein");
    assert!(!default_cli.agent);

    let agent_cli = Cli::try_parse_from(["grein", "--agent"]).expect("parse grein --agent");
    assert!(agent_cli.agent);

    let attach_agent =
        Cli::try_parse_from(["grein", "attach", "--agent"]).expect("parse attach --agent");
    assert!(attach_agent.agent);

    let standalone_agent = Cli::try_parse_from(["grein", "--agent", "standalone"])
        .expect("parse --agent standalone");
    assert!(standalone_agent.agent);
}

#[tokio::test]
async fn test_engine_client_port_uses_injected_in_memory_preference_repository() {
    let temp = tempfile::tempdir().expect("tempdir");
    let session_db = temp.path().join("session.db");
    let engine = InProcessEngine::open_with_config(&session_db, temp.path().to_path_buf(), None)
        .expect("open engine");
    let prefs = std::sync::Arc::new(grein_core::UserPrefsStore::in_memory().expect("in-memory prefs"));
    let port = EngineClientPort::new_with_mode_and_prefs(
        engine,
        InitialSessionMode::Agent,
        prefs,
    );

    assert_eq!(
        port.load_preferences().await.expect("initial load"),
        UserPreferencesSnapshot::default()
    );
    port.save_theme_preference("nord").await.expect("theme");
    port.save_keybinding_preference("quit", "q")
        .await
        .expect("keybinding");
    port.save_ui_preference("sidebar_width", "34")
        .await
        .expect("ui");

    let snap = port.load_preferences().await.expect("reload");
    assert_eq!(snap.theme.as_deref(), Some("nord"));
    assert_eq!(snap.keybindings.get("quit").map(String::as_str), Some("q"));
    assert_eq!(snap.ui.get("sidebar_width").map(String::as_str), Some("34"));

    port.remove_keybinding_preference("quit")
        .await
        .expect("remove keybinding");
    let reloaded = port.load_preferences().await.expect("reload 2");
    assert!(!reloaded.keybindings.contains_key("quit"));
}

#[tokio::test]
async fn test_engine_client_port_terminal_mode_empty_db_hydrates_root_terminal_only() {
    let temp = tempfile::tempdir().expect("tempdir");
    let canonical_dir = std::fs::canonicalize(temp.path()).expect("canonical tempdir");
    let db_path = temp.path().join("term_first.db");

    let engine =
        InProcessEngine::open_with_config(&db_path, canonical_dir.clone(), None).expect("open");
    let port = EngineClientPort::new_with_mode(engine, InitialSessionMode::Terminal);

    let session = port.hydrate_session().await.expect("hydrate terminal mode");
    assert_eq!(session.threads.len(), 1);
    assert_eq!(session.active_thread_id, session.threads[0].id);
    assert_eq!(session.threads[0].title, "Terminal");
    assert!(session.threads[0].turns.is_empty());

    let meta = port.thread_metadata().await.expect("thread_metadata");
    assert_eq!(meta.len(), 1);
    assert_eq!(meta[0].thread_id, session.active_thread_id);
    assert_eq!(meta[0].kind, grein_core::model::ThreadKind::Terminal);
    assert!(meta[0].parent_thread_id.is_none());
    assert_eq!(
        meta[0].cwd.as_deref(),
        Some(canonical_dir.to_string_lossy().as_ref())
    );

    let mut rx = port.events_rx.lock().await.take().expect("events_rx");
    let mut events = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        events.push(ev);
    }
    assert!(
        events.iter().any(|ev| matches!(
            ev,
            TuiEngineEvent::ThreadMetadataUpdated { meta }
                if meta.len() == 1
                    && meta[0].thread_id == session.active_thread_id
                    && meta[0].kind == grein_core::model::ThreadKind::Terminal
        )),
        "offline hydrate_session must emit ThreadMetadataUpdated for root terminal: {events:?}"
    );
}

#[tokio::test]
async fn test_engine_client_port_agent_mode_empty_db_hydrates_main_agent() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("agent_first.db");

    let engine = InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
        .expect("open");
    let port = EngineClientPort::new_with_mode(engine, InitialSessionMode::Agent);

    let session = port.hydrate_session().await.expect("hydrate agent mode");
    assert_eq!(session.threads.len(), 1);
    assert_eq!(session.active_thread_id, session.threads[0].id);
    assert_eq!(session.threads[0].title, "Main");

    let meta = port.thread_metadata().await.expect("thread_metadata");
    assert_eq!(meta.len(), 1);
    assert_eq!(meta[0].kind, grein_core::model::ThreadKind::Agent);
}

#[tokio::test]
async fn test_engine_client_port_reopen_terminal_only_db_preserves_threads_without_main() {
    let temp = tempfile::tempdir().expect("tempdir");
    let db_path = temp.path().join("reopen_term_only.db");

    let term_id = {
        let engine =
            InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
                .expect("open 1");
        let port = EngineClientPort::new_with_mode(engine, InitialSessionMode::Terminal);
        let session = port.hydrate_session().await.expect("hydrate 1");
        assert_eq!(session.threads.len(), 1);
        session.active_thread_id
    };

    for mode in [InitialSessionMode::Terminal, InitialSessionMode::Agent] {
        let reopened =
            InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None)
                .expect("reopen");
        let port = EngineClientPort::new_with_mode(reopened, mode);
        let session = port.hydrate_session().await.expect("rehydrate");
        assert_eq!(
            session.threads.len(),
            1,
            "reopening terminal-only DB in {mode:?} mode must not inject Main agent"
        );
        assert_eq!(session.active_thread_id, term_id);
        assert_eq!(session.threads[0].id, term_id);
        assert_eq!(session.threads[0].title, "Terminal");

        let meta = port.thread_metadata().await.expect("meta");
        assert_eq!(meta.len(), 1);
        assert_eq!(meta[0].kind, grein_core::model::ThreadKind::Terminal);
    }
}

#[tokio::test]
async fn test_engine_client_port_terminal_mode_with_live_stream_hydrates_cleanly() {
    let temp = tempfile::tempdir().expect("tempdir");
    let canonical_dir = std::fs::canonicalize(temp.path()).expect("canonical tempdir");
    let db_path = temp.path().join("live_term_first.db");

    let engine =
        InProcessEngine::open_with_config(&db_path, canonical_dir.clone(), None).expect("open");
    let port = EngineClientPort::new_with_mode(engine, InitialSessionMode::Terminal);

    let mut rx = port.subscribe_events().await.expect("subscribe");
    let session = port.hydrate_session().await.expect("hydrate");
    assert_eq!(session.threads.len(), 1);
    assert_eq!(session.threads[0].title, "Terminal");
    assert_eq!(session.active_thread_id, session.threads[0].id);

    let mut events = Vec::new();
    while let Ok(Some(ev)) =
        tokio::time::timeout(Duration::from_millis(100), rx.recv()).await
    {
        events.push(ev);
    }
    assert!(
        events.iter().any(|ev| matches!(
            ev,
            TuiEngineEvent::ThreadMetadataUpdated { meta }
                if meta.iter().any(|m| {
                    m.thread_id == session.active_thread_id
                        && m.kind == grein_core::model::ThreadKind::Terminal
                })
        )),
        "live stream terminal-first hydration must deliver ThreadMetadataUpdated: {events:?}"
    );
}

static PREFS_ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct EnvVarGuard {
    saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl EnvVarGuard {
    fn capture(keys: &[&'static str]) -> Self {
        let saved = keys
            .iter()
            .map(|&k| (k, std::env::var_os(k)))
            .collect();
        Self { saved }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        for (k, val) in self.saved.drain(..) {
            match val {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
    }
}

#[test]
fn test_cli_parses_doctor_settings_and_theme_subcommands() {
    let doc = Cli::try_parse_from(["grein", "doctor", "--json"]).expect("parse doctor");
    assert_eq!(
        doc.command,
        Some(Command::Doctor(crate::args::DoctorArgs { json: true }))
    );

    let set_exp = Cli::try_parse_from([
        "grein",
        "settings",
        "export",
        "out.json",
        "--only",
        "theme,ui",
    ])
    .expect("parse settings export");
    match set_exp.command {
        Some(Command::Settings(s)) => match s.command {
            crate::args::SettingsCommand::Export(args) => {
                assert_eq!(args.file.as_deref(), Some(std::path::Path::new("out.json")));
                assert_eq!(args.format, None);
                assert_eq!(args.only, vec!["theme".to_string(), "ui".to_string()]);
            }
            other => panic!("unexpected settings command: {other:?}"),
        },
        other => panic!("unexpected command: {other:?}"),
    }

    let set_imp = Cli::try_parse_from([
        "grein",
        "settings",
        "import",
        "-",
        "--format",
        "json",
        "--replace",
    ])
    .expect("parse settings import");
    match set_imp.command {
        Some(Command::Settings(s)) => match s.command {
            crate::args::SettingsCommand::Import(args) => {
                assert_eq!(args.file, std::path::PathBuf::from("-"));
                assert_eq!(args.format, crate::args::InputFormatArg::Json);
                assert!(args.replace);
            }
            other => panic!("unexpected settings command: {other:?}"),
        },
        other => panic!("unexpected command: {other:?}"),
    }

    let theme_exp = Cli::try_parse_from(["grein", "theme", "export", "--format", "toml"])
        .expect("parse theme export");
    match theme_exp.command {
        Some(Command::Theme(t)) => match t.command {
            crate::args::ThemeCommand::Export(args) => {
                assert_eq!(args.file, None);
                assert_eq!(args.format, Some(crate::args::OutputFormatArg::Toml));
            }
            other => panic!("unexpected theme command: {other:?}"),
        },
        other => panic!("unexpected command: {other:?}"),
    }

    let theme_imp =
        Cli::try_parse_from(["grein", "theme", "import", "colleague.toml", "--replace"])
            .expect("parse theme import");
    match theme_imp.command {
        Some(Command::Theme(t)) => match t.command {
            crate::args::ThemeCommand::Import(args) => {
                assert_eq!(args.file, std::path::PathBuf::from("colleague.toml"));
                assert_eq!(args.format, crate::args::InputFormatArg::Auto);
                assert!(args.replace);
            }
            other => panic!("unexpected theme command: {other:?}"),
        },
        other => panic!("unexpected command: {other:?}"),
    }
}

#[tokio::test]
async fn test_engine_client_port_global_preferences_round_trip() {
    let _lock = PREFS_ENV_LOCK.lock().await;
    let _guard = EnvVarGuard::capture(&["GREIN_CONFIG_DB", "XDG_CONFIG_HOME", "HOME"]);

    let temp = tempfile::tempdir().expect("tempdir");
    let prefs_db = temp.path().join("nested").join("prefs.db");
    std::env::set_var("GREIN_CONFIG_DB", &prefs_db);
    std::env::remove_var("XDG_CONFIG_HOME");

    let session_db = temp.path().join("session.db");
    let engine =
        InProcessEngine::open_with_config(&session_db, temp.path().to_path_buf(), None)
            .expect("open engine");
    let port = EngineClientPort::new(engine);

    let initial = port.load_preferences().await.expect("initial load_preferences");
    assert_eq!(initial, UserPreferencesSnapshot::default());
    assert!(
        !prefs_db.exists(),
        "load_preferences must not eagerly create prefs DB when absent"
    );

    port.save_ui_preference("sidebar_width", "36")
        .await
        .expect("save ui pref");
    port.save_keybinding_preference("quit", "q")
        .await
        .expect("save keybinding quit");
    port.save_keybinding_preference("focus_next", "Tab")
        .await
        .expect("save keybinding focus_next");
    port.save_theme_preference("nord")
        .await
        .expect("save theme nord");

    grein_core::UserPrefsStore::open(&prefs_db)
        .expect("open store")
        .set_theme_pref("accent", "#abcdef")
        .expect("set accent");

    let hydrated = port.load_preferences().await.expect("load hydrated prefs");
    assert_eq!(hydrated.theme.as_deref(), Some("nord"));
    assert_eq!(
        hydrated.theme_overrides.get("accent").map(String::as_str),
        Some("#abcdef")
    );
    assert_eq!(
        hydrated.keybindings.get("quit").map(String::as_str),
        Some("q")
    );
    assert_eq!(
        hydrated.keybindings.get("focus_next").map(String::as_str),
        Some("Tab")
    );
    assert_eq!(
        hydrated.ui.get("sidebar_width").map(String::as_str),
        Some("36")
    );

    // Switching theme clears previous custom theme overrides cleanly.
    port.save_theme_preference("dracula")
        .await
        .expect("save theme dracula");
    port.remove_keybinding_preference("quit")
        .await
        .expect("remove keybinding quit");

    let updated = port.load_preferences().await.expect("load updated prefs");
    assert_eq!(updated.theme.as_deref(), Some("dracula"));
    assert!(
        !updated.theme_overrides.contains_key("accent"),
        "save_theme_preference must clear prior theme color overrides: {:?}",
        updated.theme_overrides
    );
    assert!(!updated.keybindings.contains_key("quit"));
    assert_eq!(
        updated.keybindings.get("focus_next").map(String::as_str),
        Some("Tab")
    );
    assert_eq!(
        updated.ui.get("sidebar_width").map(String::as_str),
        Some("36")
    );
}

#[tokio::test]
async fn test_engine_client_port_preferences_handle_unset_and_unwritable_home_safely() {
    let _lock = PREFS_ENV_LOCK.lock().await;
    let _guard = EnvVarGuard::capture(&["GREIN_CONFIG_DB", "XDG_CONFIG_HOME", "HOME"]);

    let temp = tempfile::tempdir().expect("tempdir");
    let session_db = temp.path().join("session.db");
    let engine =
        InProcessEngine::open_with_config(&session_db, temp.path().to_path_buf(), None)
            .expect("open engine");
    let port = EngineClientPort::new(engine);

    std::env::remove_var("GREIN_CONFIG_DB");
    std::env::remove_var("XDG_CONFIG_HOME");
    std::env::remove_var("HOME");

    assert_eq!(
        port.load_preferences().await.expect("load with unset env"),
        UserPreferencesSnapshot::default()
    );
    port.save_theme_preference("nord")
        .await
        .expect("save_theme_preference with None path");
    port.save_keybinding_preference("quit", "q")
        .await
        .expect("save_keybinding_preference with None path");
    port.remove_keybinding_preference("quit")
        .await
        .expect("remove_keybinding_preference with None path");
    port.save_ui_preference("sidebar_width", "32")
        .await
        .expect("save_ui_preference with None path");

    // Simulates Nix homeless-shelter read-only HOME when prefs.db does not exist.
    std::env::set_var("HOME", "/nonexistent/grein-homeless-shelter");
    assert_eq!(
        port.load_preferences()
            .await
            .expect("load with nonexistent HOME"),
        UserPreferencesSnapshot::default()
    );
}
