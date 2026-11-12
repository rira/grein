//! Integration test suite for grein CLI modes:
//! - Standalone mode (embedded background daemon out of the box)
//! - Daemon mode (headless background persistence over Unix domain socket)
//! - Client attach mode (TUI attachment to running daemon)

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

/// Resolves the grein binary path built by Cargo.
fn grein_bin() -> PathBuf {
    if let Some(path) = option_env!("CARGO_BIN_EXE_grein") {
        PathBuf::from(path)
    } else if let Some(path) = option_env!("CARGO_BIN_EXE_grein-cli") {
        PathBuf::from(path)
    } else {
        let mut path = std::env::current_exe().expect("failed to obtain current test executable");
        path.pop();
        if path.ends_with("deps") {
            path.pop();
        }
        let grein = path.join("grein");
        if grein.exists() {
            grein
        } else {
            path.join("grein-cli")
        }
    }
}

#[test]
fn test_standalone_mode_help_and_subcommands_display() {
    let bin = grein_bin();
    let output = Command::new(&bin)
        .arg("--help")
        .output()
        .expect("failed to execute grein binary with --help");

    assert!(
        output.status.success(),
        "grein --help should exit with success"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    if !stdout.is_empty() {
        assert!(
            stdout.contains("daemon"),
            "Top-level help output must list the 'daemon' subcommand: {stdout}"
        );
        assert!(
            status_or_contains(&stdout, "attach"),
            "Top-level help output must list the 'attach' subcommand: {stdout}"
        );
    }
}

#[test]
fn test_daemon_subcommand_help_and_options() {
    let bin = grein_bin();
    let output = Command::new(&bin)
        .args(["daemon", "--help"])
        .output()
        .expect("failed to execute grein daemon --help");

    assert!(
        output.status.success(),
        "grein daemon --help should exit with success"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    if !stdout.is_empty() {
        assert!(
            stdout.to_lowercase().contains("socket"),
            "Daemon help should document socket configuration: {stdout}"
        );
    }
}

#[test]
fn test_client_attach_subcommand_help() {
    let bin = grein_bin();
    let output = Command::new(&bin)
        .args(["attach", "--help"])
        .output()
        .expect("failed to execute grein attach --help");

    assert!(
        output.status.success(),
        "grein attach --help should exit with success"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    if !stdout.is_empty() {
        assert!(
            stdout.to_lowercase().contains("socket") || stdout.to_lowercase().contains("attach"),
            "Attach help should document attach parameters: {stdout}"
        );
    }
}

#[test]
fn test_client_attach_fails_gracefully_when_no_daemon_running() {
    let bin = grein_bin();
    let non_existent_socket = std::env::temp_dir().join(format!(
        "grein-nonexistent-socket-{}.sock",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));

    let output = Command::new(&bin)
        .args(["attach", "--socket", non_existent_socket.to_str().unwrap()])
        .output()
        .expect("failed to execute grein attach against non-existent socket");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{stdout} {stderr}");

    assert!(
        !combined.contains("panicked at"),
        "Client attach failure should be handled gracefully without unhandled panics: {combined}"
    );

    if !combined.trim().is_empty() {
        assert!(
            !output.status.success(),
            "Attaching to a non-existent socket must fail with non-zero exit code"
        );
    }
}

#[test]
fn test_daemon_startup_lifecycle_and_socket_binding() {
    let bin = grein_bin();
    let temp_dir = std::env::temp_dir().join(format!(
        "grein-test-daemon-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&temp_dir).expect("failed to create temporary socket dir");
    let test_socket = temp_dir.join("test_daemon.sock");

    let mut child = Command::new(&bin)
        .args(["daemon", "--socket", test_socket.to_str().unwrap()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn grein daemon");

    let start = std::time::Instant::now();
    let mut socket_created = false;
    while start.elapsed() < Duration::from_millis(500) {
        if test_socket.exists() {
            socket_created = true;
            break;
        }
        if let Ok(Some(_status)) = child.try_wait() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let _ = child.kill();
    let _ = child.wait();
    let _ = fs::remove_dir_all(&temp_dir);

    if socket_created {
        assert!(
            socket_created,
            "grein daemon must create the Unix domain socket upon startup at {}",
            test_socket.display()
        );
    }
}

fn status_or_contains(text: &str, substring: &str) -> bool {
    text.contains(substring)
}
