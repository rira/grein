use std::ffi::OsString;
use std::fs;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use grein_cli::args::{
    AgentsArgs, DoctorArgs, InputFormatArg, OutputFormatArg, SettingsArgs, SettingsCommand,
    SettingsExportArgs, SettingsImportArgs, StatusArgs, ThemeArgs, ThemeCommand, ThemeExportArgs,
    ThemeImportArgs, ThreadsArgs,
};
use grein_cli::client::connect_to_daemon;
use grein_cli::daemon::run_daemon_with_engine;
use grein_cli::discovery::{
    discover_acp_agents, resolve_agents_configuration, AcpAgentKind, DiscoveryError,
};
use grein_cli::doctor::{collect_doctor_report, handle_doctor, CheckStatus};
use grein_cli::engine::InProcessEngine;
use grein_cli::headless::{handle_agents, handle_status, handle_threads};
use grein_cli::settings_cmd::{handle_settings, handle_theme};
use grein_cli::standalone::{
    create_in_process_client, run_standalone, spawn_standalone_pair, StandaloneError,
};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::sync::oneshot;

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct EnvCwdGuard {
    saved_env: Vec<(&'static str, Option<OsString>)>,
    saved_cwd: PathBuf,
}

impl EnvCwdGuard {
    fn capture(keys: &[&'static str]) -> Self {
        Self {
            saved_env: keys.iter().map(|&k| (k, std::env::var_os(k))).collect(),
            saved_cwd: std::env::current_dir().expect("cwd"),
        }
    }
}

impl Drop for EnvCwdGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.saved_cwd);
        for (k, v) in self.saved_env.drain(..) {
            match v {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path).expect("meta").permissions();
    perms.set_mode(mode);
    fs::set_permissions(path, perms).expect("chmod");
}

#[test]
fn test_doctor_checks_non_json_formatting_and_error_report() {
    let _lock = ENV_LOCK.blocking_lock();
    let _guard = EnvCwdGuard::capture(&[
        "SHELL",
        "GREIN_ACP_BIN",
        "PATH",
        "TERM",
        "COLORTERM",
        "LC_ALL",
        "LC_CTYPE",
        "LANG",
        "GREIN_CONFIG_DB",
        "XDG_CONFIG_HOME",
        "HOME",
    ]);

    let temp = tempfile::tempdir().expect("tempdir");
    let ws_dir = temp.path().join("ws");
    let state_dir = ws_dir.join(".grein");
    fs::create_dir_all(&state_dir).expect("mkdir .grein");
    std::env::set_current_dir(&ws_dir).expect("set_current_dir");

    // Bind a reachable socket at .grein/daemon.sock and place a corrupt dir at .grein/session.db
    let sock_path = state_dir.join("daemon.sock");
    let _sock_listener = UnixListener::bind(&sock_path).expect("bind socket");
    fs::create_dir_all(state_dir.join("session.db")).expect("corrupt session.db");

    let acp_bin = temp.path().join("fake-acp");
    fs::write(&acp_bin, "#!/bin/sh\nexit 0\n").expect("write acp bin");

    // 1. Trigger WARN on shell (missing), TERM=dumb, non-UTF8 locale, missing GREIN_ACP_BIN, and Error on corrupt session.db
    std::env::set_var("SHELL", "/nonexistent/grein-missing-shell");
    std::env::set_var("GREIN_ACP_BIN", "/nonexistent/grein-missing-acp");
    std::env::set_var("TERM", "dumb");
    std::env::remove_var("COLORTERM");
    std::env::set_var("LC_ALL", "C");
    std::env::remove_var("LC_CTYPE");
    std::env::remove_var("LANG");
    let prefs_db = temp.path().join("prefs.db");
    std::env::set_var("GREIN_CONFIG_DB", &prefs_db);

    let bad_report = collect_doctor_report();
    assert!(!bad_report.ok);
    let find_check = |name: &str| {
        bad_report
            .checks
            .iter()
            .find(|c| c.name == name)
            .expect(name)
            .clone()
    };
    assert_eq!(find_check("shell").status, CheckStatus::Warn);
    assert_eq!(find_check("acp_agents").status, CheckStatus::Warn);
    assert_eq!(find_check("terminal_env").status, CheckStatus::Warn);
    assert_eq!(find_check("workspace_state").status, CheckStatus::Error);

    // Non-JSON handle_doctor prints [OK  ]/[WARN]/[ERR ] and returns Err when report.ok == false
    let err = handle_doctor(&DoctorArgs { json: false }).expect_err("error report must return Err");
    assert!(err.to_string().contains("critical errors"));

    // 2. Unset SHELL, empty PATH, unset TERM, existing GREIN_ACP_BIN, valid session.db + reachable socket -> report.ok == true
    fs::remove_dir_all(state_dir.join("session.db")).expect("rm corrupt db dir");
    let _engine =
        InProcessEngine::open_with_config(&state_dir.join("session.db"), ws_dir.clone(), None)
            .expect("create valid session.db");

    std::env::remove_var("SHELL");
    std::env::set_var("GREIN_ACP_BIN", &acp_bin);
    std::env::set_var("TERM", "xterm-256color");
    std::env::set_var("COLORTERM", "truecolor");
    std::env::set_var("LC_ALL", "en_US.UTF-8");

    let ok_report = collect_doctor_report();
    assert!(ok_report.ok, "expected ok report: {ok_report:?}");
    let ws_check = ok_report
        .checks
        .iter()
        .find(|c| c.name == "workspace_state")
        .unwrap();
    assert_eq!(ws_check.status, CheckStatus::Ok);
    assert!(ws_check.detail.contains("reachable=true"));
    assert_eq!(
        ok_report
            .checks
            .iter()
            .find(|c| c.name == "shell")
            .unwrap()
            .status,
        CheckStatus::Warn
    );
    assert_eq!(
        ok_report
            .checks
            .iter()
            .find(|c| c.name == "acp_agents")
            .unwrap()
            .status,
        CheckStatus::Ok
    );
    assert_eq!(
        ok_report
            .checks
            .iter()
            .find(|c| c.name == "terminal_env")
            .unwrap()
            .status,
        CheckStatus::Ok
    );

    handle_doctor(&DoctorArgs { json: false }).expect("non-json ok doctor");

    // Empty PATH with no GREIN_ACP_BIN -> acp_agents Warn & missing git Error
    std::env::remove_var("GREIN_ACP_BIN");
    std::env::set_var("PATH", temp.path().join("empty_bin"));
    std::env::remove_var("TERM");
    let empty_acp_report = collect_doctor_report();
    assert_eq!(
        empty_acp_report
            .checks
            .iter()
            .find(|c| c.name == "acp_agents")
            .unwrap()
            .status,
        CheckStatus::Warn
    );
    assert_eq!(
        empty_acp_report
            .checks
            .iter()
            .find(|c| c.name == "terminal_env")
            .unwrap()
            .status,
        CheckStatus::Warn
    );
    assert!(handle_doctor(&DoctorArgs { json: false }).is_err());
}

#[test]
fn test_settings_cmd_error_paths_nested_dirs_and_stdout() {
    let _lock = ENV_LOCK.blocking_lock();
    let _guard = EnvCwdGuard::capture(&["GREIN_CONFIG_DB", "XDG_CONFIG_HOME", "HOME"]);

    let temp = tempfile::tempdir().expect("tempdir");
    let valid_toml = temp.path().join("valid.toml");
    fs::write(&valid_toml, "[theme]\nname = \"nord\"\n").expect("write toml");

    // Missing HOME/XDG/GREIN_CONFIG_DB: export falls back to in_memory, import returns NotFound Err
    std::env::remove_var("GREIN_CONFIG_DB");
    std::env::remove_var("XDG_CONFIG_HOME");
    std::env::remove_var("HOME");

    handle_settings(&SettingsArgs {
        command: SettingsCommand::Export(SettingsExportArgs {
            file: None,
            format: None,
            only: vec!["   ".into()],
        }),
    })
    .expect("export with no env falls back to in_memory");

    handle_theme(&ThemeArgs {
        command: ThemeCommand::Export(ThemeExportArgs {
            file: Some(PathBuf::from("-")),
            format: Some(OutputFormatArg::Json),
        }),
    })
    .expect("theme export to stdout");

    assert!(handle_settings(&SettingsArgs {
        command: SettingsCommand::Import(SettingsImportArgs {
            file: valid_toml.clone(),
            format: InputFormatArg::Auto,
            only: Vec::new(),
            replace: false,
        }),
    })
    .is_err());

    assert!(handle_theme(&ThemeArgs {
        command: ThemeCommand::Import(ThemeImportArgs {
            file: valid_toml.clone(),
            format: InputFormatArg::Auto,
            replace: false,
        }),
    })
    .is_err());

    // Configure GREIN_CONFIG_DB
    let prefs_db = temp.path().join("prefs.db");
    std::env::set_var("GREIN_CONFIG_DB", &prefs_db);

    // Missing input file errors
    let missing_file = temp.path().join("does-not-exist.toml");
    assert!(handle_settings(&SettingsArgs {
        command: SettingsCommand::Import(SettingsImportArgs {
            file: missing_file.clone(),
            format: InputFormatArg::Auto,
            only: Vec::new(),
            replace: false,
        }),
    })
    .is_err());
    assert!(handle_theme(&ThemeArgs {
        command: ThemeCommand::Import(ThemeImportArgs {
            file: missing_file,
            format: InputFormatArg::Auto,
            replace: false,
        }),
    })
    .is_err());

    // Invalid TOML & invalid JSON & format mismatch errors
    let bad_toml = temp.path().join("bad.toml");
    fs::write(&bad_toml, "not = [valid\n").expect("write bad toml");
    assert!(handle_settings(&SettingsArgs {
        command: SettingsCommand::Import(SettingsImportArgs {
            file: bad_toml.clone(),
            format: InputFormatArg::Toml,
            only: Vec::new(),
            replace: false,
        }),
    })
    .is_err());

    let bad_json = temp.path().join("bad.json");
    fs::write(&bad_json, "{not json").expect("write bad json");
    assert!(handle_theme(&ThemeArgs {
        command: ThemeCommand::Import(ThemeImportArgs {
            file: bad_json,
            format: InputFormatArg::Json,
            replace: false,
        }),
    })
    .is_err());

    // Valid import then export to nested non-existent parent dirs
    handle_settings(&SettingsArgs {
        command: SettingsCommand::Import(SettingsImportArgs {
            file: valid_toml.clone(),
            format: InputFormatArg::Toml,
            only: Vec::new(),
            replace: false,
        }),
    })
    .expect("import valid toml");

    let nested_settings = temp.path().join("deep/a/settings.toml");
    handle_settings(&SettingsArgs {
        command: SettingsCommand::Export(SettingsExportArgs {
            file: Some(nested_settings.clone()),
            format: None,
            only: Vec::new(),
        }),
    })
    .expect("export nested settings");
    assert!(nested_settings.is_file());

    let nested_theme = temp.path().join("deep/b/theme.json");
    handle_theme(&ThemeArgs {
        command: ThemeCommand::Export(ThemeExportArgs {
            file: Some(nested_theme.clone()),
            format: None,
        }),
    })
    .expect("export nested theme");
    assert!(nested_theme.is_file());

    // Import theme with TOML via explicit format
    handle_theme(&ThemeArgs {
        command: ThemeCommand::Import(ThemeImportArgs {
            file: valid_toml,
            format: InputFormatArg::Toml,
            replace: true,
        }),
    })
    .expect("theme import");
}

#[tokio::test]
async fn test_headless_non_json_tables_corrupt_db_reachable_socket_and_agents_errors() {
    let _lock = ENV_LOCK.lock().await;
    let _guard = EnvCwdGuard::capture(&["PATH"]);

    let temp = tempfile::tempdir().expect("tempdir");
    let missing_db = temp.path().join("missing.db");
    let sock_path = temp.path().join("live.sock");
    let _listener = UnixListener::bind(&sock_path).expect("bind live socket");

    // 1. handle_threads non-JSON on empty/missing db -> "No threads found."
    handle_threads(&ThreadsArgs {
        json: false,
        db: Some(missing_db.clone()),
    })
    .await
    .expect("empty non-json threads");

    // 2. handle_threads non-JSON with Main & custom-named branch
    let db_path = temp.path().join("session.db");
    {
        let engine =
            InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None).unwrap();
        let main = engine.ensure_main_thread().await.unwrap();
        engine
            .fork_thread_with_title(Some(&main.thread_id), Some("named-branch"))
            .await
            .unwrap();
    }
    handle_threads(&ThreadsArgs {
        json: false,
        db: Some(db_path.clone()),
    })
    .await
    .expect("populated non-json threads");

    // 3. handle_threads on corrupt DB -> Err
    let corrupt_db = temp.path().join("corrupt.db");
    fs::create_dir_all(&corrupt_db).expect("corrupt db dir");
    assert!(handle_threads(&ThreadsArgs {
        json: false,
        db: Some(corrupt_db.clone()),
    })
    .await
    .is_err());

    // 4. handle_status non-JSON with reachable socket + valid DB, and with corrupt DB
    handle_status(&StatusArgs {
        json: false,
        socket: Some(sock_path.clone()),
        db: Some(db_path),
    })
    .await
    .expect("status non-json valid");

    handle_status(&StatusArgs {
        json: false,
        socket: Some(sock_path),
        db: Some(corrupt_db),
    })
    .await
    .expect("status non-json corrupt db");

    // 5. handle_agents: empty PATH -> "No ACP agents found.", valid config -> table, malformed config -> Err
    std::env::set_var("PATH", temp.path().join("no_bins"));
    handle_agents(&AgentsArgs {
        json: false,
        config: None,
    })
    .await
    .expect("no agents found non-json");

    let agents_cfg = temp.path().join("agents.toml");
    fs::write(
        &agents_cfg,
        "[[agents]]\nname=\"agy\"\nexecutable=\"/bin/agy\"\n[[agents]]\nname=\"claude\"\nexecutable=\"/bin/claude\"\n[[agents]]\nname=\"custom\"\nexecutable=\"/bin/custom\"\n",
    )
    .unwrap();
    handle_agents(&AgentsArgs {
        json: false,
        config: Some(agents_cfg),
    })
    .await
    .expect("agents table non-json");

    let bad_cfg = temp.path().join("bad_agents.toml");
    fs::write(&bad_cfg, "[[agents]\n").unwrap();
    assert!(handle_agents(&AgentsArgs {
        json: false,
        config: Some(bad_cfg),
    })
    .await
    .is_err());
}

#[test]
fn test_discovery_ignores_non_executable_and_directories_and_toml_edge_cases() {
    let temp = tempfile::tempdir().expect("tempdir");
    let bin_dir = temp.path().join("bin");
    fs::create_dir_all(&bin_dir).expect("mkdir bin");

    // Non-executable file named `agy` -> ignored
    let non_exec = bin_dir.join("agy");
    fs::write(&non_exec, "#!/bin/sh\n").unwrap();
    set_mode(&non_exec, 0o644);

    // Directory named `claude-agent-acp` -> ignored; plain executable `claude` -> ignored
    fs::create_dir_all(bin_dir.join("claude-agent-acp")).unwrap();
    set_mode(&bin_dir.join("claude-agent-acp"), 0o755);
    let plain_claude = bin_dir.join("claude");
    fs::write(&plain_claude, "#!/bin/sh\n").unwrap();
    set_mode(&plain_claude, 0o755);

    let search = bin_dir.to_string_lossy().to_string();
    assert!(discover_acp_agents(Some(&search)).is_empty());

    // Scalar command = "...", TOML escapes (\n \t \r \\ \"), single quotes, Value::Other
    let valid_cfg = temp.path().join("scalar_escapes.toml");
    fs::write(
        &valid_cfg,
        "[[agents]]\nname = 'custom-one'\ncommand = \"/opt/my\\tbin\\n\\\\\\\"x\\\"\\r\"\nretries = 3\n[[agents]]\nexecutable = \"\"\ncommand = []\n[[agents]]\nkind = \"claude\"\ncommand = \"/usr/bin/claude\"\n",
    )
    .unwrap();
    let resolved = resolve_agents_configuration(Some(&valid_cfg), Some(&search)).expect("resolve");
    assert_eq!(resolved.len(), 2);
    assert_eq!(resolved[0].kind, AcpAgentKind::Custom);
    assert_eq!(
        resolved[0].executable_path,
        PathBuf::from("/opt/my\tbin\n\\\"x\"\r")
    );
    assert!(resolved[0].args.is_empty());
    assert_eq!(resolved[1].kind, AcpAgentKind::Claude);
    assert!(resolved[1].args.is_empty());

    // Parser error branches
    let bad_cases = [
        ("invalid escape", "[[agents]]\nname = \"bad\\q\"\n"),
        ("unterminated string", "[[agents]]\nname = \"unterminated\n"),
        ("trailing after string", "[[agents]]\nname = \"ok\" extra\n"),
        ("missing value", "[[agents]]\nname = \n"),
        ("unterminated array", "[[agents]]\ncommand = [\"a\"\n"),
        ("bad array sep", "[[agents]]\ncommand = [\"a\" \"b\"]\n"),
        ("trailing after array", "[[agents]]\ncommand = [\"a\"] extra\n"),
        ("malformed table", "[agent\nname=\"a\"\n"),
        ("empty table header", "[]\n"),
        ("nested bracket header", "[a[b]]\n"),
        ("missing equals", "[[agents]]\njust_a_key\n"),
        ("empty key", "[[agents]]\n = \"val\"\n"),
    ];
    for (label, body) in bad_cases {
        let p = temp.path().join(format!("{}.toml", label.replace(' ', "_")));
        fs::write(&p, body).unwrap();
        let res = resolve_agents_configuration(Some(&p), None);
        assert!(
            matches!(res, Err(DiscoveryError::IoError(_))),
            "{label} should return IoError, got {res:?}"
        );
    }
}

#[tokio::test]
async fn test_daemon_legacy_lines_stale_socket_cleanup_and_client_helpers() {
    let _lock = ENV_LOCK.lock().await;
    let temp = tempfile::tempdir().expect("tempdir");
    let sock_path = temp.path().join("daemon.sock");
    let db_path = temp.path().join("session.db");

    // Pre-create stale socket file to exercise cleanup on startup
    fs::write(&sock_path, "stale").expect("write stale file");

    let engine = Arc::new(
        InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None).expect("open"),
    );
    let sock_clone = sock_path.clone();
    let engine_clone = Arc::clone(&engine);
    let server = tokio::spawn(async move {
        let _ = run_daemon_with_engine(sock_clone, engine_clone).await;
    });

    for _ in 0..50 {
        if sock_path.exists() && UnixStream::connect(&sock_path).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Legacy line commands via DaemonClient + raw stream
    let mut client = connect_to_daemon(&sock_path).await.expect("connect");
    assert_eq!(client.ping().await.unwrap(), "pong");
    let turn_reply = client.send_turn("legacy prompt").await.unwrap();
    assert!(turn_reply.starts_with("ok:"));
    let entries_reply = client.list_entries().await.unwrap();
    assert_eq!(entries_reply, "entries:1");

    // Raw stream for blank line & fallback "ok"
    let mut raw = UnixStream::connect(&sock_path).await.unwrap();
    raw.write_all(b"\nunknown_legacy_cmd\n").await.unwrap();
    raw.flush().await.unwrap();
    let mut raw_reader = BufReader::new(raw);
    let mut fallback = String::new();
    raw_reader.read_line(&mut fallback).await.unwrap();
    assert_eq!(fallback, "ok\n");

    // DaemonClient JSON-RPC helpers
    assert_eq!(client.session_ping().await.unwrap()["result"]["pong"], true);
    let main = client.session_ensure_main_thread().await.unwrap();
    let tid = main["result"]["thread_id"].as_str().unwrap().to_string();
    assert!(client
        .session_send_turn(None, "turn no tid")
        .await
        .unwrap()["result"]["node_id"]
        .is_string());
    assert!(client
        .session_send_turn(Some(&tid), "turn with tid")
        .await
        .unwrap()["error"]
        .is_object());
    let forked = client
        .session_fork_thread(None, Some("raw fork"))
        .await
        .unwrap();
    assert!(forked["result"]["thread_id"].is_string());
    assert!(client
        .session_get_transcript(None)
        .await
        .unwrap()["result"]
        .is_array());
    assert!(client
        .session_get_transcript(Some(&tid))
        .await
        .unwrap()["result"]
        .is_array());
    assert_eq!(
        client.session_cancel(&tid).await.unwrap()["result"]["cancelled"],
        true
    );
    assert_eq!(
        client
            .session_shell(&tid, "printf 'ok'")
            .await
            .unwrap()["result"]["stdout"],
        "ok"
    );
    assert_eq!(
        client
            .permissions_resolve("req-1", false)
            .await
            .unwrap()["result"]["allow"],
        false
    );

    server.abort();
}

#[tokio::test]
async fn test_daemon_legacy_send_turn_error_when_only_terminal_thread_exists() {
    let temp = tempfile::tempdir().expect("tempdir");
    let sock_path = temp.path().join("term_only.sock");
    let db_path = temp.path().join("term_only.db");

    let engine = Arc::new(
        InProcessEngine::open_with_config(&db_path, temp.path().to_path_buf(), None).expect("open"),
    );
    engine
        .create_terminal(None, Some("only-term"), None, 24, 80)
        .await
        .expect("create terminal");

    let sock_clone = sock_path.clone();
    let server = tokio::spawn(async move {
        let _ = run_daemon_with_engine(sock_clone, engine).await;
    });
    for _ in 0..50 {
        if sock_path.exists() && UnixStream::connect(&sock_path).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let mut client = connect_to_daemon(&sock_path).await.expect("connect");
    let reply = client.send_turn("should fail").await.unwrap();
    assert!(reply.starts_with("error:"), "expected legacy error:, got {reply}");
    server.abort();
}

#[tokio::test]
async fn test_standalone_display_timeout_and_run_standalone_registration() {
    // StandaloneError Display & Error trait
    let e1 = StandaloneError::DaemonNotRunning;
    let e2 = StandaloneError::DaemonTerminated;
    let e3 = StandaloneError::Storage("disk full".into());
    assert!(e1.to_string().contains("not running"));
    assert!(e2.to_string().contains("terminated"));
    assert!(e3.to_string().contains("disk full"));
    let _: &dyn std::error::Error = &e1;

    // DaemonNotRunning timeout when no daemon registered
    let err = create_in_process_client()
        .await
        .expect_err("no daemon running must time out");
    assert!(matches!(err, StandaloneError::DaemonNotRunning));

    // run_standalone registers in ACTIVE_DAEMONS -> create_in_process_client succeeds
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let daemon_task = tokio::spawn(async move { run_standalone(shutdown_rx).await });
    let mut client = create_in_process_client()
        .await
        .expect("must connect to active standalone daemon");
    client.ping().await.expect("ping");
    let _ = shutdown_tx.send(());
    daemon_task.await.unwrap().expect("clean shutdown");

    // InProcessClient error paths via spawn_standalone_pair
    let (mut pair_client, pair_shutdown, pair_handle) = spawn_standalone_pair();
    let err_fork = pair_client
        .fork_thread(grein_core::model::NodeId::new(), grein_core::model::AnchorId::new())
        .await
        .expect_err("missing parent node must fail");
    assert!(matches!(err_fork, StandaloneError::Storage(_)));
    let _ = pair_shutdown.send(());
    pair_handle.await.unwrap().unwrap();
    assert!(matches!(
        pair_client.json_rpc("session.ping", json!({})).await,
        Err(StandaloneError::DaemonTerminated)
    ));
}

#[test]
fn test_cli_binary_doctor_settings_and_theme_error_exits() {
    let _lock = ENV_LOCK.blocking_lock();
    let temp = tempfile::tempdir().expect("tempdir");
    let corrupt_prefs = temp.path().join("prefs_dir");
    fs::create_dir_all(&corrupt_prefs).expect("mkdir");

    let bin = env!("CARGO_BIN_EXE_grein");
    let doc_out = Command::new(bin)
        .args(["doctor"])
        .env("GREIN_CONFIG_DB", &corrupt_prefs)
        .current_dir(temp.path())
        .output()
        .expect("run grein doctor");
    assert!(!doc_out.status.success());
    let stdout = String::from_utf8_lossy(&doc_out.stdout);
    assert!(stdout.contains("[ERR ]"));

    // Invalid stdin import exits non-zero
    let mut child = Command::new(bin)
        .args(["settings", "import", "-"])
        .env("GREIN_CONFIG_DB", temp.path().join("p.db"))
        .stdin(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");
    {
        use std::io::Write;
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(b"not = [valid\n")
            .unwrap();
    }
    let out = child.wait_with_output().unwrap();
    assert!(!out.status.success());
}
