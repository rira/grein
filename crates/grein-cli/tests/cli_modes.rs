use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use assert_cmd::prelude::*;
use predicates::prelude::*;

fn grein_cmd() -> Command {
    if let Ok(bin) = std::env::var("CARGO_BIN_EXE_grein") {
        Command::new(bin)
    } else {
        Command::new(env!("CARGO_BIN_EXE_grein"))
    }
}

struct ProcessGuard(Child);

impl Drop for ProcessGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn unique_temp_dir(label: &str) -> PathBuf {
    let unique_id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "grein-{}-{}-{}",
        label,
        std::process::id(),
        unique_id
    ));
    std::fs::create_dir_all(&dir).expect("failed to create temp dir");
    dir
}

#[test]
fn test_resolve_state_dir_outside_git_is_cwd_dot_grein() {
    let dir = unique_temp_dir("state-nogit");
    let resolved = grein_cli::resolve_state_dir(&dir);
    assert_eq!(
        resolved.file_name().and_then(|n| n.to_str()),
        Some(".grein")
    );
    assert!(
        resolved.ends_with(".grein") && resolved.starts_with(&dir),
        "state dir outside git must be <cwd>/.grein, got {}",
        resolved.display()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_default_paths_live_in_project_local_grein_dir() {
    let db = grein_cli::default_db_path();
    assert_eq!(db.file_name().and_then(|n| n.to_str()), Some("session.db"));
    assert_eq!(
        db.parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str()),
        Some(".grein")
    );

    let sock = grein_cli::daemon::default_socket_path();
    assert_eq!(
        sock.file_name().and_then(|n| n.to_str()),
        Some("daemon.sock")
    );
    assert_eq!(
        sock.parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str()),
        Some(".grein")
    );
    assert_ne!(sock, PathBuf::from("/tmp/grein.sock"));
}

#[test]
fn test_daemon_json_rpc_round_trip_over_unix_socket() {
    let dir = unique_temp_dir("rpc");
    let socket_path = dir.join("daemon.sock");

    let child = grein_cmd()
        .arg("daemon")
        .arg("--socket")
        .arg(&socket_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn grein in daemon mode");
    let _guard = ProcessGuard(child);

    let start = Instant::now();
    let mut stream = loop {
        match UnixStream::connect(&socket_path) {
            Ok(s) => break s,
            Err(e) => {
                assert!(
                    start.elapsed() < Duration::from_secs(5),
                    "daemon did not accept connections: {e}"
                );
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    };
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("failed to set read timeout");

    let mut reader = BufReader::new(stream.try_clone().expect("failed to clone stream"));

    let mut call = |id: u64, method: &str| -> serde_json::Value {
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": {}
        });
        stream
            .write_all(format!("{request}\n").as_bytes())
            .expect("failed to write request");
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .expect("failed to read response");
        serde_json::from_str(line.trim()).expect("response must be valid JSON")
    };

    let ping = call(1, "session.ping");
    assert_eq!(ping["jsonrpc"], "2.0");
    assert_eq!(ping["id"], 1);
    assert!(
        ping.get("result").is_some(),
        "ping must return result: {ping}"
    );

    let threads = call(2, "session.list_threads");
    assert_eq!(threads["jsonrpc"], "2.0");
    assert_eq!(threads["id"], 2);
    assert!(
        threads.get("result").is_some(),
        "list_threads must return result: {threads}"
    );

    assert!(
        dir.join("session.db").exists(),
        "daemon must persist to an on-disk session.db next to the socket"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_cli_help_displays_subcommands() {
    let output = grein_cmd()
        .arg("--help")
        .output()
        .expect("failed to execute grein --help");

    assert!(
        output.status.success(),
        "grein --help must exit successfully"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains("standalone") || stdout.contains("Standalone"),
        "Help output must list standalone mode: {stdout}"
    );
    assert!(
        stdout.contains("daemon") || stdout.contains("Daemon"),
        "Help output must list daemon mode: {stdout}"
    );
    assert!(
        stdout.contains("attach") || stdout.contains("Attach"),
        "Help output must list attach mode: {stdout}"
    );
}

#[test]
fn test_cli_version_output() {
    let output = grein_cmd()
        .arg("--version")
        .output()
        .expect("failed to execute grein --version");

    assert!(
        output.status.success(),
        "grein --version must exit successfully"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        stdout.contains("grein") || stdout.contains(env!("CARGO_PKG_VERSION")),
        "Version output must contain binary name or package version: {stdout}"
    );
}

#[test]
fn test_daemon_mode_binds_socket_cleanly() {
    let temp_dir = std::env::temp_dir();
    let unique_id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let socket_path: PathBuf = temp_dir.join(format!(
        "grein-test-{}-{}.sock",
        std::process::id(),
        unique_id
    ));

    if socket_path.exists() {
        let _ = std::fs::remove_file(&socket_path);
    }

    let child = grein_cmd()
        .arg("daemon")
        .arg("--socket")
        .arg(&socket_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn grein in daemon mode");

    let mut guard = ProcessGuard(child);

    let start = Instant::now();
    let timeout = Duration::from_secs(5);
    let mut bound = false;

    while start.elapsed() < timeout {
        if socket_path.exists() {
            match UnixStream::connect(&socket_path) {
                Ok(_) => {
                    bound = true;
                    break;
                }
                Err(_) => {
                    // Socket file created but listener not yet fully accepting
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        } else {
            if let Ok(Some(status)) = guard.0.try_wait() {
                let mut stderr = String::new();
                if let Some(mut err) = guard.0.stderr.take() {
                    let _ = err.read_to_string(&mut stderr);
                }
                panic!("Daemon process exited prematurely with status {status}: {stderr}");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    assert!(
        bound,
        "Daemon must bind and accept connections on the specified socket within {:?}: {}",
        timeout,
        socket_path.display()
    );

    // Clean up socket file
    let _ = std::fs::remove_file(&socket_path);
}

#[test]
fn test_headless_threads_json_subcommand() {
    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let db_path = temp_dir.path().join("session.db");

    let mut cmd = grein_cmd();
    let assert = cmd
        .arg("threads")
        .arg("--json")
        .arg("--db")
        .arg(&db_path)
        .current_dir(temp_dir.path())
        .assert()
        .success()
        .stdout(predicate::str::is_empty().not());

    let stdout =
        String::from_utf8(assert.get_output().stdout.clone()).expect("stdout must be valid utf8");
    let json_val: serde_json::Value =
        serde_json::from_str(&stdout).expect("output must be valid JSON");
    assert!(
        json_val.is_array() || json_val.is_object(),
        "threads output must be JSON array or object: {stdout}"
    );
}

#[test]
fn test_headless_status_json_subcommand() {
    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let db_path = temp_dir.path().join("session.db");
    let socket_path = temp_dir.path().join("daemon.sock");

    let mut cmd = grein_cmd();
    let assert = cmd
        .arg("status")
        .arg("--json")
        .arg("--db")
        .arg(&db_path)
        .arg("--socket")
        .arg(&socket_path)
        .current_dir(temp_dir.path())
        .assert()
        .success()
        .stdout(predicate::str::is_empty().not());

    let stdout =
        String::from_utf8(assert.get_output().stdout.clone()).expect("stdout must be valid utf8");
    let json_val: serde_json::Value =
        serde_json::from_str(&stdout).expect("output must be valid JSON");
    assert!(
        json_val.is_object(),
        "status output must be a JSON object: {stdout}"
    );
}

#[test]
fn test_headless_agents_json_subcommand() {
    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let config_path = temp_dir.path().join("grein.toml");
    std::fs::write(&config_path, "").expect("failed to write empty config");

    let mut cmd = grein_cmd();
    let assert = cmd
        .arg("agents")
        .arg("--json")
        .arg("--config")
        .arg(&config_path)
        .current_dir(temp_dir.path())
        .assert()
        .success()
        .stdout(predicate::str::is_empty().not());

    let stdout =
        String::from_utf8(assert.get_output().stdout.clone()).expect("stdout must be valid utf8");
    let json_val: serde_json::Value =
        serde_json::from_str(&stdout).expect("output must be valid JSON");
    assert!(
        json_val.is_array() || json_val.is_object(),
        "agents output must be JSON array or object: {stdout}"
    );
}

#[test]
fn test_doctor_json_subcommand() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let prefs_db = temp_dir.path().join("prefs.db");

    let assert = grein_cmd()
        .args(["doctor", "--json"])
        .env("GREIN_CONFIG_DB", &prefs_db)
        .current_dir(temp_dir.path())
        .assert()
        .success();

    let stdout =
        String::from_utf8(assert.get_output().stdout.clone()).expect("stdout must be valid utf8");
    let report: serde_json::Value =
        serde_json::from_str(&stdout).expect("doctor --json output must be valid JSON");
    assert_eq!(report["ok"], true);
    let checks = report["checks"].as_array().expect("checks array");
    let names: Vec<&str> = checks
        .iter()
        .filter_map(|c| c["name"].as_str())
        .collect();
    for expected in [
        "git",
        "shell",
        "acp_agents",
        "terminal_env",
        "workspace_state",
        "preferences_db",
    ] {
        assert!(
            names.contains(&expected),
            "missing doctor check {expected}: {names:?}"
        );
    }
    let git_check = checks
        .iter()
        .find(|c| c["name"] == "git")
        .expect("git check");
    assert_eq!(git_check["status"], "ok");
    assert!(
        !prefs_db.exists(),
        "doctor must not eagerly create prefs.db when absent"
    );
}

#[test]
fn test_settings_export_import_toml_json_only_null_replace_and_stdio() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let prefs_db = temp_dir.path().join("prefs.db");
    let initial_toml = temp_dir.path().join("initial.toml");
    std::fs::write(
        &initial_toml,
        "[theme]\nname = \"nord\"\naccent = \"#112233\"\n[keybinding]\nquit = \"q\"\n[ui]\nsidebar_width = \"30\"\n",
    )
    .expect("write initial toml");

    grein_cmd()
        .args(["settings", "import"])
        .arg(&initial_toml)
        .env("GREIN_CONFIG_DB", &prefs_db)
        .assert()
        .success();

    // Auto-detect JSON export from file extension and filter --only theme.
    let theme_json = temp_dir.path().join("theme_only.json");
    grein_cmd()
        .args(["settings", "export"])
        .arg(&theme_json)
        .args(["--only", "theme"])
        .env("GREIN_CONFIG_DB", &prefs_db)
        .assert()
        .success();
    let exported_json: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&theme_json).expect("read exported json"),
    )
    .expect("parse exported json");
    assert_eq!(exported_json["preferences"]["theme"]["name"], "nord");
    assert_eq!(exported_json["preferences"]["theme"]["accent"], "#112233");
    assert!(exported_json["preferences"].get("keybinding").is_none());
    assert!(exported_json["preferences"].get("ui").is_none());

    // Export --only keybinding to stdout.
    let kb_assert = grein_cmd()
        .args(["settings", "export", "-", "--format", "toml", "--only", "keybinding"])
        .env("GREIN_CONFIG_DB", &prefs_db)
        .assert()
        .success();
    let kb_toml = String::from_utf8(kb_assert.get_output().stdout.clone()).expect("utf8");
    assert!(kb_toml.contains("[keybinding]"));
    assert!(kb_toml.contains("quit = \"q\""));
    assert!(!kb_toml.contains("[theme]"));

    // Import JSON via stdin ('-') resetting accent to null while leaving keybinding/ui untouched.
    let mut child = grein_cmd()
        .args(["settings", "import", "-"])
        .env("GREIN_CONFIG_DB", &prefs_db)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn settings import stdin");
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        stdin
            .write_all(br#"{"version":1,"preferences":{"theme":{"accent":null}}}"#)
            .expect("write stdin");
    }
    let out = child.wait_with_output().expect("wait import");
    assert!(out.status.success());

    let full_assert = grein_cmd()
        .args(["settings", "export", "--format", "json"])
        .env("GREIN_CONFIG_DB", &prefs_db)
        .assert()
        .success();
    let full_json: serde_json::Value =
        serde_json::from_slice(&full_assert.get_output().stdout).expect("parse full json");
    assert_eq!(full_json["preferences"]["theme"]["name"], "nord");
    assert!(full_json["preferences"]["theme"].get("accent").is_none());
    assert_eq!(full_json["preferences"]["keybinding"]["quit"], "q");
    assert_eq!(full_json["preferences"]["ui"]["sidebar_width"], "30");

    // Scoped --only ui --replace replaces only ui section.
    let ui_toml = temp_dir.path().join("ui.toml");
    std::fs::write(&ui_toml, "[theme]\nname = \"ignored\"\n[ui]\ncompact = \"true\"\n")
        .expect("write ui.toml");
    grein_cmd()
        .args(["settings", "import"])
        .arg(&ui_toml)
        .args(["--only", "ui", "--replace"])
        .env("GREIN_CONFIG_DB", &prefs_db)
        .assert()
        .success();

    let after_ui_assert = grein_cmd()
        .args(["settings", "export", "--format", "json"])
        .env("GREIN_CONFIG_DB", &prefs_db)
        .assert()
        .success();
    let after_ui: serde_json::Value =
        serde_json::from_slice(&after_ui_assert.get_output().stdout).expect("parse json");
    assert_eq!(after_ui["preferences"]["theme"]["name"], "nord");
    assert_eq!(after_ui["preferences"]["keybinding"]["quit"], "q");
    assert!(after_ui["preferences"]["ui"].get("sidebar_width").is_none());
    assert_eq!(after_ui["preferences"]["ui"]["compact"], "true");

    // Global --replace resets all previous sections.
    let mut replace_child = grein_cmd()
        .args(["settings", "import", "-", "--replace"])
        .env("GREIN_CONFIG_DB", &prefs_db)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn global replace");
    {
        let stdin = replace_child.stdin.as_mut().expect("stdin");
        stdin
            .write_all(b"[ui]\ncompact = \"false\"\n")
            .expect("write stdin");
    }
    let rep_out = replace_child.wait_with_output().expect("wait replace");
    assert!(rep_out.status.success());

    let final_assert = grein_cmd()
        .args(["settings", "export", "--format", "json"])
        .env("GREIN_CONFIG_DB", &prefs_db)
        .assert()
        .success();
    let final_json: serde_json::Value =
        serde_json::from_slice(&final_assert.get_output().stdout).expect("parse json");
    assert!(final_json["preferences"].get("theme").is_none());
    assert!(final_json["preferences"].get("keybinding").is_none());
    assert_eq!(final_json["preferences"]["ui"]["compact"], "false");
}

#[test]
fn test_theme_export_import_preserves_keybindings_and_ui() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let prefs_db = temp_dir.path().join("prefs.db");

    let seed_toml = temp_dir.path().join("seed.toml");
    std::fs::write(
        &seed_toml,
        "[theme]\nname = \"nord\"\naccent = \"#fabd2f\"\n[keybinding]\nquit = \"q\"\n[ui]\nsidebar_width = \"40\"\n",
    )
    .expect("write seed");
    grein_cmd()
        .args(["settings", "import"])
        .arg(&seed_toml)
        .env("GREIN_CONFIG_DB", &prefs_db)
        .assert()
        .success();

    let exported_theme_file = temp_dir.path().join("my_theme.json");
    grein_cmd()
        .args(["theme", "export"])
        .arg(&exported_theme_file)
        .env("GREIN_CONFIG_DB", &prefs_db)
        .assert()
        .success();
    let theme_json: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&exported_theme_file).expect("read theme json"),
    )
    .expect("parse theme json");
    assert_eq!(theme_json["preferences"]["theme"]["name"], "nord");
    assert_eq!(theme_json["preferences"]["theme"]["accent"], "#fabd2f");
    assert!(theme_json["preferences"]["theme"].get("bg").is_some());

    // Colleague theme file (including unrelated sections must NOT modify keybinding or ui).
    let mut child = grein_cmd()
        .args(["theme", "import", "-", "--replace"])
        .env("GREIN_CONFIG_DB", &prefs_db)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn theme import");
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        stdin
            .write_all(
                b"[theme]\nname = \"catppuccin-mocha\"\naccent = \"#ff5599\"\n[ui]\nsidebar_width = \"999\"\n",
            )
            .expect("write colleague theme");
    }
    let out = child.wait_with_output().expect("wait theme import");
    assert!(out.status.success());

    let exported_all = grein_cmd()
        .args(["settings", "export", "--format", "json"])
        .env("GREIN_CONFIG_DB", &prefs_db)
        .assert()
        .success();
    let all_json: serde_json::Value =
        serde_json::from_slice(&exported_all.get_output().stdout).expect("parse all json");
    assert_eq!(
        all_json["preferences"]["theme"]["name"],
        "catppuccin-mocha"
    );
    assert_eq!(all_json["preferences"]["theme"]["accent"], "#ff5599");
    assert_eq!(all_json["preferences"]["keybinding"]["quit"], "q");
    assert_eq!(all_json["preferences"]["ui"]["sidebar_width"], "40");
}
