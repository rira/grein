use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn test_standalone_command_wires_to_tui_runner_not_stub() {
    let mut cmd = Command::cargo_bin("grein")
        .or_else(|_| Command::cargo_bin("grein-cli"))
        .expect("grein binary should be available");

    cmd.arg("standalone");

    // The previous stub implementation printed "Grein Standalone mode" and exited immediately.
    // Once wired to grein_tui::run().await, the stub message is no longer emitted.
    cmd.assert()
        .stdout(predicate::str::contains("Grein Standalone mode").not());
}

#[test]
fn test_default_execution_wires_to_tui_runner_not_stub() {
    let mut cmd = Command::cargo_bin("grein")
        .or_else(|_| Command::cargo_bin("grein-cli"))
        .expect("grein binary should be available");

    // When invoked with no subcommand arguments, default execution routes to standalone TUI,
    // not the static stub message.
    cmd.assert()
        .stdout(predicate::str::contains("Grein Standalone mode").not());
}
