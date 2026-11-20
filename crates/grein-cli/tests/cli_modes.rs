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
