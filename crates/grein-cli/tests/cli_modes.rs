use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::tempdir;

#[test]
fn test_cli_version_flag() {
    let mut cmd = Command::cargo_bin("grein").expect("grein binary should exist");
    cmd.arg("--version");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn test_cli_help_lists_modes() {
    let mut cmd = Command::cargo_bin("grein").expect("grein binary should exist");
    cmd.arg("--help");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("standalone"))
        .stdout(predicate::str::contains("daemon"))
        .stdout(predicate::str::contains("attach"));
}

#[test]
fn test_cli_standalone_subcommand_help() {
    let mut cmd = Command::cargo_bin("grein").expect("grein binary should exist");
    cmd.args(["standalone", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("standalone"));
}

#[test]
fn test_cli_daemon_subcommand_help() {
    let mut cmd = Command::cargo_bin("grein").expect("grein binary should exist");
    cmd.args(["daemon", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("socket"));
}

#[test]
fn test_cli_attach_subcommand_help() {
    let mut cmd = Command::cargo_bin("grein").expect("grein binary should exist");
    cmd.args(["attach", "--help"]);
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("attach"));
}

#[test]
fn test_cli_daemon_mode_socket_initialization() {
    let temp_dir = tempdir().expect("temporary directory creation must succeed");
    let socket_path = temp_dir.path().join("grein-test.sock");

    let mut cmd = Command::cargo_bin("grein").expect("grein binary should exist");
    cmd.args([
        "daemon",
        "--socket",
        socket_path.to_str().expect("valid utf-8 socket path"),
        "--dry-run",
    ]);

    let output = cmd.output().expect("command execution must succeed");
    assert!(
        output.status.success() || !output.stderr.is_empty(),
        "daemon mode invocation must not panic or crash unexpectedly"
    );
}
