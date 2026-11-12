use assert_cmd::Command;
use predicates::prelude::*;
use std::time::Duration;
use tempfile::tempdir;

#[test]
fn test_cli_help_displays_available_modes_and_subcommands() {
    let mut cmd = Command::cargo_bin("grein").expect("grein binary should exist");
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("daemon"))
        .stdout(predicate::str::contains("attach"));
}

#[test]
fn test_cli_version_flag_outputs_binary_information() {
    let mut cmd = Command::cargo_bin("grein").expect("grein binary should exist");
    cmd.arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("grein"));
}

#[test]
fn test_cli_daemon_help_displays_socket_configuration() {
    let mut cmd = Command::cargo_bin("grein").expect("grein binary should exist");
    cmd.args(["daemon", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("socket"));
}

#[test]
fn test_cli_attach_help_displays_socket_target() {
    let mut cmd = Command::cargo_bin("grein").expect("grein binary should exist");
    cmd.args(["attach", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("socket"));
}

#[test]
fn test_cli_daemon_mode_spawns_and_binds_socket_cleanly() {
    let temp_dir = tempdir().expect("temporary directory creation must succeed");
    let socket_path = temp_dir.path().join("test_grein.sock");
    let socket_str = socket_path.to_str().expect("valid utf-8 socket path");

    let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("grein"))
        .args(["daemon", "--socket", socket_str])
        .spawn()
        .expect("daemon process must spawn");

    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    let mut socket_bound = false;

    while std::time::Instant::now() < deadline {
        if socket_path.exists() {
            socket_bound = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let _ = child.kill();
    let _ = child.wait();

    assert!(
        socket_bound,
        "Daemon mode must bind Unix socket at {} without exiting prematurely",
        socket_str
    );
}
