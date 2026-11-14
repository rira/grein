use std::process::Command;
use std::time::Duration;

#[test]
fn test_cli_help_flag_outputs_usage() {
    let output = Command::new(env!("CARGO_BIN_EXE_grein"))
        .arg("--help")
        .output()
        .expect("Failed to execute grein binary");

    assert!(output.status.success(), "Expected --help to succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Usage:") || stdout.contains("grein"),
        "Expected usage output, got: {stdout}"
    );
}

#[test]
fn test_cli_version_flag_outputs_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_grein"))
        .arg("--version")
        .output()
        .expect("Failed to execute grein binary");

    assert!(output.status.success(), "Expected --version to succeed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("grein"),
        "Expected version output containing 'grein', got: {stdout}"
    );
}

#[test]
fn test_cli_standalone_subcommand_exits_cleanly_in_headless_mode() {
    let output = Command::new(env!("CARGO_BIN_EXE_grein"))
        .arg("standalone")
        .output()
        .expect("Failed to execute grein standalone");

    assert!(
        output.status.success(),
        "Expected standalone subcommand in non-TTY pipe to exit 0, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_cli_default_invocation_exits_cleanly_in_headless_mode() {
    let output = Command::new(env!("CARGO_BIN_EXE_grein"))
        .output()
        .expect("Failed to execute grein default");

    assert!(
        output.status.success(),
        "Expected default invocation in non-TTY pipe to exit 0, stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
async fn test_cli_daemon_and_attach_subcommands_over_socket() {
    let temp_dir = tempfile::tempdir().expect("Failed to create tempdir");
    let socket_path = temp_dir.path().join("test-grein-main.sock");

    let mut daemon_child = Command::new(env!("CARGO_BIN_EXE_grein"))
        .arg("daemon")
        .arg("--socket")
        .arg(&socket_path)
        .spawn()
        .expect("Failed to spawn grein daemon");

    let mut connected = false;
    for _ in 0..40 {
        if socket_path.exists() {
            connected = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(connected, "Daemon socket was not created in time");

    let attach_output = Command::new(env!("CARGO_BIN_EXE_grein"))
        .arg("attach")
        .arg("--socket")
        .arg(&socket_path)
        .output()
        .expect("Failed to run grein attach");

    let _ = daemon_child.kill();
    let _ = daemon_child.wait();

    assert!(
        attach_output.status.success(),
        "Expected attach to succeed in non-TTY mode, stderr: {}",
        String::from_utf8_lossy(&attach_output.stderr)
    );
}
